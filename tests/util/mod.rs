use std::borrow::Cow;

use bevy::{
    prelude::*,
    render::{
        RenderPlugin,
        settings::{RenderCreation, WgpuSettings},
    },
    winit::WinitPlugin,
};
use jackdaw::prelude::*;
use jackdaw_api_internal::lifecycle::{ExtensionAppExt as _, OperatorEntity, enable_extension};
use jackdaw_api_internal::snapshot::{ActiveSnapshotter, SceneSnapshot};

pub fn headless_app() -> App {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(RenderPlugin {
                render_creation: RenderCreation::Automatic(Box::new(WgpuSettings {
                    backends: None,
                    ..default()
                })),
                ..default()
            })
            // Headless integration tests never exercise sound. Disabling the
            // backend avoids attempts to connect to host ALSA/JACK services,
            // which can otherwise block an otherwise-complete test process.
            .disable::<bevy::audio::AudioPlugin>()
            .disable::<WinitPlugin>(),
    )
    // Ambient plugins moved to the binary entry point (matches
    // the launcher's `src/main.rs` and the static template's
    // `editor.rs.template`). Mirror that here so the editor's
    // internal `debug_assert!`s for `PhysicsSchedulePlugin` and
    // `EnhancedInputPlugin` find what they expect.
    .add_plugins((
        avian3d::prelude::PhysicsPlugins::default(),
        bevy_enhanced_input::prelude::EnhancedInputPlugin,
    ))
    .add_plugins(JackdawEditorPlugins::default());
    // Bevy 0.19's component-sync hooks (`On<Remove, SyncToRenderWorld>`) read the
    // main-world `PendingSyncEntity` resource that `SyncWorldPlugin` installs.
    // The headless `RenderPlugin` (no backend) never spins up the render world,
    // so that plugin isn't pulled in here; add it explicitly so despawning a
    // sync-component entity (e.g. through `scene.new`) doesn't panic.
    if !app.is_plugin_added::<bevy::render::sync_world::SyncWorldPlugin>() {
        app.add_plugins(bevy::render::sync_world::SyncWorldPlugin);
    }
    app
}

/// Like [`headless_app`] but also runs the startup pass and ticks one
/// frame so every built-in extension is registered, enabled, and its
/// operators populated in the `OperatorIndex`. Most operator integration
/// tests should start here.
#[expect(clippy::allow_attributes, reason = "shared across test binaries")]
#[allow(
    dead_code,
    reason = "shared across test binaries; not every test exercises this path."
)]
pub fn editor_test_app() -> App {
    let mut app = headless_app();
    app.finish();
    // First tick runs Startup + extension auto-enable so every
    // built-in's operator entities are spawned.
    app.update();
    app
}

/// Register `T` in the catalog AND enable it.
///
/// `register_extension` alone only adds the extension to the catalog; the
/// editor's normal startup enables whatever `~/.config/jackdaw/extensions.json`
/// lists, plus [`REQUIRED_EXTENSIONS`](jackdaw::extensions_config::REQUIRED_EXTENSIONS).
/// Tests don't populate the on-disk config, so custom test extensions would
/// otherwise stay disabled and their operators wouldn't resolve.
///
/// This helper runs the usual `register -> finish -> first-update` dance, then
/// force-enables the extension explicitly so `app.world_mut().operator(id)`
/// can find it. It also ticks one more frame so any setup observers (operator
/// index, BEI context attachment) have settled before the caller runs
/// assertions.
#[expect(clippy::allow_attributes, reason = "Some tests use this")]
#[allow(
    dead_code,
    reason = "shared across integration test binaries; not every test file calls it."
)]
pub fn register_and_enable_extension<T: JackdawExtension + Default>(app: &mut App) {
    app.register_extension::<T>();
    app.finish();
    // First update runs Startup (which enables whatever the on-disk config
    // lists; typically nothing relevant to the test).
    app.update();
    // Force-enable the test extension; idempotent if it was already enabled
    // (returns `None` in that case).
    enable_extension(app.world_mut(), &T::default().id());
    // Let any on-add observers for the operator entities settle before the
    // caller starts asserting.
    app.update();
}

