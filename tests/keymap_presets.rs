//! Conformance tests for the data-driven keymap: every recorded
//! default resolves to a registered operator, the classic preset
//! round-trips, and applying it binds every entry.

use bevy::prelude::*;
use jackdaw_api_internal::keymap::{
    ActiveKeymapPreset, DefaultKeymap, KeymapPreset, PresetContext, apply_keymap_preset,
};

mod util;

#[test]
fn classic_preset_entries_all_resolve_to_registered_operators() {
    let mut app = util::headless_app();
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
    let mut app = util::headless_app();
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
    let mut app = util::headless_app();
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
    let mut app = util::headless_app();
    app.finish();
    app.update();

    let defaults = app
        .world_mut()
        .get_resource_or_init::<DefaultKeymap>()
        .to_classic_preset();

    // Verify the 6 builtin names that have DefaultKeymap entries are present.
    // nav.fly is bound code-level with a Down condition and has no DefaultKeymap
    // entry (not preset-bindable this pass).
    // nav.brush_resize_up / nav.brush_resize_down are registered in BuiltinActions
    // and DefaultKeymap with PresetContext::Navigation.
    // modal.confirm / modal.step_up / modal.step_down are registered in
    // BuiltinActions but have no DefaultKeymap entries yet; they are recorded
    // into the keymap once a modal consumer exists.
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
