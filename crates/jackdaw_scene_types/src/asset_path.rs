//! The one rule for turning an authored file path into a Bevy asset path.
//!
//! The editor and the standalone runtime both derive render state from paths a
//! scene authored, and a path that resolved to two different files depending on
//! which of them opened the scene would be a scene that renders differently in
//! the game than it did while it was being made.

use std::path::Path;

use bevy::prelude::*;
use path_slash::PathExt as _;

/// Resolve an authored path against the assets directory.
///
/// Authored paths are relative to the assets root, never to the file that
/// named them: a prefab stored under `prefabs/` and a scene at the root both
/// reach the same model by writing `characters/fox.gltf`.
///
/// `assets_dir` is the caller's absolute assets directory, when it knows one.
/// An absolute input -- what scenes authored before paths were normalised
/// still hold -- has that directory stripped off it, so the load goes through
/// Bevy's approved-path machinery rather than needing
/// `UnapprovedPathMode::Allow`. An absolute path outside the assets directory
/// warns and comes back unchanged; callers should not rely on it loading.
pub fn to_asset_path(path: &str, assets_dir: Option<&Path>) -> String {
    let path = dunce::simplified(Path::new(path));
    if let Some(assets_dir) = assets_dir
        && let Ok(relative) = path.strip_prefix(dunce::simplified(assets_dir))
    {
        return relative.to_slash_lossy().into_owned();
    }
    if !path.is_absolute() {
        return path.to_slash_lossy().into_owned();
    }
    warn!(
        "Cannot load '{}': file is outside the assets directory. \
         Move it into your project's assets/ folder.",
        path.display()
    );
    path.to_slash_lossy().into_owned()
}

/// `spelled` with `/` between its folders, as asset paths are written on
/// every platform. A path Bevy or `std` joined on Windows separates with `\`;
/// elsewhere a backslash is part of a name and is kept.
pub fn slash_separated(spelled: &str) -> String {
    if cfg!(windows) {
        spelled.replace('\\', "/")
    } else {
        spelled.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_joined_path_is_spelled_with_forward_slashes() {
        let joined = Path::new("models").join("tree_LOD1.gltf");
        assert_eq!(
            slash_separated(&joined.to_string_lossy()),
            "models/tree_LOD1.gltf"
        );
        let backslashed = slash_separated("models\\tree.gltf");
        if cfg!(windows) {
            assert_eq!(backslashed, "models/tree.gltf");
        } else {
            assert_eq!(backslashed, "models\\tree.gltf");
        }
    }

    #[test]
    fn a_relative_path_is_read_from_the_assets_root_whatever_named_it() {
        assert_eq!(
            to_asset_path("characters/fox.gltf", Some(Path::new("/project/assets"))),
            "characters/fox.gltf"
        );
    }

    #[test]
    fn an_absolute_path_inside_the_assets_dir_loses_that_prefix() {
        assert_eq!(
            to_asset_path(
                "/project/assets/characters/fox.gltf",
                Some(Path::new("/project/assets"))
            ),
            "characters/fox.gltf"
        );
    }

    #[test]
    fn a_relative_path_survives_an_unknown_assets_dir() {
        assert_eq!(
            to_asset_path("characters/fox.gltf", None),
            "characters/fox.gltf"
        );
    }
}
