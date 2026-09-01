//! The user's own bindings layered over the shipped defaults: what a
//! rebind replaces, what it leaves alone, and what a file naming an
//! operator this build does not have costs the rest of the file.

use bevy::prelude::*;
use jackdaw_api_internal::keymap::{
    DefaultKeymap, KeymapPreset, PresetBinding, PresetContext, PresetInput, PresetPhase,
    UserKeymap, apply_keymap_preset, resolve_keymap,
};
use jackdaw_api_internal::lifecycle::{OperatorAction, OperatorEntity};

use crate::{CONFIG_DIR, CONFIG_LOCK, empty_config_dir, headless_app};

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
    let mut app = headless_app();
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
    let mut app = headless_app();
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
    let mut app = headless_app();
    app.finish();
    app.update();
    let defaults = classic(&mut app);
    assert_eq!(resolve_keymap(&defaults, &UserKeymap::default()), defaults);
}

#[test]
fn a_row_naming_an_absent_operator_is_skipped_without_dropping_the_rest() {
    let mut app = headless_app();
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

    let mut app = headless_app();
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

    let mut app = headless_app();
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
    let mut app = headless_app();
    app.finish();
    app.update();
    let defaults = classic(&mut app);
    let resolved = resolve_keymap(&defaults, &UserKeymap::default());

    let bound: std::collections::HashSet<&str> = resolved
        .bindings
        .iter()
        .map(|b| b.operator.as_str())
        .collect();

    // An operator with no action entity has nothing for a binding to
    // point at, so it is exempt by construction and needs no entry: it
    // was registered to be reached from a menu, a button or the command
    // palette. Deriving that half rather than listing it keeps the list
    // to the operators that could hold a chord and deliberately do not.
    let world = app.world_mut();
    let with_action: std::collections::HashSet<String> = world
        .query::<&OperatorAction>()
        .iter(world)
        .map(|action| action.0.to_string())
        .collect();
    let world = app.world_mut();
    let mut unbound: Vec<&'static str> = world
        .query::<&OperatorEntity>()
        .iter(world)
        .map(OperatorEntity::id)
        .filter(|id| !bound.contains(id) && with_action.contains(*id))
        .collect();
    unbound.sort_unstable();
    unbound.dedup();

    // What is left: operators that do have an action, and whose chord
    // lives at a raw binding site the preset format cannot yet express
    // (hold-repeat, modifier-only gestures), or that are deliberately
    // reached only from a surface of their own.
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
        "these operators have an input action, no chord, and no entry saying that is deliberate: \
         {surprises:?}\n\
         Give each one a chord with `ctx.bind_operator`, or add its id to \
         `UNBOUND_OPERATORS` in src/keybind_settings.rs saying it is reached from \
         its own surface instead."
    );
}

/// An entry that has since been given a chord is a lie the list keeps
/// telling: it says the operator is deliberately unbound while the keymap
/// says otherwise, and the next reader believes the list.
#[test]
fn no_listed_unbound_operator_actually_holds_a_chord() {
    let mut app = headless_app();
    app.finish();
    app.update();
    let defaults = classic(&mut app);
    let resolved = resolve_keymap(&defaults, &UserKeymap::default());

    let bound: std::collections::HashSet<&str> = resolved
        .bindings
        .iter()
        .map(|b| b.operator.as_str())
        .collect();
    let stale: Vec<&str> = jackdaw::keybind_settings::UNBOUND_OPERATORS
        .iter()
        .copied()
        .filter(|id| bound.contains(id))
        .collect();
    assert!(
        stale.is_empty(),
        "these operators are listed as deliberately unbound and hold a chord anyway; \
         drop them from `UNBOUND_OPERATORS` in src/keybind_settings.rs: {stale:?}"
    );
}

/// The other half of the same rot: an entry for an operator that has no
/// input action at all is now derived, so the entry says nothing.
#[test]
fn no_listed_unbound_operator_is_already_exempt_without_an_action() {
    let mut app = headless_app();
    app.finish();
    app.update();
    let world = app.world_mut();
    let with_action: std::collections::HashSet<String> = world
        .query::<&OperatorAction>()
        .iter(world)
        .map(|action| action.0.to_string())
        .collect();
    let redundant: Vec<&str> = jackdaw::keybind_settings::UNBOUND_OPERATORS
        .iter()
        .copied()
        .filter(|id| !with_action.contains(*id))
        .collect();
    assert!(
        redundant.is_empty(),
        "these operators have no input action, so they are exempt without being listed; \
         drop them from `UNBOUND_OPERATORS` in src/keybind_settings.rs: {redundant:?}"
    );
}

