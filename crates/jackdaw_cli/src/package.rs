//! `jackdaw-cli package-sdk`: stage a relocatable SDK install layout from
//! a release workspace build. This is the artifact a jackdaw release
//! ships so a downloaded editor builds projects without a source checkout
//! or a first-run bootstrap compile.
//!
//! The output mirrors the layout [`SdkPaths::for_installed_root`] resolves
//! (rooted at `--out`, pointed to by `JACKDAW_SDK_DIR`):
//!
//! ```text
//! <out>/
//!   jackdaw-rustc-wrapper
//!   jackdaw-runner            (when built; schema extraction needs it)
//!   Cargo.lock
//!   toolchain.txt
//!   sdk/
//!     manifest.txt            (basename artifacts: location-independent)
//!     host-deps/              (proc-macro dylibs)
//!     <triple>/
//!       libjackdaw_sdk.so
//!       deps/                 (the SDK runtime-closure rlibs)
//! ```
//!
//! The shipped manifest stores bare basenames rather than the absolute
//! build paths, so the redirect plan resolves them against wherever the
//! SDK is unpacked; nothing in the layout is tied to the CI build path.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use jackdaw_project_build::plan::SdkManifest;
use jackdaw_project_build::sdk_paths::SdkPaths;

/// `jackdaw-cli package-sdk --out <dir> [--workspace <path>]`.
pub fn cmd_package_sdk(args: &[String]) -> ExitCode {
    let Some(out) = flag_value(args, "--out") else {
        eprintln!("jackdaw package-sdk: --out <dir> is required");
        return ExitCode::FAILURE;
    };
    let out = PathBuf::from(out);
    let workspace = flag_value(args, "--workspace")
        .map(PathBuf::from)
        .or_else(default_workspace);
    let Some(workspace) = workspace else {
        eprintln!(
            "jackdaw package-sdk: could not locate a jackdaw workspace; pass --workspace <path>"
        );
        return ExitCode::FAILURE;
    };

    match package(&workspace, &out) {
        Ok(summary) => {
            println!("jackdaw package-sdk: staged {summary} at {}", out.display());
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("jackdaw package-sdk: {err}");
            ExitCode::FAILURE
        }
    }
}

/// `jackdaw-cli bundle --out <dir> [--workspace <path>]`: the full
/// downloadable release layout - the SDK layout (`package-sdk`) plus the
/// editor, the CLI, and the three runtime dylibs the editor and runner
/// load, staged at the bundle root beside the binaries. Combined with a
/// `rpath=$ORIGIN` link (set via RUSTFLAGS in the release build), the
/// archive runs offline with no bootstrap: extract and launch.
pub fn cmd_bundle(args: &[String]) -> ExitCode {
    let Some(out) = flag_value(args, "--out") else {
        eprintln!("jackdaw bundle: --out <dir> is required");
        return ExitCode::FAILURE;
    };
    let out = PathBuf::from(out);
    let workspace = flag_value(args, "--workspace")
        .map(PathBuf::from)
        .or_else(default_workspace);
    let Some(workspace) = workspace else {
        eprintln!("jackdaw bundle: could not locate a jackdaw workspace; pass --workspace <path>");
        return ExitCode::FAILURE;
    };
    match bundle(&workspace, &out) {
        Ok(summary) => {
            println!("jackdaw bundle: staged {summary} at {}", out.display());
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("jackdaw bundle: {err}");
            ExitCode::FAILURE
        }
    }
}

