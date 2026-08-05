#![expect(clippy::print_stdout, reason = "test prints progress diagnostics")]
//! Compat verification with no user-authored tags, via
//! [`jackdaw::project_build::linkage`].
//!
//! A project dylib built as a Rust dylib keeps its `.rustc` metadata
//! section recording every dependency's exact SVH; comparing the
//! recorded `jackdaw_sdk` hash against the running SDK's own hash
//! proves the dylib links THE running SDK. The negative control checks
//! the identity discriminates builds: verification against a DIFFERENT
//! build of the same SDK crate must fail.
//!
//! Builds the same generated shim as `reflect_auto_register`; the salted
//! project target makes the second probe a cache hit when they run together:
//!
//! ```text
//! cargo test --features dylib --target <host-triple> \
//!     --test dylib_linkage_identity -- --nocapture
//! ```
#![cfg(feature = "dylib")]

use std::path::PathBuf;

use jackdaw::project_build::linkage::{LinkageError, verify_linkage};
use jackdaw::sdk_paths::SdkPaths;

mod util;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn dylib_linkage_identity_matches_the_running_sdk() {
    let sdk = SdkPaths::for_workspace(&workspace_root());
    assert!(
        sdk.dylib_exists(),
        "SDK dylib missing; build with `cargo build -p jackdaw --features dylib --target {}`",
        sdk.triple
    );
    let fixture_dylib = util::build_reflect_fixture(&sdk).dylib;

    verify_linkage(&fixture_dylib, &sdk.dylib, sdk.toolchain.as_deref())
        .expect("the fixture dylib does not verify against the running SDK");

    // Negative control: a different build of the same SDK crate (the
    // workspace's own no-target build) must be rejected, proving the
    // identity discriminates builds, not just crate names or versions.
    let stale_sdk = workspace_root().join(format!(
        "target/debug/{}jackdaw_sdk{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ));
    if stale_sdk.exists() {
        match verify_linkage(&fixture_dylib, &stale_sdk, sdk.toolchain.as_deref()) {
            Err(LinkageError::Mismatch { .. }) => {}
            other => panic!(
                "negative control failed: expected a mismatch against a \
                 different SDK build, got {other:?}"
            ),
        }
    }

    println!("linkage identity verified against the running SDK");
}
