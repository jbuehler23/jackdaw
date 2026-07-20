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
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

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
    let (handle, server_name) = jackdaw_pie_protocol::serve().expect("open the ipc rendezvous");
    let mut child = Command::new(&sdk.runner)
        .arg(&build.dylib)
        .current_dir(stage.path())
        .env("BEVY_ASSET_ROOT", stage.path())
        .env("JACKDAW_E2E_NODE_ID", minted.to_string())
        .env("JACKDAW_PIE", &server_name)
        .env("JACKDAW_PIE_WINDOWLESS", "1")
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the runner");

    let child_stderr = child.stderr.take().expect("piped stderr");
    let stderr_buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    {
        let stderr_buf = std::sync::Arc::clone(&stderr_buf);
        std::thread::spawn(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(child_stderr);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let mut buf = stderr_buf.lock().unwrap();
                buf.push_str(&line);
                buf.push('\n');
            }
        });
    }

    let (accept_tx, accept_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = accept_tx.send(handle.accept());
    });
    let _transport = accept_rx
        .recv_timeout(Duration::from_secs(90))
        .expect("the runner never connected to the PIE link")
        .expect("ipc accept failed");

    let deadline = Instant::now() + Duration::from_secs(90);
    let mut loaded = false;
    while Instant::now() < deadline {
        {
            let buf = stderr_buf.lock().unwrap();
            if buf.contains("BSN_SCENE_LOADED") && buf.contains("has_target=true") {
                loaded = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let _ = child.kill();
    let _ = child.wait();
    let stderr = stderr_buf.lock().unwrap().clone();

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
