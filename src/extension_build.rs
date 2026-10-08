//! Builds the open extension project against the SDK and loads the result
//! into the running editor.
//!
//! A game builds as its own cargo binary (see [`crate::pie`]). An editor
//! extension with no game in it instead compiles through
//! [`jackdaw_project_build::build_project_dylib`], which links the SDK the
//! editor runs on, so the library can be loaded in-process. When the SDK has
//! not been built yet, the build waits for the on-demand SDK build first.
//!
//! Each finished library is copied to a fresh path before loading. A loaded
//! library is never unloaded, and the platform loaders hand back the mapping
//! already open for a path, so loading the build output in place would keep
//! running the first build's code.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};

use crate::build_status::{BuildState, BuildStatus};
use crate::ext_build::BuildProgress;
use crate::project_build::{BuildEvent, ProjectBuildError, build_project_dylib};
use crate::sdk_paths::SdkPaths;
use crate::sdk_setup::{SdkSetup, SdkStatus};

/// Polls extension builds and loads what they produce.
pub struct ExtensionBuildPlugin;

impl Plugin for ExtensionBuildPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ExtensionBuild>()
            .add_systems(
                Update,
                advance_extension_build.run_if(in_state(crate::AppState::Editor)),
            )
            // Loading registers the extension's systems and windows, which
            // must not happen while Update's schedule is being run.
            .add_systems(
                Last,
                load_built_extension.run_if(in_state(crate::AppState::Editor)),
            )
            .add_systems(OnExit(crate::AppState::Editor), reset_extension_build);
    }
}

/// The extension build for the open project.
#[derive(Resource, Default)]
pub struct ExtensionBuild {
    /// Whether the project at this root is an extension, decided once per
    /// root and again on each explicit rebuild.
    kind: Option<(PathBuf, bool)>,
    stage: Stage,
    /// When the last successful build finished.
    finished_at: Option<SystemTime>,
    /// The library last loaded, so an unchanged rebuild is not loaded again.
    loaded: Option<LibraryStamp>,
}

#[derive(Default)]
enum Stage {
    #[default]
    Idle,
    /// The SDK is being built; the extension build starts once it is ready.
    WaitingForSdk {
        job: Job,
        progress: Arc<Mutex<BuildProgress>>,
    },
    Running {
        job: Job,
        task: Task<Result<PathBuf, String>>,
        progress: Arc<Mutex<BuildProgress>>,
    },
    /// Built and waiting for [`load_built_extension`].
    Built { job: Job, dylib: PathBuf },
}

/// What a build is for: the project it builds, and whether it is the one
/// retry after a library failed to load against the running SDK.
#[derive(Clone)]
struct Job {
    root: PathBuf,
    retry: bool,
}

/// Identifies one build's library by what a rebuild would change.
#[derive(Clone, PartialEq, Eq, Debug)]
struct LibraryStamp {
    path: PathBuf,
    modified: SystemTime,
    len: u64,
}

impl LibraryStamp {
    fn read(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        Some(Self {
            path: path.to_path_buf(),
            modified: metadata.modified().ok()?,
            len: metadata.len(),
        })
    }
}

impl Stage {
    fn job(&self) -> Option<&Job> {
        match self {
            Self::Idle => None,
            Self::WaitingForSdk { job, .. }
            | Self::Running { job, .. }
            | Self::Built { job, .. } => Some(job),
        }
    }
}

impl ExtensionBuild {
    /// The progress of a build that has not finished, for the status bar.
    pub(crate) fn progress(&self) -> Option<Arc<Mutex<BuildProgress>>> {
        match &self.stage {
            Stage::WaitingForSdk { progress, .. } | Stage::Running { progress, .. } => {
                Some(Arc::clone(progress))
            }
            Stage::Idle | Stage::Built { .. } => None,
        }
    }

    /// When the last successful build finished.
    pub(crate) fn finished_at(&self) -> Option<SystemTime> {
        self.finished_at
    }
}

