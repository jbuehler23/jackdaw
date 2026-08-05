//! Editor-driven project builds: from an open project to a loadable
//! project dylib.
//!
//! The pipeline, per build:
//!
//! 1. [`shim::ensure_shim`]: the generated shim crate in
//!    `.jackdaw/shim/` is the build root; the user's crate is its path
//!    dependency.
//! 2. The SDK's `Cargo.lock` seeds the shim when the SDK identity changes so
//!    closure resolves at the SDK's exact versions (the user's own
//!    lock is never touched).
//! 3. [`plan::write_plan`]: the per-edge extern redirect plan for the
//!    rustc wrapper.
//! 4. `cargo rustc --crate-type dylib --target <triple>` through the
//!    wrapper, into a target dir keyed by their contents (cargo does not
//!    fingerprint wrapper behavior, so a changed plan must not reuse
//!    stale units).
//! 5. [`linkage::verify_linkage`]: the artifact provably links the
//!    running SDK before anything dlopens it.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::linkage;
use crate::plan::{self, PlanError, SdkManifest};
use crate::schema;
use crate::sdk_paths::SdkPaths;
use crate::shim::{self, ShimSpec};

#[derive(Debug)]
pub enum ProjectBuildError {
    Io(std::io::Error),
    Plan(PlanError),
    Linkage(linkage::LinkageError),
    /// The project compile itself failed; the log carries rustc's
    /// diagnostics for the problems panel.
    Compile {
        log: String,
    },
    /// The SDK this build would link against is not in a usable state.
    /// Checked before any compilation so the user is not told ten
    /// minutes in, by way of a rustc error that does not name the
    /// cause.
    UnusableSdk {
        problems: Vec<String>,
    },
}

impl std::fmt::Display for ProjectBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Plan(e) => write!(f, "plan generation failed: {e}"),
            Self::Linkage(e) => write!(f, "linkage verification failed: {e}"),
            Self::Compile { .. } => write!(f, "project failed to compile"),
            // The reporter has already printed the detail line by line;
            // repeating it here made the same paths appear three times
            // in one failure. Point at the report instead.
            Self::UnusableSdk { .. } => {
                write!(f, "the Jackdaw SDK is not usable (see above)")
            }
        }
    }
}

impl std::error::Error for ProjectBuildError {}

impl From<std::io::Error> for ProjectBuildError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<PlanError> for ProjectBuildError {
    fn from(e: PlanError) -> Self {
        Self::Plan(e)
    }
}

/// Live progress from a project build. Bevy-light so the pipeline crate
/// stays independent of the editor; the editor maps these onto its
/// `BuildProgress` sink for the footer bar and the build log.
pub enum BuildEvent {
    /// cargo finished one compile unit; `crate_name` is the unit it just
    /// produced, `done` the running count, and `fresh` whether it was a
    /// cache hit (so the UI can skip logging cached crates as compiling).
    Compiled {
        crate_name: String,
        done: u32,
        fresh: bool,
    },
    /// A line of rendered build output (a diagnostic or status note).
    Log(String),
}

/// A completed project build.
pub struct ProjectBuild {
    /// The verified project dylib.
    pub dylib: PathBuf,
    /// Redirect edges in the plan.
    pub edges: usize,
    /// The project's type schema, extracted out-of-process. `None`
    /// when extraction could not run (e.g. the runner is not built);
    /// the editor keeps its previous schema in that case.
    pub schema: Option<schema::ProjectSchema>,
}

/// Separator for the native link search path list handed to the
/// wrapper. Paths can contain spaces, and on Windows a colon follows
/// every drive letter, so neither is usable.
const PATH_LIST_SEPARATOR: &str = "\n";

/// Whether the SDK library is newer than the manifest describing it.
///
/// The manifest is a cache of one build's artifact filenames, and
/// rebuilding the SDK changes them. Nothing else notices: the old files
/// are still on disk, so the plan resolves and the failure only appears
/// inside rustc, against a transitive crate.
fn manifest_predates_sdk(sdk: &SdkPaths) -> bool {
    let modified = |path: &Path| std::fs::metadata(path).and_then(|m| m.modified()).ok();
    match (modified(&sdk.manifest), modified(&sdk.dylib)) {
        (Some(manifest), Some(dylib)) => manifest < dylib,
        // No manifest is handled by the caller; an unreadable SDK is
        // reported before this runs.
        _ => false,
    }
}

