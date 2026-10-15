//! The launcher: opening projects with the editor their pin names, the
//! Open with dialog, editor installs through `jd`, new projects and the
//! Extensions page, driven through its operators and through clicks.

use crate::util;

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use bevy::camera::RenderTarget;
use bevy::picking::{
    backend::HitData,
    events::{Pointer, PointerClick, PointerPress, PointerRelease},
    pointer::{Location, PointerButton, PointerId},
};
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, WindowRef};
use jackdaw::launcher::{
    EditorHandoff, JdJobs, JdProgram, JobKind, JobStatus, LauncherDialog, LauncherPage,
    ProjectFilter, ProjectListView,
};
use jackdaw_api::prelude::*;
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_releases::{Channel, Install, Installs, Layout};
use jackdaw_scene_types::PropertyValue;

/// A config directory of this process's own: the recent list, the
/// installed-editor registry and the extension list all live under it.
static CONFIG: LazyLock<PathBuf> = LazyLock::new(|| {
    let dir = std::env::temp_dir().join(format!("jackdaw_launcher_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a config directory for the test");
    // SAFETY: set once, before this process builds an app, and never
    // written again.
    unsafe {
        std::env::set_var(jackdaw_env::paths::CONFIG_DIR_VAR, &dir);
        std::env::set_var(jackdaw::pie::ENV_PIE_PREBUILD, "0");
    }
    dir
});

/// Point every per-user file at [`CONFIG`] before anything reads or
/// writes one, and check that it took.
fn isolated() {
    LazyLock::force(&CONFIG);
    assert_eq!(
        jackdaw_env::paths::config_dir().as_deref(),
        Some(CONFIG.as_path()),
        "the test writes only under its own config directory"
    );
}

static HANDOFFS: Mutex<Vec<(PathBuf, PathBuf)>> = Mutex::new(Vec::new());

fn record_handoff(executable: &Path, project: &Path) {
    HANDOFFS
        .lock()
        .unwrap()
        .push((executable.to_path_buf(), project.to_path_buf()));
}

fn launcher_app() -> App {
    isolated();
    let mut app = util::editor_test_app();
    app.insert_resource(JdProgram(PathBuf::from("/nonexistent/jd")));
    app.insert_resource(EditorHandoff(record_handoff));
    app
}

fn layout() -> Layout {
    isolated();
    Layout::from_env().expect("a launcher directory")
}

fn write_installs(installs: &Installs) {
    std::fs::create_dir_all(layout().root()).unwrap();
    layout().save_installs(installs).unwrap();
}

fn stable_install(version: &str, dir: &Path) -> Install {
    let path = dir.join(format!("editor-{version}"));
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(
        path.join(jackdaw_releases::installs::EDITOR_EXECUTABLE),
        b"",
    )
    .unwrap();
    Install {
        version: version.into(),
        channel: Channel::Stable,
        bevy: jackdaw_releases::version::line_of(version).unwrap(),
        bevy_rev: None,
        path,
        managed: true,
        modules: Vec::new(),
        known_good: false,
    }
}

/// A project folder whose `jackdaw.toml` pins `version`, listed in the
/// recent projects.
fn pinned_project(dir: &Path, name: &str, version: &str) -> PathBuf {
    isolated();
    let root = dir.join(name);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n"),
    )
    .unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    let bevy = jackdaw_releases::version::line_of(version).unwrap();
    std::fs::write(
        root.join("jackdaw.toml"),
        format!("[jackdaw]\nversion = \"{version}\"\nbevy = \"{bevy}\"\nchannel = \"stable\"\n"),
    )
    .unwrap();
    jackdaw::project::touch_recent(&root, name);
    root
}

fn refresh(app: &mut App) {
    jackdaw::launcher::refresh_launcher(app.world_mut());
    app.update();
}

/// Run operator `id` and let its queued work happen.
fn run(app: &mut App, id: &str, params: &[(&str, PropertyValue)]) {
    let mut call = app.world_mut().operator(id.to_string());
    for (key, value) in params {
        call = call.param(key.to_string(), value.clone());
    }
    let result = call.call().expect("the operator runs");
    assert!(
        matches!(result, OperatorResult::Finished),
        "{id} finished: {result:?}"
    );
    app.update();
}

fn path_value(path: &Path) -> PropertyValue {
    path.to_string_lossy().into_owned().into()
}

