//! Project scaffolding and side-effect-free import planning.
//!
//! Both flows produce the same contract: a normal Bevy crate with a
//! `[lib]` target, a `jackdaw.toml`, and a gitignored `.jackdaw/`
//! directory the editor owns. New projects come from templates
//! embedded in this binary (instantiated by string substitution,
//! offline). Imports are always planned before they are applied; the
//! plan is the single interface shared by `jd import` and the launcher.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use include_dir::{Dir, include_dir};
use jackdaw_env::rust_env_command;
use jackdaw_project_build::modules::{GameDependencies, ModuleCatalog, ModuleSet};
use jackdaw_project_build::runtime_wiring::{
    PIE_FEATURE, RUNTIME_CRATE, RuntimeDependency, RuntimeWiring,
};
use path_slash::PathExt as _;
use toml_edit::DocumentMut;

pub mod dev_checkout;
pub mod migrate;

/// The one Bevy minor this editor release supports.
const SUPPORTED_BEVY_MINOR: &str = jackdaw_project_build::BEVY_VERSION;

static GAME_TEMPLATE: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates/game");
static EXTENSION_TEMPLATE: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates/extension");

/// Which embedded template a new project starts from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TemplateKind {
    Game,
    Extension,
}

impl TemplateKind {
    fn dir(self) -> &'static Dir<'static> {
        match self {
            Self::Game => &GAME_TEMPLATE,
            Self::Extension => &EXTENSION_TEMPLATE,
        }
    }
}

/// What scaffolding or import changed (or left in place).
pub struct ScaffoldReport {
    pub actions: Vec<String>,
    /// A `src/lib.rs` stub was created because the project had no
    /// library target; the user must move their game code into its
    /// `GamePlugin` for their systems to run.
    pub created_lib_stub: bool,
}

/// One exact filesystem effect in an [`ImportPlan`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportChange {
    CreateDirectory {
        path: PathBuf,
    },
    WriteFile {
        path: PathBuf,
        contents: String,
        replace: bool,
    },
}

impl ImportChange {
    pub fn summary(&self) -> String {
        match self {
            Self::CreateDirectory { path } => format!("create directory {}", path.display()),
            // `replace` is the plan's intent; check the disk so the
            // preview never promises to modify a file that is not
            // there, or to create one that is.
            Self::WriteFile { path, .. } if path.is_file() => {
                format!("modify {}", path.display())
            }
            Self::WriteFile { path, .. } => format!("create {}", path.display()),
        }
    }
}

/// A complete import proposal. Constructing it never changes the project.
#[derive(Clone, Debug)]
pub struct ImportPlan {
    /// The directory the user opened, and where `jackdaw.toml` lands.
    pub root: PathBuf,
    /// The package the editor will build. Differs from `root` when the
    /// opened directory is a workspace.
    pub package_dir: PathBuf,
    pub package_name: String,
    pub changes: Vec<ImportChange>,
    pub notes: Vec<String>,
    pub migrated_bin_target: bool,
    /// The modules the game's dependencies already show it using.
    pub detected_modules: ModuleSet,
    /// The modules the project uses once the plan is applied.
    pub modules: ModuleSet,
}

impl ImportPlan {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

/// Why scaffolding could not complete.
#[derive(Debug)]
pub enum ScaffoldError {
    NoManifest(PathBuf),
    ManifestParse(String),
    /// No package could be resolved from the opened directory.
    Package(jackdaw_project_build::cargo_meta::ResolveError),
    /// An operation that assumes an integrated project ran on one that
    /// has not been set up yet.
    NotSetUp(PathBuf),
    /// `jackdaw.toml` exists but does not parse, so the project's
    /// plugin, package, and pins are all unreadable.
    SettingsParse {
        path: PathBuf,
        detail: String,
    },
    /// The project's Bevy dependency does not match the supported minor.
    BevyVersion {
        found: String,
    },
    /// The requested name does not produce a usable Rust crate name.
    ProjectName {
        raw: String,
        reason: &'static str,
    },
    Io(String),
}

impl std::fmt::Display for ScaffoldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScaffoldError::NoManifest(p) => {
                write!(
                    f,
                    "no Cargo.toml at {} (open the folder that holds your project's Cargo.toml)",
                    p.display()
                )
            }
            ScaffoldError::ManifestParse(e) => write!(f, "could not parse Cargo.toml: {e}"),
            ScaffoldError::Package(e) => write!(f, "{e}"),
            ScaffoldError::NotSetUp(path) => write!(
                f,
                "{} is not set up for jackdaw yet (no jackdaw.toml); run `jd import --apply {}` \
                 first",
                path.display(),
                path.display()
            ),
            ScaffoldError::SettingsParse { path, detail } => write!(
                f,
                "{} does not parse: {detail}. Fix it (or delete it and re-import) so jackdaw can \
                 read this project's plugin, package, and versions.",
                path.display()
            ),
            // Jackdaw's version is anchored to the Bevy minor it
            // targets, so the release a mismatched project needs is
            // mechanical: say which one instead of leaving the user to
            // work out the mapping.
            ScaffoldError::BevyVersion { found } => {
                writeln!(
                    f,
                    "this jackdaw targets Bevy {SUPPORTED_BEVY_MINOR}, and the project depends \
                     on bevy {found}. The editor and your game code have to share one Bevy."
                )?;
                // Only suggest a release that could exist: a bogus
                // requirement like `0.190` has no jackdaw to install.
                match bevy_minor_of(found) {
                    Some(minor) => writeln!(
                        f,
                        "Either install jackdaw {minor}.x (releases track the Bevy minor: \
                         https://github.com/jbuehler23/jackdaw/releases), or update the project \
                         to Bevy {SUPPORTED_BEVY_MINOR} and import again."
                    )?,
                    None => writeln!(
                        f,
                        "Update the project to Bevy {SUPPORTED_BEVY_MINOR} and import again."
                    )?,
                }
                write!(
                    f,
                    "To set up anyway and deal with it later, pass --allow-bevy-mismatch."
                )
            }
            ScaffoldError::ProjectName { raw, reason } => {
                write!(f, "`{raw}` is not a usable project name: {reason}")
            }
            ScaffoldError::Io(e) => write!(f, "{e}"),
        }
    }
}

/// CLI entry point for `jd new <name> [--extension]`.
#[expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "CLI subcommand writes its results and errors to the terminal"
)]
pub fn run_new_cli(args: &[String]) -> ExitCode {
    let kind = if args.iter().any(|a| a == "--extension") {
        TemplateKind::Extension
    } else {
        TemplateKind::Game
    };
    let Some(raw_name) = first_positional(args, &["--path", "--modules"]) else {
        eprintln!(
            "jd new: usage: jd new <name> [--extension] [--path <dir>] [--no-git] \
             [--modules <list> | --no-modules]"
        );
        return ExitCode::FAILURE;
    };
    let modules = match modules_arg(args) {
        Ok(None) => ModuleCatalog::builtin().defaults(),
        Ok(Some(_)) if kind == TemplateKind::Extension => {
            eprintln!("jd new: modules apply to games; an extension project has none");
            return ExitCode::FAILURE;
        }
        Ok(Some(modules)) => modules,
        Err(error) => {
            eprintln!("jd new: {error}");
            return ExitCode::FAILURE;
        }
    };
    let project_name = match validated_project_name(raw_name) {
        Ok(name) => name,
        Err(e) => {
            eprintln!("jd new: {e}");
            return ExitCode::FAILURE;
        }
    };
    let location = match flag_value(args, "--path") {
        Some(path) => PathBuf::from(path),
        None => match std::env::current_dir() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("jd new: {e}");
                return ExitCode::FAILURE;
            }
        },
    };
    if !location.is_dir() {
        eprintln!("jd new: {} is not a directory", location.display());
        return ExitCode::FAILURE;
    }
    let dest = location.join(&project_name);
    match scaffold_new_project_with(&dest, &project_name, kind, &modules) {
        Ok(report) => {
            println!("jd new: created {}", dest.display());
            if kind == TemplateKind::Game {
                println!("  modules: {}", describe_modules(&modules));
            }
            for a in &report.actions {
                println!("  {a}");
            }
            if !args.iter().any(|a| a == "--no-git") {
                init_git_repository(&dest);
            }
            // Announce it: on a cold registry the resolve updates the
            // index, which is slow enough to look like a hang.
            println!("  checking dependencies resolve...");
            let warnings = check_new_project(&dest);
            // The dev-checkout rewrite logs through `warn!`, which the
            // CLI has no subscriber for, so without this the user ships
            // a manifest full of absolute local paths and only finds
            // out from a collaborator's failing build.
            if let Some(checkout) = crate::dev_checkout::jackdaw_dev_checkout() {
                println!(
                    "\nnote: this jackdaw runs from the checkout at {}, so the new project \
                     depends on its crates by path. That Cargo.toml is local to this machine; \
                     switch those back to versions before sharing it.",
                    checkout.display()
                );
            }
            for warning in &warnings {
                eprintln!("\nwarning: {warning}");
            }
            match kind {
                // An extension is a library: there is nothing to run.
                // It loads into the editor when its project is open.
                TemplateKind::Extension => println!(
                    "\nNext:\n  jd open {}\nThe extension loads into the editor when that \
                     project is open; `jd build` recompiles it.",
                    dest.display()
                ),
                TemplateKind::Game => println!(
                    "\nNext:\n  jd open {}\nor `cargo run` in that directory to play standalone.",
                    dest.display()
                ),
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("jd new: {e}");
            ExitCode::FAILURE
        }
    }
}

/// A problem with a freshly created project that its first build would
/// otherwise be the first to report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewProjectWarning {
    /// The project sits inside a cargo workspace that no longer resolves.
    EnclosingWorkspaceBroken {
        /// The new project's directory.
        project: PathBuf,
        /// The root of the enclosing workspace.
        workspace: PathBuf,
        /// Cargo's first error line for the workspace.
        detail: String,
    },
    /// The project's own dependencies do not resolve.
    DependenciesUnresolved {
        /// The new project's directory.
        project: PathBuf,
        /// Where this editor's jackdaw crates come from, as a phrase.
        source: String,
        /// Cargo's first error line for the project.
        detail: String,
    },
}

impl NewProjectWarning {
    /// One line naming the problem.
    pub fn summary(&self) -> String {
        match self {
            Self::EnclosingWorkspaceBroken { workspace, .. } => format!(
                "This folder is inside another cargo workspace ({})",
                workspace.display()
            ),
            Self::DependenciesUnresolved { source, .. } => {
                format!("The jackdaw crates could not be resolved from {source}")
            }
        }
    }

    /// One line saying what to do about it.
    pub fn fix(&self) -> String {
        match self {
            Self::EnclosingWorkspaceBroken {
                project, workspace, ..
            } => format!(
                "Add exclude = [\"{}\"] to {}, or remove [workspace] from the new project's \
                 Cargo.toml to make it a member",
                exclude_entry(project, workspace),
                workspace.join("Cargo.toml").display()
            ),
            Self::DependenciesUnresolved { project, .. } => format!(
                "Once that source is reachable, run jd doctor --project {} to check again",
                project.display()
            ),
        }
    }

    /// Cargo's own error line.
    pub fn cargo_error(&self) -> &str {
        match self {
            Self::EnclosingWorkspaceBroken { detail, .. }
            | Self::DependenciesUnresolved { detail, .. } => detail,
        }
    }
}

/// Where a project scaffolded by this editor fetches the jackdaw crates
/// from: the checkout it runs from when the scaffold was rewritten to path
/// dependencies on it, otherwise the source this build was made from.
fn jackdaw_source_description() -> String {
    match crate::dev_checkout::jackdaw_dev_checkout() {
        Some(checkout) => format!("the checkout at {}", checkout.display()),
        None => jackdaw_project_build::build_source::build_source().describe(),
    }
}

/// The project's path relative to the workspace root, as written in that
/// workspace's `exclude` list.
fn exclude_entry(project: &Path, workspace: &Path) -> String {
    project
        .strip_prefix(workspace)
        .unwrap_or(project)
        .to_slash_lossy()
        .into_owned()
}

impl std::fmt::Display for NewProjectWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EnclosingWorkspaceBroken {
                project,
                workspace,
                detail,
            } => write!(
                f,
                "`{}` is inside the cargo workspace at {}, which now reports:\n  {detail}\n\
                 The new project declares its own `[workspace]` so it builds standalone. To \
                 keep the outer workspace working too, either add this line to {}:\n  \
                 exclude = [\"{}\"]\n\
                 or delete the `[workspace]` table from the new project's Cargo.toml to make \
                 it a member.",
                project
                    .file_name()
                    .unwrap_or(project.as_os_str())
                    .to_string_lossy(),
                workspace.display(),
                workspace.join("Cargo.toml").display(),
                exclude_entry(project, workspace),
            ),
            Self::DependenciesUnresolved {
                project,
                source,
                detail,
            } => write!(
                f,
                "this project's dependencies do not resolve yet:\n  {detail}\n\
                 The jackdaw crates could not be resolved from {source}. Run \
                 `jd doctor --project {}` once that source is reachable.",
                project.display()
            ),
        }
    }
}

/// Check a freshly scaffolded project at `dest` for problems its first
/// build would hit. Runs cargo's resolver, which updates the registry
/// index first on a cold cache, so call it off the UI thread.
pub fn check_new_project(dest: &Path) -> Vec<NewProjectWarning> {
    check_new_project_with(dest, resolves)
}

/// [`check_new_project`] with the resolver supplied by the caller.
fn check_new_project_with(
    dest: &Path,
    resolves: impl Fn(&Path) -> Result<(), String>,
) -> Vec<NewProjectWarning> {
    let mut warnings = Vec::new();
    // Creating a project inside an existing repo is normal, and a
    // workspace whose `members` is a glob will adopt it. The scaffold's
    // own `[workspace]` then makes two roots, which breaks every cargo
    // command in the PARENT: a directory the user did not ask us to touch.
    if let Some(workspace) = enclosing_workspace(dest)
        && let Err(detail) = resolves(&workspace)
    {
        warnings.push(NewProjectWarning::EnclosingWorkspaceBroken {
            project: dest.to_path_buf(),
            workspace,
            detail,
        });
    }
    // A version that is not on the index yet would otherwise surface as
    // a baffling failure at the first build, with nothing connecting it
    // to the tool that wrote the file.
    if let Err(detail) = resolves(dest) {
        warnings.push(NewProjectWarning::DependenciesUnresolved {
            project: dest.to_path_buf(),
            source: jackdaw_source_description(),
            detail,
        });
    }
    warnings
}

/// The nearest ancestor directory whose `Cargo.toml` declares a
/// `[workspace]`, if any.
fn enclosing_workspace(dest: &Path) -> Option<PathBuf> {
    let mut candidate = dest.parent()?;
    loop {
        let manifest = candidate.join("Cargo.toml");
        if std::fs::read_to_string(&manifest).is_ok_and(|text| {
            text.parse::<DocumentMut>()
                .is_ok_and(|doc| doc.contains_key("workspace"))
        }) {
            return Some(candidate.to_path_buf());
        }
        candidate = candidate.parent()?;
    }
}

/// Whether cargo can resolve a project's full dependency graph,
/// reporting its first error line. Unlike the `--no-deps` metadata call
/// used to find a package, this one does the version solving.
fn resolves(root: &Path) -> Result<(), String> {
    let output = rust_env_command("cargo")
        .current_dir(root)
        .args(["metadata", "--format-version", "1"])
        .output()
        .map_err(|error| format!("could not run cargo: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(stderr
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("error"))
        .unwrap_or("cargo could not resolve this project's dependencies")
        .to_string())
}

/// The first bare argument, skipping flags and the values belonging to
/// the `value_flags` that take one.
fn first_positional<'a>(args: &'a [String], value_flags: &[&str]) -> Option<&'a String> {
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if value_flags.contains(&arg.as_str()) {
            skip_next = true;
            continue;
        }
        if !arg.starts_with('-') {
            return Some(arg);
        }
    }
    None
}

/// The modules `--modules <list>` or `--no-modules` ask for, or `None`
/// when neither is given.
fn modules_arg(args: &[String]) -> Result<Option<ModuleSet>, String> {
    let none = args.iter().any(|arg| arg == "--no-modules");
    match (flag_value(args, "--modules"), none) {
        (Some(_), true) => Err("pass --modules or --no-modules, not both".to_string()),
        (Some(list), false) => {
            let catalog = ModuleCatalog::builtin();
            catalog
                .parse_list(&list)
                .and_then(|set| catalog.available(set))
                .map(Some)
        }
        (None, true) => Ok(Some(ModuleSet::new())),
        (None, false) => Ok(None),
    }
}

/// The names of `modules` for a summary line, or `none`.
fn describe_modules(modules: &ModuleSet) -> String {
    if modules.is_empty() {
        return "none".to_string();
    }
    ModuleCatalog::builtin().names(&modules.iter().cloned().collect::<Vec<_>>())
}

/// The value following `flag` in `args`, or the `--flag=value` form.
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg == flag {
            return it.next().cloned();
        }
        if let Some(value) = arg.strip_prefix(&format!("{flag}=")) {
            return Some(value.to_string());
        }
    }
    None
}

/// `git init` a freshly scaffolded project, matching what `cargo new`
/// does. Skipped when the destination already sits inside a repository
/// (so scaffolding into an existing checkout does not nest one) and
/// when git is unavailable. Best effort: a failure is not worth
/// aborting a successful scaffold over.
pub fn init_git_repository(dest: &Path) {
    let inside_repo = jackdaw_env::without_console_window(&mut std::process::Command::new("git"))
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(dest)
        .output()
        .is_ok_and(|out| out.status.success() && out.stdout.starts_with(b"true"));
    if inside_repo {
        return;
    }
    let _ = jackdaw_env::without_console_window(&mut std::process::Command::new("git"))
        .arg("init")
        .arg("--quiet")
        .current_dir(dest)
        .status();
}

