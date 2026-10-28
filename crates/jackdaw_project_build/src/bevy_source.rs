//! Where a project gets the Bevy this build was made against.
//!
//! A release targets a Bevy minor from crates.io, and a project asks for that
//! minor. A build that tracks Bevy's main branch asks for the branch, the way
//! every jackdaw crate does, and pins the commit in the project's lockfile:
//! the editor's types and its SDK are built from [`BEVY_REV`], so a project
//! has to resolve exactly that commit. Cargo allows no `rev` beside the
//! branch the jackdaw crates name; it would be a second copy of Bevy.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use jackdaw_env::user_toolchain_command;

use crate::{BEVY_REV, BEVY_VERSION};

/// The repository Bevy's main branch is taken from.
pub const BEVY_GIT: &str = "https://github.com/bevyengine/bevy";

/// The branch a build that tracks Bevy takes it from.
pub const BEVY_BRANCH: &str = "main";

/// The body of the inline table a project's `bevy` dependency states, before
/// any `features` or `default-features`: `version = "0.20"`, or the git
/// branch.
pub fn requirement() -> String {
    match BEVY_REV {
        Some(_) => format!("git = \"{BEVY_GIT}\", branch = \"{BEVY_BRANCH}\""),
        None => format!("version = \"{BEVY_VERSION}\""),
    }
}

/// The value of a project's `bevy` dependency: `"0.20"`, or the git branch
/// as an inline table.
pub fn dependency() -> String {
    match BEVY_REV {
        Some(_) => format!("{{ {} }}", requirement()),
        None => format!("\"{BEVY_VERSION}\""),
    }
}

/// The commit `lockfile` pins the `bevy` package's git source to.
pub fn locked_rev(lockfile: &str) -> Option<String> {
    let table: toml::Table = toml::from_str(lockfile).ok()?;
    table
        .get("package")?
        .as_array()?
        .iter()
        .filter_map(toml::Value::as_table)
        .filter(|package| package.get("name").and_then(toml::Value::as_str) == Some("bevy"))
        .find_map(|package| {
            let source = package.get("source")?.as_str()?.strip_prefix("git+")?;
            let (_, rev) = source.rsplit_once('#')?;
            Some(rev.to_string())
        })
}

/// Make the project at `root` resolve Bevy at [`BEVY_REV`], moving its
/// lockfile with `cargo update --precise` when it names another commit or
/// does not exist yet. Returns what it did, for the build log; `Ok(None)` for
/// a build on a Bevy release, a project that takes Bevy from the registry,
/// and one already at the commit.
pub fn pin_project(root: &Path) -> Result<Option<String>, String> {
    let Some(rev) = BEVY_REV else {
        return Ok(None);
    };
    let lockfile = workspace_lockfile(root)?;
    let previous = match std::fs::read_to_string(&lockfile) {
        Ok(text) => match locked_rev(&text) {
            Some(locked) if locked == rev => return Ok(None),
            Some(locked) => Some(locked),
            None if text.contains("name = \"bevy\"") => return Ok(None),
            None => None,
        },
        Err(_) => None,
    };
    let mut command = user_toolchain_command("cargo");
    command
        .args(["update", "--package", "bevy", "--precise", rev])
        .current_dir(root)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    crate::detach_from_host_build(&mut command);
    let output = command
        .output()
        .map_err(|error| format!("could not run cargo: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "could not pin bevy to {rev}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(Some(match previous {
        Some(previous) => {
            format!("moved bevy from {previous} to {rev}, the commit this editor is built against")
        }
        None => format!("pinned bevy to {rev}, the commit this editor is built against"),
    }))
}

/// The lockfile of the workspace `root` belongs to.
fn workspace_lockfile(root: &Path) -> Result<PathBuf, String> {
    let output = user_toolchain_command("cargo")
        .args(["locate-project", "--workspace", "--message-format", "plain"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("could not run cargo: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let manifest = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    Ok(manifest.with_file_name("Cargo.lock"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAIN: &str = "git+https://github.com/bevyengine/bevy?branch=main#";

    #[test]
    fn the_locked_commit_is_read_from_the_bevy_package() {
        let lockfile = format!(
            "version = 4\n\n\
             [[package]]\nname = \"bevy_ecs\"\nversion = \"0.20.0-dev\"\nsource = \"{MAIN}bbbb\"\n\n\
             [[package]]\nname = \"bevy\"\nversion = \"0.20.0-dev\"\nsource = \"{MAIN}aaaa\"\n"
        );
        assert_eq!(locked_rev(&lockfile).as_deref(), Some("aaaa"));
    }

    #[test]
    fn a_registry_bevy_has_no_locked_commit() {
        let lockfile = "version = 4\n\n[[package]]\nname = \"bevy\"\nversion = \"0.20.0\"\n\
                        source = \"registry+https://github.com/rust-lang/crates.io-index\"\n";
        assert_eq!(locked_rev(lockfile), None);
    }

    #[test]
    fn a_project_asks_for_bevy_where_this_build_got_it() {
        let dependency: toml::Value =
            toml::from_str(&format!("bevy = {}", dependency())).expect("a TOML value");
        let bevy = &dependency["bevy"];
        match BEVY_REV {
            Some(_) => {
                assert_eq!(bevy["git"].as_str(), Some(BEVY_GIT));
                assert_eq!(bevy["branch"].as_str(), Some(BEVY_BRANCH));
            }
            None => assert_eq!(bevy.as_str(), Some(BEVY_VERSION)),
        }
        let table: toml::Value = toml::from_str(&format!(
            "bevy = {{ {}, default-features = false }}",
            requirement()
        ))
        .expect("an inline table body");
        assert_eq!(table["bevy"]["default-features"].as_bool(), Some(false));
    }
}