/// Click `entity` the way a user does: press, click and release, the
/// pointer events a button turns into `Activate`.
fn click(app: &mut App, entity: Entity) {
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(app.world())
        .expect("headless apps still have a primary window");
    let target = RenderTarget::Window(WindowRef::Primary)
        .normalize(Some(window))
        .expect("the primary window normalizes");
    let pointer = || {
        Pointer::new(
            PointerId::Mouse,
            Location {
                target: target.clone(),
                position: Vec2::ZERO,
            },
        )
    };
    let hit = || HitData::new(Entity::PLACEHOLDER, 0.0, None, None);
    app.world_mut().trigger(PointerPress {
        entity,
        pointer: pointer(),
        button: PointerButton::Primary,
        hit: hit(),
        count: 1,
    });
    app.update();
    app.world_mut().trigger(PointerClick {
        entity,
        pointer: pointer(),
        button: PointerButton::Primary,
        hit: hit(),
        duration: Duration::ZERO,
        count: 1,
    });
    app.world_mut().trigger(PointerRelease {
        entity,
        pointer: pointer(),
        button: PointerButton::Primary,
        hit: hit(),
    });
    app.update();
    app.update();
}

/// The button that runs `id` with `key` set to `value`.
fn button(app: &mut App, id: &str, key: &str, value: &str) -> Entity {
    for _ in 0..4 {
        app.update();
    }
    app.world_mut()
        .query::<(Entity, &ButtonOperatorCall)>()
        .iter(app.world())
        .find(|(_, call)| {
            call.id == id
                && call
                    .params
                    .iter()
                    .any(|(k, v)| k == key && v == &PropertyValue::from(value.to_string()))
        })
        .map(|(entity, _)| entity)
        .unwrap_or_else(|| {
            let shown: Vec<String> = app
                .world_mut()
                .query::<&ButtonOperatorCall>()
                .iter(app.world())
                .map(ToString::to_string)
                .collect();
            let projects = app.world().resource::<jackdaw::launcher::Launcher>().projects.len();
            let texts: Vec<String> = app
                .world_mut()
                .query::<&Text>()
                .iter(app.world())
                .map(|text| text.0.clone())
                .collect();
            panic!("no button runs {id} with {key}={value}; shown: {shown:?} projects {projects} texts {texts:?}")
        })
}

fn jobs(app: &App) -> Vec<(JobKind, Vec<String>, JobStatus)> {
    app.world()
        .resource::<JdJobs>()
        .jobs
        .iter()
        .map(|job| (job.kind.clone(), job.args.clone(), job.status.clone()))
        .collect()
}

#[test]
fn clicking_a_page_in_the_sidebar_and_a_filter_shows_them() {
    let mut app = launcher_app();
    let editors = button(&mut app, "launcher.show_page", "page", "editors");
    click(&mut app, editors);
    assert_eq!(
        app.world().resource::<LauncherPage>(),
        &LauncherPage::Editors
    );

    let projects = button(&mut app, "launcher.show_page", "page", "projects");
    click(&mut app, projects);
    let needs = button(
        &mut app,
        "launcher.project_filter",
        "filter",
        "needs-editor",
    );
    click(&mut app, needs);
    assert_eq!(
        app.world().resource::<ProjectListView>().filter,
        ProjectFilter::NeedsEditor
    );
}

#[test]
fn clicking_a_project_pinned_to_another_install_hands_it_to_that_editor() {
    let dir = tempfile::tempdir().unwrap();
    let install = stable_install("0.30.1", dir.path());
    let mut installs = Installs::default();
    installs.add(install.clone());
    write_installs(&installs);
    let project = pinned_project(dir.path(), "pinned-elsewhere", "0.30.0");

    let mut app = launcher_app();
    refresh(&mut app);
    let row = button(
        &mut app,
        "launcher.open_project",
        "path",
        &project.to_string_lossy(),
    );
    click(&mut app, row);

    let handed = HANDOFFS.lock().unwrap().clone();
    assert!(
        handed.contains(&(install.executable(), project.clone())),
        "the newest install of the pinned line opens it: {handed:?}"
    );
    assert!(
        app.world()
            .get_resource::<jackdaw::project::ProjectRoot>()
            .is_none(),
        "and it does not open here"
    );
}

#[test]
fn a_project_whose_editor_is_missing_asks_and_installing_runs_jd() {
    let dir = tempfile::tempdir().unwrap();
    write_installs(&Installs::default());
    let project = pinned_project(dir.path(), "needs-old-editor", "0.17.3");

    let mut app = launcher_app();
    refresh(&mut app);
    run(
        &mut app,
        "launcher.open_project",
        &[("path", path_value(&project))],
    );
    let LauncherDialog::OpenWith(form) = app.world().resource::<LauncherDialog>().clone() else {
        panic!("the open-with dialog asks what to do");
    };
    assert_eq!(form.project, project);

    run(
        &mut app,
        "launcher.open_with_choice",
        &[("choice", "install".to_string().into())],
    );
    run(&mut app, "launcher.open_with_confirm", &[]);
    let jobs = jobs(&app);
    let (kind, args, status) = jobs.last().expect("an install was started");
    assert_eq!(
        kind,
        &JobKind::Install {
            query: "0.17".into()
        }
    );
    assert_eq!(args, &["editor", "install", "0.17", "--json"]);
    assert!(
        matches!(status, JobStatus::Failed(reason) if reason.contains("/nonexistent/jd")),
        "a jd that cannot run is reported on the job: {status:?}"
    );
    assert_eq!(
        app.world().resource::<LauncherDialog>(),
        &LauncherDialog::Downloads,
        "and Downloads shows it"
    );
}

