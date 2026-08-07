//! `cargo xtask package-sdk`: stage a relocatable SDK install layout from
//! a release workspace build. This is the artifact a jackdaw release
//! ships so a downloaded editor builds extensions without a source
//! checkout or a first-run bootstrap compile.
//!
//! The output mirrors the layout [`SdkPaths::for_installed_root`] resolves
//! (rooted at `--out`, pointed to by `JACKDAW_SDK_DIR`):
//!
//! ```text
//! <out>/
//!   jackdaw-rustc-wrapper
//!   Cargo.lock
//!   toolchain.txt
//!   sdk/
//!     manifest.txt            (basename artifacts: location-independent)
//!     host-deps/              (proc-macro dylibs)
//!     native-libs/            (import libs the SDK links by bare name)
//!     <triple>/
//!       libjackdaw_sdk.so
//!       deps/                 (SDK rlibs plus shared runtime dylibs)
//! ```
//!
//! The shipped manifest stores bare basenames rather than the absolute
//! build paths, so the redirect plan resolves them against wherever the
//! SDK is unpacked; nothing in the layout is tied to the CI build path.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use jackdaw_project_build::plan::SdkManifest;
use jackdaw_project_build::sdk_paths::SdkPaths;

/// `cargo xtask package-sdk --out <dir> [--workspace <path>]`.
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

/// `cargo xtask bundle --out <dir> [--workspace <path>]`: the full
/// downloadable release layout - the SDK layout (`package-sdk`) plus the
/// editor, the CLI, and the runtime dylibs the editor and extensions
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
    // The SDK layout first (sdk/, wrapper, Cargo.lock, toolchain).
    let sdk_summary = package(workspace, out)?;
    let sdk = SdkPaths::for_workspace_profile(workspace, "release");
    let profile_dir = sdk
        .dylib
        .parent()
        .ok_or_else(|| "release SDK dylib has no parent dir".to_string())?;

    // The editor and public CLI, staged beside the SDK's wrapper.
    // EXE_SUFFIX is `.exe` on Windows, empty elsewhere.
    for bin in ["jackdaw", "jd"] {
        let name = format!("{bin}{}", std::env::consts::EXE_SUFFIX);
        let src = profile_dir.join(&name);
        if !src.is_file() {
            return Err(format!("release binary missing: {}", src.display()));
        }
        copy_into(&src, out)?;
    }

    // The runtime dylibs the editor and extensions NEED, at the bundle root
    // so a `rpath=$ORIGIN` link resolves them. Bevy and Jackdaw are
    // deliberately separate libraries: combining their exports in the SDK
    // facade exceeds Windows' PE export-table ceiling, while embedding
    // either runtime in a consumer gives it private registries and TypeIds.
    // Dynamic std comes from the pinned toolchain rather than the workspace
    // build. Names carry the platform's dylib prefix (`lib` on Unix, none
    // on Windows).
    let prefix = std::env::consts::DLL_PREFIX;
    let std_lib = format!("{prefix}std");
    let mut dylibs = 0;
    for required in ["bevy_dylib", "jackdaw_dylib"] {
        copy_rust_dependency_dylib(&sdk, out, required)?;
        dylibs += 1;
    }
    match target_libdir(&sdk) {
        Some(libdir) => dylibs += copy_matching(&libdir, out, &[std_lib.as_str()])?,
        None => eprintln!(
            "jackdaw bundle: warning: could not resolve the toolchain lib dir; std not bundled (the editor will need it on the library path)"
        ),
    }

    relocate_macos_install_names(out, &sdk.triple)?;

    Ok(format!(
        "{sdk_summary}; + editor, jd, {dylibs} runtime dylibs"
    ))
}

