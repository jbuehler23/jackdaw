#![cfg(feature = "dylib")]
#![expect(clippy::print_stdout, reason = "e2e test reports what it staged")]
//! Stages a bundle with `cargo xtask bundle`, resolves the SDK from the bundle
//! layout rather than the dev tree, and builds a game against it: the path an
//! installed user takes.
//!
//! Requires a release SDK, which the bundle is cut from.

use std::path::PathBuf;
use std::process::Command;

use jackdaw::project_build::build_project_dylib;
use jackdaw::project_build::shim::ShimSpec;
use jackdaw::sdk_paths::SdkPaths;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn game_builds_against_a_staged_bundle_sdk() {
    let root = workspace_root();
    let release_sdk = SdkPaths::for_workspace_profile(&root, "release");
    if !release_sdk.dylib_exists() {
        let msg = format!(
            "no release SDK at {}. Build one with \
             `cargo build -p jackdaw --features dylib --release --target {}`.",
            release_sdk.dylib.display(),
            release_sdk.triple
        );
        // In a release/CI context this test must actually run; a silent skip
        // would let a broken bundle ship. Locally, without a release build, it
        // skips so a plain `cargo test` stays cheap.
        assert!(
            std::env::var_os("JACKDAW_BUNDLE_SMOKE_REQUIRED").is_none(),
            "bundle_smoke was required but {msg}"
        );
        println!("SKIP bundle_smoke: {msg}");
        return;
    }

    let status = Command::new("cargo")
        .args(["build", "-p", "xtask", "--release"])
        .current_dir(&root)
        .status()
        .expect("spawn cargo for xtask");
    assert!(status.success(), "xtask failed to build");

    // Not the system temp dir: a bundle is ~3GB and `/tmp` is often a tmpfs.
    let staging = tempfile::Builder::new()
        .prefix("bundle-smoke-")
        .tempdir_in(root.join("target"))
        .expect("tempdir under target/");
    let bundle = staging.path().join("jackdaw-bundle");
    let xtask = root
        .join("target/release")
        .join(format!("xtask{}", std::env::consts::EXE_SUFFIX));
    let staged = Command::new(&xtask)
        .arg("bundle")
        .arg("--out")
        .arg(&bundle)
        .arg("--workspace")
        .arg(&root)
        .output()
        .expect("spawn cargo xtask bundle");
    assert!(
        staged.status.success(),
        "bundle staging failed:\n{}",
        String::from_utf8_lossy(&staged.stderr)
    );

    // Resolved the way an installed editor does, from the bundle root.
    let sdk = SdkPaths::for_installed_root(&bundle);
    assert!(
        sdk.manifest.is_file(),
        "bundle is missing {}",
        sdk.manifest.display()
    );
    assert!(sdk.wrapper.is_file(), "bundle is missing the rustc wrapper");
    assert!(sdk.runner.is_file(), "bundle is missing the game runner");
    assert!(sdk.dylib_exists(), "bundle is missing the SDK dylib");
    assert!(sdk.lockfile.is_file(), "bundle is missing Cargo.lock");

    // `None` workspace root: an installed bundle has no checkout to fall back on.
    let build_dir = staging.path().join("build");
    std::fs::create_dir_all(&build_dir).expect("create build dir");
    let spec = ShimSpec {
        package_name: "bsn_scene_game".into(),
        crate_name: "bsn_scene_game".into(),
        project_root: root.join("tests/fixtures/bsn_game"),
        game_plugin: Some("GamePlugin".into()),
        extension_type: None,
    };
    let build = build_project_dylib(&spec, &build_dir, &sdk, None, &mut |_| {})
        .expect("build the fixture game against the bundle SDK");
    assert!(
        build.dylib.exists(),
        "game dylib missing at {}",
        build.dylib.display()
    );

    println!(
        "BUNDLE SMOKE PASS: staged a bundle and built {} against its SDK",
        build.dylib.display()
    );
}
