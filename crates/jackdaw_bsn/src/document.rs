//! Editor document model for the `.bsn` scene format.
//!
//! This is a flat, string-typed AST that the editor mutates and later emits
//! back to `.bsn` text. It is a distinct model from the parser AST in
//! [`crate::parse`]: the parser AST captures the exact parsed structure, while
//! this document AST is the editor's stable working representation. The loader
//! bridges the two.
//!
//! Nodes live as entities in a private [`World`] held by [`SceneBsnAst`].

use std::path::Path;

use bevy::asset::{AssetServer, ReflectHandle};
use bevy::ecs::entity::Entity;
use bevy::ecs::prelude::{Component, Resource};
use bevy::ecs::world::World;
use bevy::platform::collections::HashMap;
use bevy::reflect::enums::VariantType;
use bevy::reflect::{PartialReflect, ReflectRef, TypeRegistry};

/// Check if `stored_path` is an enum variant of `base_path`.
/// e.g. `foo::Bar::Sphere` is a variant of `foo::Bar`.
fn is_enum_variant_of(stored_path: &str, base_path: &str) -> bool {
    stored_path.starts_with(base_path)
        && stored_path.as_bytes().get(base_path.len()) == Some(&b':')
        && stored_path[base_path.len()..].starts_with("::")
        && !stored_path[base_path.len() + 2..].contains("::")
}

/// A list of patches that together define one BSN entity.
/// Each child entity has a [`BsnPatch`] component.
#[derive(Component)]
pub struct BsnPatches(pub Vec<Entity>);

/// A single patch within a [`BsnPatches`] list.
#[derive(Component, Clone)]
pub enum BsnPatch {
    /// `#Name` entity name reference.
    Name(String),
    /// `:"path.bsn"` base scene inheritance.
    Base(String),
    /// `type::Path` a bare type (unit struct or enum variant, all defaults).
    Type(String),
    /// `type::Path { field: value, ... }` struct with field overrides.
    Struct(BsnStructData),
    /// `type::Path(value, ...)` tuple struct with positional values.
    TupleStruct(BsnTupleStructData),
    /// `@type::Path { ... }` template type.
    Template(String, Option<BsnStructFields>),
    /// `Children [...]` child entity relation.
    Children(Vec<Entity>),
}

/// Fields of a BSN struct patch: `TypePath { field: expr, ... }`.
#[derive(Clone)]
pub struct BsnStructData {
    pub type_path: String,
    pub fields: BsnStructFields,
}

/// Ordered list of named fields.
#[derive(Clone, Default)]
pub struct BsnStructFields(pub Vec<BsnField>);

/// A single `name: value` field.
#[derive(Clone)]
pub struct BsnField {
    pub name: String,
    pub value: BsnValue,
}

/// Tuple struct data: `TypePath(value, ...)`.
#[derive(Clone)]
pub struct BsnTupleStructData {
    pub type_path: String,
    pub values: Vec<BsnValue>,
}

/// A BSN expression value (the right-hand side of a field or tuple element).
#[derive(Clone)]
pub enum BsnValue {
    /// `1.0`
    Float(f64),
    /// `42`
    Int(i128),
    /// `true`
    Bool(bool),
    /// `"string"`
    String(String),
    /// `type::Path` unit type or enum variant.
    Type(String),
    /// `type::Path { field: value }` nested struct.
    Struct(BsnStructData),
    /// `type::Path(value)` nested tuple struct.
    TupleStruct(BsnTupleStructData),
    /// `[value, value, ...]` list/vec.
    List(Vec<BsnValue>),
    /// `map[(key, value), ...]` map/HashMap.
    Map(Vec<(BsnValue, BsnValue)>),
}

/// Resource holding the BSN AST for the currently loaded scene.
/// The AST is stored as entities in a separate [`World`].
#[derive(Resource, Default)]
pub struct SceneBsnAst {
    /// The AST world; each entity is an AST node.
    pub world: World,
    /// Root-level entity patches (top-level entities in the scene).
    pub roots: Vec<Entity>,
    /// Maps ECS scene entities to AST patches entities.
    pub ecs_to_ast: HashMap<Entity, Entity>,
    /// Maps AST patches entities to ECS scene entities (reverse of above).
    pub ast_to_ecs: HashMap<Entity, Entity>,
}

/// Component types on a document node that were computed by editor systems
/// rather than authored by the user. Derived components are skipped on save;
/// an explicit user edit promotes the component to authored.
#[derive(Component, Default)]
pub struct DerivedComponents(pub bevy::platform::collections::HashSet<String>);

/// Component on every ECS entity that was spawned from (or synced to) BSN.
/// Points back to the AST node entity in [`SceneBsnAst::world`].
#[derive(Component)]
pub struct AstNodeRef {
    pub patches_entity: Entity,
}

impl SceneBsnAst {
    /// Create a new AST node for an entity with the given patches.
    pub fn create_entity_node(&mut self, patches: Vec<BsnPatch>) -> Entity {
        let patch_entities: Vec<Entity> = patches
            .into_iter()
            .map(|patch| self.world.spawn(patch).id())
            .collect();
        self.world.spawn(BsnPatches(patch_entities)).id()
    }

    /// Add a patches entity to the root list.
    pub fn add_to_roots(&mut self, patches_entity: Entity) {
        self.roots.push(patches_entity);
    }