/// Rewrite the staged Mach-O files so the bundle is self-contained.
///
/// rustc leaves a Mach-O dylib's install name as its build-tree path, and
/// every consumer copies that path into its own load commands. A staged
/// `jd` therefore asked dyld for
/// `/Users/runner/work/.../target/.../deps/libjackdaw_dylib.dylib`: in CI
/// that resolved to whatever a later build left there (a different
/// resolution, missing a `type_info::CELL` the binary imported), and on a
/// user's machine it resolves to nothing at all. ELF does not have this
/// problem because it records only the soname and `$ORIGIN` does the rest.
///
/// Three rewrites make the bundle relocatable, mirroring what `$ORIGIN`
/// gives Linux for free:
/// - every staged dylib's id becomes `@rpath/<name>`, so anything linked
///   against the staged copy records a relocatable reference;
/// - every staged binary's and dylib's load commands pointing at a
///   staged name are redirected to `@rpath/<name>`;
/// - the binaries get rpaths for the bundle root and the SDK dirs, so a
///   dlopened extension dylib (whose own references go through `@rpath`
///   once it links the rewritten SDK) resolves from the running process.
///
/// Editing a Mach-O invalidates its code signature, and Apple Silicon
/// refuses to run unsigned binaries, so every touched file is re-signed
/// ad hoc.
fn relocate_macos_install_names(out: &Path, triple: &str) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    let sdk_dir = out.join("sdk").join(triple);
    let dylib_dirs = [out.to_path_buf(), sdk_dir.clone(), sdk_dir.join("deps")];
    let mut dylibs: Vec<PathBuf> = Vec::new();
    for dir in &dylib_dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "dylib") {
                dylibs.push(path);
            }
        }
    }
    let staged_names: std::collections::BTreeSet<String> = dylibs
        .iter()
        .filter_map(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .collect();

    let binaries: Vec<PathBuf> = ["jackdaw", "jd", "jackdaw-rustc-wrapper"]
        .iter()
        .map(|b| out.join(b))
        .filter(|p| p.is_file())
        .collect();

    for dylib in &dylibs {
        let name = dylib.file_name().expect("staged dylib has a name");
        let mut id = std::ffi::OsString::from("@rpath/");
        id.push(name);
        run_tool(
            Command::new("install_name_tool")
                .arg("-id")
                .arg(&id)
                .arg(dylib),
        )?;
    }
    for file in dylibs.iter().chain(binaries.iter()) {
        for dep in macho_load_paths(file)? {
            let Some(base) = Path::new(&dep).file_name() else {
                continue;
            };
            let base_str = base.to_string_lossy();
            if !staged_names.contains(base_str.as_ref()) || dep.starts_with("@rpath/") {
                continue;
            }
            let mut target = std::ffi::OsString::from("@rpath/");
            target.push(base);
            run_tool(
                Command::new("install_name_tool")
                    .arg("-change")
                    .arg(&dep)
                    .arg(&target)
                    .arg(file),
            )?;
        }
    }
    for bin in &binaries {
        for rpath in [
            "@loader_path".to_string(),
            format!("@loader_path/sdk/{triple}"),
            format!("@loader_path/sdk/{triple}/deps"),
        ] {
            // Duplicate rpaths are an error from the tool and fine for
            // us: the link itself already added `@loader_path`.
            let _ = Command::new("install_name_tool")
                .args(["-add_rpath", &rpath])
                .arg(bin)
                .output();
        }
    }
    for file in dylibs.iter().chain(binaries.iter()) {
        run_tool(
            Command::new("codesign")
                .args(["--force", "--sign", "-"])
                .arg(file),
        )?;
    }
    Ok(())
}

