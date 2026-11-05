//! What a game under Play reads in place of files that lag the editor.
//!
//! The editor sends [`ControlEvent::PrefabFiles`] and names a folder of sidecars
//! in its answer to a scene request. Nothing else creates the overlay, so a
//! game with no editor link reads only its files. Every path goes through
//! the checks a file gets: a prefab must be inside the asset root, and the sidecar
//! folder must be `.jackdaw/play` of a project holding the asset root.
//!
//! [`ControlEvent::PrefabFiles`]: jackdaw_pie_protocol::ControlEvent::PrefabFiles

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw_bsn::SceneBsnAst;

/// What the editor told this game to read in place of its files.
#[derive(Resource, Default)]
struct PlayOverlay {
    /// Unsaved prefab text, by the prefab's normalized path in its text form.
    prefabs: HashMap<PathBuf, String>,
    /// The folder of unsaved sidecars, laid out like the asset folder.
    sidecars: Option<PathBuf>,
}

/// Take prefab documents from the editor, as `(path, text)` under the asset
/// folder. The embedded assets of a text load now, so edits the editor sends from
/// here on can name them.
pub fn load_prefab_files(world: &mut World, files: &[(String, Option<String>)]) {
    let Some(root) = crate::assets_root(world).map(|root| jackdaw_prefab::normalize_path(&root))
    else {
        warn!("PIE: no asset root found; the editor's unsaved prefabs are ignored");
        return;
    };
    for (path, text) in files {
        let full = jackdaw_prefab::normalize_path(&root.join(path));
        if !full.starts_with(&root) {
            warn!("PIE: ignoring the prefab {path} the editor sent: it is outside the asset root");
            continue;
        }
        let key = jackdaw_bsn::text_twin(&full);
        match text {
            Some(text) => {
                crate::node_sync::load_scene_assets(world, text);
                world
                    .get_resource_or_init::<PlayOverlay>()
                    .prefabs
                    .insert(key, text.clone());
            }
            None => {
                world
                    .get_resource_or_init::<PlayOverlay>()
                    .prefabs
                    .remove(&key);
            }
        }
    }
}

/// Take the folder of unsaved sidecars the editor named. Refused, with a warning,
/// unless it is `.jackdaw/play` of a folder holding the asset root.
pub fn note_sidecar_overlay(world: &mut World, named: &str) {
    let folder = jackdaw_prefab::normalize_path(Path::new(named));
    let project = folder
        .ends_with(Path::new(".jackdaw").join("play"))
        .then(|| folder.parent().and_then(Path::parent))
        .flatten();
    let holds_assets = project
        .zip(crate::assets_root(world))
        .is_some_and(|(project, assets)| {
            real_path(&jackdaw_prefab::normalize_path(&assets)).starts_with(real_path(project))
        });
    if !holds_assets {
        warn!("PIE: ignoring the sidecar folder {named} the editor named");
        return;
    }
    world.get_resource_or_init::<PlayOverlay>().sidecars = Some(folder);
}

fn real_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The document the editor sent for the prefab at `path`, ready to instance.
///
/// Its asset roots go: they loaded when the text arrived, and the handles made
/// then are the ones later definitions write into. A text that does not parse
/// falls back to the file.
pub(crate) fn overlaid_prefab(
    world: &World,
    path: &Path,
    assets_root: &Path,
) -> Option<SceneBsnAst> {
    let overlay = world.get_resource::<PlayOverlay>()?;
    let text = overlay.prefabs.get(&jackdaw_bsn::text_twin(path))?;
    let mut ast = match crate::parse_bsn_text(text) {
        Ok(ast) => ast,
        Err(err) => {
            warn!(
                "PIE: the unsaved prefab {} does not parse, using its file: {err}",
                path.display()
            );
            return None;
        }
    };
    let registry = world.resource::<AppTypeRegistry>().read();
    let roots: Vec<Entity> = ast
        .roots
        .iter()
        .copied()
        .filter(|&root| !crate::is_asset_root(&ast, root, &registry))
        .collect();
    ast.roots = roots;
    jackdaw_prefab::prepare_prefab_document(&mut ast, path, assets_root);
    Some(ast)
}

