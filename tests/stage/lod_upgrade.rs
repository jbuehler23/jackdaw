//! LOD groups written as a model and its level children, or as a model that
//! names its levels by file name, open as plain placements of the model with
//! the levels in its import settings: in memory, as one undo entry, and on
//! disk only once saved.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw::viewport::MainViewportCamera;
use jackdaw_api::prelude::*;
use jackdaw_commands::CommandHistory;
use jackdaw_scene_types::model_import::{LevelShow, ModelLevels, read_model_meta};
use jackdaw_scene_types::{GltfSource, LodGroup};

use crate::util;
use crate::util::OperatorResultExt as _;

const TREE: &str = "models/tree.gltf";

fn mesh_file() -> String {
    r#"{
  "asset": {"version": "2.0"},
  "scene": 0,
  "scenes": [{"nodes": [0]}],
  "nodes": [{"name": "Tree", "mesh": 0}],
  "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}],
  "accessors": [
    {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 0], "max": [1, 2, 1]}
  ],
  "bufferViews": [{"buffer": 0, "byteLength": 36}],
  "buffers": [{"byteLength": 36, "uri": "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAIA/AAAAAAAAAEAAAAAA"}]
}"#
    .to_string()
}

fn levels_block(heights: [f32; 3]) -> String {
    let mut text = String::from("jackdaw_scene_types::types::LodGroup {\n    levels: [\n");
    for height in heights {
        text.push_str(&format!(
            "        jackdaw_scene_types::types::LodLevel {{ screen_height: {height} }},\n"
        ));
    }
    text.push_str("    ],\n    size: 2.0,\n}\n");
    text
}

/// A group of the tree's three levels, each level child carrying `extra`.
fn level_children(first_id: Option<u32>, extra: &str) -> String {
    let files = ["tree.gltf", "tree_LOD1.gltf", "tree_LOD2.gltf"];
    let mut text = String::from("bevy_ecs::hierarchy::Children [\n");
    for (index, file) in files.iter().enumerate() {
        text.push_str(&format!(
            "    #LOD{index}\n    bevy_transform::components::transform::Transform\n    jackdaw_scene_types::types::GltfSource {{ path: \"models/{file}\", scene_index: 0 }}\n"
        ));
        if let Some(first) = first_id {
            text.push_str(&format!(
                "    jackdaw::prefab::components::PrefabEntityId({})\n",
                first + index as u32
            ));
        }
        if index == 1 {
            text.push_str(extra);
        }
        if index + 1 < files.len() {
            text.push_str("    ,\n");
        }
    }
    text.push_str("]\n");
    text
}

fn grove_prefab() -> String {
    format!(
        "jackdaw::prefab::components::Prefab\njackdaw::prefab::components::PrefabEntityId(0)\n#Grove\nbevy_transform::components::transform::Transform\nbevy_ecs::hierarchy::Children [\n#Tree\nbevy_transform::components::transform::Transform\n{}jackdaw::prefab::components::PrefabEntityId(1)\n{}]\n",
        levels_block([0.5, 0.25, 0.1]),
        level_children(Some(2), "")
    )
}

/// An instance of the grove, its listing of the group node carrying `group`
/// and of the second level carrying `level`.
fn grove_instance(group: &str, level: &str) -> String {
    format!(
        "#GroveOne\njackdaw::prefab::components::IsA {{ source: \"prefabs/grove.bsn\", deleted: [] }}\njackdaw::prefab::components::PrefabEntityId(0)\nbevy_transform::components::transform::Transform\nbevy_ecs::hierarchy::Children [\n    jackdaw::prefab::components::PrefabEntityId(1)\n{group}    bevy_ecs::hierarchy::Children [\n        jackdaw::prefab::components::PrefabEntityId(2)\n        ,\n        jackdaw::prefab::components::PrefabEntityId(3)\n{level}        ,\n        jackdaw::prefab::components::PrefabEntityId(4)\n    ]\n]\n"
    )
}

fn oak(extra: &str) -> String {
    format!(
        "#Oak\nbevy_transform::components::transform::Transform\n{}{}",
        levels_block([0.5, 0.25, 0.1]),
        level_children(None, extra)
    )
}

fn scene_of(nodes: &[String]) -> String {
    format!("bevy_ecs::hierarchy::Children [\n{}]\n", nodes.join(",\n"))
}

struct Project {
    _dir: tempfile::TempDir,
    assets: PathBuf,
    scene: PathBuf,
}

fn project(scene: &str) -> Project {
    let dir = tempfile::tempdir().expect("tempdir");
    let assets = dir.path().join("assets");
    std::fs::create_dir_all(assets.join("models")).expect("models");
    std::fs::create_dir_all(assets.join("prefabs")).expect("prefabs");
    for file in ["tree.gltf", "tree_LOD1.gltf", "tree_LOD2.gltf"] {
        std::fs::write(assets.join("models").join(file), mesh_file()).expect("model");
    }
    std::fs::write(assets.join("prefabs/grove.bsn"), grove_prefab()).expect("prefab");
    let path = assets.join("grove_scene.bsn");
    std::fs::write(&path, scene).expect("scene");
    Project {
        assets,
        scene: path,
        _dir: dir,
    }
}