/// The dependency paths in a Mach-O file's load commands, via `otool -L`.
fn macho_load_paths(file: &Path) -> Result<Vec<String>, String> {
    let output = Command::new("otool")
        .arg("-L")
        .arg(file)
        .output()
        .map_err(|e| format!("running otool on {}: {e}", file.display()))?;
    if !output.status.success() {
        return Err(format!(
            "otool -L {} failed: {}",
            file.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    // Lines after the header are `\t<path> (compatibility version ...)`.
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect())
}

fn run_tool(cmd: &mut Command) -> Result<(), String> {
    let output = cmd
        .output()
        .map_err(|e| format!("running {:?}: {e}", cmd.get_program()))?;
    if !output.status.success() {
        return Err(format!(
            "{:?} failed: {}",
            cmd.get_program(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
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

/// Copy the exact Rust dylib a built SDK records for `crate_name`.
///
/// A restored Cargo cache can hold several hashed builds of the same crate.
/// Prefix-copying all of them bloats the archive and can accidentally ship an
/// obsolete ABI beside the one the SDK imports. Rust metadata names the exact
/// dependency crate identifier, which is also the dylib's hashed basename.
fn copy_rust_dependency_dylib(sdk: &SdkPaths, to: &Path, crate_name: &str) -> Result<(), String> {
    let mut cmd = Command::new("rustc");
    if let Some(channel) = &sdk.toolchain {
        cmd.arg(format!("+{channel}"));
    }
    let output = cmd
        .arg("-Zls=root")
        .arg(&sdk.dylib)
        .output()
        .map_err(|e| format!("reading metadata from {}: {e}", sdk.dylib.display()))?;
    if !output.status.success() {
        return Err(format!(
            "reading metadata from {} failed: {}",
            sdk.dylib.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let dependency_prefix = format!("{crate_name}-");
    let dump = String::from_utf8_lossy(&output.stdout);
    let crate_id = dump.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let _index = parts.next()?;
        let id = parts.next()?;
        (id == crate_name || id.starts_with(&dependency_prefix)).then_some(id)
    });
    let Some(crate_id) = crate_id else {
        return Err(format!(
            "{} does not record a dynamic dependency on {crate_name}; the shared-runtime split is not active",
            sdk.dylib.display()
        ));
    };
    let file = format!(
        "{}{}{}",
        std::env::consts::DLL_PREFIX,
        crate_id,
        std::env::consts::DLL_SUFFIX
    );
    let src = sdk.deps.join(&file);
    if !src.is_file() {
        return Err(format!(
            "{crate_name} is recorded as {file}, but that artifact is missing from {}",
            sdk.deps.display()
        ));
    }
    copy(&src, &to.join(&file))?;
    copy_import_lib(&src, to)
}

/// Copy dylibs in `from` whose filename starts with one of `prefixes`.
/// Used for dynamic std in the toolchain directory, where there is only one
/// artifact for the pinned compiler rather than Cargo's cache of hashed builds.
fn copy_matching(from: &Path, to: &Path, prefixes: &[&str]) -> Result<usize, String> {
    let ext = dylib_ext();
    let mut count = 0;
    let entries =
        std::fs::read_dir(from).map_err(|e| format!("reading {}: {e}", from.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.extension().is_some_and(|e| e == ext)
            && prefixes.iter().any(|prefix| name.starts_with(prefix))
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
    let native_out = sdk_out.join("native-libs");
    create_dir(&deps_out)?;
    create_dir(&host_deps_out)?;

    // The SDK dylib itself.
    copy_into(&sdk.dylib, &triple_out)?;
    copy_import_lib(&sdk.dylib, &triple_out)?;

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

    // Every OTHER rlib generation in the deps dir too, beyond the
    // manifest's one-per-crate picks. A target dir that held two
    // resolutions leaves the picks internally inconsistent: some staged
    // rlib pins a transitive at the generation the manifest did NOT
    // pick, and the extension build dies on a bare "can't find crate" for
    // a crate whose file was read and was healthy. Proven on macOS by
    // A/B: the same extern fails against the picked set and passes when
    // every generation travels. rustc resolves transitives by exact
    // hash, so the extra files can never shadow anything; they only
    // close holes. The manifest still names the picks, which is what
    // the redirect plan points direct edges at.
    let mut extra_rlibs = 0usize;
    if let Ok(entries) = std::fs::read_dir(&sdk.deps) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "rlib") {
                let name = path.file_name().expect("dir entry has a filename");
                let dst = deps_out.join(name);
                if !dst.exists() {
                    copy(&path, &dst)?;
                    extra_rlibs += 1;
                }
            }
        }
    }

    // The recorded proc-macro list, shipped as basenames like the
    // manifest: the wrapper rewrites a rewritten unit's proc-macro
    // externs to these exact files (resolved against the staged
    // host-deps dir), because an explicit --extern otherwise restricts
    // rustc to the project's own macro build, which only loads SDK
    // rlibs where the two builds happen to hash-match.
    let recorded_list: String = jackdaw_project_build::plan::read_host_deps(&sdk.manifest)
        .iter()
        .filter_map(|entry| Path::new(entry).file_name())
        .map(|name| format!("{}\n", name.to_string_lossy()))
        .collect();
    write(&sdk_out.join("jackdaw_sdk_host_deps.txt"), &recorded_list)?;

    // Native import libraries the SDK's crates reference by bare name
    // (the `windows` crates ship their own). The recorded search paths
    // point into the build machine's cargo registry, which does not
    // exist on the machine that unpacks this, so the libraries are
    // staged here and the path rewritten to the one directory that
    // travels with the bundle.
    let native_libs = stage_native_libs(&sdk, &native_out)?;
    // Empty when nothing was staged, rather than a path to a directory
    // that was never created. Every platform whose native libraries are
    // system wide records none, and a consumer should see that plainly.
    let staged_paths = if native_libs > 0 {
        format!("{}\n", native_out.display())
    } else {
        String::new()
    };
    write(&sdk_out.join("jackdaw_sdk_link_paths.txt"), &staged_paths)?;

    // Runtime dylibs the extension links through under `prefer-dynamic` sit
    // beside the closure rlibs in the triple deps dir, not in the manifest
    // (which lists only rlibs). An extension dylib NEEDs the Bevy,
    // Jackdaw, and SDK sonames, so the linker must find all of them on the
    // deps search path. (`libstd` is not among them: rustc resolves it from
    // its own sysroot at link time, and the editor bundle ships it for load
    // time.)
    let cdylibs = copy_dylibs(&sdk.deps, &deps_out)?;

    // Proc-macro dylibs the SDK rlibs reference at extension-compile time.
    // From the recorded list when the manifest generation captured one:
    // the host deps dir accumulates generations from other package
    // selections (the wrapper build, earlier runs restored from cache),
    // and staging the directory wholesale shipped proc macros the rlibs
    // were not built against. Every candidate then failed `metadata
    // mismatch` at extension-compile time, reported as `can't find crate`
    // for whichever SDK crate held the chain.
    // The union of the recorded list and the directory: the exact list
    // pins the generation the rlibs were built against, but it is only
    // as complete as the enumeration that wrote it (a cached manifest
    // skips regeneration, and macOS shipped a list missing
    // jackdaw_api_macros while Windows had it). The sweep restores
    // completeness; rustc picks among candidates by hash, so a stale
    // extra is rejected harmlessly while a missing exact file is a
    // build-stopping hole.
    let recorded_macros = jackdaw_project_build::plan::read_host_deps(&sdk.manifest);
    let mut staged_names = std::collections::BTreeSet::new();
    for artifact in &recorded_macros {
        let src = Path::new(artifact);
        if let Some(name) = src.file_name()
            && src.is_file()
        {
            copy(src, &host_deps_out.join(name))?;
            staged_names.insert(name.to_os_string());
        }
    }
    let swept = copy_dylibs(&sdk.host_deps, &host_deps_out)?;
    let macros = staged_names.len().max(swept);

    // Host tools next to the editor.
    copy_into(&sdk.wrapper, out)?;

    // The SDK's exact lockfile and toolchain, so an extension resolves the
    // shared closure at the same versions and compiles with the same rustc.
    if sdk.lockfile.is_file() {
        copy(&sdk.lockfile, &out.join("Cargo.lock"))?;
    }
    if let Some(channel) = &sdk.toolchain {
        write(&out.join("toolchain.txt"), &format!("{channel}\n"))?;
    }

    Ok(format!(
        "{rlibs}+{extra_rlibs} rlibs, {cdylibs} runtime cdylibs, {macros} proc-macro dylibs, \
         {native_libs} native import libs"
    ))
}

/// Generate `sdk.manifest` from the (already built) release editor when a
/// prior build has not left one. A no-op once present; in CI the editor is
/// compiled first, so this just re-reads its artifact list.
fn ensure_manifest(workspace: &Path, sdk: &SdkPaths) -> Result<(), String> {
    // Present is not the same as current. A manifest names one build's
    // exact rlib filenames, so an SDK rebuilt since leaves it pointing at
    // artifacts from before: the bundle then redirects consumers to an
    // older `wgpu_hal` than the one their own graph resolves, and the
    // extension fails to compile with two versions of a crate in scope. The
    // bundle is the one artifact a user cannot repair, so this is checked
    // rather than assumed.
    let stale = match (
        std::fs::metadata(&sdk.manifest).and_then(|m| m.modified()),
        std::fs::metadata(&sdk.dylib).and_then(|m| m.modified()),
    ) {
        (Ok(manifest), Ok(dylib)) => manifest < dylib,
        _ => false,
    };
    if sdk.manifest.is_file() && !stale {
        return Ok(());
    }
    // `--bins`, matching the build that produced the binaries being
    // staged. Without it cargo resolves a different configuration of the
    // workspace crates: the two sets coexist in the target dir, so this
    // step recompiled seven crates and relinked `jackdaw` every time,
    // twenty minutes on Linux and forty on Windows. Worse than the cost,
    // it left the shipped editor built from one resolution while the
    // manifest and SDK dylib beside it came from another, and those are
    // meant to be the single compilation that makes the editor and a
    // loaded extension agree on `TypeId`.
    SdkManifest::generate(
        workspace,
        sdk,
        &[
            "-p",
            "jackdaw",
            "--features",
            "dylib",
            "--release",
            "--bins",
        ],
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
            copy_import_lib(&path, to)?;
            count += 1;
        }
    }
    Ok(count)
}

/// Stage a Windows DLL's import library beside it. Linking against a
/// DLL goes through its `<name>.dll.lib`, which cargo writes as a
/// sibling; a bundle that ships only the DLL loads at runtime but
/// cannot be linked against, so every extension build against the staged
/// SDK failed with every cross-boundary symbol undefined at once. ELF
/// and Mach-O link against the shared object directly, so elsewhere the
/// sibling never exists and this is a no-op.
fn copy_import_lib(dylib: &Path, to: &Path) -> Result<(), String> {
    let Some(name) = dylib.file_name() else {
        return Ok(());
    };
    let mut lib_name = name.to_os_string();
    lib_name.push(".lib");
    let sibling = dylib.with_file_name(&lib_name);
    if sibling.is_file() {
        copy(&sibling, &to.join(&lib_name))?;
    }
    Ok(())
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

/// Copy the native import libraries the SDK's crates link by bare name
/// into the bundle, so a machine without the build machine's cargo
/// registry can still resolve them.
///
/// Only the libraries themselves travel: the recorded search paths are
/// absolute into `~/.cargo/registry` and are meaningless anywhere else,
/// so the bundle records its own directory instead. A no-op where the
/// SDK recorded no paths, which is every platform whose native
/// libraries are system wide.
fn stage_native_libs(sdk: &SdkPaths, native_out: &Path) -> Result<usize, String> {
    let recorded = jackdaw_project_build::plan::read_link_paths(&sdk.manifest);
    if recorded.is_empty() {
        return Ok(0);
    }
    std::fs::create_dir_all(native_out).map_err(|e| format!("{}: {e}", native_out.display()))?;
    let mut copied = 0usize;
    for dir in recorded {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_lib = path
                .extension()
                .is_some_and(|ext| ext == "lib" || ext == "a");
            if !is_lib {
                continue;
            }
            let Some(name) = path.file_name() else {
                continue;
            };
            copy(&path, &native_out.join(name))?;
            copied += 1;
        }
    }
    Ok(copied)
}
