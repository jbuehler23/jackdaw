//! The editor's project build glue: the pre-build on open, the auto-build on
//! source change, the persisted build settings, and the schema watch that
//! refreshes project types. Re-exports the editor-independent build pipeline
//! ([`jackdaw_project_build`]) at the editor's historical module path.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use jackdaw_api::pie::PlayState;

use crate::build_status::BuildStatus;
use crate::pie::{
    BuildState as PieBuildState, PieSession, project_root, source_is_current,
    source_unchanged_since, spawn_game_build,
};
use crate::run_config::RunConfigs;

pub use jackdaw_project_build::{
    BuildEvent, BuildLoad, ProjectBinaryBuild, ProjectBuild, ProjectBuildError,
    build_project_binary, build_project_binary_with_load, build_project_dylib, cargo_meta,
    detach_from_host_build, linkage, plan, prepare_game_command, project_manifest, shim,
    shim_spec_for_project,
};

pub struct ProjectBuildPlugin;

impl Plugin for ProjectBuildPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PiePrebuildState>()
            .init_resource::<crate::project_types::ProjectTypes>()
            .init_resource::<crate::project_definitions::ProjectDefinitionKinds>()
            .init_resource::<ProjectSchemaWatch>()
            .init_resource::<EditorBuildSettings>()
            .add_systems(
                Update,
                (
                    prebuild_play_target,
                    sync_project_build_settings,
                    watch_project_source_for_edit,
                    watch_project_schema,
                )
                    .run_if(in_state(crate::AppState::Editor)),
            );
    }
}

/// One-shot flag so the background pre-build is attempted only once per editor
/// session, without re-running `cargo metadata` every frame.
#[derive(Resource, Default)]
pub struct PiePrebuildState {
    attempted: bool,
}

/// Turns the open-time pre-build off when set to `0`, `false` or empty.
pub const ENV_PIE_PREBUILD: &str = "JACKDAW_PIE_PREBUILD";

/// Whether [`ENV_PIE_PREBUILD`]'s value asks for the pre-build. Absent
/// means yes.
fn env_allows_prebuild(raw: Option<&str>) -> bool {
    !raw.is_some_and(|value| matches!(value.trim(), "" | "0" | "false"))
}

/// Whether this session should pre-build. The environment wins, so a
/// scripted or CI run can suppress it whatever the project remembers.
fn prebuild_wanted(world: &World) -> bool {
    env_allows_prebuild(
        std::env::var_os(ENV_PIE_PREBUILD)
            .and_then(|value| value.into_string().ok())
            .as_deref(),
    ) && world
        .get_resource::<EditorBuildSettings>()
        .is_none_or(|settings| settings.prebuild)
}

/// Pre-build the game in the background once the editor opens, so the
/// first Play reuses a warm artifact instead of compiling on demand.
/// Mirrors `launch_instance`'s build but with no pending spawn: it only
/// drives `PieSession.builds` to `Done`, which a later Play reuses
/// immediately.
pub(crate) fn prebuild_play_target(world: &mut World) {
    if world.resource::<PiePrebuildState>().attempted {
        return;
    }
    // Wait until the project's run configs have loaded.
    if world.get_resource::<RunConfigs>().is_none() {
        return;
    }
    // Run configs are loaded; this is the one-shot attempt regardless of outcome.
    world.resource_mut::<PiePrebuildState>().attempted = true;
    let Some(root) = project_root(world) else {
        return;
    };
    // Loading an extension built in an earlier session compiles nothing, so
    // it does not wait on the pre-build setting.
    if crate::extension_build::load_recorded_build(world, &root) {
        info!("PIE: loading the extension's last build, which is up to date");
        return;
    }
    if !prebuild_wanted(world) {
        info!("PIE: pre-build disabled; the first Play will compile the game");
        return;
    }
    if crate::extension_build::is_extension_project(world, &root, false) {
        info!("PIE: building the extension in the background to load it");
        crate::extension_build::start_extension_build(world, &root);
        return;
    }
    let spec = match crate::project_build::shim_spec_for_project(&root) {
        Ok(spec) => spec,
        Err(error) => {
            // Without a resolvable package there is nothing to compile,
            // so the editor would otherwise open with an empty Add
            // Component list and no explanation anywhere.
            let reason = error.to_string();
            warn!("PIE: cannot build {}: {reason}", root.display());
            if let Some(mut status) = world.get_resource_mut::<BuildStatus>() {
                status.state = crate::build_status::BuildState::Blocked { reason };
            }
            return;
        }
    };
    let mut session = world.non_send_mut::<PieSession>();
    if session.builds.contains_key(&root) {
        return; // already built or building (the user may have hit Play already)
    }
    info!("PIE: pre-building the game in the background for a fast first Play");
    let (task, progress) = spawn_game_build(&root, spec, BuildLoad::Background);
    session.builds.insert(
        root,
        PieBuildState::Running {
            task,
            pending: Vec::new(),
            progress,
        },
    );
}

