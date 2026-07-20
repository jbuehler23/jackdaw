//! Editor-driven project builds: from an open project to a loadable
//! project dylib.
//!
//! The pipeline, per build:
//!
//! 1. [`shim::ensure_shim`]: the generated shim crate in
//!    `.jackdaw/shim/` is the build root; the user's crate is its path
//!    dependency.
//! 2. The SDK's `Cargo.lock` is copied in so the shared dependency
//!    closure resolves at the SDK's exact versions (the user's own
//!    lock is never touched).
//! 3. [`plan::write_plan`]: the per-edge extern redirect plan for the
//!    rustc wrapper.
//! 4. `cargo rustc --crate-type dylib --target <triple>` through the
//!    wrapper, into a target dir salted with the plan (cargo does not
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
}

impl std::fmt::Display for ProjectBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Plan(e) => write!(f, "plan generation failed: {e}"),
            Self::Linkage(e) => write!(f, "linkage verification failed: {e}"),
            Self::Compile { .. } => write!(f, "project failed to compile"),
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

    // Seed the shim with the SDK's lockfile so the shared dependency
    // closure resolves to the exact versions the SDK was built with.
    // A freshly created project otherwise drifts to newer patch
    // releases (a bumped transitive dep pulling a newer libc), and a
    // redirected rlib built against the SDK's version then clashes with
    // the project's. The user's own extra dependencies resolve on top,
    // as new lock entries, untouched.
    if sdk.lockfile.is_file() {
        std::fs::copy(&sdk.lockfile, shim_dir.join("Cargo.lock"))?;
    }

    let manifest = match SdkManifest::load(&sdk.manifest) {
        Ok(manifest) if !manifest.is_empty() => manifest,
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

    let plan_path = jackdaw_dir.join("plan.txt");
    let edges = plan::write_plan(&shim_dir, &manifest, &sdk.deps, &plan_path)?;

    // Salt the target dir with everything cargo cannot fingerprint:
    // the plan content and the wrapper binary. A stale salt dir is
    // removed so they cannot accumulate.
    let salt = build_salt(&plan_path, &sdk.wrapper)?;
    let target_root = jackdaw_dir.join("target");
    let target_dir = target_root.join(&salt);
    if let Ok(entries) = std::fs::read_dir(&target_root) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy() != salt {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }

    // The static SDK links the project dylib against the prebuilt bevy and
    // jackdaw_api rlibs (one shared bevy compilation, matching the editor's
    // TypeIds via the rmeta trick) rather than a dll. Resolve them from the
    // manifest; the shared-dylib path stays as a fallback.
    let resolve_rlib = |name: &str| -> Option<PathBuf> {
        manifest.artifact_for(name).map(|artifact| {
            let p = Path::new(artifact);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                sdk.deps.join(artifact)
            }
        })
    };
    let static_rlibs = resolve_rlib("bevy").zip(resolve_rlib("jackdaw_api"));
    let is_static = static_rlibs.is_some();

    let mut cmd = Command::new("cargo");
    cmd.args(["rustc", "--crate-type", "dylib", "--target", &sdk.triple])
        // Machine-readable stream so the editor can show live per-crate
        // progress; rustc diagnostics arrive rendered in the same stream.
        .arg("--message-format=json-render-diagnostics")
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
        .env("JACKDAW_SDK_DEPS", &sdk.deps)
        .env("JACKDAW_SDK_HOST_DEPS", &sdk.host_deps)
        .env("JACKDAW_SDK_EXTERN_MAP", &plan_path);
    match static_rlibs {
        Some((bevy_rlib, api_rlib)) => {
            cmd.env("JACKDAW_SDK_STATIC", "1")
                .env("JACKDAW_SDK_BEVY_RLIB", bevy_rlib)
                .env("JACKDAW_SDK_API_RLIB", api_rlib);
        }
        None => {
            cmd.env("JACKDAW_SDK_DYLIB", &sdk.dylib);
        }
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
        return Err(ProjectBuildError::Compile { log });
    }

    let dylib = target_dir
        .join(&sdk.triple)
        .join("debug")
        .join(dylib_file_name("jackdaw_shim"));
    // In the shared-dylib model, prove the artifact links the running SDK
    // before anything dlopens it. In the static model the dylib embeds bevy
    // and depends on no SDK dll, so there is nothing to verify against.
    if !is_static {
        linkage::verify_linkage(&dylib, &sdk.dylib).map_err(ProjectBuildError::Linkage)?;
    }

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
/// editor's run config named, when there is one; the CLI passes `None`
/// and falls back to source detection and the `GamePlugin` convention.
/// Shared by the editor's in-process build and `jackdaw build`.
pub fn shim_spec_for_project(root: &Path, configured_plugin: Option<String>) -> Option<ShimSpec> {
    let meta = crate::cargo_meta::CargoMeta::load(root)?;
    let (package_name, crate_name) = meta.lib_package()?;
    let extension_type = crate::detect::detect_extension(root).map(|(_, name)| name);
    let detected_plugin = crate::detect::detect_plugin(root, &crate_name)
        .and_then(|p| p.split_once("::").map(|(_, name)| name.to_string()));
    let game_plugin = configured_plugin
        .or(detected_plugin)
        .or_else(|| extension_type.is_none().then(|| "GamePlugin".to_string()));
    Some(ShimSpec {
        package_name,
        crate_name,
        project_root: root.to_path_buf(),
        game_plugin,
        extension_type,
    })
}

fn build_salt(plan_path: &Path, wrapper: &Path) -> std::io::Result<String> {
    let mut hasher = DefaultHasher::new();
    std::fs::read(plan_path)?.hash(&mut hasher);
    if let Ok(meta) = std::fs::metadata(wrapper) {
        if let Ok(modified) = meta.modified() {
            modified.hash(&mut hasher);
        }
        meta.len().hash(&mut hasher);
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
