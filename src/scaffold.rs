//! Project scaffolding: `jackdaw new`, `jackdaw init`, and the code
//! behind the launcher's New Project and Import actions.
//!
//! Both flows produce the same contract: a normal Bevy crate with a
//! `[lib]` target, a `jackdaw.toml`, and a gitignored `.jackdaw/`
//! directory the editor owns. New projects come from templates
//! embedded in this binary (instantiated by string substitution,
//! offline); imports write additive files only and never move user
//! code or change how the project's own builds behave.
//!
//! Projects wired up by the retired static-link scaffold are detected
//! on import and cleaned: the editor feature, editor binary, cargo
//! aliases, and profile pins it wrote are removed.

use std::path::{Path, PathBuf};

use bevy::app::AppExit;
use include_dir::{Dir, include_dir};
use toml_edit::{DocumentMut, Item};

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

/// Why scaffolding could not complete.
#[derive(Debug)]
pub enum ScaffoldError {
    NoManifest(PathBuf),
    ManifestParse(String),
    NoPackageName,
    /// The project's Bevy dependency does not match the supported minor.
    BevyVersion {
        found: String,
    },
    Io(String),
}

impl std::fmt::Display for ScaffoldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScaffoldError::NoManifest(p) => {
                write!(
                    f,
                    "no Cargo.toml at {} (run this in your project root)",
                    p.display()
                )
            }
            ScaffoldError::ManifestParse(e) => write!(f, "could not parse Cargo.toml: {e}"),
            ScaffoldError::NoPackageName => write!(f, "Cargo.toml has no [package] name"),
            ScaffoldError::BevyVersion { found } => write!(
                f,
                "this jackdaw release supports Bevy {SUPPORTED_BEVY_MINOR}; the project \
                 depends on bevy {found}. Update the project (or use a matching jackdaw \
                 release) and import again."
            ),
            ScaffoldError::Io(e) => write!(f, "{e}"),
        }
    }
}

/// CLI entry point for `jackdaw new <name> [--extension]`.
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
    let Some(raw_name) = args.iter().find(|a| !a.starts_with("--")) else {
        eprintln!("jackdaw new: usage: jackdaw new <name> [--extension]");
        return AppExit::error();
    };
    let project_name = sanitize_project_name(raw_name);
    if project_name.is_empty() {
        eprintln!("jackdaw new: `{raw_name}` is not a usable project name");
        return AppExit::error();
    }
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("jackdaw new: {e}");
            return AppExit::error();
        }
    };
    let dest = cwd.join(&project_name);
    match scaffold_new_project(&dest, &project_name, kind) {
        Ok(report) => {
            println!("jackdaw new: created {}", dest.display());
            for a in &report.actions {
                println!("  {a}");
            }
            println!("\nNext: open {project_name}/ in jackdaw, or `cargo run` to play standalone.");
            AppExit::Success
        }
        Err(e) => {
            eprintln!("jackdaw new: {e}");
            AppExit::error()
        }
    }
}

/// CLI entry point for `jackdaw init [--plugin <Type>]`: import the
/// project in the current directory.
#[expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "CLI subcommand writes its results and errors to the terminal"
)]
pub fn run_init_cli(args: &[String]) -> AppExit {
    let plugin = parse_plugin_arg(args);
    let root = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("jackdaw init: {e}");
            return AppExit::error();
        }
    };
    match import_project(&root, plugin) {
        Ok(report) => {
            println!("jackdaw init: {}", root.display());
            if report.actions.is_empty() {
                println!("  already set up, nothing to do");
            } else {
                for a in &report.actions {
                    println!("  {a}");
                }
            }
            if report.created_lib_stub {
                println!(
                    "\nA src/lib.rs stub with a GamePlugin was created; move your game \
                     setup into it so the editor can run your systems on Play."
                );
            }
            println!("\nNext: open this folder in jackdaw.");
            AppExit::Success
        }
        Err(e) => {
            eprintln!("jackdaw init: {e}");
            AppExit::error()
        }
    }
}

