#![expect(clippy::print_stdout, reason = "test prints progress diagnostics")]
//! Compat verification with no user-authored tags, via
//! [`jackdaw::project_build::linkage`].
//!
//! An extension-style dylib built as a Rust dylib keeps its `.rustc` metadata
//! section recording every dependency's exact SVH; comparing the
//! recorded `jackdaw_sdk` hash against the running SDK's own hash
//! proves the dylib links THE running SDK. The negative control checks
//! the identity discriminates builds: verification against a DIFFERENT
//! build of the same SDK crate must fail.
//!
//! Builds its own extension through the SDK pipeline:
//!
//! ```text
//! cargo test --features dylib --target <host-triple> \
//!     --test dylib_linkage_identity -- --nocapture
//! ```
#![cfg(feature = "dylib")]

use std::path::PathBuf;

use jackdaw::project_build::linkage::{LinkageError, verify_linkage};
use jackdaw::project_build::shim::ShimSpec;
use jackdaw::project_build::{BuildEvent, build_project_dylib};
use jackdaw::sdk_paths::SdkPaths;
use path_slash::PathExt as _;

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
    let fixture_dylib = build_extension(&sdk);

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

/// Build a one-type extension through the editor's extension pipeline and
/// return its dylib.
fn build_extension(sdk: &SdkPaths) -> PathBuf {
    let project = util::staged_fixture("dylib_linkage_identity", "extension");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(project.join("src")).expect("create the extension project");
    std::fs::write(
        project.join("Cargo.toml"),
        format!(
            r#"[package]
name = "linkage_extension"
version = "0.1.0"
edition = "2024"
publish = false

[workspace]

[dependencies]
bevy = {{ version = "0.19", default-features = false }}
jackdaw_extension = {{ path = "{}" }}
"#,
            workspace_root()
                .join("crates/jackdaw_extension")
                .to_slash_lossy()
        ),
    )
    .expect("write the extension manifest");
    std::fs::write(
        project.join("src/lib.rs"),
        r#"use jackdaw_extension::prelude::*;

#[derive(Default)]
pub struct LinkageExtension;

impl JackdawExtension for LinkageExtension {
    fn id(&self) -> String {
        "linkage_extension".into()
    }
    fn register(&self, _: &mut ExtensionRegistrar<'_>) {}
}
"#,
    )
    .expect("write the extension source");
    let spec = ShimSpec {
        package_name: "linkage_extension".into(),
        crate_name: "linkage_extension".into(),
        project_root: project.clone(),
        extension_type: Some("LinkageExtension".into()),
    };
    build_project_dylib(
        &spec,
        &project.join(".jackdaw"),
        sdk,
        Some(&workspace_root()),
        &mut |event| {
            if let BuildEvent::Log(line) = event {
                println!("{line}");
            }
        },
    )
    .expect("the extension builds through the SDK pipeline")
    .dylib
}
