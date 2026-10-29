//! Where a tilemap's `.jdtiles` sidecar lives: a slash-separated path
//! relative to the document that names it.

use std::path::{Path, PathBuf};

pub use jackdaw_data_path::DataPathError;

/// File extension of a tilemap sidecar.
pub const EXTENSION: &str = "jdtiles";

/// Validates `data_path` and joins it to `base`, the folder of the document
/// that names it (or that folder under a game's assets root).
pub fn resolve_path(base: &Path, data_path: &str) -> Result<PathBuf, DataPathError> {
    jackdaw_data_path::resolve(base, data_path, EXTENSION)
}

/// A fresh data path beside the scene `scene_stem` names,
/// `<stem>.tilemap-N.jdtiles` with the lowest `N` that `taken` refuses.
pub fn mint_data_path(scene_stem: &str, taken: impl Fn(&str) -> bool) -> String {
    let stem = if scene_stem.is_empty() {
        "untitled"
    } else {
        scene_stem
    };
    (0u32..)
        .map(|n| format!("{stem}.tilemap-{n}.{EXTENSION}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_beneath_the_scene_folder() {
        assert_eq!(
            resolve_path(Path::new("levels"), "forest.tilemap-0.jdtiles"),
            Ok(PathBuf::from("levels/forest.tilemap-0.jdtiles"))
        );
        assert_eq!(
            resolve_path(Path::new("levels"), "../a.jdtiles"),
            Err(DataPathError::InvalidComponent)
        );
        assert_eq!(
            resolve_path(Path::new("levels"), "a.jdterrain"),
            Err(DataPathError::WrongExtension)
        );
    }

    #[test]
    fn mints_the_lowest_free_name_beside_the_scene() {
        assert_eq!(
            mint_data_path("forest", |_| false),
            "forest.tilemap-0.jdtiles"
        );
        assert_eq!(
            mint_data_path("forest", |p| p == "forest.tilemap-0.jdtiles"),
            "forest.tilemap-1.jdtiles"
        );
        assert_eq!(mint_data_path("", |_| false), "untitled.tilemap-0.jdtiles");
        assert!(resolve_path(Path::new(""), &mint_data_path("forest", |_| false)).is_ok());
    }
}
