use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

use rand::Rng;

use bevy::reflect::{TypeRegistry, UnnamedField};
use bevy::{prelude::*, reflect::NamedField};

use crate::format::{JsnAssets, JsnEntity, JsnMetadata, JsnScene};

/// Stable per-node identifier that persists in the `.jsn` and is attached to
/// the spawned ECS entity. Lets a running game map a live entity back to the
/// authored scene node it came from (PIE "save runtime values" needs this).
///
/// Like `BrushStableId`, it survives the snapshot respawn cycle and the
/// save/load round-trip; unlike `BrushStableId`, it is carried as a structural
/// field on the node (see `JsnEntityNode::id`) so it is the canonical on-disk
/// form rather than just another reflected component.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug, Reflect)]
#[reflect(Component, @crate::EditorHidden)]
pub struct JsnNodeId(pub u64);

/// Reflect type path for [`JsnNodeId`], used when projecting the structural
/// node id to and from the reflected-component representation.
pub const JSN_NODE_ID_TYPE_PATH: &str = "jackdaw_jsn::ast::JsnNodeId";

/// Lower bound of the sparse id range. Ids below this are legacy values minted
/// by the old monotonic-from-1 counter and can collide across sessions.
pub const SPARSE_MIN: u64 = 1 << 32;
/// Upper bound of the random seed draw. The counter is seeded below this and
/// then advances freely, so minted ids are not clamped to it; the value leaves
/// headroom below `u64::MAX` for a session to mint without wrapping.
const SPARSE_MAX: u64 = 1 << 63;

/// Process-global source of fresh node ids, seeded once from a random base in
/// `[SPARSE_MIN, SPARSE_MAX)`. A random base keeps ids unique across processes
/// and across independently-authored files: two sessions land in different
/// ranges, so their ids never collide. Within a session ids stay monotonic.
static NEXT_NODE_ID: LazyLock<AtomicU64> =
    LazyLock::new(|| AtomicU64::new(rand::rng().random_range(SPARSE_MIN..SPARSE_MAX)));

