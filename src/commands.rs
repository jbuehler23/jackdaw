use std::any::TypeId;

use bevy::{
    ecs::{
        component::ComponentId,
        reflect::{AppTypeRegistry, ReflectComponent},
    },
    prelude::*,
};
use serde::de::DeserializeSeed;

// Re-export the core command framework from the jackdaw_commands crate
pub use jackdaw_commands::{CommandGroup, CommandHistory, EditorCommand};

use crate::EditorEntity;
use crate::selection::{Selected, Selection};

pub struct CommandHistoryPlugin;

impl Plugin for CommandHistoryPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(CommandHistory::default());
    }
}

pub struct SetComponentField {
    pub entity: Entity,
    pub component_type_id: TypeId,
    pub field_path: String,
    pub old_value: Box<dyn PartialReflect>,
    pub new_value: Box<dyn PartialReflect>,
}

impl EditorCommand for SetComponentField {
    fn execute(&mut self, world: &mut World) {
        apply_reflected_value(
            world,
            self.entity,
            self.component_type_id,
            &self.field_path,
            &*self.new_value,
        );
    }

    fn undo(&mut self, world: &mut World) {
        apply_reflected_value(
            world,
            self.entity,
            self.component_type_id,
            &self.field_path,
            &*self.old_value,
        );
    }

    fn description(&self) -> &str {
        "Set component field"
    }
}

fn apply_reflected_value(
    world: &mut World,
    entity: Entity,
    component_type_id: TypeId,
    field_path: &str,
    value: &dyn PartialReflect,
) {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();

    let Some(registration) = registry.get(component_type_id) else {
        return;
    };
    let Some(reflect_component) = registration.data::<ReflectComponent>() else {
        return;
    };

    let Some(reflected) = reflect_component.reflect_mut(world.entity_mut(entity)) else {
        return;
    };

    if field_path.is_empty() {
        // Apply to the entire component (e.g. a top-level enum component)
        reflected.into_inner().apply(value);
    } else {
        let Ok(field) = reflected.into_inner().reflect_path_mut(field_path) else {
            return;
        };
        field.apply(value);
    }
}

pub struct SetTransform {
    pub entity: Entity,
    pub old_transform: Transform,
    pub new_transform: Transform,
}

impl EditorCommand for SetTransform {
    fn execute(&mut self, world: &mut World) {
        if let Some(mut transform) = world.get_mut::<Transform>(self.entity) {
            *transform = self.new_transform;
        }
        sync_component_to_ast::<Transform>(
            world,
            self.entity,
            "bevy_transform::components::transform::Transform",
            &self.new_transform,
        );
    }

    fn undo(&mut self, world: &mut World) {
        if let Some(mut transform) = world.get_mut::<Transform>(self.entity) {
            *transform = self.old_transform;
        }
        sync_component_to_ast::<Transform>(
            world,
            self.entity,
            "bevy_transform::components::transform::Transform",
            &self.old_transform,
        );
    }

    fn description(&self) -> &str {
        "Set transform"
    }
}

pub struct ReparentEntity {
    pub entity: Entity,
    pub old_parent: Option<Entity>,
    pub new_parent: Option<Entity>,
}

impl EditorCommand for ReparentEntity {
    fn execute(&mut self, world: &mut World) {
        set_parent(world, self.entity, self.new_parent);
    }

    fn undo(&mut self, world: &mut World) {
        set_parent(world, self.entity, self.old_parent);
    }

    fn description(&self) -> &str {
        "Reparent entity"
    }
}

