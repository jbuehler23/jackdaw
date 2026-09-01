//! The keymap: the shipped presets and the user's own overrides layered
//! over them.
//!
//! One binary per theme: each module below was its own test binary, and
//! linking the editor once instead of twice is what the split cost.

#[path = "../util/mod.rs"]
mod util;

mod capture_gate;
mod keybind_dialog;
mod keymap_presets;
mod keymap_user_overrides;

use bevy::prelude::App;

/// A config directory of this test binary's own.
///
/// `headless_app` builds the editor, and the editor reads the user keymap
/// from the config directory at startup. Without this the suite would read
/// whoever is running it: a developer with a rebound chord would see these
/// tests fail on their machine and nowhere else.
///
/// The redirect is process-wide, so it belongs at the binary's root rather
/// than in either module: it is installed once, and every app either
/// module builds goes through [`headless_app`] below and so is built after
/// it.
pub(crate) static CONFIG_DIR: std::sync::LazyLock<std::path::PathBuf> =
    std::sync::LazyLock::new(|| {
        let dir = std::env::temp_dir().join(format!("jackdaw_keymap_tests_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a config directory for the suite");
        // SAFETY: the only writer, and it runs before this binary builds an
        // app, so nothing is reading the variable meanwhile. The value
        // outlives the process.
        unsafe { std::env::set_var(jackdaw_env::paths::CONFIG_DIR_VAR, &dir) };
        dir
    });

/// One config directory serves the whole binary, and its tests run in
/// parallel: whoever is touching the keymap file holds this.
pub(crate) static CONFIG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The suite's own config directory, with no keymap file in it.
pub(crate) fn empty_config_dir() -> &'static std::path::Path {
    let dir = CONFIG_DIR.as_path();
    let _ = std::fs::remove_file(dir.join("keymap.json"));
    dir
}

/// Take `app` into the editor proper.
///
/// A headless app is parked in `AppState::ProjectSelect`, and most of the
/// editor's systems -- the keybind dialog's, the status bar's -- are gated
/// on the state it is not in. The panels that read the open project fail
/// their parameter validation without one, so a project goes in first.
pub(crate) fn enter_editor(app: &mut App) {
    use bevy::prelude::*;
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: CONFIG_DIR.join("project"),
            config: default(),
        });
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    for _ in 0..4 {
        app.update();
    }
}

/// A headless editor that read an empty override file, whatever any other
/// test is doing with that file meanwhile.
pub(crate) fn headless_app() -> App {
    let guard = CONFIG_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    empty_config_dir();
    let app = util::headless_app();
    drop(guard);
    app
}
