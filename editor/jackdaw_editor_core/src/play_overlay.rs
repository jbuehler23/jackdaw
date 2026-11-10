//! What a running game reads in place of files that lag the editor.
//!
//! A scene reaches prefabs and sidecars the game reads off disk at spawn. For
//! each scene it asks for, the game gets the unsaved ones with the answer: the
//! text of each prefab the scene reaches, through `IsA` or a terrain's scatter,
//! that differs from its last
//! saved state, spelled as a save spells it, and a copy of each sidecar with
//! unsaved changes, under a folder of that scene's own in `.jackdaw/play/` of the
//! project, so no document reads another's copies. Nothing a save writes is
//! touched. Text and copies of data with no unsaved change are never made, so the
//! game reads the normal file.

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Component, Path, PathBuf};

use bevy::prelude::*;
use jackdaw_bsn::SceneBsnAst;
use jackdaw_pie_protocol::ControlEvent;

use crate::pie::InstanceKey;
use crate::play_scene::{
    AssetOwner, DocumentAssets, PlayAssetNames, PlayScene, warn_absolute, with_session_names,
};
use crate::prefab::canonical_prefab_path;
use crate::prefab::operators::prefab_body;
use crate::prefab::resolver_bsn::clone_scene;
use crate::prefab::save_load::{read_prefab_ast, resolve_source_path, source_root};
use crate::scene_io::hooks::{PlayCopies, PlayDocument, export_play_sidecars, play_prefabs};
use crate::scene_io::save::{GameDocument, live_document_for_game, snapshot_for_game};
use crate::scenes::{Scenes, TabContent};

/// A prefab's unsaved document, with its text still to be written.
pub(crate) struct UnwrittenPrefab {
    /// Its path under the asset folder.
    pub(crate) path: String,
    pub(crate) ast: SceneBsnAst,
    /// The `#Name` of each asset root it holds.
    pub(crate) embedded: Vec<String>,
}

/// A prefab's unsaved text.
pub(crate) struct UnsavedPrefab {
    /// Its path under the asset folder.
    pub(crate) path: String,
    pub(crate) text: String,
}

impl UnsavedPrefab {
    fn digest(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.text.hash(&mut hasher);
        hasher.finish()
    }
}

/// What a scene reaches beyond its own text.
#[derive(Default)]
pub(crate) struct Overlay {
    pub(crate) prefabs: Vec<UnwrittenPrefab>,
    /// What those prefabs' texts hold or name.
    pub(crate) assets: DocumentAssets,
    /// The scene's private folder, when a sidecar of it or its prefabs is in there.
    pub(crate) sidecars: Option<String>,
}

/// The private folder of the project's copies, laid out like its asset folder.
fn overlay_root(world: &World) -> Option<PathBuf> {
    let project = world.get_resource::<crate::project::ProjectRoot>()?;
    Some(project.jackdaw_dir().join("play"))
}

/// The folder of the copies a game reads for `scene`. Each document has its own,
/// whatever its folder and whatever data paths it names.
fn scene_folder(play: &Path, scene: &PlayScene) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    match scene {
        PlayScene::Path(path) => ("path", path).hash(&mut hasher),
        PlayScene::Untitled(name) => ("untitled", name).hash(&mut hasher),
    }
    play.join(format!("{:016x}", hasher.finish()))
}

/// Whether `dir` stays below the folder it is joined to.
fn is_plain_relative(dir: &Path) -> bool {
    dir.components()
        .all(|part| matches!(part, Component::Normal(_)))
}

/// The overlay of the document `target` with its text going to the game, held in
/// tab `tab` if any, counting as being in `dir` under the asset folder. `folder` is
/// that folder on disk and `ast` that text.
pub(crate) fn overlay_of_document(
    world: &mut World,
    tab: Option<usize>,
    target: &PlayScene,
    ast: &SceneBsnAst,
    folder: &Path,
    dir: &str,
) -> Overlay {
    if !is_plain_relative(Path::new(dir)) {
        return Overlay::default();
    }
    let file = match (tab, target) {
        (Some(tab), _) => world.resource::<Scenes>().tabs[tab].path.clone(),
        (None, PlayScene::Path(path)) => crate::asset_index::assets_dir(world)
            .and_then(|assets| jackdaw_bsn::existing_form(&assets.join(path))),
        (None, PlayScene::Untitled(_)) => None,
    };
    let scene = SceneInOverlay {
        tab,
        target,
        file: file.as_deref(),
        ast,
        folder,
        dir: Path::new(dir),
        sidecars: true,
    };
    overlay_of(world, &scene)
}

