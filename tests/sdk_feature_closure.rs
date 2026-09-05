//! The SDK must resolve at least the features a project resolves.
//!
//! Cargo resolves features per package selection, and a packaged SDK is
//! built from `jackdaw_sdk` alone while a project's graph is rooted at
//! the generated shim. Anything the project turns on that the SDK did
//! not gets compiled against SDK rlibs built without it, and the error
//! names a crate the user never mentioned: a scaffolded project pulls
//! `bevy_math/approx` through the physics integration, and an SDK whose
//! `glam` lacks `approx` fails with missing trait impls inside
//! `bevy_math`.
//!
//! Checking the real resolution needs a full project graph. Checking the
//! manifests is cheap, catches the way this actually regresses (a
//! feature added to the runtime and not to the SDK), and runs offline.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn manifest(relative: &str) -> toml::Value {
    let path = workspace_root().join(relative);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    text.parse()
        .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// Every feature a crate declares, minus `default` (implied) and the
/// implicit `dep:`-style optional-dependency features, which are not
/// separately selectable in the way this test means.
fn declared_features(relative: &str) -> BTreeSet<String> {
    manifest(relative)
        .get("features")
        .and_then(toml::Value::as_table)
        .map(|table| {
            table
                .keys()
                .filter(|name| *name != "default")
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// The features `jackdaw_sdk` requests of a dependency.
fn sdk_requests(dependency: &str) -> BTreeSet<String> {
    let sdk = manifest("crates/jackdaw_sdk/Cargo.toml");
    let entry = sdk
        .get("dependencies")
        .and_then(|d| d.get(dependency))
        .unwrap_or_else(|| {
            panic!("jackdaw_sdk must depend on {dependency}; see this test's module docs")
        });
    entry
        .get("features")
        .and_then(toml::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(toml::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn the_sdk_enables_every_runtime_feature() {
    let declared = declared_features("crates/jackdaw_runtime/Cargo.toml");
    let requested = sdk_requests("jackdaw_runtime");

    let missing: Vec<&String> = declared.difference(&requested).collect();
    assert!(
        missing.is_empty(),
        "jackdaw_sdk does not enable jackdaw_runtime features {missing:?}. A project that \
         enables one of them resolves shared crates with features the SDK was built without, \
         and fails to compile against the SDK's rlibs. Add them to the `features` list in \
         crates/jackdaw_sdk/Cargo.toml."
    );
}

/// The SDK is what a project links; a project reaches the runtime
/// through it, so the dependency has to be a real one rather than
/// optional or dev-only.
#[test]
fn the_sdk_depends_on_the_runtime_unconditionally() {
    let sdk = manifest("crates/jackdaw_sdk/Cargo.toml");
    let entry = sdk
        .get("dependencies")
        .and_then(|d| d.get("jackdaw_runtime"))
        .expect("jackdaw_sdk depends on jackdaw_runtime");
    assert!(
        entry.get("optional").and_then(toml::Value::as_bool) != Some(true),
        "the runtime dependency must not be optional: a packaged SDK is built from this \
         crate's default resolution"
    );
}

/// The recipe a source-free install builds its SDK from must contain
/// every crate that closure now reaches, or the bootstrap build fails
/// on a missing path dependency.
#[test]
fn the_recipe_ships_the_runtime_closure() {
    let recipe = workspace_root().join("crates/jackdaw_project_build/build.rs");
    assert!(recipe.is_file(), "recipe assembler is where it was");

    // Path dependencies of the runtime have to exist as crates the
    // recipe copies. `crates/*` is the recipe's member glob, so the
    // check is that each lives there rather than outside the workspace.
    for dependency in path_dependencies("crates/jackdaw_runtime/Cargo.toml") {
        let path = workspace_root().join("crates").join(&dependency);
        assert!(
            path.join("Cargo.toml").is_file(),
            "jackdaw_runtime depends on `{dependency}`, which is not under crates/ and so is \
             not in the SDK recipe"
        );
    }
}

/// Names of a manifest's path dependencies, as directory names under
/// `crates/`.
fn path_dependencies(relative: &str) -> Vec<String> {
    let value = manifest(relative);
    let Some(table) = value.get("dependencies").and_then(toml::Value::as_table) else {
        return Vec::new();
    };
    table
        .values()
        .filter_map(|entry| entry.get("path").and_then(toml::Value::as_str))
        .filter_map(|path| Path::new(path).file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .collect()
}
