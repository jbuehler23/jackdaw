//! Keeping every dependency on a registry release, apart from Bevy itself.
//!
//! Jackdaw's main branch tracks Bevy's: the bevy crates may come from
//! [`BEVY_GIT`] on its `main` branch, spelled the same way everywhere, and the
//! lockfiles pin that branch to one commit. No other manifest entry in the
//! repository, `integrations/` included, may take a dependency from git or
//! patch one in, and no module may add one to a game. An integration whose
//! upstream has no release for the current Bevy is kept and marked unavailable
//! instead.

use std::path::{Path, PathBuf};
use std::process::Command;

use jackdaw_project_build::modules::ModuleCatalog;

/// The one repository a dependency may come from by git.
pub const BEVY_GIT: &str = "https://github.com/bevyengine/bevy";

/// The branch of [`BEVY_GIT`] the bevy crates track.
pub const BEVY_BRANCH: &str = "main";

/// Check every tracked `Cargo.toml`, every tracked `Cargo.lock` and the
/// module catalog.
pub fn cmd() -> bool {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let (manifests, lockfiles) = match (
        tracked(&root, &["Cargo.toml", "**/Cargo.toml"]),
        tracked(&root, &["Cargo.lock", "**/Cargo.lock"]),
    ) {
        (Ok(manifests), Ok(lockfiles)) => (manifests, lockfiles),
        (Err(error), _) | (_, Err(error)) => {
            eprintln!("list manifests: {error}");
            return false;
        }
    };
    let mut ok = true;
    for manifest in &manifests {
        let Some(table) = read_toml(&root, manifest) else {
            ok = false;
            continue;
        };
        for problem in git_sources(&table) {
            eprintln!("{}: {problem}", manifest.display());
            ok = false;
        }
    }
    let mut pinned: Option<(String, PathBuf)> = None;
    for lockfile in &lockfiles {
        let Some(table) = read_toml(&root, lockfile) else {
            ok = false;
            continue;
        };
        match bevy_commit(&table) {
            Err(problem) => {
                eprintln!("{}: {problem}", lockfile.display());
                ok = false;
            }
            Ok(None) => {}
            Ok(Some(commit)) => match &pinned {
                Some((first, at)) if *first != commit => {
                    eprintln!(
                        "{} pins bevy to {commit}, {} to {first}",
                        lockfile.display(),
                        at.display()
                    );
                    ok = false;
                }
                Some(_) => {}
                None => pinned = Some((commit, lockfile.clone())),
            },
        }
    }
    for module in ModuleCatalog::builtin().modules() {
        for krate in module.crates.iter().filter(|krate| krate.git.is_some()) {
            eprintln!("module {} adds {} to games from git", module.id, krate.name);
            ok = false;
        }
    }
    if ok {
        let at = pinned.map_or_else(String::new, |(commit, _)| format!(" at {commit}"));
        eprintln!(
            "{} manifests and the module catalog take nothing from git but bevy {BEVY_BRANCH}{at}",
            manifests.len()
        );
    }
    ok
}

fn read_toml(root: &Path, path: &Path) -> Option<toml::Table> {
    let text = std::fs::read_to_string(root.join(path))
        .map_err(|error| eprintln!("{}: {error}", path.display()))
        .ok()?;
    toml::from_str(&text)
        .map_err(|error| eprintln!("{}: {error}", path.display()))
        .ok()
}

fn tracked(root: &Path, patterns: &[&str]) -> Result<Vec<PathBuf>, String> {
    let output = Command::new("git")
        .args(["ls-files", "-z", "--"])
        .args(patterns)
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

/// What is wrong with the git sources in `manifest`: each dependency taken
/// from git other than a bevy crate on [`BEVY_GIT`] `main`, and every crate
/// it patches.
pub fn git_sources(manifest: &toml::Table) -> Vec<String> {
    const SECTIONS: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];
    let mut tables: Vec<&toml::Table> = SECTIONS
        .iter()
        .filter_map(|section| manifest.get(*section)?.as_table())
        .collect();
    if let Some(workspace) = manifest.get("workspace").and_then(toml::Value::as_table)
        && let Some(dependencies) = workspace
            .get("dependencies")
            .and_then(toml::Value::as_table)
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
    let mut problems: Vec<String> = tables
        .into_iter()
        .flatten()
        .filter_map(|(name, spec)| {
            let spec = spec.as_table().filter(|spec| spec.contains_key("git"))?;
            (!is_tracked_bevy(name, spec)).then(|| {
                if name.starts_with("bevy") && spec.get("git").and_then(toml::Value::as_str) == Some(BEVY_GIT) {
                    format!("{name} must track bevy as {{ git = \"{BEVY_GIT}\", branch = \"{BEVY_BRANCH}\" }}")
                } else {
                    format!("{name} comes from git")
                }
            })
        })
        .collect();
    for source in manifest
        .get("patch")
        .and_then(toml::Value::as_table)
        .into_iter()
        .flat_map(toml::Table::values)
        .filter_map(toml::Value::as_table)
    {
        problems.extend(source.keys().map(|name| format!("{name} is patched")));
    }
    problems.sort();
    problems.dedup();
    problems
}

