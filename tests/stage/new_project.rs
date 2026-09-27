//! Creating a project from the launcher's New Project dialog.

use crate::util;

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use bevy::camera::RenderTarget;
use bevy::picking::{
    backend::HitData,
    events::{Click, Pointer},
    pointer::{Location, PointerButton, PointerId},
};
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, WindowRef};
use jackdaw::project_select::NewProjectCreateButton;
use jackdaw::scaffold::TemplateKind;
use jackdaw_feathers::text_edit::TextEditValue;

/// A config directory of this process's own, with the background game build
/// switched off. Creating a project remembers where it was made, and opening
/// one would otherwise start a cargo build of it; neither belongs to whoever
/// runs the suite. Set before the app is built, and never changed after.
static ISOLATED: LazyLock<PathBuf> = LazyLock::new(|| {
    let dir = std::env::temp_dir().join(format!("jackdaw_new_project_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a config directory for the test");
    // SAFETY: set once, before this process builds an app, and never
    // written again.
    unsafe {
        std::env::set_var(jackdaw_env::paths::CONFIG_DIR_VAR, &dir);
        std::env::set_var(jackdaw::pie::ENV_PIE_PREBUILD, "0");
    }
    dir
});

/// The dialog opens where the last project was made.
fn remember_location(location: &Path) {
    std::fs::write(
        ISOLATED.join("last_new_project_location"),
        location.to_string_lossy().as_bytes(),
    )
    .expect("the location is remembered");
}

fn open_project(app: &App) -> Option<PathBuf> {
    app.world()
        .get_resource::<jackdaw::project::ProjectRoot>()
        .map(|root| root.root.clone())
}

/// Click `button` the way a user does: the `Pointer<Click>` its observer
/// watches for.
fn click(app: &mut App, button: Entity) {
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(app.world())
        .expect("headless apps still have a primary window");
    let target = RenderTarget::Window(WindowRef::Primary)
        .normalize(Some(window))
        .expect("the primary window normalizes");
    app.world_mut().trigger(Pointer::new(
        PointerId::Mouse,
        Location {
            target,
            position: Vec2::ZERO,
        },
        Click {
            button: PointerButton::Primary,
            hit: HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
            duration: Duration::ZERO,
            count: 1,
        },
        button,
    ));
    app.update();
}

fn create_button(app: &mut App) -> Entity {
    app.world_mut()
        .query_filtered::<Entity, With<NewProjectCreateButton>>()
        .single(app.world())
        .expect("the dialog shows one Create button")
}

fn open_after(app: &mut App, project: &Path) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while open_project(app).as_deref() != Some(project) && Instant::now() < deadline {
        app.update();
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn create_in_the_new_game_dialog_makes_the_offered_project_and_opens_it() {
    let projects = tempfile::tempdir().expect("tempdir");
    remember_location(projects.path());
    let mut app = util::editor_test_app();

    jackdaw::project_select::open_new_project_modal(app.world_mut(), TemplateKind::Game);
    for _ in 0..4 {
        app.update();
    }
    assert!(
        app.world_mut()
            .query::<&TextEditValue>()
            .iter(app.world())
            .any(|value| value.0 == "my_game"),
        "the name field offers a name before anything is typed"
    );
    let create = create_button(&mut app);
    click(&mut app, create);

    let offered = jackdaw::scaffold::validated_project_name("my_game").expect("a usable name");
    let project = projects.path().join(offered);
    open_after(&mut app, &project);

    assert_eq!(
        open_project(&app).as_deref(),
        Some(project.as_path()),
        "the editor opened the project the dialog offered to make"
    );
    for file in ["Cargo.toml", "jackdaw.toml", "assets/scene.bsn"] {
        assert!(project.join(file).is_file(), "the new project holds {file}");
    }
    app.update();
    assert_eq!(
        app.world().resource::<State<jackdaw::AppState>>().get(),
        &jackdaw::AppState::Editor,
        "and the launcher handed over to the editor"
    );
}

#[test]
fn a_typed_name_names_the_project_the_dialog_creates() {
    let projects = tempfile::tempdir().expect("tempdir");
    remember_location(projects.path());
    let mut app = util::editor_test_app();

    jackdaw::project_select::open_new_project_modal(app.world_mut(), TemplateKind::Game);
    jackdaw::project_select::create_new_project(app.world_mut(), "Moss Valley");

    let project = projects.path().join("moss-valley");
    open_after(&mut app, &project);
    assert_eq!(
        open_project(&app).as_deref(),
        Some(project.as_path()),
        "the editor opened the project named in the dialog"
    );
}

#[test]
fn a_name_the_command_line_would_refuse_is_refused_by_the_dialog_too() {
    let projects = tempfile::tempdir().expect("tempdir");
    remember_location(projects.path());
    let mut app = util::editor_test_app();

    jackdaw::project_select::open_new_project_modal(app.world_mut(), TemplateKind::Game);
    jackdaw::project_select::create_new_project(app.world_mut(), "9 lives");
    for _ in 0..4 {
        app.update();
    }

    assert_eq!(
        std::fs::read_dir(projects.path())
            .expect("the location exists")
            .count(),
        0,
        "nothing was scaffolded for a refused name"
    );
    assert!(open_project(&app).is_none(), "and no project was opened");
}
