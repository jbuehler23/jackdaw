//! Tracks user-originated live edits (inspector edits sent to the running
//! game) with their authored baselines, so they can be marked in the
//! inspector, reviewed in the Live Changes tray, saved to the scene, or
//! reverted. Game-driven component changes are never tracked; only edits the
//! inspector itself dispatches.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_jsn::{JsnNodeId, SceneJsnAst};

use crate::commands::{CommandGroup, CommandHistory, EditorCommand, SetJsnField};

/// Identifies one tracked field. Keyed by the game-side entity bits only:
/// the authored node id is resolved against whatever AST is open at record
/// time, so keeping it in the key would split one logical edit into two
/// entries (with conflicting baselines) across tab switches.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LiveEditKey {
    /// Game-side entity bits (always present; the edit went to the game).
    pub bits: u64,
    pub type_path: String,
    pub field_path: String,
}

/// One tracked edit: the authored field value at first edit and the latest
/// live field value sent to the game.
#[derive(Debug, Clone)]
pub struct LiveEditEntry {
    /// Authored node id when the edited entity maps to the open scene.
    /// Backfilled on a later edit if the first edit could not resolve one;
    /// a resolved id is never overwritten by a failed resolution.
    pub node_id: Option<u64>,
    /// Authored value of the field when first edited; `None` when the
    /// component (or entity) is not authored at all.
    pub baseline: Option<serde_json::Value>,
    /// Latest field value the inspector sent to the game.
    pub live_value: serde_json::Value,
    /// Display label captured at record time (entity name + field), so the
    /// tray can label entries even after the entity despawns.
    pub label: String,
}

/// Which per-entry action the tray asked for. Stashed on [`LiveEditLog`]
/// together with the entry key before the matching operator is dispatched;
/// the operator consumes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveEditAction {
    Save,
    Revert,
}

/// All tracked edits for the focused instance, in first-edit order. Cleared
/// on focus change; the stop path prompts before clearing.
#[derive(Resource, Default)]
pub struct LiveEditLog {
    pub entries: Vec<(LiveEditKey, LiveEditEntry)>,
    /// Set by the tray before dispatching `pie.live_edit_save` or
    /// `pie.live_edit_revert`; taken by the operator. Operator parameters
    /// are flat property values, so the struct key travels through the
    /// resource instead, the same way selection-driven operators read
    /// `Selection` rather than receive an entity parameter.
    pub pending_action: Option<(LiveEditAction, LiveEditKey)>,
}

impl LiveEditLog {
    pub fn get_mut(&mut self, key: &LiveEditKey) -> Option<&mut LiveEditEntry> {
        self.entries
            .iter_mut()
            .find_map(|(k, e)| (k == key).then_some(e))
    }

