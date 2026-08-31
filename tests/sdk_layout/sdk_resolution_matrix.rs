#![expect(clippy::print_stdout, reason = "test reports the matrix it checked")]
//! Which SDK a given layout resolves to, as a table.
//!
//! Every SDK bug found by hand this week was a resolution bug: a release
//! editor resolving a debug SDK, a manifest cached past the build it
//! described, an in-tree SDK kept in preference to a usable one. Each
//! surfaced as a rustc error naming a crate the user never wrote, nine
//! minutes into a build, because nothing checked the layout first.
//!
//! These are pure path and metadata rules, so they can be checked from
//! a directory tree in milliseconds. That is the point: the expensive
//! end-to-end builds stay for what only they can catch (feature
//! resolution, real linkage), and the layout contract is pinned here.

use std::path::{Path, PathBuf};

use jackdaw_project_build::sdk_paths::{SdkPaths, host_triple};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("jackdaw_matrix_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A checkout with an SDK built at `profile`.
fn checkout_with_sdk(root: &Path, profile: &str) {
    let triple_dir = root.join("target").join(host_triple()).join(profile);
    std::fs::create_dir_all(triple_dir.join("deps")).unwrap();
    std::fs::create_dir_all(root.join("target").join(profile).join("deps")).unwrap();
    std::fs::write(triple_dir.join(dylib_name()), b"sdk").unwrap();
    std::fs::write(triple_dir.join("jackdaw_sdk_manifest.txt"), b"").unwrap();
    std::fs::write(root.join("Cargo.lock"), b"").unwrap();
}

fn dylib_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "jackdaw_sdk.dll"
    } else if cfg!(target_os = "macos") {
        "libjackdaw_sdk.dylib"
    } else {
        "libjackdaw_sdk.so"
    }
}

/// The profile a resolved SDK belongs to, read back off its path. This
/// is the property that broke: the resolver hardcoded `debug`, so a
/// release editor pointed at debug artifacts, which cannot link because
/// cargo bakes the profile into each crate's `-C metadata`.
fn profile_of(sdk: &SdkPaths) -> String {
    sdk.dylib
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[test]
fn a_checkout_resolves_the_profile_it_was_built_at() {
    for profile in ["debug", "release"] {
        let root = scratch(profile);
        checkout_with_sdk(&root, profile);

        let sdk = SdkPaths::for_workspace_profile(&root, profile);
        assert_eq!(profile_of(&sdk), profile, "SDK profile follows the request");
        assert!(sdk.dylib_exists(), "{}", sdk.dylib.display());
        assert!(
            sdk.problems().is_empty() || sdk.problems().iter().all(|p| p.contains("wrapper")),
            "only the absent wrapper is a problem here: {:?}",
            sdk.problems()
        );
        // The host deps dir carries proc macros and must track the same
        // profile: mixing them is how a stale macro build gets linked.
        assert!(
            sdk.host_deps.starts_with(root.join("target").join(profile)),
            "host deps follow the profile too: {}",
            sdk.host_deps.display()
        );

        let _ = std::fs::remove_dir_all(&root);
    }
    println!("checked debug and release resolve to matching layouts");
}

#[test]
fn detection_prefers_release_then_falls_back_to_debug() {
    let root = scratch("detect");
    checkout_with_sdk(&root, "debug");
    let sdk = SdkPaths::for_workspace_detect(&root).expect("a debug SDK is found");
    assert_eq!(profile_of(&sdk), "debug");

    // With both present, release wins, matching what a packaged build
    // is cut from.
    checkout_with_sdk(&root, "release");
    let sdk = SdkPaths::for_workspace_detect(&root).expect("a release SDK is found");
    assert_eq!(profile_of(&sdk), "release");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_checkout_with_no_sdk_resolves_to_nothing() {
    let root = scratch("empty");
    std::fs::create_dir_all(root.join("target")).unwrap();
    assert!(
        SdkPaths::for_workspace_detect(&root).is_none(),
        "an unbuilt checkout has no SDK to offer"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `JACKDAW_SDK_DIR` is the one knob a packager sets, and pointing it at
/// any of the three shapes should work rather than only the bundle it
/// was documented for.
#[test]
fn the_override_reads_every_shape() {
    // Bundle: `sdk/` beside the executable.
    let bundle = scratch("bundle");
    let triple_dir = bundle.join("sdk").join(host_triple());
    std::fs::create_dir_all(triple_dir.join("deps")).unwrap();
    std::fs::create_dir_all(bundle.join("sdk").join("host-deps")).unwrap();
    std::fs::write(triple_dir.join(dylib_name()), b"sdk").unwrap();
    std::fs::write(bundle.join("sdk").join("manifest.txt"), b"").unwrap();
    assert!(SdkPaths::for_override_root(&bundle).dylib_exists());

    // Checkout: `target/<triple>/<profile>/`.
    let checkout = scratch("override_checkout");
    checkout_with_sdk(&checkout, "release");
    assert!(SdkPaths::for_override_root(&checkout).dylib_exists());

    // Prepared cache: a checkout one level down, under `build/`.
    let cache = scratch("override_cache");
    let build = cache.join("build");
    checkout_with_sdk(&build, "release");
    std::fs::write(build.join("Cargo.toml"), b"[workspace]\n").unwrap();
    assert!(
        SdkPaths::for_override_root(&cache).dylib_exists(),
        "a cache directory resolves through its build/ workspace"
    );

    for dir in [&bundle, &checkout, &cache] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// A path that holds no SDK at all must say what is absent rather than
/// resolving to something half-formed that fails later.
#[test]
fn an_empty_override_names_what_is_missing() {
    let root = scratch("override_empty");
    std::fs::create_dir_all(&root).unwrap();
    let problems = SdkPaths::for_override_root(&root).problems();
    assert!(
        problems.iter().any(|p| p.contains("no SDK library")),
        "{problems:?}"
    );
    assert!(
        problems.iter().any(|p| p.contains("no rustc wrapper")),
        "{problems:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
