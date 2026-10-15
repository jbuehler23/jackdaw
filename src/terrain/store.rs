//! The terrain store, and the scene stem a new terrain's sidecar is named after.

use bevy::prelude::*;

pub use jackdaw_terrain::store::*;

/// File stem of the scene currently being saved or edited, for naming a
/// new terrain's sidecar. Falls back to `untitled` for unsaved scenes.
pub fn active_scene_stem(world: &World) -> String {
    world
        .get_resource::<crate::scene_io::SceneFilePath>()
        .and_then(|scene| scene.path.as_ref())
        .and_then(|path| {
            std::path::Path::new(path)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "untitled".to_string())
}
