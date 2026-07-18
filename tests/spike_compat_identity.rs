//! Spike 3 probe, now the pipeline's regression test: compat
//! verification with no user-authored tags, via
//! [`jackdaw::project_build::linkage`].
//!
//! A project dylib built as a Rust dylib keeps its `.rustc` metadata
//! section recording every dependency's exact SVH; comparing the
//! recorded `jackdaw_sdk` hash against the running SDK's own hash
//! proves the dylib links THE running SDK. The negative control checks
//! the identity discriminates builds: verification against a DIFFERENT
//! build of the same SDK crate must fail.
//!
//! Requires the spike 1 dylib; run after (or alongside):
//!
//! ```text
//! cargo test --features dylib --target <host-triple> \
//!     --test spike_compat_identity -- --nocapture
//! ```
#![cfg(feature = "dylib")]

use std::path::PathBuf;

use jackdaw::project_build::linkage::{LinkageError, verify_linkage};
use jackdaw::sdk_paths::SdkPaths;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn dylib_linkage_identity_matches_the_running_sdk() {
    let sdk = SdkPaths::for_workspace(&workspace_root());
    let spike_dylib = workspace_root().join(format!(
        ".scratch/project-onboarding/spike1/spike_game/target-spike/{}/debug/{}spike_game{}",
        sdk.triple,
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ));
    assert!(
        sdk.dylib_exists(),
        "SDK dylib missing; build with `cargo build -p jackdaw --features dylib --target {}`",
        sdk.triple
    );
    assert!(
        spike_dylib.exists(),
        "spike dylib missing; run the spike_auto_register test first"
    );

    verify_linkage(&spike_dylib, &sdk.dylib)
        .expect("SPIKE FAILED: the spike dylib does not verify against the running SDK");

    // Negative control: a different build of the same SDK crate (the
    // workspace's own no-target build) must be rejected, proving the
    // identity discriminates builds, not just crate names or versions.
    let stale_sdk = workspace_root().join(format!(
        "target/debug/{}jackdaw_sdk{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ));
    if stale_sdk.exists() {
        match verify_linkage(&spike_dylib, &stale_sdk) {
            Err(LinkageError::Mismatch { .. }) => {}
            other => panic!(
                "negative control failed: expected a mismatch against a \
                 different SDK build, got {other:?}"
            ),
        }
    }

    println!("SPIKE PASSED: linkage identity verified against the running SDK");
}
