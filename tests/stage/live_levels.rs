//! A LOD group's levels come and go with the camera without touching the
//! document: nothing is saved, undone or listed for them.

use std::path::Path;
use std::time::{Duration, Instant};

use bevy::camera::primitives::Aabb;
use bevy::prelude::*;
use jackdaw::progress::EditorProgress;
use jackdaw::scenes::load_progress::SCENE_LOAD;
use jackdaw::viewport::MainViewportCamera;
use jackdaw_api::prelude::*;
use jackdaw_commands::CommandHistory;
use jackdaw_runtime::{LiveLevelProgress, LiveLevels, LodPart};
use jackdaw_scene_types::model_parts::{FlatModel, ModelPart, ModelParts};
use jackdaw_widgets::tree_view::TreeIndex;

use crate::util;
use crate::util::OperatorResultExt as _;

const TREE_LEVELS: [&str; 3] = [
    "models/tree.gltf",
    "models/tree_LOD1.gltf",
    "models/tree_LOD2.gltf",
];

fn group_at(name: &str, x: f32) -> String {
    let mut text = format!(
        "    #{name}\n    bevy_transform::components::transform::Transform {{\n        translation: glam::Vec3 {{ x: {x}, y: 0.0, z: 0.0 }},\n    }}\n    jackdaw_scene_types::types::LodGroup {{\n        levels: [\n"
    );
    for height in [0.5, 0.25, 0.1] {
        text.push_str(&format!(
            "            jackdaw_scene_types::types::LodLevel {{ screen_height: {height} }},\n"
        ));
    }
    text.push_str("        ],\n        size: 2.0,\n    }\n    bevy_ecs::hierarchy::Children [\n");
    for (index, path) in TREE_LEVELS.iter().enumerate() {
        text.push_str(&format!(
            "        #LOD{index}\n        bevy_transform::components::transform::Transform\n        jackdaw_scene_types::types::GltfSource {{\n            path: \"{path}\",\n            scene_index: 0,\n        }}\n"
        ));
        if index + 1 < TREE_LEVELS.len() {
            text.push_str("        ,\n");
        }
    }
    text.push_str("    ]\n");
    text
}

fn forest() -> String {
    format!(
        "bevy_ecs::hierarchy::Children [\n{}    ,\n{}]\n",
        group_at("Oak", 0.0),
        group_at("Ash", 40.0)
    )
}

fn tree_model() -> FlatModel {
    FlatModel {
        parts: vec![ModelPart {
            mesh: Handle::default(),
            material: Handle::default(),
            material_name: None,
            local: Transform::IDENTITY,
        }],
        bounds: Aabb::from_min_max(Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 2.0, 1.0)),
        needs_instance: false,
    }
}

/// An editor holding the tree models already flattened, so its levels come in
/// without any file to read.
fn editor_with_trees() -> App {
    let mut app = util::editor_test_app();
    let mut models = app.world_mut().resource_mut::<ModelParts>();
    for path in TREE_LEVELS {
        models.insert(path, tree_model());
    }
    app
}

fn open(app: &mut App, scene: &Path) {
    app.world_mut()
        .operator("scene.open")
        .param("path", scene.display().to_string())
        .call()
        .expect("scene.open dispatches")
        .assert_finished();
    let deadline = Instant::now() + Duration::from_secs(60);
    while app
        .world()
        .resource::<EditorProgress>()
        .is_running(SCENE_LOAD)
        && Instant::now() < deadline
    {
        app.update();
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Stand the main viewport's camera at `eye`, giving the windowless editor
/// one if it has none.
fn place_camera(app: &mut App, eye: Vec3) {
    let placed = Transform::from_translation(eye).looking_at(Vec3::ZERO, Vec3::Y);
    let camera = app
        .world_mut()
        .query_filtered::<Entity, With<MainViewportCamera>>()
        .iter(app.world())
        .next();
    match camera {
        Some(camera) => {
            app.world_mut().entity_mut(camera).insert(placed);
        }
        None => {
            app.world_mut()
                .spawn((Camera3d::default(), MainViewportCamera, placed));
        }
    }
}

fn settle(app: &mut App) {
    for _ in 0..12 {
        app.update();
    }
}

fn parts(app: &mut App) -> Vec<Entity> {
    app.world_mut()
        .query_filtered::<Entity, With<LodPart>>()
        .iter(app.world())
        .collect()
}

/// Camera positions that cross every switch of both groups, near and far.
const FLIGHT: [Vec3; 5] = [
    Vec3::new(0.0, 0.0, 2.0),
    Vec3::new(0.0, 0.0, 5.0),
    Vec3::new(0.0, 0.0, 12.0),
    Vec3::new(40.0, 0.0, 2.0),
    Vec3::new(20.0, 0.0, 200.0),
];

#[test]
fn saving_after_flying_across_every_switch_writes_the_same_bytes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scene = dir.path().join("forest.bsn");
    std::fs::write(&scene, forest()).expect("write the scene");
    let mut app = editor_with_trees();
    open(&mut app, &scene);
    settle(&mut app);
    assert!(jackdaw::scene_io::save_scene(app.world_mut()));
    let before = std::fs::read(&scene).expect("read the save");

    let mut seen = 0;
    for eye in FLIGHT {
        place_camera(&mut app, eye);
        settle(&mut app);
        seen += parts(&mut app).len();
    }
    assert!(seen > 0, "the flight placed no levels");
    assert!(jackdaw::scene_io::save_scene(app.world_mut()));

    assert_eq!(std::fs::read(&scene).expect("read the save"), before);
}

#[test]
fn level_swaps_add_no_undo_history_and_no_outliner_rows() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scene = dir.path().join("forest.bsn");
    std::fs::write(&scene, forest()).expect("write the scene");
    let mut app = editor_with_trees();
    open(&mut app, &scene);
    settle(&mut app);
    let history = app.world().resource::<CommandHistory>().undo_stack.len();

    for eye in FLIGHT {
        place_camera(&mut app, eye);
        settle(&mut app);
        for part in parts(&mut app) {
            assert!(
                !app.world().resource::<TreeIndex>().contains_anywhere(part),
                "a level part has an outliner row"
            );
        }
    }

    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        history
    );
}