    pub fn remove(&mut self, key: &LiveEditKey) {
        self.entries.retain(|(k, _)| k != key);
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The authored node id for a preview entity, when it maps to the open
/// scene's AST. `None` for ephemeral projected entities.
pub fn node_id_for_entity(world: &World, preview: Entity) -> Option<u64> {
    let ast = world.resource::<SceneJsnAst>();
    ast.ecs_to_jsn
        .get(&preview)
        .and_then(|&idx| ast.nodes.get(idx))
        .and_then(|node| node.id)
        .map(|id| id.0)
}

/// Reverse lookup of the game-entity bits behind a projected preview
/// entity. `None` when the entity is not a live projection.
pub(crate) fn live_bits_for_preview(
    projection: &crate::pie_projection::PieProjection,
    preview: Entity,
) -> Option<u64> {
    projection
        .by_bits
        .iter()
        .find_map(|(b, &e)| (e == preview).then_some(*b))
}

/// Compact JSON of an authored baseline for the inspector's live-edit
/// dot tooltip. `None` baselines render as "(not authored)"; values
/// longer than 40 characters truncate with a trailing ellipsis.
pub(crate) fn truncate_json_for_display(value: Option<&serde_json::Value>) -> String {
    truncate_json_to(value, 40)
}

/// Compact JSON capped at `max` characters with a trailing ellipsis.
/// `None` renders as "(not authored)". The tray uses a tighter cap
/// than the tooltip so two values fit on one row.
pub(crate) fn truncate_json_to(value: Option<&serde_json::Value>, max: usize) -> String {
    let Some(value) = value else {
        return "(not authored)".to_string();
    };
    let json = serde_json::to_string(value).unwrap_or_default();
    if json.chars().count() <= max {
        return json;
    }
    let head: String = json.chars().take(max).collect();
    format!("{head}...")
}

/// Record one live edit. Called from the inspector's live dispatch with the
/// data it already has in hand. The baseline (the authored field value) is
/// captured only on the first edit of a field; later edits update the live
/// value and backfill the node id if the first edit could not resolve one.
pub fn record_live_edit(
    world: &mut World,
    preview: Entity,
    bits: u64,
    type_path: &str,
    field_path: &str,
    field_value: serde_json::Value,
) {
    let node_id = node_id_for_entity(world, preview);
    let key = LiveEditKey {
        bits,
        type_path: type_path.to_string(),
        field_path: field_path.to_string(),
    };
    if let Some(entry) = world.resource_mut::<LiveEditLog>().get_mut(&key) {
        entry.live_value = field_value;
        entry.node_id = entry.node_id.or(node_id);
        return;
    }
    // A zone hot-reload respawns the same authored node under new bits, so the
    // exact-key lookup misses. Re-key the existing entry for this node and
    // field instead of duplicating it, keeping its baseline and label.
    if let Some(node_id) = node_id {
        let mut log = world.resource_mut::<LiveEditLog>();
        if let Some((existing_key, existing_entry)) = log.entries.iter_mut().find(|(k, e)| {
            e.node_id == Some(node_id) && k.type_path == type_path && k.field_path == field_path
        }) {
            existing_key.bits = bits;
            existing_entry.live_value = field_value;
            return;
        }
    }
    let registry = world.resource::<AppTypeRegistry>().clone();
    let baseline = {
        let registry = registry.read();
        world
            .resource::<SceneJsnAst>()
            .get_component(preview, type_path)
            .and_then(|component| {
                jackdaw_jsn::ast::get_field_in_component_json(
                    component, type_path, field_path, &registry,
                )
            })
            .cloned()
    };
    let name = world
        .get::<Name>(preview)
        .map(|n| n.as_str().to_string())
        .unwrap_or_else(|| format!("entity {bits:x}"));
    let short_type = type_path.rsplit("::").next().unwrap_or(type_path);
    let label = if field_path.is_empty() {
        format!("{name} / {short_type}")
    } else {
        format!("{name} / {short_type}.{field_path}")
    };
    world.resource_mut::<LiveEditLog>().entries.push((
        key,
        LiveEditEntry {
            node_id,
            baseline,
            live_value: field_value,
            label,
        },
    ));
}

/// Resolve the editor entity an entry currently refers to: the authored
/// entity for the entry's node id when mapped, else the projected preview
/// for the key's bits. `None` when neither resolves (entity despawned and
/// unmapped).
pub fn resolve_entry_entity(
    ast: &SceneJsnAst,
    projection: &crate::pie_projection::PieProjection,
    key: &LiveEditKey,
    entry: &LiveEditEntry,
) -> Option<Entity> {
    if let Some(node_id) = entry.node_id
        && let Some(entity) = ast.entity_for_node_id(JsnNodeId(node_id))
    {
        return Some(entity);
    }
    projection.by_bits.get(&key.bits).copied()
}

/// The current full JSON of `entity`'s `type_path` component, serialized
/// through reflection. `Value::Null` when the type is unregistered, the
/// entity is gone, or the component is absent.
pub(crate) fn serialize_component_json(
    world: &World,
    entity: Entity,
    type_path: &str,
) -> serde_json::Value {
    use bevy::ecs::reflect::ReflectComponent;
    use bevy::reflect::serde::TypedReflectSerializer;

    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let Some(registration) = registry.get_with_type_path(type_path) else {
        return serde_json::Value::Null;
    };
    let Ok(entity_ref) = world.get_entity(entity) else {
        return serde_json::Value::Null;
    };
    registration
        .data::<ReflectComponent>()
        .and_then(|rc| rc.reflect(entity_ref))
        .and_then(|reflected| {
            let serializer = TypedReflectSerializer::new(reflected, &registry);
            serde_json::to_value(&serializer).ok()
        })
        .unwrap_or(serde_json::Value::Null)
}

/// Look up an entry by key, cloned out of the log.
fn cloned_entry(world: &World, key: &LiveEditKey) -> Option<LiveEditEntry> {
    world
        .resource::<LiveEditLog>()
        .entries
        .iter()
        .find_map(|(k, e)| (k == key).then(|| e.clone()))
}

/// The live preview entity for `bits`, if it still exists.
fn live_preview_for_bits(world: &World, bits: u64) -> Option<Entity> {
    world
        .resource::<crate::pie_projection::PieProjection>()
        .by_bits
        .get(&bits)
        .copied()
        .filter(|&e| world.get_entity(e).is_ok())
}

/// Build the save command for one tracked edit on an entity bound to an
/// authored node. Edits with an authored baseline write the field; edits
/// without one belong to a component absent from the node, so the whole
/// component is serialized from the entity and authored in one write (a
/// field-level write would no-op on the missing component key). `None` when
/// the entity no longer carries the component.
fn build_save_command(
    world: &World,
    entity: Entity,
    key: &LiveEditKey,
    entry: &LiveEditEntry,
) -> Option<Box<dyn EditorCommand>> {
    match entry.baseline.clone() {
        Some(baseline) => Some(Box::new(SetJsnField {
            entity,
            type_path: key.type_path.clone(),
            field_path: key.field_path.clone(),
            old_value: baseline,
            new_value: entry.live_value.clone(),
            was_derived: false,
        })),
        None => {
            let full = serialize_component_json(world, entity, &key.type_path);
            if full.is_null() {
                return None;
            }
            Some(Box::new(SetJsnField {
                entity,
                type_path: key.type_path.clone(),
                field_path: String::new(),
                old_value: serde_json::Value::Null,
                new_value: full,
                was_derived: false,
            }))
        }
    }
}

/// Write one tracked edit into the authored scene as an undoable command,
/// then drop the entry. Entries with an authored node write through
/// [`SetJsnField`]; entries without one belong to a runtime-spawned entity
/// and route through whole-entity promotion, which authors the entity and
/// every tracked field on it in one step.
pub fn save_entry_to_scene(world: &mut World, key: &LiveEditKey) {
    let Some(entry) = cloned_entry(world, key) else {
        warn!(
            "save live edit: no tracked entry for {}.{}",
            key.type_path, key.field_path
        );
        return;
    };

    if entry.node_id.is_some() {
        let ast = world.resource::<SceneJsnAst>();
        let projection = world.resource::<crate::pie_projection::PieProjection>();
        let Some(entity) = resolve_entry_entity(ast, projection, key, &entry) else {
            warn!(
                "save live edit: entity for '{}' no longer resolves",
                entry.label
            );
            return;
        };
        let Some(mut cmd) = build_save_command(world, entity, key, &entry) else {
            warn!(
                "save live edit: {} no longer has {}, keeping the entry",
                entry.label, key.type_path
            );
            return;
        };
        cmd.execute(world);
        world.resource_mut::<CommandHistory>().push_executed(cmd);
        world.resource_mut::<LiveEditLog>().remove(key);
        return;
    }

    // No authored node: promote the whole runtime entity into the scene,
    // which authors every tracked field on it at once.
    let Some(preview) = live_preview_for_bits(world, key.bits) else {
        warn!(
            "save live edit: entity for '{}' no longer resolves",
            entry.label
        );
        return;
    };
    crate::pie::promote_ephemeral_to_authored(world, preview, key.bits);
    world
        .resource_mut::<LiveEditLog>()
        .entries
        .retain(|(k, _)| k.bits != key.bits);
}

/// Send the authored baseline back to the running game for one tracked
/// field, restoring what the scene says, then drop the entry. A no-op with a
/// warn when the entry has no baseline or its entity no longer resolves.
pub fn revert_entry(world: &mut World, key: &LiveEditKey) {
    use jackdaw_pie_protocol::ControlEvent;

    let Some(entry) = cloned_entry(world, key) else {
        warn!(
            "revert live edit: no tracked entry for {}.{}",
            key.type_path, key.field_path
        );
        return;
    };
    let Some(baseline) = entry.baseline.clone() else {
        warn!(
            "revert live edit: '{}' has no authored baseline",
            entry.label
        );
        return;
    };
    let entity = {
        let ast = world.resource::<SceneJsnAst>();
        let projection = world.resource::<crate::pie_projection::PieProjection>();
        resolve_entry_entity(ast, projection, key, &entry)
    };
    let Some(entity) = entity else {
        warn!(
            "revert live edit: entity for '{}' no longer resolves",
            entry.label
        );
        return;
    };

    // The running game may have respawned the entity under new bits since the
    // edit was recorded, so address the revert through the current projection
    // rather than the bits stored in the key.
    let current_bits = {
        let projection = world.resource::<crate::pie_projection::PieProjection>();
        live_bits_for_preview(projection, entity)
    };
    let Some(current_bits) = current_bits else {
        warn!(
            "revert live edit: '{}' has no live counterpart right now, keeping the entry",
            entry.label
        );
        return;
    };

    // Merge the baseline field into the entity's current full component
    // JSON so the game receives a complete canonical component value.
    let registry = world.resource::<AppTypeRegistry>().clone();
    let mut merged = serialize_component_json(world, entity, &key.type_path);
    {
        let registry = registry.read();
        jackdaw_jsn::ast::set_field_in_component_json(
            &mut merged,
            &key.type_path,
            &key.field_path,
            baseline,
            &registry,
        );
    }
    // The entity lost the component: the serialize returned Null and the
    // field merge had nothing to write into. Sending that to the game would
    // set a null component, so keep the entry instead. An empty field path
    // carries a whole-component baseline and may legitimately re-add it.
    if merged.is_null() && !key.field_path.is_empty() {
        warn!(
            "live edit revert: {} no longer has {}, keeping the entry",
            entry.label, key.type_path
        );
        return;
    }

    crate::pie::send_control_to_focused(
        world,
        ControlEvent::SetComponent {
            entity: current_bits,
            type_path: key.type_path.clone(),
            value: merged.clone(),
        },
    );
    crate::pie_projection::apply_component_value(world, entity, &key.type_path, &merged);
    world.resource_mut::<LiveEditLog>().remove(key);
}

/// Save every tracked edit as one undoable command group, then clear the
/// log. Entries bound to authored nodes batch into a single history entry;
/// entries without one promote their runtime entity (once per entity).
/// Entries whose entity no longer resolves stay in the log with a warn.
pub fn apply_all_to_scene(world: &mut World) {
    let entries: Vec<(LiveEditKey, LiveEditEntry)> =
        world.resource::<LiveEditLog>().entries.clone();
    if entries.is_empty() {
        return;
    }

    let mut sub_commands: Vec<Box<dyn EditorCommand>> = Vec::new();
    let mut promote_bits: Vec<u64> = Vec::new();
    let mut handled: Vec<LiveEditKey> = Vec::new();
    let mut stale = 0usize;

    for (key, entry) in &entries {
        if entry.node_id.is_some() {
            let ast = world.resource::<SceneJsnAst>();
            let projection = world.resource::<crate::pie_projection::PieProjection>();
            let Some(entity) = resolve_entry_entity(ast, projection, key, entry) else {
                stale += 1;
                continue;
            };
            let Some(cmd) = build_save_command(world, entity, key, entry) else {
                stale += 1;
                continue;
            };
            sub_commands.push(cmd);
            handled.push(key.clone());
        } else if live_preview_for_bits(world, key.bits).is_some() {
            if !promote_bits.contains(&key.bits) {
                promote_bits.push(key.bits);
            }
            handled.push(key.clone());
        } else {
            stale += 1;
        }
    }

    let count = sub_commands.len();
    if count == 1 {
        if let Some(mut only) = sub_commands.into_iter().next() {
            only.execute(world);
            world.resource_mut::<CommandHistory>().push_executed(only);
        }
    } else if count > 1 {
        let mut cmd: Box<dyn EditorCommand> = Box::new(CommandGroup {
            label: "Apply live edits to scene".to_string(),
            commands: sub_commands,
        });
        cmd.execute(world);
        world.resource_mut::<CommandHistory>().push_executed(cmd);
    }

    for bits in promote_bits {
        if let Some(preview) = live_preview_for_bits(world, bits) {
            crate::pie::promote_ephemeral_to_authored(world, preview, bits);
        }
    }

    world
        .resource_mut::<LiveEditLog>()
        .entries
        .retain(|(k, _)| !handled.contains(k));
    if stale > 0 {
        warn!("apply live edits: {stale} stale entries kept in the log");
    }
}

/// Forget every tracked edit without touching the scene or the game.
pub fn discard_all(world: &mut World) {
    let mut log = world.resource_mut::<LiveEditLog>();
    log.entries.clear();
    log.pending_action = None;
}

fn live_edit_action_pending(log: Res<LiveEditLog>) -> bool {
    log.pending_action.is_some()
}

fn live_edit_log_has_entries(log: Res<LiveEditLog>) -> bool {
    !log.is_empty()
}

/// Save the pending tracked edit (set by the tray) into the authored scene.
#[operator(
    id = "pie.live_edit_save",
    label = "Save Live Edit",
    description = "Write the pending live edit into the authored scene.",
    is_available = live_edit_action_pending
)]
pub(crate) fn pie_live_edit_save(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        let Some((action, key)) = world.resource_mut::<LiveEditLog>().pending_action.take() else {
            warn!("live edit save: no pending action set");
            return;
        };
        if action != LiveEditAction::Save {
            warn!("live edit save: pending action is {action:?}");
            return;
        }
        save_entry_to_scene(world, &key);
    });
    OperatorResult::Finished
}

