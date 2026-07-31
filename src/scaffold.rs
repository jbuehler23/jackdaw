//! Project scaffolding and side-effect-free import planning.
//!
//! Both flows produce the same contract: a normal Bevy crate with a
//! `[lib]` target, a `jackdaw.toml`, and a gitignored `.jackdaw/`
//! directory the editor owns. New projects come from templates
//! embedded in this binary (instantiated by string substitution,
//! offline). Imports are always planned before they are applied; the
//! plan is the single interface shared by `jd import` and the launcher.

use std::path::{Path, PathBuf};

use bevy::app::AppExit;
use include_dir::{Dir, include_dir};
use toml_edit::DocumentMut;

/// The one Bevy minor this editor release supports.
const SUPPORTED_BEVY_MINOR: &str = "0.19";

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
pub fn run_new_cli(args: &[String]) -> AppExit {
    let kind = if args.iter().any(|a| a == "--extension") {
        TemplateKind::Extension
    } else {
        TemplateKind::Game
    };
    let Some(raw_name) = first_positional(args, &["--path"]) else {
        eprintln!("jd new: usage: jd new <name> [--extension] [--path <dir>] [--no-git]");
        return AppExit::error();
    };
    let project_name = match validated_project_name(raw_name) {
        Ok(name) => name,
        Err(e) => {
            eprintln!("jd new: {e}");
            return AppExit::error();
        }
    };
    let location = match flag_value(args, "--path") {
        Some(path) => PathBuf::from(path),
        None => match std::env::current_dir() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("jd new: {e}");
                return AppExit::error();
            }
        },
    };
    if !location.is_dir() {
        eprintln!("jd new: {} is not a directory", location.display());
        return AppExit::error();
    }
    let dest = location.join(&project_name);
    match scaffold_new_project(&dest, &project_name, kind) {
        Ok(report) => {
            println!("jd new: created {}", dest.display());
            for a in &report.actions {
                println!("  {a}");
            }
            if !args.iter().any(|a| a == "--no-git") {
                init_git_repository(&dest);
            }
            // Creating a project inside an existing repo is normal, and
            // a workspace whose `members` is a glob will adopt it. The
            // scaffold's own `[workspace]` then makes two roots, which
            // breaks every cargo command in the PARENT: a directory the
            // user did not ask us to touch.
            if let Some(parent) = enclosing_workspace(&dest)
                && let Err(detail) = resolves(&parent)
            {
                eprintln!(
                    "\nwarning: `{}` is inside the cargo workspace at {}, which now reports:\n  \
                     {detail}\n\
                     The new project declares its own `[workspace]` so it builds standalone. To \
                     keep the outer workspace working too, either add this line to {}:\n  \
                     exclude = [\"{}\"]\n\
                     or delete the `[workspace]` table from the new project's Cargo.toml to make \
                     it a member.",
                    project_name,
                    parent.display(),
                    parent.join("Cargo.toml").display(),
                    dest.strip_prefix(&parent).unwrap_or(&dest).display(),
                );
            }

            let resolution = {
                // Announce it: on a cold registry this updates the
                // index, which is slow enough to look like a hang.
                println!("  checking dependencies resolve...");
                resolves(&dest)
            };
            // The dev-checkout rewrite logs through `warn!`, which the
            // CLI has no subscriber for, so without this the user ships
            // a manifest full of absolute local paths and only finds
            // out from a collaborator's failing build.
            if let Some(checkout) = crate::new_project::jackdaw_dev_checkout() {
                println!(
                    "\nnote: this jackdaw runs from the checkout at {}, so the new project \
                     depends on its crates by path. That Cargo.toml is local to this machine; \
                     switch those back to versions before sharing it.",
                    checkout.display()
                );
            }
            // A version that is not on the index yet would otherwise
            // surface as a baffling failure at the user's first `cargo
            // run`, with nothing connecting it to the tool that wrote
            // the file.
            if let Err(detail) = resolution {
                eprintln!(
                    "\nwarning: this project's dependencies do not resolve yet:\n  {detail}\n\
                     The scaffold is written and correct; the jackdaw crates it asks for are \
                     not on crates.io at this version. Run `jd doctor --project {}` after the \
                     matching release is published.",
                    dest.display()
                );
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
            AppExit::Success
        }
        Err(e) => {
            eprintln!("jd new: {e}");
            AppExit::error()
        }
    }
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
    let output = std::process::Command::new("cargo")
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
pub(crate) fn init_git_repository(dest: &Path) {
    let inside_repo = std::process::Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(dest)
        .output()
        .is_ok_and(|out| out.status.success() && out.stdout.starts_with(b"true"));
    if inside_repo {
        return;
    }
    let _ = std::process::Command::new("git")
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
pub fn run_import_cli(args: &[String]) -> AppExit {
    let plugin = parse_plugin_arg(args);
    let package = flag_value(args, "--package");
    let requested_root = first_positional(args, &["--plugin", "--package"]).map(PathBuf::from);
    let root = match requested_root
        .map(Ok)
        .unwrap_or_else(std::env::current_dir)
        .and_then(std::fs::canonicalize)
    {
        Ok(d) => d,
        Err(e) => {
            eprintln!("jd import: {e}");
            return AppExit::error();
        }
    };
    let allow_mismatch = args.iter().any(|arg| arg == "--allow-bevy-mismatch");
    match plan_import_with(&root, plugin, package.as_deref(), allow_mismatch) {
        Ok(plan) if plan.is_empty() => {
            println!("jd import: {} is already set up", root.display());
            println!("\nNext: `jd open {}`", root.display());
            AppExit::Success
        }
        Ok(plan) => {
            println!("jd import preview: {}", root.display());
            if plan.package_dir != plan.root {
                println!("  package: {}", plan.package_name);
            }
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
                return AppExit::Success;
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
                    AppExit::Success
                }
                Err(e) => {
                    eprintln!("jd import: {e}");
                    AppExit::error()
                }
            }
        }
        Err(e) => {
            eprintln!("jd import: {e}");
            AppExit::error()
        }
    }
}

