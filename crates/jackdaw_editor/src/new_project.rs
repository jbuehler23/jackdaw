//! Scaffolding user projects via Bevy CLI.
//!
//! The editor's **New Project** flow creates a fresh extension or
//! game project by shelling out to `bevy new -t <URL/SUBDIR> --yes
//! <NAME>`. Templates live in-tree under `templates/` within the
//! jackdaw repo (extensions split by linkage; the game template is
//! unified); we pass cargo-generate's `URL SUBDIR` syntax so a single
//! repo URL plus a subdir picks the right one.
//! Pinning to the running editor's [`CARGO_PKG_VERSION`] keeps users
//! on stable jackdaw from picking up incompatible main-branch
//! template changes.
//!
//! Call [`scaffold_project`] from a worker thread (it spawns
//! `bevy` and blocks until the subprocess exits). The UI wires
//! this up behind an `AsyncComputeTaskPool` task.

use std::path::{Path, PathBuf};
use std::process::Command;

use bevy::log::{info, warn};

/// Repo root the four templates live under. Pre-fills the
/// scaffolder's Template field; the user can edit the field to
/// point at a fork or a local path. cargo-generate auto-detects
/// the value as a git URL or directory.
pub const TEMPLATE_REPO_URL: &str = "https://github.com/jbuehler23/jackdaw";

/// Static extension template subdir.
pub const TEMPLATE_EXTENSION_STATIC_SUBDIR: &str = "templates/extension-static";

/// Dylib extension template subdir.
pub const TEMPLATE_EXTENSION_DYLIB_SUBDIR: &str = "templates/extension-dylib";

/// Unified game template subdir. Produces both a cdylib (for in-editor
/// PIE via dlopen) and a bin (for standalone runs); the static/dylib
/// distinction was collapsed in Phase 3 of the PIE render bridge.
pub const TEMPLATE_GAME_SUBDIR: &str = "templates/game";

/// Which template variant the scaffolded project uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TemplateLinkage {
    /// Plain `rlib`/`bin` crate linking `jackdaw` directly.
    #[default]
    Static,
    /// `cdylib` for hot-reload. The editor and the user's cdylib
    /// share one compile graph through the per-project shared
    /// `target-dir` written by [`write_cargo_config`].
    Dylib,
}

/// Which template preset the user opened the scaffolder with.
/// `Custom` bypasses the preset→URL mapping and lets the user
/// paste any Bevy-CLI-compatible URL.
#[derive(Clone, Debug)]
pub enum TemplatePreset {
    Extension,
    Game,
    Custom(String),
}

impl TemplatePreset {
    /// Resolve the preset to a concrete `bevy new -t` argument for
    /// the given linkage. Built-in presets compose `<repo> <subdir>`
    /// pointing at the in-tree templates; `Custom` is whatever the
    /// user pasted (ignores `linkage`).
    pub fn url(&self, linkage: TemplateLinkage) -> String {
        match (self.git_url(), self.subdir(linkage)) {
            (url, Some(subdir)) => format!("{url} {subdir}"),
            (url, None) => url.to_string(),
        }
    }

    /// Just the git URL portion (no subdir). Used to pre-fill the
    /// Template field in the New Project modal.
    ///
    /// When running from a jackdaw source checkout, returns the local
    /// checkout path instead of the GitHub URL so contributors iterate
    /// against their working-tree templates without push/clone cycles.
    /// `scaffold_project` detects the local-path case and routes to
    /// `cargo-generate --path` accordingly.
    pub fn git_url(&self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::Extension | Self::Game => {
                if let Some(checkout) = jackdaw_dev_checkout() {
                    std::borrow::Cow::Owned(checkout.display().to_string())
                } else {
                    std::borrow::Cow::Borrowed(TEMPLATE_REPO_URL)
                }
            }
            Self::Custom(url) => std::borrow::Cow::Owned(
                url.split_whitespace().next().unwrap_or(url).to_string(),
            ),
        }
    }

    /// Just the subdir portion for the given linkage, or `None` if
    /// the preset doesn't have one (custom URL with no subdir).
    ///
    /// For `Game`, `linkage` is ignored: the unified game template
    /// produces both a cdylib (for editor PIE) and a bin (for
    /// standalone runs), so a single subdir covers both modes.
    pub fn subdir(&self, linkage: TemplateLinkage) -> Option<&str> {
        match self {
            Self::Extension => Some(match linkage {
                TemplateLinkage::Static => TEMPLATE_EXTENSION_STATIC_SUBDIR,
                TemplateLinkage::Dylib => TEMPLATE_EXTENSION_DYLIB_SUBDIR,
            }),
            Self::Game => Some(TEMPLATE_GAME_SUBDIR),
            Self::Custom(url) => {
                let mut parts = url.split_whitespace();
                let _ = parts.next();
                parts.next()
            }
        }
    }

    /// `true` for the extension preset, which has Static/Dylib
    /// variants (so the UI knows whether to show the linkage
    /// selector). The game preset uses a unified template; the UI
    /// shows a different control (default play mode) instead.
    pub fn supports_linkage_selector(&self) -> bool {
        matches!(self, Self::Extension)
    }
}

