//! Scaffolding user projects via Bevy CLI.
//!
//! The editor's **New Project** flow creates a fresh extension or
//! game project by shelling out to `bevy new -t <URL> --yes
//! <NAME>`. Templates live under `templates/` in the jackdaw repo
//! (sub-dirs: `game-static`, `game`, `extension-static`,
//! `extension`); released binaries pull them from GitHub at
//! scaffold time, and dev builds running from a source checkout
//! point at the working-tree copies via `cargo-generate --path`.
//!
//! Call [`scaffold_project`] from a worker thread (it spawns
//! `bevy` or `cargo-generate` and blocks until the subprocess
//! exits). The UI wires this up behind an `AsyncComputeTaskPool`
//! task.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::process::Command;

use bevy::log::{info, warn};

use crate::sdk_paths::SdkPaths;

/// Single-repo URL for all built-in templates. Each variant lives
/// in its own sub-directory; the scaffolder composes
/// `<URL> <SUBDIR>` and passes the subdir through bevy CLI's
/// passthrough to cargo-generate. Overridable via
/// `JACKDAW_TEMPLATE_REPO_URL` for forks.
pub const TEMPLATE_REPO_URL: &str = "https://github.com/jbuehler23/jackdaw";

/// Static extension template subdir.
pub const TEMPLATE_EXTENSION_STATIC_SUBDIR: &str = "templates/extension-static";

/// Dylib extension template subdir.
pub const TEMPLATE_EXTENSION_DYLIB_SUBDIR: &str = "templates/extension";

/// Static game template subdir.
pub const TEMPLATE_GAME_STATIC_SUBDIR: &str = "templates/game-static";

/// Dylib game template subdir.
pub const TEMPLATE_GAME_DYLIB_SUBDIR: &str = "templates/game";