fn parse_plugin_arg(args: &[String]) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--plugin" {
            return it.next().cloned();
        }
        if let Some(v) = a.strip_prefix("--plugin=") {
            return Some(v.to_string());
        }
    }
    None
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
    // deps at the local workspace so it builds against the
    // in-development version instead of an unpublished crates.io one.
    crate::new_project::rewrite_jackdaw_dep_for_dev_checkout(dest);

    Ok(ScaffoldReport {
        actions: vec![format!("scaffolded {written} files")],
        created_lib_stub: false,
    })
}

/// Import an existing Bevy project: verify the Bevy version, ensure a
/// lib target (stub offered for bin-only projects), record the plugin
/// type, and write the additive jackdaw files. Never touches user code
/// or how the project's own builds behave. Reusable by the launcher's
/// Import action; also cleans projects wired by the retired
/// static-link scaffold.
pub fn import_project(
    root: &Path,
    plugin_override: Option<String>,
) -> Result<ScaffoldReport, ScaffoldError> {
    let manifest_path = root.join("Cargo.toml");
    if !manifest_path.is_file() {
        return Err(ScaffoldError::NoManifest(manifest_path));
    }
    let manifest_text =
        std::fs::read_to_string(&manifest_path).map_err(|e| ScaffoldError::Io(format!("{e}")))?;
    let mut doc: DocumentMut = manifest_text
        .parse()
        .map_err(|e| ScaffoldError::ManifestParse(format!("{e}")))?;

    let package_name = doc
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .ok_or(ScaffoldError::NoPackageName)?
        .to_string();
    let lib_name = package_name.replace('-', "_");

    check_bevy_version(&doc)?;

    let mut actions = Vec::new();

    if detect_legacy_scaffold(&doc) {
        clean_legacy_scaffold(root, &mut doc, &mut actions)?;
        std::fs::write(&manifest_path, doc.to_string())
            .map_err(|e| ScaffoldError::Io(format!("{e}")))?;
    }

    // Lib target: an explicit [lib] or the conventional src/lib.rs.
    let has_lib = doc.get("lib").is_some() || root.join("src/lib.rs").is_file();
    let mut created_lib_stub = false;
    if !has_lib {
        std::fs::write(root.join("src/lib.rs"), lib_stub_source())
            .map_err(|e| ScaffoldError::Io(format!("{e}")))?;
        actions.push("created src/lib.rs with a GamePlugin stub".to_string());
        created_lib_stub = true;
    }

    // Plugin type: explicit override, then source detection, then the
    // GamePlugin convention (which is what the stub provides).
    let plugin = plugin_override
        .or_else(|| {
            detect_plugin(root, &lib_name)
                .and_then(|p| p.split_once("::").map(|(_, name)| name.to_string()))
        })
        .unwrap_or_else(|| "GamePlugin".to_string());

    let jackdaw_toml = root.join("jackdaw.toml");
    if !jackdaw_toml.exists() {
        std::fs::write(&jackdaw_toml, jackdaw_toml_source(&plugin))
            .map_err(|e| ScaffoldError::Io(format!("{e}")))?;
        actions.push("wrote jackdaw.toml".to_string());
    }

    std::fs::create_dir_all(root.join(".jackdaw"))
        .map_err(|e| ScaffoldError::Io(format!("{e}")))?;
    if ensure_gitignored(root)? {
        actions.push("gitignored .jackdaw/".to_string());
    }

    Ok(ScaffoldReport {
        actions,
        created_lib_stub,
    })
}

