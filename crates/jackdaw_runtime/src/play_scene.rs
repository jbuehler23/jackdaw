use std::path::Path;

use bevy::asset::AssetPath;
use bevy::prelude::*;

use crate::JackdawSceneRoot;

/// Under the editor's Play, the scene open in the editor loads in place of
/// the game's main scene: every [`JackdawSceneRoot`] asking for `replaces`
/// loads `scene` instead, and every other root is left alone.
///
/// [`JackdawPlugin`](crate::JackdawPlugin) inserts it when the editor names
/// both scenes, and only then watches scene roots; a resource inserted before
/// the plugin is added works the same way.
#[derive(Resource, Clone, Debug)]
pub struct PieSceneOverride {
    scene: AssetPath<'static>,
    replaces: AssetPath<'static>,
}

impl PieSceneOverride {
    /// Load `scene` wherever the game loads `replaces`, both paths inside the
    /// asset folder.
    pub fn new(scene: impl AsRef<Path>, replaces: impl AsRef<Path>) -> Self {
        Self {
            scene: AssetPath::from_path(scene.as_ref()).into_owned(),
            replaces: AssetPath::from_path(replaces.as_ref()).into_owned(),
        }
    }
}

/// The override the editor asks for under Play (`JACKDAW_PIE` set): the open
/// scene `JACKDAW_PIE_SCENE` in place of the main scene `JACKDAW_PIE_REPLACES`.
#[cfg(feature = "pie")]
pub(crate) fn override_from_env() -> Option<PieSceneOverride> {
    std::env::var_os("JACKDAW_PIE")?;
    let scene = std::env::var("JACKDAW_PIE_SCENE").ok()?;
    let replaces = std::env::var("JACKDAW_PIE_REPLACES").ok()?;
    if scene.is_empty() || replaces.is_empty() {
        return None;
    }
    Some(PieSceneOverride::new(scene, replaces))
}

pub(crate) fn redirect_scene_root(
    insert: On<Insert<JackdawSceneRoot>>,
    mut roots: Query<&mut JackdawSceneRoot>,
    play_scene: Res<PieSceneOverride>,
    asset_server: Res<AssetServer>,
) -> Result {
    let mut root = roots.get_mut(insert.entity)?;
    if root.0.path() != Some(&play_scene.replaces) {
        return Ok(());
    }
    info!(
        "Play: loading {} in place of {}",
        play_scene.scene, play_scene.replaces
    );
    root.0 = asset_server.load(play_scene.scene.clone());
    Ok(())
}