/// [`overlay_of_document`] for part of the document, its unsaved prefabs alone. The
/// copies stay as the whole document's walk left them.
pub(crate) fn prefabs_of_document_part(
    world: &mut World,
    tab: usize,
    target: &PlayScene,
    ast: &SceneBsnAst,
    folder: &Path,
    dir: &str,
) -> Overlay {
    if !is_plain_relative(Path::new(dir)) {
        return Overlay::default();
    }
    let file = world.resource::<Scenes>().tabs[tab].path.clone();
    let scene = SceneInOverlay {
        tab: Some(tab),
        target,
        file: file.as_deref(),
        ast,
        folder,
        dir: Path::new(dir),
        sidecars: false,
    };
    overlay_of(world, &scene)
}

/// The unsaved text of the prefab at `path` under the asset folder, when only the
/// cache holds it: no tab does.
pub(crate) fn unsaved_cached_prefab(world: &mut World, path: &str) -> Option<GameDocument> {
    let assets = crate::asset_index::assets_dir(world)?;
    if !is_plain_relative(Path::new(path)) {
        return None;
    }
    let source = assets.join(path);
    world
        .get_resource::<crate::prefab::PrefabAstCache>()?
        .get(&source)?;
    let namespace = source_root(world, source.parent().unwrap_or(&assets));
    held_prefab(world, &source, &namespace)?.unsaved
}

/// The overlay of the scene file at `path` under the asset folder, which a game
/// will load itself. A file that does not read has none.
pub(crate) fn overlay_of_file(world: &mut World, path: &str) -> Overlay {
    let relative = Path::new(path);
    let Some(assets) = crate::asset_index::assets_dir(world) else {
        return Overlay::default();
    };
    if !is_plain_relative(relative) {
        return Overlay::default();
    }
    let full = assets.join(relative);
    let Some(file) = jackdaw_bsn::existing_form(&full) else {
        return Overlay::default();
    };
    let parsed = jackdaw_bsn::read_document_text(&file)
        .ok()
        .and_then(|text| jackdaw_bsn::parse_bsn_text(&text).ok());
    let Some(ast) = parsed else {
        return Overlay::default();
    };
    let folder = file.parent().map(Path::to_path_buf).unwrap_or_default();
    let tab = tab_of_file(world, &file);
    let target = PlayScene::Path(path.to_string());
    let scene = SceneInOverlay {
        tab,
        target: &target,
        file: Some(&file),
        ast: &ast,
        folder: &folder,
        dir: relative.parent().unwrap_or(Path::new("")),
        sidecars: true,
    };
    overlay_of(world, &scene)
}

/// The document a closure starts from.
struct SceneInOverlay<'a> {
    tab: Option<usize>,
    target: &'a PlayScene,
    file: Option<&'a Path>,
    ast: &'a SceneBsnAst,
    folder: &'a Path,
    dir: &'a Path,
    /// Whether to write the copies of unsaved sidecars.
    sidecars: bool,
}