/// CLI entry point for `jd import [path] [--plugin <Type>] [--apply]`.
#[expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "CLI subcommand writes its results and errors to the terminal"
)]
pub fn run_import_cli(args: &[String]) -> ExitCode {
    let plugin = parse_plugin_arg(args);
    let package = flag_value(args, "--package");
    let requested_root =
        first_positional(args, &["--plugin", "--package", "--modules"]).map(PathBuf::from);
    let modules = match modules_arg(args) {
        Ok(modules) => modules,
        Err(error) => {
            eprintln!("jd import: {error}");
            return ExitCode::FAILURE;
        }
    };
    let root = match requested_root
        .map(Ok)
        .unwrap_or_else(std::env::current_dir)
        .and_then(dunce::canonicalize)
    {
        Ok(d) => d,
        Err(e) => {
            eprintln!("jd import: {e}");
            return ExitCode::FAILURE;
        }
    };
    let allow_mismatch = args.iter().any(|arg| arg == "--allow-bevy-mismatch");
    match plan_import_with_modules(
        &root,
        plugin,
        package.as_deref(),
        allow_mismatch,
        modules.as_ref(),
    ) {
        Ok(plan) if plan.is_empty() => {
            println!("jd import: {} is already set up", root.display());
            if !RuntimeWiring::inspect(&plan.package_dir).complete() {
                for note in &plan.notes {
                    println!("  note: {note}");
                }
            }
            println!("\nNext: `jd open {}`", root.display());
            ExitCode::SUCCESS
        }
        Ok(plan) => {
            println!("jd import preview: {}", root.display());
            if plan.package_dir != plan.root {
                println!("  package: {}", plan.package_name);
            }
            println!("  modules: {}", describe_modules(&plan.modules));
            for change in &plan.changes {
                println!("  {}", change.summary());
            }
            for note in &plan.notes {
                println!("  note: {note}");
            }
            if !args.iter().any(|arg| arg == "--apply") {
                println!(
                    "\nNo files changed. Run `jd import --apply {}` to apply.",
                    root.display()
                );
                return ExitCode::SUCCESS;
            }
            match apply_import_plan(&plan) {
                Ok(report) => {
                    println!("\nApplied {} change(s).", report.actions.len());
                    if report.created_lib_stub {
                        println!(
                            "\nYour project had no library target, so a `GamePlugin` stub was \
                             created in src/lib.rs. Move your game setup there from main.rs; the \
                             editor only sees components that live in the library."
                        );
                    }
                    println!("\nNext: `jd open {}`", root.display());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("jd import: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Err(e) => {
            eprintln!("jd import: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Plan the edits that bring a project up to this jackdaw: refresh the
/// `[jackdaw]` pins, and bump any `jackdaw_*` requirement to the
/// anchored minor.
///
/// Side-effect free, like the import plan: the changes are previewed,
/// then applied on a second explicit action.
pub fn plan_upgrade_project(root: &Path) -> Result<ImportPlan, ScaffoldError> {
    let manifest_path = root.join("Cargo.toml");
    if !manifest_path.is_file() {
        return Err(ScaffoldError::NoManifest(manifest_path));
    }
    // Upgrading is for projects that are already set up. Writing a
    // pins-only `jackdaw.toml` here would leave a project with no
    // plugin, no run config, and no `.jackdaw/`, while making it look
    // set up to every later check, which permanently skips the import
    // flow that would have completed it.
    if !jackdaw_project_build::project_manifest::ProjectManifest::exists(root) {
        return Err(ScaffoldError::NotSetUp(root.to_path_buf()));
    }
    let settings = jackdaw_project_build::project_manifest::ProjectManifest::read_checked(root)
        .map_err(|detail| ScaffoldError::SettingsParse {
            path: jackdaw_project_build::project_manifest::path(root),
            detail,
        })?;
    let package = jackdaw_project_build::cargo_meta::resolve_project_package(
        root,
        settings.package.as_deref(),
    )
    .map_err(ScaffoldError::Package)?;

    let mut changes = Vec::new();
    let mut notes = Vec::new();

    // What the project's own manifest says it targets, which is what
    // the pins have to describe. An upgrade moves the jackdaw version;
    // it does not migrate anyone's Bevy for them.
    let project_bevy = jackdaw_project_build::project_manifest::targeted_bevy_from_manifest(root);
    if project_bevy
        .as_deref()
        .is_some_and(|minor| minor != jackdaw_project_build::BEVY_VERSION)
    {
        notes.push(format!(
            "this project targets bevy {}, not {}; upgrading records the jackdaw version but \
             leaves that mismatch in place",
            project_bevy.as_deref().unwrap_or_default(),
            jackdaw_project_build::BEVY_VERSION
        ));
    }

    // The bevy pin as recorded before the rewrite, so the preview can
    // name that change too. A note saying only "records jackdaw 0.19.0"
    // while silently moving the bevy pin describes half of what it does.
    let recorded_bevy = settings.jackdaw.bevy.clone();

    let modules = settings.modules(root);
    let settings_path = jackdaw_project_build::project_manifest::path(root);
    let settings = std::fs::read_to_string(&settings_path).unwrap_or_default();
    let mut new_settings = settings.clone();
    if let Some(updated) = rewrite_pins(&settings, project_bevy.as_deref()) {
        new_settings = updated;
        let recorded_bevy = recorded_bevy.as_deref().unwrap_or("none");
        let new_bevy = project_bevy
            .as_deref()
            .unwrap_or(jackdaw_project_build::BEVY_VERSION);
        if recorded_bevy == new_bevy {
            notes.push(format!(
                "records jackdaw {}",
                jackdaw_project_build::VERSION
            ));
        } else {
            notes.push(format!(
                "records jackdaw {}, and corrects the bevy pin {recorded_bevy} -> {new_bevy} to \
                 match this project's Cargo.toml",
                jackdaw_project_build::VERSION
            ));
        }
    }

    // Both manifests can carry jackdaw requirements: the member's own,
    // and the workspace root's `[workspace.dependencies]` that members
    // inherit with `{ workspace = true }`. Missing the root misses the
    // more common workspace idiom entirely.
    // Only when the project is on this editor's Bevy. Moving
    // `jackdaw_runtime` to a version that requires Bevy 0.19 inside a
    // project pinned to an older Bevy produces a manifest that cannot
    // resolve, which is worse than leaving it alone.
    let bump = project_bevy
        .as_deref()
        .is_none_or(|minor| minor == jackdaw_project_build::BEVY_VERSION);
    if !bump {
        notes.push(
            "leaving the jackdaw dependencies alone: moving them forward would require a bevy \
             this project does not use"
                .to_string(),
        );
    }
    let mut manifests = vec![package.dir.join("Cargo.toml")];
    if package.dir != root {
        manifests.push(root.join("Cargo.toml"));
    }
    let requirement = jackdaw_project_build::jackdaw_requirement();
    let mut bumped_any = false;
    let mut dropped = Vec::new();
    for manifest_path in manifests {
        let cargo = std::fs::read_to_string(&manifest_path)
            .map_err(|e| ScaffoldError::Io(format!("{}: {e}", manifest_path.display())))?;
        let mut updated = cargo.clone();
        if bump && let Some(bumped) = bump_jackdaw_requirements(&updated, &requirement) {
            updated = bumped;
            bumped_any = true;
        }
        if let Ok(mut doc) = updated.parse::<DocumentMut>() {
            let removed = drop_removed_runtime_features(&mut doc);
            if !removed.is_empty() {
                updated = doc.to_string();
                dropped.extend(removed);
            }
        }
        if updated != cargo {
            changes.push(ImportChange::WriteFile {
                path: manifest_path,
                contents: updated,
                replace: true,
            });
        }
    }
    if bumped_any {
        notes.push(format!("requests the jackdaw crates at {requirement}"));
    }
    dropped.sort();
    dropped.dedup();
    if !dropped.is_empty() {
        let catalog = ModuleCatalog::builtin();
        let mut target = modules;
        target.extend(
            dropped
                .iter()
                .filter_map(|feature| catalog.replacing_feature(feature))
                .map(|module| module.id.clone()),
        );
        let target = catalog.complete(target);
        new_settings = jackdaw_project_build::modules::with_modules(&new_settings, &target)
            .map_err(|detail| ScaffoldError::SettingsParse {
                path: settings_path.clone(),
                detail,
            })?;
        notes.extend(removed_feature_notes(&dropped));
    }
    if new_settings != settings {
        changes.push(ImportChange::WriteFile {
            path: settings_path,
            contents: new_settings,
            replace: true,
        });
    }

    Ok(ImportPlan {
        root: root.to_path_buf(),
        package_dir: package.dir,
        package_name: package.name,
        changes,
        notes,
        migrated_bin_target: false,
        detected_modules: ModuleSet::new(),
        modules: ModuleSet::new(),
    })
}

/// Bring the `[jackdaw]` pins up to this build. `None` when they are
/// already current.
///
/// `bevy` is only advanced when the project's own manifest actually
/// targets this Bevy minor. Writing it unconditionally would mark a
/// mismatched project healthy without changing anything about it,
/// undoing the very flag `--allow-bevy-mismatch` records so the user
/// can deal with it later.
fn rewrite_pins(settings: &str, project_bevy: Option<&str>) -> Option<String> {
    let mut doc: DocumentMut = settings.parse().ok()?;
    let get = |key: &str| {
        doc.get("jackdaw")
            .and_then(|table| table.get(key))
            .and_then(|value| value.as_str())
            .map(str::to_string)
    };
    let bevy = match project_bevy {
        Some(minor) => minor.to_string(),
        // Nothing conclusive in the manifest: keep whatever the project
        // already recorded rather than asserting this editor's.
        None => get("bevy").unwrap_or_else(|| jackdaw_project_build::BEVY_VERSION.to_string()),
    };
    if get("version").as_deref() == Some(jackdaw_project_build::VERSION)
        && get("bevy").as_deref() == Some(bevy.as_str())
    {
        return None;
    }
    // Edit the existing table in place where there is one, so its
    // position in the file, and any other keys or comments inside it,
    // survive. Replacing the item wholesale moves it to the end and
    // drops anything the user put there.
    let pins: DocumentMut = jackdaw_project_build::project_manifest::pins_toml_for_bevy(&bevy)
        .parse()
        .ok()?;
    match doc
        .get_mut("jackdaw")
        .and_then(|item| item.as_table_like_mut())
    {
        Some(existing) => {
            // Assign through the existing item rather than `insert`:
            // `insert` replaces the whole key-value entry, taking any
            // comment the user wrote above the key with it.
            let mut set = |key: &str, text: &str| match existing.get_mut(key) {
                Some(item) => *item = toml_edit::value(text),
                None => {
                    existing.insert(key, toml_edit::value(text));
                }
            };
            set("version", jackdaw_project_build::VERSION);
            set("bevy", &bevy);
        }
        None => doc["jackdaw"] = pins["jackdaw"].clone(),
    }
    Some(doc.to_string())
}

/// Point every `jackdaw_*` dependency at `requirement`. `None` when
/// nothing needed changing (already current, or path/git deps, which
/// state no version to bump).
fn bump_jackdaw_requirements(cargo: &str, requirement: &str) -> Option<String> {
    const TABLES: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];
    let mut doc: DocumentMut = cargo.parse().ok()?;
    let mut changed = false;

    for table in TABLES {
        if let Some(deps) = doc.get_mut(table).and_then(|item| item.as_table_like_mut()) {
            changed |= bump_jackdaw_deps(deps, requirement);
        }
    }
    // A platform-gated jackdaw dependency is still a jackdaw
    // dependency; leaving it behind contradicts what the upgrade says
    // it did.
    if let Some(targets) = doc
        .get_mut("target")
        .and_then(|item| item.as_table_like_mut())
    {
        for (_, cfg) in targets.iter_mut() {
            let Some(cfg) = cfg.as_table_like_mut() else {
                continue;
            };
            for table in TABLES {
                if let Some(deps) = cfg.get_mut(table).and_then(|item| item.as_table_like_mut()) {
                    changed |= bump_jackdaw_deps(deps, requirement);
                }
            }
        }
    }
    // `[workspace.dependencies]`, which members inherit with
    // `{ workspace = true }`. In a workspace this is usually the only
    // place a version is written down at all.
    if let Some(workspace) = doc
        .get_mut("workspace")
        .and_then(|item| item.as_table_like_mut())
        && let Some(deps) = workspace
            .get_mut("dependencies")
            .and_then(|item| item.as_table_like_mut())
    {
        changed |= bump_jackdaw_deps(deps, requirement);
    }
    changed.then(|| doc.to_string())
}

/// Move every `jackdaw_*` entry in one dependency table to
/// `requirement`. Returns whether anything changed.
fn bump_jackdaw_deps(deps: &mut dyn toml_edit::TableLike, requirement: &str) -> bool {
    let mut changed = false;
    let names: Vec<String> = deps
        .iter()
        .map(|(name, _)| name.to_string())
        .filter(|name| name.starts_with("jackdaw_"))
        .collect();
    for name in names {
        let Some(entry) = deps.get_mut(&name) else {
            continue;
        };
        if let Some(existing) = entry.as_str() {
            if existing != requirement {
                *entry = toml_edit::value(requirement);
                changed = true;
            }
        } else if let Some(table) = entry.as_table_like_mut() {
            // A path or git dependency resolves elsewhere; there is no
            // version requirement to bring forward.
            if table.get("path").is_some() || table.get("git").is_some() {
                continue;
            }
            let current = table.get("version").and_then(|v| v.as_str());
            if current.is_some() && current != Some(requirement) {
                table.insert("version", toml_edit::value(requirement));
                changed = true;
            }
        }
    }
    changed
}

/// CLI entry point for `jd upgrade [path] [--apply]`.
#[expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "CLI subcommand writes its results and errors to the terminal"
)]
pub fn run_upgrade_cli(args: &[String]) -> ExitCode {
    let requested = first_positional(args, &[]).map(PathBuf::from);
    let root = match requested
        .clone()
        .map(Ok)
        .unwrap_or_else(std::env::current_dir)
        .and_then(dunce::canonicalize)
    {
        Ok(root) => root,
        Err(error) => {
            match requested {
                Some(path) => eprintln!("jd upgrade: {}: {error}", path.display()),
                None => eprintln!("jd upgrade: {error}"),
            }
            return ExitCode::FAILURE;
        }
    };
    match plan_upgrade_project(&root) {
        Ok(plan) if plan.is_empty() => {
            println!(
                "jd upgrade: {} is already on jackdaw {}",
                root.display(),
                jackdaw_project_build::VERSION
            );
            // Notes survive an empty plan, and the one that matters is
            // the bevy mismatch: `jd doctor` sends people to upgrade for
            // exactly that, and reporting only "already on jackdaw
            // 0.19.0" reads as if there were nothing to fix.
            for note in &plan.notes {
                println!("  note: {note}");
            }
            ExitCode::SUCCESS
        }
        Ok(plan) => {
            println!("jd upgrade preview: {}", root.display());
            for change in &plan.changes {
                println!("  {}", change.summary());
            }
            for note in &plan.notes {
                println!("  note: {note}");
            }
            if !args.iter().any(|arg| arg == "--apply") {
                println!(
                    "\nNo files changed. Run `jd upgrade --apply {}` to apply.",
                    root.display()
                );
                return ExitCode::SUCCESS;
            }
            match apply_import_plan(&plan) {
                Ok(report) => {
                    println!("\nApplied {} change(s).", report.actions.len());
                    println!("\nNext: `jd build {}`", root.display());
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("jd upgrade: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Err(error) => {
            eprintln!("jd upgrade: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The editor binary installed beside the running executable, or
/// `jackdaw` from `PATH` when the location is unknown.
pub fn editor_executable() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.parent()
                .map(|parent| parent.join(editor_executable_name()))
        })
        .unwrap_or_else(|| PathBuf::from(editor_executable_name()))
}

fn editor_executable_name() -> &'static str {
    if cfg!(windows) {
        "jackdaw.exe"
    } else {
        "jackdaw"
    }
}

fn parse_plugin_arg(args: &[String]) -> Option<String> {
    flag_value(args, "--plugin")
}

/// What a new game's starter scene holds. The game template ships the 3D
/// one; the others replace it after scaffolding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StarterScene {
    /// A camera, a sun and the game plugin that loads them.
    #[default]
    ThreeD,
    /// A 2D camera and a sprite.
    TwoD,
    /// An empty root, for a game that builds its world in code.
    Empty,
}

const STARTER_SCENE_2D: &str = "\
// Starter scene. Authored by the jackdaw editor; safe to edit by hand.

#Root
bevy_transform::components::transform::Transform
bevy_camera::visibility::Visibility::Visible
bevy_ecs::hierarchy::Children [
    #\"Main Camera\"
    bevy_camera::components::Camera2d
    bevy_transform::components::transform::Transform
    bevy_camera::visibility::Visibility::Visible
    --
    #Sprite
    bevy_sprite::sprite::Sprite
    bevy_transform::components::transform::Transform {
        scale: glam::Vec3 { x: 64.0, y: 64.0, z: 1.0 },
    }
    bevy_camera::visibility::Visibility::Visible
]
";

const STARTER_SCENE_EMPTY: &str = "\
// Starter scene. Authored by the jackdaw editor; safe to edit by hand.

#Root
bevy_transform::components::transform::Transform
bevy_camera::visibility::Visibility::Visible
";

/// Replace the starter scene of the game scaffolded at `dest` with
/// `starter`'s.
pub fn write_starter_scene(dest: &Path, starter: StarterScene) -> Result<(), ScaffoldError> {
    let text = match starter {
        StarterScene::ThreeD => return Ok(()),
        StarterScene::TwoD => STARTER_SCENE_2D,
        StarterScene::Empty => STARTER_SCENE_EMPTY,
    };
    let scene = dest.join("assets").join("scene.bsn");
    std::fs::write(&scene, text)
        .map_err(|error| ScaffoldError::Io(format!("{}: {error}", scene.display())))
}

/// Scaffold a new project at `dest` from an embedded template,
/// substituting the project name, crate name, authors, and title
/// placeholders. A game starts with the default modules.
pub fn scaffold_new_project(
    dest: &Path,
    project_name: &str,
    kind: TemplateKind,
) -> Result<ScaffoldReport, ScaffoldError> {
    scaffold_new_project_with(
        dest,
        project_name,
        kind,
        &ModuleCatalog::builtin().defaults(),
    )
}

/// As [`scaffold_new_project`], with the game using `modules`: they are
/// recorded in `jackdaw.toml`, and their runtime features, crates and
/// plugins are written into the game. Requirements are added. Reusable by
/// the launcher's New Project action.
pub fn scaffold_new_project_with(
    dest: &Path,
    project_name: &str,
    kind: TemplateKind,
    modules: &ModuleSet,
) -> Result<ScaffoldReport, ScaffoldError> {
    let modules = ModuleCatalog::builtin().complete(modules.clone());
    if dest.exists()
        && dest
            .read_dir()
            .map(|mut d| d.next().is_some())
            .unwrap_or(false)
    {
        return Err(ScaffoldError::Io(format!(
            "{} already exists and is not empty",
            dest.display()
        )));
    }

    let crate_name = project_name.replace('-', "_");
    let title = title_case(project_name);
    let authors = git_authors();

    let mut files = Vec::new();
    collect_template_files(kind.dir(), &mut files);

    let mut written = 0usize;
    for file in files {
        let rel = file.path().to_slash_lossy().into_owned();
        let (dest_rel, contents): (PathBuf, Vec<u8>) = match rel.strip_suffix(".template") {
            Some(stripped) => {
                let text = std::str::from_utf8(file.contents()).map_err(|_| {
                    ScaffoldError::Io(format!("template file {rel} is not valid UTF-8"))
                })?;
                let rendered = substitute_placeholders(
                    text,
                    project_name,
                    &crate_name,
                    &authors,
                    &title,
                    &modules,
                );
                // Dotfiles are stored undotted so they do not act on
                // the jackdaw repository itself.
                let stripped = if stripped == "gitignore" {
                    ".gitignore"
                } else {
                    stripped
                };
                (PathBuf::from(stripped), rendered.into_bytes())
            }
            None => (PathBuf::from(&rel), file.contents().to_vec()),
        };

        let out_path = dest.join(&dest_rel);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ScaffoldError::Io(format!("{e}")))?;
        }
        std::fs::write(&out_path, contents).map_err(|e| ScaffoldError::Io(format!("{e}")))?;
        written += 1;
    }

    // In a jackdaw source checkout, repoint the new project's jackdaw
    // deps at the local workspace so it builds against the code being
    // worked on rather than the last release.
    crate::dev_checkout::rewrite_jackdaw_dep_for_dev_checkout(dest);

    Ok(ScaffoldReport {
        actions: vec![format!("scaffolded {written} files")],
        created_lib_stub: false,
    })
}

/// Plan integration of an existing Bevy project without changing it.
pub fn plan_import_project(
    root: &Path,
    plugin_override: Option<String>,
) -> Result<ImportPlan, ScaffoldError> {
    plan_import_package(root, plugin_override, None)
}

/// As [`plan_import_project`], targeting a named workspace member.
///
/// `jackdaw.toml` and `.jackdaw/` always land at `root` (the directory
/// the user opened), while the source changes land in the resolved
/// package. For a single-package project the two are the same
/// directory.
pub fn plan_import_package(
    root: &Path,
    plugin_override: Option<String>,
    package_override: Option<&str>,
) -> Result<ImportPlan, ScaffoldError> {
    plan_import_with(root, plugin_override, package_override, false)
}

/// As [`plan_import_package`], optionally proceeding despite a Bevy
/// minor mismatch.
///
/// The mismatch is a real blocker for building project code, but
/// setting a project up is additive and reversible, and a user
/// mid-upgrade has a legitimate reason to do it before their code
/// compiles. Refusing with no override turns a warning into a dead end.
pub fn plan_import_with(
    root: &Path,
    plugin_override: Option<String>,
    package_override: Option<&str>,
    allow_bevy_mismatch: bool,
) -> Result<ImportPlan, ScaffoldError> {
    plan_import_with_modules(
        root,
        plugin_override,
        package_override,
        allow_bevy_mismatch,
        None,
    )
}

/// As [`plan_import_with`], choosing the modules the project uses.
///
/// Modules the game's dependencies already show are always kept: import
/// only adds. `None` adds nothing beyond those, except that a game without
/// `jackdaw_runtime` yet gets the default modules with it.
pub fn plan_import_with_modules(
    root: &Path,
    plugin_override: Option<String>,
    package_override: Option<&str>,
    allow_bevy_mismatch: bool,
    modules: Option<&ModuleSet>,
) -> Result<ImportPlan, ScaffoldError> {
    let manifest_path = root.join("Cargo.toml");
    if !manifest_path.is_file() {
        return Err(ScaffoldError::NoManifest(manifest_path));
    }
    // A `jackdaw.toml` that does not parse is worse than none: every
    // later check treats its existence as proof the project is set up,
    // so import found nothing to do and reported success, which is the
    // fix `jd doctor` sends people here to apply.
    if jackdaw_project_build::project_manifest::ProjectManifest::exists(root)
        && let Err(detail) =
            jackdaw_project_build::project_manifest::ProjectManifest::read_checked(root)
    {
        return Err(ScaffoldError::SettingsParse {
            path: jackdaw_project_build::project_manifest::path(root),
            detail,
        });
    }

    let package =
        jackdaw_project_build::cargo_meta::resolve_project_package(root, package_override)
            .map_err(ScaffoldError::Package)?;
    let package_dir = package.dir.clone();
    let package_manifest = package_dir.join("Cargo.toml");
    let manifest_text = std::fs::read_to_string(&package_manifest)
        .map_err(|e| ScaffoldError::Io(format!("{}: {e}", package_manifest.display())))?;
    let doc: DocumentMut = manifest_text
        .parse()
        .map_err(|e| ScaffoldError::ManifestParse(format!("{e}")))?;

    // A workspace member can inherit `bevy = { workspace = true }`, so
    // the root manifest is the second place to look.
    let workspace_doc = (package_dir != root)
        .then(|| std::fs::read_to_string(&manifest_path).ok())
        .flatten()
        .and_then(|text| text.parse::<DocumentMut>().ok());
    let mut changes = Vec::new();
    let mut notes = Vec::new();
    let mut migrated_bin_target = false;

    // On a mismatch the pins must record what the PROJECT targets, not
    // what this editor does. Writing the editor's version would make
    // every later check report health and silently disable the warning
    // the user chose to defer, which is the opposite of deferring it.
    let mut pinned_bevy = None;
    let mut bevy_mismatch = false;
    match check_bevy_version(&doc, workspace_doc.as_ref()) {
        Ok(()) => {}
        Err(error) if allow_bevy_mismatch => {
            bevy_mismatch = true;
            if let ScaffoldError::BevyVersion { found } = &error {
                pinned_bevy = bevy_minor_of(found);
            }
            notes.push(format!("proceeding despite a Bevy mismatch: {error}"));
            notes.push(
                "jackdaw.toml will record the Bevy this project targets, so the mismatch keeps \
                 being reported until you resolve it"
                    .to_string(),
            );
        }
        Err(error) => return Err(error),
    }

    if package_dir != root {
        notes.push(format!(
            "workspace detected; `{}` is the game package (override with the `package` key in \
             jackdaw.toml)",
            package.name
        ));
    }

    // A runtime built for this editor's Bevy cannot join a project on
    // another one, so a mismatched import only reports what is missing. A
    // declaration import cannot safely extend is left to the user, and no
    // source may name the crate until it is a plain dependency.
    let wiring = RuntimeWiring::inspect(&package_dir);
    let catalog = ModuleCatalog::builtin();
    let game = GameDependencies::read(&package_dir, root);
    let detected_modules = catalog.detect(&game);
    let chosen_modules = match modules {
        Some(chosen) => chosen.clone(),
        None if wiring.dependency == RuntimeDependency::Missing => catalog.defaults(),
        None => ModuleSet::new(),
    };
    let selected_modules =
        catalog.complete(chosen_modules.union(&detected_modules).cloned().collect());
    let declared_plain = wiring.dependency == RuntimeDependency::Plain;
    let wire_runtime = !bevy_mismatch
        && matches!(
            wiring.dependency,
            RuntimeDependency::Missing | RuntimeDependency::Plain
        );
    let plugin_ready = wire_runtime || declared_plain;
    let pie_always = wiring.pie_feature || (wire_runtime && !wiring.pie_via_feature);
    let windowless_ready = plugin_ready && pie_always;
    if let RuntimeDependency::Unsupported(form) = wiring.dependency {
        notes.push(format!(
            "Cargo.toml declares jackdaw_runtime as {form}, so import leaves it and your \
             sources alone; make it a plain [dependencies] entry with the `pie` feature and \
             re-run `jd import`"
        ));
    }
    if wire_runtime
        && let Some(edit) =
            runtime_dependency_edit(&manifest_text, &wiring, &selected_modules, &game)
    {
        changes.push(ImportChange::WriteFile {
            path: package_manifest.clone(),
            contents: edit.contents,
            replace: true,
        });
        notes.extend(edit.notes);
    }
    let added_modules: Vec<String> = selected_modules
        .difference(&detected_modules)
        .cloned()
        .collect();
    if wire_runtime {
        notes.extend(plugin_notes(
            &package_dir,
            &added_modules.iter().cloned().collect(),
        ));
    }
    if !wire_runtime && !added_modules.is_empty() {
        notes.push(format!(
            "jackdaw.toml records {} as used, but Cargo.toml is left for you to update",
            catalog.names(&added_modules)
        ));
    }

    // Lib target: an explicit [lib] or the conventional src/lib.rs.
    let has_lib = doc.get("lib").is_some() || package_dir.join("src/lib.rs").is_file();
    // The plugin the migration will declare. Not always `GamePlugin`: a
    // crate that already defines that name gets a differently-named
    // wrapper, and recording the wrong one points the shim at a type
    // that will not exist.
    let mut migrated_plugin: Option<String> = None;
    if !has_lib {
        match crate::migrate::plan_migration(&package_dir, &package.crate_name, windowless_ready) {
            Ok(migration) => {
                let main_path = package_dir.join("src/main.rs");
                let original_main = std::fs::read_to_string(&main_path)
                    .map_err(|e| ScaffoldError::Io(format!("{e}")))?;
                changes.push(ImportChange::WriteFile {
                    path: package_dir.join("src/main.rs.bak"),
                    contents: original_main,
                    replace: package_dir.join("src/main.rs.bak").exists(),
                });
                changes.push(ImportChange::WriteFile {
                    path: main_path,
                    contents: migration.main_rs,
                    replace: true,
                });
                changes.push(ImportChange::WriteFile {
                    path: package_dir.join("src/lib.rs"),
                    contents: migration.lib_rs,
                    replace: false,
                });
                migrated_plugin = Some(migration.plugin.clone());
                notes.extend(migration.notes);
                migrated_bin_target = true;
            }
            Err(error) => {
                changes.push(ImportChange::WriteFile {
                    path: package_dir.join("src/lib.rs"),
                    contents: lib_stub_source(plugin_ready),
                    replace: false,
                });
                notes.push(format!(
                    "automatic bin-to-library conversion was unavailable: {error}; \
                     move game setup into GamePlugin before Play"
                ));
            }
        }
    }

    // Plugin type: explicit override, then source detection, then the
    // GamePlugin convention (which is what the stub provides). Recorded
    // in jackdaw.toml for setup notes and `jd doctor`.
    let candidates = jackdaw_project_build::detect::plugin_candidates(&package_dir);
    let detected = detect_plugin(&package_dir, &package.crate_name)
        .and_then(|p| p.split_once("::").map(|(_, name)| name.to_string()));
    // A migration or a stub creates a plugin, so it can be recorded even
    // though the scan above ran before those files existed. The
    // migration reports the name it chose; the stub always uses
    // `GamePlugin`.
    let will_create_plugin = (!has_lib).then(|| {
        migrated_plugin
            .clone()
            .unwrap_or_else(|| "GamePlugin".to_string())
    });
    let plugin = plugin_override
        .clone()
        .or_else(|| detected.clone())
        .or(will_create_plugin);
    // An override is the user's call; warn when the crate does not
    // declare that name so doctor does not later fail on it.
    if let Some(name) = &plugin_override
        && !candidates.iter().any(|candidate| candidate == name)
    {
        notes.push(format!(
            "`{name}` was requested but {} declares no such plugin{}",
            package.crate_name,
            if candidates.is_empty() {
                String::new()
            } else {
                format!(" (found: {})", candidates.join(", "))
            }
        ));
    }
    if plugin_override.is_none() {
        // Distinguish "none exists" from "one exists but is not
        // reachable from outside the crate": telling a user with a
        // perfectly good plugin that none was found sends them looking
        // for the wrong problem.
        let unreachable: Vec<String> = jackdaw_project_build::detect::plugin_paths(&package_dir)
            .into_iter()
            .filter(|found| found.crate_path.is_none())
            .map(|found| found.type_name)
            .collect();
        match &plugin {
            Some(name) if detected.is_some() => notes.push(format!("game plugin: {name}")),
            Some(name) => notes.push(format!("game plugin: {name} (created by this setup)")),
            // Recording a name the crate does not define would leave
            // doctor failing on a key that does not match source, so
            // leave it unset and say what to do when there is one.
            None if candidates.len() >= 2 => notes.push(format!(
                "several plugins found ({}) and none is named GamePlugin; set `plugin` in \
                 jackdaw.toml to name the root one, and add it from `main.rs` for Play",
                candidates.join(", ")
            )),
            None if !unreachable.is_empty() => notes.push(format!(
                "found {}, but it is not reachable from outside the crate; make its module \
                 `pub mod` or re-export it from lib.rs, then set `plugin` in jackdaw.toml",
                unreachable.join(", ")
            )),
            // "Your source does not parse" is a different problem from
            // "you have no plugin", with a different next action.
            None => match jackdaw_project_build::detect::lib_parse_error(&package_dir) {
                Some(error) => notes.push(format!(
                    "could not read {}: {error}. Fix it and re-run, or set `plugin` in \
                     jackdaw.toml by hand.",
                    package_dir.join("src/lib.rs").display()
                )),
                None => notes.push(
                    "no `impl Plugin for ...` found; the editor will still read this crate's \
                     components. Add a Bevy Plugin in the library and wire it from `main.rs` \
                     for Play."
                        .to_string(),
                ),
            },
        }
    }

    // A migration and the library stub wire the runtime themselves.
    if !migrated_bin_target {
        if bevy_mismatch && wiring.dependency == RuntimeDependency::Missing {
            let mut features = module_runtime_features(&selected_modules);
            features.push(PIE_FEATURE.to_string());
            notes.push(format!(
                "add the runtime once the project is on bevy {}: cargo add jackdaw_runtime@{} \
                 --features {}",
                jackdaw_project_build::BEVY_VERSION,
                jackdaw_project_build::BEVY_VERSION,
                features.join(",")
            ));
        }
        if !wiring.plugin && (has_lib || !plugin_ready) {
            let edit = plugin_ready
                .then_some(plugin.as_deref())
                .flatten()
                .and_then(|name| runtime_plugin_edit(&package_dir, name));
            match edit {
                Some((path, contents)) => {
                    changes.push(ImportChange::WriteFile {
                        path,
                        contents,
                        replace: true,
                    });
                    if let Some(name) = plugin.as_deref().and_then(|p| p.rsplit("::").next())
                        && let Ok(main) = std::fs::read_to_string(package_dir.join("src/main.rs"))
                        && !jackdaw_project_build::runtime_wiring::mentions_ident(&main, name)
                    {
                        notes.push(format!(
                            "src/main.rs does not add `{name}`; add it to your App so Play and \
                             component discovery reach the runtime plugin"
                        ));
                    }
                }
                None => notes.push(crate::migrate::runtime_wiring_note()),
            }
        }
        if !wiring.windowless {
            let main_path = package_dir.join("src/main.rs");
            let wrapped = windowless_ready
                .then(|| std::fs::read_to_string(&main_path).ok())
                .flatten()
                .and_then(|source| crate::migrate::wrap_default_plugins(&source));
            match wrapped {
                Some(contents) => changes.push(ImportChange::WriteFile {
                    path: main_path,
                    contents,
                    replace: true,
                }),
                None => notes.push(crate::migrate::windowless_note()),
            }
        }
    }

    let jackdaw_toml = root.join("jackdaw.toml");
    if !jackdaw_toml.exists() {
        let workspace_package =
            (package_override.is_some() || package_dir != root).then_some(package.name.as_str());
        changes.push(ImportChange::WriteFile {
            path: jackdaw_toml,
            contents: jackdaw_toml_source(
                plugin.as_deref(),
                workspace_package,
                pinned_bevy.as_deref(),
                &selected_modules,
            ),
            replace: false,
        });
    }

    let jackdaw_dir = root.join(".jackdaw");
    if !jackdaw_dir.is_dir() {
        changes.push(ImportChange::CreateDirectory { path: jackdaw_dir });
    }

    let gitignore = root.join(".gitignore");
    let existing_gitignore = std::fs::read_to_string(&gitignore).unwrap_or_default();
    if !has_jackdaw_gitignore(&existing_gitignore) {
        let mut contents = existing_gitignore;
        if !contents.is_empty() && !contents.ends_with('\n') {
            contents.push('\n');
        }
        contents.push_str("/.jackdaw\n");
        changes.push(ImportChange::WriteFile {
            path: gitignore.clone(),
            contents,
            replace: gitignore.exists(),
        });
    }

    Ok(ImportPlan {
        root: root.to_path_buf(),
        package_dir,
        package_name: package.name,
        changes,
        notes,
        migrated_bin_target,
        detected_modules,
        modules: selected_modules,
    })
}

/// Apply a previously reviewed import plan.
pub fn apply_import_plan(plan: &ImportPlan) -> Result<ScaffoldReport, ScaffoldError> {
    let mut actions = Vec::with_capacity(plan.changes.len());
    for change in &plan.changes {
        match change {
            ImportChange::CreateDirectory { path } => {
                std::fs::create_dir_all(path)
                    .map_err(|e| ScaffoldError::Io(format!("{}: {e}", path.display())))?;
            }
            ImportChange::WriteFile { path, contents, .. } => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| ScaffoldError::Io(format!("{}: {e}", parent.display())))?;
                }
                std::fs::write(path, contents)
                    .map_err(|e| ScaffoldError::Io(format!("{}: {e}", path.display())))?;
            }
        }
        actions.push(change.summary());
    }
    Ok(ScaffoldReport {
        actions,
        created_lib_stub: plan.changes.iter().any(|change| {
            matches!(
                change,
                ImportChange::WriteFile {
                    path,
                    replace: false,
                    ..
                } if path.ends_with("src/lib.rs")
            )
        }) && !plan.migrated_bin_target,
    })
}

/// Compatibility helper for callers that have already presented their own
/// confirmation UI.
pub fn import_project(
    root: &Path,
    plugin_override: Option<String>,
) -> Result<ScaffoldReport, ScaffoldError> {
    let plan = plan_import_project(root, plugin_override)?;
    apply_import_plan(&plan)
}

/// The project's declared bevy dependency must be semver-compatible
/// with the supported minor. Projects without a direct bevy dependency
/// pass (an extension crate may express it indirectly); the build
/// surfaces any real mismatch later.
///
/// A workspace member inheriting `bevy = { workspace = true }` is
/// checked against `workspace`'s `[workspace.dependencies]`.
fn check_bevy_version(
    doc: &DocumentMut,
    workspace: Option<&DocumentMut>,
) -> Result<(), ScaffoldError> {
    let Some(req) = declared_bevy_req(doc, workspace) else {
        return Ok(());
    };
    if requirement_admits_supported_bevy(&req) {
        Ok(())
    } else {
        Err(ScaffoldError::BevyVersion { found: req })
    }
}

/// Whether a cargo version requirement can resolve to the Bevy minor
/// this editor supports.
///
/// A textual prefix test gets this wrong in both directions: it accepts
/// `0.190` (which starts with `0.19` but is a different minor) and
/// rejects a perfectly compatible range like `>=0.19, <0.21`. Parsing
/// the requirement and asking whether it matches a version in the
/// supported minor answers the question actually being asked.
fn requirement_admits_supported_bevy(req: &str) -> bool {
    let Ok(parsed) = req.parse::<semver::VersionReq>() else {
        // An unparseable requirement is cargo's problem to report, not
        // a reason to block an import.
        return true;
    };
    let Some((major, minor)) = SUPPORTED_BEVY_MINOR.split_once('.') else {
        return true;
    };
    let (Ok(major), Ok(minor)) = (major.parse::<u64>(), minor.parse::<u64>()) else {
        return true;
    };
    // Probe the ends of the supported minor: a requirement admits it if
    // it matches any patch in the range, and cargo requirements are
    // contiguous over patch.
    [0, u64::MAX]
        .iter()
        .any(|patch| parsed.matches(&semver::Version::new(major, minor, *patch)))
}

/// The bevy version requirement a manifest declares, if it states one
/// at all. `None` covers no bevy dependency and path/git dependencies,
/// which express no version for us to compare.
fn declared_bevy_req(doc: &DocumentMut, workspace: Option<&DocumentMut>) -> Option<String> {
    let bevy = doc.get("dependencies")?.get("bevy")?;
    if bevy
        .get("workspace")
        .and_then(toml_edit::Item::as_bool)
        .unwrap_or(false)
    {
        let inherited = workspace?
            .get("workspace")?
            .get("dependencies")?
            .get("bevy")?;
        return version_req_of(inherited);
    }
    version_req_of(bevy)
}

/// The `version` a dependency entry states, in either the plain-string
/// or table form.
fn version_req_of(item: &toml_edit::Item) -> Option<String> {
    item.as_str()
        .map(str::to_string)
        .or_else(|| {
            item.get("version")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .filter(|req| !req.is_empty())
}

/// An edit to a game's Cargo manifest, with what it does in words.
struct ManifestEdit {
    contents: String,
    notes: Vec<String>,
}

/// `manifest` with `jackdaw_runtime` declared, or with `pie` and the
/// features of `modules` added to an existing plain declaration, plus the
/// crates `modules` add. `None` when nothing is missing or the declaration
/// is one this edit does not touch. A member of a workspace that declares
/// the runtime inherits it from there.
///
/// Edited in place so the user's formatting, comments and key order
/// survive. A `{ workspace = true }` entry gets the features on the
/// member's side, which cargo unions with whatever the workspace declares.
fn runtime_dependency_edit(
    manifest: &str,
    wiring: &RuntimeWiring,
    modules: &ModuleSet,
    game: &GameDependencies,
) -> Option<ManifestEdit> {
    let mut doc: DocumentMut = manifest.parse().ok()?;
    let mut notes = Vec::new();
    let module_features = module_runtime_features(modules);
    match wiring.dependency {
        RuntimeDependency::Missing => {
            let mut features = module_features;
            features.push(PIE_FEATURE.to_string());
            let source = if wiring.workspace_declares {
                "workspace = true".to_string()
            } else {
                jackdaw_dep_requirement(RUNTIME_CRATE)
            };
            let quoted = features
                .iter()
                .map(|feature| format!("\"{feature}\""))
                .collect::<Vec<_>>()
                .join(", ");
            insert_dependency(
                &mut doc,
                RUNTIME_CRATE,
                &format!("{{ {source}, features = [{quoted}] }}"),
            )?;
            notes.push(format!(
                "Cargo.toml: adds jackdaw_runtime (features {}), which component discovery \
                 and Play need",
                features.join(", ")
            ));
        }
        RuntimeDependency::Plain => {
            let mut wanted = Vec::new();
            if !wiring.pie_available() {
                wanted.push(PIE_FEATURE.to_string());
            }
            wanted.extend(
                module_features
                    .into_iter()
                    .filter(|feature| !game.runtime_features.contains(feature)),
            );
            if !wanted.is_empty() {
                add_runtime_features(&mut doc, &wanted)?;
                notes.push(match wanted.as_slice() {
                    [only] if only == PIE_FEATURE => {
                        "Cargo.toml: enables the `pie` feature of jackdaw_runtime, which Play needs"
                            .to_string()
                    }
                    _ => format!(
                        "Cargo.toml: enables jackdaw_runtime features {}",
                        wanted.join(", ")
                    ),
                });
            }
        }
        RuntimeDependency::Unsupported(_) => return None,
    }
    notes.extend(removed_feature_notes(&drop_removed_runtime_features(
        &mut doc,
    )));
    notes.extend(add_module_crates(&mut doc, game, modules)?);
    (doc.to_string() != manifest).then(|| ManifestEdit {
        contents: doc.to_string(),
        notes,
    })
}

/// The runtime features and crate names `doc` declares itself, without
/// anything a workspace adds.
fn declared_dependencies(doc: &DocumentMut) -> GameDependencies {
    let mut game = GameDependencies::default();
    let Some(deps) = doc
        .get("dependencies")
        .and_then(toml_edit::Item::as_table_like)
    else {
        return game;
    };
    for (key, entry) in deps.iter() {
        let name = entry
            .get("package")
            .and_then(toml_edit::Item::as_str)
            .unwrap_or(key);
        game.crates.insert(name.to_string());
        if name == RUNTIME_CRATE {
            let features = entry.get("features").and_then(toml_edit::Item::as_array);
            for feature in features.into_iter().flatten().filter_map(|f| f.as_str()) {
                game.runtime_features.insert(feature.to_string());
            }
        }
    }
    game
}

/// Insert `name = <value>` into `[dependencies]`, `value` being TOML.
fn insert_dependency(doc: &mut DocumentMut, name: &str, value: &str) -> Option<()> {
    let parsed: DocumentMut = format!("entry = {value}").parse().ok()?;
    let mut value = parsed.get("entry")?.as_value()?.clone();
    value.decor_mut().clear();
    doc.entry("dependencies")
        .or_insert_with(toml_edit::table)
        .as_table_like_mut()?
        .insert(name, toml_edit::Item::Value(value));
    Some(())
}

/// The `jackdaw_runtime` entry of `doc` as a table, turning a bare version
/// string into `{ version = ... }` first.
fn runtime_entry_table(doc: &mut DocumentMut) -> Option<&mut toml_edit::Item> {
    let entry = doc
        .get_mut("dependencies")?
        .as_table_like_mut()?
        .get_mut(RUNTIME_CRATE)?;
    if let Some(value) = entry.as_value()
        && let Some(version) = value.as_str()
    {
        let decor = value.decor().clone();
        let mut table = toml_edit::InlineTable::new();
        table.insert("version", version.into());
        let mut replacement = toml_edit::Value::from(table);
        *replacement.decor_mut() = decor;
        *entry = toml_edit::Item::Value(replacement);
    }
    Some(entry)
}

/// Add `features` to the `jackdaw_runtime` entry of `doc`.
fn add_runtime_features(doc: &mut DocumentMut, features: &[String]) -> Option<()> {
    let entry = runtime_entry_table(doc)?;
    let table = entry.as_table_like_mut()?;
    match table
        .get_mut("features")
        .and_then(toml_edit::Item::as_array_mut)
    {
        Some(array) => {
            for feature in features {
                array.push(feature.as_str());
            }
        }
        None => {
            let mut array = toml_edit::Array::new();
            for feature in features {
                array.push(feature.as_str());
            }
            table.insert("features", toml_edit::value(array));
        }
    }
    if let Some(inline) = entry.as_inline_table_mut() {
        inline.fmt();
    }
    Some(())
}

/// Take `features` off the `jackdaw_runtime` entry of `doc`.
fn remove_runtime_features(doc: &mut DocumentMut, features: &[String]) -> Option<()> {
    let entry = runtime_entry_table(doc)?;
    if let Some(array) = entry
        .as_table_like_mut()?
        .get_mut("features")
        .and_then(toml_edit::Item::as_array_mut)
    {
        array.retain(|value| {
            value
                .as_str()
                .is_none_or(|f| !features.iter().any(|x| x == f))
        });
        array.fmt();
    }
    Some(())
}

/// Take the `jackdaw_runtime` features that no longer exist off the entries
/// in `doc`'s `[dependencies]` and `[workspace.dependencies]`. Returns the
/// features taken off.
fn drop_removed_runtime_features(doc: &mut DocumentMut) -> Vec<String> {
    let catalog = ModuleCatalog::builtin();
    let mut dropped = Vec::new();
    let mut drop_from = |deps: Option<&mut toml_edit::Item>| {
        let Some(array) = deps
            .and_then(toml_edit::Item::as_table_like_mut)
            .and_then(|deps| deps.get_mut(RUNTIME_CRATE))
            .and_then(toml_edit::Item::as_table_like_mut)
            .and_then(|entry| entry.get_mut("features"))
            .and_then(toml_edit::Item::as_array_mut)
        else {
            return;
        };
        array.retain(|value| match value.as_str() {
            Some(feature) if catalog.replacing_feature(feature).is_some() => {
                dropped.push(feature.to_string());
                false
            }
            _ => true,
        });
        array.fmt();
    };
    drop_from(doc.get_mut("dependencies"));
    drop_from(
        doc.get_mut("workspace")
            .and_then(|workspace| workspace.get_mut("dependencies")),
    );
    dropped.sort();
    dropped.dedup();
    dropped
}

/// A note per removed `jackdaw_runtime` feature in `features`, naming the
/// module recorded in its place.
fn removed_feature_notes(features: &[String]) -> Vec<String> {
    let catalog = ModuleCatalog::builtin();
    features
        .iter()
        .filter_map(|feature| {
            let module = catalog.replacing_feature(feature)?;
            let mut note = format!(
                "Cargo.toml: drops the `{feature}` feature of jackdaw_runtime, which no longer \
                 exists; jackdaw.toml records the {} module in its place",
                module.name
            );
            if let Some(reason) = &module.unavailable {
                note.push_str(&format!(
                    ". {} is not compatible with this editor line yet: {reason}",
                    module.name
                ));
            }
            Some(note)
        })
        .collect()
}

/// Add the crates of `modules` that `game` does not declare yet, returning
/// a note per crate.
fn add_module_crates(
    doc: &mut DocumentMut,
    game: &GameDependencies,
    modules: &ModuleSet,
) -> Option<Vec<String>> {
    let mut notes = Vec::new();
    for module in enabled_modules(modules) {
        if let Some(reason) = &module.unavailable {
            notes.push(format!(
                "{} is not compatible with this editor line, so none of its crates were added: {reason}.",
                module.name
            ));
            continue;
        }
        for krate in &module.crates {
            if game.crates.contains(&krate.name) {
                continue;
            }
            let requirement = match krate.requirement() {
                Some(requirement) => requirement,
                None => jackdaw_crate_like_runtime(doc, &krate.name)
                    .unwrap_or_else(|| module_crate_requirement(krate)),
            };
            insert_dependency(doc, &krate.name, &requirement)?;
            notes.push(format!(
                "Cargo.toml: adds {} for the {} module",
                krate.name, module.name
            ));
        }
    }
    Some(notes)
}

/// What the game's code still has to do for `added` modules: add each
/// module plugin whose crate no source file names yet.
fn plugin_notes(package_dir: &Path, added: &ModuleSet) -> Vec<String> {
    let sources: Vec<String> =
        jackdaw_project_build::runtime_wiring::source_files(&package_dir.join("src"))
            .iter()
            .filter_map(|path| std::fs::read_to_string(path).ok())
            .collect();
    enabled_modules(added)
        .flat_map(|module| &module.crates)
        .filter_map(|krate| {
            let plugin = krate.plugin.as_ref()?;
            let ident = krate.name.replace('-', "_");
            let named = sources
                .iter()
                .any(|text| jackdaw_project_build::runtime_wiring::mentions_ident(text, &ident));
            (!named).then(|| format!("add `{plugin}` to the game's App"))
        })
        .collect()
}

/// How `doc` should ask for the jackdaw crate `name`: from the same place
/// its `jackdaw_runtime` comes from, so the two stay one build. `None` when
/// the runtime entry gives nothing to copy.
fn jackdaw_crate_like_runtime(doc: &DocumentMut, name: &str) -> Option<String> {
    let entry = doc.get("dependencies")?.get(RUNTIME_CRATE)?;
    if let Some(version) = entry.as_str() {
        return Some(format!("{{ version = \"{version}\" }}"));
    }
    let table = entry.as_table_like()?;
    let text = |key: &str| table.get(key).and_then(toml_edit::Item::as_str);
    if let Some(path) = text("path") {
        let sibling = path
            .trim_end_matches(['/', '\\'])
            .strip_suffix(RUNTIME_CRATE)
            .map(|parent| format!("{parent}{name}"))?;
        return Some(format!("{{ path = '{sibling}' }}"));
    }
    if let Some(git) = text("git") {
        let pin = ["rev", "tag", "branch"]
            .into_iter()
            .find_map(|key| text(key).map(|value| format!(", {key} = \"{value}\"")))
            .unwrap_or_default();
        return Some(format!("{{ git = \"{git}\"{pin} }}"));
    }
    text("version").map(|version| format!("{{ version = \"{version}\" }}"))
}

/// A set-up project's game package and the modules it uses.
struct SetUpGame {
    settings_path: PathBuf,
    package: jackdaw_project_build::cargo_meta::PackageInfo,
    game: GameDependencies,
    modules: ModuleSet,
}

fn set_up_game(root: &Path) -> Result<SetUpGame, ScaffoldError> {
    use jackdaw_project_build::project_manifest::{self, ProjectManifest};

    if !ProjectManifest::exists(root) {
        return Err(ScaffoldError::NotSetUp(root.to_path_buf()));
    }
    let settings_path = project_manifest::path(root);
    let manifest =
        ProjectManifest::read_checked(root).map_err(|detail| ScaffoldError::SettingsParse {
            path: settings_path.clone(),
            detail,
        })?;
    let package = jackdaw_project_build::cargo_meta::resolve_project_package(
        root,
        manifest.package.as_deref(),
    )
    .map_err(ScaffoldError::Package)?;
    let game = GameDependencies::read(&package.dir, root);
    let modules = match &manifest.modules {
        Some(_) => manifest.modules(root),
        None => ModuleCatalog::builtin().detect(&game),
    };
    Ok(SetUpGame {
        settings_path,
        package,
        game,
        modules,
    })
}

/// The modules the set-up project at `root` uses: its `[modules]` table,
/// or what its game package already declares when there is none.
pub fn project_modules(root: &Path) -> Result<ModuleSet, ScaffoldError> {
    set_up_game(root).map(|game| game.modules)
}

/// Plan switching the set-up project at `root` to `modules`: the
/// `[modules]` table in `jackdaw.toml`, and the runtime features and crates
/// in the game's Cargo.toml. Requirements are added. Side-effect free, like
/// the import plan; apply it with [`apply_import_plan`].
///
/// Turning a module off takes its runtime features away but leaves its
/// crates, which the game's own code may still use.
pub fn plan_module_change(root: &Path, modules: &ModuleSet) -> Result<ImportPlan, ScaffoldError> {
    let target = ModuleCatalog::builtin().complete(modules.clone());
    let SetUpGame {
        settings_path,
        package,
        game,
        modules: current,
    } = set_up_game(root)?;
    let mut changes = Vec::new();
    let mut notes = Vec::new();

    let cargo_path = package.dir.join("Cargo.toml");
    let cargo_text = std::fs::read_to_string(&cargo_path)
        .map_err(|e| ScaffoldError::Io(format!("{}: {e}", cargo_path.display())))?;
    let mut doc: DocumentMut = cargo_text
        .parse()
        .map_err(|e| ScaffoldError::ManifestParse(format!("{e}")))?;
    let declared = declared_dependencies(&doc);
    let wanted = module_runtime_features(&target);
    let dropped: ModuleSet = current.difference(&target).cloned().collect();
    let unwanted: Vec<String> = module_runtime_features(&dropped)
        .into_iter()
        .filter(|feature| !wanted.contains(feature))
        .collect();
    let wiring = RuntimeWiring::inspect(&package.dir);
    if wiring.dependency == RuntimeDependency::Plain {
        let add: Vec<String> = wanted
            .iter()
            .filter(|feature| !game.runtime_features.contains(*feature))
            .cloned()
            .collect();
        if !add.is_empty() {
            add_runtime_features(&mut doc, &add)
                .ok_or_else(|| ScaffoldError::ManifestParse("jackdaw_runtime".to_string()))?;
            notes.push(format!(
                "Cargo.toml: enables jackdaw_runtime features {}",
                add.join(", ")
            ));
        }
        let remove: Vec<String> = unwanted
            .iter()
            .filter(|feature| declared.runtime_features.contains(*feature))
            .cloned()
            .collect();
        if !remove.is_empty() {
            remove_runtime_features(&mut doc, &remove)
                .ok_or_else(|| ScaffoldError::ManifestParse("jackdaw_runtime".to_string()))?;
            notes.push(format!(
                "Cargo.toml: turns off jackdaw_runtime features {}",
                remove.join(", ")
            ));
        }
        let inherited: Vec<&String> = unwanted
            .iter()
            .filter(|feature| {
                game.runtime_features.contains(*feature)
                    && !declared.runtime_features.contains(*feature)
            })
            .collect();
        if !inherited.is_empty() {
            notes.push(format!(
                "the workspace's jackdaw_runtime entry still enables {}; take it off there",
                inherited
                    .iter()
                    .map(|feature| feature.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    } else if !wanted.is_empty() || !unwanted.is_empty() {
        notes.push(
            "Cargo.toml does not declare jackdaw_runtime as a plain dependency, so its features \
             are left for you to change"
                .to_string(),
        );
    }
    notes.extend(
        add_module_crates(&mut doc, &game, &target)
            .ok_or_else(|| ScaffoldError::ManifestParse("[dependencies]".to_string()))?,
    );
    let added: ModuleSet = target.difference(&current).cloned().collect();
    notes.extend(plugin_notes(&package.dir, &added));
    for module in enabled_modules(&dropped) {
        for krate in &module.crates {
            if declared.crates.contains(&krate.name) {
                notes.push(format!(
                    "{} stays in Cargo.toml; remove it and its plugin if the game no longer uses it",
                    krate.name
                ));
            }
        }
    }
    let cargo_changed = doc.to_string() != cargo_text;
    if cargo_changed {
        changes.push(ImportChange::WriteFile {
            path: cargo_path,
            contents: doc.to_string(),
            replace: true,
        });
        notes.push("the game rebuilds with these changes the next time it is built".to_string());
    }

    let settings_text = std::fs::read_to_string(&settings_path)
        .map_err(|e| ScaffoldError::Io(format!("{}: {e}", settings_path.display())))?;
    let settings = jackdaw_project_build::modules::with_modules(&settings_text, &target).map_err(
        |detail| ScaffoldError::SettingsParse {
            path: settings_path.clone(),
            detail,
        },
    )?;
    if settings != settings_text {
        changes.push(ImportChange::WriteFile {
            path: settings_path,
            contents: settings,
            replace: true,
        });
    }

    Ok(ImportPlan {
        root: root.to_path_buf(),
        package_dir: package.dir,
        package_name: package.name,
        changes,
        notes,
        migrated_bin_target: false,
        detected_modules: current,
        modules: target,
    })
}

/// The library file declaring `impl Plugin for <plugin>`, rewritten so its
/// `build` adds `JackdawPlugin`. `None` unless exactly one library file
/// declares exactly one such impl with a `build` that can be edited.
fn runtime_plugin_edit(package_dir: &Path, plugin: &str) -> Option<(PathBuf, String)> {
    let name = plugin.rsplit("::").next()?;
    let src = package_dir.join("src");
    let mut declaring = jackdaw_project_build::runtime_wiring::source_files(&src)
        .into_iter()
        .filter(|path| path != &src.join("main.rs") && !path.starts_with(src.join("bin")))
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).ok()?;
            let count = crate::migrate::plugin_impl_count(&text, name);
            (count > 0).then_some((path, text, count))
        });
    let (path, text, count) = declaring.next()?;
    if count != 1 || declaring.next().is_some() {
        return None;
    }
    let contents = crate::migrate::add_runtime_plugin(&text, name)?;
    Some((path, contents))
}

fn has_jackdaw_gitignore(contents: &str) -> bool {
    contents
        .lines()
        .any(|l| l.trim() == "/.jackdaw" || l.trim() == ".jackdaw" || l.trim() == ".jackdaw/")
}

/// The library stub for a bin-only project. `with_runtime` adds
/// `JackdawPlugin` from the plugin, for a project that depends on the
/// runtime.
fn lib_stub_source(with_runtime: bool) -> String {
    let build = if with_runtime {
        "fn build(&self, app: &mut App) {\n        app.add_plugins(jackdaw_runtime::JackdawPlugin);\n    }"
    } else {
        "fn build(&self, _app: &mut App) {}"
    };
    LIB_STUB.replace("{build}", build)
}

const LIB_STUB: &str = r#"//! Game library: put gameplay here and add [`GamePlugin`] from
//! `main.rs` so `cargo run` and the editor's Play button share it.
//! Move your game setup (systems, resources, observers) in here
//! from main.rs.
//!
//! # Adding components the editor can see
//!
//! Write components anywhere in this library (any module, not
//! `main.rs`), deriving `Component, Reflect, Default` with
//! `#[reflect(Component, Default)]`. After you save, click Rebuild in
//! jackdaw (or run `jd build`) and they appear in
//! `Add Component`. No registration code is needed; Bevy's
//! `reflect_auto_register` picks up the `Reflect` derive.

use bevy::prelude::*;

#[derive(Default)]
pub struct GamePlugin;

impl Plugin for GamePlugin {
    {build}
}

// Example: an editable component. Uncomment, save, and Rebuild to see
// `Health` in the Components panel's Add Component list.
//
// #[derive(Component, Reflect, Default)]
// #[reflect(Component, Default)]
// pub struct Health {
//     pub max: f32,
//     pub current: f32,
// }
"#;

/// The `major.minor` a version requirement names, for reporting and for
/// recording what a project targets. `None` when the requirement states
/// no usable pair.
fn bevy_minor_of(req: &str) -> Option<String> {
    let head = req
        .trim_start_matches(['^', '=', '~', '>', '<'])
        .split(',')
        .next()?
        .trim();
    let mut parts = head.split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    let usable = !major.is_empty()
        && !minor.is_empty()
        && major.chars().all(|c| c.is_ascii_digit())
        && minor.chars().all(|c| c.is_ascii_digit());
    usable.then(|| format!("{major}.{minor}"))
}

fn jackdaw_toml_source(
    plugin: Option<&str>,
    workspace_package: Option<&str>,
    pinned_bevy: Option<&str>,
    modules: &ModuleSet,
) -> String {
    let package = match workspace_package {
        Some(name) => {
            format!("\n# The workspace member jackdaw builds as the game.\npackage = \"{name}\"\n")
        }
        None => String::new(),
    };
    // With no plugin to name, the key stays commented so doctor does
    // not fail on a type the crate does not declare.
    let plugin = match plugin {
        Some(name) => {
            format!(
                "# Optional name of your game's root Bevy Plugin (for setup / `jd doctor`).\n\
                 plugin = \"{name}\"\n"
            )
        }
        None => "# Optional name of your game's root Bevy Plugin (for setup / `jd doctor`).\n\
                 # Play runs your cargo binary; add the plugin from `main.rs`.\n\
                 # plugin = \"GamePlugin\"\n"
            .to_string(),
    };
    format!(
        "# jackdaw project settings.\n\
         \n\
         {plugin}\
         {package}\n\
         {pins}\n\
         {modules}\n\
         [[run]]\n\
         name = \"Play\"\n",
        pins = match pinned_bevy {
            Some(bevy) => jackdaw_project_build::project_manifest::pins_toml_for_bevy(bevy),
            None => jackdaw_project_build::project_manifest::pins_toml(),
        },
        modules = jackdaw_project_build::modules::modules_toml(modules),
    )
}

/// Recursively gather every embedded file under `dir`.
fn collect_template_files<'a>(dir: &'a Dir<'a>, out: &mut Vec<&'a include_dir::File<'a>>) {
    for entry in dir.entries() {
        match entry {
            include_dir::DirEntry::File(f) => out.push(f),
            include_dir::DirEntry::Dir(d) => collect_template_files(d, out),
        }
    }
}

/// Replace the template's placeholders. Matches the exact forms the
/// templates use; the longer `title_case` form is replaced before the
/// bare one.
fn substitute_placeholders(
    text: &str,
    project_name: &str,
    crate_name: &str,
    authors: &str,
    title: &str,
    modules: &ModuleSet,
) -> String {
    let text = replace_line(text, "{{module_crates}}", &module_crate_lines(modules));
    let text = replace_line(&text, "{{module_plugins}}", &module_plugin_lines(modules));
    text.replace("{{project-name | title_case}}", title)
        .replace(
            "{{jackdaw_modules}}",
            jackdaw_project_build::modules::modules_toml(modules).trim_end(),
        )
        .replace(
            "{{jackdaw_runtime_features}}",
            &template_runtime_features(modules),
        )
        .replace("{{crate_name}}", crate_name)
        .replace("{{authors}}", authors)
        .replace("{{project-name}}", project_name)
        .replace(
            "{{jackdaw_pins}}",
            jackdaw_project_build::project_manifest::pins_toml().trim_end(),
        )
        .replace("{{bevy_version}}", jackdaw_project_build::BEVY_VERSION)
        .replace(
            "{{jackdaw_version}}",
            &jackdaw_project_build::jackdaw_requirement(),
        )
        .replace(
            "{{jackdaw_runtime_dep}}",
            &jackdaw_dep_requirement("jackdaw_runtime"),
        )
        .replace(
            "{{jackdaw_extension_dep}}",
            &jackdaw_dep_requirement("jackdaw_extension"),
        )
}

/// Replace each line holding only `placeholder` with `lines`, whichever
/// line ending the template was checked out with.
fn replace_line(text: &str, placeholder: &str, lines: &str) -> String {
    text.replace(&format!("{placeholder}\r\n"), lines)
        .replace(&format!("{placeholder}\n"), lines)
}

/// The runtime features a new game enables: its modules' features, then
/// terrain drawing and the editor link.
fn template_runtime_features(modules: &ModuleSet) -> String {
    let mut features = module_runtime_features(modules);
    for always in ["terrain", PIE_FEATURE] {
        if !features.iter().any(|feature| feature == always) {
            features.push(always.to_string());
        }
    }
    features
        .iter()
        .map(|feature| format!("\"{feature}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The `jackdaw_runtime` features `modules` enable, in catalog order.
fn module_runtime_features(modules: &ModuleSet) -> Vec<String> {
    let mut features: Vec<String> = Vec::new();
    for module in enabled_modules(modules) {
        for feature in &module.runtime_features {
            if !features.contains(feature) {
                features.push(feature.clone());
            }
        }
    }
    features
}

/// The catalog entries of `modules`, in catalog order.
fn enabled_modules(
    modules: &ModuleSet,
) -> impl Iterator<Item = &'static jackdaw_project_build::modules::ModuleInfo> {
    ModuleCatalog::builtin()
        .modules()
        .iter()
        .filter(|module| modules.contains(&module.id))
}

/// How a game asks for a module's crate: its registry or git source, or for a jackdaw
/// crate the source the runtime comes from.
fn module_crate_requirement(krate: &jackdaw_project_build::modules::ModuleCrate) -> String {
    krate
        .requirement()
        .unwrap_or_else(|| format!("{{ {} }}", jackdaw_dep_requirement(&krate.name)))
}

/// The `[dependencies]` lines for the crates `modules` add.
fn module_crate_lines(modules: &ModuleSet) -> String {
    enabled_modules(modules)
        .flat_map(|module| &module.crates)
        .map(|krate| format!("{} = {}\n", krate.name, module_crate_requirement(krate)))
        .collect()
}

/// The `.add_plugins(...)` lines a new game's `main` adds for `modules`.
fn module_plugin_lines(modules: &ModuleSet) -> String {
    enabled_modules(modules)
        .flat_map(|module| &module.crates)
        .filter_map(|krate| krate.plugin.as_ref())
        .map(|plugin| format!("        .add_plugins({plugin})\n"))
        .collect()
}

/// How a scaffolded project asks for one of the public jackdaw crates: the
/// registry line for a released editor, the exact revision for one built from
/// git, the workspace crate for one built from a checkout. A version
/// requirement alone would not resolve for a git-installed editor.
fn jackdaw_dep_requirement(crate_name: &str) -> String {
    jackdaw_project_build::build_source::build_source().dep_requirement(crate_name)
}

/// Title-case a kebab/snake name: `my-cool-game` becomes `My Cool Game`.
fn title_case(name: &str) -> String {
    name.split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `name <email>` from git config, or an empty string when unavailable.
fn git_authors() -> String {
    let field = |key: &str| {
        jackdaw_env::without_console_window(&mut std::process::Command::new("git"))
            .args(["config", key])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    match (field("user.name"), field("user.email")) {
        (Some(name), Some(email)) => format!("{name} <{email}>"),
        (Some(name), None) => name,
        _ => String::new(),
    }
}

/// A usable crate/dir name from arbitrary input: lowercased, spaces
/// and underscores to dashes, everything else alphanumeric. Runs of
/// separators collapse, so `My  Cool_Game!` becomes `my-cool-game`.
pub fn sanitize_project_name(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.trim().chars() {
        match c {
            ' ' | '_' | '-' if !out.ends_with('-') => out.push('-'),
            ' ' | '_' | '-' => {}
            c if c.is_ascii_alphanumeric() => out.push(c.to_ascii_lowercase()),
            _ => {}
        }
    }
    out.trim_matches('-').to_string()
}

/// Rust keywords a crate name cannot be: the templates paste the crate
/// name straight into `use`/path position, so one of these would not
/// compile.
const RESERVED_CRATE_NAMES: [&str; 55] = [
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "try", "typeof", "unsized", "virtual", "yield", "gen", "test", "main",
    "std",
];

/// Sanitize `raw` and confirm the result is usable as both a directory
/// and a Rust crate name. The two scaffolding entry points (the CLI and
/// the launcher modal) share this so they reject the same inputs with
/// the same message.
pub fn validated_project_name(raw: &str) -> Result<String, ScaffoldError> {
    let name = sanitize_project_name(raw);
    let reject = |reason| {
        Err(ScaffoldError::ProjectName {
            raw: raw.to_string(),
            reason,
        })
    };
    if name.is_empty() {
        // A name of entirely non-ASCII characters sanitizes to nothing.
        // Cargo package names are ASCII, so say that rather than
        // implying the input was empty.
        return if raw.trim().is_empty() {
            reject("enter a name")
        } else {
            reject("cargo package names use ASCII letters, digits, and dashes")
        };
    }
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        return reject("a crate name cannot start with a digit");
    }
    let crate_name = name.replace('-', "_");
    if RESERVED_CRATE_NAMES.contains(&crate_name.as_str()) {
        return reject("that is a reserved Rust name");
    }
    Ok(name)
}

// `detect_extension` / `detect_plugin` moved to the bevy-light build
// pipeline crate (they are pure source scanning the shim builder needs);
// re-exported here at their historical path.
pub(crate) use jackdaw_project_build::detect::detect_plugin;

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("jackdaw_scaffold_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_project_inside_a_workspace_that_stops_resolving_is_warned_about() {
        let outer = temp_dir("broken_workspace");
        std::fs::write(outer.join("Cargo.toml"), "[workspace]\nmembers = [\"*\"]\n").unwrap();
        let dest = outer.join("games").join("my-game");
        std::fs::create_dir_all(&dest).unwrap();

        let warnings = check_new_project_with(&dest, |root| {
            if root == outer {
                Err("error: multiple workspace roots found".to_string())
            } else {
                Ok(())
            }
        });

        assert_eq!(
            warnings,
            vec![NewProjectWarning::EnclosingWorkspaceBroken {
                project: dest.clone(),
                workspace: outer.clone(),
                detail: "error: multiple workspace roots found".to_string(),
            }]
        );
        let fix = warnings[0].fix();
        assert!(fix.contains("exclude = [\"games/my-game\"]"), "got:\n{fix}");
        assert!(!warnings[0].summary().contains('`'));
        let _ = std::fs::remove_dir_all(&outer);
    }

    #[test]
    fn a_project_in_a_clean_location_has_no_warnings() {
        let location = temp_dir("clean_location");
        let dest = location.join("my-game");
        std::fs::create_dir_all(&dest).unwrap();

        assert!(enclosing_workspace(&dest).is_none());
        assert!(check_new_project_with(&dest, |_| Ok(())).is_empty());
        let _ = std::fs::remove_dir_all(&location);
    }

    #[test]
    fn a_project_whose_dependencies_do_not_resolve_is_warned_about() {
        let location = temp_dir("unresolved");
        let dest = location.join("my-game");
        std::fs::create_dir_all(&dest).unwrap();

        let warnings = check_new_project_with(&dest, |_| {
            Err("error: failed to select a version for `jackdaw_runtime`".to_string())
        });

        assert!(
            matches!(
                warnings.as_slice(),
                [NewProjectWarning::DependenciesUnresolved { project, .. }] if *project == dest
            ),
            "got: {warnings:?}"
        );
        assert!(
            warnings[0]
                .summary()
                .contains(&jackdaw_source_description()),
            "the summary names where the crates were looked for: {}",
            warnings[0].summary()
        );
        assert_eq!(
            warnings[0].cargo_error(),
            "error: failed to select a version for `jackdaw_runtime`"
        );
        let _ = std::fs::remove_dir_all(&location);
    }

    #[test]
    fn upgrade_pins_a_prerelease_editor_to_its_exact_version() {
        let requirement = jackdaw_project_build::jackdaw_requirement_for("0.19.0-rc.1");
        let cargo = "[dependencies]\njackdaw_runtime = \"0.19\"\n\
                     jackdaw_extension = { version = \"0.19\", features = [\"physics\"] }\n";
        let out = bump_jackdaw_requirements(cargo, &requirement).expect("changed");
        assert!(
            out.contains("jackdaw_runtime = \"=0.19.0-rc.1\""),
            "got:\n{out}"
        );
        assert!(out.contains("version = \"=0.19.0-rc.1\""), "got:\n{out}");

        let stable = jackdaw_project_build::jackdaw_requirement_for("0.19.0");
        let back = bump_jackdaw_requirements(&out, &stable).expect("changed");
        assert_eq!(back.matches("\"0.19\"").count(), 2, "got:\n{back}");
    }

    #[test]
    fn new_game_project_has_the_contract_files() {
        let dest = temp_dir("newgame").join("my-game");
        scaffold_new_project(&dest, "my-game", TemplateKind::Game).unwrap();
        assert!(dest.join("Cargo.toml").is_file());
        assert!(dest.join("src/lib.rs").is_file());
        assert!(dest.join("src/main.rs").is_file());
        assert!(dest.join("assets/scene.bsn").is_file());
        assert!(dest.join("jackdaw.toml").is_file());
        assert!(dest.join(".gitignore").is_file());
        let manifest = std::fs::read_to_string(dest.join("Cargo.toml")).unwrap();
        assert!(manifest.contains("name = \"my-game\""));
        assert!(!manifest.contains("crate-type"));
        assert!(!manifest.contains("[features]"));
        let lib = std::fs::read_to_string(dest.join("src/lib.rs")).unwrap();
        assert!(lib.contains("pub struct GamePlugin"));
        assert!(!lib.contains("{{"));
    }

    /// The templates state the whole jackdaw requirement rather than a
    /// version, because a version is only resolvable for a build that was
    /// released. Whichever form this build calls for is what a scaffolded
    /// manifest must end up carrying, and nothing may be left unfilled.
    #[test]
    fn the_templates_state_the_requirement_this_build_can_resolve() {
        for (path, dep) in [
            ("templates/game/Cargo.toml.template", "jackdaw_runtime"),
            (
                "templates/extension/Cargo.toml.template",
                "jackdaw_extension",
            ),
        ] {
            let template =
                std::fs::read_to_string(format!("{}/{path}", env!("CARGO_MANIFEST_DIR")))
                    .unwrap_or_else(|e| panic!("read {path}: {e}"));
            let filled = substitute_placeholders(
                &template,
                "my-game",
                "my_game",
                "",
                "My Game",
                &ModuleCatalog::builtin().defaults(),
            );
            assert!(
                !filled.contains("{{"),
                "{path}: every placeholder is filled, got: {filled}"
            );
            let line = filled
                .lines()
                .find(|line| line.trim_start().starts_with(dep))
                .unwrap_or_else(|| panic!("{path}: no {dep} dependency"));
            assert!(
                line.contains(&jackdaw_dep_requirement(dep)),
                "{path}: {dep} must state what this build can resolve, got: {line}"
            );
        }
    }

    #[test]
    fn new_extension_project_uses_the_api() {
        let dest = temp_dir("newext").join("my-ext");
        scaffold_new_project(&dest, "my-ext", TemplateKind::Extension).unwrap();
        let manifest = std::fs::read_to_string(dest.join("Cargo.toml")).unwrap();
        assert!(manifest.contains("jackdaw_extension"));
        let lib = std::fs::read_to_string(dest.join("src/lib.rs")).unwrap();
        assert!(lib.contains("JackdawExtension"));
    }

    #[test]
    fn import_adds_the_runtime_alongside_its_own_files() {
        let root = temp_dir("import");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"their-game\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nbevy = \"0.20\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "pub struct TheirPlugin;\nimpl Plugin for TheirPlugin {}\n",
        )
        .unwrap();
        let report = import_project(&root, None).unwrap();
        assert!(!report.created_lib_stub);
        let cargo = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("bevy = \"0.20\""));
        assert!(cargo.contains("jackdaw_runtime"));
        let toml = std::fs::read_to_string(root.join("jackdaw.toml")).unwrap();
        assert!(toml.contains("plugin = \"TheirPlugin\""));
        assert!(root.join(".jackdaw").is_dir());
        let gitignore = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(gitignore.contains("/.jackdaw"));
    }

    #[test]
    fn import_offers_a_stub_for_bin_only_projects() {
        let root = temp_dir("stub");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"bin-only\"\nversion = \"0.1.0\"\n\n[dependencies]\nbevy = \"0.20\"\n",
        )
        .unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();

        let report = import_project(&root, None).unwrap();
        assert!(report.created_lib_stub);
        let lib = std::fs::read_to_string(root.join("src/lib.rs")).unwrap();
        assert!(lib.contains("pub struct GamePlugin"));
    }

    #[test]
    fn version_requirements_are_compared_not_prefix_matched() {
        // Compatible with 0.20 one way or another.
        for req in [
            "0.20",
            "0.20.2",
            "^0.20",
            "=0.20.0",
            "~0.20",
            ">=0.20, <0.22",
        ] {
            assert!(
                requirement_admits_supported_bevy(req),
                "`{req}` should be accepted"
            );
        }
        // `0.200` is a different minor that merely shares a prefix.
        for req in ["0.16", "0.200", "0.19", "0.21.0-rc.1", "1.0"] {
            assert!(
                !requirement_admits_supported_bevy(req),
                "`{req}` should be rejected"
            );
        }
        // Cargo owns reporting a malformed requirement; import does not
        // block on one.
        assert!(requirement_admits_supported_bevy("not a version"));
    }

    #[test]
    fn import_rejects_a_bevy_minor_mismatch() {
        let root = temp_dir("mismatch");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"old\"\nversion = \"0.1.0\"\n\n[dependencies]\nbevy = \"0.16\"\n",
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub struct GamePlugin;\n").unwrap();
        assert!(matches!(
            import_project(&root, None),
            Err(ScaffoldError::BevyVersion { .. })
        ));
    }

    /// A virtual workspace with `members`, one of which is the game.
    fn workspace_fixture(name: &str, members: &[(&str, &str)]) -> PathBuf {
        let root = temp_dir(name);
        let list = members
            .iter()
            .map(|(member, _)| format!("\"{member}\""))
            .collect::<Vec<_>>()
            .join(", ");
        std::fs::write(
            root.join("Cargo.toml"),
            format!("[workspace]\nresolver = \"3\"\nmembers = [{list}]\n"),
        )
        .unwrap();
        for (member, deps) in members {
            let dir = root.join(member);
            std::fs::create_dir_all(dir.join("src")).unwrap();
            std::fs::write(
                dir.join("Cargo.toml"),
                format!(
                    "[package]\nname = \"{member}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
                     [dependencies]\n{deps}\n"
                ),
            )
            .unwrap();
            std::fs::write(dir.join("src/lib.rs"), "pub struct GamePlugin;\n").unwrap();
        }
        root
    }

    /// The recorded plugin is the value most likely to be wrong and the
    /// one the user is uniquely able to check, so it must never be a
    /// silent guess.
    #[test]
    fn the_plan_states_which_plugin_it_recorded() {
        let root = temp_dir("plugin-note");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"g\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nbevy = \"0.20\"\n",
        )
        .unwrap();
        // A type merely named `*Plugin` above the real one, which a
        // first-name-wins scan would record instead of `MyGamePlugin`.
        std::fs::write(
            root.join("src/lib.rs"),
            "pub struct AudioPlugin;\nimpl Plugin for MyGamePlugin {}\n",
        )
        .unwrap();

        let plan = plan_import_project(&root, None).unwrap();
        assert!(
            plan.notes.iter().any(|n| n.contains("MyGamePlugin")),
            "the plan must state the plugin it recorded: {:?}",
            plan.notes
        );
        assert!(plan.changes.iter().any(|change| matches!(
            change,
            ImportChange::WriteFile { contents, path, .. }
                if path.ends_with("jackdaw.toml") && contents.contains("plugin = \"MyGamePlugin\"")
        )));
    }

    /// With no plugin to find, the key stays commented. Recording a
    /// plugin the crate does not define would leave doctor failing on
    /// a name that does not match source.
    #[test]
    fn no_plugin_means_no_plugin_key() {
        let root = temp_dir("no-plugin-key");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"g\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nbevy = \"0.20\"\n",
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub struct Health(f32);\n").unwrap();

        let plan = plan_import_project(&root, None).unwrap();
        apply_import_plan(&plan).unwrap();
        let manifest = jackdaw_project_build::project_manifest::ProjectManifest::read(&root);
        assert_eq!(
            manifest.plugin, None,
            "must not name a plugin that is absent"
        );
        let text = std::fs::read_to_string(root.join("jackdaw.toml")).unwrap();
        assert!(text.contains("# plugin = "), "got:\n{text}");
    }

    #[test]
    fn an_undetectable_plugin_is_flagged_not_asserted() {
        let root = temp_dir("plugin-guess");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"g\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nbevy = \"0.20\"\n",
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn helper() {}\n").unwrap();

        let plan = plan_import_project(&root, None).unwrap();
        assert!(
            plan.notes
                .iter()
                .any(|n| n.contains("no `impl Plugin for ...` found")),
            "got: {:?}",
            plan.notes
        );
    }

    /// A bin-only project gets `GamePlugin` created for it, so recording
    /// it is correct even though the pre-setup scan found nothing.
    #[test]
    fn a_created_game_plugin_is_recorded() {
        let root = temp_dir("created-plugin");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"g\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nbevy = \"0.20\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/main.rs"),
            "use bevy::prelude::*;\nfn main() { App::new().add_plugins(DefaultPlugins).run(); }\n",
        )
        .unwrap();

        let plan = plan_import_project(&root, None).unwrap();
        apply_import_plan(&plan).unwrap();
        let manifest = jackdaw_project_build::project_manifest::ProjectManifest::read(&root);
        assert_eq!(manifest.plugin.as_deref(), Some("GamePlugin"));
        // And the type it names really exists afterwards.
        let lib = std::fs::read_to_string(root.join("src/lib.rs")).unwrap();
        assert!(lib.contains("impl Plugin for GamePlugin"), "got:\n{lib}");
    }

    /// A project set up by an older jackdaw pins that version and its
    /// crate line, so upgrading has to move both.
    #[test]
    fn upgrade_refreshes_the_pins_and_the_crate_requirements() {
        let root = temp_dir("upgrade");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"g\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nbevy = \"0.20\"\n\
             jackdaw_runtime = { version = \"0.14\", features = [\"navmesh\"] }\n",
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), "impl Plugin for GamePlugin {}\n").unwrap();
        std::fs::write(
            root.join("jackdaw.toml"),
            "plugin = \"GamePlugin\"\n\n[jackdaw]\nversion = \"0.14.0\"\nbevy = \"0.14\"\n\n\
             [[run]]\nname = \"Play\"\n",
        )
        .unwrap();

        let plan = plan_upgrade_project(&root).unwrap();
        assert!(!plan.is_empty());
        apply_import_plan(&plan).unwrap();

        let settings = jackdaw_project_build::project_manifest::ProjectManifest::read(&root);
        assert_eq!(
            jackdaw_project_build::project_manifest::compare_pins(&settings.jackdaw),
            jackdaw_project_build::project_manifest::PinStatus::Match
        );
        // Everything else in the settings file survives the edit.
        assert_eq!(settings.plugin.as_deref(), Some("GamePlugin"));
        let settings_text = std::fs::read_to_string(root.join("jackdaw.toml")).unwrap();
        assert!(settings_text.contains("[[run]]"), "got:\n{settings_text}");

        let cargo = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(
            cargo.contains(&format!(
                "version = \"{}\"",
                jackdaw_project_build::jackdaw_requirement()
            )),
            "got:\n{cargo}"
        );
        // The bump must not drop the features the project asked for.
        assert!(cargo.contains("navmesh"), "got:\n{cargo}");
        // Nor touch anything that is not a jackdaw crate.
        assert!(cargo.contains("bevy = \"0.20\""), "got:\n{cargo}");
    }

    fn set_up_project(name: &str, runtime: &str) -> PathBuf {
        let root = temp_dir(name);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"g\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
                 [dependencies]\nbevy = \"0.20\"\njackdaw_runtime = {runtime}\n"
            ),
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), "impl Plugin for GamePlugin {}\n").unwrap();
        std::fs::write(
            root.join("jackdaw.toml"),
            format!(
                "plugin = \"GamePlugin\"\n\n{}",
                jackdaw_project_build::project_manifest::pins_toml_for_bevy(
                    jackdaw_project_build::BEVY_VERSION
                )
            ),
        )
        .unwrap();
        root
    }

    #[test]
    fn upgrade_drops_a_removed_runtime_feature_and_records_its_module() {
        let root = set_up_project(
            "upgrade-removed-feature",
            &format!(
                "{{ version = \"{}\", features = [\"physics\", \"pie\"] }}",
                jackdaw_project_build::jackdaw_requirement()
            ),
        );

        let plan = plan_upgrade_project(&root).unwrap();
        assert!(
            plan.notes
                .iter()
                .any(|note| note.contains("`physics`") && note.contains("not compatible")),
            "got: {:?}",
            plan.notes
        );
        apply_import_plan(&plan).unwrap();

        let cargo = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("features = [\"pie\"]"), "got:\n{cargo}");
        assert_eq!(modules_table(&root).get("physics"), Some(&true));
        assert!(plan_upgrade_project(&root).unwrap().is_empty());
    }

    #[test]
    fn upgrade_drops_a_removed_runtime_feature_from_the_workspace_entry() {
        let root = set_up_project("upgrade-removed-workspace-feature", "{ workspace = true }");
        let cargo = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            format!(
                "{cargo}\n[workspace.dependencies]\n\
                 jackdaw_runtime = {{ version = \"{}\", features = [\"pie\", \"physics\"] }}\n",
                jackdaw_project_build::jackdaw_requirement()
            ),
        )
        .unwrap();

        apply_import_plan(&plan_upgrade_project(&root).unwrap()).unwrap();

        let cargo = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(!cargo.contains("physics"), "got:\n{cargo}");
        assert_eq!(modules_table(&root).get("physics"), Some(&true));
    }

    #[test]
    fn import_drops_a_removed_runtime_feature() {
        let root = bevy_game(
            "import-removed-feature",
            "bevy = \"0.20\"\njackdaw_runtime = { version = \"0.19\", features = [\"physics\"] }\n",
        );
        let plan = plan_import_project(&root, None).unwrap();
        assert_eq!(plan.modules, modules(&["physics"]));
        let cargo = planned_contents(&plan, &root.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("features = [\"pie\"]"), "got:\n{cargo}");
    }

    /// Deferring a mismatch must not silence it. Recording this
    /// editor's pins would make every later check report health, which
    /// is the opposite of deferring.
    #[test]
    fn setting_up_past_a_mismatch_records_the_projects_bevy() {
        let root = temp_dir("mismatch-pins");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"g\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nbevy = \"0.16\"\n",
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), "impl Plugin for GamePlugin {}\n").unwrap();

        let plan = plan_import_with(&root, None, None, true).unwrap();
        assert!(
            !plan.changes.iter().any(|change| matches!(
                change,
                ImportChange::WriteFile { path, .. } if path.ends_with("Cargo.toml")
            )),
            "a runtime for another bevy must not be added"
        );
        apply_import_plan(&plan).unwrap();

        let manifest = jackdaw_project_build::project_manifest::ProjectManifest::read(&root);
        assert_eq!(manifest.jackdaw.bevy.as_deref(), Some("0.16"));
        assert!(
            jackdaw_project_build::project_manifest::compare_project(&root, &manifest)
                .is_blocking(),
            "the mismatch must keep being reported"
        );
    }

    /// Upgrading a project that was never set up must refuse: writing a
    /// pins-only jackdaw.toml would make it look integrated and skip the
    /// import flow that completes it.
    #[test]
    fn upgrade_refuses_a_project_that_was_never_set_up() {
        let root = temp_dir("upgrade-unset");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"g\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nbevy = \"0.20\"\n",
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), "impl Plugin for GamePlugin {}\n").unwrap();

        let error = plan_upgrade_project(&root).unwrap_err();
        assert!(matches!(error, ScaffoldError::NotSetUp(_)), "got {error:?}");
        assert!(error.to_string().contains("jd import --apply"));
        assert!(!root.join("jackdaw.toml").exists());
    }

    #[test]
    fn a_malformed_settings_file_is_reported_not_ignored() {
        let root = temp_dir("bad-settings");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"g\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nbevy = \"0.20\"\n",
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), "impl Plugin for GamePlugin {}\n").unwrap();
        std::fs::write(
            root.join("jackdaw.toml"),
            "plugin = \nthis is not toml [[\n",
        )
        .unwrap();

        let error = plan_upgrade_project(&root).unwrap_err();
        assert!(
            matches!(error, ScaffoldError::SettingsParse { .. }),
            "got {error:?}"
        );
    }

    /// A platform-gated jackdaw dependency is still a jackdaw
    /// dependency; leaving it behind contradicts what upgrade claims.
    #[test]
    fn upgrade_reaches_target_specific_dependencies() {
        let cargo = "[dependencies]\njackdaw_runtime = \"0.14\"\n\n\
                     [target.'cfg(target_arch = \"wasm32\")'.dependencies]\n\
                     jackdaw_web = \"0.14\"\n";
        let out = bump_jackdaw_requirements(cargo, &jackdaw_project_build::jackdaw_requirement())
            .expect("changed");
        assert_eq!(
            out.matches(&format!(
                "\"{}\"",
                jackdaw_project_build::jackdaw_requirement()
            ))
            .count(),
            2,
            "both entries should move:\n{out}"
        );
    }

    /// Upgrading moves the jackdaw version; it does not migrate the
    /// user's Bevy. Recording this editor's minor would mark a
    /// mismatched project healthy without changing anything, undoing
    /// the flag `--allow-bevy-mismatch` deliberately left behind.
    #[test]
    fn upgrade_does_not_launder_away_a_bevy_mismatch() {
        let root = temp_dir("upgrade-mismatch");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"g\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nbevy = \"0.16\"\n",
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), "impl Plugin for GamePlugin {}\n").unwrap();
        std::fs::write(
            root.join("jackdaw.toml"),
            "plugin = \"GamePlugin\"\n\n[jackdaw]\nversion = \"0.14.0\"\nbevy = \"0.16\"\n\n\
             [[run]]\nname = \"Play\"\n",
        )
        .unwrap();

        let plan = plan_upgrade_project(&root).unwrap();
        apply_import_plan(&plan).unwrap();

        let manifest = jackdaw_project_build::project_manifest::ProjectManifest::read(&root);
        assert_eq!(manifest.jackdaw.bevy.as_deref(), Some("0.16"));
        assert_eq!(
            manifest.jackdaw.version.as_deref(),
            Some(jackdaw_project_build::VERSION)
        );
        assert!(
            jackdaw_project_build::project_manifest::compare_project(&root, &manifest)
                .is_blocking(),
            "the mismatch must survive the upgrade"
        );
        // And the user is told, rather than left to notice.
        assert!(
            plan.notes.iter().any(|note| note.contains("bevy 0.16")),
            "got: {:?}",
            plan.notes
        );
    }

    /// `[workspace.dependencies]` is where a workspace writes versions
    /// down; members inherit with `{ workspace = true }`.
    #[test]
    fn upgrade_reaches_workspace_dependencies() {
        let cargo = "[workspace]\nmembers = [\"game\"]\n\n\
                     [workspace.dependencies]\njackdaw_runtime = \"0.14\"\n";
        let out = bump_jackdaw_requirements(cargo, &jackdaw_project_build::jackdaw_requirement())
            .expect("changed");
        assert!(
            out.contains(&format!(
                "jackdaw_runtime = \"{}\"",
                jackdaw_project_build::jackdaw_requirement()
            )),
            "got:\n{out}"
        );
    }

    /// The pins table must stay where the user put it, keeping any
    /// other keys and comments inside it.
    #[test]
    fn refreshing_pins_keeps_the_table_in_place() {
        let settings = "plugin = \"GamePlugin\"\n\n\
                        [jackdaw]\n# why this is here\nversion = \"0.14.0\"\nbevy = \"0.20\"\n\
                        note = \"keep me\"\n\n\
                        [[run]]\nname = \"Play\"\n";
        let out = rewrite_pins(settings, Some("0.20")).expect("changed");
        assert!(out.contains("note = \"keep me\""), "got:\n{out}");
        assert!(out.contains("# why this is here"), "got:\n{out}");
        assert!(
            out.find("[jackdaw]") < out.find("[[run]]"),
            "the table must not be relocated past the run configs:\n{out}"
        );
    }

    #[test]
    fn upgrade_is_a_no_op_on_a_current_project() {
        let root = temp_dir("upgrade-current");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"g\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nbevy = \"0.20\"\n",
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), "impl Plugin for GamePlugin {}\n").unwrap();
        std::fs::write(
            root.join("jackdaw.toml"),
            format!(
                "plugin = \"GamePlugin\"\n\n{}\n[[run]]\nname = \"Play\"\n",
                jackdaw_project_build::project_manifest::pins_toml()
            ),
        )
        .unwrap();

        assert!(plan_upgrade_project(&root).unwrap().is_empty());
    }

    /// A path or git dependency resolves outside the registry, so there
    /// is no version requirement to move and rewriting one would break
    /// a working local setup.
    #[test]
    fn upgrade_leaves_path_dependencies_alone() {
        let cargo =
            "[dependencies]\njackdaw_runtime = { path = '/checkout/crates/jackdaw_runtime' }\n";
        assert!(
            bump_jackdaw_requirements(cargo, &jackdaw_project_build::jackdaw_requirement())
                .is_none()
        );
    }

    #[test]
    fn import_resolves_the_game_member_of_a_workspace() {
        let root = workspace_fixture(
            "ws",
            &[("tools", "clap = \"4\""), ("game", "bevy = \"0.20\"")],
        );

        let plan = plan_import_project(&root, None).unwrap();
        assert_eq!(plan.package_name, "game");
        assert_eq!(
            dunce::canonicalize(&plan.package_dir).unwrap(),
            dunce::canonicalize(root.join("game")).unwrap()
        );
        // The settings file belongs to the folder the user opened, not
        // to the member; the member is recorded inside it instead.
        assert!(plan.changes.iter().any(|change| matches!(
            change,
            ImportChange::WriteFile { path, contents, .. }
                if path == &root.join("jackdaw.toml") && contents.contains("package = \"game\"")
        )));
        assert!(plan.notes.iter().any(|note| note.contains("workspace")));
    }

    #[test]
    fn import_asks_which_member_when_several_use_bevy() {
        let root = workspace_fixture(
            "wsambig",
            &[("client", "bevy = \"0.20\""), ("server", "bevy = \"0.20\"")],
        );

        let err = plan_import_project(&root, None).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("--package"), "got: {message}");
        assert!(message.contains("client") && message.contains("server"));

        let plan = plan_import_package(&root, None, Some("server")).unwrap();
        assert_eq!(plan.package_name, "server");
    }

    #[test]
    fn import_records_package_when_the_game_is_the_workspace_root() {
        let root = temp_dir("wsroot");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("helper/src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\".\", \"helper\"]\nresolver = \"2\"\n\n\
             [package]\nname = \"the-game\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nbevy = \"0.20\"\nhelper = { path = \"helper\" }\n",
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub struct GamePlugin;\n").unwrap();
        std::fs::write(
            root.join("helper/Cargo.toml"),
            "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nbevy = \"0.20\"\n",
        )
        .unwrap();
        std::fs::write(root.join("helper/src/lib.rs"), "pub struct HelperPlugin;\n").unwrap();

        let plan = plan_import_package(&root, None, Some("the-game")).unwrap();
        assert!(plan.changes.iter().any(|change| matches!(
            change,
            ImportChange::WriteFile { path, contents, .. }
                if path == &root.join("jackdaw.toml")
                    && contents.contains("package = \"the-game\"")
        )));
    }

    #[test]
    fn import_checks_a_workspace_inherited_bevy_version() {
        let root = temp_dir("wsinherit");
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nresolver = \"3\"\nmembers = [\"game\"]\n\n\
             [workspace.dependencies]\nbevy = \"0.16\"\n",
        )
        .unwrap();
        let member = root.join("game");
        std::fs::create_dir_all(member.join("src")).unwrap();
        std::fs::write(
            member.join("Cargo.toml"),
            "[package]\nname = \"game\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nbevy = { workspace = true }\n",
        )
        .unwrap();
        std::fs::write(member.join("src/lib.rs"), "pub struct GamePlugin;\n").unwrap();

        assert!(matches!(
            plan_import_project(&root, None),
            Err(ScaffoldError::BevyVersion { found }) if found == "0.16"
        ));
    }

    #[test]
    fn a_broken_manifest_reports_what_cargo_said() {
        let root = temp_dir("broken");
        std::fs::write(root.join("Cargo.toml"), "[package\nname = oops\n").unwrap();
        let err = plan_import_project(&root, None).unwrap_err();
        assert!(matches!(err, ScaffoldError::Package(_)), "got {err:?}");
        assert!(err.to_string().to_lowercase().contains("error"));
    }

    #[test]
    fn setup_records_the_versions_it_was_created_with() {
        let root = temp_dir("pins");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"pinned\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nbevy = \"0.20\"\n",
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub struct GamePlugin;\n").unwrap();
        import_project(&root, None).unwrap();

        let manifest = jackdaw_project_build::project_manifest::ProjectManifest::read(&root);
        assert_eq!(
            jackdaw_project_build::project_manifest::compare_pins(&manifest.jackdaw),
            jackdaw_project_build::project_manifest::PinStatus::Match
        );
    }

    #[test]
    fn scaffolded_projects_record_their_versions() {
        let dest = temp_dir("newpins").join("pinned-game");
        scaffold_new_project(&dest, "pinned-game", TemplateKind::Game).unwrap();
        let toml = std::fs::read_to_string(dest.join("jackdaw.toml")).unwrap();
        assert!(
            !toml.contains("{{"),
            "placeholders must be substituted:\n{toml}"
        );
        let manifest = jackdaw_project_build::project_manifest::ProjectManifest::read(&dest);
        assert_eq!(
            jackdaw_project_build::project_manifest::compare_pins(&manifest.jackdaw),
            jackdaw_project_build::project_manifest::PinStatus::Match
        );
        let cargo = std::fs::read_to_string(dest.join("Cargo.toml")).unwrap();
        assert!(cargo.contains(&format!(
            "bevy = \"{}\"",
            jackdaw_project_build::BEVY_VERSION
        )));
        assert!(!cargo.contains("{{"));
    }

    #[test]
    fn project_names_that_cannot_be_crate_names_are_rejected() {
        assert_eq!(
            validated_project_name("My Cool Game!").unwrap(),
            "my-cool-game"
        );
        assert_eq!(
            validated_project_name("  spaced  out ").unwrap(),
            "spaced-out"
        );
        for bad in ["", "***", "2d-shooter", "crate", "impl", "type"] {
            assert!(
                validated_project_name(bad).is_err(),
                "`{bad}` should be rejected"
            );
        }
    }

    fn bevy_game(name: &str, deps: &str) -> PathBuf {
        let root = temp_dir(name);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"my-game\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
                 [dependencies]\n{deps}"
            ),
        )
        .unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "use bevy::prelude::*;\n\npub struct GamePlugin;\n\n\
             impl Plugin for GamePlugin {\n    fn build(&self, _app: &mut App) {}\n}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/main.rs"),
            "use bevy::prelude::*;\n\nfn main() {\n    App::new()\n        \
             .add_plugins(DefaultPlugins)\n        .add_plugins(my_game::GamePlugin)\n        \
             .run();\n}\n",
        )
        .unwrap();
        root
    }

    fn planned_contents(plan: &ImportPlan, path: &Path) -> Option<String> {
        plan.changes.iter().find_map(|change| match change {
            ImportChange::WriteFile {
                path: written,
                contents,
                ..
            } if written == path => Some(contents.clone()),
            _ => None,
        })
    }

    #[test]
    fn import_wraps_default_plugins_in_the_common_main_shapes() {
        let shapes = [
            "fn main() {\n    App::new()\n        .add_plugins(DefaultPlugins)\n        \
             .add_plugins(my_game::GamePlugin)\n        .run();\n}\n",
            "fn main() { App::new().add_plugins(DefaultPlugins)\
             .add_plugins(my_game::GamePlugin).run(); }\n",
            "fn main() {\n    App::new()\n        .add_plugins((DefaultPlugins, my_game::GamePlugin))\n        \
             .run();\n}\n",
            "fn main() {\n    App::new()\n        .add_plugins(bevy::DefaultPlugins)\n        \
             .add_plugins(my_game::GamePlugin)\n        .run();\n}\n",
            "fn main() {\n    App::new()\n        .add_plugins((\n            \
             DefaultPlugins.set(WindowPlugin::default()),\n            my_game::GamePlugin,\n        \
             ))\n        .run();\n}\n",
            "fn main() -> ExitCode {\n    App::new()\n        .add_plugins(DefaultPlugins)\n        \
             .add_plugins(my_game::GamePlugin)\n        .run()\n}\n",
            "fn main() {\n    let mut app = App::new();\n    app.add_plugins(DefaultPlugins);\n    \
             app.add_plugins(my_game::GamePlugin);\n    app.run();\n}\n",
        ];
        for (index, shape) in shapes.iter().enumerate() {
            let root = bevy_game(&format!("wrap-shape-{index}"), "bevy = \"0.20\"\n");
            std::fs::write(
                root.join("src/main.rs"),
                format!("use bevy::prelude::*;\n\n{shape}"),
            )
            .unwrap();

            let plan = plan_import_project(&root, None).unwrap();
            let main = planned_contents(&plan, &root.join("src/main.rs"))
                .unwrap_or_else(|| panic!("main.rs is not wrapped for:\n{shape}"));
            assert!(
                main.contains("jackdaw_runtime::maybe_windowless(default_plugins)"),
                "got:\n{main}"
            );
            assert!(syn::parse_file(&main).is_ok(), "still parses:\n{main}");
            assert!(
                !plan
                    .notes
                    .iter()
                    .any(|note| note.contains("pass DefaultPlugins through")),
                "no manual step is left for:\n{shape}\n{:?}",
                plan.notes
            );
        }
    }

    #[test]
    fn import_wires_the_runtime_into_a_plain_bevy_game() {
        let root = bevy_game("wire-runtime", "bevy = \"0.20\"\n");

        let plan = plan_import_project(&root, None).unwrap();
        let cargo = planned_contents(&plan, &root.join("Cargo.toml")).expect("Cargo.toml edit");
        assert!(
            cargo.contains(&format!(
                "jackdaw_runtime = {{ {}, features = [\"pie\"] }}",
                jackdaw_dep_requirement("jackdaw_runtime")
            )),
            "got:\n{cargo}"
        );
        let lib = planned_contents(&plan, &root.join("src/lib.rs")).expect("lib.rs edit");
        assert!(
            lib.contains(
                "fn build(&self, app: &mut App) {\n        \
                 app.add_plugins(jackdaw_runtime::JackdawPlugin);\n    }"
            ),
            "got:\n{lib}"
        );
        let main = planned_contents(&plan, &root.join("src/main.rs")).expect("main.rs edit");
        assert!(
            main.contains(
                "let default_plugins = jackdaw_runtime::maybe_windowless(default_plugins);"
            ),
            "got:\n{main}"
        );
        assert!(main.contains(".add_plugins(default_plugins)"));
        assert!(
            plan.changes
                .iter()
                .any(|change| change.summary().starts_with("modify ")
                    && change.summary().ends_with("Cargo.toml")),
            "the preview lists the manifest edit"
        );
        assert!(
            !plan
                .notes
                .iter()
                .any(|note| note.contains("add_plugins(jackdaw_runtime")),
            "no manual step is left once the edit is made: {:?}",
            plan.notes
        );
    }

    #[test]
    fn a_second_import_has_nothing_to_do() {
        let root = bevy_game("reimport", "bevy = \"0.20\"\n");
        apply_import_plan(&plan_import_project(&root, None).unwrap()).unwrap();

        let again = plan_import_project(&root, None).unwrap();
        assert!(again.is_empty(), "second run planned {:?}", again.changes);
    }

    fn modules(ids: &[&str]) -> ModuleSet {
        ids.iter().map(|id| (*id).to_string()).collect()
    }

    fn modules_table(root: &Path) -> std::collections::BTreeMap<String, bool> {
        jackdaw_project_build::project_manifest::ProjectManifest::read(root)
            .modules
            .expect("a [modules] table")
    }

    #[test]
    fn the_modules_flags_choose_the_modules_and_refuse_nonsense() {
        let args = |list: &[&str]| list.iter().map(|a| (*a).to_string()).collect::<Vec<_>>();
        assert_eq!(
            modules_arg(&args(&["game", "--modules", "navmesh"])).unwrap(),
            Some(modules(&["navmesh"]))
        );
        assert_eq!(
            modules_arg(&args(&["game", "--modules=multiplayer"])).unwrap(),
            Some(modules(&["multiplayer"]))
        );
        assert_eq!(
            modules_arg(&args(&["game", "--no-modules"])).unwrap(),
            Some(ModuleSet::new())
        );
        assert_eq!(modules_arg(&args(&["game"])).unwrap(), None);
        assert!(modules_arg(&args(&["game", "--modules", "rendering"])).is_err());
        assert!(modules_arg(&args(&["game", "--modules", "physics"])).is_err());
        assert!(modules_arg(&args(&["game", "--modules", "physics", "--no-modules"])).is_err());
    }

    /// Windows checkouts can carry the templates with CRLF line endings.
    #[test]
    fn crlf_templates_still_get_their_module_lines() {
        for path in [
            "templates/game/Cargo.toml.template",
            "templates/game/src/main.rs.template",
        ] {
            let template =
                std::fs::read_to_string(format!("{}/{path}", env!("CARGO_MANIFEST_DIR")))
                    .unwrap()
                    .replace("\r\n", "\n")
                    .replace('\n', "\r\n");
            let filled = substitute_placeholders(
                &template,
                "my-game",
                "my_game",
                "",
                "My Game",
                &modules(&["multiplayer"]),
            );
            assert!(!filled.contains("{{"), "{path}: got:\n{filled}");
            assert!(
                filled.contains("jackdaw_multiplayer"),
                "{path}: got:\n{filled}"
            );
        }
    }

    #[test]
    fn a_new_game_writes_its_modules_into_cargo_toml_main_and_jackdaw_toml() {
        let dest = temp_dir("new-navmesh").join("nav-game");
        scaffold_new_project_with(
            &dest,
            "nav-game",
            TemplateKind::Game,
            &modules(&["navmesh"]),
        )
        .unwrap();
        let cargo = std::fs::read_to_string(dest.join("Cargo.toml")).unwrap();
        assert!(
            cargo.contains("features = [\"navmesh\", \"terrain\", \"pie\"] }"),
            "got:\n{cargo}"
        );
        assert!(!cargo.contains("avian3d"), "got:\n{cargo}");
        assert!(!cargo.contains("jackdaw_multiplayer"), "got:\n{cargo}");
        let table = modules_table(&dest);
        assert_eq!(table.get("physics"), Some(&false));
        assert_eq!(table.get("navmesh"), Some(&true));
        assert_eq!(table.get("multiplayer"), Some(&false));

        let bare = temp_dir("new-bare").join("bare-game");
        scaffold_new_project_with(&bare, "bare-game", TemplateKind::Game, &ModuleSet::new())
            .unwrap();
        let cargo = std::fs::read_to_string(bare.join("Cargo.toml")).unwrap();
        assert!(
            cargo.contains("features = [\"terrain\", \"pie\"] }"),
            "got:\n{cargo}"
        );
        assert!(!cargo.contains("avian3d"), "got:\n{cargo}");
        let main = std::fs::read_to_string(bare.join("src/main.rs")).unwrap();
        assert!(!main.contains("avian3d"), "got:\n{main}");
        assert!(!main.contains("{{"), "got:\n{main}");
        assert!(modules_table(&bare).values().all(|on| !on));

        let net = temp_dir("new-net").join("net-game");
        scaffold_new_project_with(
            &net,
            "net-game",
            TemplateKind::Game,
            &modules(&["multiplayer"]),
        )
        .unwrap();
        let cargo = std::fs::read_to_string(net.join("Cargo.toml")).unwrap();
        assert!(
            cargo.contains("\njackdaw_multiplayer = { "),
            "got:\n{cargo}"
        );
        let main = std::fs::read_to_string(net.join("src/main.rs")).unwrap();
        assert!(main.contains(".add_plugins(jackdaw_multiplayer::JackdawMultiplayerTypesPlugin)"));
    }

    #[test]
    fn import_detects_the_modules_a_game_already_uses_and_adds_the_chosen_ones_once() {
        let root = bevy_game(
            "import-modules",
            "bevy = \"0.20\"\n\
             jackdaw_runtime = { version = \"0.19\", features = [\"physics\", \"pie\"] }\n\
             avian3d = \"0.7\"\n",
        );
        std::fs::write(
            root.join("src/main.rs"),
            "use bevy::prelude::*;\n\nfn main() {\n    let default_plugins = \
             jackdaw_runtime::maybe_windowless(DefaultPlugins);\n    App::new()\n        \
             .add_plugins(default_plugins)\n        .add_plugins(my_game::GamePlugin)\n        \
             .run();\n}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "use bevy::prelude::*;\n\npub struct GamePlugin;\n\n\
             impl Plugin for GamePlugin {\n    fn build(&self, app: &mut App) {\n        \
             app.add_plugins(jackdaw_runtime::JackdawPlugin);\n    }\n}\n",
        )
        .unwrap();

        let plain = plan_import_project(&root, None).unwrap();
        assert_eq!(plain.detected_modules, modules(&["physics"]));
        assert_eq!(plain.modules, modules(&["physics"]));
        let cargo = planned_contents(&plain, &root.join("Cargo.toml")).expect("Cargo.toml edit");
        assert!(
            cargo.contains("features = [\"pie\"]"),
            "only the removed feature goes when no module is chosen, got:\n{cargo}"
        );

        let chosen = modules(&["navmesh", "multiplayer"]);
        let plan = plan_import_with_modules(&root, None, None, false, Some(&chosen)).unwrap();
        assert_eq!(
            plan.modules,
            modules(&["multiplayer", "navmesh", "physics"])
        );
        let cargo = planned_contents(&plan, &root.join("Cargo.toml")).expect("Cargo.toml edit");
        assert!(
            cargo.contains("features = [\"pie\", \"navmesh\"]"),
            "got:\n{cargo}"
        );
        assert!(cargo.contains("jackdaw_multiplayer = {"), "got:\n{cargo}");
        assert!(
            plan.notes
                .iter()
                .any(|note| note.contains("JackdawMultiplayerTypesPlugin")),
            "the plugin left to add is named: {:?}",
            plan.notes
        );
        apply_import_plan(&plan).unwrap();
        let table = modules_table(&root);
        assert_eq!(table.get("navmesh"), Some(&true));
        assert_eq!(table.get("multiplayer"), Some(&true));

        let again = plan_import_with_modules(&root, None, None, false, Some(&chosen)).unwrap();
        assert!(again.is_empty(), "second run planned {:?}", again.changes);
        assert_eq!(
            again.detected_modules,
            modules(&["multiplayer", "navmesh", "physics"])
        );
    }

    #[test]
    fn changing_modules_later_edits_both_manifests_and_keeps_the_games_crates() {
        let root = temp_dir("change-modules").join("change-game");
        scaffold_new_project(&root, "change-game", TemplateKind::Game).unwrap();
        let cargo_path = root.join("Cargo.toml");

        let chosen = modules(&["multiplayer", "navmesh"]);
        let plan = plan_module_change(&root, &chosen).unwrap();
        assert_eq!(plan.detected_modules, ModuleSet::new());
        apply_import_plan(&plan).unwrap();
        let cargo = std::fs::read_to_string(&cargo_path).unwrap();
        assert!(
            cargo.contains("features = [\"terrain\", \"pie\", \"navmesh\"]"),
            "got:\n{cargo}"
        );
        assert!(cargo.contains("jackdaw_multiplayer = {"), "got:\n{cargo}");
        assert_eq!(modules_table(&root).get("navmesh"), Some(&true));
        assert!(
            plan_module_change(&root, &chosen).unwrap().is_empty(),
            "asking again changes nothing"
        );

        let plan = plan_module_change(&root, &ModuleSet::new()).unwrap();
        assert!(
            plan.notes
                .iter()
                .any(|note| note.starts_with("jackdaw_multiplayer stays")),
            "{:?}",
            plan.notes
        );
        apply_import_plan(&plan).unwrap();
        let cargo = std::fs::read_to_string(&cargo_path).unwrap();
        assert!(
            cargo.contains("features = [\"terrain\", \"pie\"]"),
            "got:\n{cargo}"
        );
        assert!(cargo.contains("jackdaw_multiplayer = {"), "got:\n{cargo}");
        assert!(modules_table(&root).values().all(|on| !on));
        let settings = std::fs::read_to_string(root.join("jackdaw.toml")).unwrap();
        assert!(settings.contains("[[run]]"), "got:\n{settings}");
    }

    #[test]
    fn a_jackdaw_crate_a_module_adds_comes_from_where_the_runtime_does() {
        let doc = |entry: &str| -> DocumentMut {
            format!("[dependencies]\njackdaw_runtime = {entry}\n")
                .parse()
                .unwrap()
        };
        let like = |entry: &str| jackdaw_crate_like_runtime(&doc(entry), "jackdaw_multiplayer");
        assert_eq!(
            like("{ path = '/src/jackdaw/crates/jackdaw_runtime', features = [\"pie\"] }")
                .as_deref(),
            Some("{ path = '/src/jackdaw/crates/jackdaw_multiplayer' }")
        );
        assert_eq!(
            like("{ git = \"https://example.com/jackdaw\", rev = \"abc\" }").as_deref(),
            Some("{ git = \"https://example.com/jackdaw\", rev = \"abc\" }")
        );
        assert_eq!(like("\"0.19\"").as_deref(), Some("{ version = \"0.19\" }"));
        assert_eq!(
            like("{ version = \"=0.19.0-rc.10\", features = [] }").as_deref(),
            Some("{ version = \"=0.19.0-rc.10\" }")
        );
    }

    #[test]
    fn import_enables_pie_on_an_existing_runtime_dependency() {
        let root = bevy_game("add-pie", "bevy = \"0.20\"\njackdaw_runtime = \"0.19\"\n");
        let plan = plan_import_project(&root, None).unwrap();
        let cargo = planned_contents(&plan, &root.join("Cargo.toml")).unwrap();
        assert!(
            cargo.contains("jackdaw_runtime = { version = \"0.19\", features = [\"pie\"] }"),
            "got:\n{cargo}"
        );

        let root = bevy_game(
            "extend-features",
            "bevy = \"0.20\"\njackdaw_runtime = { version = \"0.19\", features = [\"navmesh\"] }\n",
        );
        let plan = plan_import_project(&root, None).unwrap();
        let cargo = planned_contents(&plan, &root.join("Cargo.toml")).unwrap();
        assert!(
            cargo.contains("features = [\"navmesh\", \"pie\"]"),
            "got:\n{cargo}"
        );
    }

    #[test]
    fn import_adds_pie_to_a_workspace_inherited_runtime() {
        let root = temp_dir("ws-runtime");
        let workspace = "[workspace]\nresolver = \"3\"\nmembers = [\"game\"]\n\n\
                         [workspace.dependencies]\njackdaw_runtime = \"0.19\"\n";
        std::fs::write(root.join("Cargo.toml"), workspace).unwrap();
        let member = root.join("game");
        std::fs::create_dir_all(member.join("src")).unwrap();
        std::fs::write(
            member.join("Cargo.toml"),
            "[package]\nname = \"game\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nbevy = \"0.20\"\njackdaw_runtime = { workspace = true }\n",
        )
        .unwrap();
        std::fs::write(member.join("src/lib.rs"), "pub struct GamePlugin;\n").unwrap();

        let plan = plan_import_project(&root, None).unwrap();
        let cargo = planned_contents(&plan, &member.join("Cargo.toml")).unwrap();
        assert!(
            cargo.contains("jackdaw_runtime = { workspace = true, features = [\"pie\"] }"),
            "got:\n{cargo}"
        );
        assert!(planned_contents(&plan, &root.join("Cargo.toml")).is_none());

        std::fs::write(
            root.join("Cargo.toml"),
            workspace.replace(
                "jackdaw_runtime = \"0.19\"",
                "jackdaw_runtime = { version = \"0.19\", features = [\"pie\"] }",
            ),
        )
        .unwrap();
        let plan = plan_import_project(&root, None).unwrap();
        assert!(
            planned_contents(&plan, &member.join("Cargo.toml")).is_none(),
            "pie already comes from the workspace"
        );
    }

    #[test]
    fn the_runtime_edit_keeps_manifest_comments_and_order() {
        let root = bevy_game(
            "keep-comments",
            "# Engine.\nbevy = \"0.20\" # pinned to the minor\n\n# Utilities.\nrand = \"0.9\"\n",
        );
        let before = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
        let plan = plan_import_project(&root, None).unwrap();
        let cargo = planned_contents(&plan, &root.join("Cargo.toml")).unwrap();
        assert!(cargo.starts_with(&before), "got:\n{cargo}");
        assert!(cargo[before.len()..].starts_with("jackdaw_runtime = {"));
    }

    #[test]
    fn enabling_pie_keeps_a_trailing_comment() {
        let root = bevy_game(
            "pie-comment",
            "bevy = \"0.20\"\njackdaw_runtime = \"0.19\" # scene loading\nrand = \"0.9\"\n",
        );
        let plan = plan_import_project(&root, None).unwrap();
        let cargo = planned_contents(&plan, &root.join("Cargo.toml")).unwrap();
        assert!(
            cargo.contains(
                "jackdaw_runtime = { version = \"0.19\", features = [\"pie\"] } # scene loading\n"
            ),
            "got:\n{cargo}"
        );
    }

    fn assert_leaves_sources_alone(plan: &ImportPlan, root: &Path) {
        for file in ["Cargo.toml", "src/lib.rs", "src/main.rs"] {
            assert!(
                planned_contents(plan, &root.join(file)).is_none(),
                "{file} must not be edited: {:?}",
                plan.notes
            );
        }
        assert!(
            plan.notes
                .iter()
                .any(|note| note.contains("declares jackdaw_runtime as")),
            "the plan says why: {:?}",
            plan.notes
        );
    }

    #[test]
    fn an_optional_runtime_gets_notes_not_edits() {
        let root = bevy_game(
            "optional-runtime",
            "bevy = \"0.20\"\njackdaw_runtime = { version = \"0.19\", optional = true }\n",
        );
        assert_leaves_sources_alone(&plan_import_project(&root, None).unwrap(), &root);
    }

    #[test]
    fn a_renamed_runtime_is_not_declared_twice() {
        let root = bevy_game(
            "renamed-runtime",
            "bevy = \"0.20\"\nruntime = { package = \"jackdaw_runtime\", version = \"0.19\" }\n",
        );
        assert_leaves_sources_alone(&plan_import_project(&root, None).unwrap(), &root);
    }

    #[test]
    fn a_target_specific_runtime_is_not_declared_again() {
        let root = bevy_game("target-runtime", "bevy = \"0.20\"\n");
        let cargo = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            format!("{cargo}\n[target.'cfg(unix)'.dependencies]\njackdaw_runtime = \"0.19\"\n"),
        )
        .unwrap();
        assert_leaves_sources_alone(&plan_import_project(&root, None).unwrap(), &root);
    }

    #[test]
    fn pie_from_a_package_feature_is_not_forced_on() {
        let root = bevy_game(
            "feature-pie",
            "bevy = \"0.20\"\njackdaw_runtime = \"0.19\"\n",
        );
        let cargo = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            format!("{cargo}\n[features]\neditor = [\"jackdaw_runtime/pie\"]\n"),
        )
        .unwrap();
        let plan = plan_import_project(&root, None).unwrap();
        assert!(planned_contents(&plan, &root.join("Cargo.toml")).is_none());
        assert!(
            planned_contents(&plan, &root.join("src/lib.rs"))
                .is_some_and(|lib| lib.contains("jackdaw_runtime::JackdawPlugin")),
            "the plugin needs no feature"
        );
        assert!(
            planned_contents(&plan, &root.join("src/main.rs")).is_none(),
            "maybe_windowless only exists with pie, which a default build may lack"
        );
        assert!(
            plan.notes
                .iter()
                .any(|note| note.contains("maybe_windowless"))
        );
    }

    #[test]
    fn a_member_inherits_the_runtime_its_workspace_declares() {
        let root = temp_dir("ws-declares-runtime");
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nresolver = \"3\"\nmembers = [\"game\"]\n\n\
             [workspace.dependencies]\njackdaw_runtime = \"0.19\"\n",
        )
        .unwrap();
        let member = root.join("game");
        std::fs::create_dir_all(member.join("src")).unwrap();
        std::fs::write(
            member.join("Cargo.toml"),
            "[package]\nname = \"game\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nbevy = \"0.20\"\n",
        )
        .unwrap();
        std::fs::write(member.join("src/lib.rs"), "pub struct GamePlugin;\n").unwrap();

        let plan = plan_import_project(&root, None).unwrap();
        let cargo = planned_contents(&plan, &member.join("Cargo.toml")).unwrap();
        assert!(
            cargo.contains("jackdaw_runtime = { workspace = true, features = [\"pie\"] }"),
            "got:\n{cargo}"
        );
    }

    #[test]
    fn an_uneditable_plugin_keeps_the_manual_note() {
        let root = bevy_game("manual-plugin", "bevy = \"0.20\"\n");
        std::fs::write(
            root.join("src/lib.rs"),
            "pub struct GamePlugin;\nimpl Plugin for GamePlugin {}\n",
        )
        .unwrap();
        let plan = plan_import_project(&root, None).unwrap();
        assert!(planned_contents(&plan, &root.join("src/lib.rs")).is_none());
        assert!(
            plan.notes
                .iter()
                .any(|note| note.contains("app.add_plugins(jackdaw_runtime::JackdawPlugin)")),
            "got: {:?}",
            plan.notes
        );
    }

    #[test]
    fn import_plan_is_side_effect_free_until_applied() {
        let root = temp_dir("preview");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"preview\"\nversion = \"0.1.0\"\n\n[dependencies]\nbevy = \"0.20\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/main.rs"),
            "use bevy::prelude::*;\nfn main() { App::new().add_plugins(DefaultPlugins).run(); }\n",
        )
        .unwrap();

        let main_before = std::fs::read_to_string(root.join("src/main.rs")).unwrap();
        let plan = plan_import_project(&root, None).unwrap();
        assert!(!plan.is_empty());
        assert!(!root.join("jackdaw.toml").exists());
        assert!(!root.join("src/lib.rs").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("src/main.rs")).unwrap(),
            main_before
        );

        apply_import_plan(&plan).unwrap();
        assert!(root.join("jackdaw.toml").exists());
        assert!(root.join("src/lib.rs").exists());
        assert!(root.join(".jackdaw").is_dir());
    }
}
