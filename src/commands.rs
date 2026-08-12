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
        app.insert_resource(CommandHistory::default())
            .init_resource::<FieldEditSessions>();
    }
}

/// Key for an in-progress field-edit gesture session entry.
#[derive(Clone, PartialEq, Eq, Hash)]
struct FieldEditSessionKey {
    entity: Entity,
    type_path: String,
    field_path: String,
}

/// Live field values at [`field_edit_begin`] for in-progress gestures.
///
/// Lifecycle: [`field_edit_begin`] → [`field_edit_preview`]* →
/// [`field_edit_commit`].
///
/// Preview mutates ECS before any `SetBsnField` exists. Capturing live values
/// at begin lets commit build undo baselines for derived (no-patch) components.
#[derive(Resource, Default)]
pub(crate) struct FieldEditSessions {
    /// Live field value at gesture start, keyed by entity + field.
    live_at_begin: std::collections::HashMap<FieldEditSessionKey, jackdaw_bsn::BsnValue>,
}

fn field_edit_session_targets(world: &World) -> Vec<Entity> {
    world
        .get_resource::<Selection>()
        .map(|selection| selection.entities.clone())
        .unwrap_or_default()
}

fn document_has_component_patch(world: &World, entity: Entity, type_path: &str) -> bool {
    let ast = world.resource::<jackdaw_bsn::SceneBsnAst>();
    ast.ast_for(entity)
        .is_some_and(|node| ast.find_patch_by_type_path(node, type_path).is_some())
}

fn clear_field_edit_session(world: &mut World, type_path: &str, field_path: &str) {
    let mut sessions = world.resource_mut::<FieldEditSessions>();
    sessions
        .live_at_begin
        .retain(|key, _| key.type_path != type_path || key.field_path != field_path);
}

fn peek_live_at_begin(
    world: &World,
    entity: Entity,
    type_path: &str,
    field_path: &str,
) -> Option<jackdaw_bsn::BsnValue> {
    world
        .resource::<FieldEditSessions>()
        .live_at_begin
        .get(&FieldEditSessionKey {
            entity,
            type_path: type_path.to_string(),
            field_path: field_path.to_string(),
        })
        .cloned()
}

/// Undo baseline for one target: authored document field, else `None` when
/// the component patch exists but the field does not (sparse absence), else
/// the live value captured at begin (derived), else the current live field.
fn resolve_field_edit_old_value(
    world: &World,
    entity: Entity,
    type_path: &str,
    field_path: &str,
) -> Option<jackdaw_bsn::BsnValue> {
    if let Some(authored) = authored_bsn_field(world, entity, type_path, field_path) {
        return Some(authored);
    }
    if document_has_component_patch(world, entity, type_path) {
        // Sparse patch: field was not authored. Undo must remove it, not
        // write back the live default captured for cancel.
        return None;
    }
    peek_live_at_begin(world, entity, type_path, field_path)
        .or_else(|| live_bsn_field(world, entity, type_path, field_path))
}

/// Begin a field-edit gesture for the current selection.
///
/// Captures each target's live field value so later preview ticks can mutate
/// ECS without losing the pre-gesture baseline. Idempotent for entities that
/// already have an entry for this field.
pub(crate) fn field_edit_begin(world: &mut World, type_path: &str, field_path: &str) {
    let targets = field_edit_session_targets(world);
    if targets.is_empty() {
        return;
    }

    let already_captured: std::collections::HashSet<Entity> = world
        .resource::<FieldEditSessions>()
        .live_at_begin
        .keys()
        .filter(|key| key.type_path == type_path && key.field_path == field_path)
        .map(|key| key.entity)
        .collect();

    let mut to_capture: Vec<(Entity, jackdaw_bsn::BsnValue)> = Vec::new();
    for &entity in &targets {
        if already_captured.contains(&entity) {
            continue;
        }
        if let Some(live) = live_bsn_field(world, entity, type_path, field_path) {
            to_capture.push((entity, live));
        }
    }
    if to_capture.is_empty() {
        return;
    }
    let mut sessions = world.resource_mut::<FieldEditSessions>();
    for (entity, live) in to_capture {
        sessions.live_at_begin.insert(
            FieldEditSessionKey {
                entity,
                type_path: type_path.to_string(),
                field_path: field_path.to_string(),
            },
            live,
        );
    }
}

/// Preview a field value on live ECS for the current selection.
///
/// Does not touch the scene document or mint undo. Calls [`field_edit_begin`]
/// so a baseline exists before the first write.
pub(crate) fn field_edit_preview(
    world: &mut World,
    type_path: &str,
    field_path: &str,
    value: &serde_json::Value,
) {
    field_edit_begin(world, type_path, field_path);
    let targets = field_edit_session_targets(world);
    for target in targets {
        apply_json_field_to_ecs(world, target, type_path, field_path, value);
    }
}

/// Commit a field edit: build [`SetBsnField`] commands from session / document
/// baselines, execute them, push history, and clear the gesture session.
pub(crate) fn field_edit_commit(
    world: &mut World,
    type_path: &str,
    field_path: &str,
    new_json: &serde_json::Value,
    group_label: &str,
) {
    // Immediate commits (no prior preview) still need a derived baseline.
    field_edit_begin(world, type_path, field_path);
    let targets = field_edit_session_targets(world);

    let mut sub_commands: Vec<Box<dyn EditorCommand>> = Vec::new();
    for &target in &targets {
        let old_value = resolve_field_edit_old_value(world, target, type_path, field_path);
        let Some(new_value) =
            json_field_edit_to_bsn_value(world, target, type_path, field_path, new_json)
        else {
            continue;
        };
        sub_commands.push(Box::new(SetBsnField {
            entity: target,
            type_path: type_path.to_string(),
            field_path: field_path.to_string(),
            old_value,
            new_value,
            was_derived: false,
        }));
    }
    clear_field_edit_session(world, type_path, field_path);

    if sub_commands.is_empty() {
        return;
    }

    let mut cmd: Box<dyn EditorCommand> = if sub_commands.len() == 1 {
        sub_commands.remove(0)
    } else {
        Box::new(CommandGroup {
            label: group_label.to_string(),
            commands: sub_commands,
        })
    };
    cmd.execute(world);
    world.resource_mut::<CommandHistory>().push_executed(cmd);
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

    fn sync_after_external_execute(&self, world: &mut World) {
        // Live-drag paths (gizmo, modal transform) mutate the ECS
        // Transform every frame. By the time the command reaches the
        // history, the ECS is already at `new_transform`. Only the
        // AST sync still needs to happen.
        sync_component_to_ast::<Transform>(
            world,
            self.entity,
            "bevy_transform::components::transform::Transform",
            &self.new_transform,
        );
    }
}

pub struct ReparentEntity {
    pub entity: Entity,
    pub old_parent: Option<Entity>,
    pub new_parent: Option<Entity>,
}

impl EditorCommand for ReparentEntity {
    fn execute(&mut self, world: &mut World) {
        let slot = world
            .get::<jackdaw_ui::UiSlot>(self.entity)
            .map(|slot| slot.0.clone());
        set_hierarchy_location(
            world,
            self.entity,
            HierarchyLocation {
                parent: self.new_parent,
                index: usize::MAX,
                slot,
            },
        );
    }

    fn undo(&mut self, world: &mut World) {
        let slot = world
            .get::<jackdaw_ui::UiSlot>(self.entity)
            .map(|slot| slot.0.clone());
        set_hierarchy_location(
            world,
            self.entity,
            HierarchyLocation {
                parent: self.old_parent,
                index: usize::MAX,
                slot,
            },
        );
    }

    fn description(&self) -> &str {
        "Reparent entity"
    }
}

/// Exact authored position of an entity in Jackdaw's ordered hierarchy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HierarchyLocation {
    pub parent: Option<Entity>,
    pub index: usize,
    /// Optional semantic widget slot. Containment and ordering still come
    /// from the ECS hierarchy.
    pub slot: Option<String>,
}

