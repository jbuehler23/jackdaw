#![expect(clippy::print_stderr, reason = "test prints skip diagnostics")]
//! The embedded SDK recipe must be a workspace cargo can actually load.
//!
//! Nothing else checks this. The recipe is assembled by a build script,
//! embedded as bytes, and only unpacked on a user's machine at first
//! run, so a malformed one compiles and tests clean here and then fails
//! on the user's very first launch with no SDK and therefore no ability
//! to build any project at all.
//!
//! That is not hypothetical: adding a workspace crate that depends on
//! the editor package produced exactly that. The recipe ships the
//! library crates but deliberately not the editor, so the new crate's
//! path dependency pointed at a root that is only a virtual manifest,
//! and cargo rejected the whole workspace before compiling anything.

use std::path::PathBuf;
use std::process::Command;

/// Unpack the embedded recipe into a scratch directory of its own.
/// `name` keeps concurrently-running tests from sharing (and deleting)
/// one another's copy.
fn unpack(name: &str) -> Option<PathBuf> {
    if !jackdaw_project_build::bootstrap::recipe_is_embedded() {
        return None;
    }
    // Not the system temp dir: the recipe is the whole library source
    // tree, and /tmp is often a small tmpfs.
    let root = PathBuf::from("/var/tmp");
    let dir = root.join(format!("jackdaw_recipe_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    jackdaw_project_build::bootstrap::write_recipe(&dir).expect("unpack the embedded recipe");
    Some(dir)
}

#[test]
fn the_embedded_recipe_is_a_loadable_workspace() {
    let Some(dir) = unpack("workspace") else {
        // A build without `embed-recipe` has nothing to check. CI builds
        // the editor with default features, which include it.
        assert!(
            std::env::var_os("JACKDAW_RECIPE_REQUIRED").is_none(),
            "the recipe check was required but no recipe is embedded"
        );
        eprintln!("SKIP: no embedded recipe in this build");
        return;
    };

    // Pin the toolchain the way `ensure_sdk` does, so this resolves the
    // same way the real bootstrap will.
    std::fs::write(
        dir.join("rust-toolchain.toml"),
        format!(
            "[toolchain]\nchannel = \"{}\"\n",
            jackdaw_project_build::bootstrap::SDK_TOOLCHAIN_CHANNEL
        ),
    )
    .expect("write rust-toolchain.toml");

    let output = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(&dir)
        .output()
        .expect("run cargo metadata");
    assert!(
        output.status.success(),
        "the unpacked recipe is not a loadable workspace:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Unpacking is otherwise additive, so a crate dropped between
/// versions would linger as a workspace member and keep breaking the
/// cache after the fix that removed it had already shipped.
#[test]
fn unpacking_removes_a_crate_the_recipe_no_longer_ships() {
    let Some(dir) = unpack("prune") else {
        eprintln!("SKIP: no embedded recipe in this build");
        return;
    };
    // Stand in for a crate an older jackdaw shipped and this one does not.
    let stale = dir.join("crates/jackdaw_gone");
    std::fs::create_dir_all(&stale).expect("create the stale crate");
    std::fs::write(stale.join("Cargo.toml"), "[package]\nname = \"gone\"\n").expect("write");

    jackdaw_project_build::bootstrap::write_recipe(&dir).expect("re-unpack");
    assert!(
        !stale.exists(),
        "a crate the recipe no longer ships must not survive an unpack"
    );
    // And the crates it does ship are still there.
    assert!(dir.join("crates/jackdaw_sdk/Cargo.toml").is_file());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The SDK build asks for these three packages by name. If any is
/// missing from the recipe, setup fails after the toolchain download
/// rather than before it.
#[test]
fn the_recipe_contains_every_package_the_sdk_build_names() {
    let Some(dir) = unpack("packages") else {
        eprintln!("SKIP: no embedded recipe in this build");
        return;
    };
    for package in ["jackdaw_sdk", "jackdaw_runner", "jackdaw_rustc_wrapper"] {
        assert!(
            dir.join("crates")
                .join(package)
                .join("Cargo.toml")
                .is_file(),
            "the SDK build runs `-p {package}`, so the recipe must ship it"
        );
    }
    // And not the editor, whose absence is the reason the root is a
    // virtual manifest in the first place.
    assert!(
        !dir.join("crates/jackdaw_editor").exists(),
        "crates depending on the editor package cannot resolve in the recipe"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