/// Throttle so the edit-mode source scan runs about once a second
/// rather than every frame.
#[derive(Resource)]
struct EditRebuildThrottle(Timer);

impl Default for EditRebuildThrottle {
    fn default() -> Self {
        Self(Timer::from_seconds(1.0, TimerMode::Repeating))
    }
}

/// Editor-wide project build preferences.
///
/// Kept at the top level of the project's settings file, where they were
/// written before the file grew other sections; see
/// [`crate::project_settings`].
#[derive(Resource, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct EditorBuildSettings {
    /// When true, the editor rebuilds the project automatically on source
    /// change. Opt-in and off by default: otherwise a rebuild fires
    /// behind the user's back and competes with a `cargo build` they may
    /// have started themselves. When off, rebuilds come from the Rebuild
    /// action or an out-of-editor `jackdaw build`.
    pub(crate) auto_build: bool,
    /// When true, the editor compiles the game once in the background as
    /// the project opens, so the first Play is instant. On by default.
    pub(crate) prebuild: bool,
    /// When true, Play saves every modified scene that has a file before it
    /// starts the game, so the game loads what the editor shows. On by
    /// default.
    pub(crate) save_before_play: bool,
}

impl Default for EditorBuildSettings {
    fn default() -> Self {
        Self {
            auto_build: false,
            prebuild: true,
            save_before_play: true,
        }
    }
}

fn sync_project_build_settings(
    project: Option<Res<crate::project::ProjectRoot>>,
    mut settings: ResMut<EditorBuildSettings>,
    mut loaded_root: Local<Option<PathBuf>>,
) {
    let Some(project) = project else {
        return;
    };
    if loaded_root.as_ref() == Some(&project.root) {
        return;
    }
    *loaded_root = Some(project.root.clone());
    *settings = crate::project_settings::load_section(
        &project.root,
        crate::project_settings::Section::TopLevel,
    );
}

pub(crate) fn persist_project_build_settings(world: &World) {
    let (Some(project), Some(settings)) = (
        world.get_resource::<crate::project::ProjectRoot>(),
        world.get_resource::<EditorBuildSettings>(),
    ) else {
        return;
    };
    crate::project_settings::store_section(
        &project.root,
        crate::project_settings::Section::TopLevel,
        settings,
    );
}

/// Start a game build for `root` unless one is already in flight.
/// Shared by the source watcher (auto-build) and the manual Rebuild
/// operator. The finished build persists `.jackdaw/schema.json`, which
/// [`watch_project_schema`] picks up to refresh the editor's types.
pub(crate) fn spawn_project_build(world: &mut World, root: &Path, load: BuildLoad) {
    if matches!(
        world.non_send::<PieSession>().builds.get(root),
        Some(PieBuildState::Running { .. })
    ) {
        return;
    }
    let spec = match crate::project_build::shim_spec_for_project(root) {
        Ok(spec) => spec,
        Err(error) => {
            report_unbuildable(world, root, &error);
            return;
        }
    };
    info!("PIE: building the game");
    let (task, progress) = spawn_game_build(root, spec, load);
    world.non_send_mut::<PieSession>().builds.insert(
        root.to_path_buf(),
        PieBuildState::Running {
            task,
            pending: Vec::new(),
            progress,
        },
    );
}

/// Put the reason `root` cannot be built in front of the user. Cargo could
/// not resolve the project, so nothing compiles, and a log line alone would
/// leave Play or Rebuild doing nothing visible.
pub(crate) fn report_unbuildable(world: &mut World, root: &Path, error: &impl std::fmt::Display) {
    let reason = error.to_string();
    warn!("PIE: cannot build {}: {reason}", root.display());
    crate::status_bar::notify_error(world, format!("Cannot build the game: {reason}"));
    if let Some(mut status) = world.get_resource_mut::<BuildStatus>() {
        status.state = crate::build_status::BuildState::Blocked { reason };
    }
}

