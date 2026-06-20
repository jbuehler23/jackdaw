//! The `World` applier: resolves a preset into binding entities.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy_enhanced_input::prelude::{
    Binding, BindingOf, InputModKeys, ModKeys, Press, Release, Tap,
};

use crate::keymap_conditions::{DoubleClick, ScrollTick};

use crate::lifecycle::OperatorAction;

use super::types::{
    BuiltinActions, KeymapPreset, PresetContext, PresetInput, PresetPhase, key_code_from_name,
    mouse_button_from_name,
};

/// Outcome of one preset application, for conformance checks and logs.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct KeymapApplyReport {
    /// Entries that bound to at least one action entity.
    pub applied_entries: usize,
    /// Binding entities spawned (>= `applied_entries` when an operator
    /// has multiple action entities).
    pub spawned_bindings: usize,
    pub skipped_unknown_operator: Vec<String>,
    pub skipped_unparseable_key: Vec<String>,
    /// Entries skipped because their input type, phase, or context is not
    /// handled by the applier. Each entry is `"operator-or-name: reason"`.
    pub skipped_unsupported: Vec<String>,
}

/// Marker on binding entities spawned by [`apply_keymap_preset`].
///
/// Re-application first despawns every entity carrying this marker,
/// so it replaces exactly its own previous spawns and never touches
/// bindings that were attached by other means (raw spawn sites, tests).
#[derive(Component)]
pub struct PresetSpawnedBinding;

/// Fully resolved binding, ready to spawn with the correct condition component.
enum ResolvedBinding {
    Press {
        binding: Binding,
        phase: PresetPhase,
    },
    Scroll {
        binding: Binding,
        positive: bool,
    },
}

/// Replace the preset-managed bindings of every operator action with
/// the entries in `preset`.
///
/// Entries that name an unknown operator id or carry an unparseable key
/// name are collected into the returned [`KeymapApplyReport`] rather than
/// warned inline. After the loop, one aggregated `warn!` is emitted per
/// non-empty skip vec.
///
/// Re-application is idempotent: all entities carrying [`PresetSpawnedBinding`]
/// are despawned before any new spawning begins.
pub fn apply_keymap_preset(world: &mut World, preset: &KeymapPreset) -> KeymapApplyReport {
    // Despawn every binding entity previously owned by this applier.
    let stale: Vec<Entity> = world
        .query_filtered::<Entity, With<PresetSpawnedBinding>>()
        .iter(world)
        .collect();
    for entity in stale {
        world.entity_mut(entity).despawn();
    }

    // Build a map from operator id -> list of action entity ids.
    let mut by_operator: HashMap<&'static str, Vec<Entity>> = HashMap::new();
    let mut action_query = world.query::<(Entity, &OperatorAction)>();
    for (entity, tag) in action_query.iter(world) {
        by_operator.entry(tag.0).or_default().push(entity);
    }

    // Snapshot the builtin actions map so we can look up Modal/Navigation
    // entries without holding a reference to the world.
    let builtin_snapshot: std::collections::HashMap<String, Vec<Entity>> = world
        .get_resource::<BuiltinActions>()
        .map(|b| b.map.clone())
        .unwrap_or_default();

    let mut report = KeymapApplyReport::default();
    for entry in &preset.bindings {
        // Resolve the input to a binding + phase shape. Scroll entries always
        // use ScrollTick regardless of the phase field; the phase is ignored.
        let resolved = match &entry.input {
            PresetInput::Key {
                key,
                ctrl,
                shift,
                alt,
            } => {
                let Some(key_code) = key_code_from_name(key) else {
                    report.skipped_unparseable_key.push(key.clone());
                    continue;
                };
                let mod_keys = mod_keys_from_bools(*ctrl, *shift, *alt);
                ResolvedBinding::Press {
                    binding: key_code.with_mod_keys(mod_keys),
                    phase: entry.phase,
                }
            }
            PresetInput::MouseButton {
                button,
                ctrl,
                shift,
                alt,
            } => {
                let Some(mb) = mouse_button_from_name(button) else {
                    report
                        .skipped_unparseable_key
                        .push(format!("{}: {button}", entry.operator));
                    continue;
                };
                let mod_keys = mod_keys_from_bools(*ctrl, *shift, *alt);
                ResolvedBinding::Press {
                    binding: mb.with_mod_keys(mod_keys),
                    phase: entry.phase,
                }
            }
            PresetInput::Scroll {
                up,
                ctrl,
                shift,
                alt,
            } => {
                let mod_keys = mod_keys_from_bools(*ctrl, *shift, *alt);
                ResolvedBinding::Scroll {
                    // Scroll phase is irrelevant; ScrollTick replaces it.
                    binding: Binding::MouseWheel { mod_keys },
                    positive: *up,
                }
            }
        };

        // Resolve operator id / builtin name to its action entities.
        // Operators context: look up via OperatorAction tag.
        // Modal / Navigation contexts: look up via BuiltinActions registry.
        let action_entities: Vec<Entity> = match entry.context {
            PresetContext::Operators => match by_operator.get(entry.operator.as_str()) {
                Some(v) => v.clone(),
                None => {
                    report.skipped_unknown_operator.push(entry.operator.clone());
                    continue;
                }
            },
            PresetContext::Modal | PresetContext::Navigation => {
                match builtin_snapshot.get(entry.operator.as_str()) {
                    Some(v) => v.clone(),
                    None => {
                        report.skipped_unknown_operator.push(entry.operator.clone());
                        continue;
                    }
                }
            }
        };

        // Spawn one binding entity per action entity, with the correct condition.
        let spawned = spawn_resolved(world, &resolved, &action_entities);
        report.spawned_bindings += spawned;
        report.applied_entries += 1;
    }

    // Emit one aggregated warning per non-empty skip vec.
    if !report.skipped_unknown_operator.is_empty() {
        warn!(
            "preset '{}' skipped {} unknown operators: {:?}",
            preset.name,
            report.skipped_unknown_operator.len(),
            report.skipped_unknown_operator,
        );
    }
    if !report.skipped_unparseable_key.is_empty() {
        warn!(
            "preset '{}' skipped {} unparseable keys: {:?}",
            preset.name,
            report.skipped_unparseable_key.len(),
            report.skipped_unparseable_key,
        );
    }
    if !report.skipped_unsupported.is_empty() {
        warn!(
            "preset '{}' skipped {} unsupported bindings: {:?}",
            preset.name,
            report.skipped_unsupported.len(),
            report.skipped_unsupported,
        );
    }

    report
}

