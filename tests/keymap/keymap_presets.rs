//! Conformance tests for the data-driven keymap: every recorded
//! default resolves to a registered operator, the classic preset
//! round-trips, and applying it binds every entry.

use bevy::prelude::*;
use jackdaw_api_internal::keymap::{
    ActiveKeymapPreset, DefaultKeymap, KeymapPreset, PresetBinding, PresetContext, PresetInput,
    PresetPhase, apply_keymap_preset, find_conflicts,
};

#[test]
fn classic_preset_entries_all_resolve_to_registered_operators() {
    let mut app = crate::headless_app();
    app.finish();
    app.update();

    let defaults = app
        .world_mut()
        .get_resource_or_init::<DefaultKeymap>()
        .to_classic_preset();
    assert!(
        defaults.bindings.len() >= 67,
        "expected the migrated defaults (~79+); got {}",
        defaults.bindings.len()
    );

    let report = apply_keymap_preset(app.world_mut(), &defaults);
    assert_eq!(
        report.skipped_unknown_operator,
        Vec::<String>::new(),
        "classic entries must all name registered operators"
    );
    assert_eq!(report.skipped_unparseable_key, Vec::<String>::new());
    assert_eq!(
        report.skipped_unsupported,
        Vec::<String>::new(),
        "classic preset must contain no unsupported entries"
    );
    assert_eq!(report.applied_entries, defaults.bindings.len());
}

#[test]
fn classic_preset_round_trips_through_json() {
    let mut app = crate::headless_app();
    app.finish();
    app.update();
    let defaults = app
        .world_mut()
        .get_resource_or_init::<DefaultKeymap>()
        .to_classic_preset();
    let json = serde_json::to_string_pretty(&defaults).expect("serialize");
    let back: KeymapPreset = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(defaults, back);
}

#[test]
fn reapplying_preset_is_idempotent() {
    let mut app = crate::headless_app();
    app.finish();
    app.update();
    let defaults = app
        .world_mut()
        .get_resource_or_init::<DefaultKeymap>()
        .to_classic_preset();
    let first = apply_keymap_preset(app.world_mut(), &defaults);
    let second = apply_keymap_preset(app.world_mut(), &defaults);
    assert_eq!(first.spawned_bindings, second.spawned_bindings);
}

#[test]
fn active_preset_default_is_classic() {
    assert_eq!(ActiveKeymapPreset::default().name, "classic");
}

/// The classic preset must contain an entry for every preset-recorded
/// builtin action name with the correct context tag, and applying the
/// preset must report them as applied (not skipped).
#[test]
fn classic_preset_contains_builtin_entries_and_applies_them() {
    let mut app = crate::headless_app();
    app.finish();
    app.update();

    let defaults = app
        .world_mut()
        .get_resource_or_init::<DefaultKeymap>()
        .to_classic_preset();

    // The 6 builtin names with DefaultKeymap entries. nav.fly is bound code-level
    // with a Down condition; modal.confirm / step_up / step_down are in
    // BuiltinActions but get keymap entries once a modal consumer exists.
    let builtin_names = [
        ("modal.cancel", PresetContext::Modal),
        ("modal.axis_x", PresetContext::Modal),
        ("modal.axis_y", PresetContext::Modal),
        ("modal.axis_z", PresetContext::Modal),
        ("nav.brush_resize_up", PresetContext::Navigation),
        ("nav.brush_resize_down", PresetContext::Navigation),
    ];
    for (name, ctx) in &builtin_names {
        let found = defaults
            .bindings
            .iter()
            .any(|b| b.operator == *name && b.context == *ctx);
        assert!(
            found,
            "classic preset missing builtin entry '{}' with context {:?}",
            name, ctx
        );
    }

    // Applying the classic preset must not put any builtin names into skip lists.
    let report = apply_keymap_preset(app.world_mut(), &defaults);
    assert!(
        report.skipped_unsupported.is_empty(),
        "classic preset must have no unsupported entries; got: {:?}",
        report.skipped_unsupported,
    );
    for (name, _) in &builtin_names {
        assert!(
            !report.skipped_unknown_operator.contains(&name.to_string()),
            "builtin '{}' must be applied, not skipped as unknown",
            name,
        );
    }
}

/// Every operator bound to `input` in the classic preset, sorted.
fn operators_on(defaults: &KeymapPreset, input: &PresetInput) -> Vec<String> {
    let mut found: Vec<String> = defaults
        .bindings
        .iter()
        .filter(|binding| &binding.input == input)
        .map(|binding| binding.operator.clone())
        .collect();
    found.sort();
    found
}

fn classic(app: &mut App) -> KeymapPreset {
    app.finish();
    app.update();
    app.world_mut()
        .get_resource_or_init::<DefaultKeymap>()
        .to_classic_preset()
}

