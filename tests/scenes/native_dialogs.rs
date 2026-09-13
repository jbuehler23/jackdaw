//! Where the native file and folder dialogs open.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw::native_dialog::{
    DialogMemory, DialogPurpose, browsing_directory, remember_pick, start_directory,
};
use jackdaw::project::{ProjectConfig, ProjectRoot, load_project_config};
use jackdaw::project_window::ProjectWindowState;
use jackdaw::scene_io::SceneFilePath;
use jackdaw::scenes::{SceneTab, Scenes};

struct Project {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

fn project() -> Project {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let root = dir.path().to_path_buf();
    std::fs::create_dir_all(root.join("assets/props")).expect("the assets tree");
    std::fs::create_dir_all(root.join("scenes")).expect("a scenes folder");
    Project { _dir: dir, root }
}

fn world_with(project: &Project) -> World {
    let mut world = World::new();
    world.insert_resource(ProjectRoot::new(
        project.root.clone(),
        ProjectConfig::default(),
    ));
    world.insert_resource(DialogMemory::default());
    world
}

fn browse_at(world: &mut World, directory: &Path) {
    world.insert_resource(ProjectWindowState::at(directory));
}

fn open_scene_at(world: &mut World, path: &Path) {
    let mut tab = SceneTab::new_untitled(0);
    tab.path = Some(path.to_path_buf());
    world.insert_resource(Scenes {
        tabs: vec![tab],
        active: 0,
    });
}

fn same_folder(left: &Path, right: &Path) -> bool {
    let resolve = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    resolve(left) == resolve(right)
}

#[test]
fn a_dialog_opens_in_the_folder_the_project_window_is_showing() {
    let project = project();
    let mut world = world_with(&project);
    let props = project.root.join("assets/props");
    browse_at(&mut world, &props);

    let start = browsing_directory(&world).expect("a start directory");
    assert!(
        same_folder(&start, &props),
        "expected {}, got {}",
        props.display(),
        start.display()
    );
}

#[test]
fn a_folder_outside_the_project_gives_way_to_the_open_scene() {
    let project = project();
    let elsewhere = tempfile::tempdir().expect("a folder outside the project");
    let mut world = world_with(&project);
    browse_at(&mut world, elsewhere.path());
    let scene = project.root.join("scenes/town.bsn");
    std::fs::write(&scene, "").expect("a scene file");
    open_scene_at(&mut world, &scene);

    let start = browsing_directory(&world).expect("a start directory");
    assert!(
        same_folder(&start, &project.root.join("scenes")),
        "expected the open scene's folder, got {}",
        start.display()
    );
}

#[test]
fn with_nothing_open_a_dialog_starts_in_the_assets_directory() {
    let project = project();
    let world = world_with(&project);

    let start = browsing_directory(&world).expect("a start directory");
    assert!(
        same_folder(&start, &project.root.join("assets")),
        "expected the assets directory, got {}",
        start.display()
    );
}

#[test]
fn a_purpose_reopens_in_the_folder_it_was_last_used_in() {
    let project = project();
    let mut world = world_with(&project);
    let props = project.root.join("assets/props");
    let texture = props.join("bark.png");
    std::fs::write(&texture, "").expect("a texture file");

    remember_pick(&mut world, DialogPurpose::Texture, &texture);

    let start = start_directory(&world, DialogPurpose::Texture).expect("a start directory");
    assert!(
        same_folder(&start, &props),
        "expected the remembered folder, got {}",
        start.display()
    );
    assert!(
        same_folder(
            &start_directory(&world, DialogPurpose::Prefab).expect("a start directory"),
            &project.root.join("assets")
        ),
        "another purpose keeps its own folder"
    );
}

#[test]
fn the_bundle_dialog_reopens_where_the_last_bundle_came_from() {
    let project = project();
    let downloads = tempfile::tempdir().expect("a folder outside the project");
    let mut world = world_with(&project);
    let props = project.root.join("assets/props");
    browse_at(&mut world, &props);
    let bundle = downloads.path().join("terrain-tools.jdext");
    std::fs::write(&bundle, "").expect("a bundle file");

    remember_pick(&mut world, DialogPurpose::Bundle, &bundle);

    let start = start_directory(&world, DialogPurpose::Bundle).expect("a start directory");
    assert!(
        same_folder(&start, downloads.path()),
        "expected the folder the last bundle came from, got {}",
        start.display()
    );
    assert!(
        same_folder(
            &start_directory(&world, DialogPurpose::Prefab).expect("a start directory"),
            &props
        ),
        "a project file still follows the Project window"
    );
}

#[test]
fn the_bundle_dialog_falls_back_to_the_browsing_folder_before_any_install() {
    let project = project();
    let mut world = world_with(&project);
    let props = project.root.join("assets/props");
    browse_at(&mut world, &props);

    let start = start_directory(&world, DialogPurpose::Bundle).expect("a start directory");
    assert!(
        same_folder(&start, &props),
        "expected the browsing folder, got {}",
        start.display()
    );
}

#[test]
fn a_remembered_folder_survives_reopening_the_project() {
    let project = project();
    let mut world = world_with(&project);
    let props = project.root.join("assets/props");
    remember_pick(&mut world, DialogPurpose::Prefab, &props.join("crate.bsn"));

    let config = load_project_config(&project.root).expect("the project config was written");
    let mut reopened = World::new();
    reopened.insert_resource(ProjectRoot::new(project.root.clone(), config));
    let mut memory = DialogMemory::default();
    memory.restore(
        reopened
            .resource::<ProjectRoot>()
            .config
            .dialog_directories
            .clone(),
    );
    reopened.insert_resource(memory);

    let start = start_directory(&reopened, DialogPurpose::Prefab).expect("a start directory");
    assert!(
        same_folder(&start, &props),
        "expected the folder remembered before the reopen, got {}",
        start.display()
    );
}

#[test]
fn a_remembered_folder_that_is_gone_falls_back_to_the_project() {
    let project = project();
    let mut world = world_with(&project);
    let removed = project.root.join("assets/props");
    remember_pick(&mut world, DialogPurpose::Image, &removed.join("sky.png"));
    std::fs::remove_dir_all(&removed).expect("remove the folder");

    let start = start_directory(&world, DialogPurpose::Image).expect("a start directory");
    assert!(
        same_folder(&start, &project.root.join("assets")),
        "expected the assets directory, got {}",
        start.display()
    );
}

#[test]
fn the_save_dialog_follows_the_scene_that_is_open() {
    let project = project();
    let mut world = world_with(&project);
    let scene = project.root.join("scenes/town.bsn");
    std::fs::write(&scene, "").expect("a scene file");
    world.insert_resource(SceneFilePath {
        path: Some(scene.to_string_lossy().into_owned()),
        ..Default::default()
    });

    let start = start_directory(&world, DialogPurpose::Scene).expect("a start directory");
    assert!(
        same_folder(&start, &project.root.join("scenes")),
        "expected the open scene's folder, got {}",
        start.display()
    );
}