impl HierarchyLocation {
    /// Read an entity's current parent, sibling index, and UI slot.
    pub fn from_world(world: &World, entity: Entity) -> Self {
        let parent = world.get::<ChildOf>(entity).map(ChildOf::parent);
        let index = parent
            .and_then(|parent| world.get::<Children>(parent))
            .and_then(|children| children.iter().position(|child| child == entity))
            .unwrap_or_else(|| {
                let Some(ast) = world.get_resource::<jackdaw_bsn::SceneBsnAst>() else {
                    return 0;
                };
                let Some(node) = ast.ast_for(entity) else {
                    return 0;
                };
                ast.roots
                    .iter()
                    .position(|candidate| *candidate == node)
                    .unwrap_or(0)
            });
        let slot = world
            .get::<jackdaw_ui::UiSlot>(entity)
            .map(|slot| slot.0.clone());
        Self {
            parent,
            index,
            slot,
        }
    }
}

/// Undoable reparent/reorder operation used by the outliner and UI canvas.
pub struct MoveEntity {
    pub entity: Entity,
    pub old: HierarchyLocation,
    pub new: HierarchyLocation,
}

impl MoveEntity {
    pub fn new(world: &World, entity: Entity, new: HierarchyLocation) -> Self {
        Self {
            entity,
            old: HierarchyLocation::from_world(world, entity),
            new,
        }
    }
}

impl EditorCommand for MoveEntity {
    fn execute(&mut self, world: &mut World) {
        set_hierarchy_location(world, self.entity, self.new.clone());
    }

    fn undo(&mut self, world: &mut World) {
        set_hierarchy_location(world, self.entity, self.old.clone());
    }

    fn description(&self) -> &str {
        "Move entity"
    }
}

/// Reparent `entity` under `parent` (or to top-level if `None`), keeping
/// the live scene document authoritative: the node's place in the document
/// hierarchy is the source of truth; the ECS `ChildOf` is mirrored from it
/// so the visual scene tracks the document. Preserves the entity's world
/// position across the move.
///
/// Any code path that needs to change an entity's parent should call
/// this (or push `ReparentEntity` through the command history) -- never
/// `world.entity_mut(e).insert(ChildOf(..))` directly. Bypassing the
/// document update leaves the node's parent stale, and later consumers
/// (prefab save, scene serialization, tab swap) read the document and
/// silently disagree with the visible hierarchy.
pub(crate) fn set_parent(world: &mut World, entity: Entity, parent: Option<Entity>) {
    let slot = world
        .get::<jackdaw_ui::UiSlot>(entity)
        .map(|slot| slot.0.clone());
    set_hierarchy_location(
        world,
        entity,
        HierarchyLocation {
            parent,
            index: usize::MAX,
            slot,
        },
    );
}

/// Apply an exact ordered hierarchy location to both the live ECS and BSN
/// document while preserving an entity's world-space transform.
pub fn set_hierarchy_location(world: &mut World, entity: Entity, location: HierarchyLocation) {
    let current_world = world.get::<GlobalTransform>(entity).copied();
    let new_parent_world = location
        .parent
        .and_then(|parent| world.get::<GlobalTransform>(parent).copied());

    jackdaw_bsn::sync_hierarchy_to_ast_at(world, entity, location.parent, location.index);

    match location.parent {
        Some(parent) => {
            let index = world
                .get::<Children>(parent)
                .map(|children| location.index.min(children.len()))
                .unwrap_or(0);
            world.entity_mut(parent).insert_children(index, &[entity]);
        }
        None => {
            world.entity_mut(entity).remove::<ChildOf>();
        }
    }

    match location.slot {
        Some(slot) => {
            let slot = jackdaw_ui::UiSlot(slot);
            world.entity_mut(entity).insert(slot.clone());
            sync_component_to_ast(world, entity, "jackdaw_ui::UiSlot", &slot);
        }
        None => {
            world.entity_mut(entity).remove::<jackdaw_ui::UiSlot>();
            let node = world
                .get_resource::<jackdaw_bsn::SceneBsnAst>()
                .and_then(|ast| ast.ast_for(entity));
            if let Some(node) = node {
                world
                    .resource_mut::<jackdaw_bsn::SceneBsnAst>()
                    .remove_component_patch(node, "jackdaw_ui::UiSlot");
            }
        }
    }

    let new_transform =
        if let (Some(world_tf), Some(parent_world)) = (current_world, new_parent_world) {
            Some(Transform::from_matrix(
                (parent_world.affine().inverse() * world_tf.affine()).into(),
            ))
        } else if location.parent.is_none() {
            current_world.map(|w| Transform::from_matrix(w.affine().into()))
        } else {
            None
        };
    if let Some(new_tf) = new_transform
        && let Some(mut tf) = world.get_mut::<Transform>(entity)
    {
        *tf = new_tf;
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
    /// Type paths of components inserted by `#[require]` (or other side
    /// effects) during `execute`.
    required_companions: Vec<String>,
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
            required_companions: Vec::new(),
        }
    }
}

impl EditorCommand for AddComponent {
    fn execute(&mut self, world: &mut World) {
        info!(
            "AddComponent::execute entered: type_path={}, type_id={:?}, component_id={:?}, entity={:?}",
            self.type_path, self.type_id, self.component_id, self.entity
        );
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();

        let Some(registration) = registry.get(self.type_id) else {
            warn!(
                "AddComponent::execute: registry has no entry for type_id {:?} (type_path={})",
                self.type_id, self.type_path
            );
            return;
        };

        // `build_reflective_default` lets user components reach
        // the editor without `#[derive(Default)]` by walking
        // their fields recursively. Falls back to
        // `ReflectDefault` when the type opted in.
        let Some(default_value) =
            crate::reflect_default::build_reflective_default(self.type_id, &registry)
        else {
            warn!(
                "AddComponent::execute: type {} has no `ReflectDefault` and a field is an \
                 opaque type, list, map, or set with no default. Add `Default` to derives \
                 and `#[reflect(...)]`, or simplify the fields to reflected primitives.",
                self.type_path
            );
            return;
        };
        if registration.data::<ReflectComponent>().is_none() {
            warn!(
                "AddComponent::execute: type {} has no ReflectComponent. Add `Component` \
                 to `#[reflect(...)]`.",
                self.type_path
            );
            return;
        }

        // Snapshot reflected components before insert so we can tell which
        // companions `#[require]` (and similar) added, without writing them
        // into the document.
        drop(registry);
        let before = reflected_component_type_paths(world, self.entity);

        // Insert triggers `#[require]`, which may pull in
        // dependents (e.g. `RigidBody` requires `Position`,
        // `Rotation`, etc.).
        info!(
            "AddComponent: inserting `{}` (type_id {:?}, component_id {:?}) on entity {:?}",
            self.type_path, self.type_id, self.component_id, self.entity
        );
        {
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
                default_value.as_partial_reflect(),
                &registry,
            );
        }
        let has_after = world
            .get_entity(self.entity)
            .ok()
            .map(|e| e.archetype().components().contains(&self.component_id))
            .unwrap_or(false);
        info!(
            "AddComponent: post-insert, entity {:?} has component_id {:?}: {has_after}",
            self.entity, self.component_id
        );

        let after = reflected_component_type_paths(world, self.entity);
        self.required_companions = after
            .into_iter()
            .filter(|type_path| type_path != &self.type_path && !before.contains(type_path))
            .collect();
        if !self.required_companions.is_empty() {
            info!(
                "AddComponent: {} #[require] companions stay ECS-only (not authored): {:?}",
                self.required_companions.len(),
                self.required_companions
            );
        }