/// Plan the edits that bring a project up to this jackdaw: refresh the
/// `[jackdaw]` pins, and bump any `jackdaw_*` requirement to the
/// anchored minor.
///
/// Pins were previously written once at setup and never revisited, so
/// an editor upgrade left every project claiming the version that
/// created it and its dependency requirements pointing at the old line
/// of releases. Side-effect free, like the import plan: the same
/// changes are previewed, then applied on a second explicit action.
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

    let settings_path = jackdaw_project_build::project_manifest::path(root);
    let settings = std::fs::read_to_string(&settings_path).unwrap_or_default();
    if let Some(updated) = rewrite_pins(&settings, project_bevy.as_deref()) {
        changes.push(ImportChange::WriteFile {
            path: settings_path,
            contents: updated,
            replace: true,
        });
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
    let mut bumped_any = false;
    let mut manifests = Vec::new();
    if project_bevy
        .as_deref()
        .is_none_or(|minor| minor == jackdaw_project_build::BEVY_VERSION)
    {
        manifests.push(package.dir.join("Cargo.toml"));
        if package.dir != root {
            manifests.push(root.join("Cargo.toml"));
        }
    } else {
        notes.push(
            "leaving the jackdaw dependencies alone: moving them forward would require a bevy \
             this project does not use"
                .to_string(),
        );
    }
    for manifest_path in manifests {
        let cargo = std::fs::read_to_string(&manifest_path)
            .map_err(|e| ScaffoldError::Io(format!("{}: {e}", manifest_path.display())))?;
        if let Some(updated) = bump_jackdaw_requirements(&cargo) {
            changes.push(ImportChange::WriteFile {
                path: manifest_path,
                contents: updated,
                replace: true,
            });
            bumped_any = true;
        }
    }
    if bumped_any {
        notes.push(format!("requests the jackdaw crates at {JACKDAW_DEP_REQ}"));
    }

    Ok(ImportPlan {
        root: root.to_path_buf(),
        package_dir: package.dir,
        package_name: package.name,
        changes,
        notes,
        migrated_bin_target: false,
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

/// Point every `jackdaw_*` dependency at the anchored version. `None`
/// when nothing needed changing (already current, or path/git deps,
/// which state no version to bump).
fn bump_jackdaw_requirements(cargo: &str) -> Option<String> {
    const TABLES: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];
    let mut doc: DocumentMut = cargo.parse().ok()?;
    let mut changed = false;

    for table in TABLES {
        if let Some(deps) = doc.get_mut(table).and_then(|item| item.as_table_like_mut()) {
            changed |= bump_jackdaw_deps(deps);
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
                    changed |= bump_jackdaw_deps(deps);
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
        changed |= bump_jackdaw_deps(deps);
    }
    changed.then(|| doc.to_string())
}

/// Move every `jackdaw_*` entry in one dependency table to the anchored
/// version. Returns whether anything changed.
fn bump_jackdaw_deps(deps: &mut dyn toml_edit::TableLike) -> bool {
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
            if existing != JACKDAW_DEP_REQ {
                *entry = toml_edit::value(JACKDAW_DEP_REQ);
                changed = true;
            }
        } else if let Some(table) = entry.as_table_like_mut() {
            // A path or git dependency resolves elsewhere; there is no
            // version requirement to bring forward.
            if table.get("path").is_some() || table.get("git").is_some() {
                continue;
            }
            let current = table.get("version").and_then(|v| v.as_str());
            if current.is_some() && current != Some(JACKDAW_DEP_REQ) {
                table.insert("version", toml_edit::value(JACKDAW_DEP_REQ));
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
pub fn run_upgrade_cli(args: &[String]) -> AppExit {
    let requested = first_positional(args, &[]).map(PathBuf::from);
    let root = match requested
        .clone()
        .map(Ok)
        .unwrap_or_else(std::env::current_dir)
        .and_then(std::fs::canonicalize)
    {
        Ok(root) => root,
        Err(error) => {
            match requested {
                Some(path) => eprintln!("jd upgrade: {}: {error}", path.display()),
                None => eprintln!("jd upgrade: {error}"),
            }
            return AppExit::error();
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
            AppExit::Success
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
                return AppExit::Success;
            }
            match apply_import_plan(&plan) {
                Ok(report) => {
                    println!("\nApplied {} change(s).", report.actions.len());
                    println!("\nNext: `jd build {}`", root.display());
                    AppExit::Success
                }
                Err(error) => {
                    eprintln!("jd upgrade: {error}");
                    AppExit::error()
                }
            }
        }
        Err(error) => {
            eprintln!("jd upgrade: {error}");
            AppExit::error()
        }
    }
}

/// Launch the GUI with a selected project.
#[expect(clippy::print_stderr, reason = "CLI errors are written to stderr")]
pub fn run_open_cli(args: &[String]) -> AppExit {
    let root = args
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let root = match root.canonicalize() {
        Ok(root) => root,
        Err(error) => {
            eprintln!("jd open: {}: {error}", root.display());
            return AppExit::error();
        }
    };
    if !root.is_dir() {
        eprintln!("jd open: {} is not a directory", root.display());
        return AppExit::error();
    }
    let editor = std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.parent()
                .map(|parent| parent.join(editor_executable_name()))
        })
        .unwrap_or_else(|| PathBuf::from(editor_executable_name()));
    // Pass the project directly. Reordering the user's global recents
    // file to smuggle the argument through would leave that edit behind
    // even when the launch fails, and would race with any other jackdaw
    // process.
    match std::process::Command::new(editor)
        .env(crate::project::ENV_OPEN_PROJECT, &root)
        .spawn()
    {
        Ok(_) => AppExit::Success,
        Err(error) => {
            eprintln!("jd open: could not launch jackdaw: {error}");
            AppExit::error()
        }
    }
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

/// Scaffold a new project at `dest` from an embedded template,
/// substituting the project name, crate name, authors, and title
/// placeholders. Reusable by the launcher's New Project action.
pub fn scaffold_new_project(
    dest: &Path,
    project_name: &str,
    kind: TemplateKind,
) -> Result<ScaffoldReport, ScaffoldError> {
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
        let rel = file.path().to_string_lossy().replace('\\', "/");
        let (dest_rel, contents): (PathBuf, Vec<u8>) = match rel.strip_suffix(".template") {
            Some(stripped) => {
                let text = std::str::from_utf8(file.contents()).map_err(|_| {
                    ScaffoldError::Io(format!("template file {rel} is not valid UTF-8"))
                })?;
                let rendered =
                    substitute_placeholders(text, project_name, &crate_name, &authors, &title);
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
    crate::new_project::rewrite_jackdaw_dep_for_dev_checkout(dest);

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
    match check_bevy_version(&doc, workspace_doc.as_ref()) {
        Ok(()) => {}
        Err(error) if allow_bevy_mismatch => {
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

    // Lib target: an explicit [lib] or the conventional src/lib.rs.
    let has_lib = doc.get("lib").is_some() || package_dir.join("src/lib.rs").is_file();
    // The plugin the migration will declare. Not always `GamePlugin`: a
    // crate that already defines that name gets a differently-named
    // wrapper, and recording the wrong one points the shim at a type
    // that will not exist.
    let mut migrated_plugin: Option<String> = None;
    if !has_lib {
        match crate::migrate::plan_migration(&package_dir, &package.crate_name) {
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
                    contents: lib_stub_source().to_string(),
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
    // GamePlugin convention (which is what the stub provides). Whatever
    // is chosen is stated in the notes, because a wrong plugin here only
    // shows up much later as a build error inside the generated shim.
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
    // An override is the user's call, but a name their crate does not
    // declare only fails much later, inside the generated shim.
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
        // Distinguish "none exists" from "one exists but the shim
        // cannot name it": telling a user with a perfectly good plugin
        // that none was found sends them looking for the wrong problem.
        let unreachable: Vec<String> = jackdaw_project_build::detect::plugin_paths(&package_dir)
            .into_iter()
            .filter(|found| found.crate_path.is_none())
            .map(|found| found.type_name)
            .collect();
        match &plugin {
            Some(name) if detected.is_some() => notes.push(format!("game plugin: {name}")),
            Some(name) => notes.push(format!("game plugin: {name} (created by this setup)")),
            // Recording a name the crate does not define would make the
            // generated shim reference a nonexistent type, so leave the
            // key unset and say what to do when there is one.
            None if candidates.len() >= 2 => notes.push(format!(
                "several plugins found ({}) and none is named GamePlugin; set `plugin` in \
                 jackdaw.toml to pick the one Play should run",
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
                     components. Add a plugin and set `plugin` in jackdaw.toml to use Play."
                        .to_string(),
                ),
            },
        }
    }

    // Authoring levels is only half the loop; the game has to load them.
    // Import never edits the manifest, so say exactly what to add. A
    // migration emits this itself, so avoid saying it twice.
    let has_runtime_dep = doc
        .get("dependencies")
        .and_then(|deps| deps.get("jackdaw_runtime"))
        .is_some();
    if !has_runtime_dep && !migrated_bin_target {
        notes.push(crate::migrate::runtime_wiring_note());
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

fn has_jackdaw_gitignore(contents: &str) -> bool {
    contents
        .lines()
        .any(|l| l.trim() == "/.jackdaw" || l.trim() == ".jackdaw" || l.trim() == ".jackdaw/")
}

fn lib_stub_source() -> &'static str {
    r#"//! Game library: the editor runs [`GamePlugin`] on Play, and your
//! own binary can add it too. Move your game setup (systems,
//! resources, observers) in here from main.rs.
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
    fn build(&self, _app: &mut App) {}
}

// Example: an editable component. Uncomment, save, and Rebuild to see
// `Health` in the inspector's Add Component list.
//
// #[derive(Component, Reflect, Default)]
// #[reflect(Component, Default)]
// pub struct Health {
//     pub max: f32,
//     pub current: f32,
// }
"#
}

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
) -> String {
    let package = match workspace_package {
        Some(name) => {
            format!("\n# The workspace member jackdaw builds as the game.\npackage = \"{name}\"\n")
        }
        None => String::new(),
    };
    // With no plugin to name, the key stays commented: the shim only
    // emits a game entry when one is set, and pointing it at a type the
    // crate does not define would break the build.
    let plugin = match plugin {
        Some(name) => {
            format!("# The game plugin type inside your lib crate.\nplugin = \"{name}\"\n")
        }
        None => "# The game plugin type inside your lib crate. Set this once you have one.\n\
                 # plugin = \"GamePlugin\"\n"
            .to_string(),
    };
    format!(
        "# jackdaw project settings.\n\
         \n\
         {plugin}\
         {package}\n\
         {pins}\n\
         [[run]]\n\
         name = \"Play\"\n",
        pins = match pinned_bevy {
            Some(bevy) => jackdaw_project_build::project_manifest::pins_toml_for_bevy(bevy),
            None => jackdaw_project_build::project_manifest::pins_toml(),
        }
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
) -> String {
    text.replace("{{project-name | title_case}}", title)
        .replace("{{crate_name}}", crate_name)
        .replace("{{authors}}", authors)
        .replace("{{project-name}}", project_name)
        .replace(
            "{{jackdaw_pins}}",
            jackdaw_project_build::project_manifest::pins_toml().trim_end(),
        )
        .replace("{{bevy_version}}", jackdaw_project_build::BEVY_VERSION)
        .replace("{{jackdaw_version}}", JACKDAW_DEP_REQ)
}

/// The version requirement scaffolded projects request for the public
/// jackdaw crates. Jackdaw's version is anchored to the Bevy minor, so
/// a project set up by this editor asks for exactly the line of
/// releases that targets the same engine.
const JACKDAW_DEP_REQ: &str = jackdaw_project_build::BEVY_VERSION;

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
        std::process::Command::new("git")
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
    fn import_writes_additive_files_only() {
        let root = temp_dir("import");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"their-game\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nbevy = \"0.19\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "pub struct TheirPlugin;\nimpl Plugin for TheirPlugin {}\n",
        )
        .unwrap();
        let before = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();

        let report = import_project(&root, None).unwrap();
        assert!(!report.created_lib_stub);
        assert_eq!(
            before,
            std::fs::read_to_string(root.join("Cargo.toml")).unwrap()
        );
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
            "[package]\nname = \"bin-only\"\nversion = \"0.1.0\"\n\n[dependencies]\nbevy = \"0.19\"\n",
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
        // Compatible with 0.19 one way or another.
        for req in [
            "0.19",
            "0.19.2",
            "^0.19",
            "=0.19.0",
            "~0.19",
            ">=0.19, <0.21",
        ] {
            assert!(
                requirement_admits_supported_bevy(req),
                "`{req}` should be accepted"
            );
        }
        // `0.190` is a different minor that merely shares a prefix.
        for req in ["0.16", "0.190", "0.20", "0.20.0-rc.1", "1.0"] {
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
             [dependencies]\nbevy = \"0.19\"\n",
        )
        .unwrap();
        // A type merely named `*Plugin` above the real one: the old
        // first-name-wins scan recorded `AudioPlugin` here.
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

    /// With no plugin to find, the convention is still recorded, but the
    /// guess is stated rather than presented as a detection.
    /// Recording a plugin the crate does not define would make the
    /// generated shim reference a nonexistent type, failing the build
    /// inside a crate the user never wrote.
    #[test]
    fn no_plugin_means_no_plugin_key() {
        let root = temp_dir("no-plugin-key");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"g\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nbevy = \"0.19\"\n",
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
             [dependencies]\nbevy = \"0.19\"\n",
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
             [dependencies]\nbevy = \"0.19\"\n",
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

    /// A project set up by an older jackdaw claims that version forever
    /// and keeps requesting the old crate line, so upgrading has to
    /// move both.
    #[test]
    fn upgrade_refreshes_the_pins_and_the_crate_requirements() {
        let root = temp_dir("upgrade");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"g\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nbevy = \"0.19\"\n\
             jackdaw_runtime = { version = \"0.14\", features = [\"physics\"] }\n",
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
            cargo.contains(&format!("version = \"{JACKDAW_DEP_REQ}\"")),
            "got:\n{cargo}"
        );
        // The bump must not drop the features the project asked for.
        assert!(cargo.contains("physics"), "got:\n{cargo}");
        // Nor touch anything that is not a jackdaw crate.
        assert!(cargo.contains("bevy = \"0.19\""), "got:\n{cargo}");
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
        apply_import_plan(&plan).unwrap();

        let manifest = jackdaw_project_build::project_manifest::ProjectManifest::read(&root);
        assert_eq!(manifest.jackdaw.bevy.as_deref(), Some("0.16"));
        assert!(
            jackdaw_project_build::project_manifest::compare_project(&root, &manifest)
                .is_blocking(),
            "the mismatch must keep being reported"
        );
    }

    /// Upgrading a project that was never set up used to write a
    /// pins-only jackdaw.toml, which made it look integrated and
    /// permanently skipped the import flow that would have completed it.
    #[test]
    fn upgrade_refuses_a_project_that_was_never_set_up() {
        let root = temp_dir("upgrade-unset");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"g\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nbevy = \"0.19\"\n",
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
             [dependencies]\nbevy = \"0.19\"\n",
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
        let out = bump_jackdaw_requirements(cargo).expect("changed");
        assert_eq!(
            out.matches(&format!("\"{JACKDAW_DEP_REQ}\"")).count(),
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
        let out = bump_jackdaw_requirements(cargo).expect("changed");
        assert!(
            out.contains(&format!("jackdaw_runtime = \"{JACKDAW_DEP_REQ}\"")),
            "got:\n{out}"
        );
    }

    /// The pins table must stay where the user put it, keeping any
    /// other keys and comments inside it.
    #[test]
    fn refreshing_pins_keeps_the_table_in_place() {
        let settings = "plugin = \"GamePlugin\"\n\n\
                        [jackdaw]\n# why this is here\nversion = \"0.14.0\"\nbevy = \"0.19\"\n\
                        note = \"keep me\"\n\n\
                        [[run]]\nname = \"Play\"\n";
        let out = rewrite_pins(settings, Some("0.19")).expect("changed");
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
             [dependencies]\nbevy = \"0.19\"\n",
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
        assert!(bump_jackdaw_requirements(cargo).is_none());
    }

    #[test]
    fn import_resolves_the_game_member_of_a_workspace() {
        let root = workspace_fixture(
            "ws",
            &[("tools", "clap = \"4\""), ("game", "bevy = \"0.19\"")],
        );

        let plan = plan_import_project(&root, None).unwrap();
        assert_eq!(plan.package_name, "game");
        assert_eq!(plan.package_dir, root.join("game"));
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
            &[("client", "bevy = \"0.19\""), ("server", "bevy = \"0.19\"")],
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
             [dependencies]\nbevy = \"0.19\"\nhelper = { path = \"helper\" }\n",
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub struct GamePlugin;\n").unwrap();
        std::fs::write(
            root.join("helper/Cargo.toml"),
            "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nbevy = \"0.19\"\n",
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
             [dependencies]\nbevy = \"0.19\"\n",
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

    #[test]
    fn import_plan_is_side_effect_free_until_applied() {
        let root = temp_dir("preview");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"preview\"\nversion = \"0.1.0\"\n\n[dependencies]\nbevy = \"0.19\"\n",
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
