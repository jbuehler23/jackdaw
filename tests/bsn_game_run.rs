#![cfg(feature = "dylib")]
#![expect(
    clippy::print_stdout,
    reason = "e2e test prints the runtime proof line"
)]
//! A game built through `build_project_dylib` and run via `jackdaw-runner`
//! loads and spawns an authored `.bsn` scene at runtime. The game's stderr
//! marker reports which authored entities reached the live world.
//!
//! ```text
//! cargo test --features "dylib runner" --target <host-triple> \
//!     --test bsn_game_run -- --nocapture
//! ```

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use jackdaw::project_build::build_project_dylib;
use jackdaw::project_build::shim::ShimSpec;
use jackdaw::sdk_paths::SdkPaths;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn built_game_loads_and_spawns_a_bsn_scene() {
    let sdk = SdkPaths::for_workspace(&workspace_root());
    let triple = sdk.triple.clone();
    assert!(
        sdk.wrapper_exists(),
        "SDK wrapper missing; build --features dylib --target {triple}"
    );

    // Prebuilt in production; here it shares the SDK graph.
    let status = Command::new("cargo")
        .args(["build", "-p", "jackdaw_runner", "--target", &triple])
        .current_dir(workspace_root())
        .status()
        .expect("build jackdaw-runner");
    assert!(status.success(), "runner build failed");

    let game_dir = workspace_root().join("tests/fixtures/bsn_game");
    // Not the committed fixture, and not the system temp dir (often a tmpfs).
    let jackdaw_dir = workspace_root().join("target/bsn_game_run");
    std::fs::create_dir_all(&jackdaw_dir).expect("create build dir");
    let spec = ShimSpec {
        package_name: "bsn_scene_game".into(),
        crate_name: "bsn_scene_game".into(),
        project_root: game_dir.clone(),
        game_plugin: Some("GamePlugin".into()),
        extension_type: None,
    };
    let build = build_project_dylib(
        &spec,
        &jackdaw_dir,
        &sdk,
        Some(&workspace_root()),
        &mut |_| {},
    )
    .expect("build the project dylib the editor way");
    let dylib = build.dylib;
    assert!(dylib.exists(), "game dylib missing at {}", dylib.display());

    // Run from the game dir so `assets/scene.bsn` resolves.
    let (handle, server_name) = jackdaw_pie_protocol::serve().expect("open the ipc rendezvous");
    let mut child = Command::new(&sdk.runner)
        .arg(&dylib)
        .current_dir(&game_dir)
        // Overrides the inherited CARGO_MANIFEST_DIR, which bevy would
        // otherwise treat as the asset root.
        .env("BEVY_ASSET_ROOT", &game_dir)
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

    // The game's JackdawPlugin connects on boot; accept so it does not block.
    let (accept_tx, accept_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = accept_tx.send(handle.accept());
    });
    let _transport = accept_rx
        .recv_timeout(Duration::from_secs(90))
        .expect("the runner never connected to the PIE link")
        .expect("ipc accept failed");

    // Wait for the game to load the `.bsn` and report the authored probe.
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
        "the built game never loaded the .bsn scene \
         (no `BSN_SCENE_LOADED ... has_target=true`); runner stderr:\n{stderr}"
    );

    for line in stderr.lines() {
        if line.contains("BSN_SCENE_LOADED") {
            println!("E2E PASS (built game via runner loaded the .bsn scene): {line}");
        }
    }
}
