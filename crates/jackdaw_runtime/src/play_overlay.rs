//! What a game under Play reads in place of files that lag the editor.
//!
//! The editor sends [`ControlEvent::PrefabFiles`] and names a folder of sidecars
//! in its answer to a scene request, which goes on the root that asked. Nothing
//! else creates the overlay, so a game with no editor link reads only its files.
//! Every path goes through the checks a file gets: a prefab must be inside the
//! asset root, and the sidecar folder must be directly under `.jackdaw/play` of a
//! project holding the asset root.
//!
//! [`ControlEvent::PrefabFiles`]: jackdaw_pie_protocol::ControlEvent::PrefabFiles

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw_bsn::SceneBsnAst;

/// What the editor told this game to read in place of its files.
#[derive(Resource, Default)]
pub(crate) struct PlayOverlay {
    /// Unsaved prefab text, by the prefab's normalized path in its text form.
    prefabs: HashMap<PathBuf, String>,
}

/// The folder of a scene root's unsaved sidecars, laid out like the asset folder,
/// as the editor named it answering that root.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct SidecarOverlay(PathBuf);

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

/// The folder of unsaved sidecars the editor named. Refused, with a warning, unless
/// it is directly under `.jackdaw/play` of a folder holding the asset root.
pub fn sidecar_overlay(world: &World, named: &str) -> Option<SidecarOverlay> {
    let folder = jackdaw_prefab::normalize_path(Path::new(named));
    let project = folder
        .parent()
        .filter(|play| play.ends_with(Path::new(".jackdaw").join("play")))
        .and_then(Path::parent)
        .and_then(Path::parent);
    let holds_assets = project
        .zip(crate::assets_root(world))
        .is_some_and(|(project, assets)| {
            real_path(&jackdaw_prefab::normalize_path(&assets)).starts_with(real_path(project))
        });
    if !holds_assets {
        warn!("PIE: ignoring the sidecar folder {named} the editor named");
        return None;
    }
    Some(SidecarOverlay(folder))
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
    let registry = world.resource::<AppTypeRegistry>().read();
    overlay.prefab(&registry, path, assets_root)
}

impl PlayOverlay {
    /// [`overlaid_prefab`], for a system holding the overlay and the registry.
    pub(crate) fn prefab(
        &self,
        registry: &bevy::reflect::TypeRegistry,
        path: &Path,
        assets_root: &Path,
    ) -> Option<SceneBsnAst> {
        let text = self.prefabs.get(&jackdaw_bsn::text_twin(path))?;
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
        let roots: Vec<Entity> = ast
            .roots
            .iter()
            .copied()
            .filter(|&root| !crate::is_asset_root(&ast, root, registry))
            .collect();
        ast.roots = roots;
        jackdaw_prefab::prepare_prefab_document(&mut ast, path, assets_root);
        Some(ast)
    }
}

/// Where the scene under `root` keeps unsaved copies of the sidecars of the document
/// in `dir`, `assets` being the asset root. A copy may or may not be there when it
/// is read; see [`read_sidecar`].
#[cfg(any(feature = "terrain", feature = "tilemap"))]
pub(crate) fn overlaid_sidecar_dir(
    world: &World,
    root: Entity,
    assets: &Path,
    dir: &Path,
) -> Option<PathBuf> {
    let overlay = world.get::<SidecarOverlay>(root)?;
    let under = jackdaw_prefab::normalize_path(dir);
    let under = under
        .strip_prefix(jackdaw_prefab::normalize_path(assets))
        .ok()?;
    Some(overlay.0.join(under))
}

/// The bytes of a sidecar: its unsaved copy at `copy` while that is there, else its
/// file. The copy can go between the spawn and this read, when the author saves or a
/// later answer finds nothing unsaved, and the file is then the right data.
#[cfg(any(feature = "terrain", feature = "tilemap"))]
pub(crate) fn read_sidecar(copy: Option<&Path>, file: &Path) -> std::io::Result<Vec<u8>> {
    if let Some(copy) = copy {
        match std::fs::read(copy) {
            Ok(bytes) => return Ok(bytes),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => warn!(
                "PIE: reading {} instead of its unsaved copy {}: {err}",
                file.display(),
                copy.display()
            ),
        }
    }
    std::fs::read(file)
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
        let app = game(&assets);

        let elsewhere = tempfile::tempdir().expect("a temp dir");
        for named in [
            elsewhere.path().join(".jackdaw/play/scene"),
            dir.path().join("somewhere"),
            dir.path().join(".jackdaw/play"),
            dir.path().join(".jackdaw/play/scene/deeper"),
            dir.path().join(".jackdaw/play/../../.."),
        ] {
            assert_eq!(
                sidecar_overlay(app.world(), &named.to_string_lossy()),
                None,
                "{}",
                named.display()
            );
        }

        let folder = dir.path().join(".jackdaw/play/scene");
        assert_eq!(
            sidecar_overlay(app.world(), &folder.to_string_lossy()),
            Some(SidecarOverlay(jackdaw_prefab::normalize_path(&folder)))
        );
    }

    #[cfg(any(feature = "terrain", feature = "tilemap"))]
    #[test]
    fn a_roots_copies_are_laid_out_like_the_asset_folder() {
        let (dir, assets) = project();
        let mut app = game(&assets);
        let folder = dir.path().join(".jackdaw/play/scene");
        let overlay = sidecar_overlay(app.world(), &folder.to_string_lossy()).expect("allowed");
        let with = app.world_mut().spawn(overlay).id();
        let without = app.world_mut().spawn_empty().id();
        let levels = assets.join("levels");
        assert_eq!(
            overlaid_sidecar_dir(app.world(), with, &assets, &levels),
            Some(jackdaw_prefab::normalize_path(&folder).join("levels"))
        );
        assert_eq!(
            overlaid_sidecar_dir(app.world(), without, &assets, &levels),
            None
        );
    }

    #[cfg(any(feature = "terrain", feature = "tilemap"))]
    #[test]
    fn a_copy_that_has_gone_falls_back_to_the_file() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let (copy, file) = (dir.path().join("copy"), dir.path().join("file"));
        std::fs::write(&file, b"saved").expect("a file");
        std::fs::write(&copy, b"unsaved").expect("a copy");
        assert_eq!(read_sidecar(Some(&copy), &file).expect("read"), b"unsaved");
        std::fs::remove_file(&copy).expect("gone");
        assert_eq!(read_sidecar(Some(&copy), &file).expect("read"), b"saved");
        assert_eq!(read_sidecar(None, &file).expect("read"), b"saved");
    }
}
