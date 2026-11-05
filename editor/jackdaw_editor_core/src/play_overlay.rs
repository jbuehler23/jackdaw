//! What a running game reads in place of files that lag the editor.
//!
//! A scene reaches prefabs and sidecars the game reads off disk at spawn. For
//! each scene it asks for, the game gets the unsaved ones with the answer: the
//! text of each prefab in the scene's `IsA` closure that differs from its last
//! saved state, spelled as a save spells it, and a copy of each sidecar that
//! differs from its file, under `.jackdaw/play/` of the project. Nothing a save
//! writes is touched. Text and copies of data with no unsaved change are never
//! made, so the game reads the normal file.

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Component, Path, PathBuf};

use bevy::prelude::*;
use jackdaw_bsn::SceneBsnAst;
use jackdaw_pie_protocol::ControlEvent;

use crate::pie::InstanceKey;
use crate::play_scene::{DocumentAssets, PlayAssetNames, warn_absolute, with_session_names};
use crate::prefab::canonical_prefab_path;
use crate::prefab::operators::prefab_body;
use crate::prefab::resolver_bsn::{PREFAB_TYPE, clone_scene};
use crate::prefab::save_load::{read_prefab_ast, resolve_source_path, source_root};
use crate::scene_io::hooks::{PlayDocument, export_play_sidecars};
use crate::scene_io::save::{GameDocument, emit_bsn_scene_for_game, emit_snapshot_for_game};
use crate::scenes::{Scenes, TabContent};

/// A prefab's unsaved text.
pub(crate) struct UnsavedPrefab {
    /// Its path under the asset folder.
    path: String,
    text: String,
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
    pub(crate) prefabs: Vec<UnsavedPrefab>,
    /// What those prefabs' texts hold or name.
    pub(crate) assets: DocumentAssets,
    /// The private folder, when a sidecar of the scene or its prefabs is in it.
    pub(crate) sidecars: Option<String>,
}

/// The private folder of the project's copies, laid out like its asset folder.
fn overlay_root(world: &World) -> Option<PathBuf> {
    let project = world.get_resource::<crate::project::ProjectRoot>()?;
    Some(project.jackdaw_dir().join("play"))
}

/// Whether `dir` stays below the folder it is joined to.
fn is_plain_relative(dir: &Path) -> bool {
    dir.components()
        .all(|part| matches!(part, Component::Normal(_)))
}

/// The overlay of the scene in tab `tab`, whose document counts as being in `dir`
/// under the asset folder. `folder` is that folder on disk and `ast` the text
/// going to the game.
pub(crate) fn overlay_of_tab(
    world: &mut World,
    tab: usize,
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
        file: file.as_deref(),
        ast,
        folder,
        dir: Path::new(dir),
    };
    overlay_of(world, &scene)
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
    let scene = SceneInOverlay {
        tab: None,
        file: Some(&file),
        ast: &ast,
        folder: &folder,
        dir: relative.parent().unwrap_or(Path::new("")),
    };
    overlay_of(world, &scene)
}

/// The document a closure starts from.
struct SceneInOverlay<'a> {
    tab: Option<usize>,
    file: Option<&'a Path>,
    ast: &'a SceneBsnAst,
    folder: &'a Path,
    dir: &'a Path,
}

fn overlay_of(world: &mut World, scene: &SceneInOverlay) -> Overlay {
    let mut overlay = Overlay::default();
    let (Some(assets), Some(play)) = (crate::asset_index::assets_dir(world), overlay_root(world))
    else {
        return overlay;
    };
    let namespace = source_root(world, scene.folder);
    let mut copied = export_copies(
        world,
        &PlayDocument {
            tab: scene.tab,
            file: scene.file,
            ast: scene.ast,
            overlay_dir: &play.join(scene.dir),
        },
    );

    let mut pending = sources_of(scene.ast, &namespace, scene.folder);
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
        copied |= export_copies(
            world,
            &PlayDocument {
                tab: held.tab,
                file: Some(&source),
                ast: &held.ast,
                overlay_dir: &play.join(relative.parent().unwrap_or(Path::new(""))),
            },
        );
        if let Some(document) = held.unsaved {
            add_unsaved(world, &mut overlay, &relative, &document);
        }
    }
    overlay.sidecars = copied.then(|| play.to_string_lossy().into_owned());
    overlay
}

/// Run the sidecar hooks for one document. A failure costs the game that data,
/// not the scene.
fn export_copies(world: &mut World, document: &PlayDocument) -> bool {
    match export_play_sidecars(world, document) {
        Ok(copied) => copied,
        Err(err) => {
            let message = format!("The game may miss unsaved terrain or tilemap data: {err}");
            warn!("PIE: {message}");
            crate::status_bar::notify_warn(world, message);
            false
        }
    }
}