/// Revert the pending tracked edit (set by the tray) in the running game.
#[operator(
    id = "pie.live_edit_revert",
    label = "Revert Live Edit",
    description = "Send the authored value of the pending live edit back to the running game.",
    is_available = live_edit_action_pending
)]
pub(crate) fn pie_live_edit_revert(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        let Some((action, key)) = world.resource_mut::<LiveEditLog>().pending_action.take() else {
            warn!("live edit revert: no pending action set");
            return;
        };
        if action != LiveEditAction::Revert {
            warn!("live edit revert: pending action is {action:?}");
            return;
        }
        revert_entry(world, &key);
    });
    OperatorResult::Finished
}

/// Save every tracked live edit into the authored scene.
#[operator(
    id = "pie.live_edits_apply_all",
    label = "Apply All Live Edits",
    description = "Write every tracked live edit into the authored scene.",
    is_available = live_edit_log_has_entries
)]
pub(crate) fn pie_live_edits_apply_all(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(apply_all_to_scene);
    OperatorResult::Finished
}

/// Drop every tracked live edit without saving or reverting.
#[operator(
    id = "pie.live_edits_discard_all",
    label = "Discard All Live Edits",
    description = "Forget every tracked live edit without touching the scene or the game.",
    is_available = live_edit_log_has_entries
)]
pub(crate) fn pie_live_edits_discard_all(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(discard_all);
    OperatorResult::Finished
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pie_projection::PieProjection;
    use jackdaw_jsn::ast::JsnEntityNode;
    use std::collections::{HashMap, HashSet};

    const TRANSFORM_PATH: &str = "bevy_transform::components::transform::Transform";

    #[derive(Component, Reflect, Default)]
    #[reflect(Component)]
    struct Health {
        current: f32,
    }

    fn health_path() -> String {
        <Health as bevy::reflect::TypePath>::type_path().to_string()
    }

    fn transform_json() -> serde_json::Value {
        serde_json::json!({
            "translation": [1.0, 2.0, 3.0],
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "scale": [1.0, 1.0, 1.0],
        })
    }

    fn build_world() -> (World, Entity, JsnNodeId) {
        let mut world = World::new();
        world.init_resource::<AppTypeRegistry>();
        world.init_resource::<PieProjection>();
        world.init_resource::<LiveEditLog>();
        world.init_resource::<CommandHistory>();
        {
            let registry = world.resource::<AppTypeRegistry>().clone();
            let mut w = registry.write();
            w.register::<Transform>();
            w.register::<Health>();
        }

        // One authored node bound to a preview entity, carrying a Transform.
        let preview_entity = world
            .spawn((Name::new("player"), Transform::from_xyz(1.0, 2.0, 3.0)))
            .id();
        let node_id = JsnNodeId::next();
        let mut components = HashMap::new();
        components.insert(TRANSFORM_PATH.to_string(), transform_json());
        let mut ast = SceneJsnAst::default();
        ast.nodes.push(JsnEntityNode {
            id: Some(node_id),
            parent: None,
            components,
            derived_components: HashSet::new(),
            ecs_entity: Some(preview_entity),
        });
        ast.ecs_to_jsn.insert(preview_entity, 0);
        world.insert_resource(ast);

        (world, preview_entity, node_id)
    }

    #[test]
    fn live_bits_reverse_resolves_projected_entities() {
        let mut world = World::new();
        world.init_resource::<PieProjection>();
        let preview = world.spawn_empty().id();
        let other = world.spawn_empty().id();
        world
            .resource_mut::<PieProjection>()
            .by_bits
            .insert(7, preview);

        let projection = world.resource::<PieProjection>();
        assert_eq!(live_bits_for_preview(projection, preview), Some(7));
        assert_eq!(live_bits_for_preview(projection, other), None);
    }

    #[test]
    fn truncate_json_for_display_truncates_and_marks_unauthored() {
        assert_eq!(truncate_json_for_display(None), "(not authored)");

        let short = serde_json::json!([1.0, 2.0]);
        assert_eq!(truncate_json_for_display(Some(&short)), "[1.0,2.0]");

        let long = serde_json::json!("a".repeat(48));
        let display = truncate_json_for_display(Some(&long));
        assert_eq!(display.chars().count(), 43, "40 chars plus the ellipsis");
        assert!(display.ends_with("..."));
        assert!(display.starts_with("\"aaa"));
    }

    #[test]
    fn truncate_json_to_respects_a_custom_cap() {
        assert_eq!(truncate_json_to(None, 24), "(not authored)");

        let short = serde_json::json!([1.0, 2.0]);
        assert_eq!(truncate_json_to(Some(&short), 24), "[1.0,2.0]");

        let long = serde_json::json!("a".repeat(48));
        let display = truncate_json_to(Some(&long), 24);
        assert_eq!(display.chars().count(), 27, "24 chars plus the ellipsis");
        assert!(display.ends_with("..."));
    }

    #[test]
    fn first_edit_captures_baseline_and_second_updates_live_value() {
        let (mut world, preview, node_id) = build_world();

        record_live_edit(
            &mut world,
            preview,
            7,
            TRANSFORM_PATH,
            "translation",
            serde_json::json!([9.0, 2.0, 3.0]),
        );

        {
            let log = world.resource::<LiveEditLog>();
            assert_eq!(log.entries.len(), 1);
            let (key, entry) = &log.entries[0];
            assert_eq!(key.bits, 7);
            assert_eq!(entry.node_id, Some(node_id.0));
            assert_eq!(entry.baseline, Some(serde_json::json!([1.0, 2.0, 3.0])));
            assert_eq!(entry.live_value, serde_json::json!([9.0, 2.0, 3.0]));
            assert_eq!(entry.label, "player / Transform.translation");
        }

        record_live_edit(
            &mut world,
            preview,
            7,
            TRANSFORM_PATH,
            "translation",
            serde_json::json!([5.0, 5.0, 5.0]),
        );

        let log = world.resource::<LiveEditLog>();
        assert_eq!(log.entries.len(), 1, "second edit must not add an entry");
        let (_, entry) = &log.entries[0];
        assert_eq!(
            entry.baseline,
            Some(serde_json::json!([1.0, 2.0, 3.0])),
            "baseline is captured once, on first edit"
        );
        assert_eq!(entry.live_value, serde_json::json!([5.0, 5.0, 5.0]));
    }

    #[test]
    fn unauthored_component_records_a_none_baseline() {
        let (mut world, preview, _node_id) = build_world();

        record_live_edit(
            &mut world,
            preview,
            7,
            "some_game::Health",
            "current",
            serde_json::json!(50.0),
        );

        let log = world.resource::<LiveEditLog>();
        assert_eq!(log.entries.len(), 1);
        let (_, entry) = &log.entries[0];
        assert_eq!(entry.baseline, None);
        assert_eq!(entry.live_value, serde_json::json!(50.0));
    }

    #[test]
    fn unmapped_entity_keys_by_bits_only() {
        let (mut world, _preview, _node_id) = build_world();

        // Ephemeral preview entity: projected from the game but not in the AST.
        let ephemeral = world.spawn_empty().id();
        world
            .resource_mut::<PieProjection>()
            .by_bits
            .insert(42, ephemeral);

        record_live_edit(
            &mut world,
            ephemeral,
            42,
            TRANSFORM_PATH,
            "translation",
            serde_json::json!([0.0, 1.0, 0.0]),
        );

        let (key, entry) = {
            let log = world.resource::<LiveEditLog>();
            assert_eq!(log.entries.len(), 1);
            let (key, entry) = &log.entries[0];
            assert_eq!(entry.node_id, None);
            assert_eq!(key.bits, 42);
            (key.clone(), entry.clone())
        };
        assert_eq!(
            resolve_entry_entity(
                world.resource::<SceneJsnAst>(),
                world.resource::<PieProjection>(),
                &key,
                &entry
            ),
            Some(ephemeral)
        );
    }

    #[test]
    fn resolve_prefers_node_id_over_bits() {
        let (mut world, preview, node_id) = build_world();

        // A stale by_bits mapping must not win over the authored node id.
        let stale = world.spawn_empty().id();
        world
            .resource_mut::<PieProjection>()
            .by_bits
            .insert(7, stale);

        let key = LiveEditKey {
            bits: 7,
            type_path: TRANSFORM_PATH.to_string(),
            field_path: "translation".to_string(),
        };
        let entry = LiveEditEntry {
            node_id: Some(node_id.0),
            baseline: None,
            live_value: serde_json::Value::Null,
            label: String::new(),
        };
        assert_eq!(
            resolve_entry_entity(
                world.resource::<SceneJsnAst>(),
                world.resource::<PieProjection>(),
                &key,
                &entry
            ),
            Some(preview)
        );
    }

    #[test]
    fn re_edit_after_node_id_loss_keeps_one_entry_and_baseline() {
        let (mut world, preview, node_id) = build_world();

        record_live_edit(
            &mut world,
            preview,
            7,
            TRANSFORM_PATH,
            "translation",
            serde_json::json!([9.0, 2.0, 3.0]),
        );

        // Same game entity re-edited through a preview entity that no longer
        // resolves to an authored node (e.g. after a tab switch rebuilt the
        // AST around different editor entities).
        let unbound = world.spawn_empty().id();
        record_live_edit(
            &mut world,
            unbound,
            7,
            TRANSFORM_PATH,
            "translation",
            serde_json::json!([4.0, 4.0, 4.0]),
        );

        let log = world.resource::<LiveEditLog>();
        assert_eq!(log.entries.len(), 1, "re-edit must reuse the entry");
        let (_, entry) = &log.entries[0];
        assert_eq!(
            entry.baseline,
            Some(serde_json::json!([1.0, 2.0, 3.0])),
            "baseline survives a re-edit that lost the node id"
        );
        assert_eq!(entry.live_value, serde_json::json!([4.0, 4.0, 4.0]));
        assert_eq!(
            entry.node_id,
            Some(node_id.0),
            "a resolved node id is never overwritten by a failed resolution"
        );
    }

    #[test]
    fn re_edit_after_respawn_rekeys_instead_of_duplicating() {
        let (mut world, preview, _node_id) = build_world();
        record_live_edit(
            &mut world,
            preview,
            0xA1,
            TRANSFORM_PATH,
            "translation",
            serde_json::json!([4.0, 2.0, 3.0]),
        );
        // The zone hot-reload respawned the same authored node under new game
        // bits; re-editing the same field must reuse the entry, not duplicate.
        record_live_edit(
            &mut world,
            preview,
            0xB2,
            TRANSFORM_PATH,
            "translation",
            serde_json::json!([5.0, 2.0, 3.0]),
        );
        let log = world.resource::<LiveEditLog>();
        assert_eq!(log.entries.len(), 1, "same logical field, one entry");
        assert_eq!(log.entries[0].0.bits, 0xB2, "re-keyed to the live bits");
        assert_eq!(
            log.entries[0].1.baseline,
            Some(serde_json::json!([1.0, 2.0, 3.0])),
            "baseline survives the re-key"
        );
        assert_eq!(
            log.entries[0].1.live_value,
            serde_json::json!([5.0, 2.0, 3.0]),
            "the latest live value is kept"
        );
    }

    #[test]
    fn unnamed_entity_label_uses_bits() {
        let (mut world, _preview, _node_id) = build_world();

        let ephemeral = world.spawn_empty().id();
        record_live_edit(
            &mut world,
            ephemeral,
            42,
            TRANSFORM_PATH,
            "translation",
            serde_json::json!([0.0, 1.0, 0.0]),
        );

        let log = world.resource::<LiveEditLog>();
        let (_, entry) = &log.entries[0];
        assert_eq!(entry.label, "entity 2a / Transform.translation");
    }

    #[test]
    fn empty_field_path_label_omits_field_segment() {
        let (mut world, preview, _node_id) = build_world();

        record_live_edit(&mut world, preview, 7, TRANSFORM_PATH, "", transform_json());

        let log = world.resource::<LiveEditLog>();
        let (_, entry) = &log.entries[0];
        assert_eq!(entry.label, "player / Transform");
    }

    fn key_for(bits: u64, field_path: &str) -> LiveEditKey {
        LiveEditKey {
            bits,
            type_path: TRANSFORM_PATH.to_string(),
            field_path: field_path.to_string(),
        }
    }

    fn ast_field(world: &World, entity: Entity, field_path: &str) -> Option<serde_json::Value> {
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        world
            .resource::<SceneJsnAst>()
            .get_component(entity, TRANSFORM_PATH)
            .and_then(|component| {
                jackdaw_jsn::ast::get_field_in_component_json(
                    component,
                    TRANSFORM_PATH,
                    field_path,
                    &registry,
                )
            })
            .cloned()
    }

    #[test]
    fn save_entry_writes_the_ast_field_and_drops_the_entry() {
        let (mut world, preview, _node_id) = build_world();

        record_live_edit(
            &mut world,
            preview,
            7,
            TRANSFORM_PATH,
            "translation",
            serde_json::json!([9.0, 2.0, 3.0]),
        );

        save_entry_to_scene(&mut world, &key_for(7, "translation"));

        assert_eq!(
            ast_field(&world, preview, "translation"),
            Some(serde_json::json!([9.0, 2.0, 3.0])),
            "the AST holds the saved live value"
        );
        assert!(world.resource::<LiveEditLog>().is_empty());
        assert_eq!(world.resource::<CommandHistory>().undo_stack.len(), 1);
    }

    #[test]
    fn revert_requires_a_baseline() {
        let (mut world, preview, _node_id) = build_world();

        // The running game still has a live counterpart for the preview entity;
        // revert addresses it through the projection.
        world
            .resource_mut::<PieProjection>()
            .by_bits
            .insert(7, preview);

        // Unauthored component: no baseline, revert must keep the entry.
        record_live_edit(
            &mut world,
            preview,
            7,
            "some_game::Health",
            "current",
            serde_json::json!(50.0),
        );
        let no_baseline_key = LiveEditKey {
            bits: 7,
            type_path: "some_game::Health".to_string(),
            field_path: "current".to_string(),
        };
        revert_entry(&mut world, &no_baseline_key);
        assert_eq!(
            world.resource::<LiveEditLog>().entries.len(),
            1,
            "an entry without a baseline is kept on revert"
        );

        // Authored field: revert restores the baseline and drops the entry.
        record_live_edit(
            &mut world,
            preview,
            7,
            TRANSFORM_PATH,
            "translation",
            serde_json::json!([9.0, 2.0, 3.0]),
        );
        revert_entry(&mut world, &key_for(7, "translation"));
        let log = world.resource::<LiveEditLog>();
        assert_eq!(log.entries.len(), 1, "the authored entry was removed");
        assert_eq!(log.entries[0].0.type_path, "some_game::Health");
        assert_eq!(
            world.get::<Transform>(preview).map(|t| t.translation),
            Some(Vec3::new(1.0, 2.0, 3.0)),
            "the preview entity shows the reverted baseline"
        );
    }

    #[test]
    fn apply_all_groups_into_one_history_entry_and_clears() {
        let (mut world, preview, _node_id) = build_world();

        record_live_edit(
            &mut world,
            preview,
            7,
            TRANSFORM_PATH,
            "translation",
            serde_json::json!([9.0, 2.0, 3.0]),
        );
        record_live_edit(
            &mut world,
            preview,
            7,
            TRANSFORM_PATH,
            "scale",
            serde_json::json!([2.0, 2.0, 2.0]),
        );

        apply_all_to_scene(&mut world);

        assert_eq!(
            ast_field(&world, preview, "translation"),
            Some(serde_json::json!([9.0, 2.0, 3.0]))
        );
        assert_eq!(
            ast_field(&world, preview, "scale"),
            Some(serde_json::json!([2.0, 2.0, 2.0]))
        );
        assert!(world.resource::<LiveEditLog>().is_empty());
        assert_eq!(
            world.resource::<CommandHistory>().undo_stack.len(),
            1,
            "both edits land as one undoable group"
        );
    }

    #[test]
    fn post_stop_apply_writes_via_node_id() {
        let (mut world, preview, _node_id) = build_world();

        world
            .resource_mut::<PieProjection>()
            .by_bits
            .insert(7, preview);
        record_live_edit(
            &mut world,
            preview,
            7,
            TRANSFORM_PATH,
            "translation",
            serde_json::json!([9.0, 2.0, 3.0]),
        );
        // Stop teardown drops every projected mapping; resolution must go
        // through the entry's authored node id instead.
        world.resource_mut::<PieProjection>().by_bits.clear();

        apply_all_to_scene(&mut world);

        assert_eq!(
            ast_field(&world, preview, "translation"),
            Some(serde_json::json!([9.0, 2.0, 3.0])),
            "the edit landed in the AST via the node id"
        );
        assert!(world.resource::<LiveEditLog>().is_empty());
        assert_eq!(world.resource::<CommandHistory>().undo_stack.len(), 1);
    }

    #[test]
    fn discard_all_clears_without_touching_ast() {
        let (mut world, preview, _node_id) = build_world();

        record_live_edit(
            &mut world,
            preview,
            7,
            TRANSFORM_PATH,
            "translation",
            serde_json::json!([9.0, 2.0, 3.0]),
        );

        discard_all(&mut world);

        assert!(world.resource::<LiveEditLog>().is_empty());
        assert_eq!(
            ast_field(&world, preview, "translation"),
            Some(serde_json::json!([1.0, 2.0, 3.0])),
            "the authored value is untouched"
        );
        assert_eq!(world.resource::<CommandHistory>().undo_stack.len(), 0);
    }

    #[test]
    fn save_keeps_a_stale_entry_when_nothing_resolves() {
        let (mut world, _preview, _node_id) = build_world();

        // Ephemeral entity tracked, then despawned with its projection gone.
        let ephemeral = world.spawn(Transform::default()).id();
        world
            .resource_mut::<PieProjection>()
            .by_bits
            .insert(42, ephemeral);
        record_live_edit(
            &mut world,
            ephemeral,
            42,
            TRANSFORM_PATH,
            "translation",
            serde_json::json!([0.0, 1.0, 0.0]),
        );
        world.despawn(ephemeral);
        world.resource_mut::<PieProjection>().by_bits.clear();

        save_entry_to_scene(&mut world, &key_for(42, "translation"));

        assert_eq!(
            world.resource::<LiveEditLog>().entries.len(),
            1,
            "a stale entry stays in the log"
        );
        assert_eq!(world.resource::<CommandHistory>().undo_stack.len(), 0);
    }

    /// Inserts an unauthored Health component on the fixture entity, records
    /// a live edit for it, and saves it. Returns the key used for the save.
    fn save_unauthored_health(world: &mut World, preview: Entity) -> LiveEditKey {
        world.entity_mut(preview).insert(Health { current: 50.0 });
        record_live_edit(
            world,
            preview,
            7,
            &health_path(),
            "current",
            serde_json::json!(50.0),
        );
        let key = LiveEditKey {
            bits: 7,
            type_path: health_path(),
            field_path: "current".to_string(),
        };
        save_entry_to_scene(world, &key);
        key
    }

    #[test]
    fn save_authors_an_unauthored_component_on_a_mapped_entity() {
        let (mut world, preview, _node_id) = build_world();

        save_unauthored_health(&mut world, preview);

        assert_eq!(
            world
                .resource::<SceneJsnAst>()
                .get_component(preview, &health_path()),
            Some(&serde_json::json!({ "current": 50.0 })),
            "the whole component is authored on the node"
        );
        assert!(world.resource::<LiveEditLog>().is_empty());
        assert_eq!(world.resource::<CommandHistory>().undo_stack.len(), 1);
    }

    #[test]
    fn undo_of_a_newly_authored_component_removes_it() {
        let (mut world, preview, _node_id) = build_world();

        save_unauthored_health(&mut world, preview);

        let mut cmd = world
            .resource_mut::<CommandHistory>()
            .undo_stack
            .pop()
            .expect("the save pushed one command");
        cmd.undo(&mut world);

        assert_eq!(
            world
                .resource::<SceneJsnAst>()
                .get_component(preview, &health_path()),
            None,
            "undo removes the component entry instead of writing null"
        );
        assert!(
            world.get::<Health>(preview).is_none(),
            "undo removes the ECS component too"
        );
    }

    #[test]
    fn revert_keeps_the_entry_when_the_component_is_gone() {
        let (mut world, preview, _node_id) = build_world();

        record_live_edit(
            &mut world,
            preview,
            7,
            TRANSFORM_PATH,
            "translation",
            serde_json::json!([9.0, 2.0, 3.0]),
        );
        world
            .resource_mut::<PieProjection>()
            .by_bits
            .insert(7, preview);
        world.entity_mut(preview).remove::<Transform>();

        revert_entry(&mut world, &key_for(7, "translation"));

        assert_eq!(
            world.resource::<LiveEditLog>().entries.len(),
            1,
            "the entry stays when the entity lost the component"
        );
    }

    #[test]
    fn revert_without_a_live_counterpart_keeps_the_entry() {
        let (mut world, preview, _node_id) = build_world();

        record_live_edit(
            &mut world,
            preview,
            7,
            TRANSFORM_PATH,
            "translation",
            serde_json::json!([9.0, 2.0, 3.0]),
        );
        // No projection mapping to the preview entity: the running game has no
        // live counterpart right now, so revert keeps the entry and sends
        // nothing.
        assert!(world.resource::<PieProjection>().by_bits.is_empty());

        revert_entry(&mut world, &key_for(7, "translation"));

        assert_eq!(
            world.resource::<LiveEditLog>().entries.len(),
            1,
            "an entry with no live counterpart is kept on revert"
        );
        assert_eq!(
            world.get::<Transform>(preview).map(|t| t.translation),
            Some(Vec3::new(1.0, 2.0, 3.0)),
            "the preview entity is untouched: nothing was applied"
        );
    }

    #[test]
    fn apply_all_keeps_stale_entries_and_handles_the_rest() {
        let (mut world, preview, _node_id) = build_world();

        record_live_edit(
            &mut world,
            preview,
            7,
            TRANSFORM_PATH,
            "translation",
            serde_json::json!([9.0, 2.0, 3.0]),
        );
        // An entry whose bits resolve to nothing: no node id and no
        // projection mapping.
        world.resource_mut::<LiveEditLog>().entries.push((
            key_for(99, "translation"),
            LiveEditEntry {
                node_id: None,
                baseline: None,
                live_value: serde_json::json!([0.0, 0.0, 0.0]),
                label: "ghost".to_string(),
            },
        ));

        apply_all_to_scene(&mut world);

        assert_eq!(
            ast_field(&world, preview, "translation"),
            Some(serde_json::json!([9.0, 2.0, 3.0])),
            "the resolvable edit landed in the AST"
        );
        let log = world.resource::<LiveEditLog>();
        assert_eq!(log.entries.len(), 1, "the stale entry is kept");
        assert_eq!(log.entries[0].0.bits, 99);
        assert_eq!(world.resource::<CommandHistory>().undo_stack.len(), 1);
    }

    #[test]
    fn save_promotes_an_unmapped_entity_and_drops_its_entries() {
        let (mut world, _preview, _node_id) = build_world();

        let ephemeral = world
            .spawn((Name::new("spawned"), Transform::from_xyz(4.0, 5.0, 6.0)))
            .id();
        world
            .resource_mut::<PieProjection>()
            .by_bits
            .insert(42, ephemeral);
        record_live_edit(
            &mut world,
            ephemeral,
            42,
            TRANSFORM_PATH,
            "translation",
            serde_json::json!([4.0, 5.0, 6.0]),
        );
        record_live_edit(
            &mut world,
            ephemeral,
            42,
            TRANSFORM_PATH,
            "scale",
            serde_json::json!([2.0, 2.0, 2.0]),
        );

        save_entry_to_scene(&mut world, &key_for(42, "translation"));

        let ast = world.resource::<SceneJsnAst>();
        assert_eq!(ast.nodes.len(), 2, "promotion authored a new node");
        assert_eq!(ast.nodes[1].ecs_entity, Some(ephemeral));
        assert!(ast.nodes[1].components.contains_key(TRANSFORM_PATH));
        assert!(
            world.resource::<LiveEditLog>().is_empty(),
            "every entry for the promoted entity is dropped"
        );
    }
}
