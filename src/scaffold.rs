//! `jackdaw init`: wire the editor into an existing Bevy project.
//!
//! Scaffolds the minimal pieces a project needs to open in the editor: the
//! `editor` cargo feature, an `editor` binary built on [`crate::editor_main`],
//! the jackdaw + runtime + avian dependencies, a `jackdaw.toml` run config, and
//! the `cargo editor` / `cargo play` aliases. Idempotent: re-running fills in
//! only what is missing.
//!
//! The editor must statically link the project's library to see its reflected
//! component types, so the project needs a `[lib]` target (`src/lib.rs`). A
//! bin-only project gets a `GamePlugin` stub written for it (the user then moves
//! their game code in); we cannot safely move their code for them.

use std::path::{Path, PathBuf};

use bevy::app::AppExit;
use include_dir::{Dir, include_dir};
use toml_edit::{Array, DocumentMut, Item, Table, value};

const JACKDAW_VERSION: &str = "0.5";
const JACKDAW_RUNTIME_VERSION: &str = "0.5";
const AVIAN_VERSION: &str = "0.6";

/// The recommended new-project template, embedded into the binary so `jackdaw
/// new` scaffolds offline with no cargo-generate dependency.
static GAME_STATIC_TEMPLATE: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates/game-static");

/// What `jackdaw init` changed (or left in place).
pub struct ScaffoldReport {
    pub actions: Vec<String>,
    /// A `src/lib.rs` stub was created because the project had no library
    /// target; the user must move their game code into its `GamePlugin` for
    /// their components to show up in the editor.
    pub created_lib_stub: bool,
}

/// Why scaffolding could not complete.
#[derive(Debug)]
pub enum ScaffoldError {
    NoManifest(PathBuf),
    ManifestParse(String),
    NoPackageName,
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
            ScaffoldError::Io(e) => write!(f, "{e}"),
        }
    }
}