/// A warning when a source checkout's SDK predates the editor using it.
///
/// Only for that origin: a bundle ships both together, and a prepared
/// cache is rebuilt whenever the recipe changes. In a checkout they are
/// separate build commands, and `cargo run` does not touch the triple
/// dir the SDK lives in, so rebuilding the editor alone leaves the two
/// describing different code.
fn sdk_older_than_editor(sdk: &SdkPaths) -> Option<String> {
    if sdk.origin != crate::sdk_paths::SdkOrigin::DevCheckout {
        return None;
    }
    let modified = |path: &Path| std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let sdk_built = modified(&sdk.dylib)?;
    let editor_built = modified(&std::env::current_exe().ok()?)?;
    if sdk_built >= editor_built {
        return None;
    }
    Some(format!(
        "warning: the SDK at {} is older than the editor running it, so this build may not \
         link. Rebuild it with `cargo build -p jackdaw --features dylib --release --target {}`, \
         or use the SDK from `jd setup`.",
        sdk.dylib.display(),
        sdk.triple,
    ))
}

/// What to do about an unusable SDK. Which of the three install paths
/// is in play decides the answer, and the commonest cause is an
/// override aimed at a directory that holds no SDK while a prepared one
/// exists.
pub fn sdk_remedy(sdk: &SdkPaths) -> String {
    if sdk.origin == crate::sdk_paths::SdkOrigin::Override {
        let prepared = crate::bootstrap::cache_dir()
            .map(|dir| format!(" (a prepared one is at {})", dir.display()))
            .unwrap_or_default();
        return format!(
            "unset JACKDAW_SDK_DIR to use the SDK this jackdaw found for itself{prepared}"
        );
    }
    "run `jd setup` to prepare one".to_string()
}