/// While editing (not playing) and only when auto-build is enabled,
/// rebuild the game when its source changes so a newly added or
/// edited component appears in the editor without a restart. The
/// completed build persists the schema, which `watch_project_schema`
/// loads. Off by default; the manual Rebuild action is the normal path.
fn watch_project_source_for_edit(world: &mut World) {
    if *world.resource::<State<PlayState>>().get() != PlayState::Stopped {
        return;
    }
    if !world.resource::<EditorBuildSettings>().auto_build {
        return;
    }
    {
        let delta = world.resource::<Time>().delta();
        let mut throttle = world.get_resource_or_insert_with(EditRebuildThrottle::default);
        if !throttle.0.tick(delta).just_finished() {
            return;
        }
    }
    let Some(root) = project_root(world) else {
        return;
    };
    if crate::extension_build::is_extension_project(world, &root, false) {
        // Measured from when the build finished, not from the library's
        // mtime: a change that compiles to nothing new leaves the library
        // untouched and would otherwise rebuild on every tick.
        let finished = world
            .get_resource::<crate::extension_build::ExtensionBuild>()
            .and_then(crate::extension_build::ExtensionBuild::finished_at);
        if let Some(finished) = finished
            && !source_unchanged_since(&root, finished)
        {
            crate::extension_build::start_extension_build(world, &root);
        }
        return;
    }
    // Only rebuild once the project has been built and loaded at least
    // once; the initial build is driven by `prebuild_play_target`.
    let built_path = match world.non_send::<PieSession>().builds.get(&root) {
        Some(PieBuildState::Done(path)) => path.clone(),
        _ => return,
    };
    if source_is_current(&root, &built_path) {
        return;
    }
    spawn_project_build(world, &root, BuildLoad::Background);
}

/// Throttle for the schema-file poll so it stats the file about twice a
/// second rather than every frame.
#[derive(Resource)]
struct ProjectSchemaWatch {
    last_mtime: Option<std::time::SystemTime>,
    throttle: Timer,
}

impl Default for ProjectSchemaWatch {
    fn default() -> Self {
        Self {
            last_mtime: None,
            throttle: Timer::from_seconds(0.5, TimerMode::Repeating),
        }
    }
}

/// Refresh the editor's known project component types from
/// `<project>/.jackdaw/schema.json` whenever that file changes. This is
/// the single pickup path for project types: the editor's own build and
/// an out-of-editor `jackdaw build` both write the file, and only this
/// system reads it into `ProjectTypes`. On editor start it also loads a
/// schema left by a previous session, so components are known before any
/// rebuild.
/// Read `<project>/.jackdaw/schema.json` into `ProjectTypes` and publish
/// the document-only set, returning the component count when the file was
/// newer than the last read. `None` means nothing changed and callers
/// should not re-announce.
pub fn refresh_project_types(world: &mut World) -> Option<usize> {
    let root = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(|p| p.root.clone())?;
    let jackdaw_dir = root.join(".jackdaw");
    let path = jackdaw_schema::schema_path(&jackdaw_dir);
    let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
    if world
        .get_resource::<ProjectSchemaWatch>()
        .is_some_and(|watch| watch.last_mtime == Some(mtime))
    {
        return None;
    }
    let schema = jackdaw_schema::read_schema(&jackdaw_dir)?;
    let native =
        crate::project_types::native_type_paths(&world.resource::<AppTypeRegistry>().read());
    let component_count = {
        let mut project_types = world.resource_mut::<crate::project_types::ProjectTypes>();
        project_types.update(&schema, &native);
        project_types.components().count()
    };
    crate::project_types::publish_document_only_types(world);
    crate::project_definitions::register_project_definitions(world);
    if let Some(mut watch) = world.get_resource_mut::<ProjectSchemaWatch>() {
        watch.last_mtime = Some(mtime);
    }
    Some(component_count)
}