/// CLI entry point for `jackdaw init`. Run from a project directory.
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
    match scaffold_existing_project(&root, plugin) {
        Ok(report) => {
            println!("jackdaw init: {}", root.display());
            if report.actions.is_empty() {
                println!("  already set up, nothing to do");
            } else {
                for a in &report.actions {
                    println!("  {a}");
                }
            }
            println!("\nNext: `cargo editor` (or open this project from the jackdaw launcher).");
            println!(
                "For embedded play (the game inside the editor's Game panel), wrap your \
                 main.rs DefaultPlugins with `jackdaw_runtime::maybe_windowless(..)` under \
                 `#[cfg(feature = \"pie\")]`. See the migration guide."
            );
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

/// Wire jackdaw into the project at `root`. Reusable by the CLI and the
/// launcher's "Set up jackdaw" action. `plugin_override` names the game plugin
/// to link (e.g. `my_game::MyGamePlugin`); when `None`, the project's `src/lib.rs`
/// is scanned for a `pub struct ...Plugin`, falling back to `<crate>::GamePlugin`.
pub fn scaffold_existing_project(
    root: &Path,
    plugin_override: Option<String>,
) -> Result<ScaffoldReport, ScaffoldError> {
    let manifest_path = root.join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|_| ScaffoldError::NoManifest(manifest_path.clone()))?;
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e| ScaffoldError::ManifestParse(format!("{e}")))?;

    let package_name = doc
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .ok_or(ScaffoldError::NoPackageName)?
        .to_string();
    let lib_name = package_name.replace('-', "_");

    let mut actions = Vec::new();
    let mut manifest_changed = false;

    // The editor links the project's lib to discover its component types. If the
    // project has no library target, create a `src/lib.rs` stub with a
    // `GamePlugin` so setup can proceed; the user then moves their game code into
    // it (we can't safely move it for them).
    let has_lib = root.join("src/lib.rs").is_file() || doc.get("lib").is_some();
    let mut created_lib_stub = false;
    if !has_lib {
        let lib_path = root.join("src/lib.rs");
        if let Some(parent) = lib_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ScaffoldError::Io(format!("{e}")))?;
        }
        std::fs::write(&lib_path, lib_stub_source())
            .map_err(|e| ScaffoldError::Io(format!("{e}")))?;
        actions.push("created src/lib.rs (move your game code into GamePlugin)".into());
        created_lib_stub = true;
    }

    // Dependencies.
    let deps = doc
        .entry("dependencies")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| ScaffoldError::ManifestParse("[dependencies] is not a table".into()))?;
    if !deps.contains_key("jackdaw") {
        deps["jackdaw"] = inline_dep(&[
            ("version", JACKDAW_VERSION.into()),
            ("default-features", false.into()),
            ("optional", true.into()),
        ]);
        actions.push("added dependency `jackdaw` (editor, optional)".into());
        manifest_changed = true;
    }
    if !deps.contains_key("jackdaw_runtime") {
        let mut features = Array::new();
        features.push("physics");
        let mut t = toml_edit::InlineTable::new();
        t.insert("version", JACKDAW_RUNTIME_VERSION.into());
        t.insert("features", toml_edit::Value::Array(features));
        deps["jackdaw_runtime"] = Item::Value(toml_edit::Value::InlineTable(t));
        actions.push("added dependency `jackdaw_runtime` (features = [\"physics\"])".into());
        manifest_changed = true;
    }
    if !deps.contains_key("avian3d") {
        deps["avian3d"] = value(AVIAN_VERSION);
        actions.push("added dependency `avian3d`".into());
        manifest_changed = true;
    }

    // `editor` feature.
    let features = doc
        .entry("features")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| ScaffoldError::ManifestParse("[features] is not a table".into()))?;
    if !features.contains_key("editor") {
        let mut arr = Array::new();
        arr.push("dep:jackdaw");
        arr.push("jackdaw_runtime/pie");
        features["editor"] = value(arr);
        actions.push("added `editor` feature".into());
        manifest_changed = true;
    }
    if !features.contains_key("pie") {
        let mut arr = Array::new();
        arr.push("jackdaw_runtime/pie");
        features["pie"] = value(arr);
        actions.push("added `pie` feature".into());
        manifest_changed = true;
    }

    // Build the (huge, editor-only) `jackdaw` crate at opt-level 1. The common
    // Bevy dev profile sets `[profile.dev.package."*"] opt-level = 3`, which
    // would optimize `jackdaw` like any dep and need ~8GB to compile (OOMs
    // alongside the running editor). It's never linked into the shipped game,
    // so there's no runtime cost. Other deps keep their opt-level.
    if set_jackdaw_profile_opt_level(&mut doc)? {
        actions.push("set `jackdaw` editor crate to opt-level 1 (avoids OOM)".into());
        manifest_changed = true;
    }

    // `[[bin]] editor`.
    let has_editor_bin = doc
        .get("bin")
        .and_then(|b| b.as_array_of_tables())
        .map(|aot| {
            aot.iter()
                .any(|t| t.get("name").and_then(|n| n.as_str()) == Some("editor"))
        })
        .unwrap_or(false);
    if !has_editor_bin {
        let bins = doc
            .entry("bin")
            .or_insert(Item::ArrayOfTables(toml_edit::ArrayOfTables::new()))
            .as_array_of_tables_mut()
            .ok_or_else(|| {
                ScaffoldError::ManifestParse("[[bin]] is not an array of tables".into())
            })?;
        let mut t = Table::new();
        t["name"] = value("editor");
        let mut rf = Array::new();
        rf.push("editor");
        t["required-features"] = value(rf);
        bins.push(t);
        actions.push("added `editor` binary target".into());
        manifest_changed = true;
    }

    // `default-run` so plain `cargo run` stays unambiguous once the editor bin
    // exists. Only when there is a default game binary (`src/main.rs`).
    if root.join("src/main.rs").is_file()
        && let Some(pkg) = doc.get_mut("package").and_then(|p| p.as_table_mut())
        && !pkg.contains_key("default-run")
    {
        pkg["default-run"] = value(&package_name);
        actions.push("set `package.default-run`".into());
        manifest_changed = true;
    }

    if manifest_changed {
        std::fs::write(&manifest_path, doc.to_string())
            .map_err(|e| ScaffoldError::Io(format!("{e}")))?;
    }

    // `src/bin/editor.rs`.
    let plugin_path = plugin_override
        .or_else(|| detect_plugin(root, &lib_name))
        .unwrap_or_else(|| format!("{lib_name}::GamePlugin"));
    let editor_rs = root.join("src/bin/editor.rs");
    if !editor_rs.exists() {
        if let Some(parent) = editor_rs.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ScaffoldError::Io(format!("{e}")))?;
        }
        std::fs::write(&editor_rs, editor_bin_source(&plugin_path))
            .map_err(|e| ScaffoldError::Io(format!("{e}")))?;
        actions.push(format!("wrote src/bin/editor.rs (links `{plugin_path}`)"));
    }

    // `jackdaw.toml`.
    let jackdaw_toml = root.join("jackdaw.toml");
    if !jackdaw_toml.exists() {
        std::fs::write(&jackdaw_toml, jackdaw_toml_source(&package_name))
            .map_err(|e| ScaffoldError::Io(format!("{e}")))?;
        actions.push("wrote jackdaw.toml".into());
    }

    // `.cargo/config.toml` aliases.
    if ensure_cargo_aliases(root)? {
        actions.push("added `cargo editor` / `cargo play` aliases".into());
    }

    // In a jackdaw source checkout, repoint the project's jackdaw deps at the
    // local workspace via path + `[patch.crates-io]`, so it builds against the
    // in-development version instead of failing to resolve an unpublished
    // crates.io version (crates.io still has the older published jackdaw).
    if crate::new_project::rewrite_jackdaw_dep_for_dev_checkout(
        root,
        crate::new_project::TemplateLinkage::Static,
    ) {
        actions.push("pointed jackdaw deps at the local dev checkout".into());
    }

    Ok(ScaffoldReport {
        actions,
        created_lib_stub,
    })
}