fn set_parent(world: &mut World, entity: Entity, parent: Option<Entity>) {
    // Preserve world position across reparent. Compute the entity's current
    // world transform, then update its local Transform so that:
    //   new_parent_global * new_local = current_world
    // This prevents the brush from "jumping" (or disappearing off-screen)
    // when parented under an entity at a non-origin position.
    let current_world = world.get::<GlobalTransform>(entity).copied();
    let new_parent_world = parent.and_then(|p| world.get::<GlobalTransform>(p).copied());

    match parent {
        Some(p) => {
            world.entity_mut(entity).insert(ChildOf(p));
        }
        None => {
            world.entity_mut(entity).remove::<ChildOf>();
        }
    }

    let new_transform =
        if let (Some(world_tf), Some(parent_world)) = (current_world, new_parent_world) {
            Some(Transform::from_matrix(
                (parent_world.affine().inverse() * world_tf.affine()).into(),
            ))
        } else if parent.is_none() {
            current_world.map(|w| Transform::from_matrix(w.affine().into()))
        } else {
            None
        };
    if let Some(new_tf) = new_transform
        && let Some(mut tf) = world.get_mut::<Transform>(entity)
    {
        *tf = new_tf;
    }

    // Update AST parent and (if changed) Transform
    let parent_idx = {
        let ast = world.resource::<jackdaw_jsn::SceneJsnAst>();
        parent.and_then(|p| ast.ecs_to_jsn.get(&p).copied())
    };
    {
        let mut ast = world.resource_mut::<jackdaw_jsn::SceneJsnAst>();
        if let Some(node) = ast.node_for_entity_mut(entity) {
            node.parent = parent_idx;
        }
    }
    if let Some(new_tf) = new_transform {
        sync_component_to_ast(
            world,
            entity,
            "bevy_transform::components::transform::Transform",
            &new_tf,
        );
    }
}

pub struct AddComponent {
    pub entity: Entity,
    pub type_id: TypeId,
    pub component_id: ComponentId,
    pub type_path: String,
    /// Type paths of components that were auto-promoted to the AST via
    /// `#[require]` during `execute`. Cleaned up on `undo`.
    promoted_components: Vec<String>,
}

impl AddComponent {
    pub fn new(
        entity: Entity,
        type_id: TypeId,
        component_id: ComponentId,
        type_path: String,
    ) -> Self {
        Self {
            entity,
            type_id,
            component_id,
            type_path,
            promoted_components: Vec::new(),
        }
    }
}

impl EditorCommand for AddComponent {
    fn execute(&mut self, world: &mut World) {
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();

        let Some(registration) = registry.get(self.type_id) else {
            return;
        };

        // Create default value
        let Some(reflect_default) = registration.data::<ReflectDefault>() else {
            warn!("No ReflectDefault for component  -- cannot add");
            return;
        };
        let default_value = reflect_default.default();
        let Some(reflect_component) = registration.data::<ReflectComponent>() else {
            return;
        };

        // Insert the component  -- this triggers #[require] which may add
        // many more components (e.g. RigidBody requires Position, Rotation,
        // LinearVelocity, etc.).
        reflect_component.insert(
            &mut world.entity_mut(self.entity),
            default_value.as_partial_reflect(),
            &registry,
        );

        // Sync the explicitly-added component to AST
        let serializer =
            bevy::reflect::serde::TypedReflectSerializer::new(default_value.as_ref(), &registry);
        if let Ok(json_value) = serde_json::to_value(&serializer) {
            drop(registry);
            world
                .resource_mut::<jackdaw_jsn::SceneJsnAst>()
                .set_component(self.entity, &self.type_path, json_value);
        }

        // Sync any components added by #[require] to the AST so they're
        // editable and persist with the scene. This captures avian physics
        // internals, required transform components, etc.
        self.promoted_components = sync_required_to_ast(world, self.entity);
    }

    fn undo(&mut self, world: &mut World) {
        // Resolve promoted components' ComponentIds via the type registry
        // so we can remove them from the ECS as well as the AST.
        let registry = world.resource::<AppTypeRegistry>().clone();
        let reg = registry.read();
        let promoted_component_ids: Vec<bevy::ecs::component::ComponentId> = self
            .promoted_components
            .iter()
            .filter_map(|type_path| {
                let type_id = reg.get_with_type_path(type_path)?.type_id();
                world.components().get_id(type_id)
            })
            .collect();
        drop(reg);

        if let Ok(mut entity) = world.get_entity_mut(self.entity) {
            entity.remove_by_id(self.component_id);
            for cid in &promoted_component_ids {
                entity.remove_by_id(*cid);
            }
        }
        // Remove the explicitly-added component + all promoted components from AST
        if let Some(node) = world
            .resource_mut::<jackdaw_jsn::SceneJsnAst>()
            .node_for_entity_mut(self.entity)
        {
            node.components.remove(&self.type_path);
            node.derived_components.remove(&self.type_path);
            for promoted in &self.promoted_components {
                node.components.remove(promoted);
                node.derived_components.remove(promoted);
            }
        }
        // Trigger inspector rebuild so the UI reflects the removal immediately.
        if let Ok(mut ec) = world.get_entity_mut(self.entity) {
            ec.insert(crate::inspector::InspectorDirty);
        }
    }