fn watch_project_schema(world: &mut World) {
    {
        let delta = world.resource::<Time>().delta();
        let mut watch = world.resource_mut::<ProjectSchemaWatch>();
        if !watch.throttle.tick(delta).just_finished() {
            return;
        }
    }
    let Some(component_count) = refresh_project_types(world) else {
        return;
    };

    // Surface the outcome in the footer so the user knows the build
    // finished and how many of their components loaded.
    world
        .resource_mut::<crate::build_status::BuildStatus>()
        .state = crate::build_status::BuildState::Ready {
        at: Instant::now(),
        components: component_count,
    };

    // Repaint the open inspector: a shape change (a field added, removed,
    // or retyped in game code) must show without a reselect.
    let selected: Vec<Entity> = world
        .resource::<crate::selection::Selection>()
        .entities
        .clone();
    for entity in selected {
        if let Ok(mut ec) = world.get_entity_mut(entity) {
            ec.insert(crate::inspector::InspectorDirty);
        }
    }
    info!("PIE: refreshed project types from schema ({component_count} project components)");
}

/// Mirror the active game or extension build into the editor's
/// `BuildStatus` so the footer shows what is compiling, and clear it once
/// no build remains.
pub(crate) fn reconcile_build_status(world: &mut World) {
    let building = world
        .non_send::<PieSession>()
        .builds
        .values()
        .find_map(|build| match build {
            PieBuildState::Running { progress, .. } => Some(Arc::clone(progress)),
            _ => None,
        })
        .or_else(|| {
            world
                .get_resource::<crate::extension_build::ExtensionBuild>()
                .and_then(crate::extension_build::ExtensionBuild::progress)
        });
    let project = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(|p| p.root.clone())
        .unwrap_or_default();
    let Some(mut status) = world.get_resource_mut::<BuildStatus>() else {
        return;
    };
    match building {
        Some(progress) => {
            status.outcome = None;
            status.failed_log = None;
            status.state = crate::build_status::BuildState::Building {
                project,
                started: Instant::now(),
                progress,
            };
        }
        None => match &status.state {
            // A build ended with no outcome latched yet: go Idle.
            // `poll_builds` (Failed) and `watch_project_schema` (Ready)
            // set the real outcome the same or next frame.
            crate::build_status::BuildState::Building { .. } => {
                status.state = crate::build_status::BuildState::Idle;
            }
            // Keep a recent success on screen, then let it fade to Idle.
            crate::build_status::BuildState::Ready { at, .. } => {
                if at.elapsed() > Duration::from_secs(6) {
                    status.state = crate::build_status::BuildState::Idle;
                }
            }
            // Sticky: a failure stays until the next build starts, and only
            // the user fixing their project clears a block.
            crate::build_status::BuildState::Failed { .. }
            | crate::build_status::BuildState::Blocked { .. }
            | crate::build_status::BuildState::Idle => {}
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A world where `prebuild_play_target` would run, over a project root
    /// naming no cargo package so nothing is actually spawned.
    fn world_ready_to_prebuild(prebuild: bool) -> World {
        let mut world = World::new();
        world.init_resource::<PiePrebuildState>();
        world.init_resource::<BuildStatus>();
        world.insert_resource(RunConfigs::default());
        world.insert_resource(EditorBuildSettings {
            auto_build: false,
            prebuild,
            save_before_play: true,
        });
        world.insert_resource(crate::project::ProjectRoot {
            root: std::env::temp_dir().join("jackdaw-prebuild-test-no-such-project"),
            config: default(),
        });
        world.insert_non_send(PieSession::default());
        world
    }

    #[test]
    fn the_setting_off_starts_no_build() {
        let mut world = world_ready_to_prebuild(false);

        prebuild_play_target(&mut world);

        assert!(world.non_send::<PieSession>().builds.is_empty());
        assert!(matches!(
            world.resource::<BuildStatus>().state,
            crate::build_status::BuildState::Idle
        ));
        assert!(world.resource::<PiePrebuildState>().attempted);
    }

    #[test]
    fn the_setting_on_reaches_the_build_attempt() {
        let mut world = world_ready_to_prebuild(true);

        prebuild_play_target(&mut world);

        assert!(matches!(
            world.resource::<BuildStatus>().state,
            crate::build_status::BuildState::Blocked { .. }
        ));
    }

    #[test]
    fn only_an_explicit_off_disables_the_prebuild() {
        assert!(env_allows_prebuild(None));
        assert!(env_allows_prebuild(Some("1")));
        assert!(env_allows_prebuild(Some("yes")));
        assert!(!env_allows_prebuild(Some("0")));
        assert!(!env_allows_prebuild(Some("false")));
        assert!(!env_allows_prebuild(Some(" ")));
    }
}