/// Stub `src/lib.rs` written when an existing project has no library target.
/// Holds an empty `GamePlugin` for the user to fill in.
fn lib_stub_source() -> &'static str {
    "//! Game code shared between the standalone binary and the jackdaw editor.\n\
     //!\n\
     //! Move your gameplay (systems, observers, resources) and your\n\
     //! `#[derive(Component, Reflect)]` components into `GamePlugin` so the\n\
     //! editor can discover them. Created by `jackdaw init`.\n\
     \n\
     use bevy::prelude::*;\n\
     \n\
     /// Your game's plugin. The editor links this so your components show up in\n\
     /// the inspector; the standalone binary adds it too.\n\
     #[derive(Default)]\n\
     pub struct GamePlugin;\n\
     \n\
     impl Plugin for GamePlugin {\n\
     \x20   fn build(&self, _app: &mut App) {\n\
     \x20       // TODO: register components and add systems, e.g.\n\
     \x20       // app.add_systems(Update, my_system);\n\
     \x20   }\n\
     }\n"
}

/// Ensure `[profile.dev.package.jackdaw] opt-level = 1` is present. Returns true
/// if it was added. Intermediate tables are marked implicit so an otherwise
/// profile-less manifest doesn't gain stray empty `[profile]` headers.
fn set_jackdaw_profile_opt_level(doc: &mut DocumentMut) -> Result<bool, ScaffoldError> {
    // Only mark intermediate tables implicit when we create them (so they're
    // empty and the header is safely omittable). Existing tables (which may
    // carry direct values like `[profile.dev] opt-level = 1`) are left as-is.
    let had_profile = doc.get("profile").is_some();
    let profile = doc
        .entry("profile")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| ScaffoldError::ManifestParse("[profile] is not a table".into()))?;
    if !had_profile {
        profile.set_implicit(true);
    }
    let had_dev = profile.get("dev").is_some();
    let dev = profile
        .entry("dev")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| ScaffoldError::ManifestParse("[profile.dev] is not a table".into()))?;
    if !had_dev {
        dev.set_implicit(true);
    }
    let had_package = dev.get("package").is_some();
    let package = dev
        .entry("package")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| {
            ScaffoldError::ManifestParse("[profile.dev.package] is not a table".into())
        })?;
    if !had_package {
        package.set_implicit(true);
    }
    if package.contains_key("jackdaw") {
        return Ok(false);
    }
    let mut t = Table::new();
    t["opt-level"] = value(1);
    package["jackdaw"] = Item::Table(t);
    Ok(true)
}

fn inline_dep(fields: &[(&str, toml_edit::Value)]) -> Item {
    let mut t = toml_edit::InlineTable::new();
    for (k, v) in fields {
        t.insert(*k, v.clone());
    }
    Item::Value(toml_edit::Value::InlineTable(t))
}

/// Scan `src/lib.rs` for the first `pub struct ...Plugin` to reference in the
/// generated editor binary.
fn detect_plugin(root: &Path, lib_name: &str) -> Option<String> {
    let src = std::fs::read_to_string(root.join("src/lib.rs")).ok()?;
    for line in src.lines() {
        let line = line.trim_start();
        let Some(rest) = line.strip_prefix("pub struct ") else {
            continue;
        };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if name.ends_with("Plugin") && name.len() > "Plugin".len() {
            return Some(format!("{lib_name}::{name}"));
        }
    }
    None
}

