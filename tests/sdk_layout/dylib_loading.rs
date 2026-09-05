//! Integration coverage for the dynamic-extension load path: builds
//! `tests/fixtures/test_fixture_extension` as a cdylib and drives
//! `jackdaw_loader::load_from_path` against it.
//!
//! These cover the loader's job (dlopen, ctor lookup, catalog registration,
//! handle retention, error paths), not invoking operators from the loaded
//! extension: without the `dylib` feature and the `jackdaw_sdk` proxy dylib,
//! host and cdylib get separate static copies of bevy, so `TypeId` and
//! `ComponentId` do not unify across the boundary.

use crate::util;

use std::{
    mem::ManuallyDrop,
    path::{Path, PathBuf},
};

use bevy::prelude::*;
use jackdaw_api_internal::lifecycle::ExtensionCatalog;
use jackdaw_loader::{LoadError, LoadedDylibs, load_from_path};

/// Resolve the path to the fixture cdylib cargo produced. The fixture is a
/// dev-dependency of the root crate, so its cdylib lands in the same `deps/`
/// directory as this test binary (including under an explicit `--target`);
/// resolving relative to `current_exe` therefore picks this session's copy.
/// `target/<profile>/` is a fallback for harnesses that copied it top-level.
fn fixture_path() -> PathBuf {
    let filename = format!(
        "{}test_fixture_extension{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX,
    );
    let exe_deps_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let mut candidates = Vec::new();
    if let Some(deps) = &exe_deps_dir {
        candidates.push(deps.join(&filename));
        if let Some(profile_root) = deps.parent() {
            candidates.push(profile_root.join(&filename));
        }
    }
    for candidate in &candidates {
        if candidate.exists() {
            return candidate.clone();
        }
    }
    panic!(
        "fixture artifact missing. Checked: {}. If running a trimmed test \
         harness, `cargo build -p test_fixture_extension --lib` first.",
        candidates
            .iter()
            .map(|c| c.display().to_string())
            .collect::<Vec<_>>()
            .join(", "),
    );
}

/// Headless `App` with an empty `DylibLoaderPlugin` wired in so
/// `LoadedDylibs` exists but no on-disk directory is scanned.
/// Tests drive loading explicitly via `load_from_path`.
fn headless_app_with_empty_dylib_loader() -> LeakyApp {
    let app = util::headless_app();
    LeakyApp(ManuallyDrop::new(app))
}

/// Skip the `App`'s destructor. Dropping an `App` holding a cdylib-loaded
/// extension runs `LoadedDylibs`' `dlclose` at an indeterminate moment relative
/// to the `Extension` entity whose vtable lives inside that library, and if the
/// library unloads first the drop glue segfaults.
#[derive(Deref, DerefMut)]
struct LeakyApp(ManuallyDrop<App>);

impl Drop for LeakyApp {
    fn drop(&mut self) {
        // intentionally don't call `std::mem::drop(self.0)`!
    }
}

#[test]
fn load_from_path_registers_extension() {
    let path = fixture_path();
    let mut app = headless_app_with_empty_dylib_loader();
    app.finish();
    app.update();

    assert_eq!(app.world().resource::<LoadedDylibs>().len(), 0);

    let id = load_from_path(app.world_mut(), &path).expect("load should succeed");
    assert_eq!(id, "test_fixture");

    let catalog = app.world().resource::<ExtensionCatalog>();
    assert!(
        catalog.contains("test_fixture"),
        "fixture extension missing from catalog after load"
    );
    assert_eq!(app.world().resource::<LoadedDylibs>().len(), 1);
}

#[test]
fn repeat_load_is_idempotent() {
    let path = fixture_path();
    let mut app = headless_app_with_empty_dylib_loader();
    app.finish();
    app.update();

    load_from_path(app.world_mut(), &path).expect("first load should succeed");
    load_from_path(app.world_mut(), &path).expect("second load should succeed");

    // Catalog entry is a singleton per name. The loader checks
    // `contains()` on the second call and skips re-registration
    // rather than failing.
    let catalog = app.world().resource::<ExtensionCatalog>();
    let count = catalog.iter().filter(|n| *n == "test_fixture").count();
    assert_eq!(count, 1, "catalog should hold exactly one entry");

    // Both library handles are retained so any live function
    // pointers from either copy stay callable.
    assert_eq!(app.world().resource::<LoadedDylibs>().len(), 2);
}

#[test]
fn missing_file_is_libloading_error() {
    // `load_from_path` early-returns before touching `LoadedDylibs`
    // for dlopen failures, so the base `headless_app()` (no
    // DylibLoaderPlugin) is enough here. No dylib was loaded; the
    // App can drop normally.
    let mut app = util::headless_app();
    let err = load_from_path(
        app.world_mut(),
        &PathBuf::from("/nonexistent/definitely-not-a-real.so"),
    )
    .expect_err("loading a nonexistent path should fail");
    assert!(
        matches!(err, LoadError::Libloading(_)),
        "expected LoadError::Libloading, got {err:?}"
    );
}