/// Whether `spec` takes the bevy crate `name` from [`BEVY_GIT`] on
/// [`BEVY_BRANCH`], with nothing else pinning or renaming it.
fn is_tracked_bevy(name: &str, spec: &toml::Table) -> bool {
    name.starts_with("bevy")
        && spec.get("git").and_then(toml::Value::as_str) == Some(BEVY_GIT)
        && spec.get("branch").and_then(toml::Value::as_str) == Some(BEVY_BRANCH)
        && !["rev", "tag", "package", "version"]
            .iter()
            .any(|key| spec.contains_key(*key))
}

/// The commit a lockfile pins the bevy crates to, `None` when it has no
/// bevy from git, or what is wrong: another git source, or bevy crates
/// split across commits or branches.
pub fn bevy_commit(lockfile: &toml::Table) -> Result<Option<String>, String> {
    let expected = format!("git+{BEVY_GIT}?branch={BEVY_BRANCH}#");
    let mut commit: Option<String> = None;
    for package in lockfile
        .get("package")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_table)
    {
        let Some(source) = package.get("source").and_then(toml::Value::as_str) else {
            continue;
        };
        if !source.starts_with("git+") {
            continue;
        }
        let name = package
            .get("name")
            .and_then(toml::Value::as_str)
            .unwrap_or_default();
        let Some(rev) = source.strip_prefix(&expected) else {
            return Err(format!("{name} resolves from {source}"));
        };
        match &commit {
            Some(first) if first != rev => {
                return Err(format!("{name} is at {rev}, other bevy crates at {first}"));
            }
            Some(_) => {}
            None => commit = Some(rev.to_string()),
        }
    }
    Ok(commit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_and_patched_dependencies_are_found_in_every_section() {
        let manifest: toml::Table = toml::from_str(
            r#"
            [dependencies]
            bevy = { git = "https://github.com/bevyengine/bevy", branch = "main" }
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
            [
                "forked comes from git",
                "helper comes from git",
                "patched is patched",
                "shared comes from git",
                "unix_only comes from git",
            ]
        );
    }

    #[test]
    fn registry_path_and_bevy_main_dependencies_pass() {
        let manifest: toml::Table = toml::from_str(
            r#"
            [dependencies]
            bevy = { git = "https://github.com/bevyengine/bevy", branch = "main", default-features = false }
            bevy_reflect = { git = "https://github.com/bevyengine/bevy", branch = "main" }
            glam = "0.33"
            local = { path = "../local" }
            "#,
        )
        .unwrap();
        assert!(git_sources(&manifest).is_empty());
    }

    #[test]
    fn bevy_pinned_any_other_way_is_rejected() {
        let manifest: toml::Table = toml::from_str(
            r#"
            [dependencies]
            bevy = { git = "https://github.com/bevyengine/bevy", rev = "abc" }
            bevy_math = { git = "https://github.com/bevyengine/bevy", branch = "release-0.20" }
            bevy_ecs = { git = "https://github.com/someone/bevy", branch = "main" }
            "#,
        )
        .unwrap();
        let problems = git_sources(&manifest);
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems[0].starts_with("bevy must track bevy"));
        assert_eq!(problems[1], "bevy_ecs comes from git");
        assert!(problems[2].starts_with("bevy_math must track bevy"));
    }

    fn lockfile(sources: &[(&str, &str)]) -> toml::Table {
        let packages: String = sources
            .iter()
            .map(|(name, source)| {
                format!(
                    "[[package]]\nname = \"{name}\"\nversion = \"0.1.0\"\nsource = \"{source}\"\n\n"
                )
            })
            .collect();
        toml::from_str(&format!("version = 4\n\n{packages}")).unwrap()
    }

    #[test]
    fn a_lockfile_pins_every_bevy_crate_to_one_commit() {
        let main = "git+https://github.com/bevyengine/bevy?branch=main#";
        let lock = lockfile(&[
            ("bevy", &format!("{main}aaaa")),
            ("bevy_ecs", &format!("{main}aaaa")),
            (
                "glam",
                "registry+https://github.com/rust-lang/crates.io-index",
            ),
        ]);
        assert_eq!(bevy_commit(&lock), Ok(Some("aaaa".to_string())));

        let split = lockfile(&[
            ("bevy", &format!("{main}aaaa")),
            ("bevy_ecs", &format!("{main}bbbb")),
        ]);
        assert!(bevy_commit(&split).is_err());

        let other = lockfile(&[("forked", "git+https://example.com/forked#cccc")]);
        assert!(bevy_commit(&other).is_err());

        let registry = lockfile(&[(
            "glam",
            "registry+https://github.com/rust-lang/crates.io-index",
        )]);
        assert_eq!(bevy_commit(&registry), Ok(None));
    }
}