/// The project's declared bevy dependency must be semver-compatible
/// with the supported minor. Projects without a direct bevy dependency
/// pass (a workspace or an extension crate may express it indirectly);
/// the build surfaces any real mismatch later.
fn check_bevy_version(doc: &DocumentMut) -> Result<(), ScaffoldError> {
    let Some(bevy) = doc.get("dependencies").and_then(|d| d.get("bevy")) else {
        return Ok(());
    };
    let req = bevy
        .as_str()
        .map(str::to_string)
        .or_else(|| {
            bevy.get("version")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    if req.is_empty() {
        // Path or git dependency; the build decides.
        return Ok(());
    }
    let normalized = req.trim_start_matches(['^', '=', '~']);
    if normalized.starts_with(SUPPORTED_BEVY_MINOR) {
        Ok(())
    } else {
        Err(ScaffoldError::BevyVersion { found: req })
    }
}

/// Whether the manifest carries the retired static-link scaffold's
/// wiring (editor feature, editor bin, optional jackdaw dep).
fn detect_legacy_scaffold(doc: &DocumentMut) -> bool {
    let has_editor_feature = doc.get("features").and_then(|f| f.get("editor")).is_some();
    let has_editor_bin = doc
        .get("bin")
        .and_then(|b| b.as_array_of_tables())
        .is_some_and(|bins| {
            bins.iter()
                .any(|t| t.get("name").and_then(|n| n.as_str()) == Some("editor"))
        });
    has_editor_feature || has_editor_bin
}

/// Remove everything the retired static-link scaffold wrote: the
/// editor/pie features, the editor bin target and its source, the
/// optional jackdaw dependency, the profile pin, the default-run, and
/// the cargo aliases.
fn clean_legacy_scaffold(
    root: &Path,
    doc: &mut DocumentMut,
    actions: &mut Vec<String>,
) -> Result<(), ScaffoldError> {
    if let Some(features) = doc.get_mut("features").and_then(|f| f.as_table_mut())
        && features.remove("editor").is_some() | features.remove("pie").is_some()
    {
        actions.push("removed the editor/pie cargo features".to_string());
    }
    if let Some(bins) = doc.get_mut("bin").and_then(|b| b.as_array_of_tables_mut()) {
        let before = bins.len();
        bins.retain(|t| t.get("name").and_then(|n| n.as_str()) != Some("editor"));
        if bins.len() != before {
            actions.push("removed the [[bin]] editor target".to_string());
        }
    }
    if doc
        .get("bin")
        .and_then(|b| b.as_array_of_tables())
        .is_some_and(toml_edit::ArrayOfTables::is_empty)
    {
        doc.remove("bin");
    }
    if let Some(deps) = doc.get_mut("dependencies").and_then(|d| d.as_table_mut())
        && deps.remove("jackdaw").is_some()
    {
        actions.push("removed the optional jackdaw dependency".to_string());
    }
    if let Some(package) = doc.get_mut("package").and_then(|p| p.as_table_mut())
        && package.remove("default-run").is_some()
    {
        actions.push("removed package.default-run".to_string());
    }
    let removed_profile = remove_nested(doc, &["profile", "dev", "package", "jackdaw"]);
    if removed_profile {
        actions.push("removed the jackdaw profile pin".to_string());
    }

    let editor_bin = root.join("src/bin/editor.rs");
    if editor_bin.exists() {
        std::fs::remove_file(&editor_bin).map_err(|e| ScaffoldError::Io(format!("{e}")))?;
        actions.push("deleted src/bin/editor.rs".to_string());
        let bin_dir = root.join("src/bin");
        let _ = std::fs::remove_dir(bin_dir);
    }

    let cargo_config = root.join(".cargo/config.toml");
    if let Ok(text) = std::fs::read_to_string(&cargo_config)
        && let Ok(mut config) = text.parse::<DocumentMut>()
    {
        let mut changed = false;
        if let Some(alias) = config.get_mut("alias").and_then(|a| a.as_table_mut()) {
            changed |= alias.remove("editor").is_some();
            changed |= alias.remove("play").is_some();
        }
        if config
            .get("alias")
            .and_then(|a| a.as_table())
            .is_some_and(toml_edit::Table::is_empty)
        {
            config.remove("alias");
        }
        if changed {
            std::fs::write(&cargo_config, config.to_string())
                .map_err(|e| ScaffoldError::Io(format!("{e}")))?;
            actions.push("removed the cargo editor/play aliases".to_string());
        }
    }

    Ok(())
}

/// Remove a nested table by path, returning whether anything was
/// removed. Empty parents are left in place; `toml_edit` drops empty
/// implicit tables on serialization.
fn remove_nested(doc: &mut DocumentMut, path: &[&str]) -> bool {
    fn walk(item: &mut Item, path: &[&str]) -> bool {
        match path {
            [] => false,
            [last] => item
                .as_table_like_mut()
                .is_some_and(|t| t.remove(last).is_some()),
            [head, rest @ ..] => item
                .as_table_like_mut()
                .and_then(|t| t.get_mut(head))
                .is_some_and(|next| walk(next, rest)),
        }
    }
    walk(doc.as_item_mut(), path)
}

/// Append the `.jackdaw/` entry to the project's `.gitignore`,
/// creating the file if absent. Returns whether anything was written.
fn ensure_gitignored(root: &Path) -> Result<bool, ScaffoldError> {
    let path = root.join(".gitignore");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing
        .lines()
        .any(|l| l.trim() == "/.jackdaw" || l.trim() == ".jackdaw" || l.trim() == ".jackdaw/")
    {
        return Ok(false);
    }
    let mut out = existing;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("/.jackdaw\n");
    std::fs::write(&path, out).map_err(|e| ScaffoldError::Io(format!("{e}")))?;
    Ok(true)
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
//! jackdaw (or run `jackdaw-cli build`) and they appear in
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

fn jackdaw_toml_source(plugin: &str) -> String {
    format!(
        "# jackdaw project settings.\n\
         \n\
         # The game plugin type inside your lib crate.\n\
         plugin = \"{plugin}\"\n\
         \n\
         [[run]]\n\
         name = \"Play\"\n"
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
/// and underscores to dashes, everything else alphanumeric.
pub fn sanitize_project_name(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.trim().chars() {
        match c {
            ' ' | '_' => out.push('-'),
            c if c.is_ascii_alphanumeric() => out.push(c.to_ascii_lowercase()),
            '-' => out.push('-'),
            _ => {}
        }
    }
    out.trim_matches('-').to_string()
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
        assert!(manifest.contains("jackdaw_api"));
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
        std::fs::write(root.join("src/lib.rs"), "pub struct TheirPlugin;\n").unwrap();
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
    fn import_rejects_a_bevy_minor_mismatch() {
        let root = temp_dir("mismatch");
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"old\"\nversion = \"0.1.0\"\n\n[dependencies]\nbevy = \"0.16\"\n",
        )
        .unwrap();
        assert!(matches!(
            import_project(&root, None),
            Err(ScaffoldError::BevyVersion { .. })
        ));
    }

    #[test]
    fn import_cleans_the_legacy_static_scaffold() {
        let root = temp_dir("legacy");
        std::fs::create_dir_all(root.join("src/bin")).unwrap();
        std::fs::create_dir_all(root.join(".cargo")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            concat!(
                "[package]\nname = \"old-game\"\nversion = \"0.1.0\"\ndefault-run = \"old-game\"\n\n",
                "[features]\neditor = [\"dep:jackdaw\"]\npie = []\n\n",
                "[dependencies]\nbevy = \"0.19\"\njackdaw = { version = \"0.5\", optional = true }\n\n",
                "[[bin]]\nname = \"editor\"\nrequired-features = [\"editor\"]\n\n",
                "[profile.dev.package.jackdaw]\nopt-level = 1\n",
            ),
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub struct GamePlugin;\n").unwrap();
        std::fs::write(root.join("src/bin/editor.rs"), "fn main() {}\n").unwrap();
        std::fs::write(
            root.join(".cargo/config.toml"),
            "[alias]\neditor = \"run --bin editor --features editor\"\nplay = \"run\"\n",
        )
        .unwrap();

        let report = import_project(&root, None).unwrap();
        let manifest = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(!manifest.contains("editor"));
        assert!(!manifest.contains("jackdaw = "));
        assert!(!manifest.contains("default-run"));
        assert!(!manifest.contains("[profile.dev.package.jackdaw]"));
        assert!(!root.join("src/bin/editor.rs").exists());
        let config = std::fs::read_to_string(root.join(".cargo/config.toml")).unwrap();
        assert!(!config.contains("editor"));
        assert!(report.actions.iter().any(|a| a.contains("editor/pie")));
    }
}