    /// Remove a patches entity from the root list.
    pub fn remove_from_roots(&mut self, patches_entity: Entity) {
        self.roots.retain(|&e| e != patches_entity);
    }

    /// Register an ECS entity to AST node mapping (both directions).
    pub fn link(&mut self, ecs_entity: Entity, ast_entity: Entity) {
        self.ecs_to_ast.insert(ecs_entity, ast_entity);
        self.ast_to_ecs.insert(ast_entity, ecs_entity);
    }

    /// Remove an ECS entity mapping.
    pub fn unlink(&mut self, ecs_entity: Entity) {
        if let Some(ast_entity) = self.ecs_to_ast.remove(&ecs_entity) {
            self.ast_to_ecs.remove(&ast_entity);
        }
    }

    /// Remove the mapping for an AST node, clearing both directions. The
    /// counterpart to [`unlink`](Self::unlink) keyed by the AST node so a
    /// subtree teardown can drop every descendant's link, not just the top.
    fn unlink_ast(&mut self, ast_entity: Entity) {
        if let Some(ecs_entity) = self.ast_to_ecs.remove(&ast_entity) {
            self.ecs_to_ast.remove(&ecs_entity);
        }
    }

    /// Get the AST entity for an ECS entity.
    pub fn ast_for(&self, ecs_entity: Entity) -> Option<Entity> {
        self.ecs_to_ast.get(&ecs_entity).copied()
    }

    /// Get the ECS entity for an AST patches entity.
    pub fn ecs_for_ast(&self, ast_entity: Entity) -> Option<Entity> {
        self.ast_to_ecs.get(&ast_entity).copied()
    }

    /// Get the patches for an AST entity.
    pub fn get_patches(&self, patches_entity: Entity) -> Option<&BsnPatches> {
        self.world.get::<BsnPatches>(patches_entity)
    }

    /// Get a mutable reference to patches for an AST entity.
    pub fn get_patches_mut(&mut self, patches_entity: Entity) -> Option<&mut BsnPatches> {
        self.world
            .get_mut::<BsnPatches>(patches_entity)
            .map(bevy::ecs::change_detection::Mut::into_inner)
    }

    /// Get a specific patch component.
    pub fn get_patch(&self, patch_entity: Entity) -> Option<&BsnPatch> {
        self.world.get::<BsnPatch>(patch_entity)
    }

    /// The component type paths authored on `patches_entity`. Skips the
    /// Children relation, base inheritance, and name references, which are
    /// not components.
    pub fn component_type_paths(&self, patches_entity: Entity) -> Vec<String> {
        let Some(patches) = self.get_patches(patches_entity) else {
            return Vec::new();
        };
        patches
            .0
            .iter()
            .filter_map(|&patch_entity| {
                self.get_patch(patch_entity).and_then(|patch| match patch {
                    BsnPatch::Type(tp) => Some(tp.clone()),
                    BsnPatch::Struct(data) => Some(data.type_path.clone()),
                    BsnPatch::TupleStruct(data) => Some(data.type_path.clone()),
                    BsnPatch::Template(tp, _) => Some(tp.clone()),
                    _ => None,
                })
            })
            .collect()
    }

    /// Get the [`BsnPatch::Name`] value for an AST entity, if present.
    pub fn get_name(&self, patches_entity: Entity) -> Option<&str> {
        let patches = self.get_patches(patches_entity)?;
        for &pe in &patches.0 {
            if let Some(BsnPatch::Name(name)) = self.get_patch(pe) {
                return Some(name.as_str());
            }
        }
        None
    }

    /// Whether `type_path` is marked derived (computed, not authored) on this
    /// node.
    pub fn is_derived(&self, patches_entity: Entity, type_path: &str) -> bool {
        self.world
            .get::<DerivedComponents>(patches_entity)
            .is_some_and(|d| d.0.contains(type_path))
    }

    /// Clear the derived mark on `type_path` (a user edit makes it authored).
    /// Returns true when the component was previously derived.
    pub fn promote_derived(&mut self, patches_entity: Entity, type_path: &str) -> bool {
        self.world
            .get_mut::<DerivedComponents>(patches_entity)
            .map(|mut d| d.0.remove(type_path))
            .unwrap_or(false)
    }

    /// Mark `type_path` as derived on this node.
    pub fn demote_to_derived(&mut self, patches_entity: Entity, type_path: &str) {
        if let Some(mut d) = self.world.get_mut::<DerivedComponents>(patches_entity) {
            d.0.insert(type_path.to_string());
            return;
        }
        let mut set = bevy::platform::collections::HashSet::default();
        set.insert(type_path.to_string());
        if let Ok(mut node) = self.world.get_entity_mut(patches_entity) {
            node.insert(DerivedComponents(set));
        }
    }

    /// Remove the component patch for `type_path` from a node (the undo of
    /// authoring a component that did not exist before). The patch entity is
    /// despawned and dropped from the node's patch list.
    pub fn remove_component_patch(&mut self, patches_entity: Entity, type_path: &str) {
        let Some(patch_entity) = self.find_patch_by_type_path(patches_entity, type_path) else {
            return;
        };
        if let Some(patches) = self.get_patches_mut(patches_entity) {
            patches.0.retain(|&pe| pe != patch_entity);
        }
        self.world.despawn(patch_entity);
    }