fn overlay_of(world: &mut World, scene: &SceneInOverlay) -> Overlay {
    let mut overlay = Overlay::default();
    let (Some(assets), Some(play)) = (crate::asset_index::assets_dir(world), overlay_root(world))
    else {
        return overlay;
    };
    let namespace = source_root(world, scene.folder);
    let copies_root = scene_folder(&play, scene.target);
    let document = PlayDocument {
        tab: scene.tab,
        instanced: false,
        file: scene.file,
        ast: scene.ast,
        overlay_dir: &copies_root.join(scene.dir),
    };
    if scene.sidecars {
        world.get_resource_or_init::<PlayCopies>().begin_walk();
        export_copies(world, &document);
    }

    let mut pending = sources_of(scene.ast, &namespace, scene.folder);
    pending.extend(data_prefabs(world, &document, &assets));
    let mut seen = HashSet::new();
    while let Some(source) = pending.pop() {
        if !seen.insert(canonical_prefab_path(&source)) {
            continue;
        }
        let Some(relative) = under_assets(&source, &namespace, &assets) else {
            continue;
        };
        let Some(held) = held_prefab(world, &source, &namespace) else {
            continue;
        };
        let parent = source.parent().map(Path::to_path_buf).unwrap_or_default();
        pending.extend(sources_of(&held.ast, &namespace, &parent));
        let document = PlayDocument {
            tab: held.tab,
            instanced: true,
            file: Some(&source),
            ast: &held.ast,
            overlay_dir: &copies_root.join(relative.parent().unwrap_or(Path::new(""))),
        };
        pending.extend(data_prefabs(world, &document, &assets));
        if scene.sidecars {
            export_copies(world, &document);
        }
        if let Some(document) = held.unsaved {
            let owner = match held.tab {
                Some(tab) => AssetOwner::of_tab(world, tab),
                None => AssetOwner::File(relative.to_string_lossy().replace('\\', "/")),
            };
            add_unsaved(world, &mut overlay, &relative, document, &owner);
        }
    }
    if scene.sidecars {
        let copied = world.resource_mut::<PlayCopies>().finish_walk(&copies_root);
        overlay.sidecars = copied.then(|| copies_root.to_string_lossy().into_owned());
    }
    overlay
}

/// Run the sidecar hooks for one document. A failure costs the game that data,
/// not the scene.
fn export_copies(world: &mut World, document: &PlayDocument) {
    let exported = world.resource_scope(|world, mut copies: Mut<PlayCopies>| {
        export_play_sidecars(world, document, &mut copies)
    });
    if let Err(err) = exported {
        let message = format!("The game may miss unsaved terrain or tilemap data: {err}");
        warn!("PIE: {message}");
        crate::status_bar::notify_warn(world, message);
    }
}

/// `document` as the unsaved prefab at `relative`, under session names and spelled
/// as a save spells it.
fn add_unsaved(
    world: &mut World,
    overlay: &mut Overlay,
    relative: &Path,
    document: GameDocument,
    owner: &AssetOwner,
) {
    warn_absolute(world, &document.absolute);
    let mut ast = with_session_names(world, document, owner);
    let assets = DocumentAssets::of(world, &mut ast);
    let embedded = assets
        .embedded
        .iter()
        .map(|(name, _)| name.clone())
        .collect();
    overlay.assets.embedded.extend(assets.embedded);
    overlay.assets.files.extend(assets.files);
    overlay.prefabs.push(UnwrittenPrefab {
        path: relative.to_string_lossy().replace('\\', "/"),
        ast,
        embedded,
    });
}

/// The file each `IsA` of `ast`, a document in `folder`, names.
fn sources_of(ast: &SceneBsnAst, namespace: &Path, folder: &Path) -> Vec<PathBuf> {
    ast.entities_with_component(jackdaw_prefab::ISA_TYPE)
        .into_iter()
        .filter_map(|node| jackdaw_prefab::read_isa_source(ast, node))
        .map(|source| resolve_source_path(&source, namespace, folder))
        .collect()
}

/// The file of each prefab `document`'s data names, such as a terrain's scatter.
fn data_prefabs(world: &World, document: &PlayDocument, assets: &Path) -> Vec<PathBuf> {
    play_prefabs(world, document)
        .into_iter()
        .filter(|path| is_plain_relative(Path::new(path)))
        .map(|path| assets.join(path))
        .collect()
}

/// `source`'s path under the asset folder; the game refuses any other.
fn under_assets(source: &Path, namespace: &Path, assets: &Path) -> Option<PathBuf> {
    let source = jackdaw_prefab::normalize_path(source);
    source
        .strip_prefix(jackdaw_prefab::normalize_path(namespace))
        .or_else(|_| source.strip_prefix(jackdaw_prefab::normalize_path(assets)))
        .ok()
        .map(Path::to_path_buf)
}

