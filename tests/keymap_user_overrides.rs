//! The user's own bindings layered over the shipped defaults: what a
//! rebind replaces, what it leaves alone, and what a file naming an
//! operator this build does not have costs the rest of the file.

use bevy::prelude::*;
use jackdaw_api_internal::keymap::{
    DefaultKeymap, KeymapPreset, PresetBinding, PresetContext, PresetInput, PresetPhase,
    UserKeymap, apply_keymap_preset, resolve_keymap,
};
use jackdaw_api_internal::lifecycle::OperatorEntity;

mod util;

fn row(operator: &str, key: &str) -> PresetBinding {
    PresetBinding {
        operator: operator.to_string(),
        input: PresetInput::key(key),
        phase: PresetPhase::Press,
        context: PresetContext::Operators,
    }
}

fn classic(app: &mut App) -> KeymapPreset {
    app.world_mut()
        .get_resource_or_init::<DefaultKeymap>()
        .to_classic_preset()
}

/// An operator the shipped keymap binds, so the override cases have a
/// real target rather than a synthetic one.
const REBOUND: &str = "history.undo";

#[test]
fn an_override_replaces_every_default_row_of_that_operator() {
    let mut app = util::headless_app();
    app.finish();
    app.update();
    let defaults = classic(&mut app);
    assert!(
        defaults.bindings.iter().any(|b| b.operator == REBOUND),
        "{REBOUND} must ship with a binding for this test to mean anything"
    );

    let user = UserKeymap {
        bindings: vec![row(REBOUND, "F9")],
    };
    let resolved = resolve_keymap(&defaults, &user);

    let rebound: Vec<&PresetBinding> = resolved
        .bindings
        .iter()
        .filter(|b| b.operator == REBOUND)
        .collect();
    assert_eq!(
        rebound,
        vec![&row(REBOUND, "F9")],
        "the operator's rows must be exactly the user's, with the shipped chord gone"
    );
}

#[test]
fn an_override_leaves_every_other_operator_alone() {
    let mut app = util::headless_app();
    app.finish();
    app.update();
    let defaults = classic(&mut app);
    let user = UserKeymap {
        bindings: vec![row(REBOUND, "F9")],
    };
    let resolved = resolve_keymap(&defaults, &user);

    let untouched_before: Vec<&PresetBinding> = defaults
        .bindings
        .iter()
        .filter(|b| b.operator != REBOUND)
        .collect();
    let untouched_after: Vec<&PresetBinding> = resolved
        .bindings
        .iter()
        .filter(|b| b.operator != REBOUND)
        .collect();
    assert_eq!(
        untouched_before, untouched_after,
        "rebinding one operator must not move or drop any other row"
    );
}

#[test]
fn an_empty_user_keymap_resolves_to_the_shipped_keymap() {
    let mut app = util::headless_app();
    app.finish();
    app.update();
    let defaults = classic(&mut app);
    assert_eq!(resolve_keymap(&defaults, &UserKeymap::default()), defaults);
}

#[test]
fn a_row_naming_an_absent_operator_is_skipped_without_dropping_the_rest() {
    let mut app = util::headless_app();
    app.finish();
    app.update();
    let defaults = classic(&mut app);
    let user = UserKeymap {
        bindings: vec![row("not.an.operator", "F10"), row(REBOUND, "F9")],
    };
    let resolved = resolve_keymap(&defaults, &user);
    let report = apply_keymap_preset(app.world_mut(), &resolved);

    assert_eq!(
        report.skipped_unknown_operator,
        vec!["not.an.operator".to_string()],
        "only the unresolvable row is skipped"
    );
    assert_eq!(
        report.applied_entries,
        resolved.bindings.len() - 1,
        "every other row still applies"
    );
    assert!(
        report.spawned_bindings > 0,
        "an unknown row must not cost the keymap its bindings"
    );
}

