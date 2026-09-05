//! An installed bundle keeps its SDK in `sdk/` beside the executable, not in
//! the dev checkout's `target/<triple>/debug` layout.

use jackdaw::sdk_paths::SdkPaths;

#[test]
fn installed_root_points_at_the_bundle_sdk_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let sdk = SdkPaths::for_installed_root(root);

    assert_eq!(sdk.manifest, root.join("sdk").join("manifest.txt"));
    assert_eq!(sdk.lockfile, root.join("Cargo.lock"));
    assert!(
        sdk.dylib.starts_with(root.join("sdk")),
        "dylib should live under the bundle's sdk dir, got {}",
        sdk.dylib.display()
    );
    assert!(
        sdk.wrapper.starts_with(root),
        "wrapper ships next to the executable"
    );
    assert!(
        !sdk.manifest
            .to_string_lossy()
            .contains("jackdaw_sdk_manifest.txt"),
        "installed layout must not use the dev manifest file name"
    );
}

/// `JACKDAW_SDK_DIR` is how a launcher points a packaged build at its SDK, so
/// it has to win over the dev-checkout probe that runs next.
#[test]
fn explicit_sdk_dir_wins_the_resolution_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    // SAFETY: nextest runs every test in its own process, so nothing else is
    // reading the environment while this runs.
    unsafe { std::env::set_var("JACKDAW_SDK_DIR", root) };
    let resolved = SdkPaths::compute();
    unsafe { std::env::remove_var("JACKDAW_SDK_DIR") };

    let expected = SdkPaths::for_installed_root(root);
    assert_eq!(
        resolved.manifest, expected.manifest,
        "JACKDAW_SDK_DIR must resolve to the installed layout"
    );
    assert_eq!(resolved.dylib, expected.dylib);
}

/// The dev layout must stay distinct, or a dev checkout would look for an
/// installed bundle that isn't there (and vice versa).
#[test]
fn installed_and_dev_layouts_differ() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let installed = SdkPaths::for_installed_root(root);
    let dev = SdkPaths::for_workspace(root);

    assert_ne!(installed.manifest, dev.manifest);
    assert_ne!(installed.dylib, dev.dylib);
    assert!(
        dev.dylib.starts_with(root.join("target")),
        "dev layout resolves under target/, got {}",
        dev.dylib.display()
    );
}
