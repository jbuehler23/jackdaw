//! Opening a project keeps the window drawing and says what it is doing.
//!
//! A project of any size used to reach the editor through a handful of frames
//! that each did everything: walking the assets, spawning the scene, handing
//! every model to the scene spawner. Nothing drew while they ran and nothing
//! said why. The steps below are spread over frames, and the footer names the
//! one running.

use crate::util;

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw::asset_index::AssetIndex;
use jackdaw::entity_ops::PendingModelRoots;
use jackdaw::status_bar::EditorPhase;

const MATERIAL: &str = "#slate\nbevy_pbr::pbr_material::StandardMaterial {}\n";

const SCENE: &str = "bevy_ecs::hierarchy::Children [\n    \
                     #Root\n    \
                     bevy_transform::components::transform::Transform\n]\n";

/// How many models the scene below names.
const MODELS: usize = 3;

/// A scene of `MODELS` entities, each naming a model of its own.
fn models_scene() -> String {
    let mut text = String::from("bevy_ecs::hierarchy::Children [\n");
    for index in 0..MODELS {
        text.push_str(&format!(
            "    #Model{index}\n                 jackdaw_scene_types::types::GltfSource {{\n                     path: \"models/model{index}.gltf\",\n                     scene_index: 0,\n    }}\n                 bevy_transform::components::transform::Transform,\n"
        ));
    }
    text.push_str("]\n");
    text
}

/// A project whose remembered tab is a scene naming [`MODELS`] models.
fn model_project() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write(&root.join("assets/scene.bsn"), &models_scene());
    write(
        &root.join(".jackdaw/project.json"),
        r#"{"name":"fixture","last_open_tabs":["assets/scene.bsn"],"last_active_tab":0}"#,
    );
    tmp
}

/// A project with one scene remembered as its open tab and one material file
/// for the walk to find.
fn fixture_project() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write(&root.join("assets/scene.bsn"), SCENE);
    write(&root.join("assets/materials/slate.bsn"), MATERIAL);
    write(
        &root.join(".jackdaw/project.json"),
        r#"{"name":"fixture","last_open_tabs":["assets/scene.bsn"],"last_active_tab":0}"#,
    );
    tmp
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("the folder is made");
    std::fs::write(path, text).expect("the file is written");
}

/// An editor at the launcher, asked to open `root` the way a handoff does.
fn editor_opening(root: &Path) -> App {
    let mut app = util::editor_test_app();
    app.world_mut()
        .insert_resource(jackdaw::project_select::PendingAutoOpen {
            path: root.to_path_buf(),
            skip_build: true,
        });
    app
}

fn phase(app: &App) -> Option<String> {
    app.world()
        .resource::<EditorPhase>()
        .current()
        .map(str::to_owned)
}

fn open_scene(app: &App) -> Option<PathBuf> {
    app.world()
        .resource::<jackdaw::scenes::Scenes>()
        .tabs
        .iter()
        .find_map(|tab| tab.path.clone())
}

#[test]
fn the_footer_names_the_scene_it_is_opening_before_it_opens_it() {
    let tmp = fixture_project();
    let mut app = editor_opening(tmp.path());

    // The handoff frame, then the first editor frame: the scene is named but
    // not yet spawned, so the window draws with the name on it.
    app.update();
    app.update();

    assert_eq!(
        phase(&app).as_deref(),
        Some("Opening scene"),
        "the footer names the scene before the frame that spawns it"
    );
    assert_eq!(
        open_scene(&app),
        None,
        "and nothing has been opened yet, so that frame had room to draw"
    );

    app.update();

    assert_eq!(
        open_scene(&app),
        Some(tmp.path().join("assets/scene.bsn")),
        "the frame after the one that named it opens it"
    );
}

#[test]
fn the_frame_that_opens_a_project_does_not_walk_its_assets() {
    let tmp = fixture_project();
    let mut app = editor_opening(tmp.path());

    app.update();
    app.update();

    assert_eq!(
        app.world().resource::<AssetIndex>().iter().count(),
        0,
        "the frame that enters the editor asks for the walk rather than doing it"
    );

    for _ in 0..16 {
        app.update();
    }

    assert!(
        app.world()
            .resource::<AssetIndex>()
            .get(Path::new("materials/slate.bsn"))
            .is_some(),
        "and the walk that ran off the main thread fills the index a few frames later"
    );
}