/// Run the full pipeline for one project. `jackdaw_dir` is the
/// project's `.jackdaw/`; `dev_workspace` points at the jackdaw
/// checkout in dev runs (where the SDK manifest is generated rather
/// than shipped).
pub fn build_project_dylib(
    spec: &ShimSpec,
    jackdaw_dir: &Path,
    sdk: &SdkPaths,
    dev_workspace: Option<&Path>,
    report: &mut dyn FnMut(BuildEvent),
) -> Result<ProjectBuild, ProjectBuildError> {
    // Checked before the shim is even written: an SDK with no library or
    // no wrapper fails the same way whatever the project is, and it does
    // so minutes in. Reported through the log too, so the editor's build
    // panel shows what to fix instead of a bare failure line.
    let problems = sdk.problems();
    if !problems.is_empty() {
        report(BuildEvent::Log(format!(
            "the Jackdaw SDK at {} is not usable:",
            sdk.dylib.display()
        )));
        for problem in &problems {
            report(BuildEvent::Log(format!("  {problem}")));
        }
        report(BuildEvent::Log(format!("  fix: {}", sdk_remedy(sdk))));
        return Err(ProjectBuildError::UnusableSdk { problems });
    }

    // A checkout builds the editor and its SDK with separate commands
    // (`cargo run` writes neither into the triple dir), so they drift
    // apart silently. The project links the SDK, so an SDK older than
    // the editor running it fails linkage verification after a full
    // build. Say so in the second it costs to compare two timestamps.
    if let Some(note) = sdk_older_than_editor(sdk) {
        report(BuildEvent::Log(note));
    }

    let shim_dir = shim::ensure_shim(spec, jackdaw_dir)?;

    // Pin the shim's toolchain to the SDK's. The shim is outside the
    // user's workspace, so it would otherwise build with the ambient
    // default toolchain and its rlibs would be rejected as compiled by
    // an incompatible rustc. This file lives only in the generated
    // shim; the user's own project builds keep their own toolchain.
    if let Some(channel) = &sdk.toolchain {
        let contents = format!("[toolchain]\nchannel = \"{channel}\"\n");
        let path = shim_dir.join("rust-toolchain.toml");
        let stale = std::fs::read_to_string(&path).ok();
        if stale.as_deref() != Some(&contents) {
            std::fs::write(&path, contents)?;
        }
    }

    // A manifest that predates its SDK names artifacts from an older
    // build. Those files usually still exist, so nothing looks wrong
    // until rustc reports that a crate they were compiled against has
    // been replaced (`E0460: found possibly newer version of crate ...`)
    // naming a crate the user has never heard of. Only checked where it
    // can be regenerated: a shipped SDK has no workspace to rebuild
    // from, and its manifest and dylib are extracted together anyway.
    //
    // Two signals, because neither catches the other. An mtime older
    // than the SDK means it describes a previous build; artifacts that
    // no longer exist mean cargo has since pruned what it named. A
    // manifest can fail either check while passing the other.
    let mut stale = dev_workspace.is_some() && manifest_predates_sdk(sdk);
    if stale {
        report(BuildEvent::Log(
            "the SDK has been rebuilt since its manifest was written; regenerating it".to_string(),
        ));
    }
    if !stale && dev_workspace.is_some() {
        let missing = SdkManifest::load(&sdk.manifest)
            .map(|manifest| manifest.missing_artifacts(3))
            .unwrap_or_default();
        if !missing.is_empty() {
            report(BuildEvent::Log(format!(
                "the SDK manifest names artifacts that are gone ({}); regenerating it",
                missing.join(", ")
            )));
            stale = true;
        }
    }
    let manifest = match SdkManifest::load(&sdk.manifest) {
        Ok(manifest) if !manifest.is_empty() && !stale => manifest,
        _ => match dev_workspace {
            Some(root) => SdkManifest::generate_dev(root, sdk)?,
            None => {
                return Err(ProjectBuildError::Plan(PlanError::Parse(format!(
                    "SDK manifest missing at {}",
                    sdk.manifest.display()
                ))));
            }
        },
    };

    // Seed the shim with the SDK's lockfile so the shared dependency
    // closure resolves to the exact versions the SDK was built with.
    // A freshly created project otherwise drifts to newer patch
    // releases (a bumped transitive dep pulling a newer libc), and a
    // redirected rlib built against the SDK's version then clashes with
    // the project's. The user's own extra dependencies resolve on top,
    // as new lock entries, untouched.
    //
    // After the manifest, not before: the SDK's identity is partly the
    // manifest's contents, so hashing it first turned a manifest that
    // had yet to be generated into a bare "No such file or directory"
    // from the wrong end of the pipeline.
    if sdk.lockfile.is_file() {
        let identity = sdk_identity(sdk)?;
        let identity_path = shim_dir.join(".jackdaw-sdk-id");
        let prior_identity = std::fs::read_to_string(&identity_path).ok();
        if prior_identity.as_deref() != Some(identity.as_str()) {
            let lockfile = std::fs::read(&sdk.lockfile)?;
            let shim_lock = shim_dir.join("Cargo.lock");
            if std::fs::read(&shim_lock).ok().as_deref() != Some(lockfile.as_slice()) {
                std::fs::write(shim_lock, lockfile)?;
            }
            std::fs::write(identity_path, &identity)?;
        }
    }

    let plan_path = jackdaw_dir.join("plan.txt");
    let edges = plan::write_plan(&shim_dir, &manifest, &sdk.deps, &plan_path)?;

    // Key the target dir by everything cargo cannot fingerprint: the
    // plan and wrapper contents.
    let salt = build_salt(&plan_path, &sdk.wrapper)?;
    let target_root = jackdaw_dir.join("target");
    let target_dir = target_root.join(&salt);
    retain_recent_target_dirs(&target_root, &salt);

    let mut cmd = Command::new("cargo");
    cmd.args(["rustc", "--crate-type", "dylib", "--target", &sdk.triple])
        // Machine-readable stream so the editor can show live per-crate
        // progress, with rustc's diagnostics in it.
        //
        // Plain `json`, deliberately, not `json-render-diagnostics`:
        // that variant makes cargo render diagnostics to its own stderr
        // and emit no `compiler-message` at all, so this parser saw none
        // and every failed build reported "no diagnostics captured"
        // while the actual error went to a stream nothing reads. The
        // editor showed the user nothing, and a failing release job
        // reported an empty log beside a compiler error only a human
        // scrolling the raw output would find.
        .arg("--message-format=json")
        // Strip the shim: the editor loads it for its types and entry,
        // never debugs it, so the embedded debuginfo (the bulk of the
        // artifact, since the shim statically links the project's own
        // crates) is dead weight. Stripping shrinks it by roughly an
        // order of magnitude, which matters because a loaded dylib is
        // never unloaded, so every reload's pages stay resident.
        .args(["--", "-C", "strip=symbols"])
        .current_dir(&shim_dir)
        // Incremental codegen and dylib linking do not mix: incremental
        // reuse can leave references to local anon constants that live
        // hidden inside the SDK, failing the link with undefined hidden
        // symbols.
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", &target_dir)
        .env("RUSTC_WRAPPER", &sdk.wrapper)
        .env("JACKDAW_SDK_DYLIB", &sdk.dylib)
        .env("JACKDAW_SDK_DEPS", &sdk.deps)
        .env("JACKDAW_SDK_HOST_DEPS", &sdk.host_deps)
        .env("JACKDAW_SDK_EXTERN_MAP", &plan_path);
    // Directories holding native import libraries the SDK's crates
    // reference. Their `#[link]` directives reach a consumer through
    // crate metadata, but the search paths only ever existed in the
    // SDK's own build, so without these the link fails on a bare
    // library name it cannot find.
    let link_paths = plan::read_link_paths(&sdk.manifest);
    if !link_paths.is_empty() {
        cmd.env(
            "JACKDAW_SDK_LINK_PATHS",
            link_paths.join(PATH_LIST_SEPARATOR),
        );
    }
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;

    // Drain the JSON stream: bump the compile counter per artifact and
    // forward rendered diagnostics, both to the live `report` sink and
    // into `log`, which becomes the compile error on failure.
    let stdout = child.stdout.take().expect("piped stdout");
    let mut done = 0u32;
    let mut log = String::new();
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        parse_build_line(&line, &mut done, &mut log, report);
    }
    let status = child.wait()?;
    if !status.success() {
        if log.trim().is_empty() {
            log.push_str("project failed to compile (no diagnostics captured)");
        }
        if let Some(note) = stale_sdk_hint(&log, sdk, &manifest) {
            log.push_str(&note);
        }
        return Err(ProjectBuildError::Compile { log });
    }

    let dylib = target_dir
        .join(&sdk.triple)
        .join("debug")
        .join(dylib_file_name("jackdaw_shim"));
    // Prove the artifact imports the exact SDK facade before anything dlopens
    // it. The facade in turn imports the shipped Bevy and Jackdaw runtimes.
    linkage::verify_linkage(&dylib, &sdk.dylib, sdk.toolchain.as_deref())
        .map_err(ProjectBuildError::Linkage)?;

    // Extract the project's type schema out-of-process so the editor
    // learns its components without mapping the dylib. A missing runner
    // or a failed extraction is not fatal: the editor keeps its prior
    // schema and Play still works.
    let schema = match schema::run_extractor(&sdk.runner, &dylib) {
        Ok(schema) => Some(schema),
        Err(err) => {
            tracing::warn!("project schema extraction skipped: {err}");
            None
        }
    };

    // Persist the schema so pickup is decoupled from building: the
    // editor watches this file and refreshes its component types when it
    // changes, whether this build ran in-process or from a terminal
    // `jackdaw build`. A write failure is not fatal to the build.
    if let Some(schema) = &schema
        && let Err(err) = schema::write_schema(jackdaw_dir, schema)
    {
        tracing::warn!("failed to persist project schema: {err}");
    }

    Ok(ProjectBuild {
        dylib,
        edges,
        schema,
    })
}

