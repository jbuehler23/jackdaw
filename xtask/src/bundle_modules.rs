//! `cargo xtask bundle-modules --bundle <dir>`: build the editor extension of
//! every available module against a staged bundle's SDK, pack each with the
//! bundle's own `jd`, and stage them under `extensions/` in the bundle, where
//! the editor loads them from.

use std::path::{Path, PathBuf};
use std::process::Command;

use jackdaw_project_build::build_source::crate_path;
use jackdaw_project_build::detect::detect_extension;
use jackdaw_project_build::modules::ModuleCatalog;
use jackdaw_project_build::shim::ShimSpec;
use jackdaw_project_build::{BuildEvent, build_project_dylib, sdk_paths::SdkPaths};

/// `bundle-modules --bundle <dir> [--workspace <path>]`. The workspace
/// defaults to the checkout this xtask belongs to.
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
        eprintln!("usage: cargo xtask bundle-modules --bundle <DIR> [--workspace <PATH>]");
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
        Ok(ids) => {
            println!(
                "bundled the {} module extensions in {bundle}",
                ids.join(", ")
            );
            true
        }
        Err(error) => {
            eprintln!("bundle-modules: {error}");
            false
        }
    }
}

/// The ids of the extensions a bundle of this checkout ships.
pub fn shipped_extensions() -> Vec<String> {
    ModuleCatalog::builtin()
        .modules()
        .iter()
        .filter(|module| module.unavailable.is_none() && module.extension_crate.is_some())
        .filter_map(|module| module.extension.clone())
        .collect()
}

fn run(workspace: &Path, bundle: &Path) -> Result<Vec<String>, String> {
    let workspace = &std::path::absolute(workspace)
        .map_err(|e| format!("resolving {}: {e}", workspace.display()))?;
    let bundle =
        &std::path::absolute(bundle).map_err(|e| format!("resolving {}: {e}", bundle.display()))?;
    let sdk = SdkPaths::for_installed_root(bundle);
    let jd = bundle.join(format!("jd{}", std::env::consts::EXE_SUFFIX));
    if !jd.is_file() {
        return Err(format!("{} has no jd", bundle.display()));
    }
    // Under target/ rather than the system temp dir, which is often a small
    // tmpfs; each extension build is several GB.
    let staging = workspace
        .join("target")
        .join(format!("bundle-modules-{}", std::process::id()));
    let result = build_all(workspace, bundle, &staging, &sdk, &jd);
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn build_all(
    workspace: &Path,
    bundle: &Path,
    staging: &Path,
    sdk: &SdkPaths,
    jd: &Path,
) -> Result<Vec<String>, String> {
    std::fs::create_dir_all(staging).map_err(|e| format!("creating {}: {e}", staging.display()))?;
    // A key for this bundle only: the archive, not the bundle signature, is
    // what a user trusts, and nothing outside the archive should verify
    // against it.
    let key = staging.join("bundle-key.pk8");
    run_jd(jd, &["extension", "keygen"], &[&key])?;
    let out = bundle.join(jackdaw_package::BUNDLED_EXTENSIONS_DIR);
    let mut ids = Vec::new();
    for module in ModuleCatalog::builtin().modules() {
        let (Some(id), Some(crate_name), None) = (
            module.extension.as_deref(),
            module.extension_crate.as_deref(),
            module.unavailable.as_ref(),
        ) else {
            continue;
        };
        let crate_dir = crate_path(workspace, crate_name);
        let (_, extension_type) = detect_extension(&crate_dir).ok_or_else(|| {
            format!(
                "{} has no JackdawExtension implementation",
                crate_dir.display()
            )
        })?;
        eprintln!("building the {} module's extension `{id}`", module.name);
        let spec = ShimSpec {
            package_name: crate_name.to_string(),
            crate_name: crate_name.replace('-', "_"),
            project_root: crate_dir.clone(),
            extension_type: Some(extension_type),
        };
        let build_dir = staging.join(crate_name);
        std::fs::create_dir_all(&build_dir)
            .map_err(|e| format!("creating {}: {e}", build_dir.display()))?;
        let build = build_project_dylib(&spec, &build_dir, sdk, None, &mut |event| {
            if let BuildEvent::Log(line) = event {
                eprintln!("{line}");
            }
        })
        .map_err(|e| format!("building `{id}` against the bundle SDK: {e}"))?;
        let packed = staging.join(format!("{id}.jdext"));
        let project = crate_dir.display().to_string();
        let library = build.dylib.display().to_string();
        let key = key.display().to_string();
        let packed_path = packed.display().to_string();
        run_jd(
            jd,
            &[
                "extension",
                "pack",
                "--project",
                &project,
                "--library",
                &library,
                "--id",
                id,
                "--label",
                &module.name,
                "--version",
                jackdaw_project_build::VERSION,
                "--publisher",
                "Jackdaw",
                "--license",
                "MIT OR Apache-2.0",
                "--key",
                &key,
                "--out",
                &packed_path,
            ],
            &[],
        )?;
        let bytes =
            std::fs::read(&packed).map_err(|e| format!("reading {}: {e}", packed.display()))?;
        let staged = jackdaw_package::stage_bundled(&out, &bytes)
            .map_err(|e| format!("staging `{id}` in {}: {e}", out.display()))?;
        eprintln!("staged `{id}` at {}", staged.library_path.display());
        ids.push(id.to_string());
    }
    Ok(ids)
}

fn run_jd(jd: &Path, args: &[&str], paths: &[&PathBuf]) -> Result<(), String> {
    let status = Command::new(jd)
        .args(args)
        .args(paths)
        .status()
        .map_err(|e| format!("running {}: {e}", jd.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`jd {}` failed with {status}", args.join(" ")))
    }
}
