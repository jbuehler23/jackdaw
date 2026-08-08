#![cfg(feature = "dylib")]
#![expect(clippy::print_stdout, reason = "e2e test reports what it staged")]
//! Stages a bundle with `cargo xtask bundle`, resolves the SDK from the bundle
//! layout rather than the dev tree, and builds an extension against it: the
//! path an installed user takes for marketplace dylibs.
//!
//! Requires a release SDK, which the bundle is cut from.

use std::path::{Path, PathBuf};
use std::process::Command;

use jackdaw::project_build::build_project_dylib;
use jackdaw::project_build::shim::ShimSpec;
use jackdaw::sdk_paths::SdkPaths;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn bundle_has_runtime(bundle: &Path, crate_id: &str) -> bool {
    let prefix = format!("{}{crate_id}", std::env::consts::DLL_PREFIX);
    let suffix = std::env::consts::DLL_SUFFIX;
    std::fs::read_dir(bundle)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with(&prefix) && name.ends_with(suffix)
        })
}

#[test]
fn extension_builds_against_a_staged_bundle_sdk() {
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

    // Not the system temp dir: a bundle is ~3GB and `/tmp` is often a tmpfs.
    let staging = tempfile::Builder::new()
        .prefix("bundle-smoke-")
        .tempdir_in(root.join("target"))
        .expect("tempdir under target/");
    let bundle = match std::env::var_os("JACKDAW_BUNDLE_ROOT") {
        Some(path) => PathBuf::from(path),
        None => {
            // `xtask` is deliberately outside the workspace, so it is only
            // reachable through its own manifest.
            let status = Command::new("cargo")
                .args(["build", "--release", "--manifest-path", "xtask/Cargo.toml"])
                .current_dir(&root)
                .status()
                .expect("spawn cargo for xtask");
            assert!(status.success(), "xtask failed to build");

            let bundle = staging.path().join("jackdaw-bundle");
            let xtask = root
                .join("xtask/target/release")
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
            bundle
        }
    };

    // Resolved the way an installed editor does, from the bundle root.
    let sdk = SdkPaths::for_installed_root(&bundle);
    assert!(
        sdk.manifest.is_file(),
        "bundle is missing {}",
        sdk.manifest.display()
    );
    assert!(sdk.wrapper.is_file(), "bundle is missing the rustc wrapper");
    assert!(sdk.dylib_exists(), "bundle is missing the SDK dylib");
    assert!(sdk.lockfile.is_file(), "bundle is missing Cargo.lock");
    assert!(
        bundle_has_runtime(&bundle, "bevy_dylib"),
        "bundle is missing the shared Bevy runtime"
    );
    assert!(
        bundle_has_runtime(&bundle, "jackdaw_dylib"),
        "bundle is missing the shared Jackdaw runtime"
    );

    // Build an extension dylib against the staged SDK.
    let build_dir = staging.path().join("build");
    std::fs::create_dir_all(&build_dir).expect("create build dir");
    let extension_dir = staging.path().join("ext");
    std::fs::create_dir_all(extension_dir.join("src")).expect("create extension dir");
    std::fs::write(
        extension_dir.join("Cargo.toml"),
        format!(
            r#"[package]
            name = "bundle_smoke_ext"
            version = "0.1.0"
            edition = "2024"
            publish = false

            [workspace]

            [dependencies]
            bevy = {{ version = "0.19", default-features = false }}
            jackdaw_extension = {{ path = "{}" }}
            "#,
            root.join("crates/jackdaw_extension")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .expect("write extension Cargo.toml");
    std::fs::write(
        extension_dir.join("src/lib.rs"),
        r#"use bevy::prelude::*;
            use jackdaw_extension::prelude::*;

            #[derive(Default)]
            pub struct SmokeExtension;

            impl JackdawExtension for SmokeExtension {
                fn id(&self) -> String { "bundle_smoke".into() }
                fn register(&self, _: &mut ExtensionRegistrar<'_>) {}
            }
            "#,
    )
    .expect("write extension lib");
    let spec = ShimSpec {
        package_name: "bundle_smoke_ext".into(),
        crate_name: "bundle_smoke_ext".into(),
        project_root: extension_dir,
        extension_type: Some("SmokeExtension".into()),
    };
    let build = build_project_dylib(&spec, &build_dir, &sdk, None, &mut |_| {})
        .expect("build an extension dylib against the bundle SDK");
    assert!(
        build.dylib.exists(),
        "extension dylib missing at {}",
        build.dylib.display()
    );

    println!(
        "BUNDLE SMOKE PASS: staged a bundle and built {} against its SDK",
        build.dylib.display()
    );
}