/// Turn one line of `cargo --message-format=json-render-diagnostics` into a
/// [`BuildEvent`]: a finished compile unit bumps `done`; a rendered
/// diagnostic becomes log lines (also accumulated into `log` for the
/// compile error). Non-JSON or other records are ignored.
fn parse_build_line(
    line: &str,
    done: &mut u32,
    log: &mut String,
    report: &mut dyn FnMut(BuildEvent),
) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return;
    };
    match value.get("reason").and_then(serde_json::Value::as_str) {
        Some("compiler-artifact") => {
            *done += 1;
            let crate_name = value
                .get("target")
                .and_then(|t| t.get("name"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let fresh = value
                .get("fresh")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            report(BuildEvent::Compiled {
                crate_name,
                done: *done,
                fresh,
            });
        }
        Some("compiler-message") => {
            if let Some(rendered) = value
                .get("message")
                .and_then(|m| m.get("rendered"))
                .and_then(serde_json::Value::as_str)
            {
                for l in rendered.lines() {
                    log.push_str(l);
                    log.push('\n');
                    report(BuildEvent::Log(l.to_string()));
                }
            }
        }
        _ => {}
    }
}

/// Build a [`ShimSpec`] for a project from the filesystem alone, no
/// running editor required. `configured_plugin` is the game plugin the
/// editor's run config named, when there is one; `None` falls back to
/// the project's `jackdaw.toml`, then source detection, then the
/// `GamePlugin` convention. Shared by the editor's in-process build and
/// `jd build`.
///
/// In a workspace the target member comes from `jackdaw.toml`'s
/// `package` key, or from the one member that depends on Bevy. The
/// shim's path dependency and the source scans then point at that
/// member's directory, not at the opened root.
pub fn shim_spec_for_project(root: &Path, configured_plugin: Option<String>) -> Option<ShimSpec> {
    let manifest = crate::project_manifest::ProjectManifest::read(root);
    let package =
        crate::cargo_meta::resolve_project_package(root, manifest.package.as_deref()).ok()?;
    let package_dir = &package.dir;
    let extension_type = crate::detect::detect_extension(package_dir).map(|(_, name)| name);
    let detected_plugin = crate::detect::detect_plugin(package_dir, &package.crate_name)
        .and_then(|p| p.split_once("::").map(|(_, name)| name.to_string()));
    // The shim pastes this name into `app.add_plugins(<crate>::<name>)`,
    // so naming a type the crate does not define turns into a compile
    // error inside a generated crate the user never wrote and is told
    // not to edit. Every source of the name is checked against what the
    // crate actually declares, including a configured one: a stale
    // `plugin` key in `jackdaw.toml` (or one written before the type
    // was renamed) must not be trusted blindly.
    //
    // A project with no plugin at all (a plain component library) is a
    // valid shape: its shim omits the game entry and still contributes
    // the crate's reflected types.
    let found = crate::detect::plugin_paths(package_dir);
    let resolve = |name: &str| {
        found
            .iter()
            .find(|candidate| {
                candidate.type_name == name || candidate.crate_path.as_deref() == Some(name)
            })
            .and_then(|candidate| candidate.crate_path.clone())
    };
    let game_plugin = configured_plugin
        .or(manifest.plugin)
        .and_then(|configured| {
            let resolved = resolve(&configured);
            if resolved.is_none() {
                tracing::warn!(
                    "jackdaw.toml names plugin `{configured}`, which {} does not declare; \
                     ignoring it",
                    package.crate_name
                );
            }
            resolved
        })
        .or(detected_plugin)
        .or_else(|| {
            (extension_type.is_none() && resolve("GamePlugin").is_some())
                .then(|| "GamePlugin".to_string())
        });
    Some(ShimSpec {
        package_name: package.name,
        crate_name: package.crate_name,
        project_root: package.dir,
        game_plugin,
        extension_type,
    })
}

/// The most recently built project dylib under `<jackdaw_dir>/target`,
/// if there is one. The build keys its target directory by a salt that
/// only the build knows, so consumers that just want "whatever was built
/// last" (packaging, tooling) search rather than recompute it.
pub fn last_built_dylib(jackdaw_dir: &Path) -> Option<PathBuf> {
    let wanted = dylib_file_name("jackdaw_shim");
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    // <jackdaw_dir>/target/<salt>/<triple>/debug/<dylib>
    for salt in std::fs::read_dir(jackdaw_dir.join("target"))
        .ok()?
        .flatten()
    {
        let Ok(triples) = std::fs::read_dir(salt.path()) else {
            continue;
        };
        for triple in triples.flatten() {
            let candidate = triple.path().join("debug").join(&wanted);
            let Ok(modified) = candidate.metadata().and_then(|meta| meta.modified()) else {
                continue;
            };
            if newest.as_ref().is_none_or(|(best, _)| modified > *best) {
                newest = Some((modified, candidate));
            }
        }
    }
    newest.map(|(_, path)| path)
}

/// The crate name a rustc "can't find crate for" line names, if that is
/// what the line is.
fn unresolved_crate_name(line: &str) -> Option<String> {
    let rest = line.split_once("can't find crate for `")?.1;
    let name = rest.split_once('`')?.0;
    (!name.is_empty()).then(|| name.replace('-', "_"))
}

/// Turn "can't find crate for X" into something the user can act on.
///
/// The project compiles against prebuilt SDK rlibs, so rustc failing to
/// resolve one of the SDK's own crates means those artifacts do not
/// agree with each other, not that the user's code is wrong. It reads
/// as a bewildering error about a crate they have never heard of,
/// arriving after ten minutes of compiling. A dev checkout hits this
/// most: its `target/` accumulates several builds of the same crate
/// under different feature resolutions, and the SDK there is preferred
/// over the bootstrapped cache.
fn stale_sdk_hint(log: &str, sdk: &SdkPaths, manifest: &SdkManifest) -> Option<String> {
    // Both shapes rustc uses when the SDK's own artifacts disagree.
    // E0463 is a crate it cannot find at all; E0460 is one it found
    // several of, none matching what the dependent was built against.
    //
    // `which X depends on` is what separates those from a user's own
    // missing dependency: rustc only adds it when the crate is needed to
    // load metadata for an rlib it was handed, which is to say one the
    // redirect plan pointed at. A crate the project itself names is
    // reported without it. Listing SDK crate names instead missed
    // `jackdaw_api_internal`, which is exactly the sort of transitive
    // crate that fails this way.
    //
    // A bare `can't find crate for X` also counts when X is one the SDK
    // holds. rustc words it that way when the `--extern` flag is absent
    // altogether, which is not something a redirect does, and it is
    // otherwise indistinguishable from a dependency the user forgot to
    // declare. Naming the SDK is what makes it clear which side to look
    // at: a macOS bundle failed exactly this way on `image`, a crate no
    // one involved had ever named.
    let unresolved = log
        .lines()
        .find(|line| {
            (line.contains("can't find crate for")
                || line.contains("found possibly newer version of crate"))
                && line.contains("depends on")
        })
        .or_else(|| {
            log.lines().find(|line| {
                unresolved_crate_name(line)
                    .is_some_and(|name| manifest.artifact_for(&name).is_some())
            })
        })?;
    Some(format!(
        "\nnote: {}\n\
         note: that is an SDK crate, so this is a mismatch between the prebuilt SDK \
         artifacts rather than an error in your project.\n\
         note: the SDK in use is {}\n\
         note: if that is a source checkout, delete {} and build again; it is a cache of one \
         build's filenames and is rewritten automatically.\n\
         note: failing that, `cargo clean` in the checkout, or unset it so the SDK prepared by \
         `jd setup` is used instead.\n",
        unresolved.trim(),
        sdk.deps.display(),
        sdk.manifest.display(),
    ))
}

/// How many keyed target directories to keep, including the current
/// one. Each holds a full compilation of the project's dependency
/// closure, bevy included, so this trades disk against the ten-plus
/// minutes it takes to rebuild one.
const TARGET_DIRS_KEPT: usize = 2;

/// Drop old keyed target directories, keeping the current one and the
/// most recently created others.
///
/// Ordering is by directory mtime, which is creation time here: cargo
/// writes nested files, which does not touch the salt directory itself.
/// With only two kept that is enough to make an A/B flip stable, and
/// avoids maintaining a usage stamp for a heuristic this coarse.
///
/// Deleting every other directory outright made switching build
/// configuration cost a full rebuild of bevy every time. The salt
/// covers the wrapper binary, so alternating a debug and a release
/// editor (or upgrading and rolling back) flips it back and forth, and
/// each flip threw away a cache that was about to be wanted again.
/// Keeping the previous one makes that flip nearly free while still
/// bounding growth.
fn retain_recent_target_dirs(target_root: &Path, current: &str) {
    let Ok(entries) = std::fs::read_dir(target_root) else {
        return;
    };
    let mut others: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy() != current)
        .filter_map(|entry| {
            let used = entry.metadata().and_then(|meta| meta.modified()).ok()?;
            Some((used, entry.path()))
        })
        .collect();
    // Newest first; keep enough to leave room for the current one.
    others.sort_by_key(|(used, _)| std::cmp::Reverse(*used));
    for (_, path) in others.into_iter().skip(TARGET_DIRS_KEPT.saturating_sub(1)) {
        let _ = std::fs::remove_dir_all(path);
    }
}