fn bundle(workspace: &Path, out: &Path) -> Result<String, String> {
    // The SDK layout first (sdk/, wrapper, runner, Cargo.lock, toolchain).
    let sdk_summary = package(workspace, out)?;
    let sdk = SdkPaths::for_workspace_profile(workspace, "release");
    let profile_dir = sdk
        .dylib
        .parent()
        .ok_or_else(|| "release SDK dylib has no parent dir".to_string())?;

    // The editor and CLI, staged beside the SDK's wrapper and runner.
    // EXE_SUFFIX is `.exe` on Windows, empty elsewhere.
    for bin in ["jackdaw", "jackdaw-cli"] {
        let name = format!("{bin}{}", std::env::consts::EXE_SUFFIX);
        let src = profile_dir.join(&name);
        if !src.is_file() {
            return Err(format!("release binary missing: {}", src.display()));
        }
        copy_into(&src, out)?;
    }

    // The runtime cdylibs the editor and runner NEED, at the bundle root so
    // a `rpath=$ORIGIN` link resolves them: bevy + jackdaw from the deps
    // dir, and std from the pinned toolchain (it is not in the workspace
    // build). Project dylibs NEED the same sonames, so a loaded project
    // resolves them here too. Names carry the platform's dylib prefix
    // (`lib` on unix, none on Windows).
    let prefix = std::env::consts::DLL_PREFIX;
    let bevy = format!("{prefix}bevy_dylib");
    let jackdaw = format!("{prefix}jackdaw_dylib");
    let std_lib = format!("{prefix}std");
    let mut dylibs = copy_matching(&sdk.deps, out, &[bevy.as_str(), jackdaw.as_str()])?;
    match target_libdir(&sdk) {
        Some(libdir) => dylibs += copy_matching(&libdir, out, &[std_lib.as_str()])?,
        None => eprintln!(
            "jackdaw bundle: warning: could not resolve the toolchain lib dir; std not bundled (the editor will need it on the library path)"
        ),
    }

    Ok(format!(
        "{sdk_summary}; + editor, cli, {dylibs} runtime dylibs"
    ))
}