impl JsnNodeId {
    /// Mint a fresh node id by advancing the global counter.
    pub fn next() -> Self {
        JsnNodeId(NEXT_NODE_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// Report whether a loaded scene carries node ids that break the global-key
/// invariant: any duplicate id, any id below `SPARSE_MIN` (minted by the old
/// counter), or any missing id. Such scenes are re-minted on load.
pub fn needs_id_migration(scene: &JsnScene) -> bool {
    let mut seen = HashSet::new();
    for entity in &scene.scene {
        match entity.id {
            Some(id) if id >= SPARSE_MIN => {
                if !seen.insert(id) {
                    return true;
                }
            }
            // Below the sparse range, or no id at all: legacy, must heal.
            _ => return true,
        }
    }
    false
}

/// In-memory scene document  -- the single source of truth for scene data.
///
/// All editor mutations should go through this resource. ECS entities exist
/// as a preview layer, kept in sync by `apply_dirty_jsn_to_ecs`.
///
/// Structurally parallel to the BSN branch's `SceneBsnAst`, enabling a
/// mechanical swap when BSN ships upstream.
#[derive(Resource, Default, Clone, PartialEq)]
pub struct SceneJsnAst {
    /// Entity nodes, indexed by position.
    pub nodes: Vec<JsnEntityNode>,
    /// Map from ECS preview entity to node index.
    pub ecs_to_jsn: HashMap<Entity, usize>,
    /// Indices of nodes whose ECS preview entities need re-sync.
    pub dirty_indices: HashSet<usize>,
    /// Inline assets table (materials, images, etc.).
    pub assets: JsnAssets,
}

/// A single entity in the scene document.
///
/// Mirrors `JsnEntity` from the file format  -- `name` and `parent` are
/// structural fields, everything else (Transform, Visibility, Brush, etc.)
/// lives in `components` as `serde_json::Value`.
#[derive(Clone, PartialEq)]
pub struct JsnEntityNode {
    /// Stable id for this node, persisted in the `.jsn` and attached to the
    /// spawned ECS entity. `None` only transiently before a fresh id is
    /// minted (`from_jsn_scene`, `create_node`, `add_root`, `add_child`).
    pub id: Option<JsnNodeId>,
    /// Parent index into `SceneJsnAst::nodes`.
    pub parent: Option<usize>,
    /// All component data keyed by type path (e.g. `"bevy_transform::components::transform::Transform"`).
    /// Includes Name, Transform, Visibility  -- everything is a component.
    pub components: HashMap<String, serde_json::Value>,
    /// Components auto-added via Bevy's `#[require]` attributes (e.g., avian's
    /// `Position`, `ColliderAabb`, `ComputedMass`, etc.). These are:
    ///
    /// - **Displayed** in the inspector (for debugging / advanced editing)
    /// - **NOT serialized** to the scene file (they're recreated at runtime)
    /// - **Promoted to authored** if the user explicitly edits one (removed from
    ///   this set -> persisted on next save)
    ///
    /// Populated by `sync_required_to_ast` after `AddComponent`.
    pub derived_components: HashSet<String>,
    /// The ECS entity used to preview this node in the viewport.
    pub ecs_entity: Option<Entity>,
}

impl SceneJsnAst {
    /// Populate from a loaded `JsnScene` and the ECS entities that were spawned for it.
    ///
    /// `entity_map` maps JSN entity index -> spawned ECS entity.
    pub fn from_jsn_scene(scene: &JsnScene, entity_map: &[Entity]) -> Self {
        let mut ecs_to_jsn = HashMap::new();
        let mut nodes: Vec<JsnEntityNode> = scene
            .scene
            .iter()
            .enumerate()
            .map(|(i, jsn)| {
                let ecs_entity = entity_map.get(i).copied();
                if let Some(e) = ecs_entity {
                    ecs_to_jsn.insert(e, i);
                }
                // The structural `id` is canonical. Fall back to a stray
                // `JsnNodeId` reflected into `components` (defensive against
                // older save paths), and finally mint a fresh id so every
                // loaded node is identifiable.
                let mut components = jsn.components.clone();
                let id = jsn
                    .id
                    .map(JsnNodeId)
                    .or_else(|| {
                        components
                            .remove(JSN_NODE_ID_TYPE_PATH)
                            .as_ref()
                            .and_then(serde_json::Value::as_u64)
                            .map(JsnNodeId)
                    })
                    .unwrap_or_else(JsnNodeId::next);
                JsnEntityNode {
                    id: Some(id),
                    parent: jsn.parent,
                    components,
                    derived_components: HashSet::new(),
                    ecs_entity,
                }
            })
            .collect();

        // Older scenes minted ids from a per-process counter that reset every
        // run, so a loaded scene can carry duplicate or low-range ids that
        // collapse distinct nodes onto one entity in the by-id match. Re-mint
        // every node to a sparse id when that is detected. Parent links are
        // stored by index, so they are unaffected.
        if needs_id_migration(scene) {
            for node in &mut nodes {
                node.id = Some(JsnNodeId::next());
            }
        }

        Self {
            nodes,
            ecs_to_jsn,
            dirty_indices: HashSet::new(),
            assets: scene.assets.clone(),
        }
    }

    /// Emit a `JsnScene` for serialization to disk.
    pub fn to_jsn_scene(&self, metadata: JsnMetadata) -> JsnScene {
        let scene = self
            .nodes
            .iter()
            .map(|node| JsnEntity {
                id: node.id.map(|id| id.0),
                parent: node.parent,
                components: node.components.clone(),
            })
            .collect();

        JsnScene {
            jsn: crate::format::JsnHeader::default(),
            metadata,
            assets: self.assets.clone(),
            editor: None,
            scene,
        }
    }

    /// Look up a node by ECS entity.
    pub fn node_for_entity(&self, entity: Entity) -> Option<&JsnEntityNode> {
        self.ecs_to_jsn
            .get(&entity)
            .and_then(|&idx| self.nodes.get(idx))
    }

    /// Look up a node mutably by ECS entity.
    pub fn node_for_entity_mut(&mut self, entity: Entity) -> Option<&mut JsnEntityNode> {
        self.ecs_to_jsn
            .get(&entity)
            .copied()
            .and_then(|idx| self.nodes.get_mut(idx))
    }

    /// Find the index of the node carrying the given stable [`JsnNodeId`].
    ///
    /// Linear scan over `nodes`; ids are unique, so the first match is the
    /// only one. Used to map a running game's live entity back to its
    /// authored node (the PIE "save runtime values" path).
    pub fn node_index_by_id(&self, id: JsnNodeId) -> Option<usize> {
        self.nodes.iter().position(|n| n.id == Some(id))
    }

    /// Look up the editor preview ECS entity for the node carrying the given
    /// stable [`JsnNodeId`]. `None` when no node has that id, or the matching
    /// node has no preview entity (e.g. an inherited node spawned ECS-only).
    pub fn entity_for_node_id(&self, id: JsnNodeId) -> Option<Entity> {
        let idx = self.node_index_by_id(id)?;
        self.nodes.get(idx).and_then(|n| n.ecs_entity)
    }

    /// Mark a node as dirty so its ECS entity will be re-synced.
    pub fn mark_dirty(&mut self, entity: Entity) {
        if let Some(&idx) = self.ecs_to_jsn.get(&entity) {
            self.dirty_indices.insert(idx);
        }
    }

    /// Clear all state (e.g. when loading a new scene or creating a blank scene).
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.ecs_to_jsn.clear();
        self.dirty_indices.clear();
        self.assets = JsnAssets::default();
    }

    // -- Entity lifecycle ---------------------------------------------

    /// Create a new node for an ECS entity and register it in the AST.
    /// Returns the node index.
    pub fn create_node(&mut self, ecs_entity: Entity, parent: Option<Entity>) -> usize {
        let parent_idx = parent.and_then(|p| self.ecs_to_jsn.get(&p).copied());
        let idx = self.nodes.len();
        self.nodes.push(JsnEntityNode {
            id: Some(JsnNodeId::next()),
            parent: parent_idx,
            components: HashMap::new(),
            derived_components: HashSet::new(),
            ecs_entity: Some(ecs_entity),
        });
        self.ecs_to_jsn.insert(ecs_entity, idx);
        idx
    }

    /// Remove a node by ECS entity. Returns the removed node for undo.
    /// Orphaned children (their parent pointed at the removed node) are
    /// promoted to roots, not silently reparented to node 0.
    pub fn remove_node(&mut self, entity: Entity) -> Option<JsnEntityNode> {
        let idx = self.ecs_to_jsn.remove(&entity)?;
        if idx < self.nodes.len() {
            let node = self.nodes.remove(idx);
            for entry in self.ecs_to_jsn.values_mut() {
                if *entry > idx {
                    *entry -= 1;
                }
            }
            for node in &mut self.nodes {
                if let Some(parent) = node.parent {
                    if parent == idx {
                        node.parent = None;
                    } else if parent > idx {
                        node.parent = Some(parent - 1);
                    }
                }
            }
            self.dirty_indices.remove(&idx);
            Some(node)
        } else {
            None
        }
    }

    /// Check if an ECS entity is tracked in the AST.
    pub fn contains_entity(&self, entity: Entity) -> bool {
        self.ecs_to_jsn.contains_key(&entity)
    }

    // -- Component field accessors --------------------------------------

    /// Get a component's full JSON value by type path.
    pub fn get_component(&self, entity: Entity, type_path: &str) -> Option<&serde_json::Value> {
        self.node_for_entity(entity)?.components.get(type_path)
    }

    /// Set a component's full JSON value by type path, marking the node dirty.
    pub fn set_component(&mut self, entity: Entity, type_path: &str, value: serde_json::Value) {
        if let Some(node) = self.node_for_entity_mut(entity) {
            node.components.insert(type_path.to_string(), value);
        }
        self.mark_dirty(entity);
    }

    /// Get a nested field within a component's JSON by dotted path.
    /// Uses the type registry to resolve named fields to array indices when
    /// the JSON value is an array (e.g., Vec3 serializes as [x, y, z]).
    pub fn get_component_field(
        &self,
        entity: Entity,
        type_path: &str,
        field_path: &str,
        registry: &TypeRegistry,
    ) -> Option<&serde_json::Value> {
        let component = self.get_component(entity, type_path)?;
        get_field_in_component_json(component, type_path, field_path, registry)
    }

    /// Set a nested field within a component's JSON by dotted path, marking dirty.
    /// Uses the type registry to resolve named fields to array indices.
    /// An empty `field_path` writes the whole component, inserting the entry
    /// when the component is not yet authored on the node.
    pub fn set_component_field(
        &mut self,
        entity: Entity,
        type_path: &str,
        field_path: &str,
        value: serde_json::Value,
        registry: &TypeRegistry,
    ) {
        if field_path.is_empty() {
            if let Some(node) = self.node_for_entity_mut(entity) {
                node.components.insert(type_path.to_string(), value);
            }
            self.mark_dirty(entity);
            return;
        }
        let registration = registry.get_with_type_path(type_path);
        if let Some(node) = self.node_for_entity_mut(entity)
            && let Some(component) = node.components.get_mut(type_path)
            && let Some(registration) = registration
        {
            typed_json_path_set(component, field_path, value, registration, registry);
        }
        self.mark_dirty(entity);
    }

    // -- Index-keyed structural accessors -------------------------------

    /// Add a new top-level node. Returns its index.
    pub fn add_root(&mut self) -> usize {
        let idx = self.nodes.len();
        self.nodes.push(JsnEntityNode {
            id: Some(JsnNodeId::next()),
            parent: None,
            components: HashMap::new(),
            derived_components: HashSet::new(),
            ecs_entity: None,
        });
        idx
    }

    /// Add a new child node under `parent`. Returns its index.
    pub fn add_child(&mut self, parent: usize) -> usize {
        let idx = self.nodes.len();
        self.nodes.push(JsnEntityNode {
            id: Some(JsnNodeId::next()),
            parent: Some(parent),
            components: HashMap::new(),
            derived_components: HashSet::new(),
            ecs_entity: None,
        });
        idx
    }

    pub fn insert_component(&mut self, key: usize, type_path: &str, value: serde_json::Value) {
        if let Some(node) = self.nodes.get_mut(key) {
            node.components.insert(type_path.to_string(), value);
        }
    }

    pub fn replace_component(&mut self, key: usize, type_path: &str, value: serde_json::Value) {
        self.insert_component(key, type_path, value);
    }

    pub fn get_component_at(&self, key: usize, type_path: &str) -> Option<&serde_json::Value> {
        self.nodes
            .get(key)
            .and_then(|n| n.components.get(type_path))
    }

    pub fn components_at(&self, key: usize) -> Option<&HashMap<String, serde_json::Value>> {
        self.nodes.get(key).map(|n| &n.components)
    }

    pub fn entities_with_component<'a>(
        &'a self,
        type_path: &'a str,
    ) -> impl Iterator<Item = usize> + 'a {
        self.nodes.iter().enumerate().filter_map(move |(i, n)| {
            if n.components.contains_key(type_path) {
                Some(i)
            } else {
                None
            }
        })
    }