#[test]
fn opening_a_copy_moves_the_copy_to_this_editor_and_leaves_the_original() {
    let dir = tempfile::tempdir().unwrap();
    write_installs(&Installs::default());
    let project = pinned_project(dir.path(), "older-project", "0.17.3");

    let mut app = launcher_app();
    refresh(&mut app);
    run(
        &mut app,
        "launcher.open_with",
        &[("path", path_value(&project))],
    );
    run(
        &mut app,
        "launcher.open_with_choice",
        &[
            ("choice", "copy".to_string().into()),
            ("version", jackdaw_project_build::VERSION.to_string().into()),
        ],
    );
    run(&mut app, "launcher.open_with_confirm", &[]);

    let line = jackdaw_project_build::BEVY_VERSION;
    let copy = dir.path().join(format!("older-project-{line}"));
    util::update_until(
        &mut app,
        "the copy to open",
        Duration::from_secs(60),
        |app| {
            app.world()
                .get_resource::<jackdaw::project::ProjectRoot>()
                .is_some_and(|root| root.root == copy)
        },
    );
    let copied = std::fs::read_to_string(copy.join("jackdaw.toml")).unwrap();
    assert!(
        copied.contains(&format!("version = \"{}\"", jackdaw_project_build::VERSION)),
        "{copied}"
    );
    let original = std::fs::read_to_string(project.join("jackdaw.toml")).unwrap();
    assert!(original.contains("0.17.3"), "{original}");
}

#[test]
fn uninstalling_an_editor_projects_open_with_asks_first_then_forces() {
    let dir = tempfile::tempdir().unwrap();
    let install = stable_install("0.30.1", dir.path());
    let project = pinned_project(dir.path(), "uses-it", "0.30.1");
    let mut installs = Installs::default();
    installs.add(install);
    installs.record_project(&project, "0.30.1");
    write_installs(&installs);

    let mut app = launcher_app();
    refresh(&mut app);
    run(
        &mut app,
        "launcher.uninstall_editor",
        &[("version", "0.30.1".to_string().into())],
    );
    let LauncherDialog::Uninstall { version, projects } =
        app.world().resource::<LauncherDialog>().clone()
    else {
        panic!("uninstalling an editor a project uses asks first");
    };
    assert_eq!(version, "0.30.1");
    assert_eq!(projects, vec![project]);
    assert!(
        jobs(&app).is_empty(),
        "nothing is removed before the answer"
    );

    let anyway = button(&mut app, "launcher.uninstall_editor", "version", "0.30.1");
    click(&mut app, anyway);
    let (kind, args, _) = jobs(&app).pop().expect("the removal ran");
    assert_eq!(
        kind,
        JobKind::Remove {
            version: "0.30.1".into()
        }
    );
    assert_eq!(args, &["editor", "remove", "0.30.1", "--force", "--json"]);
}

#[test]
fn the_new_project_page_creates_the_chosen_template_pinned_to_this_editor() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = launcher_app();
    run(
        &mut app,
        "launcher.new_project",
        &[("template", "2d".to_string().into())],
    );
    assert_eq!(
        app.world().resource::<LauncherPage>(),
        &LauncherPage::NewProject
    );
    run(
        &mut app,
        "launcher.new_project_module",
        &[
            ("id", "navmesh".to_string().into()),
            ("enabled", true.into()),
        ],
    );
    let modules = app
        .world()
        .resource::<jackdaw::launcher::NewProjectForm>()
        .modules
        .clone();
    assert!(modules.contains("navmesh"), "navmesh is on: {modules:?}");
    let refused = app
        .world_mut()
        .operator("launcher.new_project_module")
        .param("id", "physics")
        .param("enabled", true)
        .call()
        .expect("the operator runs");
    assert!(matches!(refused, OperatorResult::Cancelled));
    assert!(
        !app.world()
            .resource::<jackdaw::launcher::NewProjectForm>()
            .modules
            .contains("physics"),
        "physics is not compatible with this Bevy, so it stays off"
    );
    run(
        &mut app,
        "launcher.new_project_git",
        &[("enabled", false.into())],
    );
    run(
        &mut app,
        "launcher.new_project_create",
        &[
            ("name", "flat_game".to_string().into()),
            ("location", path_value(dir.path())),
        ],
    );
    let project = dir
        .path()
        .join(jackdaw::scaffold::validated_project_name("flat_game").expect("a usable name"));
    util::update_until(
        &mut app,
        "the new project to open",
        Duration::from_secs(60),
        |app| {
            app.world()
                .get_resource::<jackdaw::project::ProjectRoot>()
                .is_some_and(|root| root.root == project)
        },
    );
    util::update_until(
        &mut app,
        "the 2D starter scene's sprite",
        Duration::from_secs(30),
        |app| {
            app.world_mut()
                .query_filtered::<(), With<Sprite>>()
                .iter(app.world())
                .next()
                .is_some()
        },
    );
    let manifest = std::fs::read_to_string(project.join("jackdaw.toml")).unwrap();
    assert!(
        manifest.contains(&format!("version = \"{}\"", jackdaw_project_build::VERSION)),
        "pinned to this editor: {manifest}"
    );
    assert!(manifest.contains("navmesh = true"), "{manifest}");
    assert!(
        !project.join(".git").exists(),
        "no repository was asked for"
    );
}