/// Collect every registered operator id in the world. Reads from
/// `OperatorEntity` components rather than the (private) `OperatorIndex`
/// resource. Sorted so test failures are stable.
#[expect(clippy::allow_attributes, reason = "shared across test binaries")]
#[allow(dead_code, reason = "smoke + availability tests use this")]
pub fn iter_operator_ids(app: &mut App) -> Vec<Cow<'static, str>> {
    let mut ids: Vec<Cow<'static, str>> = app
        .world_mut()
        .query::<&OperatorEntity>()
        .iter(app.world())
        .map(|op| Cow::Borrowed(op.id()))
        .collect();
    ids.sort();
    ids
}

/// Copy a fixture crate from `tests/fixtures/<name>` into a staging dir under
/// `target/` and return the staged path. Building one writes a lockfile, a
/// redirect plan and a target dir, none of which belong in the committed tree.
/// The staged target dir survives between runs so dependencies are not rebuilt.
///
/// Staging preserves the layout, so a fixture depending on `../sibling` works
/// as long as the caller stages that sibling too.
#[expect(clippy::allow_attributes, reason = "shared across test binaries")]
#[allow(dead_code, reason = "the SDK-pipeline tests use this")]
pub fn stage_fixture(name: &str) -> std::path::PathBuf {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src = root.join("tests/fixtures").join(name);
    assert!(src.is_dir(), "no fixture crate at {}", src.display());
    let dst = root.join("target/fixture-stage").join(name);
    copy_dir(&src, &dst);
    dst
}

#[expect(clippy::allow_attributes, reason = "shared across test binaries")]
#[allow(dead_code, reason = "only reachable through stage_fixture")]
fn copy_dir(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("create staging dir");
    for entry in std::fs::read_dir(src).expect("read fixture dir") {
        let entry = entry.expect("fixture dir entry");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to);
        } else {
            std::fs::copy(&from, &to)
                .unwrap_or_else(|e| panic!("copy {} -> {}: {e}", from.display(), to.display()));
        }
    }
}

/// Every registered `(id, label)` pair. Two entries sharing an id means two
/// subsystems registered that id; the dispatcher's index is last-registration
/// -wins, so only one of them is reachable by id.
#[expect(clippy::allow_attributes, reason = "shared across test binaries")]
#[allow(dead_code, reason = "scene op id tests use this")]
pub fn operator_id_labels(app: &mut App) -> Vec<(&'static str, &'static str)> {
    app.world_mut()
        .query::<&OperatorEntity>()
        .iter(app.world())
        .map(|op| (op.id(), op.label()))
        .collect()
}

/// Every label registered under `id`. More than one entry means the id is
/// ambiguous; see [`operator_id_labels`].
#[expect(clippy::allow_attributes, reason = "shared across test binaries")]
#[allow(dead_code, reason = "scene op id tests use this")]
pub fn operator_labels(app: &mut App, id: &str) -> Vec<&'static str> {
    app.world_mut()
        .query::<&OperatorEntity>()
        .iter(app.world())
        .filter(|op| op.id() == id)
        .map(OperatorEntity::label)
        .collect()
}