    pub fn roots(&self) -> impl Iterator<Item = usize> + '_ {
        self.nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| if n.parent.is_none() { Some(i) } else { None })
    }

    pub fn children_of(&self, parent: usize) -> impl Iterator<Item = usize> + '_ {
        self.nodes.iter().enumerate().filter_map(move |(i, n)| {
            if n.parent == Some(parent) {
                Some(i)
            } else {
                None
            }
        })
    }

    /// Depth-first descendants of `root`, EXCLUDING the root itself.
    pub fn descendants_of(&self, root: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(cur) = stack.pop() {
            for child in self.children_of(cur) {
                out.push(child);
                stack.push(child);
            }
        }
        out
    }

    /// Clone a node and its descendants from `other` into `self` under `parent`.
    /// Returns the new index of the cloned root.
    pub fn clone_node_into(
        &mut self,
        other: &SceneJsnAst,
        src_root: usize,
        parent: usize,
    ) -> usize {
        let mut idx_map: HashMap<usize, usize> = HashMap::new();
        let mut queue: Vec<usize> = vec![src_root];
        while let Some(src_idx) = queue.first().copied() {
            queue.remove(0);
            let Some(src_node) = other.nodes.get(src_idx) else {
                continue;
            };
            let dst_parent = if src_idx == src_root {
                Some(parent)
            } else {
                src_node.parent.and_then(|p| idx_map.get(&p).copied())
            };
            let dst_idx = self.nodes.len();
            self.nodes.push(JsnEntityNode {
                // A clone is a new node, so it gets a fresh id rather than
                // sharing the source's (mirrors paste minting fresh ids).
                id: Some(JsnNodeId::next()),
                parent: dst_parent,
                components: src_node.components.clone(),
                derived_components: src_node.derived_components.clone(),
                ecs_entity: None,
            });
            idx_map.insert(src_idx, dst_idx);
            for child in other.children_of(src_idx) {
                queue.push(child);
            }
        }
        idx_map[&src_root]
    }

    pub fn key_for_entity(&self, entity: Entity) -> Option<usize> {
        self.ecs_to_jsn.get(&entity).copied()
    }

    /// Remove a component from a node. No-op if the node or component is missing.
    pub fn remove_component(&mut self, key: usize, type_path: &str) {
        if let Some(node) = self.nodes.get_mut(key) {
            node.components.remove(type_path);
        }
    }

    /// Walk up the parent chain from `key` (inclusive) until a node carrying
    /// `type_path` is found; returns its index, or None if none exists.
    pub fn ancestor_with_component(&self, key: usize, type_path: &str) -> Option<usize> {
        let mut current = key;
        loop {
            if self.nodes.get(current)?.components.contains_key(type_path) {
                return Some(current);
            }
            current = self.nodes.get(current)?.parent?;
        }
    }
}

// -- Type-aware JSON path navigation ------------------------------------
//
// Mirrors BSN's BsnValue tree navigation. Uses the TypeRegistry to resolve
// named fields to array indices when the JSON value is an array (e.g., Vec3
// serializes as [x, y, z] but reflection paths use `translation.x`).

use bevy::reflect::{EnumInfo, TypeInfo, TypeRegistration, VariantInfo};

/// Read a nested field by dotted path from a standalone component JSON value,
/// resolving named fields to array indices via the type registry. Uses the
/// same path syntax as [`set_field_in_component_json`]: dot-separated
/// segments, bracket notation for list elements (e.g. `faces[0]`), and
/// automatic enum-variant unwrapping. An empty `field_path` returns the whole
/// component value. Returns `None` when `type_path` is not registered or any
/// path segment is absent.
pub fn get_field_in_component_json<'a>(
    component: &'a serde_json::Value,
    type_path: &str,
    field_path: &str,
    registry: &TypeRegistry,
) -> Option<&'a serde_json::Value> {
    let registration = registry.get_with_type_path(type_path)?;
    typed_json_path_get(component, field_path, registration, registry)
}