    fn description(&self) -> &str {
        "Add component"
    }
}

pub struct RemoveComponent {
    pub entity: Entity,
    pub type_id: TypeId,
    pub component_id: ComponentId,
    pub type_path: String,
    /// Snapshot of the component's value before removal, for undo.
    pub snapshot: Box<dyn PartialReflect>,
    /// AST snapshot for undo.
    pub ast_snapshot: Option<serde_json::Value>,
}

impl EditorCommand for RemoveComponent {
    fn execute(&mut self, world: &mut World) {
        // Snapshot from AST before removal
        self.ast_snapshot = world
            .resource::<jackdaw_jsn::SceneJsnAst>()
            .get_component(self.entity, &self.type_path)
            .cloned();
        if let Ok(mut entity) = world.get_entity_mut(self.entity) {
            entity.remove_by_id(self.component_id);
        }
        // Remove from AST
        if let Some(node) = world
            .resource_mut::<jackdaw_jsn::SceneJsnAst>()
            .node_for_entity_mut(self.entity)
        {
            node.components.remove(&self.type_path);
        }
    }

    fn undo(&mut self, world: &mut World) {
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();

        let Some(registration) = registry.get(self.type_id) else {
            return;
        };
        let Some(reflect_component) = registration.data::<ReflectComponent>() else {
            return;
        };

        reflect_component.insert(
            &mut world.entity_mut(self.entity),
            &*self.snapshot,
            &registry,
        );
        drop(registry);

        // Restore AST snapshot
        if let Some(json_value) = self.ast_snapshot.take() {
            world
                .resource_mut::<jackdaw_jsn::SceneJsnAst>()
                .set_component(self.entity, &self.type_path, json_value);
        }
    }

    fn description(&self) -> &str {
        "Remove component"
    }
}

pub struct SpawnEntity {
    /// The entity that was spawned (set after first execute).
    pub spawned: Option<Entity>,
    /// Builder function that spawns the entity and returns its Entity id.
    pub spawn_fn: Box<dyn Fn(&mut World) -> Entity + Send + Sync>,
    pub label: String,
}

impl EditorCommand for SpawnEntity {
    fn execute(&mut self, world: &mut World) {
        let entity = (self.spawn_fn)(world);
        self.spawned = Some(entity);
    }

    fn undo(&mut self, world: &mut World) {
        if let Some(entity) = self.spawned.take() {
            deselect_entities(world, &[entity]);
            world
                .resource_mut::<jackdaw_jsn::SceneJsnAst>()
                .remove_node(entity);
            if let Ok(entity_mut) = world.get_entity_mut(entity) {
                entity_mut.despawn();
            }
        }
    }

    fn description(&self) -> &str {
        &self.label
    }
}

pub struct DespawnEntity {
    pub entity: Entity,
    pub scene_snapshot: DynamicScene,
    pub parent: Option<Entity>,
    pub label: String,
}

impl DespawnEntity {
    pub fn from_world(world: &World, entity: Entity) -> Self {
        let parent = world.get::<ChildOf>(entity).map(|c| c.0);
        let scene = snapshot_entity(world, entity);
        Self {
            entity,
            scene_snapshot: scene,
            parent,
            label: format!("Despawn entity {entity}"),
        }
    }
}

impl EditorCommand for DespawnEntity {
    fn execute(&mut self, world: &mut World) {
        deselect_entities(world, &[self.entity]);
        world
            .resource_mut::<jackdaw_jsn::SceneJsnAst>()
            .remove_node(self.entity);
        if let Ok(entity_mut) = world.get_entity_mut(self.entity) {
            entity_mut.despawn();
        }
    }

    fn undo(&mut self, world: &mut World) {
        // Re-build the scene from scratch and write it back
        let scene = snapshot_rebuild(&self.scene_snapshot);
        let mut entity_map = bevy::ecs::entity::hash_map::EntityHashMap::default();
        let _ = scene.write_to_world(world, &mut entity_map);
        if let Some(&new_id) = entity_map.get(&self.entity) {
            self.entity = new_id;
        }
        crate::scene_io::register_entity_in_ast(world, self.entity);
    }

    fn description(&self) -> &str {
        &self.label
    }
}