#[test]
fn opening_a_project_names_its_phases_and_stops_naming_when_it_is_done() {
    let tmp = fixture_project();
    let mut app = editor_opening(tmp.path());

    // The footer has room for one line, so what it shows is the phase begun
    // most recently; every phase that ran is collected here.
    let mut named: Vec<String> = Vec::new();
    for _ in 0..48 {
        app.update();
        let running: Vec<String> = app
            .world()
            .resource::<EditorPhase>()
            .running()
            .map(str::to_owned)
            .collect();
        for line in running {
            if !named.contains(&line) {
                named.push(line);
            }
        }
    }

    assert!(
        named.iter().any(|line| line == "Opening scene"),
        "the footer named the scene it was opening, out of {named:?}"
    );
    assert!(
        named
            .iter()
            .any(|line| line == "Indexing the project's assets"),
        "and the walk that fills the index, out of {named:?}"
    );
    assert_eq!(
        phase(&app),
        None,
        "and once the project is open the footer goes back to the tool"
    );
}

/// A texture is not a document, so nothing the asset index holds changes when
/// one appears. The panel used to re-read the folder whenever its grid was
/// spawned; now it follows the walk instead, and a walk that changed no
/// document still has to reach it.
#[test]
fn a_texture_dropped_in_after_the_project_opened_reaches_the_materials_panel() {
    let tmp = fixture_project();
    let mut app = editor_opening(tmp.path());
    for _ in 0..24 {
        app.update();
    }

    let textures = tmp.path().join("assets/textures");
    std::fs::create_dir_all(&textures).expect("the folder is made");
    for file in ["moss_albedo.png", "moss_normal.png"] {
        std::fs::write(textures.join(file), [0xffu8; 64]).expect("the texture is written");
    }
    // The walk the watcher would ask for, run here rather than waiting on the
    // filesystem watcher to report the write.
    jackdaw::asset_index::rescan_asset_index(app.world_mut());
    for _ in 0..24 {
        app.update();
    }

    let registry = app
        .world()
        .resource::<jackdaw::material_assets::MaterialRegistry>();
    assert!(
        registry.get_by_name("moss").is_some(),
        "the panel never listed a texture set added after the project opened: {:?}",
        registry.entries.iter().map(|e| &e.name).collect::<Vec<_>>()
    );
}

/// The document used to be spawned once for the tab and once more by the
/// driver that respawns the scene when the prefab cache changes, so every
/// model in it was queued twice and the footer counted twice the scene.
#[test]
fn opening_a_project_queues_one_model_for_each_the_document_names() {
    let tmp = model_project();
    let mut app = editor_opening(tmp.path());

    let mut most = 0;
    for _ in 0..24 {
        app.update();
        most = most.max(app.world().resource::<PendingModelRoots>().len());
    }

    assert_eq!(
        most, MODELS,
        "the document names {MODELS} models and was queued as {most}"
    );
}

/// A queued model belongs to the scene that authored it, so opening another
/// scene over it leaves nothing queued: the entities those entries name have
/// gone with the scene, and a render root handed to one of them would put an
/// instance under nothing.
#[test]
fn a_scene_taken_down_takes_the_models_it_had_queued_with_it() {
    let tmp = model_project();
    let mut app = editor_opening(tmp.path());
    for _ in 0..8 {
        app.update();
    }
    for index in 0..16 {
        app.world_mut().spawn((
            Name::new(format!("Waiting{index}")),
            Transform::default(),
            jackdaw_scene_types::GltfSource {
                path: format!("models/waiting{index}.gltf"),
                scene_index: 0,
            },
        ));
    }
    app.world_mut().flush();
    assert!(
        !app.world().resource::<PendingModelRoots>().is_empty(),
        "the scene's models are waiting their turn"
    );

    let other = tmp.path().join("assets/empty.bsn");
    write(&other, SCENE);
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &other);

    assert!(
        app.world().resource::<PendingModelRoots>().is_empty(),
        "a scene that has gone left models queued against its entities"
    );
}