/// Launch a built game binary windowless over the PIE link, wait for
/// the game to report `BSN_SCENE_LOADED ... has_target=true` on stderr, and
/// return whether it did along with the captured stderr. Shared by the
/// end-to-end Play tests.
#[expect(clippy::allow_attributes, reason = "shared across test binaries")]
#[allow(dead_code, reason = "only the binary Play e2e tests use this")]
pub fn run_windowless_game(
    binary: &std::path::Path,
    cwd: &std::path::Path,
    extra_env: &[(&str, &std::ffi::OsStr)],
) -> (bool, String) {
    use std::io::BufRead as _;
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};

    let (handle, server_name) = jackdaw_pie_protocol::serve().expect("open the ipc rendezvous");
    let mut command = std::process::Command::new(binary);
    jackdaw_project_build::prepare_game_command(&mut command, binary);
    command
        .current_dir(cwd)
        .env("JACKDAW_PIE", &server_name)
        .env("JACKDAW_PIE_WINDOWLESS", "1")
        .stderr(std::process::Stdio::piped());
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let mut child = command.spawn().expect("spawn the game");

    let child_stderr = child.stderr.take().expect("piped stderr");
    let stderr_buf = Arc::new(Mutex::new(String::new()));
    {
        let stderr_buf = Arc::clone(&stderr_buf);
        std::thread::spawn(move || {
            let reader = std::io::BufReader::new(child_stderr);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let mut buf = stderr_buf.lock().unwrap();
                buf.push_str(&line);
                buf.push('\n');
            }
        });
    }

    // The game's JackdawPlugin connects on boot; accept so it does not block.
    let (accept_tx, accept_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = accept_tx.send(handle.accept());
    });
    // On timeout the child's stderr is the only evidence of why it never
    // got as far as connecting.
    let accepted = accept_rx.recv_timeout(Duration::from_secs(90));
    if accepted.is_err() {
        let captured = stderr_buf.lock().unwrap().clone();
        let captured = if captured.trim().is_empty() {
            "(the game produced no output at all)".to_string()
        } else {
            captured
        };
        let _ = child.kill();
        panic!("the game never connected to the PIE link. Its output was:\n{captured}");
    }
    let _transport = accepted.expect("checked above").expect("ipc accept failed");

    let deadline = Instant::now() + Duration::from_secs(90);
    let mut loaded = false;
    while Instant::now() < deadline {
        {
            let buf = stderr_buf.lock().unwrap();
            if buf.contains("BSN_SCENE_LOADED") && buf.contains("has_target=true") {
                loaded = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let _ = child.kill();
    let _ = child.wait();
    let stderr = stderr_buf.lock().unwrap().clone();
    (loaded, stderr)
}

/// Capture a scene snapshot via the `ActiveSnapshotter`. Wrapper around
/// the standard `resource_scope` dance used by the dispatcher.
#[expect(clippy::allow_attributes, reason = "shared across test binaries")]
#[allow(dead_code, reason = "modal + undo tests use this")]
pub fn snapshot(app: &mut App) -> Box<dyn SceneSnapshot> {
    app.world_mut()
        .resource_scope(|world, snapshotter: Mut<ActiveSnapshotter>| snapshotter.0.capture(world))
}

#[expect(clippy::allow_attributes, reason = "Some tests use this")]
#[allow(
    dead_code,
    reason = "shared across integration test binaries; not every test file exercises operator dispatch."
)]
pub trait OperatorResultExt: Copy {
    /// Asserts that the operator finished successfully and panics if it did not.
    /// Hidden away in test utils so extension devs don't fall into the trap of actually doing this in production.
    fn assert_finished(self);

    /// Asserts that the operator was cancelled (e.g. its availability
    /// gate refused, or the call hit a no-op early-return). Used by
    /// gate-blocked dispatch tests.
    fn assert_cancelled(self);

    /// Asserts that the operator returned `Running`, indicating it has
    /// entered a modal session. Used by modal start tests.
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

/// Make sure the SDK's manifest and its native link search paths exist
/// before a pipeline test drives cargo directly.
///
/// The driver generates these on demand, but these tests bypass it and
/// invoke `cargo rustc` with the wrapper themselves. Without the search
/// paths a consumer cannot find import libraries the SDK's crates link
/// by bare name, which on Windows is every `windows.*.lib` and fails the
/// link.
#[expect(clippy::allow_attributes, reason = "shared across test binaries")]
#[allow(dead_code, reason = "used by the SDK pipeline tests only")]
pub fn ensure_sdk_metadata(sdk: &jackdaw::sdk_paths::SdkPaths) {
    use jackdaw::project_build::plan::SdkManifest;

    let workspace = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    SdkManifest::generate(&workspace, sdk, &["-p", "jackdaw", "--features", "dylib"])
        .expect("generate the SDK manifest for the fixture");
}
