//! A model's levels of detail found at import are held in memory as undo
//! entries, and reach the model's `.meta` only on Save or Apply.

use std::path::Path;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use jackdaw::progress::EditorProgress;
use jackdaw::scenes::load_progress::SCENE_LOAD;
use jackdaw_api::prelude::*;
use jackdaw_commands::CommandHistory;
use jackdaw_scene_types::model_import::{
    LevelShow, LodImportSource, ModelLodIndex, read_model_meta,
};

use crate::util;
use crate::util::OperatorResultExt as _;

/// A glTF of one triangle under each of `nodes`, the nth one `n` units along x.
fn gltf_with_nodes(nodes: &[&str]) -> String {
    let listed: Vec<String> = nodes
        .iter()
        .enumerate()
        .map(|(index, name)| {
            format!(r#"{{"name": "{name}", "mesh": 0, "translation": [{index}, 0, 0]}}"#)
        })
        .collect();
    let roots: Vec<String> = (0..nodes.len()).map(|index| index.to_string()).collect();
    format!(
        r#"{{
  "asset": {{"version": "2.0"}},
  "scene": 0,
  "scenes": [{{"nodes": [{roots}]}}],
  "nodes": [{listed}],
  "meshes": [{{"primitives": [{{"attributes": {{"POSITION": 0}}}}]}}],
  "accessors": [
    {{"componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 0], "max": [1, 2, 0]}}
  ]
}}"#,
        roots = roots.join(", "),
        listed = listed.join(", "),
    )
}

fn project() -> (App, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let models = tmp.path().join("assets").join("models");
    std::fs::create_dir_all(&models).expect("models folder");
    for file in ["tree.gltf", "tree_LOD1.gltf", "tree_LOD2.gltf"] {
        std::fs::write(models.join(file), gltf_with_nodes(&["Trunk"])).expect("model");
    }
    std::fs::write(
        models.join("bush.gltf"),
        gltf_with_nodes(&["Bush_LOD0", "Bush_LOD1", "Stones"]),
    )
    .expect("model");
    let scene = tmp.path().join("assets").join("grove.bsn");
    std::fs::write(&scene, "bevy_ecs::hierarchy::Children [\n]\n").expect("scene");
    let mut app = util::editor_test_app();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: tmp.path().to_path_buf(),
            config: default(),
        });
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
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
    }
    (app, tmp)
}

fn import(app: &mut App, path: &str) {
    app.world_mut()
        .operator("model.lod.import")
        .param("path", path.to_string())
        .call()
        .expect("model.lod.import dispatches")
        .assert_finished();
    app.update();
}

fn meta(tmp: &Path, model: &str) -> std::path::PathBuf {
    tmp.join("assets").join(format!("{model}.meta"))
}

fn history(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

#[test]
fn importing_finds_the_files_beside_a_model_and_writes_its_meta_only_on_save() {
    let (mut app, tmp) = project();
    let before = history(&app);

    import(&mut app, "models/tree.gltf");

    let lod = app
        .world()
        .resource::<ModelLodIndex>()
        .get("models/tree.gltf")
        .cloned()
        .expect("levels in memory");
    assert_eq!(lod.source, LodImportSource::SiblingFiles);
    let shows: Vec<LevelShow> = lod.levels.iter().map(|level| level.show.clone()).collect();
    assert_eq!(
        shows,
        [
            LevelShow::Model,
            LevelShow::File("tree_LOD1.gltf".into()),
            LevelShow::File("tree_LOD2.gltf".into()),
        ]
    );
    assert!((lod.size - 2.0).abs() < 1e-4, "measured {}", lod.size);
    assert_eq!(history(&app), before + 1);
    assert!(!meta(tmp.path(), "models/tree.gltf").exists());

    assert!(jackdaw::scene_io::save_scene(app.world_mut()));

    let written = std::fs::read(meta(tmp.path(), "models/tree.gltf")).expect("the meta");
    assert_eq!(
        read_model_meta(&written).and_then(|settings| settings.lod),
        Some((*lod).clone())
    );
}

#[test]
fn nodes_named_by_level_inside_one_file_import_as_node_levels() {
    let (mut app, _tmp) = project();

    import(&mut app, "models/bush.gltf");

    let lod = app
        .world()
        .resource::<ModelLodIndex>()
        .get("models/bush.gltf")
        .cloned()
        .expect("levels in memory");
    assert_eq!(lod.source, LodImportSource::NodeSuffixes);
    let shows: Vec<LevelShow> = lod.levels.iter().map(|level| level.show.clone()).collect();
    assert_eq!(
        shows,
        [
            LevelShow::Nodes(vec!["Bush_LOD0".into(), "Stones".into()]),
            LevelShow::Nodes(vec!["Bush_LOD1".into(), "Stones".into()]),
        ]
    );
}

#[test]
fn undoing_an_import_drops_the_settings_it_made_and_save_writes_none() {
    let (mut app, tmp) = project();
    import(&mut app, "models/tree.gltf");

    app.world_mut()
        .operator("history.undo")
        .call()
        .expect("history.undo dispatches")
        .assert_finished();
    app.update();

    assert!(
        app.world()
            .resource::<ModelLodIndex>()
            .get("models/tree.gltf")
            .is_none()
    );
    assert!(jackdaw::scene_io::save_scene(app.world_mut()));
    assert!(!meta(tmp.path(), "models/tree.gltf").exists());
}

#[test]
fn reimporting_keeps_the_screen_heights_of_saved_settings() {
    let (mut app, _tmp) = project();
    import(&mut app, "models/tree.gltf");
    let mut lod = (**app
        .world()
        .resource::<ModelLodIndex>()
        .get("models/tree.gltf")
        .expect("levels"))
    .clone();
    lod.levels[0].screen_height = 0.42;
    jackdaw::model_lod::edit_model_levels(app.world_mut(), "models/tree.gltf", Some(lod));

    import(&mut app, "models/tree.gltf");

    let kept = app
        .world()
        .resource::<ModelLodIndex>()
        .get("models/tree.gltf")
        .expect("levels")
        .levels[0]
        .screen_height;
    assert_eq!(kept, 0.42);
}

#[test]
fn apply_writes_one_models_meta_without_saving_the_scene() {
    let (mut app, tmp) = project();
    import(&mut app, "models/tree.gltf");
    import(&mut app, "models/bush.gltf");

    app.world_mut()
        .operator("model.lod.apply")
        .param("path", "models/bush.gltf".to_string())
        .call()
        .expect("model.lod.apply dispatches")
        .assert_finished();
    app.update();

    assert!(meta(tmp.path(), "models/bush.gltf").exists());
    assert!(!meta(tmp.path(), "models/tree.gltf").exists());
}