/// Set a nested field by dotted path inside a standalone component JSON
/// value, resolving named fields to array indices via the type registry.
///
/// This is the same navigation [`SceneJsnAst::set_component_field`] runs,
/// exposed for callers that hold a component value outside the AST (the
/// PIE live mirror keeps component JSON keyed by type path). `component`
/// is mutated in place; a `field_path` of `""` replaces the whole value.
/// A no-op when `type_path` isn't registered.
pub fn set_field_in_component_json(
    component: &mut serde_json::Value,
    type_path: &str,
    field_path: &str,
    value: serde_json::Value,
    registry: &TypeRegistry,
) {
    let Some(registration) = registry.get_with_type_path(type_path) else {
        return;
    };
    typed_json_path_set(component, field_path, value, registration, registry);
}

/// Resolve a field name to an array index using type info.
/// Returns `None` if the type doesn't have named fields or the name isn't found.
fn field_index_from_type_info(type_info: &TypeInfo, field_name: &str) -> Option<usize> {
    match type_info {
        TypeInfo::Struct(s) => s.index_of(field_name),
        TypeInfo::TupleStruct(_) => field_name.parse::<usize>().ok(),
        _ => None,
    }
}

/// Given a JSON value representing an enum in Bevy's reflect serialization
/// format (`{"VariantName": inner}` for struct/tuple, `"VariantName"` for
/// unit), return the variant name and a reference to the inner JSON.
///
/// For unit variants the "inner" is the string itself  -- callers must check
/// the variant kind via `EnumInfo` before descending further.
fn enum_variant_from_json(json: &serde_json::Value) -> Option<(&str, &serde_json::Value)> {
    match json {
        serde_json::Value::Object(map) if map.len() == 1 => {
            let (name, inner) = map.iter().next()?;
            Some((name.as_str(), inner))
        }
        serde_json::Value::String(name) => Some((name.as_str(), json)),
        _ => None,
    }
}

fn enum_variant_from_json_mut(
    json: &mut serde_json::Value,
) -> Option<(String, &mut serde_json::Value)> {
    match json {
        serde_json::Value::Object(map) if map.len() == 1 => {
            let name = map.keys().next().cloned()?;
            let inner = map.get_mut(&name)?;
            Some((name, inner))
        }
        _ => None,
    }
}

/// Find a field on the current variant by name (or index for tuple variants)
/// and return its [`TypeRegistration`]. Used to advance `current_reg` after an
/// enum has been unwrapped during path navigation.
fn variant_field_type_registration<'a>(
    enum_info: &EnumInfo,
    variant_name: &str,
    field_name: &str,
    registry: &'a TypeRegistry,
) -> Option<&'a TypeRegistration> {
    let variant = enum_info.variant(variant_name)?;
    let field_type_id = match variant {
        VariantInfo::Struct(s) => s.field(field_name).map(NamedField::type_id)?,
        VariantInfo::Tuple(t) => {
            let idx: usize = field_name.parse().ok()?;
            t.field_at(idx).map(UnnamedField::type_id)?
        }
        VariantInfo::Unit(_) => return None,
    };
    registry.get(field_type_id)
}

/// Whether `segment` addresses field 0 of a single-field (newtype) tuple
/// variant. Bevy serializes such a variant with its one field flattened
/// (`{"Mirror": {..}}`, not `{"Mirror": [{..}]}`), so the path navigator must
/// treat index 0 as the inner value itself rather than looking for an array
/// element or an object key named "0".
fn is_newtype_index0(enum_info: &EnumInfo, variant_name: &str, segment: &str) -> bool {
    segment == "0"
        && matches!(
            enum_info.variant(variant_name),
            Some(VariantInfo::Tuple(t)) if t.field_len() == 1
        )
}

/// Get the [`TypeRegistration`] for a field by name, advancing through the type tree.
fn field_type_registration<'a>(
    type_info: &TypeInfo,
    field_name: &str,
    registry: &'a TypeRegistry,
) -> Option<&'a TypeRegistration> {
    let field_type_id = match type_info {
        TypeInfo::Struct(s) => s.field(field_name).map(NamedField::type_id),
        TypeInfo::TupleStruct(ts) => {
            let idx = field_name.parse::<usize>().ok()?;
            ts.field_at(idx).map(UnnamedField::type_id)
        }
        TypeInfo::List(l) => Some(l.item_ty().id()),
        _ => None,
    }?;
    registry.get(field_type_id)
}

/// Navigate into a JSON value using a dotted field path and type info.
fn typed_json_path_get<'a>(
    root: &'a serde_json::Value,
    path: &str,
    registration: &TypeRegistration,
    registry: &TypeRegistry,
) -> Option<&'a serde_json::Value> {
    if path.is_empty() {
        return Some(root);
    }

    let mut current = root;
    let mut current_reg = registration;

    for segment in path.split('.').filter(|s| !s.is_empty()) {
        let type_info = current_reg.type_info();

        // Auto-unwrap enums: Bevy's reflect-path for `ColliderConstructor::Sphere`
        // treats `"radius"` as a field of the *current variant*, not a sibling of
        // the variant tag. Mirror that by descending into the variant's inner
        // JSON object before consuming the segment.
        if let TypeInfo::Enum(enum_info) = type_info {
            let (variant_name, inner) = enum_variant_from_json(current)?;
            let next_reg =
                variant_field_type_registration(enum_info, variant_name, segment, registry)?;
            let next_val = if is_newtype_index0(enum_info, variant_name, segment) {
                // A single-field (newtype) tuple variant is serialized with the
                // inner value flattened, not wrapped in a one-element array, so
                // index 0 is the inner value itself.
                inner
            } else {
                match inner {
                    serde_json::Value::Object(_) => inner.get(segment)?,
                    serde_json::Value::Array(_) => {
                        let idx: usize = segment.parse().ok()?;
                        inner.get(idx)?
                    }
                    _ => return None,
                }
            };
            current = next_val;
            current_reg = next_reg;
            continue;
        }

        // Handle bracket indexing (e.g., "faces[0]")
        if let Some(bracket_pos) = segment.find('[') {
            let key = &segment[..bracket_pos];
            let idx_str = &segment[bracket_pos + 1..segment.len() - 1];
            let idx: usize = idx_str.parse().ok()?;

            // Navigate to the key first
            current = navigate_json_field(current, key, type_info)?;
            // Then look up the list element type
            if let Some(key_reg) = field_type_registration(type_info, key, registry) {
                current_reg = key_reg;
            }
            // Navigate into the array
            current = current.get(idx)?;
            // Advance type info to list element
            let list_info = current_reg.type_info();
            if let TypeInfo::List(l) = list_info
                && let Some(elem_reg) = registry.get(l.item_ty().id())
            {
                current_reg = elem_reg;
            }
        } else {
            // Simple field navigation
            current = navigate_json_field(current, segment, type_info)?;
            // Advance type info
            if let Some(next_reg) = field_type_registration(type_info, segment, registry) {
                current_reg = next_reg;
            }
        }
    }

    Some(current)
}