#[test]
fn reapplying_a_resolved_keymap_is_idempotent() {
    use jackdaw_api_internal::keymap::PresetSpawnedBinding;

    let mut app = util::headless_app();
    app.finish();
    app.update();
    let defaults = classic(&mut app);
    let user = UserKeymap {
        bindings: vec![row(REBOUND, "F9")],
    };
    let resolved = resolve_keymap(&defaults, &user);

    let live_bindings = |app: &mut App| {
        let world = app.world_mut();
        world
            .query_filtered::<Entity, With<PresetSpawnedBinding>>()
            .iter(world)
            .count()
    };

    let first = apply_keymap_preset(app.world_mut(), &resolved);
    let after_first = live_bindings(&mut app);
    let second = apply_keymap_preset(app.world_mut(), &resolved);
    let after_second = live_bindings(&mut app);

    assert_eq!(first.spawned_bindings, second.spawned_bindings);
    assert_eq!(first.applied_entries, second.applied_entries);
    assert_eq!(
        after_first, after_second,
        "re-applying must replace the keymap's bindings, not add a second copy"
    );
}

/// A rebound operator ends up bound to the new chord and to nothing
/// else: the shipped chord's binding entity is gone, not shadowed.
#[test]
fn applying_a_rebind_leaves_exactly_one_binding_on_the_new_chord() {
    use bevy_enhanced_input::prelude::{Binding, ModKeys};
    use jackdaw_api_internal::keymap::PresetSpawnedBinding;
    use jackdaw_api_internal::lifecycle::OperatorAction;

    let mut app = util::headless_app();
    app.finish();
    app.update();
    let defaults = classic(&mut app);
    let user = UserKeymap {
        bindings: vec![row(REBOUND, "F9")],
    };
    let resolved = resolve_keymap(&defaults, &user);
    apply_keymap_preset(app.world_mut(), &resolved);

    let world = app.world_mut();
    let actions: Vec<Entity> = world
        .query::<(Entity, &OperatorAction)>()
        .iter(world)
        .filter(|(_, tag)| tag.0 == REBOUND)
        .map(|(entity, _)| entity)
        .collect();
    assert!(!actions.is_empty(), "{REBOUND} must have an action entity");

    let bindings: Vec<Binding> = world
        .query_filtered::<(&Binding, &ChildOf), With<PresetSpawnedBinding>>()
        .iter(world)
        .filter(|(_, child_of)| actions.contains(&child_of.parent()))
        .map(|(binding, _)| *binding)
        .collect();
    assert_eq!(
        bindings.len(),
        actions.len(),
        "one binding per action entity, and no leftover from the shipped chord"
    );
    for binding in bindings {
        assert!(
            matches!(
                binding,
                Binding::Keyboard {
                    key: KeyCode::F9,
                    mod_keys
                } if mod_keys == ModKeys::empty()
            ),
            "the surviving binding must be the rebound chord, got {binding:?}"
        );
    }
}

/// The file the editor writes and reads back. Pinned so a change to the
/// serde shape is a decision rather than a silently unreadable file in
/// everyone's config directory.
#[test]
fn user_keymap_json_is_the_documented_shape() {
    let keymap = UserKeymap {
        bindings: vec![PresetBinding {
            operator: "history.undo".into(),
            input: PresetInput::key("F9").ctrl(),
            phase: PresetPhase::Press,
            context: PresetContext::Operators,
        }],
    };
    let json = serde_json::to_string(&keymap).expect("serialize");
    assert_eq!(
        json,
        r#"{"bindings":[{"operator":"history.undo","input":{"type":"Key","key":"F9","ctrl":true}}]}"#
    );
    let back: UserKeymap = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, keymap);
}

#[test]
fn an_absent_bindings_field_parses_as_an_empty_keymap() {
    let keymap: UserKeymap = serde_json::from_str("{}").expect("an empty object must parse");
    assert_eq!(keymap, UserKeymap::default());
}

