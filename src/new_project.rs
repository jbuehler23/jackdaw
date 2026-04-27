//! Scaffolding user projects via Bevy CLI.
//!
//! The editor's **New Project** flow creates a fresh extension or
//! game project by shelling out to `bevy new -t <URL> --yes
//! <NAME>`. Templates live in their own GitHub repos (see
//! [`TEMPLATE_EXTENSION_STATIC_URL`] / [`TEMPLATE_GAME_STATIC_URL`]
//! and the Dylib counterparts) so the jackdaw binary itself carries
//! no template files; users always pull the latest at scaffold time.
//!
//! Call [`scaffold_project`] from a worker thread (it spawns
//! `bevy` and blocks until the subprocess exits). The UI wires
//! this up behind an `AsyncComputeTaskPool` task.

use std::path::{Path, PathBuf};
use std::process::Command;

use bevy::log::{info, warn};

use crate::sdk_paths::SdkPaths;

/// Static extension template. Overridable via
/// `JACKDAW_TEMPLATE_EXTENSION_STATIC_URL`.
pub const TEMPLATE_EXTENSION_STATIC_URL: &str =
    "https://github.com/jbuehler23/jackdaw_template_extension_static";

/// Static game template. Overridable via
/// `JACKDAW_TEMPLATE_GAME_STATIC_URL`.
pub const TEMPLATE_GAME_STATIC_URL: &str =
    "https://github.com/jbuehler23/jackdaw_template_game_static";

/// Dylib extension template. Overridable via
/// `JACKDAW_TEMPLATE_EXTENSION_DYLIB_URL`, falling back to the legacy
/// `JACKDAW_TEMPLATE_EXTENSION_URL`.
pub const TEMPLATE_EXTENSION_DYLIB_URL: &str =
    "https://github.com/jbuehler23/jackdaw_template_extension";

/// Dylib game template. Overridable via
/// `JACKDAW_TEMPLATE_GAME_DYLIB_URL`, falling back to the legacy
/// `JACKDAW_TEMPLATE_GAME_URL`.
pub const TEMPLATE_GAME_DYLIB_URL: &str = "https://github.com/jbuehler23/jackdaw_template_game";

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
    /// Resolve the preset to a concrete URL for the given linkage,
    /// consulting env vars for the built-in presets. `Custom` ignores
    /// `linkage` (the URL is whatever the user pasted).
    pub fn url(&self, linkage: TemplateLinkage) -> String {
        match self {
            Self::Extension => match linkage {
                TemplateLinkage::Static => std::env::var("JACKDAW_TEMPLATE_EXTENSION_STATIC_URL")
                    .unwrap_or_else(|_| TEMPLATE_EXTENSION_STATIC_URL.to_string()),
                TemplateLinkage::Dylib => std::env::var("JACKDAW_TEMPLATE_EXTENSION_DYLIB_URL")
                    .or_else(|_| std::env::var("JACKDAW_TEMPLATE_EXTENSION_URL"))
                    .unwrap_or_else(|_| TEMPLATE_EXTENSION_DYLIB_URL.to_string()),
            },
            Self::Game => match linkage {
                TemplateLinkage::Static => std::env::var("JACKDAW_TEMPLATE_GAME_STATIC_URL")
                    .unwrap_or_else(|_| TEMPLATE_GAME_STATIC_URL.to_string()),
                TemplateLinkage::Dylib => std::env::var("JACKDAW_TEMPLATE_GAME_DYLIB_URL")
                    .or_else(|_| std::env::var("JACKDAW_TEMPLATE_GAME_URL"))
                    .unwrap_or_else(|_| TEMPLATE_GAME_DYLIB_URL.to_string()),
            },
            Self::Custom(url) => url.clone(),
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

/// Run `bevy new -t <template_url> --yes <name>` in `location`.
/// Returns the absolute path to the scaffolded project root.
/// Blocks until `bevy` exits; call from a worker thread.
///
/// For `Dylib` linkage, writes a `.cargo/config.toml` that routes
/// cargo through `jackdaw-rustc-wrapper` so the scaffolded project
/// links against `libjackdaw_sdk`. For `Static` linkage the config
/// is not written; the project depends on `jackdaw` directly.
pub fn scaffold_project(
    name: &str,
    location: &Path,
    template_url: &str,
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

    // Sanity-check that `bevy` is on PATH before invoking it, so
    // the error surfaced to the user distinguishes a missing CLI
    // from an actual scaffold failure.
    let bevy = which_bevy().ok_or(ScaffoldError::BevyCliNotFound)?;

    let output = Command::new(&bevy)
        .current_dir(location)
        .args(["new", "-t", template_url, "--yes", name])
        .output()
        .map_err(ScaffoldError::Spawn)?;

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