/// Navigate one level into a JSON value using a field name and type info.
/// Handles both Object (named key) and Array (field index from type info).
fn navigate_json_field<'a>(
    json: &'a serde_json::Value,
    field_name: &str,
    type_info: &TypeInfo,
) -> Option<&'a serde_json::Value> {
    match json {
        serde_json::Value::Object(_) => json.get(field_name),
        serde_json::Value::Array(_) => {
            // Resolve named field to array index via type info
            let idx = if let Ok(i) = field_name.parse::<usize>() {
                i
            } else {
                field_index_from_type_info(type_info, field_name)?
            };
            json.get(idx)
        }
        _ => None,
    }
}

/// Set a value at a dotted field path within a JSON value, using type info.
fn typed_json_path_set(
    root: &mut serde_json::Value,
    path: &str,
    value: serde_json::Value,
    registration: &TypeRegistration,
    registry: &TypeRegistry,
) {
    if path.is_empty() {
        *root = value;
        return;
    }

    let segments: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
    let mut current = root;
    let mut current_reg = registration;

    for (i, segment) in segments.iter().enumerate() {
        let is_last = i == segments.len() - 1;
        let type_info = current_reg.type_info();

        // Auto-unwrap enums: descend into the variant's inner value so paths
        // like `"radius"` address the field on the current variant rather
        // than inserting a sibling of the variant tag.
        if let TypeInfo::Enum(enum_info) = type_info {
            let Some((variant_name, inner)) = enum_variant_from_json_mut(current) else {
                return;
            };
            let Some(next_reg) =
                variant_field_type_registration(enum_info, &variant_name, segment, registry)
            else {
                return;
            };
            let newtype_index0 = is_newtype_index0(enum_info, &variant_name, segment);
            if is_last {
                if newtype_index0 {
                    // Setting index 0 of a newtype variant replaces the inner
                    // value directly (it is flattened, not array-wrapped).
                    *inner = value;
                    return;
                }
                // Set the field inside the variant's inner value.
                match inner {
                    serde_json::Value::Object(map) => {
                        map.insert(segment.to_string(), value);
                    }
                    serde_json::Value::Array(arr) => {
                        if let Ok(idx) = segment.parse::<usize>()
                            && idx < arr.len()
                        {
                            arr[idx] = value;
                        }
                    }
                    _ => {}
                }
                return;
            }
            // Descend into the variant's field and continue. A newtype variant
            // flattens its single field, so index 0 is the inner value itself.
            let field_val = if newtype_index0 {
                Some(inner)
            } else {
                match inner {
                    serde_json::Value::Object(map) => map.get_mut(*segment),
                    serde_json::Value::Array(arr) => match segment.parse::<usize>() {
                        Ok(idx) => arr.get_mut(idx),
                        Err(_) => return,
                    },
                    _ => None,
                }
            };
            let Some(next) = field_val else { return };
            current = next;
            current_reg = next_reg;
            continue;
        }

        if let Some(bracket_pos) = segment.find('[') {
            let key = &segment[..bracket_pos];
            let idx_str = &segment[bracket_pos + 1..segment.len() - 1];
            let idx: usize = match idx_str.parse() {
                Ok(i) => i,
                Err(_) => return,
            };
            // Navigate to key
            let next = navigate_json_field_mut(current, key, type_info);
            let Some(arr_val) = next else { return };
            if let Some(key_reg) = field_type_registration(type_info, key, registry) {
                current_reg = key_reg;
            }
            if is_last {
                if let Some(arr) = arr_val.as_array_mut()
                    && idx < arr.len()
                {
                    arr[idx] = value;
                }
                return;
            }
            current = match arr_val.get_mut(idx) {
                Some(v) => v,
                None => return,
            };
            let list_info = current_reg.type_info();
            if let TypeInfo::List(l) = list_info
                && let Some(elem_reg) = registry.get(l.item_ty().id())
            {
                current_reg = elem_reg;
            }
        } else {
            if is_last {
                set_json_field(current, segment, value, type_info);
                return;
            }
            let next_reg = field_type_registration(type_info, segment, registry);
            let Some(next) = navigate_json_field_mut(current, segment, type_info) else {
                return;
            };
            current = next;
            if let Some(nr) = next_reg {
                current_reg = nr;
            }
        }
    }
}

/// Navigate one level into a mutable JSON value.
fn navigate_json_field_mut<'a>(
    json: &'a mut serde_json::Value,
    field_name: &str,
    type_info: &TypeInfo,
) -> Option<&'a mut serde_json::Value> {
    match json {
        serde_json::Value::Object(_) => json.get_mut(field_name),
        serde_json::Value::Array(_) => {
            let idx = if let Ok(i) = field_name.parse::<usize>() {
                i
            } else {
                field_index_from_type_info(type_info, field_name)?
            };
            json.get_mut(idx)
        }
        _ => None,
    }
}