/// Every operator either has a chord in the resolved keymap or is on the
/// list of operators that deliberately have none. A new operator with no
/// binding and no entry here fails, which is the moment to decide which
/// of the two it is.
#[test]
fn every_operator_is_bound_or_listed_as_unbound() {
    let mut app = util::headless_app();
    app.finish();
    app.update();
    let defaults = classic(&mut app);
    let resolved = resolve_keymap(&defaults, &UserKeymap::default());

    let bound: std::collections::HashSet<&str> = resolved
        .bindings
        .iter()
        .map(|b| b.operator.as_str())
        .collect();

    let world = app.world_mut();
    let mut unbound: Vec<&'static str> = world
        .query::<&OperatorEntity>()
        .iter(world)
        .map(OperatorEntity::id)
        .filter(|id| !bound.contains(id))
        .collect();
    unbound.sort_unstable();
    unbound.dedup();

    // Operators reached from a menu, a button, or the command palette,
    // plus the ones whose chord lives at a raw binding site the preset
    // format cannot yet express (hold-repeat, modifier-only gestures).
    let known_unbound: std::collections::HashSet<&str> =
        jackdaw::keybind_settings::UNBOUND_OPERATORS
            .iter()
            .copied()
            .collect();
    let surprises: Vec<&str> = unbound
        .iter()
        .copied()
        .filter(|id| !known_unbound.contains(id))
        .collect();
    assert!(
        surprises.is_empty(),
        "these operators have no chord and are not listed as deliberately unbound: {surprises:?}"
    );
}

/// The dialog lists every registered operator exactly once, so nothing
/// the editor can do is unreachable from the keybind interface.
#[test]
fn the_dialog_seeds_one_row_per_operator() {
    let mut app = util::headless_app();
    app.finish();
    app.update();

    let operator_count = {
        let world = app.world_mut();
        world.query::<&OperatorEntity>().iter(world).count()
    };
    let pending = jackdaw::keybind_settings::pending_from_world(app.world_mut());
    assert_eq!(
        pending.rows.len(),
        operator_count,
        "one row per operator, no duplicates and no omissions"
    );
    assert!(
        operator_count > 100,
        "the editor registers far more than a handful of operators; got {operator_count}"
    );
}

/// Operators whose chord is attached at a raw binding site rather than
/// through the keymap are listed and marked as not editable, so the
/// chord is visible even though the dialog cannot change it.
#[test]
fn a_raw_bound_operator_is_listed_as_fixed() {
    let mut app = util::headless_app();
    app.finish();
    app.update();
    let pending = jackdaw::keybind_settings::pending_from_world(app.world_mut());

    let palette = pending
        .rows
        .iter()
        .find(|row| row.operator == "command_palette.toggle")
        .expect("the command palette is registered");
    assert!(
        !palette.is_editable(),
        "an operator bound outside the keymap is a fixed row"
    );
    assert!(
        !palette.fixed.is_empty(),
        "a fixed row must show the chord it is fixed on"
    );
}

/// Rebind, save, and come back: the file the dialog writes resolves to
/// the keymap the dialog was showing.
#[test]
fn saving_a_rebind_reproduces_the_keymap_after_a_reload() {
    let mut app = util::headless_app();
    app.finish();
    app.update();

    let mut pending = jackdaw::keybind_settings::pending_from_world(app.world_mut());
    pending.rebind(REBOUND, PresetInput::key("F9"));
    let expected = pending.chords_of(REBOUND);

    // What Save writes, read back the way a later launch reads it.
    let user = pending.to_user_keymap();
    let json = serde_json::to_string(&user).expect("serialize");
    let reloaded: UserKeymap = serde_json::from_str(&json).expect("deserialize");

    let defaults = classic(&mut app);
    let resolved = resolve_keymap(&defaults, &reloaded);
    app.world_mut().insert_resource(reloaded);

    let reopened = jackdaw::keybind_settings::pending_from_world(app.world_mut());
    assert_eq!(
        reopened.chords_of(REBOUND),
        expected,
        "the reopened dialog must show the chord that was saved"
    );
    assert!(
        resolved
            .bindings
            .iter()
            .any(|b| b.operator == REBOUND && b.input == PresetInput::key("F9")),
        "the applied keymap must carry the saved chord"
    );
    assert_eq!(
        reopened.to_user_keymap(),
        user,
        "reopening and saving again must not change the file"
    );
}