/// The folder holding an unsaved copy of the sidecar `data_path` names beside the
/// document in `dir`, if there is a copy.
#[cfg(any(feature = "terrain", feature = "tilemap"))]
pub(crate) fn overlaid_sidecar_dir(
    world: &World,
    root: &Path,
    dir: &Path,
    data_path: &str,
) -> Option<PathBuf> {
    use std::path::Component;

    let overlay = world.get_resource::<PlayOverlay>()?.sidecars.as_ref()?;
    let names_a_file = !data_path.is_empty()
        && Path::new(data_path)
            .components()
            .all(|part| matches!(part, Component::Normal(_)));
    if !names_a_file {
        return None;
    }
    let under = jackdaw_prefab::normalize_path(dir);
    let under = under
        .strip_prefix(jackdaw_prefab::normalize_path(root))
        .ok()?;
    let copy_dir = overlay.join(under);
    copy_dir.join(data_path).is_file().then_some(copy_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(assets: &Path) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::asset::AssetPlugin {
            file_path: assets.to_string_lossy().into_owned(),
            ..Default::default()
        });
        app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
        app.add_plugins(crate::JackdawPlugin);
        app
    }

    fn project() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("a temp dir");
        let assets = dir.path().join("assets");
        std::fs::create_dir_all(&assets).expect("assets");
        (dir, assets)
    }

    #[test]
    fn a_game_with_no_link_holds_no_overlay() {
        let (_dir, assets) = project();
        let app = game(&assets);
        assert!(!app.world().contains_resource::<PlayOverlay>());
        assert!(
            overlaid_prefab(app.world(), &assets.join("door.bsn"), &assets).is_none(),
            "nothing stands in for a file"
        );
    }

    #[test]
    fn a_prefab_path_outside_the_asset_root_is_refused() {
        let (_dir, assets) = project();
        let mut app = game(&assets);
        load_prefab_files(
            app.world_mut(),
            &[
                ("../door.bsn".to_string(), Some("#Door\n".to_string())),
                ("/etc/door.bsn".to_string(), Some("#Door\n".to_string())),
            ],
        );
        let held = app
            .world_mut()
            .get_resource_or_init::<PlayOverlay>()
            .prefabs
            .len();
        assert_eq!(held, 0);
    }

    #[test]
    fn a_sent_prefab_stands_in_until_the_editor_sends_it_back_to_the_file() {
        let (_dir, assets) = project();
        let mut app = game(&assets);
        let path = jackdaw_prefab::normalize_path(&assets.join("door.bsn"));
        let root = jackdaw_prefab::normalize_path(&assets);
        load_prefab_files(
            app.world_mut(),
            &[("door.bsn".to_string(), Some("#Door\n".to_string()))],
        );
        assert!(overlaid_prefab(app.world(), &path, &root).is_some());
        load_prefab_files(app.world_mut(), &[("door.bsn".to_string(), None)]);
        assert!(overlaid_prefab(app.world(), &path, &root).is_none());
    }

    #[test]
    fn only_a_private_folder_of_the_project_holds_sidecars() {
        let (dir, assets) = project();
        let mut app = game(&assets);
        let held = |app: &App| {
            app.world()
                .get_resource::<PlayOverlay>()
                .and_then(|overlay| overlay.sidecars.clone())
        };

        let elsewhere = tempfile::tempdir().expect("a temp dir");
        for named in [
            elsewhere.path().join(".jackdaw/play"),
            dir.path().join("somewhere"),
            dir.path().join(".jackdaw/play/../../.."),
        ] {
            note_sidecar_overlay(app.world_mut(), &named.to_string_lossy());
            assert_eq!(held(&app), None, "{}", named.display());
        }

        let play = dir.path().join(".jackdaw/play");
        note_sidecar_overlay(app.world_mut(), &play.to_string_lossy());
        assert_eq!(held(&app), Some(jackdaw_prefab::normalize_path(&play)));
    }

    #[cfg(any(feature = "terrain", feature = "tilemap"))]
    #[test]
    fn a_sidecar_is_read_from_the_overlay_only_when_the_copy_exists() {
        let (dir, assets) = project();
        let mut app = game(&assets);
        let play = dir.path().join(".jackdaw/play");
        note_sidecar_overlay(app.world_mut(), &play.to_string_lossy());
        std::fs::create_dir_all(play.join("levels")).expect("the folder");
        std::fs::write(play.join("levels/map.jdtiles"), b"cells").expect("a copy");

        let levels = assets.join("levels");
        assert_eq!(
            overlaid_sidecar_dir(app.world(), &assets, &levels, "map.jdtiles"),
            Some(jackdaw_prefab::normalize_path(&play).join("levels"))
        );
        assert_eq!(
            overlaid_sidecar_dir(app.world(), &assets, &levels, "other.jdtiles"),
            None
        );
        assert_eq!(
            overlaid_sidecar_dir(app.world(), &assets, &levels, "../levels/map.jdtiles"),
            None
        );
    }
}
