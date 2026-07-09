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