fn build_salt(plan_path: &Path, wrapper: &Path) -> std::io::Result<String> {
    let mut hasher = DefaultHasher::new();
    std::fs::read(plan_path)?.hash(&mut hasher);
    std::fs::read(wrapper)?.hash(&mut hasher);
    Ok(format!("{:016x}", hasher.finish()))
}

fn sdk_identity(sdk: &SdkPaths) -> std::io::Result<String> {
    let mut hasher = DefaultHasher::new();
    sdk.triple.hash(&mut hasher);
    sdk.toolchain.hash(&mut hasher);
    for path in [&sdk.lockfile, &sdk.manifest, &sdk.wrapper] {
        std::fs::read(path)?.hash(&mut hasher);
    }
    Ok(format!("{:016x}", hasher.finish()))
}

fn dylib_file_name(crate_name: &str) -> String {
    format!(
        "{}{crate_name}{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk_paths::host_triple;

    /// A failed build has to carry the compiler's own words. This is the
    /// half of that which can be checked without running cargo: the
    /// other half is passing `--message-format=json`, because
    /// `json-render-diagnostics` emits no `compiler-message` at all and
    /// leaves this parser with nothing to find.
    #[test]
    fn a_compiler_message_reaches_the_failure_log() {
        let line = r#"{"reason":"compiler-message","target":{"name":"bevy_image"},"message":{"rendered":"error[E0463]: can't find crate for `image`\n"}}"#;
        let mut done = 0;
        let mut log = String::new();
        let mut seen = Vec::new();
        parse_build_line(line, &mut done, &mut log, &mut |event| {
            if let BuildEvent::Log(l) = event {
                seen.push(l);
            }
        });
        assert!(log.contains("can't find crate for `image`"), "{log:?}");
        assert_eq!(seen.len(), 1, "the live sink gets it too: {seen:?}");
    }

    /// A manifest holding just `names`, so the hint can tell an SDK
    /// crate from one the user forgot to declare.
    fn sdk_manifest_with(names: &[&str]) -> SdkManifest {
        // Keyed by the names themselves: tests run in parallel, and two
        // that happened to ask for the same count shared one path, so
        // one deleted the file the other was reading.
        let path = std::env::temp_dir().join(format!(
            "jackdaw_hint_manifest_{}_{}.txt",
            std::process::id(),
            names.join("_")
        ));
        let body: String = names
            .iter()
            .map(|n| format!("{n} 1.0.0 lib{n}-abc.rlib\n"))
            .collect();
        std::fs::write(&path, body).expect("write a manifest");
        let manifest = SdkManifest::load(&path).expect("load it back");
        let _ = std::fs::remove_file(&path);
        manifest
    }

    fn project(name: &str, lib_rs: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "jackdaw_spec_{name}_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nbevy = \"0.19\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("src/lib.rs"), lib_rs).unwrap();
        dir
    }

    /// A component library with no plugin is a valid project. Naming a
    /// `GamePlugin` it does not define would fail the build inside the
    /// generated shim, where the user cannot see or fix it.
    #[test]
    fn a_crate_without_a_plugin_gets_no_game_entry() {
        let dir = project(
            "noplugin",
            "use bevy::prelude::*;\n\
             #[derive(Component, Reflect)]\npub struct Health(f32);\n",
        );
        let spec = shim_spec_for_project(&dir, None).expect("spec");
        assert_eq!(spec.game_plugin, None);
        assert!(!shim::lib_source_for_test(&spec).contains("add_plugins"));
        // The schema extractor is still emitted: the crate's reflected
        // types are the point of building it.
        assert!(shim::lib_source_for_test(&spec).contains("jackdaw_extract_schema"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A `plugin` key naming a type the crate does not define (stale,
    /// renamed, or written by an older importer) must not reach the
    /// shim: it would fail the build inside the generated crate.
    /// A manifest older than the SDK it describes names artifacts from
    /// a previous build. They are usually still on disk, so the plan
    /// resolves and the failure surfaces only inside rustc, against a
    /// transitive crate the user has never heard of.
    #[test]
    fn a_manifest_older_than_its_sdk_is_stale() {
        let dir = std::env::temp_dir().join(format!("jackdaw_manifest_age_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let triple_dir = dir.join("target").join(host_triple()).join("release");
        std::fs::create_dir_all(&triple_dir).unwrap();
        let sdk = SdkPaths::for_workspace_profile(&dir, "release");

        std::fs::write(&sdk.manifest, b"").unwrap();
        std::fs::write(&sdk.dylib, b"sdk").unwrap();
        // Back-date the manifest rather than sleeping: filesystem mtime
        // granularity is coarse enough that two adjacent writes can
        // share a timestamp.
        let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(&sdk.manifest)
            .unwrap()
            .set_modified(hour_ago)
            .unwrap();
        assert!(
            manifest_predates_sdk(&sdk),
            "an SDK rebuilt after its manifest is stale"
        );

        // Rewriting the manifest is what regeneration does at the end of
        // `SdkManifest::generate`, and it clears the staleness.
        std::fs::write(&sdk.manifest, b"bevy 0.19.0 libbevy.rlib\n").unwrap();
        assert!(
            !manifest_predates_sdk(&sdk),
            "a manifest written after the SDK is current"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Absent files are the caller's business: a missing manifest is
    /// generated, and an unusable SDK is reported before this runs.
    #[test]
    fn a_missing_manifest_or_sdk_is_not_reported_as_stale() {
        let dir =
            std::env::temp_dir().join(format!("jackdaw_manifest_gone_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("target").join(host_triple()).join("release")).unwrap();
        let sdk = SdkPaths::for_workspace_profile(&dir, "release");
        assert!(!manifest_predates_sdk(&sdk));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_configured_plugin_is_not_trusted() {
        let dir = project("stale-key", "pub fn helper() {}\n");
        std::fs::write(
            dir.join("jackdaw.toml"),
            "plugin = \"GamePlugin\"\n[[run]]\nname = \"Play\"\n",
        )
        .unwrap();
        let spec = shim_spec_for_project(&dir, None).expect("spec");
        assert_eq!(spec.game_plugin, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A configured plugin that does exist is honoured, and resolved to
    /// the path the shim can actually name it by.
    #[test]
    fn a_configured_plugin_resolves_to_its_module_path() {
        let dir = project("configured", "pub mod game;\n");
        std::fs::write(
            dir.join("src/game.rs"),
            "impl Plugin for WorldPlugin { fn build(&self, _: &mut App) {} }\n",
        )
        .unwrap();
        std::fs::write(dir.join("jackdaw.toml"), "plugin = \"WorldPlugin\"\n").unwrap();
        let spec = shim_spec_for_project(&dir, None).expect("spec");
        assert_eq!(spec.game_plugin.as_deref(), Some("game::WorldPlugin"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_game_plugin_convention_still_applies_when_the_type_exists() {
        let dir = project(
            "convention",
            "use bevy::prelude::*;\npub struct GamePlugin;\n\
             impl Plugin for GamePlugin { fn build(&self, _: &mut App) {} }\n",
        );
        let spec = shim_spec_for_project(&dir, None).expect("spec");
        assert_eq!(spec.game_plugin.as_deref(), Some("GamePlugin"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An SDK-crate resolution failure is not the user's bug, and says
    /// so, naming the SDK actually in use.
    #[test]
    fn an_unresolvable_sdk_crate_is_explained() {
        let sdk = SdkPaths::for_workspace_profile(Path::new("/checkout"), "release");
        let log = "error[E0463]: can't find crate for `jackdaw_api_macros` which \
                   `jackdaw_api` depends on\n";
        let hint = stale_sdk_hint(log, &sdk, &sdk_manifest_with(&["jackdaw_api_macros"]))
            .expect("an SDK crate should be recognised");
        assert!(hint.contains("rather than an error in your project"));
        assert!(hint.contains("/checkout"), "names the SDK in use: {hint}");
    }

    /// The failure that reached a user: a transitive SDK crate whose
    /// build no longer matches what the redirected rlib expects. It is
    /// `E0460` rather than `E0463`, and names a crate no fixed list
    /// thought to include.
    #[test]
    fn a_replaced_transitive_sdk_crate_is_explained() {
        let sdk = SdkPaths::for_workspace_profile(Path::new("/checkout"), "release");
        let log = "error[E0460]: found possibly newer version of crate \
                   `jackdaw_api_internal` which `jackdaw_api` depends on\n";
        let hint = stale_sdk_hint(log, &sdk, &sdk_manifest_with(&["jackdaw_api_internal"]))
            .expect("E0460 is an SDK mismatch too");
        assert!(
            hint.contains("jackdaw_sdk_manifest.txt"),
            "points at the cache to delete: {hint}"
        );
    }

    /// A genuine error in the user's own code must not be blamed on the
    /// SDK.
    #[test]
    fn a_users_own_missing_crate_gets_no_sdk_hint() {
        let sdk = SdkPaths::for_workspace_profile(Path::new("/checkout"), "release");
        let log = "error[E0463]: can't find crate for `rand`\n";
        assert!(stale_sdk_hint(log, &sdk, &sdk_manifest_with(&["glam"])).is_none());
    }

    /// The shape a macOS bundle failed with: a bare `can't find crate`
    /// naming a crate the SDK holds and the user never mentioned. rustc
    /// words it that way when the `--extern` flag is absent altogether,
    /// with no `depends on` clause to key off, so it is otherwise
    /// indistinguishable from a dependency the project forgot.
    #[test]
    fn a_bare_missing_sdk_crate_is_still_attributed_to_the_sdk() {
        let sdk = SdkPaths::for_workspace_profile(Path::new("/checkout"), "release");
        let log = "error[E0463]: can't find crate for `image`\n";
        let hint = stale_sdk_hint(log, &sdk, &sdk_manifest_with(&["image", "glam"]))
            .expect("a crate the SDK holds points at the SDK");
        assert!(
            hint.contains("rather than an error in your project"),
            "{hint}"
        );
    }

    /// Switching build configuration used to delete the cache it was
    /// about to want back, costing a full rebuild of bevy each time.
    #[test]
    fn the_previous_target_dir_survives_a_configuration_flip() {
        let root = std::env::temp_dir().join(format!("jackdaw-retain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for salt in ["aaa", "bbb", "ccc"] {
            std::fs::create_dir_all(root.join(salt)).unwrap();
            // Distinct mtimes so "most recent" is well defined.
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        // Building under `ccc` keeps it plus the newest other (`bbb`).
        retain_recent_target_dirs(&root, "ccc");
        assert!(root.join("ccc").is_dir(), "the current dir is untouched");
        assert!(root.join("bbb").is_dir(), "the previous one is kept");
        assert!(!root.join("aaa").exists(), "older ones are dropped");

        // Flipping back to `bbb` finds its cache still there.
        retain_recent_target_dirs(&root, "bbb");
        assert!(root.join("bbb").is_dir());
        assert!(root.join("ccc").is_dir(), "and can flip again for free");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn build_key_depends_on_wrapper_contents_not_file_identity() {
        let root = std::env::temp_dir().join(format!("jackdaw-build-key-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let plan = root.join("plan.txt");
        let wrapper_a = root.join("wrapper-a");
        let wrapper_b = root.join("wrapper-b");
        std::fs::write(&plan, "edge").unwrap();
        std::fs::write(&wrapper_a, "same wrapper").unwrap();
        std::fs::write(&wrapper_b, "same wrapper").unwrap();

        assert_eq!(
            build_salt(&plan, &wrapper_a).unwrap(),
            build_salt(&plan, &wrapper_b).unwrap()
        );
        std::fs::write(&wrapper_b, "changed wrapper").unwrap();
        assert_ne!(
            build_salt(&plan, &wrapper_a).unwrap(),
            build_salt(&plan, &wrapper_b).unwrap()
        );
    }
}
