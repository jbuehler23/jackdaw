//! `jd self update`: replace the running `jd` with the newest stable one.

use std::path::{Path, PathBuf};

use jackdaw_releases::{Layout, ReleaseIndex, version};

use crate::archive;
use crate::fetch;

/// What `jd self update` did or would do.
#[derive(Debug, PartialEq, Eq)]
pub enum SelfUpdate {
    /// This `jd` came from `cargo install`, which updates it instead.
    CargoInstalled,
    /// This `jd` is part of a managed editor install of this version.
    PartOfInstall(String),
    UpToDate(String),
    Updated {
        from: String,
        to: String,
    },
}

/// Cargo's binary directory: `$CARGO_HOME/bin`, else `~/.cargo/bin`.
pub fn cargo_bin_dir() -> Option<PathBuf> {
    std::env::var_os("CARGO_HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|home| home.join(".cargo")))
        .map(|home| home.join("bin"))
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (dunce::canonicalize(a), dunce::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// How the `jd` at `executable` is updated, before anything is fetched.
pub fn installed_by(
    executable: &Path,
    layout: &Layout,
    cargo_bin: Option<&Path>,
) -> Option<SelfUpdate> {
    let dir = executable.parent()?;
    if cargo_bin.is_some_and(|bin| same_dir(dir, bin)) {
        return Some(SelfUpdate::CargoInstalled);
    }
    let editors = dunce::canonicalize(layout.editors()).unwrap_or_else(|_| layout.editors());
    let dir = dunce::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let install = dir.strip_prefix(&editors).ok()?.components().next()?;
    Some(SelfUpdate::PartOfInstall(
        install.as_os_str().to_string_lossy().into_owned(),
    ))
}

/// Replace `executable` (a `jd` of `current` version) with the `jd` from
/// the newest stable release for `target`.
pub fn update(
    executable: &Path,
    current: &str,
    index: &ReleaseIndex,
    target: &str,
) -> Result<SelfUpdate, String> {
    let release = index.resolve("stable")?;
    if version::compare(&release.version, current).is_le() {
        return Ok(SelfUpdate::UpToDate(current.to_string()));
    }
    let artifact = release
        .artifact_for(target)
        .ok_or_else(|| format!("{} has no build for {target}", release.version))?;
    let format = archive::Format::of(&artifact.url)
        .ok_or_else(|| format!("{}: not a known bundle format", artifact.url))?;
    let dir = executable
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", executable.display()))?;
    let staging = dir.join(format!(".jd-update-{}", std::process::id()));
    let result = (|| {
        std::fs::create_dir_all(&staging)
            .map_err(|error| format!("{}: {error}", staging.display()))?;
        let download = staging.join("bundle");
        let (_, sha256) = fetch::download(&artifact.url, &download)?;
        if !sha256.eq_ignore_ascii_case(&artifact.sha256) {
            return Err(format!("{}: checksum mismatch", artifact.url));
        }
        let unpacked = staging.join("unpacked");
        std::fs::create_dir(&unpacked)
            .map_err(|error| format!("{}: {error}", unpacked.display()))?;
        archive::unpack(&download, format, &unpacked)?;
        let name = executable.file_name().unwrap_or_default();
        let fresh = archive::bundle_root(&unpacked)?.join(jd_name());
        if !fresh.is_file() {
            return Err(format!("the {} bundle holds no jd", release.version));
        }
        let old = dir.join(format!("{}.old", name.to_string_lossy()));
        let _ = std::fs::remove_file(&old);
        std::fs::rename(executable, &old)
            .map_err(|error| format!("{}: {error}", executable.display()))?;
        if let Err(error) = std::fs::rename(&fresh, executable) {
            let _ = std::fs::rename(&old, executable);
            return Err(format!("{}: {error}", executable.display()));
        }
        let _ = std::fs::remove_file(&old);
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result?;
    Ok(SelfUpdate::Updated {
        from: current.to_string(),
        to: release.version.clone(),
    })
}

fn jd_name() -> &'static str {
    if cfg!(windows) { "jd.exe" } else { "jd" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cargo_installed_jd_is_left_to_cargo() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("cargo/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let layout = Layout::new(dir.path().join("launcher"));
        assert_eq!(
            installed_by(&bin.join("jd"), &layout, Some(&bin)),
            Some(SelfUpdate::CargoInstalled)
        );
        assert_eq!(
            installed_by(&dir.path().join("elsewhere/jd"), &layout, Some(&bin)),
            None
        );
    }

    #[test]
    fn a_jd_inside_a_managed_install_names_that_install() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::new(dir.path());
        let install = layout.editor("0.20.1");
        std::fs::create_dir_all(&install).unwrap();
        assert_eq!(
            installed_by(&install.join("jd"), &layout, None),
            Some(SelfUpdate::PartOfInstall("0.20.1".into()))
        );
    }
}