/// A prefab as the editor holds it.
struct HeldPrefab {
    /// The tab holding it.
    tab: Option<usize>,
    ast: SceneBsnAst,
    /// Its text, when that differs from its last saved state.
    unsaved: Option<GameDocument>,
}

/// The tab holding the file at `source`, unless it refused its document.
fn tab_of_file(world: &World, source: &Path) -> Option<usize> {
    let wanted = canonical_prefab_path(source);
    world.get_resource::<Scenes>()?.tabs.iter().position(|tab| {
        !tab.is_refused()
            && tab
                .path
                .as_deref()
                .is_some_and(|path| canonical_prefab_path(path) == wanted)
    })
}

/// The prefab at `source`: the live document for the tab in front, the snapshot or
/// cache entry for a tab behind, or the cache entry or file with no tab.
fn held_prefab(world: &mut World, source: &Path, namespace: &Path) -> Option<HeldPrefab> {
    let parent = source.parent().map(Path::to_path_buf).unwrap_or_default();
    let tab = tab_of_file(world, source);
    if let Some(tab) = tab {
        let scenes = world.resource::<Scenes>();
        if scenes.active == tab {
            if !world.contains_resource::<SceneBsnAst>() {
                return None;
            }
            let document = live_document_for_game(world, &parent);
            let modified = crate::scenes::operators::active_tab_modified(world);
            let (ast, unsaved) = if modified {
                (clone_scene(&document.ast), Some(document))
            } else {
                (document.ast, None)
            };
            return Some(HeldPrefab {
                tab: Some(tab),
                ast,
                unsaved,
            });
        }
        if let TabContent::Scene(Some(snapshot)) = &scenes.tabs[tab].content {
            let unsaved = scenes.tabs[tab]
                .dirty
                .then(|| snapshot_for_game(world, snapshot, &parent));
            return Some(HeldPrefab {
                tab: Some(tab),
                ast: clone_scene(snapshot),
                unsaved,
            });
        }
    }
    let differs = cached_differs_from_file(world, source, namespace);
    let cached = world
        .get_resource::<crate::prefab::PrefabAstCache>()
        .and_then(|cache| cache.get(source));
    if let Some((cached, differs)) = cached.zip(differs) {
        let unsaved = differs.then(|| snapshot_for_game(world, cached, &parent));
        return Some(HeldPrefab {
            tab,
            ast: clone_scene(cached),
            unsaved,
        });
    }
    let file = jackdaw_bsn::existing_form(source)?;
    Some(HeldPrefab {
        tab,
        ast: read_prefab_ast(&file, namespace).ok()?,
        unsaved: None,
    })
}

/// Whether the cache entry for `source`, if there is one, differs from the file.
/// Answered from the last comparison until the cache or the file moves.
fn cached_differs_from_file(world: &mut World, source: &Path, namespace: &Path) -> Option<bool> {
    let cache = world.get_resource::<crate::prefab::PrefabAstCache>()?;
    let cached = cache.get(source)?;
    let epoch = cache.epoch();
    let modified = jackdaw_bsn::existing_form(source)
        .and_then(|file| std::fs::metadata(file).ok())
        .and_then(|meta| meta.modified().ok());
    let key = canonical_prefab_path(source).as_path().to_path_buf();
    let earlier = world
        .get_resource::<PlayCopies>()
        .and_then(|copies| copies.prefab_checks.get(&key))
        .filter(|(at, file, _)| *at == epoch && *file == modified);
    if let Some(&(_, _, differs)) = earlier {
        return Some(differs);
    }
    let differs = differs_from_file(world, source, cached, namespace);
    world
        .get_resource_or_init::<PlayCopies>()
        .prefab_checks
        .insert(key, (epoch, modified, differs));
    Some(differs)
}

