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
    for package in ["jackdaw_sdk", "jackdaw_rustc_wrapper"] {
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

/// The recipe's content hash is the SDK cache's stamp: a hash that moves
/// costs a full release rebuild of the SDK on the next launch. It must
/// move for the sources the SDK build compiles, and for nothing else.
///
/// Tests, examples and benches are separate cargo targets that `cargo
/// build -p jackdaw_sdk` never touches, and they are where most edits in
/// a library crate land, so shipping them made routine work invalidate
/// the SDK.
#[test]
fn the_recipe_ships_no_test_or_example_targets() {
    let Some(dir) = unpack("targets") else {
        eprintln!("SKIP: no embedded recipe in this build");
        return;
    };

    let mut shipped = Vec::new();
    for crate_dir in std::fs::read_dir(dir.join("crates"))
        .expect("the recipe ships crates")
        .flatten()
    {
        for unshipped in ["tests", "examples", "benches"] {
            let path = crate_dir.path().join(unshipped);
            if path.is_dir() {
                shipped.push(path);
            }
        }
    }

    assert!(
        shipped.is_empty(),
        "the recipe ships target directories the SDK build never compiles: {shipped:?}"
    );
}

/// The exclusion above is by directory name. A crate that declared an
/// explicit `[[test]]`, `[[example]]` or `[[bench]]` target could point
/// at a path outside those directories, or at one inside them that the
/// recipe no longer ships -- and cargo rejects a manifest naming a
/// target file that is not there, which aborts SDK setup entirely.
#[test]
fn no_shipped_crate_declares_an_explicit_test_or_example_target() {
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("crates");
    let mut declared = Vec::new();
    for entry in std::fs::read_dir(&crates).expect("read crates/").flatten() {
        let manifest = entry.path().join("Cargo.toml");
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        for table in ["[[test]]", "[[example]]", "[[bench]]"] {
            if text.contains(table) {
                declared.push(format!("{} declares {table}", manifest.display()));
            }
        }
    }

    assert!(
        declared.is_empty(),
        "the recipe excludes tests/, examples/ and benches/ by directory name, \
         which an explicit target declaration can defeat: {declared:?}"
    );
}

/// The hash is taken over exactly the files the recipe ships, so what
/// the recipe leaves out cannot invalidate the SDK. The editor package's
/// own sources are the case that matters day to day: they live outside
/// `crates/`, the SDK build never compiles them, and editing one must
/// leave a prepared SDK alone.
#[test]
fn the_recipe_ships_nothing_from_the_editor_package() {
    let Some(dir) = unpack("editor") else {
        eprintln!("SKIP: no embedded recipe in this build");
        return;
    };

    let allowed = ["crates", "Cargo.toml", "Cargo.lock", ".cargo"];
    let unexpected: Vec<String> = std::fs::read_dir(&dir)
        .expect("read the unpacked recipe")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !allowed.contains(&name.as_str()))
        .collect();

    assert!(
        unexpected.is_empty(),
        "the recipe root should hold only the workspace crates and their \
         manifests; the editor package's own tree must stay out of the hash: \
         {unexpected:?}"
    );
    assert!(
        !dir.join("src").exists(),
        "the editor crate's src/ must not reach the recipe"
    );
}