/// The dialog lists every registered operator exactly once, so nothing
/// the editor can do is unreachable from the keybind interface.
#[test]
fn the_dialog_seeds_one_row_per_operator() {
    let mut app = headless_app();
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
    let mut app = headless_app();
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
    let mut app = headless_app();
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
    let mut app = headless_app();
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
    let mut app = headless_app();
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
    let mut app = headless_app();
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

/// The whole path a rebind takes: the dialog's Save writes a file, a later
/// session reads it back, and the operator answers on the new chord.
///
/// The pieces were covered one at a time; the disk in the middle was not, so
/// a keymap that serialized and resolved could still have been written where
/// nothing read it.
#[test]
fn a_saved_rebind_comes_back_off_disk_and_applies() {
    use jackdaw_api_internal::keymap::{load_user_keymap, save_user_keymap};

    let guard = CONFIG_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = empty_config_dir();
    let saved = UserKeymap {
        bindings: vec![row(REBOUND, "F9")],
    };
    save_user_keymap(&saved);

    let path = dir.join("keymap.json");
    assert!(path.is_file(), "Save wrote nothing to {}", path.display());

    // A later session: the file is all it has.
    let reloaded = load_user_keymap();
    assert_eq!(reloaded, saved, "what came back is what was written");

    let _ = std::fs::remove_file(&path);
    drop(guard);

    let mut app = headless_app();
    app.finish();
    app.update();
    let defaults = classic(&mut app);
    let resolved = resolve_keymap(&defaults, &reloaded);
    assert_eq!(
        resolved
            .bindings
            .iter()
            .filter(|b| b.operator == REBOUND)
            .collect::<Vec<_>>(),
        vec![&row(REBOUND, "F9")],
        "the operator answers on the chord the file names"
    );
}

/// An empty override directory is what the rest of the suite assumes, and
/// what a fresh install has.
#[test]
fn the_suite_reads_its_own_config_directory() {
    let _guard = CONFIG_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    empty_config_dir();
    assert_eq!(
        jackdaw_env::paths::keymap_path(),
        Some(CONFIG_DIR.join("keymap.json")),
        "the config seam points the editor at the suite's own directory"
    );
    assert_eq!(load_user_keymap_now(), UserKeymap::default());
}

fn load_user_keymap_now() -> UserKeymap {
    jackdaw_api_internal::keymap::load_user_keymap()
}

/// A chord two commands have shared since the editor shipped needs no
/// decision from the reader: their availability arbitrates. What the
/// dialog owes is which rows those are, which the row says itself.
#[test]
fn a_shipped_co_fire_marks_its_rows_and_stays_out_of_the_paragraph() {
    let mut app = headless_app();
    app.finish();
    app.update();
    let pending = jackdaw::keybind_settings::pending_from_world(app.world_mut());

    let shared = pending.conflicts_of("entity.copy");
    assert!(
        shared.iter().any(|line| line.contains("Copy Keyframes")),
        "the row names the other command in the words the rows are named in: {shared:?}",
    );
    assert!(
        shared
            .iter()
            .all(|line| line.contains("Ctrl") && line.contains("C -")),
        "and names the chord it is about: {shared:?}",
    );

    assert!(
        pending.user_conflicts().is_empty(),
        "nothing has been rebound, so nothing is the reader's to decide",
    );
    let text = jackdaw::keybind_settings::advisory_text(
        &pending,
        &[],
        &jackdaw_api_internal::keymap::KeymapLoadProblem::default(),
    );
    assert!(
        !text.contains("you have just bound") && !text.contains("clip.copy_keyframes"),
        "the shipped co-fires are counted, not listed one by one: {text}",
    );
    assert!(
        text.contains("arbitrated"),
        "and the count says what happens to them: {text}",
    );
}

/// A conflict a rebind in this session made is new, and is the reader's
/// to decide about, so it is named in full -- in the words the rows next
/// to it use, not in the words the log uses.
#[test]
fn a_conflict_this_session_made_is_named_in_the_paragraph() {
    let mut app = headless_app();
    app.finish();
    app.update();
    let mut pending = jackdaw::keybind_settings::pending_from_world(app.world_mut());
    pending.rebind("history.redo", PresetInput::key("KeyZ").ctrl());

    let text = jackdaw::keybind_settings::advisory_text(
        &pending,
        &[],
        &jackdaw_api_internal::keymap::KeymapLoadProblem::default(),
    );
    assert!(text.contains("you have just bound"), "{text}");
    assert!(
        text.contains("Ctrl + Z") && !text.contains("KeyZ"),
        "the chord is written the way a row writes it: {text}",
    );
    assert!(
        text.contains("Redo") && !text.contains("history.redo"),
        "and the commands by the names beside them: {text}",
    );
}

/// Several commands ship with more than one chord. A dialog that could
/// only replace turned each of those into one chord the first time it was
/// touched, and Save wrote that loss to disk.
#[test]
fn an_added_chord_survives_a_save_and_a_reload() {
    let mut app = headless_app();
    app.finish();
    app.update();
    let defaults = classic(&mut app);
    let mut pending = jackdaw::keybind_settings::pending_from_world(app.world_mut());

    let before = pending.chords_of(REBOUND);
    pending.add_chord(REBOUND, PresetInput::key("F9"));
    let after = pending.chords_of(REBOUND);
    assert_eq!(
        after.len(),
        before.len() + 1,
        "the chord it had is still there: {after:?}",
    );
    assert!(after.contains(&"F9".to_string()), "{after:?}");

    let user = pending.to_user_keymap();
    let reloaded: UserKeymap =
        serde_json::from_str(&serde_json::to_string(&user).expect("serialize")).expect("parse");
    app.world_mut().insert_resource(reloaded);
    let reopened = jackdaw::keybind_settings::pending_from_world(app.world_mut());
    assert_eq!(
        reopened.chords_of(REBOUND),
        after,
        "both chords came back off the saved keymap",
    );
    assert_eq!(
        resolve_keymap(&defaults, &pending.to_user_keymap())
            .bindings
            .iter()
            .filter(|binding| binding.operator == REBOUND)
            .count(),
        after.len(),
        "and the resolved keymap holds one row per chord",
    );

    // And a chord can be taken away again, one at a time.
    let mut reopened = reopened;
    reopened.remove_chord(REBOUND, 0);
    assert_eq!(reopened.chords_of(REBOUND), after[1..].to_vec());
}

/// A command that fires on release keeps firing on release when its chord
/// is changed. The dialog used to write `Press` for everything, turning a
/// release binding into a press one with nothing saying so.
#[test]
fn a_rebind_keeps_the_phase_the_command_fires_on() {
    let mut app = headless_app();
    app.finish();
    app.update();
    let mut pending = jackdaw::keybind_settings::pending_from_world(app.world_mut());
    pending.bindings = vec![PresetBinding {
        operator: "some.release.op".to_string(),
        input: PresetInput::key("KeyQ"),
        phase: PresetPhase::Release,
        context: PresetContext::Operators,
    }];

    pending.rebind("some.release.op", PresetInput::key("F9"));
    let row = pending
        .bindings
        .iter()
        .find(|binding| binding.operator == "some.release.op")
        .expect("the row is still there");
    assert_eq!(
        row.phase,
        PresetPhase::Release,
        "a rebind changes the chord, not when the command fires",
    );

    pending.add_chord("some.release.op", PresetInput::key("F10"));
    assert!(
        pending
            .bindings
            .iter()
            .filter(|binding| binding.operator == "some.release.op")
            .all(|binding| binding.phase == PresetPhase::Release),
        "and neither does adding one",
    );
}

/// The draw-brush modal is reached by four chords, two of them hanging
/// off marker actions of their own. The dialog used to show the two on
/// its own action, so the two the user actually presses were invisible.
#[test]
fn the_draw_brush_row_lists_the_chords_that_reach_it() {
    let mut app = headless_app();
    app.finish();
    app.update();
    let pending = jackdaw::keybind_settings::pending_from_world(app.world_mut());
    let row = pending
        .rows
        .iter()
        .find(|row| row.operator == "viewport.draw_brush_modal")
        .expect("the modal has a row");

    assert!(!row.is_editable(), "its chords are attached in code");
    for chord in ["C", "Alt + B", "B"] {
        assert!(
            row.fixed.iter().any(|held| held == chord),
            "{chord} reaches the draw-brush modal and must be listed: {:?}",
            row.fixed,
        );
    }
    assert!(
        row.reason().contains("attached in code"),
        "the row says why it cannot be changed: {}",
        row.reason(),
    );
}

/// A keymap that will not parse loads as empty, and the next Save writes
/// the whole file: leaving the unparseable one in place would destroy
/// whatever was in it without ever showing it to anyone.
///
/// A second corruption is kept too, beside the first rather than over it:
/// a rescue that clobbers the last rescue is no rescue at all.
#[test]
fn a_corrupt_keymap_is_kept_beside_itself_and_reported() {
    use jackdaw_api_internal::keymap::load_user_keymap_reporting;

    let guard = CONFIG_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = empty_config_dir();
    let path = dir.join("keymap.json");
    let kept = dir.join("keymap.json.invalid");
    let second = dir.join("keymap.json.invalid.2");
    // The first rescue is put there by hand, so what is pinned below is
    // that the corruption arriving now does not take it with it.
    let _ = std::fs::remove_file(&second);
    std::fs::write(&kept, "an earlier rescue").expect("write an earlier rescue");
    std::fs::write(&path, "{ this is not json").expect("write a corrupt keymap");

    let (keymap, problem) = load_user_keymap_reporting();
    assert_eq!(
        keymap,
        UserKeymap::default(),
        "a file that will not parse is no overrides at all",
    );
    assert_eq!(
        std::fs::read_to_string(&kept).expect("read the earlier rescue"),
        "an earlier rescue",
        "the rescue already there is untouched",
    );
    assert!(
        second.is_file(),
        "the unreadable file is kept beside it as {}",
        second.display(),
    );
    assert!(
        !path.is_file(),
        "and moved out of the way, so Save does not write over it",
    );
    assert_eq!(
        std::fs::read_to_string(&second).expect("read the kept file"),
        "{ this is not json",
        "kept exactly as it was, so it can still be rescued by hand",
    );
    assert!(problem.is_some(), "and the dialog is told");
    assert!(
        problem.message.contains("keymap.json.invalid.2"),
        "the notice names where it went: {}",
        problem.message,
    );

    let advisory = jackdaw::keybind_settings::advisory_text(
        &jackdaw::keybind_settings::PendingKeymapChanges::default(),
        &[],
        &problem,
    );
    assert!(advisory.contains("keymap.json.invalid"), "{advisory}");

    let _ = std::fs::remove_file(&kept);
    let _ = std::fs::remove_file(&second);
    drop(guard);
}

/// A file that cannot be read at all is no safer than one that will not
/// parse: the next Save writes the whole thing, so it is moved aside too.
#[test]
fn a_keymap_that_cannot_be_read_is_kept_as_well() {
    use jackdaw_api_internal::keymap::load_user_keymap_reporting;

    let guard = CONFIG_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = empty_config_dir();
    let path = dir.join("keymap.json");
    let kept = dir.join("keymap.json.invalid");
    let _ = std::fs::remove_file(&kept);
    // Bytes no `read_to_string` can turn into text, which is the shape a
    // truncated or half-written file arrives in.
    std::fs::write(&path, [0x80u8, 0x81, 0x82]).expect("write an unreadable keymap");

    let (keymap, problem) = load_user_keymap_reporting();
    assert_eq!(keymap, UserKeymap::default());
    assert!(
        kept.is_file(),
        "the unreadable file is kept as {}",
        kept.display(),
    );
    assert!(
        !path.is_file(),
        "and moved out of the way, so Save does not write over it",
    );
    assert_eq!(
        std::fs::read(&kept).expect("read the kept file"),
        vec![0x80u8, 0x81, 0x82],
        "byte for byte, so it can still be rescued by hand",
    );
    assert!(
        problem.message.contains("keymap.json.invalid"),
        "the notice names where it went: {}",
        problem.message,
    );

    let _ = std::fs::remove_file(&kept);
    drop(guard);
}

/// When the rescue itself fails there is still an unread file on disk, and
/// writing the keymap over it is the loss the rescue exists to prevent.
/// Save refuses instead, and says so.
#[test]
fn a_save_refuses_while_an_unrescued_keymap_is_still_there() {
    use jackdaw_api_internal::keymap::save_user_keymap;

    let guard = CONFIG_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = empty_config_dir();
    let path = dir.join("keymap.json");
    // The state a failed rescue leaves: a file nobody could read, still
    // where the next Save would write.
    std::fs::write(&path, "{ this is not json").expect("write a corrupt keymap");

    assert!(
        !save_user_keymap(&UserKeymap::default()),
        "the save refused rather than writing over what was there",
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("the file is still there"),
        "{ this is not json",
        "and the file is exactly as it was",
    );

    // With nothing unread in the way, the same Save writes.
    std::fs::remove_file(&path).expect("clear the corrupt file");
    assert!(save_user_keymap(&UserKeymap::default()));
    assert!(path.is_file());

    let _ = std::fs::remove_file(&path);
    drop(guard);
}

/// A keymap the editor could not read costs the user every override they
/// had. The dialog says so, but nobody opens Preferences to find out why:
/// the status bar says it too, on the first frame there is one.
#[test]
fn a_keymap_that_would_not_load_is_said_out_loud() {
    let guard = CONFIG_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = empty_config_dir();
    let path = dir.join("keymap.json");
    let kept = dir.join("keymap.json.invalid");
    let _ = std::fs::remove_file(&kept);
    std::fs::write(&path, "{ this is not json").expect("write a corrupt keymap");

    let mut app = crate::util::editor_test_app();
    crate::enter_editor(&mut app);

    let notice = app.world().resource::<jackdaw::status_bar::StatusNotice>();
    assert!(notice.is_active(), "the status bar carries the refusal");
    assert!(
        notice.text().contains("keymap.json"),
        "and names the file it could not read: {}",
        notice.text(),
    );

    let _ = std::fs::remove_file(&kept);
    let _ = std::fs::remove_file(&path);
    drop(guard);
}