        // Sync only the explicitly-added component into the scene document.
        // Companions remain live ECS state until the user edits one (which
        // mints an authored override patch).
        let tracked = world
            .resource::<jackdaw_bsn::SceneBsnAst>()
            .ast_for(self.entity)
            .is_some();
        if tracked {
            let registry = world.resource::<AppTypeRegistry>().clone();
            sync_component_to_bsn_doc(
                world,
                self.entity,
                default_value.as_partial_reflect(),
                &registry,
            );
        } else {
            warn!(
                "AddComponent: entity {:?} is not tracked in the scene document; \
                 {} is on the entity but won't persist through save/load.",
                self.entity, self.type_path
            );
        }
    }

    fn undo(&mut self, world: &mut World) {
        // Resolve require-companions' ComponentIds so undo strips them from
        // the live entity. They were never written to the document.
        let registry = world.resource::<AppTypeRegistry>().clone();
        let reg = registry.read();
        let companion_ids: Vec<bevy::ecs::component::ComponentId> = self
            .required_companions
            .iter()
            .filter_map(|type_path| {
                let type_id = reg.get_with_type_path(type_path)?.type_id();
                world.components().get_id(type_id)
            })
            .collect();
        drop(reg);

        if let Ok(mut entity) = world.get_entity_mut(self.entity) {
            entity.remove_by_id(self.component_id);
            for cid in &companion_ids {
                entity.remove_by_id(*cid);
            }
        }
        // Remove only the explicitly-added component from the document.
        {
            let mut ast = world.resource_mut::<jackdaw_bsn::SceneBsnAst>();
            if let Some(node) = ast.ast_for(self.entity) {
                ast.remove_component_patch(node, &self.type_path);
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

/// Add a project component to an entity as a document-only patch.
/// Project types are never registered as real ECS components in the
/// editor -- loading their code would leak -- so the component lives
/// purely in the scene document as a default-valued struct patch,
/// editable through the inspector's document path and materialized as
/// a real component only in the game binary at Play.
pub struct AddProjectComponent {
    pub entity: Entity,
    pub type_path: String,
    /// Whether execute actually inserted a patch (false if the node
    /// already carried the component or is untracked); gates undo.
    added: bool,
}

impl AddProjectComponent {
    pub fn new(entity: Entity, type_path: String) -> Self {
        Self {
            entity,
            type_path,
            added: false,
        }
    }
}

impl EditorCommand for AddProjectComponent {
    fn execute(&mut self, world: &mut World) {
        let mut ast = world.resource_mut::<jackdaw_bsn::SceneBsnAst>();
        let Some(node) = ast.ast_for(self.entity) else {
            warn!(
                "AddProjectComponent: entity {:?} is not tracked in the scene document; \
                 project component {} cannot be added.",
                self.entity, self.type_path
            );
            self.added = false;
            return;
        };
        if ast.find_patch_by_type_path(node, &self.type_path).is_some() {
            self.added = false;
            return;
        }
        // A struct patch with no field overrides materializes as the
        // type's Default at Play; the inspector shows the schema
        // defaults until a field is edited (which writes an override).
        let patch = jackdaw_bsn::BsnPatch::Struct(jackdaw_bsn::BsnStructData {
            type_path: self.type_path.clone(),
            fields: jackdaw_bsn::BsnStructFields(Vec::new()),
        });
        let patch_entity = ast.world.spawn(patch).id();
        if let Some(patches) = ast.get_patches_mut(node) {
            patches.0.push(patch_entity);
        }
        self.added = true;

        if let Ok(mut ec) = world.get_entity_mut(self.entity) {
            ec.insert(crate::inspector::InspectorDirty);
        }
    }

    fn undo(&mut self, world: &mut World) {
        if !self.added {
            return;
        }
        {
            let mut ast = world.resource_mut::<jackdaw_bsn::SceneBsnAst>();
            if let Some(node) = ast.ast_for(self.entity) {
                ast.remove_component_patch(node, &self.type_path);
            }
        }
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
    /// Document patch snapshot for undo.
    pub ast_snapshot: Option<jackdaw_bsn::BsnPatch>,
}

impl EditorCommand for RemoveComponent {
    fn execute(&mut self, world: &mut World) {
        // Snapshot the document patch before removal
        {
            let ast = world.resource::<jackdaw_bsn::SceneBsnAst>();
            self.ast_snapshot = ast.ast_for(self.entity).and_then(|node| {
                ast.find_patch_by_type_path(node, &self.type_path)
                    .and_then(|pe| ast.get_patch(pe))
                    .cloned()
            });
        }
        if let Ok(mut entity) = world.get_entity_mut(self.entity) {
            entity.remove_by_id(self.component_id);
        }
        // Remove from the document
        let mut ast = world.resource_mut::<jackdaw_bsn::SceneBsnAst>();
        if let Some(node) = ast.ast_for(self.entity) {
            ast.remove_component_patch(node, &self.type_path);
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

        // Restore the document patch snapshot
        if let Some(patch) = self.ast_snapshot.take() {
            let mut ast = world.resource_mut::<jackdaw_bsn::SceneBsnAst>();
            if let Some(node) = ast.ast_for(self.entity)
                && ast.find_patch_by_type_path(node, &self.type_path).is_none()
            {
                let pe = ast.world.spawn(patch).id();
                if let Some(patches) = ast.get_patches_mut(node) {
                    patches.0.push(pe);
                }
            }
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
                .resource_mut::<jackdaw_bsn::SceneBsnAst>()
                .remove_entity_node(entity);
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
    pub scene_snapshot: DynamicWorld,
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
            .resource_mut::<jackdaw_bsn::SceneBsnAst>()
            .remove_entity_node(self.entity);
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

/// Create a `DynamicWorldBuilder` that excludes computed components which become
/// stale when restored (Children references dead mesh entities, visibility flags
/// block rendering).
pub(crate) fn filtered_scene_builder<'w>(
    world: &'w World,
    type_registry: &'w bevy::reflect::TypeRegistry,
) -> DynamicWorldBuilder<'w> {
    DynamicWorldBuilder::from_world(world, type_registry)
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

/// Create a `DynamicWorld` snapshot of a single entity and all its descendants.
pub(crate) fn snapshot_entity(world: &World, entity: Entity) -> DynamicWorld {
    let type_registry = world.resource::<AppTypeRegistry>().read();
    let mut entities = Vec::new();
    collect_entity_ids(world, entity, &mut entities);
    filtered_scene_builder(world, &type_registry)
        .extract_entities(entities.into_iter())
        .build()
}

pub(crate) fn collect_entity_ids(world: &World, entity: Entity, out: &mut Vec<Entity>) {
    out.push(entity);
    if let Some(children) = world.get::<Children>(entity) {
        for child in children.iter() {
            // A dangling child reference (e.g. left by an older duplicate) points at a
            // despawned entity; skip it so callers never feed it to DynamicSceneBuilder.
            if world.get_entity(child).is_err() {
                continue;
            }
            // Skip editor-only entities and runtime-generated children
            // (e.g. BrushMeshChunk meshes). Including NonSerializable
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

/// Rebuild a `DynamicWorld` by copying its entity data (since `DynamicWorld` doesn't impl Clone).
pub(crate) fn snapshot_rebuild(scene: &DynamicWorld) -> DynamicWorld {
    DynamicWorld {
        resources: scene.resources.iter().map(|r| r.to_dynamic()).collect(),
        entities: scene
            .entities
            .iter()
            .map(|e| bevy::world_serialization::DynamicEntity {
                entity: e.entity,
                components: e.components.iter().map(|c| c.to_dynamic()).collect(),
            })
            .collect(),
    }
}

// ============================== Document-First Commands ==============================

/// Write a field into the live [`jackdaw_bsn::SceneBsnAst`] document, promote
/// the component to authored if it was derived, and mirror the change onto
/// the live ECS entity. The dispatched field-edit command for inspector and
/// tool edits.
pub struct SetBsnField {
    pub entity: Entity,
    pub type_path: String,
    pub field_path: String,
    /// Pre-edit baseline for undo. `None` when the field/component did not
    /// exist before this edit; undo then removes what execute authored.
    ///
    /// For derived (ECS-only) components, callers should supply the pre-edit
    /// live value via `field_edit_commit` / the gesture session. If still
    /// `None` at execute, the live field is captured as a fallback for
    /// immediate edits that never preview-mutated ECS.
    pub old_value: Option<jackdaw_bsn::BsnValue>,
    pub new_value: jackdaw_bsn::BsnValue,
    /// True when execute authored an override for a component that was not
    /// already in the document: either a derived (ECS-only) component, or a
    /// project (document-only) component. Undo drops that override patch;
    /// for derived components the live ECS value stays (optionally restored
    /// from [`Self::old_value`]).
    pub was_derived: bool,
}

/// Reflect type path of [`Name`], which the document stores as a
/// [`jackdaw_bsn::BsnPatch::Name`] reference patch rather than a component
/// patch. [`SetBsnField`] routes edits of this type through the name patch.
pub(crate) const NAME_TYPE_PATH: &str = "bevy_ecs::name::Name";

/// The string carried by a [`jackdaw_bsn::BsnValue`], for name edits.
fn bsn_value_string(value: &jackdaw_bsn::BsnValue) -> Option<&str> {
    match value {
        jackdaw_bsn::BsnValue::String(s) => Some(s.as_str()),
        _ => None,
    }
}

/// Set, replace, or remove the [`jackdaw_bsn::BsnPatch::Name`] patch on a
/// document node.
pub(crate) fn set_name_patch(ast: &mut jackdaw_bsn::SceneBsnAst, node: Entity, name: Option<&str>) {
    let existing = ast.get_patches(node).and_then(|patches| {
        patches
            .0
            .iter()
            .copied()
            .find(|&pe| matches!(ast.get_patch(pe), Some(jackdaw_bsn::BsnPatch::Name(_))))
    });
    match name {
        Some(name) => {
            if let Some(pe) = existing {
                ast.set_patch(pe, jackdaw_bsn::BsnPatch::Name(name.to_string()));
            } else {
                let pe = ast
                    .world
                    .spawn(jackdaw_bsn::BsnPatch::Name(name.to_string()))
                    .id();
                if let Some(patches) = ast.get_patches_mut(node) {
                    patches.0.insert(0, pe);
                }
            }
        }
        None => {
            if let Some(pe) = existing {
                if let Some(patches) = ast.get_patches_mut(node) {
                    patches.0.retain(|&x| x != pe);
                }
                ast.world.despawn(pe);
            }
        }
    }
}

impl SetBsnField {
    /// Write an entity name into the document's `#name` reference patch and
    /// mirror it onto the live ECS entity. `None` removes both.
    fn apply_name(&self, world: &mut World, name: Option<&str>) {
        {
            let mut ast = world.resource_mut::<jackdaw_bsn::SceneBsnAst>();
            if let Some(node) = ast.ast_for(self.entity) {
                set_name_patch(&mut ast, node, name);
            }
        }
        let Ok(mut entity_mut) = world.get_entity_mut(self.entity) else {
            return;
        };
        match name {
            Some(name) => {
                entity_mut.insert(Name::new(name.to_string()));
            }
            None => {
                entity_mut.remove::<Name>();
            }
        }
    }

    /// Re-apply this command's component patch from the document to the live
    /// entity, so ECS matches the document after execute or undo.
    fn mirror_patch_to_ecs(&self, world: &mut World) {
        let patch = {
            let ast = world.resource::<jackdaw_bsn::SceneBsnAst>();
            let Some(patches_entity) = ast.ast_for(self.entity) else {
                return;
            };
            ast.find_patch_by_type_path(patches_entity, &self.type_path)
                .and_then(|pe| ast.get_patch(pe))
                .cloned()
        };
        if let Some(patch) = patch {
            jackdaw_bsn::apply_component_patch(world, self.entity, &patch);
        }
    }
}

impl EditorCommand for SetBsnField {
    fn execute(&mut self, world: &mut World) {
        // Names live in the document as `#name` reference patches, not
        // component patches, so route them through the name path.
        if self.type_path == NAME_TYPE_PATH {
            let new_name = bsn_value_string(&self.new_value)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            self.apply_name(world, new_name.as_deref());
            return;
        }
        let is_project = world
            .get_resource::<crate::project_types::ProjectTypes>()
            .is_some_and(|pt| pt.is_project_component(&self.type_path));
        let had_patch = {
            let ast = world.resource::<jackdaw_bsn::SceneBsnAst>();
            ast.ast_for(self.entity)
                .is_some_and(|node| ast.find_patch_by_type_path(node, &self.type_path).is_some())
        };
        // Derived = on the live entity with no document patch. Project
        // components are document-only; a missing patch is still a first
        // author that undo should drop entirely.
        let live_before =
            !is_project && entity_has_reflected_component(world, self.entity, &self.type_path);
        // First override of a derived component: capture the pre-edit live
        // field when the caller did not supply a baseline.
        if self.old_value.is_none() && !self.field_path.is_empty() && live_before && !had_patch {
            self.old_value = live_bsn_field(world, self.entity, &self.type_path, &self.field_path);
        }
        {
            let registry = world.resource::<AppTypeRegistry>().clone();
            let registry = registry.read();
            let mut ast = world.resource_mut::<jackdaw_bsn::SceneBsnAst>();
            let Some(patches_entity) = ast.ast_for(self.entity) else {
                return;
            };
            if is_project {
                set_project_field(
                    &mut ast,
                    patches_entity,
                    &self.type_path,
                    &self.field_path,
                    self.new_value.clone(),
                );
            } else {
                jackdaw_bsn::set_bsn_field(
                    &mut ast,
                    patches_entity,
                    &self.type_path,
                    &self.field_path,
                    self.new_value.clone(),
                    &registry,
                );
            }
            if !had_patch && (is_project || live_before) {
                self.was_derived = true;
                if live_before {
                    info!(
                        "Authored override for previously derived component '{}'",
                        self.type_path
                    );
                }
            }
        }
        // A project component has no real ECS counterpart to mirror into; the
        // document is the whole of its editor state.
        if !is_project {
            self.mirror_patch_to_ecs(world);
        }
    }

    fn undo(&mut self, world: &mut World) {
        if self.type_path == NAME_TYPE_PATH {
            let old_name = self
                .old_value
                .as_ref()
                .and_then(bsn_value_string)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            self.apply_name(world, old_name.as_deref());
            return;
        }
        // A missing old value means execute authored something that did not
        // exist before: a whole component (empty field path) or a single
        // field of a sparse patch. Undo removes what execute authored.
        let removes_component = self.field_path.is_empty() && self.old_value.is_none();
        let removes_field = !self.field_path.is_empty() && self.old_value.is_none();
        let is_project = world
            .get_resource::<crate::project_types::ProjectTypes>()
            .is_some_and(|pt| pt.is_project_component(&self.type_path));
        {
            let registry = world.resource::<AppTypeRegistry>().clone();
            let registry = registry.read();
            let mut ast = world.resource_mut::<jackdaw_bsn::SceneBsnAst>();
            let Some(patches_entity) = ast.ast_for(self.entity) else {
                return;
            };
            if self.was_derived {
                // Drop the override so the component is derived (or absent
                // from the document) again. Restore the pre-edit live field
                // onto the temporary patch when we have one, so mirror can
                // put ECS back before the patch is removed.
                if let Some(old_value) = &self.old_value {
                    jackdaw_bsn::set_bsn_field(
                        &mut ast,
                        patches_entity,
                        &self.type_path,
                        &self.field_path,
                        old_value.clone(),
                        &registry,
                    );
                }
            } else if removes_component {
                ast.remove_component_patch(patches_entity, &self.type_path);
            } else if removes_field {
                jackdaw_bsn::remove_bsn_field(
                    &mut ast,
                    patches_entity,
                    &self.type_path,
                    &self.field_path,
                );
            } else if let Some(old_value) = &self.old_value {
                if is_project {
                    set_project_field(
                        &mut ast,
                        patches_entity,
                        &self.type_path,
                        &self.field_path,
                        old_value.clone(),
                    );
                } else {
                    jackdaw_bsn::set_bsn_field(
                        &mut ast,
                        patches_entity,
                        &self.type_path,
                        &self.field_path,
                        old_value.clone(),
                        &registry,
                    );
                }
            }
        }
        // Project components have no ECS counterpart; the document write above
        // is the whole of the undo.
        if is_project {
            if self.was_derived {
                let mut ast = world.resource_mut::<jackdaw_bsn::SceneBsnAst>();
                if let Some(patches_entity) = ast.ast_for(self.entity) {
                    ast.remove_component_patch(patches_entity, &self.type_path);
                }
            }
            return;
        }
        if self.was_derived {
            // Demote only: restore the pre-edit live field when captured,
            // then drop the override. The component stays on the entity.
            if self.old_value.is_some() {
                self.mirror_patch_to_ecs(world);
            }
            let mut ast = world.resource_mut::<jackdaw_bsn::SceneBsnAst>();
            if let Some(patches_entity) = ast.ast_for(self.entity) {
                ast.remove_component_patch(patches_entity, &self.type_path);
            }
        } else if removes_component {
            remove_component_from_ecs(world, self.entity, &self.type_path);
        } else if removes_field {
            // The doc field is gone; restore the ECS field to the type's
            // default, which is what the sparse patch resolves to when the
            // field is absent.
            reset_ecs_field_to_default(world, self.entity, &self.type_path, &self.field_path);
        } else {
            self.mirror_patch_to_ecs(world);
        }
    }

    fn description(&self) -> &str {
        "Set component field"
    }
}

/// Apply a JSON value to an ECS component -- either full component replacement
/// (empty `field_path`) or field-level update.
///
/// Writes only the live ECS component, leaving the scene document untouched.
/// Prefer [`field_edit_preview`] for inspector gestures; this is the live write
/// primitive that preview uses.
pub(crate) fn apply_json_field_to_ecs(
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

/// Remove a reflected component from an ECS entity by type path. A no-op when
/// the type is unregistered or the entity is gone.
/// Reset one field of a live ECS component to the type's default value:
/// the state a sparse patch resolves to when the field is not authored.
fn reset_ecs_field_to_default(
    world: &mut World,
    entity: Entity,
    type_path: &str,
    field_path: &str,
) {
    use bevy::reflect::GetPath;
    use bevy::reflect::prelude::ReflectDefault;

    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let Some(registration) = registry.get_with_type_path(type_path) else {
        return;
    };
    let Some(reflect_default) = registration.data::<ReflectDefault>() else {
        return;
    };
    let Some(reflect_component) = registration.data::<ReflectComponent>() else {
        return;
    };

    let default_instance = reflect_default.default();
    let Ok(default_field) = default_instance.reflect_path(field_path) else {
        return;
    };
    let default_field = default_field.to_dynamic();

    let Some(component) = reflect_component.reflect_mut(world.entity_mut(entity)) else {
        return;
    };
    if let Ok(field) = component.into_inner().reflect_path_mut(field_path) {
        field.apply(&*default_field);
    }
}

fn remove_component_from_ecs(world: &mut World, entity: Entity, type_path: &str) {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let Some(registration) = registry.get_with_type_path(type_path) else {
        return;
    };
    let Some(reflect_component) = registration.data::<ReflectComponent>() else {
        return;
    };
    let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
        return;
    };
    reflect_component.remove(&mut entity_mut);
}

/// Whether `entity` currently carries the reflected component named by
/// `type_path`. Used to distinguish derived (ECS-only) components from
/// components that execute will mint for the first time.
fn entity_has_reflected_component(world: &World, entity: Entity, type_path: &str) -> bool {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let Some(registration) = registry.get_with_type_path(type_path) else {
        return false;
    };
    let Some(reflect_component) = registration.data::<ReflectComponent>() else {
        return false;
    };
    let Ok(entity_ref) = world.get_entity(entity) else {
        return false;
    };
    reflect_component.reflect(entity_ref).is_some()
}

/// Convert a `serde_json::Value` into the matching reflect primitive and apply it.
/// Falls back to Bevy's typed deserialization for complex types (enums, structs)
/// that can't be handled by simple primitive downcasts.
pub(crate) fn apply_json_to_reflect(
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

/// The authored value of one field on `entity`'s document node, read from the
/// live BSN document. `None` when the entity has no node or the component or
/// field is not authored.
pub(crate) fn authored_bsn_field(
    world: &World,
    entity: Entity,
    type_path: &str,
    field_path: &str,
) -> Option<jackdaw_bsn::BsnValue> {
    let ast = world.resource::<jackdaw_bsn::SceneBsnAst>();
    let node = ast.ast_for(entity)?;
    jackdaw_bsn::get_bsn_field(ast, node, type_path, field_path)
}

/// Convert one field edit given as reflect-format JSON into the
/// [`jackdaw_bsn::BsnValue`] to author. Field-level edits merge the JSON into
/// a copy of the entity's current component so nested values convert with
/// their concrete types; whole-component edits deserialize the JSON directly.
pub(crate) fn json_field_edit_to_bsn_value(
    world: &World,
    entity: Entity,
    type_path: &str,
    field_path: &str,
    value: &serde_json::Value,
) -> Option<jackdaw_bsn::BsnValue> {
    use bevy::reflect::GetPath;
    use serde::de::DeserializeSeed;

    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let Some(registration) = registry.get_with_type_path(type_path) else {
        // Project (schema-reported) components have no editor registration; the
        // field's authored value comes straight from the extracted schema type.
        drop(registry);
        return project_field_edit_to_bsn_value(world, type_path, field_path, value);
    };
    if field_path.is_empty() {
        let deserializer =
            bevy::reflect::serde::TypedReflectDeserializer::new(registration, &registry);
        let reflected = deserializer.deserialize(value).ok()?;
        return Some(jackdaw_bsn::BsnValue::from_reflect(
            reflected.as_ref(),
            &registry,
        ));
    }
    let reflect_component = registration.data::<ReflectComponent>()?;
    let entity_ref = world.get_entity(entity).ok()?;
    let component = reflect_component.reflect(entity_ref)?;
    // Path navigation needs a concrete (`Reflect`) clone of the component.
    let mut merged: Box<dyn Reflect> = registration
        .data::<bevy::reflect::ReflectFromReflect>()?
        .from_reflect(component.as_partial_reflect())?;
    if let Ok(field) = merged.reflect_path_mut(field_path) {
        apply_json_to_reflect(field, value, &registry);
    }
    let field = merged.reflect_path(field_path).ok()?;
    Some(jackdaw_bsn::BsnValue::from_reflect(field, &registry))
}

/// Convert a field edit on a project (schema-reported) component into the
/// [`jackdaw_bsn::BsnValue`] to author, without an editor registration. The
/// field's scalar variant is chosen from its schema type path.
fn project_field_edit_to_bsn_value(
    world: &World,
    type_path: &str,
    field_path: &str,
    value: &serde_json::Value,
) -> Option<jackdaw_bsn::BsnValue> {
    let schema = world
        .get_resource::<crate::project_types::ProjectTypes>()?
        .component(type_path)?;
    let name = field_path.split('.').next().unwrap_or(field_path);
    let field = schema.fields.iter().find(|f| f.name == name)?;
    Some(
        crate::inspector::project_component_display::json_to_bsn_value_typed(
            &field.type_path,
            value,
        ),
    )
}

/// Author a flat field value on a project component's document patch without a
/// registration. Project types are never in the editor registry, so the
/// registry-gated [`jackdaw_bsn::set_bsn_field`] refuses them; this upserts the
/// named field directly on the node's struct patch instead. Nested paths set
/// only their leading segment (v1 only renders flat scalar fields).
fn set_project_field(
    ast: &mut jackdaw_bsn::SceneBsnAst,
    node: Entity,
    type_path: &str,
    field_path: &str,
    value: jackdaw_bsn::BsnValue,
) {
    let patch_entity = match ast.find_patch_by_type_path(node, type_path) {
        Some(pe) => pe,
        None => {
            let pe = ast
                .world
                .spawn(jackdaw_bsn::BsnPatch::Struct(jackdaw_bsn::BsnStructData {
                    type_path: type_path.to_string(),
                    fields: jackdaw_bsn::BsnStructFields(Vec::new()),
                }))
                .id();
            if let Some(patches) = ast.get_patches_mut(node) {
                patches.0.push(pe);
            }
            pe
        }
    };
    let Some(patch) = ast.world.get_mut::<jackdaw_bsn::BsnPatch>(patch_entity) else {
        return;
    };
    let patch = patch.into_inner();
    if let jackdaw_bsn::BsnPatch::Type(existing) = patch {
        let existing = existing.clone();
        *patch = jackdaw_bsn::BsnPatch::Struct(jackdaw_bsn::BsnStructData {
            type_path: existing,
            fields: jackdaw_bsn::BsnStructFields(Vec::new()),
        });
    }
    if field_path.is_empty() {
        if let jackdaw_bsn::BsnValue::Struct(data) = value {
            *patch = jackdaw_bsn::BsnPatch::Struct(data);
        }
        return;
    }
    let jackdaw_bsn::BsnPatch::Struct(data) = patch else {
        return;
    };
    let name = field_path.split('.').next().unwrap_or(field_path);
    if let Some(existing) = data.fields.0.iter_mut().find(|f| f.name == name) {
        existing.value = value;
    } else {
        data.fields.0.push(jackdaw_bsn::BsnField {
            name: name.to_string(),
            value,
        });
    }
}

/// Write a component value into the live scene document. The patch key
/// comes from the value's own reflected type path, so a stale
/// caller-supplied string cannot skew it; `type_path` is kept in the
/// signature for call-site clarity.
pub fn sync_component_to_ast<T: bevy::reflect::Reflect>(
    world: &mut World,
    entity: Entity,
    type_path: &str,
    value: &T,
) {
    let _ = type_path;
    let registry = world.resource::<AppTypeRegistry>().clone();
    sync_component_to_bsn_doc(world, entity, value.as_partial_reflect(), &registry);
}

/// Upsert one component's patch on the entity's BSN document node from a
/// reflected value.
pub(crate) fn sync_component_to_bsn_doc(
    world: &mut World,
    entity: Entity,
    value: &dyn bevy::reflect::PartialReflect,
    registry: &AppTypeRegistry,
) {
    let patch = {
        let reg = registry.read();
        jackdaw_bsn::component_to_bsn_patch(value, &reg)
    };
    let type_path = match value.get_represented_type_info() {
        Some(info) => info.type_path().to_string(),
        None => return,
    };
    let Some(mut ast) = world.get_resource_mut::<jackdaw_bsn::SceneBsnAst>() else {
        return;
    };
    let Some(patches_entity) = ast.ast_for(entity) else {
        return;
    };
    if let Some(existing) = ast.find_patch_by_type_path(patches_entity, &type_path) {
        ast.set_patch(existing, patch);
    } else {
        let patch_entity = ast.world.spawn(patch).id();
        if let Some(patches) = ast.get_patches_mut(patches_entity) {
            patches.0.push(patch_entity);
        }
    }
}

/// Reflected component type paths currently on `entity`, excluding structural
/// / skip-listed types. Used to diff `#[require]` companions around an insert
/// without writing them into the scene document.
fn reflected_component_type_paths(
    world: &World,
    entity: Entity,
) -> std::collections::HashSet<String> {
    use std::collections::HashSet;

    let registry = world.resource::<AppTypeRegistry>().clone();
    let reg = registry.read();
    let skip_ids = crate::scene_io::structural_skip_type_ids();
    let Ok(entity_ref) = world.get_entity(entity) else {
        return HashSet::new();
    };
    reg.iter()
        .filter(|registration| !skip_ids.contains(&registration.type_id()))
        .filter_map(|registration| {
            let type_path = registration
                .type_info()
                .type_path_table()
                .path()
                .to_string();
            if crate::scene_io::should_skip_component(&type_path) {
                return None;
            }
            let reflect_component = registration.data::<ReflectComponent>()?;
            reflect_component.reflect(entity_ref)?;
            Some(type_path)
        })
        .collect()
}

/// Reflect a live ECS component field into a [`jackdaw_bsn::BsnValue`] for
/// undo when the field was not previously authored in the document.
pub(crate) fn live_bsn_field(
    world: &World,
    entity: Entity,
    type_path: &str,
    field_path: &str,
) -> Option<jackdaw_bsn::BsnValue> {
    use bevy::reflect::GetPath;

    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let registration = registry.get_with_type_path(type_path)?;
    let reflect_component = registration.data::<ReflectComponent>()?;
    let entity_ref = world.get_entity(entity).ok()?;
    let component = reflect_component.reflect(entity_ref)?;
    let field = component.reflect_path(field_path).ok()?;
    Some(jackdaw_bsn::BsnValue::from_reflect(
        field.as_partial_reflect(),
        &registry,
    ))
}

#[cfg(test)]
mod set_bsn_field_tests {
    use super::*;
    use jackdaw_bsn::{BsnValue, SceneBsnAst, create_entity_in_ast, get_bsn_field};

    fn field_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<SceneBsnAst>();
        app.init_resource::<FieldEditSessions>();
        app.init_resource::<Selection>();
        app.init_resource::<CommandHistory>();
        app
    }

    #[test]
    fn set_bsn_field_round_trips_document_and_ecs_with_undo() {
        let mut app = field_app();
        let entity = app
            .world_mut()
            .spawn(Transform::from_xyz(1.0, 0.0, 0.0))
            .id();
        create_entity_in_ast(app.world_mut(), entity, None);
        // Author the transform into the document so the field edit has a
        // baseline value to restore.
        jackdaw_bsn::sync_to_ast(app.world_mut(), entity, std::any::TypeId::of::<Transform>());

        let type_path = "bevy_transform::components::transform::Transform";
        let mut command = SetBsnField {
            entity,
            type_path: type_path.to_string(),
            field_path: "translation.x".to_string(),
            old_value: Some(BsnValue::Float(1.0)),
            new_value: BsnValue::Float(9.0),
            was_derived: false,
        };

        command.execute(app.world_mut());
        {
            let ast = app.world().resource::<SceneBsnAst>();
            let pe = ast.ast_for(entity).expect("linked");
            let value = get_bsn_field(ast, pe, type_path, "translation.x");
            assert!(
                matches!(value, Some(BsnValue::Float(x)) if (x - 9.0).abs() < 1e-6),
                "document holds the new value"
            );
        }
        let x = app.world().get::<Transform>(entity).unwrap().translation.x;
        assert!((x - 9.0).abs() < 1e-6, "ECS mirrors the new value, got {x}");

        command.undo(app.world_mut());
        let x = app.world().get::<Transform>(entity).unwrap().translation.x;
        assert!((x - 1.0).abs() < 1e-6, "undo restores ECS, got {x}");
        {
            let ast = app.world().resource::<SceneBsnAst>();
            let pe = ast.ast_for(entity).expect("linked");
            let value = get_bsn_field(ast, pe, type_path, "translation.x");
            assert!(
                matches!(value, Some(BsnValue::Float(x)) if (x - 1.0).abs() < 1e-6),
                "undo restores the document"
            );
        }
    }

    /// A field-level execute with no old value authored a previously-absent
    /// field of a sparse patch; undo removes the field again and resets the
    /// live ECS field to the type's default.
    #[test]
    fn undo_removes_field_authored_by_execute() {
        let mut app = field_app();
        let entity = app
            .world_mut()
            .spawn(Transform::from_xyz(5.0, 0.0, 0.0))
            .id();
        create_entity_in_ast(app.world_mut(), entity, None);
        let type_path = "bevy_transform::components::transform::Transform";
        // Author only translation.x, mirroring a sparse patch.
        let mut cmd_translation = SetBsnField {
            entity,
            type_path: type_path.to_string(),
            field_path: "translation.x".to_string(),
            old_value: None,
            new_value: BsnValue::Float(5.0),
            was_derived: false,
        };
        cmd_translation.execute(app.world_mut());

        // Author a second, previously-absent field.
        let mut cmd_scale = SetBsnField {
            entity,
            type_path: type_path.to_string(),
            field_path: "scale.x".to_string(),
            old_value: None,
            new_value: BsnValue::Float(3.0),
            was_derived: false,
        };
        cmd_scale.execute(app.world_mut());
        assert_eq!(app.world().get::<Transform>(entity).unwrap().scale.x, 3.0);

        cmd_scale.undo(app.world_mut());

        // The doc field is gone and the ECS field is back at the default.
        {
            let ast = app.world().resource::<SceneBsnAst>();
            let node = ast.ast_for(entity).expect("linked");
            assert!(
                get_bsn_field(ast, node, type_path, "scale.x").is_none(),
                "undo removes the authored field from the sparse patch"
            );
        }
        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().scale.x,
            1.0,
            "the live field resets to the type default"
        );
        // The other authored field is untouched.
        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation.x,
            5.0
        );
    }

    #[test]
    fn undo_removes_component_authored_by_execute() {
        let mut app = field_app();
        let entity = app.world_mut().spawn_empty().id();
        create_entity_in_ast(app.world_mut(), entity, None);

        let type_path = "bevy_transform::components::transform::Transform";
        let registry = app.world().resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        let new_value = BsnValue::from_reflect(&Transform::from_xyz(4.0, 0.0, 0.0), &registry);
        drop(registry);

        // Whole-component mint: no prior ECS component, empty field path.
        let mut command = SetBsnField {
            entity,
            type_path: type_path.to_string(),
            field_path: String::new(),
            old_value: None,
            new_value,
            was_derived: false,
        };
        command.execute(app.world_mut());
        assert!(
            !command.was_derived,
            "mint from nothing is not a derived promote"
        );
        assert!(app.world().get::<Transform>(entity).is_some());

        command.undo(app.world_mut());
        assert!(
            app.world().get::<Transform>(entity).is_none(),
            "undo removes the component execute authored"
        );
        let ast = app.world().resource::<SceneBsnAst>();
        let pe = ast.ast_for(entity).expect("linked");
        assert!(
            ast.find_patch_by_type_path(pe, type_path).is_none(),
            "document no longer carries the authored patch"
        );
    }

    #[test]
    fn whole_component_author_of_derived_undo_keeps_ecs() {
        let mut app = field_app();
        let entity = app
            .world_mut()
            .spawn(Transform::from_xyz(2.0, 5.0, 8.0))
            .id();
        create_entity_in_ast(app.world_mut(), entity, None);

        let type_path = "bevy_transform::components::transform::Transform";
        let registry = app.world().resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        let new_value = BsnValue::from_reflect(&Transform::from_xyz(9.0, 5.0, 8.0), &registry);
        drop(registry);

        let mut command = SetBsnField {
            entity,
            type_path: type_path.to_string(),
            field_path: String::new(),
            old_value: None,
            new_value,
            was_derived: false,
        };
        command.execute(app.world_mut());
        assert!(
            command.was_derived,
            "pre-existing ECS-only component is a derived promote"
        );
        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation.x,
            9.0
        );

        command.undo(app.world_mut());
        {
            let ast = app.world().resource::<SceneBsnAst>();
            let pe = ast.ast_for(entity).unwrap();
            assert!(
                !ast.component_type_paths(pe).iter().any(|p| p == type_path),
                "undo drops the authored override"
            );
        }
        assert!(
            app.world().get::<Transform>(entity).is_some(),
            "demote keeps the live component"
        );
        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation.x,
            9.0,
            "without an old_value baseline, live state stays at the post-edit value"
        );
    }

    #[test]
    fn editing_derived_component_authors_override_and_undo_drops_it() {
        let mut app = field_app();
        let entity = app
            .world_mut()
            .spawn(Transform::from_xyz(2.0, 5.0, 8.0))
            .id();
        create_entity_in_ast(app.world_mut(), entity, None);
        // Transform is on the entity but not in the document → derived.
        let type_path = "bevy_transform::components::transform::Transform";
        {
            let ast = app.world().resource::<SceneBsnAst>();
            let pe = ast.ast_for(entity).unwrap();
            assert!(
                !ast.component_type_paths(pe).iter().any(|p| p == type_path),
                "precondition: Transform is not authored"
            );
        }

        let mut command = SetBsnField {
            entity,
            type_path: type_path.to_string(),
            field_path: "translation.x".to_string(),
            old_value: None,
            new_value: BsnValue::Float(7.0),
            was_derived: false,
        };
        command.execute(app.world_mut());
        assert!(command.was_derived, "execute records prior derived state");
        assert!(
            command.old_value.is_some(),
            "execute captures the live field for undo"
        );
        {
            let ast = app.world().resource::<SceneBsnAst>();
            let pe = ast.ast_for(entity).unwrap();
            assert!(
                ast.component_type_paths(pe).iter().any(|p| p == type_path),
                "edit authors an override patch"
            );
            assert_eq!(
                get_bsn_field(ast, pe, type_path, "translation.x"),
                Some(BsnValue::Float(7.0))
            );
            assert!(
                get_bsn_field(ast, pe, type_path, "translation.y").is_none(),
                "sibling fields stay unauthored (sparse override)"
            );
            assert!(
                get_bsn_field(ast, pe, type_path, "translation.z").is_none(),
                "sibling fields stay unauthored (sparse override)"
            );
        }
        let translation = app.world().get::<Transform>(entity).unwrap().translation;
        assert_eq!(translation, Vec3::new(7.0, 5.0, 8.0));

        command.undo(app.world_mut());
        {
            let ast = app.world().resource::<SceneBsnAst>();
            let pe = ast.ast_for(entity).unwrap();
            assert!(
                !ast.component_type_paths(pe).iter().any(|p| p == type_path),
                "undo drops the override; component is derived again"
            );
        }
        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation,
            Vec3::new(2.0, 5.0, 8.0),
            "undo restores the pre-edit live field; siblings unchanged"
        );
    }

    #[test]
    fn gesture_session_baseline_survives_live_preview_mutation() {
        let mut app = field_app();
        let entity = app
            .world_mut()
            .spawn(Transform::from_xyz(2.0, 5.0, 8.0))
            .id();
        create_entity_in_ast(app.world_mut(), entity, None);
        app.world_mut().resource_mut::<Selection>().entities = vec![entity];

        let type_path = "bevy_transform::components::transform::Transform";
        let field_path = "translation.x";

        // Preview ticks capture begin baseline then mutate live ECS.
        field_edit_preview(
            app.world_mut(),
            type_path,
            field_path,
            &serde_json::json!(7.0),
        );
        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation.x,
            7.0
        );

        // Commit must use the session baseline, not the post-preview live value.
        field_edit_commit(
            app.world_mut(),
            type_path,
            field_path,
            &serde_json::json!(7.0),
            "test",
        );
        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation.x,
            7.0
        );
        {
            let history = app.world().resource::<CommandHistory>();
            assert_eq!(history.undo_stack.len(), 1);
        }
        app.world_mut()
            .resource_scope(|world, mut history: Mut<CommandHistory>| {
                history.undo(world);
            });
        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation,
            Vec3::new(2.0, 5.0, 8.0),
            "undo restores the gesture-start live value, not the previewed value"
        );
    }

    #[test]
    fn sparse_missing_field_commit_undo_removes_field() {
        let mut app = field_app();
        let entity = app
            .world_mut()
            .spawn(Transform::from_xyz(5.0, 0.0, 0.0))
            .id();
        create_entity_in_ast(app.world_mut(), entity, None);
        app.world_mut().resource_mut::<Selection>().entities = vec![entity];
        let type_path = "bevy_transform::components::transform::Transform";

        // Author only translation.x (sparse patch).
        let mut cmd_translation = SetBsnField {
            entity,
            type_path: type_path.to_string(),
            field_path: "translation.x".to_string(),
            old_value: None,
            new_value: BsnValue::Float(5.0),
            was_derived: false,
        };
        cmd_translation.execute(app.world_mut());

        // Preview+commit a previously-absent field; undo must remove it,
        // not write the live default back into the sparse patch.
        field_edit_preview(
            app.world_mut(),
            type_path,
            "scale.x",
            &serde_json::json!(3.0),
        );
        field_edit_commit(
            app.world_mut(),
            type_path,
            "scale.x",
            &serde_json::json!(3.0),
            "test",
        );
        assert_eq!(app.world().get::<Transform>(entity).unwrap().scale.x, 3.0);

        app.world_mut()
            .resource_scope(|world, mut history: Mut<CommandHistory>| {
                history.undo(world);
            });
        {
            let ast = app.world().resource::<SceneBsnAst>();
            let node = ast.ast_for(entity).expect("linked");
            assert!(
                get_bsn_field(ast, node, type_path, "scale.x").is_none(),
                "undo removes the authored field from the sparse patch"
            );
        }
        assert_eq!(app.world().get::<Transform>(entity).unwrap().scale.x, 1.0);
    }

    /// The `#name` reference patches on an entity's document node.
    fn name_patch_count(ast: &SceneBsnAst, node: Entity) -> usize {
        ast.get_patches(node)
            .map(|patches| {
                patches
                    .0
                    .iter()
                    .filter(|&&pe| {
                        matches!(ast.get_patch(pe), Some(jackdaw_bsn::BsnPatch::Name(_)))
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    /// The document name and the ECS `Name` for an entity, together.
    fn doc_and_ecs_name(app: &App, entity: Entity) -> (Option<String>, Option<String>) {
        let ast = app.world().resource::<SceneBsnAst>();
        let doc = ast
            .ast_for(entity)
            .and_then(|node| ast.get_name(node))
            .map(str::to_string);
        let ecs = app
            .world()
            .get::<Name>(entity)
            .map(|n| n.as_str().to_string());
        (doc, ecs)
    }

    fn name_command(entity: Entity, old: Option<&str>, new: &str) -> SetBsnField {
        SetBsnField {
            entity,
            type_path: NAME_TYPE_PATH.to_string(),
            field_path: String::new(),
            old_value: old.map(|s| BsnValue::String(s.to_string())),
            new_value: BsnValue::String(new.to_string()),
            was_derived: false,
        }
    }

    #[test]
    fn naming_an_unnamed_entity_inserts_the_name_patch_and_undo_removes_it() {
        let mut app = field_app();
        let entity = app.world_mut().spawn_empty().id();
        create_entity_in_ast(app.world_mut(), entity, None);

        let mut command = name_command(entity, None, "Hero");
        command.execute(app.world_mut());
        assert_eq!(
            doc_and_ecs_name(&app, entity),
            (Some("Hero".to_string()), Some("Hero".to_string())),
            "document and ECS agree after execute"
        );

        command.undo(app.world_mut());
        assert_eq!(
            doc_and_ecs_name(&app, entity),
            (None, None),
            "undo removes the name from both document and ECS"
        );
    }

    #[test]
    fn renaming_replaces_the_existing_name_patch_and_undo_restores_it() {
        let mut app = field_app();
        let entity = app.world_mut().spawn(Name::new("Old")).id();
        // create_entity_in_ast seeds the node's name patch from the ECS Name.
        create_entity_in_ast(app.world_mut(), entity, None);

        let mut command = name_command(entity, Some("Old"), "New");
        command.execute(app.world_mut());
        assert_eq!(
            doc_and_ecs_name(&app, entity),
            (Some("New".to_string()), Some("New".to_string()))
        );
        {
            let ast = app.world().resource::<SceneBsnAst>();
            let node = ast.ast_for(entity).unwrap();
            assert_eq!(
                name_patch_count(ast, node),
                1,
                "a rename replaces the patch instead of stacking a second one"
            );
        }

        command.undo(app.world_mut());
        assert_eq!(
            doc_and_ecs_name(&app, entity),
            (Some("Old".to_string()), Some("Old".to_string())),
            "undo restores the old name in both document and ECS"
        );
        let ast = app.world().resource::<SceneBsnAst>();
        let node = ast.ast_for(entity).unwrap();
        assert_eq!(name_patch_count(ast, node), 1);
    }

    #[test]
    fn renaming_to_an_empty_string_removes_the_name_and_undo_restores_it() {
        let mut app = field_app();
        let entity = app.world_mut().spawn(Name::new("Old")).id();
        create_entity_in_ast(app.world_mut(), entity, None);

        let mut command = name_command(entity, Some("Old"), "");
        command.execute(app.world_mut());
        assert_eq!(
            doc_and_ecs_name(&app, entity),
            (None, None),
            "an empty name removes the patch and the ECS Name"
        );

        command.undo(app.world_mut());
        assert_eq!(
            doc_and_ecs_name(&app, entity),
            (Some("Old".to_string()), Some("Old".to_string()))
        );
    }
}

#[cfg(test)]
mod bsn_doc_coherence_tests {
    use super::*;
    use jackdaw_api_internal::snapshot::SceneSnapshotter;
    use jackdaw_bsn::{BsnValue, SceneBsnAst, get_bsn_field};

    #[test]
    fn undo_respawn_rebuilds_the_bsn_document() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
        app.init_resource::<SceneBsnAst>();
        app.init_resource::<crate::selection::Selection>();
        app.init_resource::<jackdaw_commands::CommandHistory>();
        // Editor-state resources the snapshotter captures alongside the doc.
        app.init_resource::<crate::brush::EditMode>();
        app.init_resource::<crate::active_tool::ActiveTool>();
        app.init_resource::<crate::gizmos::GizmoSpace>();
        app.init_resource::<crate::snapping::SnapSettings>();
        app.init_resource::<crate::view_modes::ViewModeSettings>();
        app.init_resource::<crate::viewport_overlays::OverlaySettings>();
        app.init_resource::<jackdaw_avian_integration::PhysicsOverlayConfig>();
        app.init_resource::<crate::viewport_select::GroupEditState>();

        let entity = app
            .world_mut()
            .spawn((
                Name::new("Node"),
                Transform::from_xyz(1.0, 0.0, 0.0),
                jackdaw_scene_types::SceneRootTag,
            ))
            .id();
        crate::scene_io::register_entity_in_ast(app.world_mut(), entity);

        // Snapshot through the document snapshotter (the undo baseline).
        let snapshot = crate::undo_snapshot::BsnDocumentSnapshotter.capture(app.world_mut());

        let type_path = "bevy_transform::components::transform::Transform";
        let mut command = SetBsnField {
            entity,
            type_path: type_path.to_string(),
            field_path: "translation.x".to_string(),
            old_value: Some(BsnValue::Float(1.0)),
            new_value: BsnValue::Float(9.0),
            was_derived: false,
        };
        command.execute(app.world_mut());

        // Undo restore path: respawn the world from the snapshot.
        snapshot.apply(app.world_mut());

        // The respawn re-minted the entity; the document must follow it.
        let mut query = app
            .world_mut()
            .query_filtered::<Entity, With<jackdaw_scene_types::SceneRootTag>>();
        let respawned = query.single(app.world()).expect("respawned entity");
        let ast = app.world().resource::<SceneBsnAst>();
        let pe = ast
            .ast_for(respawned)
            .expect("document links the respawned entity");
        let value = get_bsn_field(ast, pe, type_path, "translation.x");
        assert!(
            matches!(value, Some(BsnValue::Float(x)) if (x - 1.0).abs() < 1e-6),
            "document reflects the restored value"
        );
        assert!(
            ast.ast_for(entity).is_none() || entity == respawned,
            "no stale link to the pre-undo entity"
        );
    }
}