/// Whether the project at `root` is an editor extension with no game in it.
/// `refresh` asks again rather than reusing the answer for this root.
pub(crate) fn is_extension_project(world: &mut World, root: &Path, refresh: bool) -> bool {
    if !refresh
        && let Some((known, is_extension)) = world
            .get_resource::<ExtensionBuild>()
            .and_then(|build| build.kind.as_ref())
        && known == root
    {
        return *is_extension;
    }
    let is_extension = jackdaw_project_build::bootstrap::project_needs_sdk(root);
    if let Some(mut build) = world.get_resource_mut::<ExtensionBuild>() {
        build.kind = Some((root.to_path_buf(), is_extension));
    }
    is_extension
}

/// Build the extension at `root` and load it once built. Does nothing while
/// a build of the same project is in flight; one for another project is
/// dropped.
pub(crate) fn start_extension_build(world: &mut World, root: &Path) {
    let Some(build) = world.get_resource::<ExtensionBuild>() else {
        return;
    };
    if build.stage.job().is_some_and(|job| job.root == root) {
        return;
    }
    begin(
        world,
        Job {
            root: root.to_path_buf(),
            retry: false,
        },
    );
}

fn begin(world: &mut World, job: Job) {
    let progress = Arc::new(Mutex::new(BuildProgress::default()));
    let sdk_ready = world
        .get_resource_mut::<SdkSetup>()
        .is_none_or(|mut setup| sdk_is_ready(&mut setup));
    let stage = if sdk_ready {
        spawn_build(job, progress)
    } else {
        if let Ok(mut log) = progress.lock() {
            log.push_log(format!(
                "building the extension SDK first; this usually takes {}",
                jackdaw_project_build::bootstrap::SDK_BUILD_ESTIMATE
            ));
        }
        if let Some(mut setup) = world.get_resource_mut::<SdkSetup>()
            && !setup.build_pending()
        {
            setup.request_build();
        }
        Stage::WaitingForSdk { job, progress }
    };
    world.resource_mut::<ExtensionBuild>().stage = stage;
}

/// Whether an extension can build now. An SDK that cannot be built here
/// counts as ready: the build itself then reports what is wrong with it.
fn sdk_is_ready(setup: &mut SdkSetup) -> bool {
    matches!(setup.status(), SdkStatus::Ready | SdkStatus::Unavailable)
}

fn spawn_build(job: Job, progress: Arc<Mutex<BuildProgress>>) -> Stage {
    let spec = match crate::project_build::shim_spec_for_project(&job.root) {
        Ok(spec) => spec,
        Err(error) => {
            let task = AsyncComputeTaskPool::get().spawn(async move { Err(error.to_string()) });
            return Stage::Running {
                job,
                task,
                progress,
            };
        }
    };
    let jackdaw_dir = job.root.join(".jackdaw");
    let sink = Arc::clone(&progress);
    let task = AsyncComputeTaskPool::get().spawn(async move {
        if let Ok(mut p) = sink.lock() {
            p.push_log("building the extension against the SDK".to_string());
        }
        let report_sink = Arc::clone(&sink);
        let mut report = move |event: BuildEvent| {
            let Ok(mut p) = report_sink.lock() else {
                return;
            };
            match event {
                BuildEvent::Compiled {
                    crate_name,
                    done,
                    fresh,
                } => {
                    p.artifacts_done = done;
                    if !fresh {
                        p.current_crate = Some(crate_name.clone());
                        p.push_log(format!("Compiling {crate_name}"));
                    }
                }
                BuildEvent::Log(line) => p.push_log(line),
            }
        };
        let sdk = SdkPaths::compute();
        let dev_workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let dev_workspace = dev_workspace.exists().then_some(dev_workspace);
        let result = build_project_dylib(
            &spec,
            &jackdaw_dir,
            &sdk,
            dev_workspace.as_deref(),
            &mut report,
        );
        if let Ok(mut p) = sink.lock() {
            p.finished = true;
        }
        result.map(|build| build.dylib).map_err(|error| {
            let detail = match &error {
                ProjectBuildError::Compile { log } => log.clone(),
                other => other.to_string(),
            };
            if let Ok(mut p) = sink.lock() {
                p.push_log(format!("build failed: {error}"));
            }
            crate::pie::failure_summary(&error, &detail)
        })
    });
    Stage::Running {
        job,
        task,
        progress,
    }
}

