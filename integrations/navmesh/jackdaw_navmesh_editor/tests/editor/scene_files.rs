//! A bake belongs to its scene: it is written beside the scene's file, read
//! back when the scene opens, and kept with the scene's tab.

use bevy::prelude::*;
use jackdaw_api::op::{Operator as _, OperatorResult, OperatorWorldExt as _};
use jackdaw_api::terrain::TerrainDataStore;
use jackdaw_navmesh_editor::{BakeStatus, TerrainNavmeshBakeOp, TerrainNavmeshState};

use crate::util;

fn scratch_dir(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("jackdaw-navmesh-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// A new scene tab saving to `path`, holding a selected terrain.
fn scene_with_terrain(app: &mut App, path: &std::path::Path) {
    let result = app
        .world_mut()
        .operator("scene.new")
        .param("path", path.to_string_lossy().into_owned())
        .call()
        .expect("scene.new runs");
    assert_eq!(result, OperatorResult::Finished);
    util::settle(app);
    let terrain = util::selected_terrain(app);
    let terrain = app
        .world()
        .get::<jackdaw_scene_types::Terrain>(terrain)
        .expect("a terrain")
        .clone();
    app.world_mut()
        .resource_mut::<TerrainDataStore>()
        .entry_for(&terrain)
        .expect("the terrain has a document")
        .ensure_extent(64)
        .expect("one region of ground");
    util::settle(app);
}

fn bake(app: &mut App) {
    let result = app
        .world_mut()
        .operator(TerrainNavmeshBakeOp::ID)
        .call()
        .expect("the bake runs");
    assert_eq!(
        result,
        OperatorResult::Finished,
        "{:?}",
        app.world().resource::<TerrainNavmeshState>().status
    );
    for _ in 0..2_000 {
        if app.world().resource::<TerrainNavmeshState>().status != BakeStatus::Baking {
            break;
        }
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let state = app.world().resource::<TerrainNavmeshState>();
    assert_eq!(state.status, BakeStatus::Done, "the bake finished");
}

#[test]
fn a_bake_is_written_beside_its_scene_and_read_back_when_it_reopens() {
    let mut app = util::editor_app();
    let dir = scratch_dir("reopen");
    let scene = dir.join("field.bsn");
    scene_with_terrain(&mut app, &scene);
    let result = app
        .world_mut()
        .operator("scene.save")
        .param("path", scene.to_string_lossy().into_owned())
        .call()
        .expect("scene.save runs");
    assert_eq!(result, OperatorResult::Finished);
    util::settle(&mut app);

    bake(&mut app);
    let artifact = dir.join("field.jdnav");
    assert!(artifact.is_file(), "the bake lands beside the scene");

    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene);
    util::settle(&mut app);

    let state = app.world().resource::<TerrainNavmeshState>();
    assert_eq!(state.status, BakeStatus::Done);
    let baked = state.baked.as_ref().expect("the bake is read back");
    assert_eq!(baked.path.as_deref(), Some(artifact.as_path()));
    assert_eq!(baked.seconds, None, "read off disk, not baked again");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn each_scene_tab_keeps_its_own_bake() {
    let mut app = util::editor_app();
    let dir = scratch_dir("tabs");
    scene_with_terrain(&mut app, &dir.join("first.bsn"));
    bake(&mut app);
    let first = app.world().resource::<jackdaw::scenes::Scenes>().active;

    let result = app
        .world_mut()
        .operator("scene.new")
        .call()
        .expect("scene.new runs");

    assert_eq!(result, OperatorResult::Finished);
    util::settle(&mut app);
    assert!(
        app.world()
            .resource::<TerrainNavmeshState>()
            .baked
            .is_none(),
        "a new scene starts unbaked"
    );

    jackdaw::scenes::swap::swap_active_tab(app.world_mut(), first);
    util::settle(&mut app);
    assert!(
        app.world()
            .resource::<TerrainNavmeshState>()
            .baked
            .is_some(),
        "coming back to the tab brings its bake back"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