#[derive(Debug)]
pub enum ScaffoldError {
    BevyCliNotFound,
    CargoGenerateNotFound,
    InvalidName(String),
    LocationNotFound(PathBuf),
    ProjectAlreadyExists(PathBuf),
    BevyCliFailed {
        status: std::process::ExitStatus,
        stdout: String,
        stderr: String,
    },
    Spawn(std::io::Error),
}

impl std::fmt::Display for ScaffoldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BevyCliNotFound => write!(
                f,
                "`bevy` CLI not found on PATH. Install with \
                 `cargo install --locked --git https://github.com/TheBevyFlock/bevy_cli bevy_cli`."
            ),
            Self::CargoGenerateNotFound => write!(
                f,
                "`cargo-generate` not found on PATH (needed for local-path \
                 templates). Install with `cargo install cargo-generate`."
            ),
            Self::InvalidName(name) => write!(
                f,
                "`{name}` is not a valid project name. Use lowercase letters, \
                 digits, hyphens, and underscores only."
            ),
            Self::LocationNotFound(p) => write!(f, "location does not exist: {}", p.display()),
            Self::ProjectAlreadyExists(p) => write!(
                f,
                "a project already exists at {}; pick a different name or location.",
                p.display()
            ),
            Self::BevyCliFailed { status, stderr, .. } => {
                write!(f, "bevy CLI exited with {status}\n{stderr}")
            }
            Self::Spawn(e) => write!(f, "failed to spawn `bevy`: {e}"),
        }
    }
}

impl std::error::Error for ScaffoldError {}