/// Ctrl+C and Ctrl+V are claimed by both the timeline's keyframes and the entity
/// clipboard, and their availability checks are disjoint on the timeline being
/// the focused window. The whole-component clipboard sits on Ctrl+Shift.
#[test]
fn the_clipboard_chord_is_shared_by_the_entity_and_keyframe_operators() {
    let mut app = crate::headless_app();
    let defaults = classic(&mut app);

    assert_eq!(
        operators_on(&defaults, &PresetInput::key("KeyC").ctrl()),
        vec!["clip.copy_keyframes".to_string(), "entity.copy".to_string()],
    );
    assert_eq!(
        operators_on(&defaults, &PresetInput::key("KeyV").ctrl()),
        vec![
            "clip.paste_keyframes".to_string(),
            "entity.paste".to_string()
        ],
    );
    assert_eq!(
        operators_on(&defaults, &PresetInput::key("KeyC").ctrl().shift()),
        vec!["entity.copy_components".to_string()],
    );
    assert_eq!(
        operators_on(&defaults, &PresetInput::key("KeyV").ctrl().shift()),
        vec!["entity.paste_components".to_string()],
    );
}

/// Ctrl+A is a preset entry, so the keymap can report it, rebind it and
/// save it like every other binding.
#[test]
fn ctrl_a_is_a_preset_entry_for_the_add_entity_picker() {
    let mut app = crate::headless_app();
    let defaults = classic(&mut app);
    assert!(
        operators_on(&defaults, &PresetInput::key("KeyA").ctrl())
            .contains(&"entity.add_picker".to_string()),
    );
}

/// The two keys every other tool gives a viewport: Home frames what is in
/// it, Escape drops the selection. Both are preset entries.
#[test]
fn home_and_escape_are_preset_entries() {
    let mut app = crate::headless_app();
    let defaults = classic(&mut app);

    assert!(
        operators_on(&defaults, &PresetInput::key("Home"))
            .contains(&"viewport2d.frame".to_string()),
        "Home frames the canvas",
    );
    assert!(
        operators_on(&defaults, &PresetInput::key("Escape"))
            .contains(&"selection.clear".to_string()),
        "Escape drops the selection",
    );
}

/// Two actions on one chord co-fire and let `is_available` arbitrate, so a shared
/// chord is reported rather than resolved.
#[test]
fn a_chord_claimed_twice_is_reported() {
    let preset = KeymapPreset {
        name: "test".into(),
        bindings: vec![
            PresetBinding {
                operator: "a.first".into(),
                input: PresetInput::key("KeyC").ctrl(),
                phase: PresetPhase::Press,
                context: PresetContext::Operators,
            },
            PresetBinding {
                operator: "a.second".into(),
                input: PresetInput::key("KeyC").ctrl(),
                phase: PresetPhase::Press,
                context: PresetContext::Operators,
            },
            // Same key, different phase: a different chord.
            PresetBinding {
                operator: "a.third".into(),
                input: PresetInput::key("KeyC").ctrl(),
                phase: PresetPhase::Release,
                context: PresetContext::Operators,
            },
            PresetBinding {
                operator: "a.fourth".into(),
                input: PresetInput::key("KeyD").ctrl(),
                phase: PresetPhase::Press,
                context: PresetContext::Operators,
            },
        ],
    };

    let conflicts = find_conflicts(&preset);
    assert_eq!(
        conflicts,
        vec!["Ctrl+KeyC (Press, Operators): a.first, a.second".to_string()],
        "only the chord two actions share is reported",
    );

    let mut world = World::new();
    assert_eq!(
        apply_keymap_preset(&mut world, &preset).conflicts,
        conflicts,
        "the applier reports what the detector found",
    );
}

/// The chords the authoring operators added claim what they were meant to: all
/// but the shared clipboard pair belong to a single operator, and none turns up
/// in the applier's conflict report.
#[test]
fn the_authoring_chords_claim_what_they_were_meant_to() {
    let mut app = crate::headless_app();
    let defaults = classic(&mut app);

    for (input, operator) in [
        (PresetInput::key("KeyX").ctrl(), "entity.cut"),
        (PresetInput::key("ArrowUp").ctrl(), "entity.move_up"),
        (PresetInput::key("ArrowDown").ctrl(), "entity.move_down"),
        (PresetInput::key("KeyG").ctrl(), "ui.group_into"),
        (PresetInput::key("KeyG").ctrl().shift(), "ui.ungroup"),
    ] {
        assert_eq!(
            operators_on(&defaults, &input),
            vec![operator.to_string()],
            "{input:?} should be {operator}'s alone",
        );
    }

    let conflicts = find_conflicts(&defaults);
    for chord in [
        "Ctrl+KeyX",
        "Ctrl+ArrowUp",
        "Ctrl+ArrowDown",
        "Ctrl+KeyG",
        "Ctrl+Shift+KeyG",
    ] {
        assert!(
            !conflicts.iter().any(|line| line.starts_with(chord)),
            "{chord} collided with something already bound: {conflicts:?}",
        );
    }
    assert!(
        conflicts
            .iter()
            .any(|line| line.starts_with("Ctrl+KeyC") && line.contains("entity.copy")),
        "the shared copy chord should be reported, advisory: {conflicts:?}",
    );
    assert!(
        conflicts
            .iter()
            .any(|line| line.starts_with("Ctrl+KeyV") && line.contains("entity.paste")),
        "the shared paste chord should be reported, advisory: {conflicts:?}",
    );
}