/// Create a `DynamicSceneBuilder` that excludes computed components which become
/// stale when restored (Children references dead mesh entities, visibility flags
/// block rendering).
pub(crate) fn filtered_scene_builder(world: &World) -> DynamicSceneBuilder<'_> {
    DynamicSceneBuilder::from_world(world)
        .deny_component::<Children>()
        .deny_component::<GlobalTransform>()
        .deny_component::<InheritedVisibility>()
        .deny_component::<ViewVisibility>()
}

/// Deselect the given entities: remove the `Selected` component and purge them
/// from the `Selection` resource.  Must be called **before** despawning so that
/// observers can clean up tree-row UI while the entities still exist.
pub(crate) fn deselect_entities(world: &mut World, entities: &[Entity]) {
    for &entity in entities {
        if let Ok(mut ec) = world.get_entity_mut(entity) {
            ec.remove::<Selected>();
        }
    }
    let mut selection = world.resource_mut::<Selection>();
    selection.entities.retain(|e| !entities.contains(e));
}

/// Create a `DynamicScene` snapshot of a single entity and all its descendants.
pub(crate) fn snapshot_entity(world: &World, entity: Entity) -> DynamicScene {
    let mut entities = Vec::new();
    collect_entity_ids(world, entity, &mut entities);
    filtered_scene_builder(world)
        .extract_entities(entities.into_iter())
        .build()
}

pub(crate) fn collect_entity_ids(world: &World, entity: Entity, out: &mut Vec<Entity>) {
    out.push(entity);
    if let Some(children) = world.get::<Children>(entity) {
        for child in children.iter() {
            // Skip editor-only entities and runtime-generated children
            // (e.g. BrushFaceEntity meshes). Including NonSerializable
            // children causes them to be restored as orphans at origin
            // after undo, while the parent regenerates its own.
            if world.get::<EditorEntity>(child).is_some()
                || world.get::<crate::NonSerializable>(child).is_some()
            {
                continue;
            }
            collect_entity_ids(world, child, out);
        }
    }
}

/// Rebuild a `DynamicScene` by copying its entity data (since `DynamicScene` doesn't impl Clone).
pub(crate) fn snapshot_rebuild(scene: &DynamicScene) -> DynamicScene {
    DynamicScene {
        resources: scene.resources.iter().map(|r| r.to_dynamic()).collect(),
        entities: scene
            .entities
            .iter()
            .map(|e| bevy::scene::DynamicEntity {
                entity: e.entity,
                components: e.components.iter().map(|c| c.to_dynamic()).collect(),
            })
            .collect(),
    }
}

// ─────────────────────────────────── JSN-First Commands ───────────────────────────────────

pub struct SetJsnField {
    pub entity: Entity,
    pub type_path: String,
    pub field_path: String,
    pub old_value: serde_json::Value,
    pub new_value: serde_json::Value,
    /// True if the component was in the `derived_components` set before this
    /// command ran. Set on first execute so undo can demote the component back.
    pub was_derived: bool,
}

impl EditorCommand for SetJsnField {
    fn execute(&mut self, world: &mut World) {
        {
            let registry = world.resource::<AppTypeRegistry>().clone();
            let registry = registry.read();
            let mut ast = world.resource_mut::<jackdaw_jsn::SceneJsnAst>();
            ast.set_component_field(
                self.entity,
                &self.type_path,
                &self.field_path,
                self.new_value.clone(),
                &registry,
            );
            // If the user explicitly edits a derived component, promote it to
            // "authored" so the change persists on save. Remember we did so,
            // so undo can restore the derived state.
            if let Some(node) = ast.node_for_entity_mut(self.entity)
                && node.derived_components.remove(&self.type_path)
            {
                self.was_derived = true;
                info!(
                    "Promoted derived component '{}' to authored (user edited it)",
                    self.type_path
                );
            }
        }
        apply_jsn_field_to_ecs(
            world,
            self.entity,
            &self.type_path,
            &self.field_path,
            &self.new_value,
        );
    }

    fn undo(&mut self, world: &mut World) {
        {
            let registry = world.resource::<AppTypeRegistry>().clone();
            let registry = registry.read();
            let mut ast = world.resource_mut::<jackdaw_jsn::SceneJsnAst>();
            ast.set_component_field(
                self.entity,
                &self.type_path,
                &self.field_path,
                self.old_value.clone(),
                &registry,
            );
            // Restore derived state if execute promoted it to authored.
            if self.was_derived
                && let Some(node) = ast.node_for_entity_mut(self.entity)
            {
                node.derived_components.insert(self.type_path.clone());
            }
        }
        apply_jsn_field_to_ecs(
            world,
            self.entity,
            &self.type_path,
            &self.field_path,
            &self.old_value,
        );
    }

