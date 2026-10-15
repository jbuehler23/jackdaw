//! Keeping every dependency on a registry release.
//!
//! No manifest in the repository, `integrations/` included, may take a
//! dependency from git or patch one in, and no module may add one to a game.
//! An integration whose upstream has no release for the current Bevy is kept
//! and marked unavailable instead.

use std::path::{Path, PathBuf};
use std::process::Command;

use jackdaw_project_build::modules::ModuleCatalog;

/// Check every tracked `Cargo.toml` and the module catalog.
pub fn cmd() -> bool {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let manifests = match tracked_manifests(&root) {
        Ok(manifests) => manifests,
        Err(error) => {
            eprintln!("list manifests: {error}");
            return false;
        }
    };
    let mut ok = true;
    for manifest in &manifests {
        let text = match std::fs::read_to_string(root.join(manifest)) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("{}: {error}", manifest.display());
                ok = false;
                continue;
            }
        };
        let table: toml::Table = match toml::from_str(&text) {
            Ok(table) => table,
            Err(error) => {
                eprintln!("{}: {error}", manifest.display());
                ok = false;
                continue;
            }
        };
        for name in git_sources(&table) {
            eprintln!("{}: {name} comes from git", manifest.display());
            ok = false;
        }
    }
    for module in ModuleCatalog::builtin().modules() {
        for krate in module.crates.iter().filter(|krate| krate.git.is_some()) {
            eprintln!(
                "module {} adds {} to games from git",
                module.id, krate.name
            );
            ok = false;
        }
    }
    if ok {
        eprintln!(
            "{} manifests and the module catalog take no dependency from git",
            manifests.len()
        );
    }
    ok
}

fn tracked_manifests(root: &Path) -> Result<Vec<PathBuf>, String> {
    let output = Command::new("git")
        .args(["ls-files", "-z", "--", "Cargo.toml", "**/Cargo.toml"])
        .current_dir(root)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(String::from_utf8_lossy(path).into_owned()))
        .collect())
}

/// The dependencies `manifest` takes from git, and every crate it patches.
pub fn git_sources(manifest: &toml::Table) -> Vec<String> {
    const SECTIONS: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];
    let mut tables: Vec<&toml::Table> = SECTIONS
        .iter()
        .filter_map(|section| manifest.get(*section)?.as_table())
        .collect();
    if let Some(workspace) = manifest.get("workspace").and_then(toml::Value::as_table)
        && let Some(dependencies) = workspace.get("dependencies").and_then(toml::Value::as_table)
    {
        tables.push(dependencies);
    }
    for target in manifest
        .get("target")
        .and_then(toml::Value::as_table)
        .into_iter()
        .flat_map(toml::Table::values)
        .filter_map(toml::Value::as_table)
    {
        tables.extend(
            SECTIONS
                .iter()
                .filter_map(|section| target.get(*section)?.as_table()),
        );
    }
    let mut names: Vec<String> = tables
        .into_iter()
        .flatten()
        .filter(|(_, spec)| spec.as_table().is_some_and(|spec| spec.contains_key("git")))
        .map(|(name, _)| name.clone())
        .collect();
    for source in manifest
        .get("patch")
        .and_then(toml::Value::as_table)
        .into_iter()
        .flat_map(toml::Table::values)
        .filter_map(toml::Value::as_table)
    {
        names.extend(source.keys().cloned());
    }
    names.sort();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_and_patched_dependencies_are_found_in_every_section() {
        let manifest: toml::Table = toml::from_str(
            r#"
            [dependencies]
            bevy = "0.20"
            forked = { git = "https://example.com/forked" }
            local = { path = "../local" }

            [dev-dependencies]
            helper = { version = "1", git = "https://example.com/helper", rev = "abc" }

            [workspace.dependencies]
            shared = { git = "https://example.com/shared", branch = "main" }

            [target.'cfg(unix)'.build-dependencies]
            unix_only = { git = "https://example.com/unix" }

            [patch.crates-io]
            patched = { path = "../patched" }
            "#,
        )
        .unwrap();
        assert_eq!(
            git_sources(&manifest),
            ["forked", "helper", "patched", "shared", "unix_only"]
        );
    }

    #[test]
    fn registry_and_path_dependencies_pass() {
        let manifest: toml::Table = toml::from_str(
            r#"
            [dependencies]
            bevy = { version = "0.20", default-features = false }
            local = { path = "../local" }
            "#,
        )
        .unwrap();
        assert!(git_sources(&manifest).is_empty());
    }
}