    /// Find the document node carrying the given stable node id, i.e. a
    /// `SceneNodeId(id)` tuple-struct patch. Linear over nodes; the stable id
    /// is the cross-process identity used by the play-in-editor mapping.
    pub fn node_by_stable_id(&self, id: u64) -> Option<Entity> {
        let mut found = None;
        let mut nodes: Vec<Entity> = self.roots.clone();
        while let Some(node) = nodes.pop() {
            if let Some(patches) = self.get_patches(node) {
                for &pe in &patches.0 {
                    match self.get_patch(pe) {
                        Some(BsnPatch::TupleStruct(data))
                            if data.type_path.ends_with("SceneNodeId")
                                && matches!(
                                    data.values.first(),
                                    Some(BsnValue::Int(v)) if *v == i128::from(id)
                                ) =>
                        {
                            found = Some(node);
                        }
                        Some(BsnPatch::Children(children)) => {
                            nodes.extend(children.iter().copied());
                        }
                        _ => {}
                    }
                }
            }
            if found.is_some() {
                break;
            }
        }
        found
    }

    /// The live ECS entity for the node carrying the given stable node id.
    pub fn entity_for_stable_id(&self, id: u64) -> Option<Entity> {
        self.node_by_stable_id(id)
            .and_then(|node| self.ecs_for_ast(node))
    }

    /// Get child AST entities from [`BsnPatch::Children`], if present.
    pub fn get_children_ast(&self, patches_entity: Entity) -> Vec<Entity> {
        let Some(patches) = self.get_patches(patches_entity) else {
            return Vec::new();
        };
        for &pe in &patches.0 {
            if let Some(BsnPatch::Children(children)) = self.get_patch(pe) {
                return children.clone();
            }
        }
        Vec::new()
    }

    /// Replace a patch component on an existing entity.
    pub fn set_patch(&mut self, patch_entity: Entity, patch: BsnPatch) {
        if let Ok(mut entity_mut) = self.world.get_entity_mut(patch_entity) {
            entity_mut.insert(patch);
        }
    }

    /// Find the patch of a given type within an entity's patches list.
    /// Returns the patch entity.
    pub fn find_patch_by_type_path(
        &self,
        patches_entity: Entity,
        type_path: &str,
    ) -> Option<Entity> {
        let patches = self.get_patches(patches_entity)?;
        for &patch_entity in &patches.0 {
            if let Some(patch) = self.get_patch(patch_entity) {
                let matches = match patch {
                    BsnPatch::Type(tp) => tp == type_path || is_enum_variant_of(tp, type_path),
                    BsnPatch::Struct(data) => {
                        data.type_path == type_path
                            || is_enum_variant_of(&data.type_path, type_path)
                    }
                    BsnPatch::TupleStruct(data) => {
                        data.type_path == type_path
                            || is_enum_variant_of(&data.type_path, type_path)
                    }
                    BsnPatch::Template(tp, _) => tp == type_path,
                    _ => false,
                };
                if matches {
                    return Some(patch_entity);
                }
            }
        }
        None
    }

    /// Update or insert a struct patch for a given type path within an entity's
    /// patches list. If no patch for that type exists, creates one. Returns the
    /// patch entity.
    pub fn upsert_struct_patch(
        &mut self,
        patches_entity: Entity,
        type_path: &str,
        fields: BsnStructFields,
    ) -> Entity {
        if let Some(existing) = self.find_patch_by_type_path(patches_entity, type_path) {
            self.set_patch(
                existing,
                BsnPatch::Struct(BsnStructData {
                    type_path: type_path.to_string(),
                    fields,
                }),
            );
            return existing;
        }

        let patch_entity = self
            .world
            .spawn(BsnPatch::Struct(BsnStructData {
                type_path: type_path.to_string(),
                fields,
            }))
            .id();

        if let Some(patches) = self.get_patches_mut(patches_entity) {
            patches.0.push(patch_entity);
        }

        patch_entity
    }

    /// Move an AST node from one parent's Children to another.
    pub fn move_to_parent(
        &mut self,
        node: Entity,
        old_parent: Option<Entity>,
        new_parent: Option<Entity>,
    ) {
        if let Some(old_parent_ast) = old_parent {
            self.remove_child_from_ast(old_parent_ast, node);
        } else {
            self.remove_from_roots(node);
        }

        if let Some(new_parent_ast) = new_parent {
            self.add_child_to_ast(new_parent_ast, node);
        } else {
            self.add_to_roots(node);
        }
    }

    /// Remove a child from a parent's Children patch.
    pub fn remove_child_from_ast(&mut self, parent_ast: Entity, child_ast: Entity) {
        let Some(patches) = self.get_patches(parent_ast) else {
            return;
        };
        let patch_ids: Vec<Entity> = patches.0.clone();

        for &patch_entity in &patch_ids {
            if let Some(patch) = self.world.get_mut::<BsnPatch>(patch_entity)
                && let BsnPatch::Children(children) = patch.into_inner()
            {
                children.retain(|&e| e != child_ast);
                return;
            }
        }
    }

    /// Remove an entity's AST node entirely: detach from parent (or roots),
    /// recursively despawn all AST sub-entities, and unlink the ECS mapping for
    /// the node and every linked descendant.
    ///
    /// No-ops gracefully if the entity is not in `ecs_to_ast`.
    pub fn remove_entity_node(&mut self, ecs_entity: Entity) {
        let Some(node_ast) = self.ecs_to_ast.get(&ecs_entity).copied() else {
            return;
        };

        if let Some(parent_ast) = self.find_ast_parent_of(node_ast) {
            self.remove_child_from_ast(parent_ast, node_ast);
        } else {
            self.remove_from_roots(node_ast);
        }

        self.despawn_recursive(node_ast);
    }

