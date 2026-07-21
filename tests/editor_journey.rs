#![cfg(feature = "dylib")]
#![expect(clippy::print_stdout, reason = "e2e test reports the journey it ran")]
//! Authors a scene headless through the operators, saves it, builds a game
//! around the saved `.bsn`, runs that game, and asserts the authored entity
//! spawned in it.
//!
//! Uses the `bsn_game` fixture as the host game. That fixture waits for one
//! specific authored entity before reporting, so the journey reads the node id
//! the editor minted out of the saved file and points the fixture at it.

use std::path::PathBuf;
use std::process::Command;

use bevy::prelude::*;
use jackdaw::project_build::build_project_dylib;
use jackdaw::project_build::shim::ShimSpec;
use jackdaw::scenes::Scenes;
use jackdaw::sdk_paths::SdkPaths;
use jackdaw_api::prelude::*;
use jackdaw_scene_types::{Brush, SceneNodeId};

mod util;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Dispatch an operator by id and require it to finish. A `Cancelled` means an
/// availability gate refused, which would otherwise surface as an empty scene.
#[track_caller]
fn dispatch(app: &mut App, id: &'static str) {
    let result = app
        .world_mut()
        .operator(id)
        .call()
        .unwrap_or_else(|err| panic!("{id} dispatch errored: {err}"));
    assert_eq!(result, OperatorResult::Finished, "{id} did not finish");
}

#[test]
fn author_save_build_play() {
    let root = workspace_root();
    let sdk = SdkPaths::for_workspace(&root);
    assert!(
        sdk.wrapper_exists(),
        "SDK wrapper missing; build --features dylib --target {}",
        sdk.triple
    );

    // Author.
    let mut app = util::editor_test_app();
    dispatch(&mut app, "scene.new");
    app.update();
    dispatch(&mut app, "entity.add.cube");
    dispatch(&mut app, "entity.add.point_light");
    app.update();

    let brushes: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<Brush>>()
        .iter(app.world())
        .collect();
    assert!(
        !brushes.is_empty(),
        "entity.add.cube did not author a Brush entity, so there is nothing to save"
    );

    // `scene.save` only falls through to the native dialog when the active tab
    // has no path, so pointing the tab at a file keeps this headless.
    let stage = tempfile::Builder::new()
        .prefix("journey-")
        .tempdir_in(root.join("target"))
        .expect("tempdir under target/");
    let assets = stage.path().join("assets");
    std::fs::create_dir_all(&assets).expect("create assets dir");
    let scene_path = assets.join("scene.bsn");
    {
        let mut scenes = app.world_mut().resource_mut::<Scenes>();
        let active = scenes.active;
        scenes
            .tabs
            .get_mut(active)
            .expect("scene.new left an active tab")
            .path = Some(scene_path.clone());
    }
    dispatch(&mut app, "scene.save");
    app.update();

    let saved = std::fs::read_to_string(&scene_path)
        .unwrap_or_else(|e| panic!("scene.save wrote nothing to {}: {e}", scene_path.display()));
    assert!(
        saved.contains("Brush"),
        "the saved .bsn does not carry the authored brush:\n{saved}"
    );
    // The editor mints node ids into the document, so the id to look for is
    // whatever it wrote.
    let minted: u64 = saved
        .split(&format!("{}(", std::any::type_name::<SceneNodeId>()))
        .nth(1)
        .or_else(|| saved.split("SceneNodeId(").nth(1))
        .and_then(|rest| rest.split(')').next())
        .and_then(|digits| digits.trim().parse().ok())
        .unwrap_or_else(|| panic!("no SceneNodeId in the saved .bsn:\n{saved}"));
    println!(
        "JOURNEY: authored and saved {} bytes of .bsn, minted node id {minted}",
        saved.len()
    );
    let minted_env = minted.to_string();

    // Build a game around the saved scene, the way Play builds it.
    let build_dir = root.join("target/editor_journey");
    std::fs::create_dir_all(&build_dir).expect("create build dir");
    let spec = ShimSpec {
        package_name: "bsn_scene_game".into(),
        crate_name: "bsn_scene_game".into(),
        project_root: root.join("tests/fixtures/bsn_game"),
        game_plugin: Some("GamePlugin".into()),
        extension_type: None,
    };
    let build = build_project_dylib(&spec, &build_dir, &sdk, Some(&root), &mut |_| {})
        .expect("build the game around the authored scene");

    let status = Command::new("cargo")
        .args(["build", "-p", "jackdaw_runner", "--target", &sdk.triple])
        .current_dir(&root)
        .status()
        .expect("build jackdaw-runner");
    assert!(status.success(), "runner build failed");

    // The asset root is the staged dir, so the game loads the scene just
    // authored rather than the fixture's committed one.
    let (loaded, stderr) = util::run_windowless_game(
        &sdk.runner,
        &build.dylib,
        stage.path(),
        &[
            ("BEVY_ASSET_ROOT", stage.path().as_os_str()),
            ("JACKDAW_E2E_NODE_ID", std::ffi::OsStr::new(&minted_env)),
        ],
    );

    assert!(
        loaded,
        "the game never spawned the authored entity from the saved scene \
         (no `BSN_SCENE_LOADED ... has_target=true`); runner stderr:\n{stderr}"
    );

    for line in stderr.lines() {
        if line.contains("BSN_SCENE_LOADED") {
            println!("JOURNEY J1 PASS (authored -> saved -> built -> ran): {line}");
        }
    }
}