/// Build a `ModKeys` bitmask from the three preset boolean fields.
fn mod_keys_from_bools(ctrl: bool, shift: bool, alt: bool) -> ModKeys {
    let mut mk = ModKeys::empty();
    if ctrl {
        mk |= ModKeys::CONTROL;
    }
    if shift {
        mk |= ModKeys::SHIFT;
    }
    if alt {
        mk |= ModKeys::ALT;
    }
    mk
}

/// Spawn one binding entity per action entity and return the spawn count.
fn spawn_resolved(
    world: &mut World,
    resolved: &ResolvedBinding,
    action_entities: &[Entity],
) -> usize {
    let mut count = 0;
    for &action_entity in action_entities {
        match resolved {
            ResolvedBinding::Press { binding, phase } => match phase {
                PresetPhase::Press => {
                    world.spawn((
                        *binding,
                        Press::default(),
                        BindingOf(action_entity),
                        PresetSpawnedBinding,
                        ChildOf(action_entity),
                    ));
                }
                PresetPhase::Release => {
                    world.spawn((
                        *binding,
                        Release::default(),
                        BindingOf(action_entity),
                        PresetSpawnedBinding,
                        ChildOf(action_entity),
                    ));
                }
                PresetPhase::DoubleClick => {
                    world.spawn((
                        *binding,
                        DoubleClick::default(),
                        BindingOf(action_entity),
                        PresetSpawnedBinding,
                        ChildOf(action_entity),
                    ));
                }
                PresetPhase::Tap => {
                    world.spawn((
                        *binding,
                        Tap::new(0.2),
                        BindingOf(action_entity),
                        PresetSpawnedBinding,
                        ChildOf(action_entity),
                    ));
                }
            },
            ResolvedBinding::Scroll { binding, positive } => {
                world.spawn((
                    *binding,
                    ScrollTick::new(*positive),
                    BindingOf(action_entity),
                    PresetSpawnedBinding,
                    ChildOf(action_entity),
                ));
            }
        }
        count += 1;
    }
    count
}

#[cfg(test)]
mod tests {
    use bevy_enhanced_input::prelude::{Binding, TriggerState};

    use super::*;
    use crate::keymap::{KeymapPreset, PresetBinding, PresetContext, PresetInput, PresetPhase};
    use crate::lifecycle::OperatorAction;

    // Helpers shared by the applier tests.

    fn preset_one(operator: &str, key: &str) -> KeymapPreset {
        KeymapPreset {
            name: "test".into(),
            bindings: vec![PresetBinding {
                operator: operator.to_string(),
                input: PresetInput::key(key),
                phase: PresetPhase::Press,
                context: PresetContext::Operators,
            }],
        }
    }

    fn spawn_action(world: &mut World, operator_id: &'static str) -> Entity {
        world
            .spawn((OperatorAction(operator_id), TriggerState::default()))
            .id()
    }