    /// Find which AST entity contains `child_ast` in a Children patch.
    /// Returns `None` if `child_ast` is a root (or not found).
    fn find_ast_parent_of(&self, child_ast: Entity) -> Option<Entity> {
        if self.roots.contains(&child_ast) {
            return None;
        }
        for &root in &self.roots {
            if let Some(parent) = self.find_parent_in_subtree(root, child_ast) {
                return Some(parent);
            }
        }
        None
    }

    fn find_parent_in_subtree(&self, current: Entity, target: Entity) -> Option<Entity> {
        let patches = self.get_patches(current)?;
        for &patch_entity in &patches.0 {
            if let Some(BsnPatch::Children(children)) = self.get_patch(patch_entity) {
                if children.contains(&target) {
                    return Some(current);
                }
                for &child in children {
                    if let Some(parent) = self.find_parent_in_subtree(child, target) {
                        return Some(parent);
                    }
                }
            }
        }
        None
    }

    /// Recursively despawn an AST node and all its children/patches, dropping
    /// the ECS link for every node torn down (not just the top) so no stale
    /// `ecs_to_ast`/`ast_to_ecs` entries survive for a linked descendant.
    fn despawn_recursive(&mut self, node: Entity) {
        // Drop this node's link before its entity id is despawned and possibly
        // recycled for an unrelated AST node.
        self.unlink_ast(node);

        let children: Vec<Entity> = if let Some(patches) = self.get_patches(node) {
            patches
                .0
                .iter()
                .filter_map(|&pe| {
                    if let Some(BsnPatch::Children(child_list)) = self.get_patch(pe) {
                        Some(child_list.clone())
                    } else {
                        None
                    }
                })
                .flatten()
                .collect()
        } else {
            Vec::new()
        };

        for child in children {
            self.despawn_recursive(child);
        }

        if let Some(patches) = self.get_patches(node) {
            let patch_ids: Vec<Entity> = patches.0.clone();
            for pe in patch_ids {
                if let Ok(em) = self.world.get_entity_mut(pe) {
                    em.despawn();
                }
            }
        }
        if let Ok(em) = self.world.get_entity_mut(node) {
            em.despawn();
        }
    }

    /// Add a child to a parent's Children patch (creating one if needed).
    pub fn add_child_to_ast(&mut self, parent_ast: Entity, child_ast: Entity) {
        let Some(patches) = self.get_patches(parent_ast) else {
            return;
        };
        let patch_ids: Vec<Entity> = patches.0.clone();

        for &patch_entity in &patch_ids {
            if let Some(patch) = self.world.get_mut::<BsnPatch>(patch_entity)
                && let BsnPatch::Children(children) = patch.into_inner()
            {
                children.push(child_ast);
                return;
            }
        }

        let children_patch = self.world.spawn(BsnPatch::Children(vec![child_ast])).id();
        if let Some(patches) = self.get_patches_mut(parent_ast) {
            patches.0.push(children_patch);
        }
    }

    /// Every AST node (over all roots and their descendants) that carries a
    /// component patch of `type_path`. The match honours enum-variant type
    /// paths the same way [`find_patch_by_type_path`](Self::find_patch_by_type_path)
    /// does. Nodes are returned in pre-order (each root before its descendants).
    /// Returns an empty vector when no node carries the component.
    pub fn entities_with_component(&self, type_path: &str) -> Vec<Entity> {
        let mut out = Vec::new();
        for &root in &self.roots {
            if self.find_patch_by_type_path(root, type_path).is_some() {
                out.push(root);
            }
            for descendant in self.descendants_of(root) {
                if self.find_patch_by_type_path(descendant, type_path).is_some() {
                    out.push(descendant);
                }
            }
        }
        out
    }

    /// All AST descendants of `root_ast`, excluding `root_ast` itself. Walks the
    /// [`BsnPatch::Children`] relation recursively via
    /// [`get_children_ast`](Self::get_children_ast). Returns an empty vector when
    /// the node has no children.
    pub fn descendants_of(&self, root_ast: Entity) -> Vec<Entity> {
        let mut out = Vec::new();
        let mut stack = vec![root_ast];
        while let Some(current) = stack.pop() {
            for child in self.get_children_ast(current) {
                out.push(child);
                stack.push(child);
            }
        }
        out
    }

    /// The parent AST node that lists `ast` in its [`BsnPatch::Children`], or
    /// `None` when `ast` is a root (or is not present in the document). Public
    /// wrapper over the internal parentage walk.
    pub fn ast_parent_of(&self, ast: Entity) -> Option<Entity> {
        self.find_ast_parent_of(ast)
    }

    /// The nearest ancestor of `ast` (inclusive of `ast` itself) that carries a
    /// component patch of `type_path`, walking parentage upward via
    /// [`ast_parent_of`](Self::ast_parent_of). Returns `ast` when `ast` itself
    /// has the component, or `None` when no node on the chain carries it.
    pub fn ancestor_with_component(&self, ast: Entity, type_path: &str) -> Option<Entity> {
        let mut current = ast;
        loop {
            if self.find_patch_by_type_path(current, type_path).is_some() {
                return Some(current);
            }
            current = self.ast_parent_of(current)?;
        }
    }