#[test]
fn the_extensions_page_lists_optional_extensions_and_applies_module_changes() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = launcher_app();
    let project = dir.path().join("with-overrides");
    jackdaw::new_project::scaffold_project_with(
        "with-overrides",
        dir.path(),
        jackdaw::scaffold::TemplateKind::Game,
        &jackdaw_project_build::modules::ModuleSet::new(),
        jackdaw::scaffold::StarterScene::ThreeD,
        false,
    )
    .unwrap();
    let manifest_path = project.join("jackdaw.toml");
    let text = std::fs::read_to_string(&manifest_path).unwrap();
    let overrides = std::collections::BTreeMap::from([("acme.dialogue".to_string(), true)]);
    std::fs::write(
        &manifest_path,
        jackdaw_project_build::project_manifest::with_extensions(&text, &overrides).unwrap(),
    )
    .unwrap();

    run(
        &mut app,
        "launcher.extensions",
        &[("path", path_value(&project))],
    );
    for _ in 0..4 {
        app.update();
    }
    let texts: Vec<String> = app
        .world_mut()
        .query::<&Text>()
        .iter(app.world())
        .map(|text| text.0.clone())
        .collect();
    assert!(
        texts.iter().any(|text| text == "acme.dialogue")
            && texts
                .iter()
                .any(|text| text == "not installed in this editor"),
        "an extension the project names but this editor lacks is shown as missing: {texts:?}"
    );
    assert!(
        !texts
            .iter()
            .any(|text| text == "Inspector" || text == "Viewport"),
        "the editor's own windows are not listed as extensions: {texts:?}"
    );

    run(
        &mut app,
        "launcher.extensions_toggle",
        &[
            ("kind", "extension".to_string().into()),
            ("id", "acme.dialogue".to_string().into()),
            ("enabled", false.into()),
        ],
    );
    for id in ["physics", "navmesh"] {
        run(
            &mut app,
            "launcher.extensions_toggle",
            &[
                ("kind", "module".to_string().into()),
                ("id", id.to_string().into()),
                ("enabled", true.into()),
            ],
        );
    }
    let pending = app
        .world()
        .resource::<jackdaw::launcher::ExtensionsView>()
        .pending
        .clone();
    assert_eq!(
        pending
            .keys()
            .map(jackdaw::launcher::ExtensionItem::id)
            .collect::<Vec<_>>(),
        ["navmesh"],
        "a missing extension and an incompatible module cannot be toggled; the module change waits"
    );
    let untouched = std::fs::read_to_string(&manifest_path).unwrap();
    assert!(
        untouched.contains("navmesh = false"),
        "nothing is written before Apply"
    );

    run(&mut app, "launcher.extensions_apply", &[]);
    let manifest = jackdaw_project_build::project_manifest::ProjectManifest::read(&project);
    assert_eq!(
        manifest
            .modules
            .as_ref()
            .and_then(|table| table.get("navmesh")),
        Some(&true)
    );
    assert!(
        app.world()
            .resource::<jackdaw::launcher::ExtensionsView>()
            .pending
            .is_empty()
    );
}

#[test]
fn an_idle_launcher_spawns_and_rebuilds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = launcher_app();
    pinned_project(dir.path(), "idle-project", "0.30.0");
    refresh(&mut app);
    for _ in 0..10 {
        app.update();
    }
    let nodes = |app: &mut App| -> std::collections::BTreeSet<Entity> {
        app.world_mut()
            .query_filtered::<Entity, With<Node>>()
            .iter(app.world())
            .collect()
    };
    let before = nodes(&mut app);
    for _ in 0..30 {
        app.update();
    }
    assert_eq!(before, nodes(&mut app), "the idle launcher keeps its nodes");
}