/// The pinned toolchain's target lib dir, where the dynamic `libstd`
/// lives. Uses the SDK's channel so it matches what the dylibs link.
fn target_libdir(sdk: &SdkPaths) -> Option<PathBuf> {
    let mut cmd = Command::new("rustc");
    if let Some(channel) = &sdk.toolchain {
        cmd.arg(format!("+{channel}"));
    }
    let out = cmd.args(["--print", "target-libdir"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8(out.stdout).ok()?;
    let trimmed = path.trim();
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}

/// Copy dylibs in `from` whose filename starts with any of `prefixes`
/// into `to`, returning the count. Narrower than [`copy_dylibs`]: the
/// bundle root wants only the specific runtime cdylibs, not every dylib
/// in the source dir.
fn copy_matching(from: &Path, to: &Path, prefixes: &[&str]) -> Result<usize, String> {
    let ext = dylib_ext();
    let mut count = 0;
    let entries =
        std::fs::read_dir(from).map_err(|e| format!("reading {}: {e}", from.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.extension().is_some_and(|e| e == ext)
            && prefixes.iter().any(|p| name.starts_with(p))
        {
            copy(&path, &to.join(entry.file_name()))?;
            count += 1;
        }
    }
    Ok(count)
}

fn package(workspace: &Path, out: &Path) -> Result<String, String> {
    let sdk = SdkPaths::for_workspace_profile(workspace, "release");
    if !sdk.dylib_exists() {
        return Err(format!(
            "release SDK dylib not found at {}; build it first \
             (cargo build --release --target {} -p jackdaw --features dylib)",
            sdk.dylib.display(),
            sdk.triple
        ));
    }
    ensure_manifest(workspace, &sdk)?;

    let sdk_out = out.join("sdk");
    let triple_out = sdk_out.join(&sdk.triple);
    let deps_out = triple_out.join("deps");
    let host_deps_out = sdk_out.join("host-deps");
    create_dir(&deps_out)?;
    create_dir(&host_deps_out)?;

    // The SDK dylib itself.
    copy_into(&sdk.dylib, &triple_out)?;

    // Runtime-closure rlibs, plus a manifest that names them by basename
    // only so the plan resolves them against this install's `deps/`.
    let manifest_text = read(&sdk.manifest)?;
    let mut shipped_manifest = String::new();
    let mut rlibs = 0usize;
    for line in manifest_text.lines() {
        let mut parts = line.splitn(3, ' ');
        let (Some(name), Some(version), Some(abspath)) = (parts.next(), parts.next(), parts.next())
        else {
            return Err(format!("malformed SDK manifest line: {line}"));
        };
        let src = Path::new(abspath);
        let base = src
            .file_name()
            .ok_or_else(|| format!("manifest artifact has no filename: {abspath}"))?;
        copy(src, &deps_out.join(base))?;
        shipped_manifest.push_str(&format!("{name} {version} {}\n", base.to_string_lossy()));
        rlibs += 1;
    }
    write(&sdk_out.join("manifest.txt"), &shipped_manifest)?;

    // Runtime cdylibs the project links through under `prefer-dynamic`:
    // the bevy and jackdaw dylibs sit beside the closure rlibs in the
    // triple deps dir, not in the manifest (which lists only rlibs). A
    // project dylib NEEDs them by soname, so the linker has to find them
    // on the deps search path. (`libstd` is not among them: rustc resolves
    // it from its own sysroot at link time, and the editor bundle ships it
    // for load time.)
    let cdylibs = copy_dylibs(&sdk.deps, &deps_out)?;

    // Proc-macro dylibs the SDK rlibs reference at project-compile time.
    let macros = copy_dylibs(&sdk.host_deps, &host_deps_out)?;

    // Host tools next to the editor.
    copy_into(&sdk.wrapper, out)?;
    let runner_note = if sdk.runner.is_file() {
        copy_into(&sdk.runner, out)?;
        "runner"
    } else {
        eprintln!(
            "jackdaw package-sdk: warning: runner not built at {}; schema extraction \
             will be unavailable (cargo build --release --target {} -p jackdaw_runner)",
            sdk.runner.display(),
            sdk.triple
        );
        "no runner"
    };

    // The SDK's exact lockfile and toolchain, so a project resolves the
    // shared closure at the same versions and compiles with the same rustc.
    if sdk.lockfile.is_file() {
        copy(&sdk.lockfile, &out.join("Cargo.lock"))?;
    }
    if let Some(channel) = &sdk.toolchain {
        write(&out.join("toolchain.txt"), &format!("{channel}\n"))?;
    }

    Ok(format!(
        "{rlibs} rlibs, {cdylibs} runtime cdylibs, {macros} proc-macro dylibs, {runner_note}"
    ))
}

/// Generate `sdk.manifest` from the (already built) release editor when a
/// prior build has not left one. A no-op once present; in CI the editor is
/// compiled first, so this just re-reads its artifact list.
fn ensure_manifest(workspace: &Path, sdk: &SdkPaths) -> Result<(), String> {
    if sdk.manifest.is_file() {
        return Ok(());
    }
    SdkManifest::generate(
        workspace,
        sdk,
        &["-p", "jackdaw", "--features", "dylib", "--release"],
    )
    .map(|_| ())
    .map_err(|e| format!("generating SDK manifest: {e}"))
}

/// Copy every dynamic library in `from` into `to`, returning the count.
/// Used for the host-side proc-macro dylibs, which all carry the platform
/// dylib extension and sit beside the SDK rlibs' host build.
fn copy_dylibs(from: &Path, to: &Path) -> Result<usize, String> {
    let ext = dylib_ext();
    let mut count = 0;
    let entries =
        std::fs::read_dir(from).map_err(|e| format!("reading {}: {e}", from.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == ext) {
            let name = path.file_name().expect("dir entry has a filename");
            copy(&path, &to.join(name))?;
            count += 1;
        }
    }
    Ok(count)
}

fn dylib_ext() -> &'static str {
    if cfg!(target_os = "windows") {
        "dll"
    } else if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    }
}

/// The jackdaw workspace this CLI was built from: two levels up from the
/// crate manifest. Present in a dev checkout even after `cargo install`.
fn default_workspace() -> Option<PathBuf> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(PathBuf::from)
        .filter(|p| p.exists())
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg == flag {
            return it.next().cloned();
        }
    }
    None
}

fn create_dir(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| format!("creating {}: {e}", path.display()))
}

fn copy(src: &Path, dst: &Path) -> Result<(), String> {
    if let Some(parent) = dst.parent() {
        create_dir(parent)?;
    }
    std::fs::copy(src, dst)
        .map(|_| ())
        .map_err(|e| format!("copying {} -> {}: {e}", src.display(), dst.display()))
}

/// Copy `src` into directory `dir`, keeping its filename.
fn copy_into(src: &Path, dir: &Path) -> Result<(), String> {
    let name = src
        .file_name()
        .ok_or_else(|| format!("no filename for {}", src.display()))?;
    copy(src, &dir.join(name))
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))
}

fn write(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        create_dir(parent)?;
    }
    std::fs::write(path, contents).map_err(|e| format!("writing {}: {e}", path.display()))
}