    fn description(&self) -> &str {
        "Set component field"
    }
}

/// Apply a JSON value to an ECS component  -- either full component replacement
/// (empty `field_path`) or field-level update.
fn apply_jsn_field_to_ecs(
    world: &mut World,
    entity: Entity,
    type_path: &str,
    field_path: &str,
    value: &serde_json::Value,
) {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();

    let Some(registration) = registry.get_with_type_path(type_path) else {
        return;
    };
    let Some(reflect_component) = registration.data::<ReflectComponent>() else {
        return;
    };

    if field_path.is_empty() {
        // Full component replacement via TypedReflectDeserializer.
        // Always use `insert` (not `apply`)  -- this handles:
        //  - Immutable components like RigidBody (apply panics on immutable)
        //  - Components removed externally (e.g. avian removing ColliderConstructor)
        //  - Normal mutable components (insert replaces in-place)
        let deserializer =
            bevy::reflect::serde::TypedReflectDeserializer::new(registration, &registry);
        if let Ok(reflected) = deserializer.deserialize(value) {
            reflect_component.insert(&mut world.entity_mut(entity), reflected.as_ref(), &registry);
        }
    } else {
        // Field-level update via reflect_path_mut
        let Some(reflected) = reflect_component.reflect_mut(world.entity_mut(entity)) else {
            return;
        };
        if let Ok(field) = reflected.into_inner().reflect_path_mut(field_path) {
            apply_json_to_reflect(field, value, &registry);
        }
    }
}

/// Convert a `serde_json::Value` into the matching reflect primitive and apply it.
/// Falls back to Bevy's typed deserialization for complex types (enums, structs)
/// that can't be handled by simple primitive downcasts.
fn apply_json_to_reflect(
    field: &mut dyn bevy::reflect::PartialReflect,
    value: &serde_json::Value,
    registry: &bevy::reflect::TypeRegistry,
) {
    match value {
        serde_json::Value::Number(n) => {
            if let Some(f) = field.try_downcast_mut::<f32>() {
                *f = n.as_f64().unwrap_or_default() as f32;
            } else if let Some(f) = field.try_downcast_mut::<f64>() {
                *f = n.as_f64().unwrap_or_default();
            } else if let Some(i) = field.try_downcast_mut::<i32>() {
                *i = n.as_i64().unwrap_or_default() as i32;
            } else if let Some(i) = field.try_downcast_mut::<u32>() {
                *i = n.as_u64().unwrap_or_default() as u32;
            } else if let Some(i) = field.try_downcast_mut::<usize>() {
                *i = n.as_u64().unwrap_or_default() as usize;
            } else if let Some(i) = field.try_downcast_mut::<i8>() {
                *i = n.as_i64().unwrap_or_default() as i8;
            } else if let Some(i) = field.try_downcast_mut::<i16>() {
                *i = n.as_i64().unwrap_or_default() as i16;
            } else if let Some(i) = field.try_downcast_mut::<i64>() {
                *i = n.as_i64().unwrap_or_default();
            } else if let Some(i) = field.try_downcast_mut::<u8>() {
                *i = n.as_u64().unwrap_or_default() as u8;
            } else if let Some(i) = field.try_downcast_mut::<u16>() {
                *i = n.as_u64().unwrap_or_default() as u16;
            } else if let Some(i) = field.try_downcast_mut::<u64>() {
                *i = n.as_u64().unwrap_or_default();
            }
        }
        serde_json::Value::Bool(b) => {
            if let Some(f) = field.try_downcast_mut::<bool>() {
                *f = *b;
            }
        }
        serde_json::Value::String(s) => {
            if let Some(f) = field.try_downcast_mut::<String>() {
                *f = s.clone();
                return;
            }
            // Unit enum variants serialize as a bare string  -- fall through to the
            // typed-deserializer path below.
            try_typed_deserialize(field, value, registry);
        }
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
            // Structs, tuple structs, enum struct/tuple variants, lists, etc.
            try_typed_deserialize(field, value, registry);
        }
        serde_json::Value::Null => {}
    }
}