/// Start a build whose SDK has become ready, and settle finished builds.
fn advance_extension_build(world: &mut World) {
    let stage = std::mem::take(&mut world.resource_mut::<ExtensionBuild>().stage);
    let stage = match stage {
        Stage::WaitingForSdk { job, progress } => {
            let Some(mut setup) = world.get_resource_mut::<SdkSetup>() else {
                world.resource_mut::<ExtensionBuild>().stage = spawn_build(job, progress);
                return;
            };
            if sdk_is_ready(&mut setup) {
                spawn_build(job, progress)
            } else if setup.build_pending() {
                if let (Ok(mut p), (current, done)) = (progress.lock(), setup.compiling()) {
                    p.current_crate = current.map(str::to_string);
                    p.artifacts_done = done;
                }
                Stage::WaitingForSdk { job, progress }
            } else {
                let reason = match setup.status() {
                    SdkStatus::Failed { error, .. } => error.clone(),
                    _ => "the SDK build stopped".to_string(),
                };
                fail(
                    world,
                    &progress,
                    format!("the extension SDK did not build: {reason}"),
                );
                Stage::Idle
            }
        }
        Stage::Running {
            job,
            mut task,
            progress,
        } => match future::block_on(future::poll_once(&mut task)) {
            None => Stage::Running {
                job,
                task,
                progress,
            },
            Some(Ok(dylib)) => {
                world.resource_mut::<ExtensionBuild>().finished_at = Some(SystemTime::now());
                Stage::Built { job, dylib }
            }
            Some(Err(error)) => {
                fail(world, &progress, error);
                Stage::Idle
            }
        },
        other => other,
    };
    world.resource_mut::<ExtensionBuild>().stage = stage;
}

/// Settle a failed build. Auto-build then waits for a manual rebuild rather
/// than retrying the same source every second.
fn fail(world: &mut World, progress: &Mutex<BuildProgress>, message: String) {
    error!("Extension build failed: {message}");
    world.resource_mut::<ExtensionBuild>().finished_at = None;
    if let Ok(mut p) = progress.lock() {
        p.finished = true;
    }
    report_failure(world, message);
}

fn report_failure(world: &mut World, message: String) {
    world.resource_mut::<BuildStatus>().state = BuildState::Failed { at: Instant::now() };
    crate::status_bar::notify_error(world, message);
}

fn report_ready(world: &mut World) {
    let components = world
        .get_resource::<crate::project_types::ProjectTypes>()
        .map_or(0, |types| types.components().count());
    world.resource_mut::<BuildStatus>().state = BuildState::Ready {
        at: Instant::now(),
        components,
    };
}

/// Load a finished build into the editor, replacing the copy loaded before.
/// A build of a project that is no longer open, or one whose library is
/// unchanged since the last load, is not loaded.
fn load_built_extension(world: &mut World) {
    let (job, dylib) = {
        let mut build = world.resource_mut::<ExtensionBuild>();
        match std::mem::take(&mut build.stage) {
            Stage::Built { job, dylib } => (job, dylib),
            other => {
                build.stage = other;
                return;
            }
        }
    };
    let open = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(|project| project.root.clone());
    if open.as_ref() != Some(&job.root) {
        info!(
            "Dropping the extension build of {}, which is no longer open",
            job.root.display()
        );
        return;
    }
    let stamp = LibraryStamp::read(&dylib);
    if stamp.is_some() && world.resource::<ExtensionBuild>().loaded == stamp {
        info!("Extension unchanged since it was loaded; not loading it again");
        report_ready(world);
        return;
    }

    let staged = stage_for_load(&dylib, &job.root.join(".jackdaw").join("loaded"));
    let staged = match staged {
        Ok(staged) => staged,
        Err(error) => {
            report_failure(
                world,
                format!("Could not stage {} for loading: {error}", dylib.display()),
            );
            return;
        }
    };
    match crate::extensions_dialog::handle_install_from_path(world, staged) {
        Ok(id) => {
            info!("Loaded extension `{id}` from {}", dylib.display());
            world.resource_mut::<ExtensionBuild>().loaded = stamp;
            report_ready(world);
        }
        Err(error) if error.is_symbol_mismatch() && !job.retry => {
            warn!("Extension does not match the running SDK, rebuilding it clean: {error}");
            if let Some(cache) = build_cache_of(&job.root, &dylib) {
                let _ = std::fs::remove_dir_all(cache);
            }
            begin(
                world,
                Job {
                    root: job.root,
                    retry: true,
                },
            );
        }
        Err(error) if error.is_symbol_mismatch() => {
            report_failure(
                world,
                format!(
                    "The extension still does not match the running editor after a clean \
                     rebuild: {error}"
                ),
            );
        }
        Err(error) => {
            error!("Extension load failed: {error}");
            report_failure(world, format!("Extension did not load: {error}"));
        }
    }
}