    #[test]
    fn applier_spawns_and_replaces_bindings() {
        let mut world = World::new();

        // Spawn an action entity tagged with the operator id.
        let _action = spawn_action(&mut world, "tool.select");

        // Apply a preset with one binding; expect exactly 1 spawn.
        let preset_a = preset_one("tool.select", "KeyQ");
        let report_a = apply_keymap_preset(&mut world, &preset_a);
        assert_eq!(
            report_a.spawned_bindings, 1,
            "first application should spawn 1 binding"
        );
        assert_eq!(
            world
                .query_filtered::<Entity, With<PresetSpawnedBinding>>()
                .iter(&world)
                .count(),
            1
        );

        // Re-apply with a different key; old binding is gone, still exactly 1.
        let preset_b = preset_one("tool.select", "KeyW");
        let report_b = apply_keymap_preset(&mut world, &preset_b);
        assert_eq!(
            report_b.spawned_bindings, 1,
            "re-application should spawn 1 binding"
        );
        assert_eq!(
            world
                .query_filtered::<Entity, With<PresetSpawnedBinding>>()
                .iter(&world)
                .count(),
            1,
            "re-application must not accumulate; old binding must be removed"
        );

        // Apply with an unknown operator id; zero spawns and one skip recorded.
        let preset_unknown = preset_one("unknown.op", "KeyQ");
        let report_unknown = apply_keymap_preset(&mut world, &preset_unknown);
        assert_eq!(
            report_unknown.spawned_bindings, 0,
            "unknown operator should yield 0 spawns"
        );
        assert_eq!(report_unknown.skipped_unknown_operator.len(), 1);
    }

    #[test]
    fn applier_binds_every_action_entity_of_an_operator() {
        let mut world = World::new();

        // Two action entities share the same operator id.
        let _a1 = spawn_action(&mut world, "tool.select");
        let _a2 = spawn_action(&mut world, "tool.select");

        let preset = preset_one("tool.select", "KeyQ");
        let report = apply_keymap_preset(&mut world, &preset);

        // One binding should be spawned per action entity.
        assert_eq!(
            report.spawned_bindings, 2,
            "each action entity must receive its own binding"
        );
        assert_eq!(
            world
                .query_filtered::<Entity, With<PresetSpawnedBinding>>()
                .iter(&world)
                .count(),
            2
        );
    }

    #[test]
    fn applier_never_touches_foreign_bindings() {
        let mut world = World::new();

        let action = spawn_action(&mut world, "tool.select");

        // Spawn a binding entity WITHOUT PresetSpawnedBinding to simulate
        // a raw/manual binding (e.g. from a test or a deferred raw site).
        let foreign = world
            .spawn(Binding::Keyboard {
                key: KeyCode::KeyF,
                mod_keys: ModKeys::empty(),
            })
            .id();

        // Apply and then re-apply so the despawn pass runs.
        let preset = preset_one("tool.select", "KeyQ");
        apply_keymap_preset(&mut world, &preset);
        apply_keymap_preset(&mut world, &preset);

        // The foreign binding entity must still exist.
        assert!(
            world.get_entity(foreign).is_ok(),
            "foreign binding entity must not be despawned by the applier"
        );

        // Only the one preset-owned binding should remain, not the foreign one.
        assert_eq!(
            world
                .query_filtered::<Entity, With<PresetSpawnedBinding>>()
                .iter(&world)
                .count(),
            1
        );

        // Suppress unused variable warning.
        let _ = action;
    }

    #[test]
    fn applier_applies_scroll_input_with_scroll_tick() {
        let mut world = World::new();
        let _action = spawn_action(&mut world, "view.zoom");

        let preset = KeymapPreset {
            name: "test".into(),
            bindings: vec![PresetBinding {
                operator: "view.zoom".into(),
                input: PresetInput::scroll(true),
                phase: PresetPhase::Press,
                context: PresetContext::Operators,
            }],
        };
        let report = apply_keymap_preset(&mut world, &preset);
        assert_eq!(report.spawned_bindings, 1, "scroll must be applied");
        assert_eq!(report.applied_entries, 1);
        assert!(report.skipped_unsupported.is_empty());
        assert!(report.skipped_unknown_operator.is_empty());
        assert!(report.skipped_unparseable_key.is_empty());

        // The spawned binding must be a MouseWheel variant.
        let binding = world
            .query_filtered::<&Binding, With<PresetSpawnedBinding>>()
            .single(&world)
            .expect("exactly one preset binding must exist");
        assert!(
            matches!(binding, Binding::MouseWheel { .. }),
            "scroll entry must produce a MouseWheel binding, got {binding:?}"
        );
    }