/// Default branch / tag to scaffold from when running a released
/// jackdaw binary. Pinned to the jackdaw version at compile time so
/// users on `0.4.x` don't accidentally pick up incompatible
/// `main`-branch templates. Overridable via
/// `JACKDAW_TEMPLATE_BRANCH`. When the editor runs from a local
/// source checkout the branch is ignored (the working tree IS the
/// branch).
pub const TEMPLATE_DEFAULT_BRANCH: &str = "main";

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
    /// Resolve the preset to a `<URL> [SUBDIR]` string suitable for
    /// the New Project modal's Template field. The scaffolder splits
    /// it back apart.
    ///
    /// When the editor runs from a jackdaw source checkout
    /// (detected via [`jackdaw_dev_checkout`]), `URL` is the local
    /// checkout path so the scaffolder routes through
    /// `cargo-generate --path`; this lets contributors iterate on
    /// in-tree templates without push/clone cycles. Otherwise `URL`
    /// is the GitHub repo (overridable via the
    /// `JACKDAW_TEMPLATE_REPO_URL` / per-template env vars).
    pub fn url(&self, linkage: TemplateLinkage) -> String {
        match (self.git_url(linkage), self.subdir(linkage)) {
            (url, Some(subdir)) => format!("{url} {subdir}"),
            (url, None) => url.into_owned(),
        }
    }

    /// Composed `<URL> [SUBDIR]` form using ONLY the git URL (never
    /// the local source-checkout path). Used for the modal's
    /// Git URL field, which should always show a real remote even
    /// in dev mode; the dev-mode local shortcut surfaces through
    /// `local_template_path` and the modal's separate Local path
    /// field.
    pub fn git_url_with_subdir(&self, linkage: TemplateLinkage) -> String {
        match (self.git_url_only(linkage), self.subdir(linkage)) {
            (url, Some(subdir)) => format!("{url} {subdir}"),
            (url, None) => url.into_owned(),
        }
    }

    /// Just the git URL, ignoring source-checkout detection. Used
    /// for the modal's Git URL field; the dev-mode shortcut goes
    /// through `local_template_path` instead.
    pub fn git_url_only(&self, linkage: TemplateLinkage) -> Cow<'static, str> {
        let preset_override = match self {
            Self::Extension => match linkage {
                TemplateLinkage::Static => {
                    std::env::var("JACKDAW_TEMPLATE_EXTENSION_STATIC_URL").ok()
                }
                TemplateLinkage::Dylib => std::env::var("JACKDAW_TEMPLATE_EXTENSION_DYLIB_URL")
                    .or_else(|_| std::env::var("JACKDAW_TEMPLATE_EXTENSION_URL"))
                    .ok(),
            },
            Self::Game => match linkage {
                TemplateLinkage::Static => std::env::var("JACKDAW_TEMPLATE_GAME_STATIC_URL").ok(),
                TemplateLinkage::Dylib => std::env::var("JACKDAW_TEMPLATE_GAME_DYLIB_URL")
                    .or_else(|_| std::env::var("JACKDAW_TEMPLATE_GAME_URL"))
                    .ok(),
            },
            Self::Custom(url) => Some(url.split_whitespace().next().unwrap_or(url).to_string()),
        };
        if let Some(url) = preset_override {
            return Cow::Owned(url);
        }
        if let Ok(repo) = std::env::var("JACKDAW_TEMPLATE_REPO_URL") {
            return Cow::Owned(repo);
        }
        Cow::Borrowed(TEMPLATE_REPO_URL)
    }

    /// Resolved local template directory when running from a jackdaw
    /// source checkout. Pre-fills the modal's Local path field so
    /// dev users can scaffold from the in-tree templates without
    /// typing the path. Returns `None` for released binaries (no
    /// checkout) or `Custom` presets (no built-in path).
    pub fn local_template_path(&self, linkage: TemplateLinkage) -> Option<PathBuf> {
        if !matches!(self, Self::Extension | Self::Game) {
            return None;
        }
        let checkout = jackdaw_dev_checkout()?;
        let subdir = self.subdir(linkage)?;
        Some(checkout.join(subdir))
    }

    /// Just the URL portion (no subdir). Honours env var overrides
    /// and source-checkout detection.
    pub fn git_url(&self, linkage: TemplateLinkage) -> Cow<'static, str> {
        // Source-checkout dev path. Local templates always win over
        // remote URLs, so contributor edits to `templates/*` show
        // up immediately on the next scaffold.
        if matches!(self, Self::Extension | Self::Game)
            && let Some(checkout) = jackdaw_dev_checkout()
        {
            return Cow::Owned(checkout.display().to_string());
        }

        // Per-preset override env vars (legacy compat) take
        // precedence over the global repo override.
        let preset_override = match self {
            Self::Extension => match linkage {
                TemplateLinkage::Static => {
                    std::env::var("JACKDAW_TEMPLATE_EXTENSION_STATIC_URL").ok()
                }
                TemplateLinkage::Dylib => std::env::var("JACKDAW_TEMPLATE_EXTENSION_DYLIB_URL")
                    .or_else(|_| std::env::var("JACKDAW_TEMPLATE_EXTENSION_URL"))
                    .ok(),
            },
            Self::Game => match linkage {
                TemplateLinkage::Static => std::env::var("JACKDAW_TEMPLATE_GAME_STATIC_URL").ok(),
                TemplateLinkage::Dylib => std::env::var("JACKDAW_TEMPLATE_GAME_DYLIB_URL")
                    .or_else(|_| std::env::var("JACKDAW_TEMPLATE_GAME_URL"))
                    .ok(),
            },
            Self::Custom(url) => Some(url.split_whitespace().next().unwrap_or(url).to_string()),
        };
        if let Some(url) = preset_override {
            return Cow::Owned(url);
        }

        if let Ok(repo) = std::env::var("JACKDAW_TEMPLATE_REPO_URL") {
            return Cow::Owned(repo);
        }
        Cow::Borrowed(TEMPLATE_REPO_URL)
    }

    /// Just the subdir portion. `None` for `Custom` URLs that don't
    /// embed one.
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
                parts.next().map(|s| {
                    // Leak the heap allocation so we can return a
                    // `&'static str` keyed off this `Custom` value.
                    // Custom presets are constructed once per modal
                    // open, so the leak is bounded.
                    Box::leak(s.to_string().into_boxed_str()) as &'static str
                })
            }
        }
    }

    /// `true` for the two presets that have Static/Dylib variants
    /// (so the UI knows whether to show the linkage selector).
    pub fn supports_linkage_selector(&self) -> bool {
        matches!(self, Self::Extension | Self::Game)
    }
}