fn editor_bin_source(plugin_path: &str) -> String {
    format!(
        "//! Editor binary: links the jackdaw editor with this project's game\n\
         //! plugin so the inspector knows its component types. Generated by\n\
         //! `jackdaw init`; built with `--features editor` (try `cargo editor`).\n\
         \n\
         use bevy::prelude::*;\n\
         \n\
         fn main() -> AppExit {{\n\
         \x20   jackdaw::editor_main({plugin_path})\n\
         }}\n"
    )
}

fn jackdaw_toml_source(bin: &str) -> String {
    format!(
        "# Run configurations for the editor's Play button.\n\
         # Each [[run]] is one launchable mode. Only `bin` is required; do not\n\
         # add the `editor` feature here (that is for the editor binary only).\n\
         [[run]]\n\
         bin = \"{bin}\"\n"
    )
}

/// Ensure `.cargo/config.toml` has the `editor` and `play` aliases. Returns true
/// if anything was added.
fn ensure_cargo_aliases(root: &Path) -> Result<bool, ScaffoldError> {
    let path = root.join(".cargo/config.toml");
    let mut doc: DocumentMut = if path.exists() {
        std::fs::read_to_string(&path)
            .map_err(|e| ScaffoldError::Io(format!("{e}")))?
            .parse()
            .map_err(|e| ScaffoldError::ManifestParse(format!(".cargo/config.toml: {e}")))?
    } else {
        DocumentMut::new()
    };

    let alias = doc
        .entry("alias")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| ScaffoldError::ManifestParse("[alias] is not a table".into()))?;
    let mut changed = false;
    if !alias.contains_key("editor") {
        alias["editor"] = value("run --bin editor --features editor");
        changed = true;
    }
    if !alias.contains_key("play") {
        alias["play"] = value("run");
        changed = true;
    }

    if changed {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ScaffoldError::Io(format!("{e}")))?;
        }
        std::fs::write(&path, doc.to_string()).map_err(|e| ScaffoldError::Io(format!("{e}")))?;
    }
    Ok(changed)
}

