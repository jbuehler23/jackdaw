//! Resolves the per-project editor binary path and detects whether it
//! needs rebuilding before launch.
//!
//! The launcher uses these helpers to decide:
//! - Where the project's editor binary should live in the shared cache.
//! - Where the project's cdylib (the user's plugin) should live.
//! - Whether the editor binary is up-to-date relative to source code.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::cache_manager;

/// Reads the package name from the project's `Cargo.toml`. Returns
/// `None` if the manifest is missing or unparsable.
pub fn project_name(project_root: &Path) -> Option<String> {
    let manifest = project_root.join("Cargo.toml");
    let contents = std::fs::read_to_string(&manifest).ok()?;
    for line in contents.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("name") {
            let rest = rest.trim_start().strip_prefix('=')?.trim();
            let name = rest.trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace());
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// Returns the expected path to a project's per-project editor binary.
///
/// On Linux/macOS: `<cache>/debug/<project_name>_editor`.
/// On Windows:    `<cache>/debug/<project_name>_editor.exe`.
pub fn editor_binary_path(project_root: &Path) -> Option<PathBuf> {
    let name = project_name(project_root)?;
    let cache = cache_manager::target_dir();
    let bin_name = format!("{name}_editor");
    let bin_path = if cfg!(target_os = "windows") {
        cache.join("debug").join(format!("{bin_name}.exe"))
    } else {
        cache.join("debug").join(bin_name)
    };
    Some(bin_path)
}

/// Returns the expected path to a project's compiled cdylib (the
/// user's plugin), used by the hot-reload watcher.
///
/// On Linux:   `<cache>/debug/lib<project_name>.so`.
/// On macOS:   `<cache>/debug/lib<project_name>.dylib`.
/// On Windows: `<cache>/debug/<project_name>.dll`.
pub fn cdylib_path(project_root: &Path) -> Option<PathBuf> {
    let name = project_name(project_root)?;
    let cache = cache_manager::target_dir();
    let lib_name = if cfg!(target_os = "windows") {
        format!("{name}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{name}.dylib")
    } else {
        format!("lib{name}.so")
    };
    Some(cache.join("debug").join(lib_name))
}

/// `true` if the editor binary exists and is newer than every `.rs`
/// file in the project's `src/` tree. `false` means the binary is
/// missing or stale; the caller should run `cargo build` before
/// spawning it.
pub fn editor_binary_is_current(project_root: &Path) -> bool {
    let Some(bin_path) = editor_binary_path(project_root) else {
        return false;
    };
    let Ok(bin_mtime) = std::fs::metadata(&bin_path).and_then(|m| m.modified()) else {
        return false;
    };
    let src_dir = project_root.join("src");
    !any_rs_file_newer_than(&src_dir, bin_mtime)
}

fn any_rs_file_newer_than(dir: &Path, threshold: SystemTime) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if any_rs_file_newer_than(&path, threshold) {
                return true;
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs")
            && let Ok(modified) = entry.metadata().and_then(|m| m.modified())
            && modified > threshold
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_cargo_toml(dir: &Path, name: &str) {
        let mut f = std::fs::File::create(dir.join("Cargo.toml")).unwrap();
        writeln!(f, "[package]\nname = \"{name}\"\nversion = \"0.1.0\"").unwrap();
    }

    #[test]
    fn project_name_parsing_from_quoted_string() {
        let tmp = tempfile::tempdir().unwrap();
        write_cargo_toml(tmp.path(), "my_game_42");
        assert_eq!(project_name(tmp.path()).as_deref(), Some("my_game_42"));
    }

    #[test]
    fn project_name_returns_none_when_missing_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        // No Cargo.toml in the dir.
        assert!(project_name(tmp.path()).is_none());
    }

    #[test]
    fn editor_binary_path_uses_project_name() {
        let tmp = tempfile::tempdir().unwrap();
        write_cargo_toml(tmp.path(), "my_game");
        let path = editor_binary_path(tmp.path()).unwrap();
        let s = path.to_string_lossy();
        if cfg!(target_os = "windows") {
            assert!(s.contains("my_game_editor.exe"));
        } else {
            assert!(s.contains("my_game_editor"));
        }
    }

    #[test]
    fn cdylib_path_uses_platform_extension() {
        let tmp = tempfile::tempdir().unwrap();
        write_cargo_toml(tmp.path(), "my_game");
        let path = cdylib_path(tmp.path()).unwrap();
        let s = path.to_string_lossy();
        if cfg!(target_os = "windows") {
            assert!(s.contains("my_game.dll"));
        } else if cfg!(target_os = "macos") {
            assert!(s.contains("libmy_game.dylib"));
        } else {
            assert!(s.contains("libmy_game.so"));
        }
    }

    #[test]
    fn binary_is_not_current_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        write_cargo_toml(tmp.path(), "my_game");
        // Binary doesn't exist; should report not current.
        assert!(!editor_binary_is_current(tmp.path()));
    }
}