/// What kind of jackdaw project lives at this path, as far as the
/// launcher's editor-handoff logic cares. The launcher uses this
/// to decide whether to dlopen a cdylib (Dylib), background-build
/// a static editor binary (Static), or just open the scene in the
/// launcher's own editor (Other / unrecognised).
pub enum TemplateKind {
    /// Has the `editor` cargo feature gating an optional jackdaw
    /// dep, plus a `[[bin]] name = "editor"` with
    /// `required-features = ["editor"]`. The launcher will
    /// background-build the editor binary and offer a handoff.
    StaticGameWithEditorFeature,
    /// Has `[lib] crate-type = ["cdylib"]`. Existing dylib flow:
    /// launcher dlopens, edits in launcher's own world.
    DylibGame,
    /// Plain Rust project (or some other shape we don't recognise).
    /// Falls through to the existing scene-only authoring path.
    Other,
}

/// Inspect a project's `Cargo.toml` to figure out which
/// [`TemplateKind`] it is. Cheap text scan; returns
/// [`TemplateKind::Other`] for anything we can't classify (missing
/// manifest, malformed file, none of the known signatures).
///
/// Avoids pulling in a TOML parser dep for the launcher's
/// hot-path detection. The text scan looks for stable substrings
/// the templates emit; if a user customises their manifest in a
/// way that hides the substring, they fall back to the `Other`
/// path (open in launcher's editor with built-ins only) which is
/// a safe degradation rather than a wrong-mode crash.
pub fn detect_template_kind(project_root: &Path) -> TemplateKind {
    let manifest = project_root.join("Cargo.toml");
    let Ok(text) = std::fs::read_to_string(&manifest) else {
        return TemplateKind::Other;
    };
    let normalized: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let has_editor_feature = normalized.contains("editor=[\"dep:jackdaw\"]")
        || normalized.contains("editor=[\"dep:jackdaw\",");
    let has_editor_bin = normalized.contains("name=\"editor\"")
        && normalized.contains("required-features=[\"editor\"]");
    if has_editor_feature && has_editor_bin {
        return TemplateKind::StaticGameWithEditorFeature;
    }
    if normalized.contains("crate-type=[\"cdylib\"]") {
        return TemplateKind::DylibGame;
    }
    TemplateKind::Other
}

/// Resolve the path to the jackdaw source checkout the running
/// editor was built from, if any. Returns `Some(path)` when the
/// path exists on disk and contains a `templates/` directory (i.e.,
/// the editor is a dev build, not an installed binary). Honours
/// `JACKDAW_DEV_CHECKOUT` as a runtime override for unusual
/// setups.
pub fn jackdaw_dev_checkout() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("JACKDAW_DEV_CHECKOUT") {
        let path = PathBuf::from(p);
        if path.is_dir() {
            return Some(path);
        }
    }
    let compile_time = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut candidate = compile_time.as_path();
    loop {
        if candidate.join("templates").is_dir() && candidate.join("Cargo.toml").is_file() {
            return Some(candidate.to_path_buf());
        }
        candidate = candidate.parent()?;
    }
}

