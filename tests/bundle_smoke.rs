#![cfg(feature = "dylib")]
#![expect(clippy::print_stdout, reason = "e2e test reports what it staged")]
//! Stages a bundle with `cargo xtask bundle`, resolves the SDK from the bundle
//! layout rather than the dev tree, and builds a game against it: the path an
//! installed user takes.
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

fn bundle_has_runtime(bundle: &Path, crate_name: &str) -> bool {
    let prefix = format!("{}{}", std::env::consts::DLL_PREFIX, crate_name);
    std::fs::read_dir(bundle).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            let path = entry.path();
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix))
                && path
                    .extension()
                    .is_some_and(|ext| ext == std::env::consts::DLL_EXTENSION)
        })
    })
}

fn run_jd(jd: &Path, current_dir: &Path, args: &[&str]) -> std::process::Output {
    // `cargo test` puts the workspace build tree on the loader path and
    // children inherit it, which masks a staged binary missing its own
    // runpath. Strip it so the staged binary resolves its dylibs the way
    // it will on a user's machine.
    Command::new(jd)
        .args(args)
        .env_remove("JACKDAW_DEV_CHECKOUT")
        .env_remove("LD_LIBRARY_PATH")
        .env_remove("DYLD_LIBRARY_PATH")
        .env_remove("DYLD_FALLBACK_LIBRARY_PATH")
        .current_dir(current_dir)
        .output()
        .expect("run the staged jd binary")
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

    // Not the system temp dir: a bundle is ~3GB and `/tmp` is often a tmpfs.
    let staging = tempfile::Builder::new()
        .prefix("bundle-smoke-")
        .tempdir_in(root.join("target"))
        .expect("tempdir under target/");
    let bundle = match std::env::var_os("JACKDAW_BUNDLE_ROOT") {
        Some(path) => PathBuf::from(path),
        None => {
            // `xtask` is deliberately outside the workspace, so it is only
            // reachable through its own manifest. `-p xtask` from the root
            // resolves nothing, which is what the `cargo xtask` alias avoids
            // by passing `--manifest-path`.
            let status = Command::new("cargo")
                .args(["build", "--release", "--manifest-path", "xtask/Cargo.toml"])
                .current_dir(&root)
                .status()
                .expect("spawn cargo for xtask");
            assert!(status.success(), "xtask failed to build");

            let bundle = staging.path().join("jackdaw-bundle");
            // Its own manifest means its own target dir, not the workspace's.
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
    assert!(sdk.runner.is_file(), "bundle is missing the game runner");
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

    // Run the shipped CLI from the staged bundle, outside the source
    // checkout. This is the same embedded-template path the editor launcher
    // calls for New Game/New Extension, plus the same import planner it uses
    // for an existing Bevy project.
    let jd = bundle.join(format!("jd{}", std::env::consts::EXE_SUFFIX));
    assert!(jd.is_file(), "bundle is missing the jd binary");
    let user_projects = staging.path().join("standalone-projects");
    std::fs::create_dir_all(&user_projects).expect("create standalone project root");

    for (name, extension) in [("bundle-game", false), ("bundle-extension", true)] {
        let mut args = vec!["new", name, "--no-git", "--path"];
        let project_root = user_projects.to_string_lossy().into_owned();
        args.push(&project_root);
        if extension {
            args.push("--extension");
        }
        let output = run_jd(&jd, &user_projects, &args);
        assert!(
            output.status.success(),
            "staged jd failed to scaffold {name}:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let manifest = std::fs::read_to_string(user_projects.join(name).join("Cargo.toml"))
            .expect("read scaffolded manifest");
        assert!(
            !manifest.contains("path ="),
            "a standalone bundle scaffold must not point back at the checkout:\n{manifest}"
        );
    }

    let imported = user_projects.join("existing-bevy-game");
    std::fs::create_dir_all(imported.join("src")).expect("create imported project");
    std::fs::write(
        imported.join("Cargo.toml"),
        format!(
            "[package]\nname = \"existing-bevy-game\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nbevy = \"{}\"\n\n[workspace]\n",
            jackdaw_project_build::BEVY_VERSION
        ),
    )
    .expect("write imported manifest");
    std::fs::write(
        imported.join("src/lib.rs"),
        "use bevy::prelude::*;\npub struct ExistingPlugin;\nimpl Plugin for ExistingPlugin { fn build(&self, _: &mut App) {} }\n",
    )
    .expect("write imported lib");
    let imported_path = imported.to_string_lossy().into_owned();
    let output = run_jd(&jd, &user_projects, &["import", "--apply", &imported_path]);
    assert!(
        output.status.success(),
        "staged jd failed to import an existing project:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        imported.join("jackdaw.toml").is_file(),
        "import did not create jackdaw.toml"
    );

    // `None` workspace root: an installed bundle has no checkout to fall back on.
    let build_dir = staging.path().join("build");
    std::fs::create_dir_all(&build_dir).expect("create build dir");
    let spec = ShimSpec {
        package_name: "bsn_scene_game".into(),
        crate_name: "bsn_scene_game".into(),
        project_root: root.join("tests/fixtures/bsn_game"),
        game_plugin: Some("GamePlugin".into()),
        extension_type: Some("BundleFixtureExtension".into()),
    };
    let build = build_project_dylib(&spec, &build_dir, &sdk, None, &mut |_| {})
        .expect("build the fixture game against the bundle SDK");
    assert!(
        build.dylib.exists(),
        "game dylib missing at {}",
        build.dylib.display()
    );

    // The editor dlopens a shim with the staged facade already resident, so
    // the shim's `@rpath/libjackdaw_sdk.dylib` reference binds to the loaded
    // image by install name before dyld ever searches a path. This harness
    // is a cargo test binary whose baked-in rpaths point into the workspace
    // build tree, where a later build leaves a stable-named facade from a
    // different resolution missing the shim's symbols. Preload the staged
    // runtime chain by absolute path, dependencies first, so every `@rpath`
    // reference resolves by install-name match instead of path search.
    if cfg!(target_os = "macos") {
        let mut chain: Vec<PathBuf> = Vec::new();
        for prefix in ["libstd", "libbevy_dylib", "libjackdaw_dylib"] {
            let entries = std::fs::read_dir(&bundle).expect("read bundle root");
            let staged: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension().is_some_and(|ext| ext == "dylib")
                        && path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| name.starts_with(prefix))
                })
                .collect();
            assert!(!staged.is_empty(), "no {prefix}* dylib at the bundle root");
            chain.extend(staged);
        }
        chain.push(sdk.dylib.clone());
        for dylib in chain {
            let loaded = unsafe { libloading::Library::new(&dylib) }
                .unwrap_or_else(|err| panic!("preload staged {}: {err}", dylib.display()));
            std::mem::forget(loaded);
        }
    }

    // A marketplace extension is useful only if the installed editor can
    // load it and receive its trait object through the shared Jackdaw ABI.
    // Keep the library loaded until after the object is dropped because its
    // vtable lives in the project dylib.
    type ExtensionCtor = fn() -> Box<dyn jackdaw_api::JackdawExtension>;
    let library = unsafe { libloading::Library::new(&build.dylib) }
        .expect("load the independently built project/extension dylib");
    let extension = unsafe {
        let ctor: libloading::Symbol<'_, ExtensionCtor> = library
            .get(b"jackdaw_extension_ctor\0")
            .expect("project dylib exports the extension constructor");
        ctor()
    };
    assert_eq!(extension.id(), "bundle_fixture");
    drop(extension);
    std::mem::forget(library);

    println!(
        "BUNDLE SMOKE PASS: standalone scaffold/import and game/extension dylib {}",
        build.dylib.display()
    );
}
