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
use std::process::Command;

use jackdaw::project_build::build_project_dylib;
use jackdaw::project_build::shim::ShimSpec;
use jackdaw::sdk_paths::SdkPaths;

mod util;

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

    // Run from the game dir so `assets/scene.bsn` resolves. BEVY_ASSET_ROOT
    // overrides the inherited CARGO_MANIFEST_DIR that bevy would otherwise use.
    let (loaded, stderr) = util::run_windowless_game(
        &sdk.runner,
        &dylib,
        &game_dir,
        &[("BEVY_ASSET_ROOT", game_dir.as_os_str())],
    );

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