/// Configured branch to scaffold from. Returns:
///   * `JACKDAW_TEMPLATE_BRANCH` if set (any value).
///   * Otherwise [`TEMPLATE_DEFAULT_BRANCH`].
///
/// The scaffolder ignores the branch when scaffolding from a local
/// path (the working tree IS the branch).
pub fn template_branch() -> String {
    std::env::var("JACKDAW_TEMPLATE_BRANCH").unwrap_or_else(|_| TEMPLATE_DEFAULT_BRANCH.to_string())
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

/// Scaffold a project from `template_url` into `<location>/<name>`.
/// Returns the absolute path to the scaffolded project root.
/// Blocks until the subprocess exits; call from a worker thread.
///
/// `template_url` accepts the `<URL> [SUBDIR]` form: a single git
/// URL or local directory followed by an optional sub-directory
/// inside that repo. The composed form mirrors what
/// `bevy new -t URL -- SUBDIR` accepts.
///
/// `branch` pins the git revision when `template_url` is a remote
/// URL; ignored for local-path templates. `None` falls back to
/// [`template_branch`].
///
/// For `Dylib` linkage, writes a `.cargo/config.toml` that routes
/// cargo through `jackdaw-rustc-wrapper` so the scaffolded project
/// links against `libjackdaw_sdk`. For `Static` linkage the config
/// is not written; the project depends on `jackdaw` directly.
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
    // bevy's `-- <args>` passthrough as cargo-generate's positional.
    let mut parts = template_url.split_whitespace();
    let template_arg = parts.next().unwrap_or("").to_string();
    let subdir = parts.next();

    // Path detection: bevy CLI's `-t` always treats its value as a
    // git URL. When the user passes (or `TemplatePreset::git_url`
    // computed) a directory that exists on disk, shell out to
    // `cargo-generate --path` directly so it reads from the
    // filesystem. This is how dev builds running from a source
    // checkout pick up working-tree edits to `templates/*`.
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

    // Sanity-check that `bevy` is on PATH before invoking it.
    let bevy = which_bevy().ok_or(ScaffoldError::BevyCliNotFound)?;

    let mut cmd = Command::new(&bevy);
    cmd.current_dir(location)
        .args(["new", "-t", &template_arg, "--yes", name]);
    // bevy CLI's `new` subcommand exposes only `-t`, `--yes`, and
    // `<NAME>`. Anything else (branch pin, subfolder) rides through
    // its `-- <ARGS>` passthrough to cargo-generate, which accepts
    // `--branch BRANCH` and a positional subfolder.
    let resolved_branch = branch.map(str::to_owned).unwrap_or_else(template_branch);
    let needs_passthrough = !resolved_branch.is_empty() || subdir.is_some();
    if needs_passthrough {
        cmd.arg("--");
        if !resolved_branch.is_empty() {
            cmd.args(["--branch", resolved_branch.as_str()]);
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
    if matches!(linkage, TemplateLinkage::Dylib) {
        write_cargo_config(&project_path);
    }
    rewrite_jackdaw_dep_for_dev_checkout(&project_path, linkage);
    Ok(project_path)
}

/// Local-path scaffold: bypass `bevy new` entirely and call
/// `cargo-generate` directly with `--path`. bevy CLI's `-t` flag
/// always treats its value as a git URL (it doesn't expose
/// `--path`), so contributors iterating on in-tree templates need
/// this branch to skip the clone-to-tmp step that would otherwise
/// 404 against unpublished local paths.
///
/// `local_root[/subdir]` is the template's source directory (e.g.
/// `~/Workspace/jackdaw/templates/game-static`). `cargo-generate`
/// is required for `bevy new` to work anyway, so the binary is
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

    if matches!(linkage, TemplateLinkage::Dylib) {
        write_cargo_config(project_path);
    }
    rewrite_jackdaw_dep_for_dev_checkout(project_path, linkage);
    Ok(project_path.to_path_buf())
}

/// When the launcher is running from a jackdaw source checkout,
/// rewrite the scaffolded project's `Cargo.toml` so the `jackdaw =`
/// dep points at the local checkout via `path = "..."` rather than
/// at the template's default git+branch ref.
///
/// Why: dev contributors testing a feature branch want the
/// scaffolded project to compile against THEIR working tree, not
/// against `main`. Without this rewrite they had to manually edit
/// the scaffolded `Cargo.toml` after every scaffold to swap the
/// dep, which was a recurring papercut.
///
/// Released binaries (no source checkout detected) leave the
/// template's default in place. The template currently pins to
/// `branch = "main"` pre-1.0; switch to `tag = "v0.4.0"` (or
/// `version = "0.4"` once published to crates.io) when the next
/// release ships.
///
/// Static templates declare `jackdaw` with `optional = true`
/// (gated behind the `editor` feature); the rewrite preserves
/// that. Dylib templates don't declare `jackdaw` at all (the
/// rustc-wrapper injects `jackdaw_api`), so this function is a
/// no-op for them.
fn rewrite_jackdaw_dep_for_dev_checkout(project_path: &Path, linkage: TemplateLinkage) {
    let _ = linkage; // currently unused; kept so future linkage-specific tweaks are obvious.
    let Some(checkout) = jackdaw_dev_checkout() else {
        return;
    };
    let manifest_path = project_path.join("Cargo.toml");
    let Ok(contents) = std::fs::read_to_string(&manifest_path) else {
        return;
    };
    let Some(new_contents) = rewrite_jackdaw_dep_line(&contents, &checkout) else {
        return; // dylib template has no jackdaw dep; nothing to rewrite.
    };
    if let Err(e) = std::fs::write(&manifest_path, new_contents) {
        warn!(
            "Failed to rewrite jackdaw dep in {} for dev checkout: {e}",
            manifest_path.display()
        );
    } else {
        info!(
            "Rewrote jackdaw dep in {} to path = '{}' (dev checkout detected)",
            manifest_path.display(),
            checkout.display()
        );
    }
}

/// Single-quoted so Windows backslashes don't get parsed as TOML escapes.
fn rewrite_jackdaw_dep_line(contents: &str, checkout: &Path) -> Option<String> {
    if !contents.contains("jackdaw = {") && !contents.contains("jackdaw=") {
        return None;
    }
    let mut new_contents = String::with_capacity(contents.len());
    let mut rewritten = false;
    for line in contents.lines() {
        let trimmed = line.trim_start();
        if !rewritten && trimmed.starts_with("jackdaw = {") {
            let optional = trimmed.contains("optional = true");
            let mut replacement = format!(
                "jackdaw = {{ path = '{}', default-features = false",
                checkout.display()
            );
            if optional {
                replacement.push_str(", optional = true");
            }
            replacement.push_str(" }");
            new_contents.push_str(&replacement);
            new_contents.push('\n');
            rewritten = true;
            continue;
        }
        new_contents.push_str(line);
        new_contents.push('\n');
    }
    rewritten.then_some(new_contents)
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
         # invocation in this project directory (terminal builds,\n\
         # rust-analyzer, VSCode tasks) links the resulting cdylib\n\
         # against the same bevy compilation the jackdaw editor\n\
         # ships with, keeping TypeIds in sync.\n\
         #\n\
         # Regenerate via jackdaw's scaffolder if the SDK moves.\n\
         \n\
         [build]\n\
         rustc-wrapper = '{wrapper}'\n\
         \n\
         # Windows: `rust-lld` clears MSVC's 65,535 PE export cap.\n\
         # `jackdaw_sdk` re-exports the bevy + jackdaw_api surface,\n\
         # which is over the cap on the default MSVC linker.\n\
         [target.x86_64-pc-windows-msvc]\n\
         linker = 'rust-lld'\n\
         rustflags = ['-C', 'link-arg=-fuse-ld=lld']\n\
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

/// Resolve `cargo-generate` on PATH. Used by the local-path
/// scaffold branch which shells out to `cargo-generate` directly
/// because bevy CLI's `-t` flag doesn't expose `--path`.
pub fn which_cargo_generate() -> Option<PathBuf> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn render_cargo_config_includes_windows_linker_block() {
        let paths = SdkPaths {
            wrapper: PathBuf::from("/abs/path/jackdaw-rustc-wrapper"),
            dylib: PathBuf::from("/abs/path/libjackdaw_sdk.so"),
            deps: PathBuf::from("/abs/path/deps"),
        };
        let body = render_cargo_config(&paths);
        assert!(body.contains("[target.x86_64-pc-windows-msvc]"));
        assert!(body.contains("linker = 'rust-lld'"));
        assert!(body.contains("link-arg=-fuse-ld=lld"));
    }

    #[test]
    fn render_cargo_config_preserves_wrapper_and_env_blocks() {
        let paths = SdkPaths {
            wrapper: PathBuf::from("/w"),
            dylib: PathBuf::from("/d"),
            deps: PathBuf::from("/p"),
        };
        let body = render_cargo_config(&paths);
        assert!(body.contains("[build]"));
        assert!(body.contains("rustc-wrapper = '/w'"));
        assert!(body.contains("[env]"));
        assert!(body.contains("JACKDAW_SDK_DYLIB = '/d'"));
        assert!(body.contains("JACKDAW_SDK_DEPS = '/p'"));
    }

    #[test]
    fn rewrite_jackdaw_dep_line_handles_windows_backslashes() {
        let template = "[package]\n\
                        name = \"my_game\"\n\
                        \n\
                        [dependencies]\n\
                        jackdaw = { version = \"0.4\", optional = true }\n";
        let checkout = PathBuf::from(r"C:\Code\jackdaw");
        let out = rewrite_jackdaw_dep_line(template, &checkout).expect("should rewrite");
        assert!(
            out.contains(r"path = 'C:\Code\jackdaw'"),
            "expected literal-string path, got:\n{out}"
        );
        assert!(
            !out.contains(r#"path = "C:\Code"#),
            "must not emit basic string with raw backslashes:\n{out}"
        );
    }

    #[test]
    fn rewrite_jackdaw_dep_line_preserves_optional_flag() {
        let template = "jackdaw = { version = \"0.4\", optional = true }\n";
        let out = rewrite_jackdaw_dep_line(template, Path::new("/dev/checkout")).unwrap();
        assert!(out.contains("optional = true"));
        assert!(out.contains("default-features = false"));
    }

    #[test]
    fn rewrite_jackdaw_dep_line_omits_optional_when_absent() {
        let template = "jackdaw = { version = \"0.4\" }\n";
        let out = rewrite_jackdaw_dep_line(template, Path::new("/dev/checkout")).unwrap();
        assert!(!out.contains("optional = true"));
        assert!(out.contains("default-features = false"));
    }

    #[test]
    fn rewrite_jackdaw_dep_line_returns_none_when_no_dep() {
        let template = "[package]\nname = \"my_game\"\n\n[dependencies]\nbevy = \"0.18\"\n";
        assert!(rewrite_jackdaw_dep_line(template, Path::new("/dev/checkout")).is_none());
    }

    #[test]
    fn rewrite_jackdaw_dep_line_leaves_other_lines_intact() {
        let template = "[package]\n\
                        name = \"my_game\"\n\
                        \n\
                        [dependencies]\n\
                        bevy = \"0.18\"\n\
                        jackdaw = { version = \"0.4\", optional = true }\n\
                        \n\
                        [features]\n\
                        editor = [\"jackdaw\"]\n";
        let out = rewrite_jackdaw_dep_line(template, Path::new("/dev/checkout")).unwrap();
        assert!(out.contains("name = \"my_game\""));
        assert!(out.contains("bevy = \"0.18\""));
        assert!(out.contains("editor = [\"jackdaw\"]"));
    }
}
