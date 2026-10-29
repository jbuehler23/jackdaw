//! The traversal guard for the scene-relative paths sidecar files are named
//! by. Stricter than any one platform: backslashes, colons, drive prefixes
//! and empty, `.` and `..` components are refused everywhere.

use std::fmt;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataPathError {
    Empty,
    /// Absolute, or with a drive or root prefix.
    Absolute,
    /// A component that is not a plain name.
    InvalidComponent,
    /// Not the exact lowercase extension the file kind takes.
    WrongExtension,
}

impl fmt::Display for DataPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "sidecar path is empty"),
            Self::Absolute => write!(f, "sidecar path must be relative to the scene"),
            Self::InvalidComponent => write!(
                f,
                "sidecar path must contain only normal forward-slash-separated components"
            ),
            Self::WrongExtension => write!(f, "sidecar path has the wrong extension"),
        }
    }
}

impl std::error::Error for DataPathError {}

/// Refuses a path that is empty, absolute, or not plain names between `/`.
pub fn check_relative(path: &str) -> Result<(), DataPathError> {
    if path.is_empty() {
        return Err(DataPathError::Empty);
    }
    if path.contains('\\') {
        return Err(DataPathError::InvalidComponent);
    }
    let rooted = Path::new(path)
        .components()
        .any(|c| matches!(c, Component::Prefix(_) | Component::RootDir));
    if Path::new(path).is_absolute() || rooted {
        return Err(DataPathError::Absolute);
    }
    for (index, part) in path.split('/').enumerate() {
        if part.is_empty() || part == "." || part == ".." {
            return Err(DataPathError::InvalidComponent);
        }
        let bytes = part.as_bytes();
        if index == 0 && bytes.get(1) == Some(&b':') && bytes[0].is_ascii_alphabetic() {
            return Err(DataPathError::Absolute);
        }
        if part.contains(':') {
            return Err(DataPathError::InvalidComponent);
        }
    }
    Ok(())
}

/// Checks `path` and its `extension`, then joins it to `base`.
pub fn resolve(base: &Path, path: &str, extension: &str) -> Result<PathBuf, DataPathError> {
    check_relative(path)?;
    if Path::new(path).extension().and_then(|e| e.to_str()) != Some(extension) {
        return Err(DataPathError::WrongExtension);
    }
    Ok(base.join(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_plain_relative_paths() {
        assert_eq!(
            resolve(Path::new("levels"), "data/a.jdtiles", "jdtiles"),
            Ok(PathBuf::from("levels/data/a.jdtiles"))
        );
    }

    #[test]
    fn refuses_paths_that_leave_the_base() {
        let base = Path::new("levels");
        assert_eq!(resolve(base, "", "x"), Err(DataPathError::Empty));
        for bad in ["/etc/a.x", "C:/a.x", "C:a.x"] {
            assert_eq!(
                resolve(base, bad, "x"),
                Err(DataPathError::Absolute),
                "{bad}"
            );
        }
        for bad in ["../a.x", "a/./b.x", "a//b.x", "a\\b.x", "a/c:d.x"] {
            assert_eq!(
                resolve(base, bad, "x"),
                Err(DataPathError::InvalidComponent),
                "{bad}"
            );
        }
        assert_eq!(
            resolve(base, "a.X", "x"),
            Err(DataPathError::WrongExtension)
        );
    }
}