/// The keyed target directory `dylib` was built in, when it is one of the
/// project's own: `<root>/.jackdaw/target/<key>/<triple>/debug/<library>`.
fn build_cache_of(root: &Path, dylib: &Path) -> Option<PathBuf> {
    let cache = dylib.ancestors().nth(3)?;
    (cache.parent()? == root.join(".jackdaw").join("target")).then(|| cache.to_path_buf())
}

fn reset_extension_build(mut build: ResMut<ExtensionBuild>) {
    *build = ExtensionBuild::default();
}

/// Copy `dylib` into `dir` under a name no earlier load used, removing the
/// copies earlier loads left there. A copy that is still mapped and cannot
/// be removed is left for a later load to remove.
pub(crate) fn stage_for_load(dylib: &Path, dir: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    for entry in std::fs::read_dir(dir)?.flatten() {
        let _ = std::fs::remove_file(entry.path());
    }
    let stem = dylib
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("extension");
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let staged = dir.join(format!("{stem}-{stamp}{}", std::env::consts::DLL_SUFFIX));
    std::fs::copy(dylib, &staged)?;
    Ok(staged)
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "jackdaw-extension-build-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn job(root: &Path) -> Job {
        Job {
            root: root.to_path_buf(),
            retry: false,
        }
    }

    fn editor_world(open: &Path) -> World {
        let mut world = World::new();
        world.init_resource::<ExtensionBuild>();
        world.init_resource::<BuildStatus>();
        world.insert_resource(crate::project::ProjectRoot {
            root: open.to_path_buf(),
            config: default(),
        });
        world
    }

    #[test]
    fn each_load_gets_its_own_copy_and_replaces_the_last() {
        let dir = scratch("stage");
        let dylib = dir.join(format!("libjackdaw_shim{}", std::env::consts::DLL_SUFFIX));
        std::fs::write(&dylib, b"first").unwrap();
        let loaded = dir.join("loaded");

        let first = stage_for_load(&dylib, &loaded).unwrap();
        std::fs::write(&dylib, b"second").unwrap();
        let second = stage_for_load(&dylib, &loaded).unwrap();

        assert_ne!(first, second);
        assert_ne!(second, dylib);
        assert_eq!(std::fs::read(&second).unwrap(), b"second");
        assert!(!first.exists());
        assert_eq!(std::fs::read_dir(&loaded).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_game_project_is_not_an_extension() {
        let dir = scratch("game");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub struct GamePlugin;\n").unwrap();
        let mut world = World::new();
        world.init_resource::<ExtensionBuild>();

        assert!(!is_extension_project(&mut world, &dir, false));

        std::fs::write(
            dir.join("src/lib.rs"),
            "#[derive(Default)]\npub struct Tools;\nimpl JackdawExtension for Tools {}\n",
        )
        .unwrap();
        assert!(!is_extension_project(&mut world, &dir, false));
        assert!(is_extension_project(&mut world, &dir, true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_extension_without_an_sdk_waits_for_its_build() {
        let mut world = World::new();
        world.init_resource::<ExtensionBuild>();
        world.insert_resource(SdkSetup::with_status(SdkStatus::NotBuilt));

        start_extension_build(&mut world, Path::new("project"));

        assert!(matches!(
            world.resource::<ExtensionBuild>().stage,
            Stage::WaitingForSdk { .. }
        ));
        assert!(world.resource::<SdkSetup>().build_pending());
        assert!(world.resource::<ExtensionBuild>().progress().is_some());
    }

    #[test]
    fn a_build_for_another_project_is_replaced_not_kept() {
        let mut world = World::new();
        world.init_resource::<ExtensionBuild>();
        world.insert_resource(SdkSetup::with_status(SdkStatus::NotBuilt));

        start_extension_build(&mut world, Path::new("first"));
        start_extension_build(&mut world, Path::new("second"));

        let build = world.resource::<ExtensionBuild>();
        assert_eq!(
            build.stage.job().map(|job| job.root.clone()),
            Some(PathBuf::from("second"))
        );
    }

    #[test]
    fn a_failed_sdk_build_fails_the_waiting_extension_build() {
        let mut world = editor_world(Path::new("project"));
        world.insert_resource(SdkSetup::with_status(SdkStatus::Failed {
            error: "linker missing".to_string(),
            log: None,
        }));
        world.resource_mut::<ExtensionBuild>().stage = Stage::WaitingForSdk {
            job: job(Path::new("project")),
            progress: Arc::default(),
        };

        advance_extension_build(&mut world);

        assert!(world.resource::<ExtensionBuild>().stage.job().is_none());
        assert!(matches!(
            world.resource::<BuildStatus>().state,
            BuildState::Failed { .. }
        ));
    }

    #[test]
    fn a_build_of_a_project_no_longer_open_is_not_loaded() {
        let dir = scratch("switched");
        let dylib = dir.join("libjackdaw_shim.so");
        std::fs::write(&dylib, b"library").unwrap();
        let mut world = editor_world(&dir.join("now-open"));
        world.resource_mut::<ExtensionBuild>().stage = Stage::Built {
            job: job(&dir.join("closed")),
            dylib,
        };

        load_built_extension(&mut world);

        assert!(!dir.join("closed/.jackdaw/loaded").exists());
        assert!(!dir.join("now-open/.jackdaw/loaded").exists());
        assert!(matches!(
            world.resource::<BuildStatus>().state,
            BuildState::Idle
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unchanged_library_is_not_loaded_again() {
        let dir = scratch("unchanged");
        let dylib = dir.join("libjackdaw_shim.so");
        std::fs::write(&dylib, b"library").unwrap();
        let mut world = editor_world(&dir);
        world.resource_mut::<ExtensionBuild>().loaded = LibraryStamp::read(&dylib);
        world.resource_mut::<ExtensionBuild>().stage = Stage::Built {
            job: job(&dir),
            dylib,
        };

        load_built_extension(&mut world);

        assert!(!dir.join(".jackdaw/loaded").exists());
        assert!(matches!(
            world.resource::<BuildStatus>().state,
            BuildState::Ready { .. }
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_build_still_running_is_left_alone_by_the_loader() {
        let mut world = editor_world(Path::new("project"));
        world.resource_mut::<ExtensionBuild>().stage = Stage::WaitingForSdk {
            job: job(Path::new("project")),
            progress: Arc::default(),
        };

        load_built_extension(&mut world);

        assert!(world.resource::<ExtensionBuild>().progress().is_some());
    }

    #[test]
    fn leaving_the_editor_forgets_the_build() {
        let mut world = World::new();
        world.init_resource::<ExtensionBuild>();
        world.resource_mut::<ExtensionBuild>().stage = Stage::Built {
            job: job(Path::new("project")),
            dylib: PathBuf::from("lib.so"),
        };
        world.resource_mut::<ExtensionBuild>().finished_at = Some(SystemTime::now());

        world.run_system_once(reset_extension_build).unwrap();

        let build = world.resource::<ExtensionBuild>();
        assert!(build.stage.job().is_none());
        assert!(build.finished_at().is_none());
    }

    #[test]
    fn only_the_projects_own_build_cache_is_cleaned() {
        let root = Path::new("/work/myext");
        let dylib = root.join(".jackdaw/target/0123abcd/x86_64-unknown-linux-gnu/debug/lib.so");
        assert_eq!(
            build_cache_of(root, &dylib),
            Some(root.join(".jackdaw/target/0123abcd"))
        );
        assert_eq!(
            build_cache_of(root, Path::new("/elsewhere/a/b/c/lib.so")),
            None
        );
    }
}
