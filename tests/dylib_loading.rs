//! Integration coverage for the dynamic-extension load path.
//!
//! Builds `tests/fixtures/test_fixture_extension` as a cdylib and
//! drives `jackdaw_loader::load_from_path` against it, mirroring the
//! editor's runtime install flow from a headless test.
//!
//! ## Scope
//!
//! These tests cover the **loader's job**: dlopen, ABI version
//! check, catalog registration, library-handle retention, error
//! paths. They do **not** cover invoking operators or querying
//! components from the loaded extension: without a shared compile
//! graph between host and cdylib, the two sides get separate static
//! copies of bevy and `jackdaw_api_internal`, so `TypeId` and
//! `ComponentId` don't unify across the boundary. Operator-dispatch
//! coverage belongs in a follow-up test harness once the per-project
//! editor binary architecture is in place.

use std::{mem::ManuallyDrop, path::PathBuf};

use bevy::prelude::*;
use jackdaw_api_internal::lifecycle::ExtensionCatalog;
use jackdaw_loader::{LoadError, LoadedDylibs, LoadedKind, load_from_path, peek_kind};

mod util;

/// Resolve the path to the fixture cdylib produced by cargo as part
/// of the workspace test-target build.
///
/// `test_fixture_extension` is a `dev-dependency` of the root
/// `jackdaw` crate (see `Cargo.toml`), so cargo compiles its cdylib
/// before any test binary runs. Cargo drops the `.so` in
/// `target/<profile>/deps/` and (when a top-level build is what
/// drove the compile) also copies it to `target/<profile>/`. CI's
/// nextest-only invocation skips the top-level copy, so we check
/// the `deps/` location first.
fn fixture_path() -> PathBuf {
    let target_root = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target"));
    let profile_dir = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let filename = format!(
        "{}test_fixture_extension{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX,
    );
    let profile_root = target_root.join(profile_dir);
    let candidates = [
        profile_root.join("deps").join(&filename),
        profile_root.join(&filename),
    ];
    for candidate in &candidates {
        if candidate.exists() {
            return candidate.clone();
        }
    }
    panic!(
        "fixture artifact missing. Checked {} and {}. If running a trimmed \
         test harness, `cargo build -p test_fixture_extension --lib` first.",
        candidates[0].display(),
        candidates[1].display(),
    );
}

/// Headless `App` with an empty [`DylibLoaderPlugin`] wired in so
/// [`LoadedDylibs`] exists but no on-disk directory is scanned.
/// Tests drive loading explicitly via [`load_from_path`].
fn headless_app_with_empty_dylib_loader() -> LeakyApp {
    let app = util::headless_app();
    LeakyApp(ManuallyDrop::new(app))
}

/// Skip the `App`'s destructor. Dropping an `App` that holds a
/// cdylib-loaded extension runs [`LoadedDylibs`]' `Drop` (`dlclose`)
/// at an indeterminate moment relative to the `Extension` entity
/// that stores a trait object whose vtable lives inside that
/// library. If the library unloads first, the vtable drop-glue
/// segfaults. Leaking is harmless in a test binary — the OS reclaims
/// everything at process exit.
#[derive(Deref, DerefMut)]
struct LeakyApp(ManuallyDrop<App>);

impl Drop for LeakyApp {
    fn drop(&mut self) {
        // intentionally don't call `std::mem::drop(self.0)`!
    }
}

#[test]
fn peek_kind_classifies_extension() {
    let path = fixture_path();
    let kind = peek_kind(&path).expect("peek_kind should succeed on a valid fixture");
    match kind {
        LoadedKind::Extension(name) => assert_eq!(name, "test_fixture"),
        other => panic!("expected Extension, got {other:?}"),
    }
}

#[test]
fn load_from_path_registers_extension() {
    let path = fixture_path();
    let mut app = headless_app_with_empty_dylib_loader();
    app.finish();
    app.update();

    assert_eq!(app.world().resource::<LoadedDylibs>().len(), 0);

    let kind = load_from_path(app.world_mut(), &path).expect("load should succeed");
    assert!(matches!(kind, LoadedKind::Extension(ref n) if n == "test_fixture"));

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