/// `document` as the unsaved text of the prefab at `relative`, under session names
/// and spelled as a save spells it. A document that does not parse back leaves the
/// game on the file, with a warning.
fn add_unsaved(world: &mut World, overlay: &mut Overlay, relative: &Path, document: &GameDocument) {
    let (body, mut parsed) = match with_session_names(world, document) {
        Ok(named) => named,
        Err(err) => {
            warn!(
                "PIE: the unsaved prefab {} does not parse back, the game reads its file: {err}",
                relative.display()
            );
            return;
        }
    };
    warn_absolute(world, &document.absolute);
    let assets = DocumentAssets::of(world, &mut parsed);
    overlay.assets.embedded.extend(assets.embedded);
    overlay.assets.files.extend(assets.files);
    overlay.prefabs.push(UnsavedPrefab {
        path: relative.to_string_lossy().replace('\\', "/"),
        text: crate::asset_files::asset_file_text(PREFAB_TYPE, &body),
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
            let document = emit_bsn_scene_for_game(world, &parent);
            let ast = jackdaw_bsn::parse_emitted(&document.text).ok()?;
            let modified = crate::scenes::operators::active_tab_modified(world);
            return Some(HeldPrefab {
                tab: Some(tab),
                ast,
                unsaved: modified.then_some(document),
            });
        }
        if let TabContent::Scene(Some(snapshot)) = &scenes.tabs[tab].content {
            let unsaved = scenes.tabs[tab]
                .dirty
                .then(|| emit_snapshot_for_game(world, snapshot, &parent));
            return Some(HeldPrefab {
                tab: Some(tab),
                ast: clone_scene(snapshot),
                unsaved,
            });
        }
    }
    let cached = world
        .get_resource::<crate::prefab::PrefabAstCache>()
        .and_then(|cache| cache.get(source));
    if let Some(cached) = cached {
        let unsaved = differs_from_file(world, source, cached, namespace)
            .then(|| emit_snapshot_for_game(world, cached, &parent));
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
/// held, and the prefabs it holds that have since been saved, sent back to their
/// files.
pub(crate) fn prefab_files_missing_from(
    world: &mut World,
    key: &InstanceKey,
    prefabs: Vec<UnsavedPrefab>,
) -> Option<ControlEvent> {
    let mut sent: Vec<(String, Option<String>)> = Vec::new();
    let reached: HashSet<String> = prefabs.iter().map(|prefab| prefab.path.clone()).collect();
    let mut names = world.get_resource_or_init::<PlayAssetNames>();
    for prefab in prefabs {
        let digest = prefab.digest();
        if !names.has_given_prefab(key, &prefab.path, digest) {
            names.note_given_prefab(key, prefab.path.clone(), digest);
            sent.push((prefab.path, Some(prefab.text)));
        }
    }
    let others: Vec<String> = names
        .given_prefabs(key)
        .into_iter()
        .filter(|path| !reached.contains(path))
        .collect();
    let saved = saved_since(world, others);
    let mut names = world.resource_mut::<PlayAssetNames>();
    for path in saved {
        names.forget_given_prefab(key, &path);
        sent.push((path, None));
    }
    (!sent.is_empty()).then_some(ControlEvent::PrefabFiles { files: sent })
}

/// Those of `paths`, under the asset folder, with no unsaved change.
fn saved_since(world: &mut World, paths: Vec<String>) -> Vec<String> {
    let Some(assets) = crate::asset_index::assets_dir(world) else {
        return Vec::new();
    };
    let namespace = source_root(world, &assets);
    paths
        .into_iter()
        .filter(|path| {
            held_prefab(world, &namespace.join(path), &namespace)
                .is_none_or(|held| held.unsaved.is_none())
        })
        .collect()
}

/// Remove the copies games were reading. Runs as Play stops, and as it starts, for
/// copies an editor crash left behind.
pub fn clear_play_sidecars(world: &mut World) {
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
        crate::scene_io::hooks::put_play_copy(&copy, Some(b"cells")).expect("written");
        assert!(copy.is_file());
        assert!(crate::pie::source_unchanged_since(dir.path(), built));
    }

    #[test]
    fn a_copy_that_is_not_there_is_not_an_error_to_remove() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let copy = dir.path().join("map.jdtiles");
        assert!(!crate::scene_io::hooks::put_play_copy(&copy, None).expect("nothing to do"));
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
