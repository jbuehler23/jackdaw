//! `cargo xtask bundle-smoke --bundle <dir>`: resolve the SDK from a staged
//! bundle the way an installed editor does, and build an extension against
//! it. This is the path an installed user takes for marketplace dylibs.

use std::path::{Path, PathBuf};

use jackdaw_project_build::shim::ShimSpec;
use jackdaw_project_build::{BuildEvent, build_project_dylib, sdk_paths::SdkPaths};

/// `bundle-smoke --bundle <dir> [--workspace <path>]`. The workspace defaults
/// to the checkout this xtask belongs to.
pub fn cmd(args: &[String]) -> bool {
    let mut workspace = None;
    let mut bundle = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--workspace" => workspace = rest.next(),
            "--bundle" => bundle = rest.next(),
            _ => {}
        }
    }
    let Some(bundle) = bundle else {
        eprintln!("usage: cargo xtask bundle-smoke --bundle <DIR> [--workspace <PATH>]");
        return false;
    };
    let workspace = workspace.map_or_else(
        || {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap_or(Path::new("."))
        },
        Path::new,
    );
    match run(workspace, Path::new(bundle)) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("bundle-smoke: {error}");
            false
        }
    }
}

/// Check the bundle at `bundle` and build an extension against its SDK. The
/// extension depends on this checkout's `jackdaw_extension`, so it works
/// before the release's crates are published.
fn run(workspace: &Path, bundle: &Path) -> Result<(), String> {
    let workspace = &std::path::absolute(workspace)
        .map_err(|e| format!("resolving {}: {e}", workspace.display()))?;
    let bundle =
        &std::path::absolute(bundle).map_err(|e| format!("resolving {}: {e}", bundle.display()))?;
    let sdk = SdkPaths::for_installed_root(bundle);
    let mut missing = Vec::new();
    if !sdk.manifest.is_file() {
        missing.push(sdk.manifest.display().to_string());
    }
    if !sdk.wrapper.is_file() {
        missing.push("the rustc wrapper".to_string());
    }
    if !sdk.dylib_exists() {
        missing.push("the SDK dylib".to_string());
    }
    if !sdk.lockfile.is_file() {
        missing.push("Cargo.lock".to_string());
    }
    for runtime in ["bevy_dylib", "jackdaw_dylib"] {
        if !has_runtime(bundle, runtime) {
            missing.push(format!("the shared {runtime} runtime"));
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "{} is missing {}",
            bundle.display(),
            missing.join(", ")
        ));
    }

    // Under target/ rather than the system temp dir, which is often a small
    // tmpfs; the extension build alone is several GB.
    let staging = workspace
        .join("target")
        .join(format!("bundle-smoke-{}", std::process::id()));
    let result = build_extension(workspace, &staging, &sdk);
    let _ = std::fs::remove_dir_all(&staging);
    let dylib = result?;
    println!(
        "BUNDLE SMOKE PASS: built {} against the SDK in {}",
        dylib.display(),
        bundle.display()
    );
    Ok(())
}

fn build_extension(workspace: &Path, staging: &Path, sdk: &SdkPaths) -> Result<PathBuf, String> {
    let build_dir = staging.join("build");
    let extension_dir = staging.join("ext");
    let extension_crate = workspace
        .join("crates/jackdaw_extension")
        .to_string_lossy()
        .replace('\\', "/");
    write(
        &extension_dir.join("Cargo.toml"),
        &format!(
            r#"[package]
name = "bundle_smoke_ext"
version = "0.1.0"
edition = "2024"
publish = false

[workspace]

[dependencies]
bevy = {{ version = "0.19", default-features = false }}
jackdaw_extension = {{ path = "{extension_crate}" }}
"#
        ),
    )?;
    write(
        &extension_dir.join("src/lib.rs"),
        r#"use bevy::prelude::*;
use jackdaw_extension::prelude::*;

#[derive(Default)]
pub struct SmokeExtension;

impl JackdawExtension for SmokeExtension {
    fn id(&self) -> String { "bundle_smoke".into() }
    fn register(&self, _: &mut ExtensionRegistrar<'_>) {}
}
"#,
    )?;
    std::fs::create_dir_all(&build_dir)
        .map_err(|e| format!("creating {}: {e}", build_dir.display()))?;
    let spec = ShimSpec {
        package_name: "bundle_smoke_ext".into(),
        crate_name: "bundle_smoke_ext".into(),
        project_root: extension_dir,
        extension_type: Some("SmokeExtension".into()),
    };
    let build = build_project_dylib(&spec, &build_dir, sdk, None, &mut |event| {
        if let BuildEvent::Log(line) = event {
            eprintln!("{line}");
        }
    })
    .map_err(|e| format!("building an extension against the bundle SDK: {e}"))?;
    if !build.dylib.is_file() {
        return Err(format!(
            "extension dylib missing at {}",
            build.dylib.display()
        ));
    }
    Ok(build.dylib)
}

fn has_runtime(bundle: &Path, crate_id: &str) -> bool {
    let prefix = format!("{}{crate_id}", std::env::consts::DLL_PREFIX);
    std::fs::read_dir(bundle)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with(&prefix) && name.ends_with(std::env::consts::DLL_SUFFIX)
        })
}

fn write(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    std::fs::write(path, contents).map_err(|e| format!("writing {}: {e}", path.display()))
}
