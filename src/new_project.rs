//! Scaffolding user projects via Bevy CLI.
//!
//! The editor's **New Project** flow creates a fresh extension or
//! game project by shelling out to `bevy new -t <URL/SUBDIR> --yes
//! <NAME>`. Templates live in-tree under `templates/<kind>-<linkage>/`
//! within the jackdaw repo; we pass cargo-generate's `URL SUBDIR`
//! syntax so a single repo URL plus a subdir picks the right one.
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

use crate::sdk_paths::SdkPaths;

/// Repo root the four templates live under. Pre-fills the
/// scaffolder's Template field; the user can edit the field to
/// point at a fork or a local path. cargo-generate auto-detects
/// the value as a git URL or directory.
pub const TEMPLATE_REPO_URL: &str = "https://github.com/jbuehler23/jackdaw";

/// Static extension template subdir.
pub const TEMPLATE_EXTENSION_STATIC_SUBDIR: &str = "templates/extension-static";

/// Static game template subdir.
pub const TEMPLATE_GAME_STATIC_SUBDIR: &str = "templates/game-static";

/// Dylib extension template subdir.
pub const TEMPLATE_EXTENSION_DYLIB_SUBDIR: &str = "templates/extension-dylib";

/// Dylib game template subdir.
pub const TEMPLATE_GAME_DYLIB_SUBDIR: &str = "templates/game-dylib";

/// Which template variant the scaffolded project uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TemplateLinkage {
    /// Plain `rlib`/`bin` crate linking `jackdaw` directly.
    #[default]
    Static,
    /// `cdylib` linked against `libjackdaw_sdk` for hot-reload.
    /// Requires the editor built with `--features dylib`.
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
    pub fn git_url(&self) -> &str {
        match self {
            Self::Extension | Self::Game => TEMPLATE_REPO_URL,
            Self::Custom(url) => {
                // Custom URLs may already carry a subdir (`URL
                // SUBDIR`); split and return the leading word.
                url.split_whitespace().next().unwrap_or(url)
            }
        }
    }

    /// Just the subdir portion for the given linkage, or `None` if
    /// the preset doesn't have one (custom URL with no subdir).
    pub fn subdir(&self, linkage: TemplateLinkage) -> Option<&str> {
        match self {
            Self::Extension => Some(match linkage {
                TemplateLinkage::Static => TEMPLATE_EXTENSION_STATIC_SUBDIR,
                TemplateLinkage::Dylib => TEMPLATE_EXTENSION_DYLIB_SUBDIR,
            }),
            Self::Game => Some(match linkage {
                TemplateLinkage::Static => TEMPLATE_GAME_STATIC_SUBDIR,
                TemplateLinkage::Dylib => TEMPLATE_GAME_DYLIB_SUBDIR,
            }),
            Self::Custom(url) => {
                let mut parts = url.split_whitespace();
                let _ = parts.next();
                parts.next()
            }
        }
    }

    /// `true` for the two presets that have Static/Dylib variants
    /// (so the UI knows whether to show the linkage selector).
    pub fn supports_linkage_selector(&self) -> bool {
        matches!(self, Self::Extension | Self::Game)
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
/// cargo through `jackdaw-rustc-wrapper` so the scaffolded project
/// links against `libjackdaw_sdk`. For `Static` linkage the template
/// ships its own `.cargo/config.toml` (with the `cargo editor` alias)
/// and we leave it alone.
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
        .args(["new", "-t", &template_arg, "--yes"]);
    // Pin the template to a branch / tag when the caller supplied
    // one (the UI's Branch field, the operator's `branch` param).
    if let Some(branch) = branch {
        cmd.args(["-b", branch]);
    }
    cmd.arg(name);
    if let Some(subdir) = subdir {
        cmd.arg("--").arg(subdir);
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
    if matches!(linkage, TemplateLinkage::Dylib) {
        write_cargo_config(&project_path);
    }
    Ok(project_path)
}

/// Write a `.cargo/config.toml` into the scaffolded project with
/// absolute paths pointing at jackdaw's rustc wrapper and SDK so
/// that **any** cargo invocation (terminal, rust-analyzer, `VSCode`
/// build task, etc.) picks up the same linkage jackdaw's Build
/// button uses.
///
/// Best-effort: if the SDK or wrapper isn't on disk where
/// [`SdkPaths::compute`] expects it, we skip the write and log a
/// warning. The user can still build through jackdaw's UI, which
/// injects env vars directly regardless of on-disk discovery.
///
/// We never clobber an existing `.cargo/config.toml`; if the user
/// has customised theirs, we log a hint and leave it alone. The
/// template shouldn't ship one, so in practice we always write.
fn write_cargo_config(project_path: &Path) {
    let paths = SdkPaths::compute();
    if !paths.dylib_exists() || !paths.wrapper_exists() {
        warn!(
            "Skipping .cargo/config.toml write: SDK dylib or wrapper \
             not found at {}. Scaffolded project will only build through \
             jackdaw's Build button until you install jackdaw or set \
             JACKDAW_SDK_DIR.",
            paths
                .dylib
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        );
        return;
    }

    let cargo_dir = project_path.join(".cargo");
    let config_path = cargo_dir.join("config.toml");
    if config_path.exists() {
        warn!(
            "{} already exists; leaving it alone. Merge the following keys \
             manually if you want external-IDE builds to use jackdaw's SDK: \
             build.rustc-wrapper, env.JACKDAW_SDK_DYLIB, env.JACKDAW_SDK_DEPS.",
            config_path.display()
        );
        return;
    }

    if let Err(e) = std::fs::create_dir_all(&cargo_dir) {
        warn!("Failed to create {}: {e}", cargo_dir.display());
        return;
    }

    let body = render_cargo_config(&paths);
    if let Err(e) = std::fs::write(&config_path, body) {
        warn!("Failed to write {}: {e}", config_path.display());
        return;
    }

    info!("Wrote {}", config_path.display());
}

fn render_cargo_config(paths: &SdkPaths) -> String {
    // TOML strings need to be on a single line; backslashes on
    // Windows escape, so we use the raw-string `'…'` form. Paths
    // from SdkPaths are always absolute.
    format!(
        "# Activates jackdaw-rustc-wrapper so that any cargo\n\
         # invocation in this project directory; terminal builds,\n\
         # rust-analyzer, VSCode tasks; links the resulting cdylib\n\
         # against the same bevy compilation the jackdaw editor\n\
         # ships with, keeping TypeIds in sync.\n\
         #\n\
         # Regenerate via jackdaw's scaffolder if the SDK moves.\n\
         \n\
         [build]\n\
         rustc-wrapper = '{wrapper}'\n\
         \n\
         [env]\n\
         JACKDAW_SDK_DYLIB = '{dylib}'\n\
         JACKDAW_SDK_DEPS = '{deps}'\n",
        wrapper = paths.wrapper.display(),
        dylib = paths.dylib.display(),
        deps = paths.deps.display(),
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
/// `~/Workspace/jackdaw/templates/game-static`). cargo-generate
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
    let cargo_generate =
        which_cargo_generate().ok_or(ScaffoldError::CargoGenerateNotFound)?;

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

    if matches!(linkage, TemplateLinkage::Dylib) {
        write_cargo_config(project_path);
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