fn editor(project: &Project) -> App {
    let mut app = util::editor_test_app_reading(&project.assets);
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: project.assets.parent().expect("root").to_path_buf(),
            config: default(),
        });
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    app.world_mut().spawn((
        Camera3d::default(),
        MainViewportCamera,
        Transform::from_xyz(0.0, 0.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    app
}

fn open(app: &mut App, scene: &Path) {
    app.world_mut()
        .operator("scene.open")
        .param("path", scene.display().to_string())
        .call()
        .expect("scene.open dispatches")
        .assert_finished();
    util::settle_scene_load(app);
    for _ in 0..4 {
        app.update();
    }
}

fn named(app: &mut App, name: &str) -> Entity {
    app.world_mut()
        .query::<(Entity, &Name)>()
        .iter(app.world())
        .find(|(_, held)| held.as_str() == name)
        .map(|(entity, _)| entity)
        .unwrap_or_else(|| panic!("no entity named {name}"))
}

fn history(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

fn save(app: &mut App, project: &Project) -> (String, String) {
    assert!(jackdaw::scene_io::save_scene(app.world_mut()));
    (
        std::fs::read_to_string(&project.scene).expect("scene"),
        std::fs::read_to_string(project.assets.join("prefabs/grove.bsn")).expect("prefab"),
    )
}

fn files(project: &Project) -> Vec<Vec<u8>> {
    [
        project.scene.clone(),
        project.assets.join("prefabs/grove.bsn"),
    ]
    .iter()
    .map(|path| std::fs::read(path).expect("file"))
    .collect()
}

#[test]
fn opening_an_old_form_scene_writes_nothing_and_adds_one_history_entry() {
    let project = project(&scene_of(&[oak(""), grove_instance("", "")]));
    let before = files(&project);
    let mut app = editor(&project);

    open(&mut app, &project.scene);

    assert_eq!(files(&project), before);
    assert!(!project.assets.join("models/tree.gltf.meta").exists());
    assert_eq!(history(&app), 1);
    let oak = named(&mut app, "Oak");
    assert!(app.world().get::<LodGroup>(oak).is_none());
    assert!(app.world().get::<ModelLevels>(oak).is_some());
    let tree = named(&mut app, "Tree");
    assert!(app.world().get::<GltfSource>(tree).is_some());
    assert!(jackdaw::scene_io::is_scene_dirty(app.world()));
}

#[test]
fn one_undo_restores_the_old_form_and_redo_upgrades_again() {
    let project = project(&scene_of(&[oak(""), grove_instance("", "")]));
    let mut app = editor(&project);
    open(&mut app, &project.scene);

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    for _ in 0..4 {
        app.update();
    }
    let oak = named(&mut app, "Oak");
    assert!(app.world().get::<LodGroup>(oak).is_some());
    assert_eq!(
        app.world()
            .get::<Children>(oak)
            .map(RelationshipTarget::len),
        Some(3)
    );
    let tree = named(&mut app, "Tree");
    assert!(app.world().get::<LodGroup>(tree).is_some());
    assert!(
        app.world()
            .resource::<jackdaw_scene_types::model_import::ModelLodIndex>()
            .get(TREE)
            .is_none()
    );

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.redo(world));
    for _ in 0..4 {
        app.update();
    }
    let oak = named(&mut app, "Oak");
    assert!(app.world().get::<LodGroup>(oak).is_none());
    assert!(app.world().get::<GltfSource>(oak).is_some());
}

#[test]
fn saving_after_opening_writes_the_model_settings_and_drops_level_children_and_listings() {
    let project = project(&scene_of(&[oak(""), grove_instance("", "")]));
    let mut app = editor(&project);
    open(&mut app, &project.scene);

    let (scene, prefab) = save(&mut app, &project);

    assert!(!scene.contains("#LOD"), "{scene}");
    assert!(!scene.contains("PrefabEntityId(3)"), "{scene}");
    assert!(!scene.contains("LodGroup"), "{scene}");
    assert!(!prefab.contains("#LOD"), "{prefab}");
    assert!(!prefab.contains("LodGroup"), "{prefab}");
    let meta = std::fs::read(project.assets.join("models/tree.gltf.meta")).expect("meta");
    let lod = read_model_meta(&meta)
        .and_then(|settings| settings.lod)
        .expect("levels");
    assert_eq!(
        lod.levels
            .iter()
            .map(|level| level.show.clone())
            .collect::<Vec<_>>(),
        [
            LevelShow::Model,
            LevelShow::File("tree_LOD1.gltf".into()),
            LevelShow::File("tree_LOD2.gltf".into()),
        ]
    );
    assert!(!jackdaw::scene_io::is_scene_dirty(app.world()));
}

#[test]
fn reopening_a_saved_upgrade_adds_no_history_entry() {
    let project = project(&scene_of(&[oak(""), grove_instance("", "")]));
    let mut app = editor(&project);
    open(&mut app, &project.scene);
    save(&mut app, &project);
    drop(app);

    let mut app = editor(&project);
    let meta = std::fs::read(project.assets.join("models/tree.gltf.meta")).expect("meta");
    let saved = read_model_meta(&meta).and_then(|settings| settings.lod);
    assert!(saved.is_some(), "the save wrote the tree's levels");
    app.world_mut()
        .resource_mut::<jackdaw_scene_types::model_import::ModelLodIndex>()
        .set(TREE, saved);
    open(&mut app, &project.scene);

    assert_eq!(history(&app), 0);
    let oak = named(&mut app, "Oak");
    assert!(app.world().get::<LodGroup>(oak).is_none());
    assert!(app.world().get::<ModelLevels>(oak).is_some());
}

#[test]
fn a_group_whose_level_has_its_own_transform_stays_hand_built() {
    let moved = "    bevy_transform::components::transform::Transform { translation: glam::Vec3 { x: 1.0, y: 0.0, z: 0.0 } }\n";
    let scene = scene_of(&[oak("").replacen("#Oak", "#Elm", 1), oak(moved)]);
    let project = project(&scene);
    let mut app = editor(&project);
    open(&mut app, &project.scene);

    let (scene, _) = save(&mut app, &project);

    let elm = named(&mut app, "Elm");
    assert!(app.world().get::<LodGroup>(elm).is_none());
    let oak = named(&mut app, "Oak");
    assert!(app.world().get::<LodGroup>(oak).is_some());
    assert_eq!(scene.matches("#LOD").count(), 3, "{scene}");
}

#[test]
fn an_instance_that_changes_a_level_keeps_its_prefabs_groups() {
    let changed = "        bevy_transform::components::transform::Transform { translation: glam::Vec3 { x: 3.0, y: 0.0, z: 0.0 } }\n";
    let project = project(&scene_of(&[grove_instance("", changed)]));
    let mut app = editor(&project);
    open(&mut app, &project.scene);

    let tree = named(&mut app, "Tree");
    assert!(app.world().get::<LodGroup>(tree).is_some());
    assert_eq!(history(&app), 0);
}

#[test]
fn an_instance_overriding_a_groups_heights_gets_a_lod_override() {
    let heights = "    jackdaw_scene_types::types::LodGroup { levels: [ jackdaw_scene_types::types::LodLevel { screen_height: 0.8 }, jackdaw_scene_types::types::LodLevel { screen_height: 0.4 }, jackdaw_scene_types::types::LodLevel { screen_height: 0.2 } ] }\n";
    let project = project(&scene_of(&[grove_instance(heights, "")]));
    let mut app = editor(&project);
    open(&mut app, &project.scene);

    let tree = named(&mut app, "Tree");
    assert_eq!(
        app.world()
            .get::<jackdaw_scene_types::LodOverride>(tree)
            .and_then(|held| held.screen_heights.clone()),
        Some(vec![0.8, 0.4, 0.2])
    );
    let (scene, _) = save(&mut app, &project);
    assert!(scene.contains("LodOverride"), "{scene}");
    assert!(!scene.contains("LodGroup"), "{scene}");
}

#[test]
fn a_group_naming_its_model_opens_upgraded_and_switches_where_it_did() {
    let implied = format!(
        "#Pine\nbevy_transform::components::transform::Transform\n{}jackdaw_scene_types::types::GltfSource {{ path: \"{TREE}\", scene_index: 0 }}\n",
        levels_block([0.5, 0.25, 0.1])
    );
    let project = project(&scene_of(&[implied]));
    let mut app = editor(&project);
    open(&mut app, &project.scene);

    let pine = named(&mut app, "Pine");
    assert!(app.world().get::<LodGroup>(pine).is_none());
    let levels = app
        .world()
        .get::<ModelLevels>(pine)
        .expect("levels")
        .clone();
    assert_eq!(
        levels.models,
        [
            "models/tree.gltf",
            "models/tree_LOD1.gltf",
            "models/tree_LOD2.gltf"
        ]
    );
    let switches = app
        .world()
        .get::<jackdaw_runtime::LodSwitches>(pine)
        .expect("switches");
    let half_fov_tan = (std::f32::consts::FRAC_PI_4 / 2.0).tan();
    let first = switches.ranges[0].end_margin.start;
    assert!(
        (first - jackdaw_runtime::lod_distance(2.0, 0.5, half_fov_tan)).abs() < 0.05,
        "{first}"
    );
}

#[test]
fn a_scene_written_with_defaults_left_out_upgrades_like_one_spelling_them() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lod_levels_as_children.bsn");
    let elided = std::fs::read_to_string(&fixture).expect("fixture");
    let spelled = elided.replacen("    size: 2.0,\n", "    size: 2.0,\n    fade: 0.0,\n", 1);
    assert_ne!(elided, spelled);
    let mut saved = Vec::new();
    for text in [elided, spelled] {
        let project = project(&text);
        let mut app = editor(&project);
        open(&mut app, &project.scene);
        saved.push(save(&mut app, &project).0);
    }
    assert_eq!(saved[0], saved[1]);
    assert!(!saved[0].contains("#LOD"), "{}", saved[0]);
}