    #[test]
    fn applier_applies_mouse_button_with_press_condition() {
        let mut world = World::new();
        let _action = spawn_action(&mut world, "view.orbit");

        let preset = KeymapPreset {
            name: "test".into(),
            bindings: vec![PresetBinding {
                operator: "view.orbit".into(),
                input: PresetInput::mouse("Middle"),
                phase: PresetPhase::Press,
                context: PresetContext::Operators,
            }],
        };
        let report = apply_keymap_preset(&mut world, &preset);
        assert_eq!(report.spawned_bindings, 1, "mouse button must be applied");
        assert_eq!(report.applied_entries, 1);
        assert!(report.skipped_unsupported.is_empty());
        assert!(report.skipped_unparseable_key.is_empty());

        let binding = world
            .query_filtered::<&Binding, With<PresetSpawnedBinding>>()
            .single(&world)
            .expect("exactly one preset binding must exist");
        assert!(
            matches!(
                binding,
                Binding::MouseButton {
                    button: MouseButton::Middle,
                    ..
                }
            ),
            "mouse button entry must produce a MouseButton binding, got {binding:?}"
        );
    }

    #[test]
    fn applier_applies_double_click_phase() {
        let mut world = World::new();
        let _action = spawn_action(&mut world, "select.add");

        let preset = KeymapPreset {
            name: "test".into(),
            bindings: vec![PresetBinding {
                operator: "select.add".into(),
                input: PresetInput::mouse("Left"),
                phase: PresetPhase::DoubleClick,
                context: PresetContext::Operators,
            }],
        };
        let report = apply_keymap_preset(&mut world, &preset);
        assert_eq!(
            report.spawned_bindings, 1,
            "double-click phase must be applied"
        );
        assert_eq!(report.applied_entries, 1);
        assert!(report.skipped_unsupported.is_empty());
    }

    #[test]
    fn applier_rejects_unknown_mouse_button_name() {
        let mut world = World::new();
        let _action = spawn_action(&mut world, "some.op");

        let preset = KeymapPreset {
            name: "test".into(),
            bindings: vec![PresetBinding {
                operator: "some.op".into(),
                input: PresetInput::MouseButton {
                    button: "MiddleThumbnail".to_string(),
                    ctrl: false,
                    shift: false,
                    alt: false,
                },
                phase: PresetPhase::Press,
                context: PresetContext::Operators,
            }],
        };
        let report = apply_keymap_preset(&mut world, &preset);
        assert_eq!(report.spawned_bindings, 0, "unknown button must not spawn");
        assert_eq!(
            report.skipped_unparseable_key.len(),
            1,
            "unknown button name must land in skipped_unparseable_key"
        );
    }

    #[test]
    fn applier_applies_modal_context_when_builtin_registered() {
        let mut world = World::new();

        // Register the builtin action entity so the applier can find it.
        let action = world.spawn(TriggerState::default()).id();
        world
            .get_resource_or_init::<BuiltinActions>()
            .register("modal.confirm", action);

        let preset = KeymapPreset {
            name: "test".into(),
            bindings: vec![PresetBinding {
                operator: "modal.confirm".into(),
                input: PresetInput::key("Enter"),
                phase: PresetPhase::Press,
                context: PresetContext::Modal,
            }],
        };
        let report = apply_keymap_preset(&mut world, &preset);
        assert_eq!(
            report.spawned_bindings, 1,
            "modal-context entry must be applied when builtin is registered"
        );
        assert_eq!(report.applied_entries, 1);
        assert!(report.skipped_unsupported.is_empty());
        assert!(report.skipped_unknown_operator.is_empty());
        assert!(report.skipped_unparseable_key.is_empty());
    }

    #[test]
    fn applier_skips_modal_context_when_builtin_absent() {
        // With no BuiltinActions resource, modal entries land in
        // skipped_unknown_operator (not skipped_unsupported).
        let mut world = World::new();

        let preset = KeymapPreset {
            name: "test".into(),
            bindings: vec![PresetBinding {
                operator: "modal.confirm".into(),
                input: PresetInput::key("Enter"),
                phase: PresetPhase::Press,
                context: PresetContext::Modal,
            }],
        };
        let report = apply_keymap_preset(&mut world, &preset);
        assert_eq!(
            report.spawned_bindings, 0,
            "modal entry must not spawn when unregistered"
        );
        assert_eq!(report.applied_entries, 0);
        assert!(
            report.skipped_unsupported.is_empty(),
            "skipped_unsupported must be empty; unknown builtins go to skipped_unknown_operator"
        );
        assert_eq!(
            report.skipped_unknown_operator.len(),
            1,
            "unregistered builtin must land in skipped_unknown_operator"
        );
    }
}