/// Whether `held` spells differently from the file at `source` read the same way,
/// which leaves out what a text editor can vary in the file. A file that is
/// missing or does not read holds nothing `held` could match.
fn differs_from_file(world: &World, source: &Path, held: &SceneBsnAst, namespace: &Path) -> bool {
    let Some(saved) =
        jackdaw_bsn::existing_form(source).and_then(|file| read_prefab_ast(&file, namespace).ok())
    else {
        return true;
    };
    match (
        prefab_body(world, held, source),
        prefab_body(world, &saved, source),
    ) {
        (Ok(held), Ok(saved)) => held != saved,
        _ => true,
    }
}

/// What the game `key` needs to hold `prefabs` as the editor does, now counted as
/// held, and those of `to_files` it holds, sent back to their files.
pub(crate) fn prefab_files_missing_from(
    world: &mut World,
    key: &InstanceKey,
    prefabs: Vec<UnsavedPrefab>,
    to_files: Vec<String>,
) -> Option<ControlEvent> {
    let mut sent: Vec<(String, Option<String>)> = Vec::new();
    let mut names = world.get_resource_or_init::<PlayAssetNames>();
    for prefab in prefabs {
        let digest = prefab.digest();
        if !names.has_given_prefab(key, &prefab.path, digest) {
            names.note_given_prefab(key, prefab.path.clone(), digest);
            sent.push((prefab.path, Some(prefab.text)));
        }
    }
    let mut given: HashSet<String> = names.given_prefabs(key).into_iter().collect();
    for path in to_files {
        if given.remove(&path) {
            names.forget_given_prefab(key, &path);
            sent.push((path, None));
        }
    }
    (!sent.is_empty()).then_some(ControlEvent::PrefabFiles { files: sent })
}

/// What the game `key` holds that `reached` leaves out, as each prefab's path and,
/// when it has an unsaved change, its document.
fn held_beyond(
    world: &mut World,
    key: &InstanceKey,
    reached: &HashSet<String>,
) -> Vec<(String, Option<SceneBsnAst>)> {
    let others: Vec<String> = world
        .get_resource_or_init::<PlayAssetNames>()
        .given_prefabs(key)
        .into_iter()
        .filter(|path| !reached.contains(path))
        .collect();
    if others.is_empty() {
        return Vec::new();
    }
    let Some(assets) = crate::asset_index::assets_dir(world) else {
        return Vec::new();
    };
    let namespace = source_root(world, &assets);
    others
        .into_iter()
        .map(|path| {
            let document = held_prefab(world, &namespace.join(&path), &namespace)
                .and_then(|held| held.unsaved)
                .map(|document| document.ast);
            (path, document)
        })
        .collect()
}

/// The documents of the unsaved prefabs the game `key` holds that `reached` leaves
/// out, by path. Their text must still read for the game to stay on it.
pub(crate) fn unsaved_prefabs_beyond(
    world: &mut World,
    key: &InstanceKey,
    reached: &HashSet<String>,
) -> Vec<(String, SceneBsnAst)> {
    held_beyond(world, key, reached)
        .into_iter()
        .filter_map(|(path, document)| Some((path, document?)))
        .collect()
}

/// The prefabs the game `key` holds that `reached` leaves out and that have no
/// unsaved change, saved or discarded.
pub(crate) fn saved_prefabs_beyond(
    world: &mut World,
    key: &InstanceKey,
    reached: &HashSet<String>,
) -> Vec<String> {
    held_beyond(world, key, reached)
        .into_iter()
        .filter(|(_, document)| document.is_none())
        .map(|(path, _)| path)
        .collect()
}

/// Remove the copies games were reading. Runs as Play stops, and as it starts, for
/// copies an editor crash left behind.
pub fn clear_play_sidecars(world: &mut World) {
    if let Some(mut copies) = world.get_resource_mut::<PlayCopies>() {
        *copies = PlayCopies::default();
    }
    let Some(play) = overlay_root(world) else {
        return;
    };
    match std::fs::remove_dir_all(&play) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => warn!("PIE: could not clear {}: {err}", play.display()),
    }
}