    /// The first AST node (over all roots and their descendants, in pre-order)
    /// whose `type_path` component reads as a single integer equal to `value`.
    /// The component is read whole via `get_bsn_field(node, type_path, "")` and
    /// accepts either a bare integer scalar or a tuple struct wrapping exactly
    /// one integer, which is how a `u32`-newtype marker such as a prefab entity
    /// id serialises. Returns `None` when no node matches.
    pub fn find_node_by_component_int(&self, type_path: &str, value: u64) -> Option<Entity> {
        let mut nodes: Vec<Entity> = Vec::new();
        for &root in &self.roots {
            nodes.push(root);
            nodes.extend(self.descendants_of(root));
        }
        for node in nodes {
            let Some(whole) = crate::apply::get_bsn_field(self, node, type_path, "") else {
                continue;
            };
            if bsn_value_as_int(&whole) == Some(i128::from(value)) {
                return Some(node);
            }
        }
        None
    }
}

/// The integer a whole-component [`BsnValue`] represents, when it is either a
/// bare integer scalar or a tuple struct wrapping exactly one integer scalar.
/// `None` for every other shape.
fn bsn_value_as_int(value: &BsnValue) -> Option<i128> {
    match value {
        BsnValue::Int(v) => Some(*v),
        BsnValue::TupleStruct(data) => match data.values.as_slice() {
            [BsnValue::Int(v)] => Some(*v),
            _ => None,
        },
        _ => None,
    }
}

/// Create a new AST node in `dst` under `dst_parent`, deep-copying the component
/// patches of `src_node` from `src`. Every patch except the
/// [`BsnPatch::Children`] relation is cloned, so the new node keeps its
/// components, name, and base reference but adopts none of `src_node`'s
/// children. `dst_parent` gains the new node as a child. Returns the new node's
/// AST entity in `dst`.
pub fn clone_node_into(
    dst: &mut SceneBsnAst,
    src: &SceneBsnAst,
    src_node: Entity,
    dst_parent: Entity,
) -> Entity {
    let cloned_patches: Vec<BsnPatch> = match src.get_patches(src_node) {
        Some(patches) => patches
            .0
            .iter()
            .filter_map(|&pe| src.get_patch(pe))
            .filter(|patch| !matches!(patch, BsnPatch::Children(_)))
            .cloned()
            .collect(),
        None => Vec::new(),
    };
    let new_node = dst.create_entity_node(cloned_patches);
    dst.add_child_to_ast(dst_parent, new_node);
    new_node
}

/// Context for resolving `Handle<T>` fields to asset-path strings during BSN
/// emission. `parent_path` is the directory the emitted `.bsn` file lives in,
/// used to make emitted asset paths relative. `asset_names` maps assets that
/// have no filesystem path (catalog and scene-inline assets) to their
/// reference names (`@Name` or `#Name`), which are emitted verbatim.
pub struct BsnAssetContext<'a> {
    pub asset_server: &'a AssetServer,
    pub parent_path: &'a Path,
    pub asset_names: Option<&'a HashMap<bevy::asset::UntypedAssetId, String>>,
}

impl BsnAssetContext<'_> {
    /// Resolve a handle to its emitted string: a (relative) asset path when the
    /// asset came from disk, else its catalog/inline reference name, else an
    /// empty string.
    fn handle_string(&self, id: bevy::asset::UntypedAssetId) -> String {
        if let Some(path) = self.asset_server.get_path(id) {
            let path_str = path.to_string();
            if let Some(relative) = pathdiff::diff_paths(&path_str, self.parent_path) {
                return relative.to_string_lossy().into_owned();
            }
            return path_str;
        }
        if let Some(names) = self.asset_names
            && let Some(name) = names.get(&id)
        {
            return name.clone();
        }
        String::new()
    }
}

impl BsnValue {
    /// Create a [`BsnValue`] from a reflected value and its type info.
    pub fn from_reflect(value: &dyn PartialReflect, type_registry: &TypeRegistry) -> Self {
        Self::from_reflect_inner(value, type_registry, None)
    }

    /// Create a [`BsnValue`] from a reflected value, resolving `Handle<T>`
    /// fields (at any nesting depth) to asset-path strings via `ctx`.
    pub fn from_reflect_with_assets(
        value: &dyn PartialReflect,
        type_registry: &TypeRegistry,
        ctx: &BsnAssetContext,
    ) -> Self {
        Self::from_reflect_inner(value, type_registry, Some(ctx))
    }