/// Run `bevy new` against `template_url` in `location`. Returns the
/// absolute path to the scaffolded project root. Blocks until `bevy`
/// exits; call from a worker thread.
///
/// `template_url` is either a single value (git URL or local
/// directory; cargo-generate auto-detects which) or
/// `"<value> <subdir>"` for the in-tree built-in templates produced
/// by [`TemplatePreset::url`]. When a subdir is present we split
/// it back out and pass it through to cargo-generate via bevy's
/// `--` passthrough (`bevy new ... -- <subdir>`).
///
/// `branch` is an optional git branch / tag (the UI's Branch field
/// or the `project.new` operator's `branch` param). Useful for
/// scaffolding from a feature branch or a pinned release tag.
/// Ignored when `template_url` resolves to a local path —
/// cargo-generate detects local paths automatically and skips the
/// branch.
///
/// For `Dylib` linkage, writes a `.cargo/config.toml` that routes
/// the scaffolded project's `target-dir` at the shared jackdaw cache
/// so the editor binary and the user's cdylib compile against one
/// bevy build. For `Static` linkage the template ships its own
/// `.cargo/config.toml` (with the `cargo editor` alias) and we leave
/// it alone.
pub fn scaffold_project(
    name: &str,
    location: &Path,
    template_url: &str,
    branch: Option<&str>,
    linkage: TemplateLinkage,
) -> Result<PathBuf, ScaffoldError> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(ScaffoldError::InvalidName(name.to_string()));
    }

    if !location.is_dir() {
        return Err(ScaffoldError::LocationNotFound(location.to_path_buf()));
    }

    let project_path = location.join(name);
    if project_path.exists() {
        return Err(ScaffoldError::ProjectAlreadyExists(project_path));
    }

    // Split `URL [SUBDIR]`. The URL alone goes to bevy CLI's
    // `-t/--template` flag; if a subdir is present it rides through
    // bevy's `-- <args>` passthrough as cargo-generate's second
    // positional.
    let mut parts = template_url.split_whitespace();
    let template_arg = parts.next().unwrap_or("").to_string();
    let subdir = parts.next();

    // Path detection: bevy CLI's `-t` always treats its value as a
    // git URL (it doesn't auto-detect local paths the way
    // cargo-generate does). When the user passes a directory that
    // exists on disk, shell out to `cargo-generate` directly with
    // `--path` so it reads from the filesystem instead of trying to
    // clone. Lets contributors iterate on in-tree templates without
    // pushing each change to GitHub.
    if Path::new(&template_arg).is_dir() {
        return scaffold_from_local_path(
            name,
            location,
            &template_arg,
            subdir,
            linkage,
            &project_path,
        );
    }

    // Sanity-check that `bevy` is on PATH before invoking it, so the
    // error surfaced to the user distinguishes a missing CLI from an
    // actual scaffold failure.
    let bevy = which_bevy().ok_or(ScaffoldError::BevyCliNotFound)?;

    let mut cmd = Command::new(&bevy);
    cmd.current_dir(location)
        .args(["new", "-t", &template_arg, "--yes", name]);
    // bevy CLI's `new` subcommand exposes only `-t`, `--yes`, and
    // `<NAME>`. Anything else (branch pin, subfolder) rides through
    // its `-- <ARGS>` passthrough to cargo-generate, which accepts
    // `--branch BRANCH` and a positional subfolder.
    let needs_passthrough = branch.is_some() || subdir.is_some();
    if needs_passthrough {
        cmd.arg("--");
        if let Some(branch) = branch {
            cmd.args(["--branch", branch]);
        }
        if let Some(subdir) = subdir {
            cmd.arg(subdir);
        }
    }

    let output = cmd.output().map_err(ScaffoldError::Spawn)?;

    if !output.status.success() {
        return Err(ScaffoldError::BevyCliFailed {
            status: output.status,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    // `bevy new` is consistent about where it drops the project:
    // `<location>/<name>/`. Trust that and return.
    //
    // Write the shared `target-dir` `.cargo/config.toml` for both
    // game and dylib-extension projects; the new architecture's
    // unified game template always produces a cdylib (so the editor
    // can dlopen it) and routes builds through the shared cache
    // regardless of `linkage`.
    write_cargo_config(&project_path);

    // Local-dev convenience: when the editor is running from its
    // own source checkout, append a `[patch.crates-io]` section to
    // the scaffolded project's `Cargo.toml` so its `jackdaw = "0.4"`
    // dep resolves to our working tree instead of crates.io. The
    // template stays vanilla; only this post-scaffold edit injects
    // the patch. Released editor binaries skip this step (the
    // `JACKDAW_DEV_CHECKOUT` env, or the `CARGO_MANIFEST_DIR` build-
    // time embedded path, isn't available / valid).
    if let Some(checkout) = jackdaw_dev_checkout()
        && let Err(err) = append_patch_section(&project_path, &checkout)
    {
        warn!(
            "Failed to write [patch.crates-io] block into {}: {err}",
            project_path.display()
        );
    }

    Ok(project_path)
}

/// Resolve the path to the jackdaw source checkout the running
/// editor was built from, if any. Returns `Some(path)` when the
/// path exists on disk (i.e., the editor is being run from a dev
/// build, not an installed binary). `CARGO_MANIFEST_DIR` is set at
/// compile time; `JACKDAW_DEV_CHECKOUT` is a runtime override for
/// CI and other unusual setups.
pub(crate) fn jackdaw_dev_checkout() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("JACKDAW_DEV_CHECKOUT") {
        let path = PathBuf::from(p);
        if path.is_dir() {
            return Some(path);
        }
    }
    // After Phase 1's workspace restructure, this code lives in
    // `crates/jackdaw_editor`, so `CARGO_MANIFEST_DIR` resolves to the
    // crate's dir, not the workspace root. Walk up to find a parent
    // that contains the `crates/` directory (i.e., the workspace root).
    let compile_time = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut candidate = compile_time.as_path();
    loop {
        if candidate.join("crates").is_dir() && candidate.join("Cargo.toml").is_file() {
            return Some(candidate.to_path_buf());
        }
        candidate = candidate.parent()?;
    }
}

/// Append a `[patch.crates-io]` section to the scaffolded project's
/// `Cargo.toml` overriding the `jackdaw*` deps with `path` deps
/// pointing at the dev checkout. Idempotent: skips when the section
/// is already present (prevents double-write on repeated scaffolds
/// against the same project path, which shouldn't happen but is
/// cheap to guard).
fn append_patch_section(project_path: &Path, checkout: &Path) -> std::io::Result<()> {
    let manifest = project_path.join("Cargo.toml");
    let existing = std::fs::read_to_string(&manifest)?;
    if existing.contains("[patch.crates-io]") {
        return Ok(());
    }
    let checkout_str = checkout.display();
    let block = format!(
        "\n# Auto-injected by jackdaw editor running from a source\n\
         # checkout. Overrides crates.io deps with the local working\n\
         # tree so the scaffolded project tracks unpublished changes.\n\
         # Safe to delete once jackdaw is published with the matching\n\
         # version.\n\
         [patch.crates-io]\n\
         jackdaw_editor = {{ path = \"{checkout_str}/crates/jackdaw_editor\" }}\n\
         jackdaw_api = {{ path = \"{checkout_str}/crates/jackdaw_api\" }}\n\
         jackdaw_api_internal = {{ path = \"{checkout_str}/crates/jackdaw_api_internal\" }}\n\
         jackdaw_runtime = {{ path = \"{checkout_str}/crates/jackdaw_runtime\" }}\n",
    );
    let mut updated = existing;
    updated.push_str(&block);
    std::fs::write(&manifest, updated)?;
    info!(
        "Injected [patch.crates-io] into {} pointing at {}",
        manifest.display(),
        checkout_str
    );
    Ok(())
}

/// Write a `.cargo/config.toml` into the scaffolded project that
/// routes its `target-dir` at the shared jackdaw cache so the
/// editor and the user's cdylib compile against one bevy build.
///
/// The target dir comes from [`crate::cache_manager::target_dir`],
/// which resolves to `~/.cache/jackdaw/<version>/target/` on the
/// current platform.
///
/// Always overwrites: the template ships a placeholder comment
/// header; the scaffolder replaces it with the real `[build]`
/// section. A returning user who has customised their config is
/// not in scope here (this only runs at create time).
fn write_cargo_config(project_path: &Path) {
    let target_dir = crate::cache_manager::target_dir();
    let cargo_dir = project_path.join(".cargo");
    let config_path = cargo_dir.join("config.toml");
    let body = render_cargo_config(&target_dir);

    if let Err(e) = std::fs::create_dir_all(&cargo_dir) {
        warn!("Failed to create {}: {e}", cargo_dir.display());
        return;
    }
    if let Err(e) = std::fs::write(&config_path, &body) {
        warn!("Failed to write {}: {e}", config_path.display());
        return;
    }
    info!("Wrote {}", config_path.display());
}

fn render_cargo_config(target_dir: &std::path::Path) -> String {
    format!(
        "# Auto-generated by jackdaw scaffolder.\n\
         # Routes builds through the shared jackdaw cache so bevy and\n\
         # jackdaw_editor are compiled once globally per jackdaw version.\n\
         \n\
         [build]\n\
         target-dir = '{}'\n",
        target_dir.display(),
    )
}

/// Local-path scaffold: bypass `bevy new` entirely and call
/// `cargo-generate` directly with `--path`. bevy CLI's `-t` flag
/// always treats its value as a git URL (it doesn't expose
/// `--path`), so contributors iterating on in-tree templates need
/// this branch to skip the clone-to-tmp step that would otherwise
/// 404 against unpublished local paths.
///
/// `local_root[/subdir]` is the template's source directory (e.g.
/// `~/Workspace/jackdaw/templates/game`). cargo-generate
/// is required for bevy CLI to work anyway, so the binary is
/// generally already on PATH.
fn scaffold_from_local_path(
    name: &str,
    location: &Path,
    local_root: &str,
    subdir: Option<&str>,
    linkage: TemplateLinkage,
    project_path: &Path,
) -> Result<PathBuf, ScaffoldError> {
    let cargo_generate = which_cargo_generate().ok_or(ScaffoldError::CargoGenerateNotFound)?;

    let template_path = match subdir {
        Some(s) => Path::new(local_root).join(s),
        None => Path::new(local_root).to_path_buf(),
    };
    if !template_path.is_dir() {
        return Err(ScaffoldError::LocationNotFound(template_path));
    }

    let mut cmd = Command::new(&cargo_generate);
    cmd.current_dir(location)
        .arg("generate")
        .arg("--path")
        .arg(&template_path)
        .args(["--name", name])
        .arg("--destination")
        .arg(location)
        .arg("--silent");

    let output = cmd.output().map_err(ScaffoldError::Spawn)?;

    if !output.status.success() {
        return Err(ScaffoldError::BevyCliFailed {
            status: output.status,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    // Unified game template always produces a cdylib + bin and routes
    // builds through the shared cache regardless of `linkage`.
    let _ = linkage;
    write_cargo_config(project_path);

    // Mirror the bevy-CLI branch: when scaffolding from a source
    // checkout the project's `jackdaw_*` deps need to resolve to the
    // local working tree, not crates.io (those crates aren't published
    // yet at this jackdaw version).
    if let Some(checkout) = jackdaw_dev_checkout()
        && let Err(err) = append_patch_section(project_path, &checkout)
    {
        warn!(
            "Failed to write [patch.crates-io] block into {}: {err}",
            project_path.display()
        );
    }

    Ok(project_path.to_path_buf())
}

/// Resolve `cargo-generate` on PATH. Used by the local-path scaffold
/// branch which shells out to `cargo-generate` directly because
/// bevy CLI's `-t` flag doesn't expose `--path`.
fn which_cargo_generate() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(if cfg!(target_os = "windows") {
            "cargo-generate.exe"
        } else {
            "cargo-generate"
        });
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Resolve `bevy` on PATH. Returns the absolute path if found, so
/// the caller can invoke it without relying on shell resolution
/// (useful in GUI sessions with minimal env).
pub fn which_bevy() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(if cfg!(target_os = "windows") {
            "bevy.exe"
        } else {
            "bevy"
        });
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