/// Set a field value in a JSON value (handles both Object and Array).
fn set_json_field(
    json: &mut serde_json::Value,
    field_name: &str,
    value: serde_json::Value,
    type_info: &TypeInfo,
) {
    match json {
        serde_json::Value::Object(map) => {
            map.insert(field_name.to_string(), value);
        }
        serde_json::Value::Array(arr) => {
            let idx = if let Ok(i) = field_name.parse::<usize>() {
                i
            } else if let Some(i) = field_index_from_type_info(type_info, field_name) {
                i
            } else {
                return;
            };
            if idx < arr.len() {
                arr[idx] = value;
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::reflect::serde::TypedReflectSerializer;

    /// Build the canonical reflect JSON for a value, the same form the PIE
    /// live mirror stores and the game's `TypedReflectDeserializer` expects.
    fn to_canonical_json<T: bevy::reflect::PartialReflect>(
        value: &T,
        registry: &TypeRegistry,
    ) -> serde_json::Value {
        let serializer = TypedReflectSerializer::new(value, registry);
        serde_json::to_value(&serializer).expect("serialize reflected value")
    }

    #[test]
    fn set_field_in_component_json_sets_nested_array_field() {
        let mut registry = TypeRegistry::new();
        registry.register::<Transform>();

        let type_path = "bevy_transform::components::transform::Transform";
        let mut component = to_canonical_json(&Transform::from_xyz(1.0, 2.0, 3.0), &registry);

        // `Vec3` serializes as `[x, y, z]`, so `translation.x` must resolve
        // the named axis to array index 0 via the registry.
        set_field_in_component_json(
            &mut component,
            type_path,
            "translation.x",
            serde_json::json!(9.5),
            &registry,
        );

        // The edited axis changed; the siblings and other fields did not,
        // so the merged value is still a full, deserializable component.
        let translation = &component["translation"];
        assert_eq!(translation[0], 9.5);
        assert_eq!(translation[1], 2.0);
        assert_eq!(translation[2], 3.0);
        assert!(component.get("rotation").is_some());
        assert!(component.get("scale").is_some());
    }

    #[test]
    fn set_field_in_component_json_empty_path_replaces_value() {
        let mut registry = TypeRegistry::new();
        registry.register::<Transform>();

        let type_path = "bevy_transform::components::transform::Transform";
        let mut component = to_canonical_json(&Transform::IDENTITY, &registry);
        let replacement = to_canonical_json(&Transform::from_xyz(4.0, 5.0, 6.0), &registry);

        set_field_in_component_json(
            &mut component,
            type_path,
            "",
            replacement.clone(),
            &registry,
        );

        assert_eq!(component, replacement);
    }

    #[test]
    fn set_field_in_component_json_unregistered_type_is_noop() {
        let registry = TypeRegistry::new();
        let mut component = serde_json::json!({ "translation": [0.0, 0.0, 0.0] });
        let before = component.clone();

        set_field_in_component_json(
            &mut component,
            "not::A::RegisteredType",
            "translation.x",
            serde_json::json!(1.0),
            &registry,
        );

        assert_eq!(component, before);
    }

    #[test]
    fn get_field_reads_named_struct_field() {
        let mut registry = TypeRegistry::new();
        registry.register::<Transform>();

        let type_path = "bevy_transform::components::transform::Transform";
        let component = to_canonical_json(&Transform::from_xyz(1.0, 2.0, 3.0), &registry);

        // `Vec3` serializes as `[x, y, z]`; the named segment resolves via the registry.
        let x = get_field_in_component_json(&component, type_path, "translation.x", &registry);
        assert_eq!(x, Some(&serde_json::json!(1.0)));

        let translation =
            get_field_in_component_json(&component, type_path, "translation", &registry);
        assert_eq!(translation, Some(&serde_json::json!([1.0, 2.0, 3.0])));
    }

    #[test]
    fn get_field_empty_path_returns_whole_component() {
        let mut registry = TypeRegistry::new();
        registry.register::<Transform>();

        let type_path = "bevy_transform::components::transform::Transform";
        let component = to_canonical_json(&Transform::IDENTITY, &registry);

        let result = get_field_in_component_json(&component, type_path, "", &registry);
        assert_eq!(result, Some(&component));
    }

    #[test]
    fn get_field_missing_path_returns_none() {
        let mut registry = TypeRegistry::new();
        registry.register::<Transform>();

        let type_path = "bevy_transform::components::transform::Transform";
        let component = to_canonical_json(&Transform::IDENTITY, &registry);

        let result =
            get_field_in_component_json(&component, type_path, "does_not_exist", &registry);
        assert!(result.is_none());
    }

    #[test]
    fn get_field_unregistered_type_returns_none() {
        let registry = TypeRegistry::new();
        let component = serde_json::json!({ "translation": [0.0, 0.0, 0.0] });

        let result = get_field_in_component_json(
            &component,
            "not::A::RegisteredType",
            "translation",
            &registry,
        );
        assert!(result.is_none());
    }

    #[test]
    fn get_field_round_trips_with_set_field() {
        let mut registry = TypeRegistry::new();
        registry.register::<Transform>();

        let type_path = "bevy_transform::components::transform::Transform";
        let mut component = to_canonical_json(&Transform::IDENTITY, &registry);

        set_field_in_component_json(
            &mut component,
            type_path,
            "translation.y",
            serde_json::json!(7.0),
            &registry,
        );

        let result = get_field_in_component_json(&component, type_path, "translation.y", &registry);
        assert_eq!(result, Some(&serde_json::json!(7.0)));
    }

    // Local enum for the enum-unwrap test. Mirrors the `ColliderConstructor`
    // pattern referenced in `typed_json_path_get`.
    #[derive(Reflect, Clone)]
    enum TestShape {
        Sphere { radius: f32 },
        Box { half_x: f32, half_y: f32 },
    }

    // Local struct with a list field for the bracket-index test.
    #[derive(Reflect, Clone)]
    struct TestList {
        items: Vec<f32>,
    }

    #[test]
    fn get_field_unwraps_enum_variant_and_reads_field() {
        let mut registry = TypeRegistry::new();
        registry.register::<TestShape>();
        registry.register::<f32>();

        let type_path = "jackdaw_jsn::ast::tests::TestShape";
        // Bevy's reflect serializer emits struct-variants as `{"Sphere": {"radius": 1.0}}`.
        let component = serde_json::json!({ "Sphere": { "radius": 1.0_f32 } });

        let result = get_field_in_component_json(&component, type_path, "radius", &registry);
        assert_eq!(result, Some(&serde_json::json!(1.0_f32)));
    }

    // Newtype tuple variant wrapping a struct, mirroring the
    // `Modifier::Mirror(MeshMirror)` shape the modifier stack uses. Bevy
    // serializes it flattened: `{"Wrap": {"flag": false, ...}}`.
    #[derive(Reflect, Clone)]
    enum TestModifier {
        Wrap(TestInner),
    }

    #[derive(Reflect, Clone)]
    struct TestInner {
        flag: bool,
        amount: f32,
    }

    #[test]
    fn newtype_variant_field_round_trips_through_index_zero() {
        let mut registry = TypeRegistry::new();
        registry.register::<TestModifier>();
        registry.register::<TestInner>();
        registry.register::<bool>();
        registry.register::<f32>();

        let type_path = "jackdaw_jsn::ast::tests::TestModifier";
        let mut component = to_canonical_json(
            &TestModifier::Wrap(TestInner {
                flag: false,
                amount: 1.5,
            }),
            &registry,
        );

        // Reading the inner field through `.0` resolves the flattened newtype.
        let read = get_field_in_component_json(&component, type_path, "0.flag", &registry);
        assert_eq!(read, Some(&serde_json::json!(false)));

        // Writing through `.0` sets the inner field and reads back the new
        // value (the bug: this used to be a silent no-op so the edit reverted).
        set_field_in_component_json(
            &mut component,
            type_path,
            "0.flag",
            serde_json::json!(true),
            &registry,
        );
        let read_back = get_field_in_component_json(&component, type_path, "0.flag", &registry);
        assert_eq!(read_back, Some(&serde_json::json!(true)));

        // The sibling field is untouched.
        let amount = get_field_in_component_json(&component, type_path, "0.amount", &registry);
        assert_eq!(amount, Some(&serde_json::json!(1.5_f32)));
    }

    #[test]
    fn get_field_bracket_index_reads_list_element() {
        let mut registry = TypeRegistry::new();
        registry.register::<TestList>();
        registry.register::<Vec<f32>>();
        registry.register::<f32>();

        let type_path = "jackdaw_jsn::ast::tests::TestList";
        let component = to_canonical_json(
            &TestList {
                items: vec![10.0, 20.0, 30.0],
            },
            &registry,
        );

        let result = get_field_in_component_json(&component, type_path, "items[1]", &registry);
        assert_eq!(result, Some(&serde_json::json!(20.0_f32)));
    }

    /// A freshly authored node gets a `JsnNodeId`, and a round-trip through
    /// `to_jsn_scene` / `from_jsn_scene` preserves each node's id.
    #[test]
    fn node_ids_survive_save_load_round_trip() {
        let mut ast = SceneJsnAst::default();
        let root = ast.add_root();
        let child = ast.add_child(root);

        let root_id = ast.nodes[root].id.expect("root node should have an id");
        let child_id = ast.nodes[child].id.expect("child node should have an id");
        assert_ne!(root_id, child_id, "minted ids must be distinct");

        let scene = ast.to_jsn_scene(JsnMetadata::default());
        assert_eq!(scene.scene[root].id, Some(root_id.0));
        assert_eq!(scene.scene[child].id, Some(child_id.0));

        let reloaded = SceneJsnAst::from_jsn_scene(&scene, &[]);
        assert_eq!(reloaded.nodes[root].id, Some(root_id));
        assert_eq!(reloaded.nodes[child].id, Some(child_id));
    }

    /// Reparenting a node keeps its id; only the parent pointer changes.
    #[test]
    fn reparent_keeps_node_id() {
        let mut ast = SceneJsnAst::default();
        let a = ast.add_root();
        let b = ast.add_root();
        let original = ast.nodes[b].id.expect("node should have an id");

        ast.nodes[b].parent = Some(a);

        assert_eq!(ast.nodes[b].id, Some(original));
    }

    /// `node_index_by_id` finds the node carrying a given stable id and
    /// `entity_for_node_id` returns the preview entity bound to it.
    #[test]
    fn node_lookup_by_stable_id() {
        let mut ast = SceneJsnAst::default();
        let a = ast.add_root();
        let b = ast.add_root();
        let a_id = ast.nodes[a].id.expect("node should have an id");
        let b_id = ast.nodes[b].id.expect("node should have an id");

        // Bind a preview entity to node `b` only.
        let b_entity = Entity::from_raw_u32(7).expect("valid entity");
        ast.nodes[b].ecs_entity = Some(b_entity);
        ast.ecs_to_jsn.insert(b_entity, b);

        assert_eq!(ast.node_index_by_id(a_id), Some(a));
        assert_eq!(ast.node_index_by_id(b_id), Some(b));
        assert_eq!(ast.entity_for_node_id(b_id), Some(b_entity));
        // Node `a` has no preview entity, so there is nothing to return.
        assert_eq!(ast.entity_for_node_id(a_id), None);
    }

    /// An id that no node carries resolves to nothing for either lookup.
    #[test]
    fn node_lookup_missing_id_is_none() {
        let mut ast = SceneJsnAst::default();
        ast.add_root();
        let absent = JsnNodeId::next();
        assert_eq!(ast.node_index_by_id(absent), None);
        assert_eq!(ast.entity_for_node_id(absent), None);
    }

    /// Promoting runtime values into an authored node (the PIE "save to
    /// scene" path): given a node with a stable id and a preview entity,
    /// writing a map of component values through `set_component` leaves the
    /// node's `components` holding exactly those values and marks it dirty.
    #[test]
    fn promote_runtime_components_into_node_by_id() {
        let mut ast = SceneJsnAst::default();
        let node = ast.add_root();
        let node_id = ast.nodes[node].id.expect("node should have an id");
        // Seed a stale authored value to prove the promote overwrites it.
        ast.nodes[node].components.insert(
            "bevy_transform::components::transform::Transform".to_string(),
            serde_json::json!({ "translation": [0.0, 0.0, 0.0] }),
        );

        let entity = Entity::from_raw_u32(3).expect("valid entity");
        ast.nodes[node].ecs_entity = Some(entity);
        ast.ecs_to_jsn.insert(entity, node);

        // Runtime snapshot keyed by type path, as the PIE mirror stores it.
        let runtime: Vec<(String, serde_json::Value)> = vec![
            (
                "bevy_transform::components::transform::Transform".to_string(),
                serde_json::json!({ "translation": [1.0, 2.0, 3.0] }),
            ),
            (
                "game::Health".to_string(),
                serde_json::json!({ "current": 42 }),
            ),
        ];

        let target = ast.entity_for_node_id(node_id).expect("preview entity");
        for (type_path, value) in &runtime {
            ast.set_component(target, type_path, value.clone());
        }

        let promoted = &ast.nodes[node].components;
        assert_eq!(
            promoted["bevy_transform::components::transform::Transform"],
            serde_json::json!({ "translation": [1.0, 2.0, 3.0] }),
        );
        assert_eq!(
            promoted["game::Health"],
            serde_json::json!({ "current": 42 })
        );
        assert!(
            ast.dirty_indices.contains(&node),
            "promote must mark the node dirty so the preview ECS re-syncs"
        );
    }

    /// Build a `JsnScene` from `(id, parent)` pairs. `id == None` omits the
    /// structural id; `parent == None` omits the parent pointer.
    fn scene_from_nodes(nodes: &[(Option<u64>, Option<usize>)]) -> JsnScene {
        let scene: Vec<_> = nodes
            .iter()
            .map(|(id, parent)| {
                let mut e = serde_json::json!({ "components": {} });
                if let Some(id) = id {
                    e["id"] = serde_json::json!(id);
                }
                if let Some(parent) = parent {
                    e["parent"] = serde_json::json!(parent);
                }
                e
            })
            .collect();
        let json = serde_json::json!({
            "jsn": { "format_version": [3, 0, 0], "editor_version": "test", "bevy_version": "0.18" },
            "metadata": { "name": "t" },
            "assets": {},
            "editor": null,
            "scene": scene,
        });
        serde_json::from_value(json).expect("scene should parse")
    }

    #[test]
    fn needs_migration_detects_duplicate_ids() {
        let scene = scene_from_nodes(&[(Some(SPARSE_MIN), None), (Some(SPARSE_MIN), None)]);
        assert!(
            needs_id_migration(&scene),
            "two equal ids must trigger migration"
        );
    }

    #[test]
    fn needs_migration_detects_legacy_low_ids() {
        let scene = scene_from_nodes(&[(Some(9), None), (Some(10), None)]);
        assert!(
            needs_id_migration(&scene),
            "ids below SPARSE_MIN must trigger migration"
        );
    }

    #[test]
    fn needs_migration_detects_missing_id() {
        let scene = scene_from_nodes(&[(None, None)]);
        assert!(
            needs_id_migration(&scene),
            "a node with no id must trigger migration"
        );
    }

    #[test]
    fn needs_migration_false_for_sparse_unique() {
        let scene = scene_from_nodes(&[(Some(SPARSE_MIN), None), (Some(SPARSE_MIN + 1), None)]);
        assert!(
            !needs_id_migration(&scene),
            "distinct sparse ids need no migration"
        );
    }

    /// Minted ids land in the sparse range and never repeat back-to-back.
    #[test]
    fn minted_ids_are_sparse_and_unique() {
        let a = JsnNodeId::next();
        let b = JsnNodeId::next();
        assert_ne!(a, b, "successive mints must differ");
        assert!(a.0 >= SPARSE_MIN, "minted id must be in the sparse range");
        assert!(b.0 >= SPARSE_MIN, "minted id must be in the sparse range");
    }

    /// Colliding ids are healed to unique sparse ids on load.
    #[test]
    fn from_jsn_scene_dedupes_colliding_ids() {
        let scene = scene_from_nodes(&[(Some(10), None), (Some(10), None), (Some(10), None)]);
        let ast = SceneJsnAst::from_jsn_scene(&scene, &[]);
        let ids: Vec<u64> = ast
            .nodes
            .iter()
            .map(|n| n.id.expect("healed id").0)
            .collect();
        let unique: HashSet<u64> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len(), "healed ids must be unique");
        assert!(
            ids.iter().all(|id| *id >= SPARSE_MIN),
            "healed ids must be sparse"
        );
    }

    /// Unique but legacy-low ids are lifted into the sparse range on load.
    #[test]
    fn from_jsn_scene_remints_legacy_low_ids() {
        let scene = scene_from_nodes(&[(Some(1), None), (Some(2), None)]);
        let ast = SceneJsnAst::from_jsn_scene(&scene, &[]);
        assert!(
            ast.nodes
                .iter()
                .all(|n| n.id.expect("healed id").0 >= SPARSE_MIN)
        );
    }

    /// A scene whose ids are already sparse and unique loads untouched.
    #[test]
    fn from_jsn_scene_preserves_sparse_unique_ids() {
        let scene = scene_from_nodes(&[(Some(SPARSE_MIN + 5), None), (Some(SPARSE_MIN + 6), None)]);
        let ast = SceneJsnAst::from_jsn_scene(&scene, &[]);
        assert_eq!(ast.nodes[0].id, Some(JsnNodeId(SPARSE_MIN + 5)));
        assert_eq!(ast.nodes[1].id, Some(JsnNodeId(SPARSE_MIN + 6)));
    }

    /// Re-minting changes ids but leaves the index-based parent links intact.
    #[test]
    fn from_jsn_scene_remint_preserves_parent_links() {
        let scene = scene_from_nodes(&[(Some(10), None), (Some(10), Some(0))]);
        let ast = SceneJsnAst::from_jsn_scene(&scene, &[]);
        assert_eq!(
            ast.nodes[1].parent,
            Some(0),
            "parent index survives re-mint"
        );
        assert_ne!(
            ast.nodes[0].id, ast.nodes[1].id,
            "ids are unique after heal"
        );
    }

    /// A legacy `JsnScene` with no `id` field still loads, minting fresh ids
    /// for every node that lacks one.
    #[test]
    fn legacy_scene_without_id_mints_fresh_ids() {
        let json = serde_json::json!({
            "jsn": {
                "format_version": [3, 0, 0],
                "editor_version": "test",
                "bevy_version": "0.18"
            },
            "metadata": { "name": "legacy" },
            "assets": {},
            "editor": null,
            "scene": [
                { "components": {} },
                { "parent": 0, "components": {} }
            ]
        });
        let scene: JsnScene = serde_json::from_value(json).expect("legacy scene should parse");
        assert_eq!(scene.scene[0].id, None, "legacy entity has no on-disk id");

        let ast = SceneJsnAst::from_jsn_scene(&scene, &[]);
        let id0 = ast.nodes[0]
            .id
            .expect("legacy node 0 should be minted an id");
        let id1 = ast.nodes[1]
            .id
            .expect("legacy node 1 should be minted an id");
        assert_ne!(id0, id1, "minted ids must be distinct");
    }
}
