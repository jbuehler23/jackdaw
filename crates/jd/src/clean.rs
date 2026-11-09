//! `jd clean`: remove a project's Jackdaw build cache.

#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "command-line output goes to the terminal"
)]

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Run `jd clean [--cargo]`.
pub fn clean_command(args: &[String]) -> ExitCode {
    let cargo = args.iter().any(|arg| arg == "--cargo");
    match clean_inner(cargo) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("jd clean: {error}");
            ExitCode::FAILURE
        }
    }
}

fn clean_inner(cargo: bool) -> Result<(), String> {
    let here = std::env::current_dir().map_err(|error| error.to_string())?;
    let root = find_project_root(&here).ok_or(
        "not in a Jackdaw project (no jackdaw.toml here or above); \
         `jd editor clean` removes unused nightly editors",
    )?;
    match remove_project_target(&root)? {
        Some(freed) => println!("removed .jackdaw/target, freed {}", human_size(freed)),
        None => println!("no .jackdaw/target to remove"),
    }
    if cargo {
        let status = jackdaw_env::user_toolchain_command("cargo")
            .arg("clean")
            .current_dir(&root)
            .status()
            .map_err(|error| format!("cannot run cargo clean: {error}"))?;
        if !status.success() {
            return Err(format!("cargo clean failed: {status}"));
        }
    }
    Ok(())
}

fn find_project_root(from: &Path) -> Option<PathBuf> {
    from.ancestors()
        .find(|dir| dir.join("jackdaw.toml").is_file())
        .map(Path::to_path_buf)
}

/// Delete `<root>/.jackdaw/target`, leaving the rest of `.jackdaw/`.
/// Returns the bytes freed, or `None` when there was nothing to remove.
/// Refused while a cargo build holds a lock inside the target.
fn remove_project_target(root: &Path) -> Result<Option<u64>, String> {
    let target = root.join(".jackdaw").join("target");
    if !target.is_dir() {
        return Ok(None);
    }
    if build_running(&target, 0) {
        return Err("a build is running for this project; close the editor first".into());
    }
    let size = dir_size(&target);
    std::fs::remove_dir_all(&target).map_err(|error| format!("{}: {error}", target.display()))?;
    Ok(Some(size))
}

/// Whether any `.cargo-lock` under `dir` (target/<build>/<profile>/) is
/// held, which cargo does for the length of a build.
fn build_running(dir: &Path, depth: usize) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            return false;
        };
        if kind.is_dir() {
            depth < 4 && build_running(&path, depth + 1)
        } else if entry.file_name() == ".cargo-lock" {
            OpenOptions::new()
                .write(true)
                .open(&path)
                .is_ok_and(|file| matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock)))
        } else {
            false
        }
    })
}

fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => dir_size(&entry.path()),
            Ok(_) => entry.metadata().map(|meta| meta.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_clean_removes_only_the_target_and_reports_its_size() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let jackdaw = root.join(".jackdaw");
        std::fs::create_dir_all(jackdaw.join("target/game/debug")).unwrap();
        std::fs::create_dir_all(jackdaw.join("target/abc")).unwrap();
        std::fs::create_dir_all(jackdaw.join("thumbnails")).unwrap();
        std::fs::write(jackdaw.join("target/game/debug/a.rlib"), vec![0u8; 1500]).unwrap();
        std::fs::write(jackdaw.join("target/abc/b.rlib"), vec![0u8; 500]).unwrap();
        for file in [
            "settings.json",
            "schema.json",
            "editor.json",
            "project.json",
        ] {
            std::fs::write(jackdaw.join(file), "{}").unwrap();
        }
        std::fs::write(jackdaw.join("thumbnails/x.png"), "png").unwrap();

        assert_eq!(remove_project_target(root).unwrap(), Some(2000));
        assert!(!jackdaw.join("target").exists());
        for file in [
            "settings.json",
            "schema.json",
            "editor.json",
            "project.json",
            "thumbnails/x.png",
        ] {
            assert!(jackdaw.join(file).is_file(), "{file} is kept");
        }
        assert_eq!(remove_project_target(root).unwrap(), None);
    }

    #[test]
    fn project_clean_is_refused_while_a_build_holds_the_cargo_lock() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join(".jackdaw/target/game/debug");
        std::fs::create_dir_all(&profile).unwrap();
        let lock = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(profile.join(".cargo-lock"))
            .unwrap();
        lock.lock().unwrap();

        let error = remove_project_target(dir.path()).unwrap_err();
        assert!(error.contains("a build is running"));
        assert!(profile.is_dir());

        drop(lock);
        assert!(remove_project_target(dir.path()).unwrap().is_some());
    }

    #[test]
    fn project_root_is_found_from_a_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(find_project_root(&nested), None);
        std::fs::write(dir.path().join("jackdaw.toml"), "").unwrap();
        assert_eq!(find_project_root(&nested), Some(dir.path().to_path_buf()));
    }

    #[test]
    fn sizes_are_human_readable() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(23_400_000_000), "23.4 GB");
    }
}
