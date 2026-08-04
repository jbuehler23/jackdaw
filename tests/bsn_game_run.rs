#![expect(
    clippy::print_stdout,
    reason = "e2e test prints the runtime proof line"
)]
//! A game built through `build_project_binary` and run as its own
//! executable loads and spawns an authored `.bsn` scene at runtime. The
//! game's stderr marker reports which authored entities reached the live
//! world.
//!
//! ```text
//! cargo test --test bsn_game_run -- --nocapture
//! ```

use std::path::PathBuf;

use jackdaw::project_build::build_project_binary;
use jackdaw::project_build::shim::ShimSpec;

mod util;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn built_game_loads_and_spawns_a_bsn_scene() {
    let game_dir = workspace_root().join("tests/fixtures/bsn_game");
    let jackdaw_dir = workspace_root().join("target/bsn_game_run");
    std::fs::create_dir_all(&jackdaw_dir).expect("create build dir");
    let spec = ShimSpec {
        package_name: "bsn_scene_game".into(),
        crate_name: "bsn_scene_game".into(),
        project_root: game_dir.clone(),
        extension_type: None,
    };
    let build = build_project_binary(&spec, &jackdaw_dir, &mut |_| {})
        .expect("build the project binary the editor way");
    let binary = build.binary;
    assert!(
        binary.exists(),
        "game binary missing at {}",
        binary.display()
    );

    // Run from the game dir so `assets/scene.bsn` resolves. BEVY_ASSET_ROOT
    // overrides the inherited CARGO_MANIFEST_DIR that bevy would otherwise use.
    let (loaded, stderr) = util::run_windowless_game(
        &binary,
        &game_dir,
        &[("BEVY_ASSET_ROOT", game_dir.as_os_str())],
    );

    assert!(
        loaded,
        "the built game never loaded the .bsn scene \
         (no `BSN_SCENE_LOADED ... has_target=true`); game stderr:\n{stderr}"
    );
    println!("ok: {stderr}");
}
