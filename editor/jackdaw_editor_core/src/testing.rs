//! Headless editor apps for integration tests, shared by the editor's test
//! binaries and the feature crates' own tests.

use std::path::{Path, PathBuf};
use std::time::Duration;

use bevy::{
    prelude::*,
    render::{
        RenderPlugin,
        settings::{RenderCreation, WgpuSettings},
    },
    winit::WinitPlugin,
};
use jackdaw_api::prelude::OperatorResult;

/// The CC0 overworld tile sheet: 16 px tiles, 27 columns, 432 x 1040.
pub const OVERWORLD_SHEET: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../crates/jackdaw_tilemap/tests/data/punyworld/punyworld-overworld-tileset.png"
);

/// Wave the first-run SDK setup check past for the whole test process. A test
/// binary is built from the workspace the embedded recipe is cut from, so any
/// edit to it makes the bootstrap stamp stale and every editor app a test
/// builds would put up the setup screen.
pub fn skip_setup_check() {
    jackdaw_project_build::bootstrap::skip_setup_check();
}

/// Point the editor's per-user config directory at `dir` for this test
/// process, unless one is chosen already, so no test reads or writes the
/// keymap, recent projects or extension list of whoever runs it. The first
/// call wins; call it before the first plugin is added, so no other thread
/// reads the environment while it is set.
pub fn isolate_config_dir(dir: PathBuf) {
    static ISOLATED: std::sync::Once = std::sync::Once::new();
    ISOLATED.call_once(|| {
        let chosen =
            std::env::var_os(jackdaw_env::paths::CONFIG_DIR_VAR).is_some_and(|dir| !dir.is_empty());
        if chosen {
            return;
        }
        unsafe { std::env::set_var(jackdaw_env::paths::CONFIG_DIR_VAR, dir) };
    });
}

/// The plugins a binary adds under the editor, reading assets through
/// `assets`, with no renderer backend, window or audio.
pub fn ambient_app(assets: bevy::asset::AssetPlugin) -> App {
    skip_setup_check();
    isolate_config_dir(std::env::temp_dir().join(format!("jackdaw-config-{}", std::process::id())));
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(assets)
            .set(RenderPlugin {
                render_creation: RenderCreation::Automatic(Box::new(WgpuSettings {
                    backends: None,
                    ..default()
                })),
                ..default()
            })
            // Disabling the audio backend avoids connecting to host sound
            // services, which can block an otherwise-complete test process.
            .disable::<bevy::audio::AudioPlugin>()
            .disable::<WinitPlugin>(),
    );
    app
}

/// The core editor, over an app that already carries the ambient plugins.
pub fn add_editor_plugins(app: &mut App) {
    app.add_plugins(crate::JackdawEditorPlugins::default());
    // Component-sync hooks read the `PendingSyncEntity` resource that
    // `SyncWorldPlugin` installs, and the headless renderer never adds it.
    if !app.is_plugin_added::<bevy::extract::sync_world::SyncWorldPlugin<bevy::render::RenderApp>>()
    {
        app.add_plugins(bevy::extract::sync_world::SyncWorldPlugin::<
            bevy::render::RenderApp,
        >::default());
    }
}

/// The core editor plus `features`, past its startup frame, so every
/// built-in extension is registered and enabled. `assets` is the asset root,
/// or `None` for the default.
pub fn editor_test_app_with(assets: Option<&Path>, features: impl FnOnce(&mut App)) -> App {
    let plugin = match assets {
        Some(dir) => bevy::asset::AssetPlugin {
            file_path: dir.to_string_lossy().into_owned(),
            ..default()
        },
        None => bevy::asset::AssetPlugin::default(),
    };
    let mut app = ambient_app(plugin);
    add_editor_plugins(&mut app);
    features(&mut app);
    app.finish();
    app.update();
    app
}

/// Run frames until `done` holds, failing the test once `limit` has passed
/// without it. `what` names the awaited state in the failure.
pub fn update_until(
    app: &mut App,
    what: &str,
    limit: Duration,
    mut done: impl FnMut(&mut App) -> bool,
) {
    let started = std::time::Instant::now();
    while !done(app) {
        assert!(
            started.elapsed() < limit,
            "gave up after {limit:?} waiting for {what}"
        );
        app.update();
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Run frames until the scene load the last open began is over. The limit is
/// under the load's own stall limit, so a model that never comes in fails the
/// test instead of ending the load quietly.
pub fn settle_scene_load(app: &mut App) {
    update_until(
        app,
        "the scene load to finish",
        Duration::from_secs(45),
        |app| {
            !app.world()
                .resource::<crate::progress::EditorProgress>()
                .is_running(crate::scenes::load_progress::SCENE_LOAD)
        },
    );
}

/// Advance the app's clock by a fixed step per frame, so gestures measured
/// in seconds (double clicks, spring loads) read the same on a loaded
/// runner as on a fast machine.
pub fn fixed_frame_clock(app: &mut App) {
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        Duration::from_millis(16),
    ));
}

/// Write [`OVERWORLD_SHEET`] to `to` re-packed with a transparent pixel
/// between neighbouring tiles: 458 x 1104.
pub fn write_spaced_overworld_sheet(to: &Path) {
    let packed = image::open(OVERWORLD_SHEET)
        .expect("the sheet decodes")
        .to_rgba8();
    let (w, h) = packed.dimensions();
    let mut spaced = image::RgbaImage::new(w / 16 * 17 - 1, h / 16 * 17 - 1);
    for (x, y, pixel) in packed.enumerate_pixels() {
        spaced.put_pixel(x + x / 16, y + y / 16, *pixel);
    }
    spaced.save(to).expect("write the spaced sheet");
}

pub trait OperatorResultExt: Copy {
    /// Asserts that the operator finished successfully and panics if it did not.
    fn assert_finished(self);

    /// Asserts that the operator was cancelled (e.g. its availability gate
    /// refused, or the call hit a no-op early return).
    fn assert_cancelled(self);

    /// Asserts that the operator returned `Running`, indicating it has
    /// entered a modal session.
    fn assert_running(self);
}

impl OperatorResultExt for OperatorResult {
    fn assert_finished(self) {
        assert_eq!(self, OperatorResult::Finished, "Operator failed to finish");
    }
    fn assert_cancelled(self) {
        assert_eq!(
            self,
            OperatorResult::Cancelled,
            "Operator did not cancel as expected"
        );
    }
    fn assert_running(self) {
        assert_eq!(
            self,
            OperatorResult::Running,
            "Operator did not enter modal Running state"
        );
    }
}