/// Say at Play that unsaved model settings stay behind: the game reads them from
/// files, and they are not sent.
pub(crate) fn notice_unsaved_model_settings(world: &mut World) {
    let unsaved = world
        .get_resource::<crate::model_lod::UnsavedModelSettings>()
        .is_some_and(|held| !held.0.is_empty());
    if unsaved {
        crate::status_bar::notify_info(
            world,
            "Playing without unsaved model settings; save to include them",
        );
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::*;

    /// A copy lands long after the game was built and the build stays current.
    #[test]
    fn a_copy_in_the_private_folder_forces_no_rebuild() {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::create_dir_all(dir.path().join("src")).expect("src");
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").expect("a source");
        std::fs::write(dir.path().join("Cargo.toml"), "").expect("a manifest");
        let built = SystemTime::now() + Duration::from_secs(1);
        assert!(crate::pie::source_unchanged_since(dir.path(), built));

        std::thread::sleep(Duration::from_millis(1100));
        let copy = dir.path().join(".jackdaw/play/levels/map.jdtiles");
        PlayCopies::default()
            .settle(&copy, 1, || Ok(Some(b"cells".to_vec())))
            .expect("written");
        assert!(copy.is_file());
        assert!(crate::pie::source_unchanged_since(dir.path(), built));
    }

    #[test]
    fn a_copy_is_encoded_once_per_stamp_and_goes_when_a_walk_passes_it_over() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let copy = dir.path().join("levels/map.jdtiles");
        let mut copies = PlayCopies::default();
        let encodes = std::cell::Cell::new(0);
        let settle = |copies: &mut PlayCopies, stamp| {
            copies.begin_walk();
            copies
                .settle(&copy, stamp, || {
                    encodes.set(encodes.get() + 1);
                    Ok(Some(b"cells".to_vec()))
                })
                .expect("written");
            assert!(copies.finish_walk(dir.path()));
        };
        settle(&mut copies, 1);
        settle(&mut copies, 1);
        assert_eq!(encodes.get(), 1, "the copy was current");
        settle(&mut copies, 2);
        assert_eq!(encodes.get(), 2, "the data moved");

        std::fs::remove_file(&copy).expect("removed");
        settle(&mut copies, 2);
        assert_eq!(encodes.get(), 3, "the copy had gone");

        copies.begin_walk();
        assert!(!copies.finish_walk(dir.path()), "nothing asked for it");
        assert!(!copy.exists());
    }

    /// A folder per document keeps a copy from reaching anything else in the folder
    /// the document counts as being in.
    #[test]
    fn documents_with_the_same_data_path_keep_their_own_copies() {
        let play = Path::new("play");
        let untitled = |name: &str| scene_folder(play, &PlayScene::Untitled(name.to_string()));
        let file = |path: &str| scene_folder(play, &PlayScene::Path(path.to_string()));
        assert_ne!(untitled("Untitled"), untitled("Untitled 2"));
        assert_ne!(untitled("untitled.bsn"), file("untitled.bsn"));
        assert_ne!(file("a.bsn"), file("b.bsn"));
        assert_eq!(file("a.bsn"), file("a.bsn"));
    }

    #[test]
    fn a_folder_name_leaving_the_overlay_is_refused() {
        assert!(is_plain_relative(Path::new("levels/deep")));
        assert!(is_plain_relative(Path::new("")));
        assert!(!is_plain_relative(Path::new("../levels")));
        assert!(!is_plain_relative(Path::new("/levels")));
    }

    #[test]
    fn unsaved_model_settings_raise_a_notice() {
        let mut world = World::new();
        world.init_resource::<crate::status_bar::StatusNotice>();
        world.init_resource::<crate::model_lod::UnsavedModelSettings>();
        notice_unsaved_model_settings(&mut world);
        assert_eq!(
            world.resource::<crate::status_bar::StatusNotice>().text(),
            "",
            "nothing unsaved, nothing said"
        );

        world
            .resource_mut::<crate::model_lod::UnsavedModelSettings>()
            .0
            .insert("models/rock.glb".to_string());
        notice_unsaved_model_settings(&mut world);
        let notice = world.resource::<crate::status_bar::StatusNotice>();
        assert!(notice.text().contains("model"), "{}", notice.text());
    }
}