/// CLI entry point for `jackdaw new <name>`. Scaffolds a new static-game project
/// into `<cwd>/<name>` from the embedded template.
#[expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "CLI subcommand writes its results and errors to the terminal"
)]
pub fn run_new_cli(args: &[String]) -> AppExit {
    let Some(raw_name) = args.iter().find(|a| !a.starts_with("--")) else {
        eprintln!("jackdaw new: usage: jackdaw new <name>");
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
    match scaffold_new_project(&dest, &project_name) {
        Ok(report) => {
            println!("jackdaw new: created {}", dest.display());
            for a in &report.actions {
                println!("  {a}");
            }
            println!("\nNext: cd {project_name} && cargo editor");
            AppExit::Success
        }
        Err(e) => {
            eprintln!("jackdaw new: {e}");
            AppExit::error()
        }
    }
}

/// Scaffold a new static-game project at `dest` from the embedded template,
/// substituting the project name, crate name, authors, and title placeholders.
/// Reusable by the launcher's "+ New Game" action.
pub fn scaffold_new_project(
    dest: &Path,
    project_name: &str,
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
    collect_template_files(&GAME_STATIC_TEMPLATE, &mut files);

    let mut written = 0usize;
    for file in files {
        let rel = file.path().to_string_lossy().replace('\\', "/");
        // Generation metadata, not part of the scaffolded project.
        if rel == "cargo-generate.toml" || rel == "post-generate.rhai" {
            continue;
        }

        let (dest_rel, contents): (PathBuf, Vec<u8>) = match rel.strip_suffix(".template") {
            Some(stripped) => {
                let text = std::str::from_utf8(file.contents()).map_err(|_| {
                    ScaffoldError::Io(format!("template file {rel} is not valid UTF-8"))
                })?;
                let rendered =
                    substitute_placeholders(text, project_name, &crate_name, &authors, &title);
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

    // In a jackdaw source checkout, repoint the new project's jackdaw deps at
    // the local workspace so it builds against the in-development version
    // instead of failing to resolve an unpublished crates.io version.
    crate::new_project::rewrite_jackdaw_dep_for_dev_checkout(
        dest,
        crate::new_project::TemplateLinkage::Static,
    );

    Ok(ScaffoldReport {
        actions: vec![format!("scaffolded {written} files")],
        created_lib_stub: false,
    })
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

/// Replace the template's Liquid placeholders. Matches the exact forms the
/// templates use; the longer `title_case` form is replaced before the bare one.
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

/// Sanitize a user-supplied name into a cargo-safe kebab package name.
fn sanitize_project_name(raw: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in raw.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if (c == '-' || c == '_' || c.is_whitespace()) && !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_project(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("jackdaw_scaffold_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"my-game\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nbevy = \"0.18\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub struct MyGamePlugin;\n").unwrap();
        dir
    }

    #[test]
    fn scaffolds_an_existing_project() {
        let dir = temp_project("create");
        let report = scaffold_existing_project(&dir, None).expect("scaffold succeeds");
        assert!(!report.actions.is_empty());

        let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        assert!(manifest.contains("jackdaw"), "adds jackdaw dep");
        assert!(
            manifest.contains("jackdaw_runtime/pie"),
            "editor feature pulls pie"
        );
        assert!(manifest.contains("name = \"editor\""), "adds editor bin");
        assert!(
            manifest.contains("[profile.dev.package.jackdaw]"),
            "pins editor crate opt-level to avoid OOM"
        );
        // The generated manifest must still parse as TOML (no stray headers).
        assert!(
            manifest.parse::<toml_edit::DocumentMut>().is_ok(),
            "generated Cargo.toml is valid TOML"
        );

        let editor_rs = std::fs::read_to_string(dir.join("src/bin/editor.rs")).unwrap();
        assert!(
            editor_rs.contains("my_game::MyGamePlugin"),
            "editor.rs links the detected plugin"
        );
        assert!(editor_rs.contains("jackdaw::editor_main"));

        assert!(dir.join("jackdaw.toml").is_file());
        let aliases = std::fs::read_to_string(dir.join(".cargo/config.toml")).unwrap();
        assert!(aliases.contains("--features editor"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn re_running_is_idempotent() {
        let dir = temp_project("idempotent");
        scaffold_existing_project(&dir, None).expect("first run");
        let manifest_after_first = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();

        let report = scaffold_existing_project(&dir, None).expect("second run");
        assert!(report.actions.is_empty(), "nothing to change on re-run");
        let manifest_after_second = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        assert_eq!(
            manifest_after_first, manifest_after_second,
            "manifest unchanged"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn creates_lib_stub_when_missing() {
        let dir =
            std::env::temp_dir().join(format!("jackdaw_scaffold_nolib_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"bin-only\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();

        let report =
            scaffold_existing_project(&dir, None).expect("a bin-only project gets a lib stub");
        assert!(report.created_lib_stub, "reports the stub was created");
        let lib = std::fs::read_to_string(dir.join("src/lib.rs")).expect("lib.rs created");
        assert!(lib.contains("pub struct GamePlugin"));
        // The generated editor binary links the stub plugin.
        let editor = std::fs::read_to_string(dir.join("src/bin/editor.rs")).unwrap();
        assert!(editor.contains("bin_only::GamePlugin"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_project_scaffolds_from_embedded_template() {
        let dir = std::env::temp_dir().join(format!("jackdaw_new_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let report = scaffold_new_project(&dir, "my-cool-game").expect("scaffold succeeds");
        assert!(!report.actions.is_empty());

        // `.template` files substituted and renamed to their real names.
        let cargo = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("name = \"my-cool-game\""));
        assert!(
            !cargo.contains("{{"),
            "no unsubstituted placeholders remain"
        );
        let main_rs = std::fs::read_to_string(dir.join("src/main.rs")).unwrap();
        assert!(
            main_rs.contains("my_cool_game::"),
            "crate_name substituted to snake_case"
        );
        assert!(
            std::fs::read_to_string(dir.join("src/bin/editor.rs"))
                .unwrap()
                .contains("editor_main")
        );

        // Embedded dotfiles and verbatim files come through.
        assert!(
            dir.join(".cargo/config.toml").is_file(),
            "embedded dotfiles are scaffolded"
        );
        assert!(dir.join(".gitignore").is_file());
        assert!(dir.join("assets/scene.jsn").is_file());

        // Generation metadata and leftover `.template` files are excluded.
        assert!(!dir.join("cargo-generate.toml").exists());
        assert!(!dir.join("post-generate.rhai").exists());
        assert!(!dir.join("Cargo.toml.template").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_an_existing_non_empty_dir() {
        let dir = std::env::temp_dir().join(format!("jackdaw_new_nonempty_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("existing.txt"), "hi").unwrap();
        assert!(scaffold_new_project(&dir, "thing").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn name_helpers_normalize() {
        assert_eq!(sanitize_project_name("My Cool Game"), "my-cool-game");
        assert_eq!(sanitize_project_name("  weird__name!!  "), "weird-name");
        assert_eq!(title_case("my-cool-game"), "My Cool Game");
    }
}
