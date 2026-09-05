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
    #[cfg(feature = "dylib")]
    app.add_systems(
        OnEnter(crate::AppState::Editor),
        load_installed_extensions_on_open,
    );
}

/// Which installed bundles this session has yet to load: the ones the
/// catalog does not already hold and the user has not turned off.
///
/// The startup scan runs while the app is being built, so a bundle
/// installed by any other process afterwards - including the one that
/// built the project's own extension - is invisible until the editor is
/// restarted. Opening a project is where that is worth looking again.
#[cfg_attr(
    all(not(feature = "dylib"), not(test)),
    expect(dead_code, reason = "its only caller is behind the `dylib` feature")
)]
fn unloaded_installed<'a>(
    installed: impl IntoIterator<Item = (&'a str, &'a std::path::Path)>,
    known: impl Fn(&str) -> bool,
    turned_off: impl Fn(&str) -> bool,
) -> Vec<std::path::PathBuf> {
    installed
        .into_iter()
        .filter(|(id, _)| !known(id) && !turned_off(id))
        .map(|(_, path)| path.to_path_buf())
        .collect()
}

/// Load the installed extensions this session has not seen, on opening
/// a project.
#[cfg(feature = "dylib")]
fn load_installed_extensions_on_open(world: &mut World) {
    use jackdaw_api_internal::extensions_config::read_extension_config;
    use jackdaw_api_internal::lifecycle::ExtensionCatalog;

    let installed = match jackdaw_loader::package::list_installed() {
        Ok(installed) => installed,
        Err(error) => {
            warn!("Could not read installed extensions: {error}");
            return;
        }
    };
    let config = read_extension_config().unwrap_or_default();
    let catalog = world.resource::<ExtensionCatalog>();
    let pending = unloaded_installed(
        installed
            .iter()
            .map(|entry| (entry.manifest.id.as_str(), entry.library_path.as_path())),
        |id| catalog.contains(id),
        |id| config.get(id).is_some_and(|entry| !entry.enabled),
    );
    for library in pending {
        match jackdaw_loader::load_installed_from_path(world, &library) {
            Ok(id) => info!("Loaded extension `{id}` from {}", library.display()),
            Err(error) => warn!("Failed to load {}: {error}", library.display()),
        }
    }
}

/// Report a keymap that could not be read in the status bar, on the first frame
/// there is one. The Preferences dialog says it too, but only if opened.
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

#[cfg(test)]
mod tests {
    use super::unloaded_installed;

    use std::path::{Path, PathBuf};

    #[test]
    fn only_bundles_the_session_has_not_seen_are_loaded() {
        let installed = [
            ("known", Path::new("/ext/known.so")),
            ("fresh", Path::new("/ext/fresh.so")),
            ("off", Path::new("/ext/off.so")),
        ];
        let pending = unloaded_installed(installed, |id| id == "known", |id| id == "off");
        assert_eq!(pending, vec![PathBuf::from("/ext/fresh.so")]);
    }

    #[test]
    fn nothing_installed_loads_nothing() {
        let pending = unloaded_installed([], |_| false, |_| false);
        assert!(pending.is_empty());
    }
}