    fn from_reflect_inner(
        value: &dyn PartialReflect,
        type_registry: &TypeRegistry,
        ctx: Option<&BsnAssetContext>,
    ) -> Self {
        // Try primitives first.
        if let Some(v) = value.try_downcast_ref::<f32>() {
            return BsnValue::Float(*v as f64);
        }
        if let Some(v) = value.try_downcast_ref::<f64>() {
            return BsnValue::Float(*v);
        }
        if let Some(v) = value.try_downcast_ref::<bool>() {
            return BsnValue::Bool(*v);
        }
        if let Some(v) = value.try_downcast_ref::<String>() {
            return BsnValue::String(v.clone());
        }
        if let Some(v) = value.try_downcast_ref::<std::borrow::Cow<'static, str>>() {
            return BsnValue::String(v.to_string());
        }
        // Integer types.
        if let Some(v) = value.try_downcast_ref::<i32>() {
            return BsnValue::Int(*v as i128);
        }
        if let Some(v) = value.try_downcast_ref::<u32>() {
            return BsnValue::Int(*v as i128);
        }
        if let Some(v) = value.try_downcast_ref::<i64>() {
            return BsnValue::Int(*v as i128);
        }
        if let Some(v) = value.try_downcast_ref::<u64>() {
            return BsnValue::Int(*v as i128);
        }
        if let Some(v) = value.try_downcast_ref::<usize>() {
            return BsnValue::Int(*v as i128);
        }
        if let Some(v) = value.try_downcast_ref::<i8>() {
            return BsnValue::Int(*v as i128);
        }
        if let Some(v) = value.try_downcast_ref::<u8>() {
            return BsnValue::Int(*v as i128);
        }
        if let Some(v) = value.try_downcast_ref::<i16>() {
            return BsnValue::Int(*v as i128);
        }
        if let Some(v) = value.try_downcast_ref::<u16>() {
            return BsnValue::Int(*v as i128);
        }

        // Handle<T> fields: resolve to an asset-path string when an asset
        // context is available.
        if let Some(ctx) = ctx
            && let Some(concrete) = value.try_as_reflect()
        {
            let type_id = concrete.reflect_type_info().type_id();
            if let Some(reflect_handle) = type_registry.get_type_data::<ReflectHandle>(type_id) {
                if let Some(untyped_handle) =
                    reflect_handle.downcast_handle_untyped(concrete.as_any())
                {
                    return BsnValue::String(ctx.handle_string(untyped_handle.id()));
                }
                // Handle that failed to downcast: emit an empty string.
                return BsnValue::String(String::new());
            }
        }