#[test]
fn opening_a_scene_finishes_once_every_group_draws_something() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scene = dir.path().join("forest.bsn");
    std::fs::write(&scene, forest()).expect("write the scene");
    let mut app = editor_with_trees();
    place_camera(&mut app, Vec3::new(0.0, 0.0, 5.0));

    open(&mut app, &scene);

    let progress = *app.world().resource::<LiveLevelProgress>();
    assert_eq!(progress.groups, 2);
    assert!(
        progress.is_drawn(),
        "the load ended before {progress:?} drew"
    );
}

fn named(app: &mut App, name: &str) -> Entity {
    app.world_mut()
        .query::<(Entity, &Name)>()
        .iter(app.world())
        .find(|(_, held)| held.as_str() == name)
        .map(|(entity, _)| entity)
        .unwrap_or_else(|| panic!("no entity named {name}"))
}

#[test]
fn forcing_a_level_draws_it_at_any_distance_and_leaves_the_document_unchanged() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scene = dir.path().join("forest.bsn");
    std::fs::write(&scene, forest()).expect("write the scene");
    let mut app = editor_with_trees();
    open(&mut app, &scene);
    place_camera(&mut app, Vec3::new(0.0, 0.0, 12.0));
    settle(&mut app);
    assert!(jackdaw::scene_io::save_scene(app.world_mut()));
    let before = std::fs::read(&scene).expect("read the save");
    let oak = named(&mut app, "Oak");
    let ready = |app: &App| {
        app.world()
            .get::<LiveLevels>(oak)
            .map(LiveLevels::ready)
            .unwrap_or_default()
    };
    assert_eq!(ready(&app) & 1, 0, "LOD0 is in the world far away");

    app.world_mut()
        .operator("view.force_lod")
        .param("level", 0i64)
        .call()
        .expect("view.force_lod dispatches")
        .assert_finished();
    settle(&mut app);
    assert_eq!(ready(&app), 1, "only LOD0 is in the world while forced");

    app.world_mut()
        .operator("view.force_lod")
        .call()
        .expect("view.force_lod dispatches")
        .assert_finished();
    settle(&mut app);
    assert_eq!(ready(&app) & 1, 0, "the distance chooses again");

    assert!(jackdaw::scene_io::save_scene(app.world_mut()));
    assert_eq!(std::fs::read(&scene).expect("read the save"), before);
}

fn tree_settings() -> jackdaw_scene_types::model_import::ModelLod {
    use jackdaw_scene_types::model_import::{LevelShow, LodImportSource, ModelLod, ModelLodLevel};
    ModelLod {
        version: ModelLod::VERSION,
        source: LodImportSource::SiblingFiles,
        size: 2.0,
        fade: jackdaw_scene_types::LodFade::Snap,
        levels: [0.5, 0.25, 0.1]
            .into_iter()
            .enumerate()
            .map(|(index, screen_height)| ModelLodLevel {
                show: match index {
                    0 => LevelShow::Model,
                    _ => LevelShow::File(format!("tree_LOD{index}.gltf")),
                },
                screen_height,
            })
            .collect(),
    }
}

#[test]
fn a_model_with_lod_settings_draws_live_levels_and_saves_as_a_plain_placement() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scene = dir.path().join("grove.bsn");
    let text = format!(
        "bevy_ecs::hierarchy::Children [\n    #Oak\n    bevy_transform::components::transform::Transform\n    jackdaw_scene_types::types::GltfSource {{\n        path: \"{}\",\n        scene_index: 0,\n    }}\n]\n",
        TREE_LEVELS[0]
    );
    std::fs::write(&scene, &text).expect("write the scene");
    let mut app = editor_with_trees();
    app.world_mut()
        .resource_mut::<jackdaw_scene_types::model_import::ModelLodIndex>()
        .set(TREE_LEVELS[0], Some(tree_settings()));
    place_camera(&mut app, Vec3::new(0.0, 0.0, 5.0));
    open(&mut app, &scene);
    settle(&mut app);
    assert!(jackdaw::scene_io::save_scene(app.world_mut()));
    let saved = std::fs::read_to_string(&scene).expect("read the save");

    let oak = named(&mut app, "Oak");
    let live = app.world().get::<LiveLevels>(oak).expect("live levels");
    assert_eq!(live.model_path(1), Some(TREE_LEVELS[1]));
    assert_ne!(live.ready(), 0, "a level draws");
    assert!(
        app.world()
            .get::<bevy::world_serialization::WorldAssetRoot>(oak)
            .is_none()
    );
    assert!(!saved.contains("LodGroup"), "{saved}");
    assert!(!saved.contains("ModelLevels"), "{saved}");
}
