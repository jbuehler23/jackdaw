use bevy::prelude::*;
use jackdaw_api_internal::keymap::{
    ActiveKeymapPreset, BuiltinActions, DefaultKeymap, KeymapApplyReport, KeymapCapture,
    KeymapLoadProblem, UserKeymap, apply_keymap_preset, load_user_keymap_reporting, resolve_keymap,
};
use jackdaw_api_internal::lifecycle::enable_extension;

/// Load the user keymap, keeping what was wrong with it on disk in the
/// world so the keybind dialog can say it out loud rather than leaving
/// the overrides to look like they were never saved.
fn insert_user_keymap(app: &mut App) {
    let (keymap, problem) = load_user_keymap_reporting();
    app.insert_resource(keymap).insert_resource(problem);
}

use crate::extension_resolution::resolve_enabled_list;
use crate::input_contexts::spawn_contexts;

pub(super) fn plugin(app: &mut App) {
    // Must run after every plugin's `finish()`: BEI initializes
    // `ContextInstances<PreUpdate>` there, and spawning a context
    // entity before that resource exists panics.
    //
    // Ordering guarantee: `spawn_contexts` runs before `apply_active_keymap`
    // so the `BuiltinActions` and `DefaultKeymap` entries for modal/nav are
    // present when the applier iterates preset bindings.
    //
    // `apply_active_keymap` chains after `apply_enabled_extensions_startup`
    // so extensions have registered all DefaultKeymap entries before
    // bindings are applied.
    insert_user_keymap(app);
    app.init_resource::<BuiltinActions>()
        .init_resource::<DefaultKeymap>()
        .init_resource::<KeymapCapture>()
        .add_systems(
            Startup,
            (
                apply_enabled_extensions_startup,
                spawn_contexts,
                apply_active_keymap,
            )
                .chain(),
        )
        .add_systems(
            OnEnter(crate::AppState::Editor),
            announce_keymap_load_problem,
        );
}

/// Say out loud that the keymap on disk could not be read.
///
/// The dialog says it too, but only to someone who opens it, and nobody
/// opens Preferences to find out why their chords went back to the
/// defaults. The status bar is where the editor says what it could not do,
/// so it says this there as well, on the first frame there is a status bar
/// to say it in.
fn announce_keymap_load_problem(world: &mut World) {
    let Some(problem) = world.get_resource::<KeymapLoadProblem>() else {
        return;
    };
    if !problem.is_some() {
        return;
    }
    let message = problem.message.clone();
    crate::status_bar::notify_error(world, message);
}

/// The outcome of the last keymap application: which entries named
/// something no loaded extension provides, and which chords more than
/// one action claims. The keybind dialog reads it so a saved keymap
/// reports what it could not do rather than doing it silently.
#[derive(Resource, Default)]
pub(crate) struct LastKeymapApply(pub(crate) KeymapApplyReport);

/// Enable every catalog entry `resolve_enabled_list` reports as on.
fn apply_enabled_extensions_startup(world: &mut World) {
    let to_enable = resolve_enabled_list(world);
    for name in &to_enable {
        enable_extension(world, name);
    }
}

/// Apply the active keymap preset once extensions finish registering.
/// Only "classic" ships today; unknown names warn and fall back.
///
/// Runs at startup and again after the keybind dialog saves, so a rebind
/// takes effect in the session that made it. The user's own rows are
/// layered over the defaults here and nowhere else.
pub(crate) fn apply_active_keymap(world: &mut World) {
    let defaults = world
        .get_resource_or_init::<DefaultKeymap>()
        .to_classic_preset();
    let active = world.get_resource_or_init::<ActiveKeymapPreset>().clone();
    if active.name != "classic" {
        warn!(
            "unknown keymap preset '{}'; falling back to classic",
            active.name
        );
    }
    let user = world.get_resource_or_init::<UserKeymap>().clone();
    let resolved = resolve_keymap(&defaults, &user);
    let report = apply_keymap_preset(world, &resolved);
    info!(
        "applied keymap preset 'classic': {} entries, {} bindings, {} user rows",
        report.applied_entries,
        report.spawned_bindings,
        user.bindings.len(),
    );
    world.insert_resource(LastKeymapApply(report));
}