        // Option<Handle<T>> fields: emit the inner asset path (Some) or an
        // empty string (None) so the value round-trips through one path string.
        if let Some(ctx) = ctx
            && let ReflectRef::Enum(e) = value.reflect_ref()
        {
            let inner_is_handle = value
                .get_represented_type_info()
                .and_then(|info| match info {
                    bevy::reflect::TypeInfo::Enum(enum_info)
                        if enum_info.type_path().starts_with("core::option::Option<") =>
                    {
                        enum_info.variant("Some")
                    }
                    _ => None,
                })
                .and_then(|variant| match variant {
                    bevy::reflect::enums::VariantInfo::Tuple(tuple) => tuple.field_at(0),
                    _ => None,
                })
                .map(|field| {
                    type_registry
                        .get_type_data::<ReflectHandle>(field.type_id())
                        .is_some()
                })
                .unwrap_or(false);

            if inner_is_handle {
                match e.variant_name() {
                    "None" => return BsnValue::String(String::new()),
                    "Some" => {
                        if let Some(inner) = e.field_at(0)
                            && let Some(concrete) = inner.try_as_reflect()
                        {
                            let type_id = concrete.reflect_type_info().type_id();
                            if let Some(reflect_handle) =
                                type_registry.get_type_data::<ReflectHandle>(type_id)
                                && let Some(untyped_handle) =
                                    reflect_handle.downcast_handle_untyped(concrete.as_any())
                            {
                                return BsnValue::String(ctx.handle_string(untyped_handle.id()));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // Structs.
        if let ReflectRef::Struct(s) = value.reflect_ref() {
            let type_path = value
                .get_represented_type_info()
                .map(|info| info.type_path().to_string())
                .unwrap_or_default();
            let mut fields = Vec::new();
            for i in 0..s.field_len() {
                let name = s.name_at(i).unwrap().to_string();
                let field_value = s.field_at(i).unwrap();
                fields.push(BsnField {
                    name,
                    value: BsnValue::from_reflect_inner(field_value, type_registry, ctx),
                });
            }
            return BsnValue::Struct(BsnStructData {
                type_path,
                fields: BsnStructFields(fields),
            });
        }

        // Tuple structs.
        if let ReflectRef::TupleStruct(ts) = value.reflect_ref() {
            let type_path = value
                .get_represented_type_info()
                .map(|info| info.type_path().to_string())
                .unwrap_or_default();
            let mut values = Vec::new();
            for i in 0..ts.field_len() {
                let field_value = ts.field(i).unwrap();
                values.push(BsnValue::from_reflect_inner(
                    field_value,
                    type_registry,
                    ctx,
                ));
            }
            return BsnValue::TupleStruct(BsnTupleStructData { type_path, values });
        }

        // Enums.
        if let ReflectRef::Enum(e) = value.reflect_ref() {
            let type_path = value
                .get_represented_type_info()
                .map(|info| info.type_path().to_string())
                .unwrap_or_default();
            let variant = e.variant_name();
            let full_path = format!("{type_path}::{variant}");
            match e.variant_type() {
                VariantType::Struct => {
                    let mut fields = Vec::new();
                    for i in 0..e.field_len() {
                        let name = e.name_at(i).unwrap().to_string();
                        let field_value = e.field_at(i).unwrap();
                        fields.push(BsnField {
                            name,
                            value: BsnValue::from_reflect_inner(field_value, type_registry, ctx),
                        });
                    }
                    return BsnValue::Struct(BsnStructData {
                        type_path: full_path,
                        fields: BsnStructFields(fields),
                    });
                }
                VariantType::Tuple => {
                    let mut values = Vec::new();
                    for i in 0..e.field_len() {
                        let field_value = e.field_at(i).unwrap();
                        values.push(BsnValue::from_reflect_inner(
                            field_value,
                            type_registry,
                            ctx,
                        ));
                    }
                    return BsnValue::TupleStruct(BsnTupleStructData {
                        type_path: full_path,
                        values,
                    });
                }
                VariantType::Unit => {
                    return BsnValue::Type(full_path);
                }
            }
        }

        // Lists / Vecs.
        if let ReflectRef::List(l) = value.reflect_ref() {
            let mut items = Vec::new();
            for i in 0..l.len() {
                if let Some(item) = l.get(i) {
                    items.push(BsnValue::from_reflect_inner(item, type_registry, ctx));
                }
            }
            return BsnValue::List(items);
        }

        // Fixed-size arrays emit like lists; the applier rebuilds them against
        // the concrete array type.
        if let ReflectRef::Array(a) = value.reflect_ref() {
            let mut items = Vec::new();
            for i in 0..a.len() {
                if let Some(item) = a.get(i) {
                    items.push(BsnValue::from_reflect_inner(item, type_registry, ctx));
                }
            }
            return BsnValue::List(items);
        }

        // Maps / HashMaps.
        if let ReflectRef::Map(m) = value.reflect_ref() {
            let mut entries = Vec::new();
            for (key, val) in m.iter() {
                entries.push((
                    BsnValue::from_reflect_inner(key, type_registry, ctx),
                    BsnValue::from_reflect_inner(val, type_registry, ctx),
                ));
            }
            return BsnValue::Map(entries);
        }

        // Fallback: emit as string via Debug.
        BsnValue::String(format!("{value:?}"))
    }
}

/// Convert a component's reflected data into a BSN AST patch.
pub fn component_to_bsn_patch(
    reflected: &dyn PartialReflect,
    type_registry: &TypeRegistry,
) -> BsnPatch {
    component_to_bsn_patch_inner(reflected, type_registry, None)
}

/// Convert a component's reflected data into a BSN AST patch, resolving
/// `Handle<T>` fields (at any nesting depth) to asset-path strings via `ctx`.
pub fn component_to_bsn_patch_with_assets(
    reflected: &dyn PartialReflect,
    type_registry: &TypeRegistry,
    ctx: &BsnAssetContext,
) -> BsnPatch {
    component_to_bsn_patch_inner(reflected, type_registry, Some(ctx))
}

fn component_to_bsn_patch_inner(
    reflected: &dyn PartialReflect,
    type_registry: &TypeRegistry,
    ctx: Option<&BsnAssetContext>,
) -> BsnPatch {
    use bevy::reflect::prelude::ReflectDefault;

    let type_path = reflected
        .get_represented_type_info()
        .map(|info| info.type_path().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    match reflected.reflect_ref() {
        ReflectRef::Struct(s) => {
            let reg = type_registry.get_with_type_path(&type_path);
            let default_instance = reg
                .and_then(|r| r.data::<ReflectDefault>())
                .map(ReflectDefault::default);

            let mut fields = Vec::new();
            for i in 0..s.field_len() {
                let name = s.name_at(i).unwrap().to_string();
                let field_value = s.field_at(i).unwrap();

                // Only emit fields that differ from default.
                let should_emit = if let Some(ref default) = default_instance {
                    if let ReflectRef::Struct(ds) = default.reflect_ref() {
                        if let Some(default_field) = ds.field(&name) {
                            !field_value
                                .reflect_partial_eq(default_field)
                                .unwrap_or(false)
                        } else {
                            true
                        }
                    } else {
                        true
                    }
                } else {
                    true
                };

                if should_emit {
                    fields.push(BsnField {
                        name,
                        value: BsnValue::from_reflect_inner(field_value, type_registry, ctx),
                    });
                }
            }

            if fields.is_empty() {
                BsnPatch::Type(type_path)
            } else {
                BsnPatch::Struct(BsnStructData {
                    type_path,
                    fields: BsnStructFields(fields),
                })
            }
        }

        ReflectRef::TupleStruct(ts) => {
            let mut values = Vec::new();
            for i in 0..ts.field_len() {
                let field_value = ts.field(i).unwrap();
                values.push(BsnValue::from_reflect_inner(
                    field_value,
                    type_registry,
                    ctx,
                ));
            }
            BsnPatch::TupleStruct(BsnTupleStructData { type_path, values })
        }

        ReflectRef::Enum(e) => {
            let variant = e.variant_name();
            let full_path = format!("{type_path}::{variant}");
            match e.variant_type() {
                VariantType::Struct => {
                    let mut fields = Vec::new();
                    for i in 0..e.field_len() {
                        let name = e.name_at(i).unwrap().to_string();
                        let field_value = e.field_at(i).unwrap();
                        fields.push(BsnField {
                            name,
                            value: BsnValue::from_reflect_inner(field_value, type_registry, ctx),
                        });
                    }
                    if fields.is_empty() {
                        BsnPatch::Type(full_path)
                    } else {
                        BsnPatch::Struct(BsnStructData {
                            type_path: full_path,
                            fields: BsnStructFields(fields),
                        })
                    }
                }
                VariantType::Tuple => {
                    let mut values = Vec::new();
                    for i in 0..e.field_len() {
                        let field_value = e.field_at(i).unwrap();
                        values.push(BsnValue::from_reflect_inner(
                            field_value,
                            type_registry,
                            ctx,
                        ));
                    }
                    BsnPatch::TupleStruct(BsnTupleStructData {
                        type_path: full_path,
                        values,
                    })
                }
                VariantType::Unit => BsnPatch::Type(full_path),
            }
        }

        _ => BsnPatch::Type(type_path),
    }
}

#[cfg(test)]
mod query_tests {
    use super::*;

    const TRANSFORM: &str = "test::Transform";
    const MESH: &str = "test::Mesh";
    const PREFAB_ID: &str = "test::PrefabEntityId";

    fn prefab_id_patch(id: i128) -> BsnPatch {
        BsnPatch::TupleStruct(BsnTupleStructData {
            type_path: PREFAB_ID.to_string(),
            values: vec![BsnValue::Int(id)],
        })
    }

    /// Builds this tree, returning the node entities in a fixed order:
    ///
    /// ```text
    /// root       [Transform, PrefabEntityId(0)]
    ///   child_a  [Mesh,      PrefabEntityId(1)]
    ///     grand  [Transform, PrefabEntityId(2)]
    ///   child_b  [Transform, PrefabEntityId(3)]
    /// ```
    struct Tree {
        ast: SceneBsnAst,
        root: Entity,
        child_a: Entity,
        grand: Entity,
        child_b: Entity,
    }

    fn build_tree() -> Tree {
        let mut ast = SceneBsnAst::default();

        let root = ast.create_entity_node(vec![
            BsnPatch::Type(TRANSFORM.to_string()),
            prefab_id_patch(0),
        ]);
        let child_a =
            ast.create_entity_node(vec![BsnPatch::Type(MESH.to_string()), prefab_id_patch(1)]);
        let grand = ast.create_entity_node(vec![
            BsnPatch::Type(TRANSFORM.to_string()),
            prefab_id_patch(2),
        ]);
        let child_b = ast.create_entity_node(vec![
            BsnPatch::Type(TRANSFORM.to_string()),
            prefab_id_patch(3),
        ]);

        ast.add_to_roots(root);
        ast.add_child_to_ast(root, child_a);
        ast.add_child_to_ast(root, child_b);
        ast.add_child_to_ast(child_a, grand);

        Tree {
            ast,
            root,
            child_a,
            grand,
            child_b,
        }
    }

    #[test]
    fn entities_with_component_spans_all_depths() {
        let t = build_tree();
        let mut found = t.ast.entities_with_component(TRANSFORM);
        found.sort();
        let mut expected = vec![t.root, t.grand, t.child_b];
        expected.sort();
        assert_eq!(found, expected);

        assert_eq!(t.ast.entities_with_component(MESH), vec![t.child_a]);
        assert!(t.ast.entities_with_component("test::Absent").is_empty());
    }

    #[test]
    fn descendants_exclude_root() {
        let t = build_tree();
        let mut descendants = t.ast.descendants_of(t.root);
        descendants.sort();
        let mut expected = vec![t.child_a, t.child_b, t.grand];
        expected.sort();
        assert_eq!(descendants, expected);
        assert!(!descendants.contains(&t.root));

        assert!(t.ast.descendants_of(t.grand).is_empty());
    }

    #[test]
    fn ast_parent_of_walks_children_relation() {
        let t = build_tree();
        assert_eq!(t.ast.ast_parent_of(t.root), None);
        assert_eq!(t.ast.ast_parent_of(t.child_a), Some(t.root));
        assert_eq!(t.ast.ast_parent_of(t.child_b), Some(t.root));
        assert_eq!(t.ast.ast_parent_of(t.grand), Some(t.child_a));
    }

    #[test]
    fn ancestor_with_component_is_inclusive_of_self() {
        let t = build_tree();
        // The node itself carries Transform, so it is its own nearest match.
        assert_eq!(
            t.ast.ancestor_with_component(t.grand, TRANSFORM),
            Some(t.grand)
        );
        // Mesh lives only on child_a, an ancestor of grand.
        assert_eq!(t.ast.ancestor_with_component(t.grand, MESH), Some(t.child_a));
        // No node on child_b's chain carries Mesh.
        assert_eq!(t.ast.ancestor_with_component(t.child_b, MESH), None);
    }

    #[test]
    fn find_node_by_component_int_matches_tuple_struct_newtype() {
        let t = build_tree();
        assert_eq!(t.ast.find_node_by_component_int(PREFAB_ID, 2), Some(t.grand));
        assert_eq!(t.ast.find_node_by_component_int(PREFAB_ID, 0), Some(t.root));
        assert_eq!(t.ast.find_node_by_component_int(PREFAB_ID, 99), None);
    }

    #[test]
    fn clone_node_into_copies_components_but_not_children() {
        let src = build_tree();

        let mut dst = SceneBsnAst::default();
        let dst_root = dst.create_entity_node(vec![BsnPatch::Type("test::Root".to_string())]);
        dst.add_to_roots(dst_root);

        // child_a has a Mesh + PrefabEntityId patch and one child (grand).
        let cloned = clone_node_into(&mut dst, &src.ast, src.child_a, dst_root);

        assert_eq!(dst.get_children_ast(dst_root), vec![cloned]);

        let mut components = dst.component_type_paths(cloned);
        components.sort();
        let mut expected = vec![MESH.to_string(), PREFAB_ID.to_string()];
        expected.sort();
        assert_eq!(components, expected);

        // The single-node clone must not drag the source's children across.
        assert!(dst.get_children_ast(cloned).is_empty());
    }
}
