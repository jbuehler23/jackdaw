use std::collections::{HashMap, HashSet};

use bevy::reflect::{TypeRegistry, UnnamedField};
use bevy::{prelude::*, reflect::NamedField};

use crate::format::{JsnAssets, JsnEntity, JsnMetadata, JsnScene};

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
        let nodes = scene
            .scene
            .iter()
            .enumerate()
            .map(|(i, jsn)| {
                let ecs_entity = entity_map.get(i).copied();
                if let Some(e) = ecs_entity {
                    ecs_to_jsn.insert(e, i);
                }
                JsnEntityNode {
                    parent: jsn.parent,
                    components: jsn.components.clone(),
                    derived_components: HashSet::new(),
                    ecs_entity,
                }
            })
            .collect();

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
        let registration = registry.get_with_type_path(type_path)?;
        typed_json_path_get(component, field_path, registration, registry)
    }

    /// Set a nested field within a component's JSON by dotted path, marking dirty.
    /// Uses the type registry to resolve named fields to array indices.
    pub fn set_component_field(
        &mut self,
        entity: Entity,
        type_path: &str,
        field_path: &str,
        value: serde_json::Value,
        registry: &TypeRegistry,
    ) {
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
            let next_val = match inner {
                serde_json::Value::Object(_) => inner.get(segment)?,
                serde_json::Value::Array(_) => {
                    let idx: usize = segment.parse().ok()?;
                    inner.get(idx)?
                }
                _ => return None,
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
            if is_last {
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
            // Descend into the variant's field and continue.
            let field_val = match inner {
                serde_json::Value::Object(map) => map.get_mut(*segment),
                serde_json::Value::Array(arr) => match segment.parse::<usize>() {
                    Ok(idx) => arr.get_mut(idx),
                    Err(_) => return,
                },
                _ => None,
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