/// Look up the field's `TypeRegistration` via its represented type info and run
/// `TypedReflectDeserializer` on the JSON, then apply the result.
fn try_typed_deserialize(
    field: &mut dyn bevy::reflect::PartialReflect,
    value: &serde_json::Value,
    registry: &bevy::reflect::TypeRegistry,
) {
    let Some(type_info) = field.get_represented_type_info() else {
        return;
    };
    let Some(registration) = registry.get(type_info.type_id()) else {
        return;
    };
    let deserializer = bevy::reflect::serde::TypedReflectDeserializer::new(registration, registry);
    if let Ok(reflected) = deserializer.deserialize(value) {
        field.apply(reflected.as_ref());
    }
}

/// Serialize a component to JSON and store it in the AST.
pub fn sync_component_to_ast<T: bevy::reflect::Reflect>(
    world: &mut World,
    entity: Entity,
    type_path: &str,
    value: &T,
) {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let processor = crate::scene_io::AstSerializerProcessor;
    let serializer =
        bevy::reflect::serde::TypedReflectSerializer::with_processor(value, &registry, &processor);
    if let Ok(json_value) = serde_json::to_value(&serializer) {
        drop(registry);
        world
            .resource_mut::<jackdaw_jsn::SceneJsnAst>()
            .set_component(entity, type_path, json_value);
    }
}

/// Scan an entity for reflected components that exist in the ECS but not yet
/// in the JSN AST, and serialize them into the AST.
///
/// This captures components added implicitly by Bevy's `#[require]`
/// attributes (e.g., `RigidBody` requiring `Position`, `Rotation`,
/// `LinearVelocity`, etc.). After this call, those components are editable
/// in the inspector via the normal `SetJsnField` path and persist with
/// scene save/load.
///
/// Designed to be upstream-compatible with BSN  -- the AST becomes the full
/// authoritative representation, not just the user's explicit additions.
///
/// Returns the type paths of newly-promoted components (for undo cleanup).
pub fn sync_required_to_ast(world: &mut World, entity: Entity) -> Vec<String> {
    use std::collections::HashSet;

    let registry = world.resource::<AppTypeRegistry>().clone();
    let reg = registry.read();

    // Snapshot what's currently in the AST for this entity
    let existing: HashSet<String> = world
        .resource::<jackdaw_jsn::SceneJsnAst>()
        .node_for_entity(entity)
        .map(|n| n.components.keys().cloned().collect())
        .unwrap_or_default();

    let skip_ids: HashSet<TypeId> = HashSet::from([
        TypeId::of::<GlobalTransform>(),
        TypeId::of::<InheritedVisibility>(),
        TypeId::of::<ViewVisibility>(),
        TypeId::of::<ChildOf>(),
        TypeId::of::<Children>(),
    ]);

    let processor = crate::scene_io::AstSerializerProcessor;
    let Ok(entity_ref) = world.get_entity(entity) else {
        return vec![];
    };

    // Collect serializable components not yet in the AST
    let mut to_add: Vec<(String, serde_json::Value)> = Vec::new();

    for registration in reg.iter() {
        if skip_ids.contains(&registration.type_id()) {
            continue;
        }
        let type_path = registration
            .type_info()
            .type_path_table()
            .path()
            .to_string();
        if existing.contains(&type_path) {
            continue;
        }
        if crate::scene_io::should_skip_component(&type_path) {
            continue;
        }
        let Some(reflect_component) = registration.data::<ReflectComponent>() else {
            continue;
        };
        let Some(component) = reflect_component.reflect(entity_ref) else {
            continue;
        };
        let serializer = bevy::reflect::serde::TypedReflectSerializer::with_processor(
            component, &reg, &processor,
        );
        if let Ok(value) = serde_json::to_value(&serializer) {
            to_add.push((type_path, value));
        }
    }

    drop(reg);

    let promoted: Vec<String> = to_add.iter().map(|(path, _)| path.clone()).collect();

    if !promoted.is_empty() {
        info!(
            "sync_required_to_ast: {} derived components promoted for entity {entity}",
            promoted.len()
        );
        let mut ast = world.resource_mut::<jackdaw_jsn::SceneJsnAst>();
        for (type_path, value) in to_add {
            ast.set_component(entity, &type_path, value);
        }
        // Mark as derived  -- displayed in inspector but NOT persisted on save.
        if let Some(node) = ast.node_for_entity_mut(entity) {
            for path in &promoted {
                node.derived_components.insert(path.clone());
            }
        }
    }

    promoted
}