/// Resetting the row that was rebound takes it back off the file
/// entirely, rather than pinning it to whatever the defaults are today.
#[test]
fn resetting_a_rebound_row_leaves_nothing_in_the_file() {
    let mut app = util::headless_app();
    app.finish();
    app.update();

    let mut pending = jackdaw::keybind_settings::pending_from_world(app.world_mut());
    pending.rebind(REBOUND, PresetInput::key("F9"));
    assert!(!pending.to_user_keymap().bindings.is_empty());

    pending.reset(REBOUND);
    assert_eq!(
        pending.to_user_keymap(),
        UserKeymap::default(),
        "a reset row must leave the file with nothing to say about it"
    );

    pending.rebind(REBOUND, PresetInput::key("F9"));
    pending.reset_all();
    assert_eq!(pending.to_user_keymap(), UserKeymap::default());
}

/// An operator with no input action behind it has nothing for a chord
/// to attach to, so the dialog says so instead of offering a rebind the
/// applier would then refuse.
#[test]
fn a_menu_only_operator_is_listed_but_not_offered_a_chord() {
    let mut app = util::headless_app();
    app.finish();
    app.update();
    let pending = jackdaw::keybind_settings::pending_from_world(app.world_mut());

    let menu_only = pending
        .rows
        .iter()
        .find(|row| row.operator == "app.open_keybinds")
        .expect("opening the keybind dialog is an operator");
    assert!(
        !menu_only.bindable,
        "an operator reached only from a menu has no action to bind"
    );
    assert!(!menu_only.is_editable());

    let editable = pending
        .rows
        .iter()
        .find(|row| row.operator == REBOUND)
        .expect("undo is an operator");
    assert!(editable.bindable && editable.is_editable());

    // Every row the dialog offers a rebind on must be one the applier can
    // actually bind, or the dialog would be writing a file that does
    // nothing.
    let world = app.world_mut();
    let with_action: std::collections::HashSet<String> = world
        .query::<&jackdaw_api_internal::lifecycle::OperatorAction>()
        .iter(world)
        .map(|action| action.0.to_string())
        .collect();
    for row in pending.rows.iter().filter(|row| row.is_editable()) {
        assert!(
            with_action.contains(&row.operator),
            "{} is offered a rebind but has no action to bind",
            row.operator
        );
    }
}

/// The rows the dialog cannot edit are listed together under one
/// heading each. Grouping them by the operator's own name instead would
/// scatter them through the list and repeat their heading at each one.
#[test]
fn the_fixed_rows_are_one_run_of_the_list() {
    let mut app = util::headless_app();
    app.finish();
    app.update();
    let pending = jackdaw::keybind_settings::pending_from_world(app.world_mut());

    let fixed_positions: Vec<usize> = pending
        .rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.category == "Fixed")
        .map(|(index, _)| index)
        .collect();
    assert!(
        fixed_positions.len() > 1,
        "the editor has more than one raw-bound operator"
    );
    assert_eq!(
        fixed_positions.last().unwrap() - fixed_positions.first().unwrap() + 1,
        fixed_positions.len(),
        "the fixed rows must be contiguous, not scattered through the list"
    );
    for row in &pending.rows {
        assert_eq!(
            row.is_editable(),
            row.category != "Fixed" && row.category != "Menu only",
            "a row's heading and whether it can be edited must agree"
        );
    }
}
