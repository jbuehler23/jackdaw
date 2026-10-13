//! `jd self update`: replace a standalone `jd` with the newest stable one.

use std::path::{Path, PathBuf};

use jackdaw_releases::installs::EDITOR_EXECUTABLE;
use jackdaw_releases::{Layout, ReleaseIndex, version};

use crate::archive;
use crate::editors;
use crate::fetch;

/// What `jd self update` did, or why it left `jd` alone.
#[derive(Debug, PartialEq, Eq)]
pub enum SelfUpdate {
    /// This `jd` came from `cargo install`, which updates it instead.
    CargoInstalled,
    /// This `jd` is part of a managed editor install of this version.
    PartOfInstall(String),
    /// This `jd` sits in an editor bundle, such as one the install script
    /// unpacked, and must stay the same version as that editor.
    InBundle(PathBuf),
    /// This `jd` was built in a cargo `target/` directory.
    DevelopmentBuild,
    /// This `jd` is a prerelease or development version.
    Prerelease(String),
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

fn canonical(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Why the `jd` at `executable`, of version `current`, must not replace
/// itself, or `None` when it may.
pub fn refusal(
    executable: &Path,
    current: &str,
    layout: &Layout,
    cargo_bin: Option<&Path>,
) -> Option<SelfUpdate> {
    let dir = canonical(executable.parent()?);
    if cargo_bin.is_some_and(|bin| canonical(bin) == dir) {
        return Some(SelfUpdate::CargoInstalled);
    }
    if let Ok(inside) = dir.strip_prefix(canonical(&layout.editors()))
        && let Some(install) = inside.components().next()
    {
        return Some(SelfUpdate::PartOfInstall(
            install.as_os_str().to_string_lossy().into_owned(),
        ));
    }
    if dir.join(EDITOR_EXECUTABLE).exists() || dir.join("sdk").join("manifest.txt").exists() {
        return Some(SelfUpdate::InBundle(dir));
    }
    if dir
        .components()
        .any(|component| component.as_os_str() == "target")
    {
        return Some(SelfUpdate::DevelopmentBuild);
    }
    match semver::Version::parse(current) {
        Ok(parsed) if parsed.pre.is_empty() => None,
        _ => Some(SelfUpdate::Prerelease(current.to_string())),
    }
}

/// Replace `executable` (a `jd` of `current` version) with the `jd` from
/// the newest stable release for `target`, after checking its size and
/// checksum and that it runs and reports that release.
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
        let sha256 = fetch::download(&artifact.url, &download, artifact.size)?;
        if !sha256.eq_ignore_ascii_case(&artifact.sha256) {
            return Err(format!("{}: checksum mismatch", artifact.url));
        }
        let unpacked = staging.join("unpacked");
        std::fs::create_dir(&unpacked)
            .map_err(|error| format!("{}: {error}", unpacked.display()))?;
        archive::unpack(&download, format, &unpacked)?;
        let fresh = archive::bundle_root(&unpacked)?.join(jd_name());
        if !fresh.is_file() {
            return Err(format!("the {} bundle holds no jd", release.version));
        }
        let reported = editors::version_output(&fresh)?;
        let expected = format!("jd {}", release.version);
        if reported
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ")
            != expected
        {
            return Err(format!(
                "the new jd reports `{}`, not {expected}",
                reported.trim()
            ));
        }
        let old = dir.join(format!("{}.old", jd_name()));
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

    fn standalone(dir: &Path) -> PathBuf {
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        bin.join("jd")
    }

    #[test]
    fn a_standalone_release_jd_may_update_itself() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::new(dir.path().join("launcher"));
        assert_eq!(
            refusal(&standalone(dir.path()), "0.20.1", &layout, None),
            None
        );
    }

    #[test]
    fn a_cargo_installed_jd_is_left_to_cargo() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("cargo/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let layout = Layout::new(dir.path().join("launcher"));
        assert_eq!(
            refusal(&bin.join("jd"), "0.20.1", &layout, Some(&bin)),
            Some(SelfUpdate::CargoInstalled)
        );
    }

    #[test]
    fn a_jd_inside_a_managed_install_names_that_install() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::new(dir.path());
        let install = layout.editor("0.20.1");
        std::fs::create_dir_all(&install).unwrap();
        assert_eq!(
            refusal(&install.join("jd"), "0.20.1", &layout, None),
            Some(SelfUpdate::PartOfInstall("0.20.1".into()))
        );
    }

    #[test]
    fn a_jd_beside_its_editor_or_sdk_stays_with_that_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::new(dir.path().join("launcher"));
        let jd = standalone(dir.path());
        std::fs::write(jd.with_file_name(EDITOR_EXECUTABLE), b"").unwrap();
        assert!(matches!(
            refusal(&jd, "0.20.1", &layout, None),
            Some(SelfUpdate::InBundle(_))
        ));

        let other = dir.path().join("install/0.20.1");
        std::fs::create_dir_all(other.join("sdk")).unwrap();
        std::fs::write(other.join("sdk/manifest.txt"), b"").unwrap();
        assert!(matches!(
            refusal(&other.join("jd"), "0.20.1", &layout, None),
            Some(SelfUpdate::InBundle(_))
        ));
    }

    #[test]
    fn development_and_prerelease_builds_are_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::new(dir.path().join("launcher"));
        let built = dir.path().join("target/debug");
        std::fs::create_dir_all(&built).unwrap();
        assert_eq!(
            refusal(&built.join("jd"), "0.20.1", &layout, None),
            Some(SelfUpdate::DevelopmentBuild)
        );
        assert_eq!(
            refusal(&standalone(dir.path()), "0.21.0-rc.1", &layout, None),
            Some(SelfUpdate::Prerelease("0.21.0-rc.1".into()))
        );
    }
}
