#![expect(clippy::print_stdout, reason = "test prints progress diagnostics")]
//! An extension built through the SDK pipeline carries the hash of the SDK it
//! was built against, and the loader opens it only in an editor on that SDK.
//!
//! The fixture extension links more public functions than a Windows DLL can
//! export (65,535), which a `cdylib` never exports, so it must build and load
//! on every platform. A copy that names another SDK build must be refused
//! before it is opened.
//!
//! ```text
//! cargo test --features dylib --target <host-triple> \
//!     --test dylib_linkage_identity -- --nocapture
//! ```
#![cfg(feature = "dylib")]

use std::path::PathBuf;

use jackdaw::project_build::linkage::{LinkageError, sdk_svh, verify_linkage};
use jackdaw::project_build::shim::ShimSpec;
use jackdaw::project_build::{BuildEvent, build_project_dylib};
use jackdaw::sdk_paths::SdkPaths;
use jackdaw_loader::LoadError;
use path_slash::PathExt as _;

mod util;

/// More than the 65,535 exports a Windows DLL can hold.
const PUBLIC_FUNCTIONS: usize = 70_000;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn an_extension_loads_only_into_an_editor_on_its_sdk() {
    let sdk = SdkPaths::for_workspace(&workspace_root());
    assert!(
        sdk.dylib_exists(),
        "SDK dylib missing; build with `cargo build -p jackdaw --features dylib --target {}`",
        sdk.triple
    );
    let svh = sdk_svh(&sdk.dylib, sdk.toolchain.as_deref()).expect("read the SDK's crate hash");
    let library = build_extension(&sdk);

    verify_linkage(&library, &svh).expect("the extension names the SDK it was built against");
    match verify_linkage(&library, "0") {
        Err(LinkageError::Mismatch { recorded, .. }) => assert_eq!(recorded, svh),
        other => panic!("expected a mismatch against another SDK hash, got {other:?}"),
    }

    jackdaw_loader::set_running_sdk(Ok(svh.clone()));
    let id = jackdaw_loader::probe(&library).expect("the extension loads into this SDK");
    assert_eq!(id, "oversized_extension");

    let foreign = library.with_file_name(format!(
        "{}foreign_extension{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ));
    let mut bytes = std::fs::read(&library).expect("read the extension");
    let marker = jackdaw_package::sdk_identity::MARKER;
    let at = bytes
        .windows(marker.len())
        .rposition(|window| window == marker)
        .expect("the extension carries the SDK marker")
        + marker.len();
    bytes[at] = if bytes[at] == b'z' { b'y' } else { b'z' };
    std::fs::write(&foreign, &bytes).expect("write the altered copy");
    match jackdaw_loader::probe(&foreign) {
        Err(error @ LoadError::SdkMismatch(_)) => {
            assert!(error.to_string().contains("rebuild"), "{error}");
        }
        other => panic!("expected the altered copy to be refused, got {other:?}"),
    }
    let _ = std::fs::remove_file(foreign);

    println!("the extension loads into its own SDK and a copy for another is refused");
}

/// Build an extension with [`PUBLIC_FUNCTIONS`] public functions through the
/// editor's extension pipeline and return its library.
fn build_extension(sdk: &SdkPaths) -> PathBuf {
    let project = util::staged_fixture("dylib_linkage_identity", "oversized");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(project.join("src")).expect("create the extension project");
    std::fs::write(
        project.join("Cargo.toml"),
        format!(
            r#"[package]
name = "oversized_extension"
version = "0.1.0"
edition = "2024"
publish = false

[workspace]

[dependencies]
bevy = {{ version = "0.20", default-features = false }}
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

pub mod many;

#[derive(Default)]
pub struct OversizedExtension;

impl JackdawExtension for OversizedExtension {
    fn id(&self) -> String {
        "oversized_extension".into()
    }
    fn register(&self, _: &mut ExtensionRegistrar<'_>) {}
}
"#,
    )
    .expect("write the extension source");
    let many: String = (0..PUBLIC_FUNCTIONS)
        .map(|n| format!("#[inline(never)]\npub fn exported_{n}() -> usize {{ {n} }}\n"))
        .collect();
    std::fs::write(project.join("src/many.rs"), many).expect("write the public functions");
    let spec = ShimSpec {
        package_name: "oversized_extension".into(),
        crate_name: "oversized_extension".into(),
        project_root: project.clone(),
        extension_type: Some("OversizedExtension".into()),
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
