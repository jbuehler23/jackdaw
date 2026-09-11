//! One index of every asset file the open project holds, keyed by the path the
//! file sits at under `assets/`.
//!
//! A path is the identity. An entry records what the file says it holds, the
//! kind that claims that type, the value the editor loaded from it and the
//! modification time the load saw, so a file rewritten by another tool reloads
//! into the handle references already point at. `by_id` turns a loaded handle
//! back into the path it came from, and `by_stem` resolves the bare names that
//! older scenes and sidecars still spell.
//!
//! The index is built by one walk of the project's assets when the project
//! opens and kept in step by a watcher on the same directory. A kind the
//! editor has compiled in and loads elsewhere, such as an animation graph,
//! is listed here without a value.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, mpsc};
use std::time::SystemTime;

use bevy::asset::{ReflectAsset, UntypedAssetId, UntypedHandle};
use bevy::prelude::*;
use jackdaw_api::prelude::{AssetKind, AssetKinds};
use jackdaw_bsn::BsnStructData;
use path_slash::PathExt as _;

use crate::asset_files::{AssetFileKind, AssetKindCache, walk_document_files};
use crate::definition_assets::{MATERIAL_KIND, definition_name_of};
use crate::project::ProjectRoot;

/// What the editor loaded out of an asset file.
#[derive(Clone, Debug)]
pub enum AssetValue {
    /// A value in its type's asset store.
    Handle(UntypedHandle),
    /// A value of a type the editor knows only as the project's schema, held
    /// as the patch its file spells.
    Schema(Box<BsnStructData>),
    /// A file whose kind is loaded by the subsystem that owns it.
    Unloaded,
}

impl AssetValue {
    pub fn handle(&self) -> Option<&UntypedHandle> {
        match self {
            Self::Handle(handle) => Some(handle),
            _ => None,
        }
    }

    pub fn schema(&self) -> Option<&BsnStructData> {
        match self {
            Self::Schema(data) => Some(data),
            _ => None,
        }
    }
}

/// One asset file, as the index knows it.
#[derive(Clone, Debug)]
pub struct AssetEntry {
    /// Where the file sits, relative to the project's assets directory.
    pub path: PathBuf,
    pub kind: String,
    pub type_path: String,
    pub file_kind: AssetFileKind,
    pub value: AssetValue,
    pub mtime: SystemTime,
}

impl AssetEntry {
    /// The bare name the file's stem gives it: everything before the first dot
    /// of its file name, so `torch.item.bsn` is `torch`.
    pub fn name(&self) -> String {
        definition_name_of(&self.path)
    }
}

/// Every asset file the open project holds.
#[derive(Resource, Default)]
pub struct AssetIndex {
    entries: BTreeMap<PathBuf, AssetEntry>,
    by_id: HashMap<UntypedAssetId, PathBuf>,
    warned_stems: Mutex<HashSet<String>>,
}

impl AssetIndex {
    pub fn get(&self, path: &Path) -> Option<&AssetEntry> {
        self.entries.get(path)
    }

    pub fn get_mut(&mut self, path: &Path) -> Option<&mut AssetEntry> {
        self.entries.get_mut(path)
    }

    pub fn iter(&self) -> impl Iterator<Item = &AssetEntry> {
        self.entries.values()
    }

    pub fn of_kind<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a AssetEntry> {
        self.entries
            .values()
            .filter(move |entry| entry.kind == kind)
    }

    /// The files of a kind, as the paths that name them.
    pub fn paths_of_kind(&self, kind: &str) -> Vec<String> {
        self.of_kind(kind)
            .map(|entry| entry.path.to_slash_lossy().into_owned())
            .collect()
    }

    /// The file a loaded handle came from.
    pub fn path_of_id(&self, id: UntypedAssetId) -> Option<&Path> {
        self.by_id.get(&id).map(PathBuf::as_path)
    }

    /// The entry a loaded handle came from.
    pub fn by_handle(&self, handle: &UntypedHandle) -> Option<&AssetEntry> {
        self.entries.get(self.by_id.get(&handle.id())?)
    }

    /// Every file whose stem is `stem`.
    pub fn stem_paths(&self, stem: &str) -> Vec<PathBuf> {
        self.entries
            .values()
            .filter(|entry| entry.name() == stem)
            .map(|entry| entry.path.clone())
            .collect()
    }

    /// The file a bare name stands for, for the references written before
    /// paths. A stem two files share stands for neither, and says so once.
    pub fn by_stem(&self, stem: &str) -> Option<&AssetEntry> {
        let mut matching = self.entries.values().filter(|entry| entry.name() == stem);
        let first = matching.next()?;
        let Some(second) = matching.next() else {
            return Some(first);
        };
        if let Ok(mut warned) = self.warned_stems.lock()
            && warned.insert(stem.to_string())
        {
            warn!(
                "'{stem}' names both {} and {}; spell the one you mean as a path",
                first.path.display(),
                second.path.display()
            );
        }
        None
    }

    /// The material file holding this name, for the references and the panels
    /// that still spell a material by its stem.
    pub fn material_named(&self, name: &str) -> Option<&AssetEntry> {
        self.by_stem(name)
            .filter(|entry| entry.kind == MATERIAL_KIND)
    }

    pub fn insert(&mut self, entry: AssetEntry) {
        if let Some(handle) = entry.value.handle() {
            self.by_id.insert(handle.id(), entry.path.clone());
        }
        self.entries.insert(entry.path.clone(), entry);
    }

    pub fn remove(&mut self, path: &Path) -> Option<AssetEntry> {
        let entry = self.entries.remove(path)?;
        if let Some(handle) = entry.value.handle() {
            self.by_id.remove(&handle.id());
        }
        Some(entry)
    }

    /// Drop every entry whose kind is no longer registered.
    fn retain_kinds(&mut self, kinds: &[String]) {
        let gone: Vec<PathBuf> = self
            .entries
            .values()
            .filter(|entry| !kinds.contains(&entry.kind))
            .map(|entry| entry.path.clone())
            .collect();
        for path in gone {
            self.remove(&path);
        }
    }
}

/// What a rescan of the project's assets found.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct AssetRescan {
    /// Files that appeared since the last scan.
    pub added: Vec<PathBuf>,
    /// Files that have gone.
    pub removed: Vec<PathBuf>,
    /// Files that changed on disk and were read again into the handle they
    /// already had.
    pub reloaded: Vec<PathBuf>,
}

/// The project's assets directory, if one is open.
pub fn assets_dir(world: &World) -> Option<PathBuf> {
    world
        .get_resource::<ProjectRoot>()
        .map(ProjectRoot::assets_dir)
}

/// An indexed path as the file it names on disk.
pub fn absolute_path(world: &World, path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    match assets_dir(world) {
        Some(assets) => assets.join(path),
        None => path.to_path_buf(),
    }
}

/// A file on disk as the path the index keys it by. `None` for a file outside
/// the project's assets, including one a relative path walks out to.
pub fn indexed_path(world: &World, path: &Path) -> Option<PathBuf> {
    let assets = assets_dir(world)?;
    if let Ok(relative) = path.strip_prefix(&assets) {
        return Some(relative.to_path_buf());
    }
    let stays_under = !path.is_absolute()
        && !path
            .components()
            .any(|part| part == std::path::Component::ParentDir);
    stays_under.then(|| path.to_path_buf())
}

/// Whether an indexed path is the project's catalog file, which holds what has
/// no file of its own and is no asset in its own right.
pub fn is_catalog_file(path: &Path) -> bool {
    matches!(path.to_str(), Some("catalog.bsn") | Some("catalog.jsn"))
}

/// What a file says it holds, and the kind that claims it.
fn kind_of_file(path: &Path, kinds: &AssetKinds, cache: &mut AssetKindCache) -> Option<AssetKind> {
    let AssetFileKind::Asset { type_path } = cache.check(path, kinds) else {
        return None;
    };
    kinds.by_type_path(&type_path).cloned()
}

/// Read one asset file into whatever holds values of its kind.
///
/// A material the editor is already using under the name this file's stem
/// gives it takes the file's value rather than a second handle, so the panels
/// and the brush faces holding it follow what the file says.
pub fn load_asset_value(world: &mut World, kind: &AssetKind, path: &Path) -> Option<AssetValue> {
    if kind.kind == MATERIAL_KIND {
        let handle = crate::material_assets::load_material_file(world, path)?;
        let in_use = material_handle_in_use(world, path);
        return match in_use {
            Some(in_use) => Some(move_value(world, kind, &handle, &in_use)),
            None => Some(AssetValue::Handle(handle)),
        };
    }
    if !kind.scanned() {
        return Some(AssetValue::Unloaded);
    }
    crate::definition_assets::read_asset_file(world, kind, path)
}

/// The handle a material's name already answers to, when no file of its own
/// has claimed it: a set detected from its textures, or one created and saved
/// in this session.
fn material_handle_in_use(world: &World, path: &Path) -> Option<UntypedHandle> {
    let name = definition_name_of(path);
    let listed = world
        .get_resource::<crate::material_assets::MaterialRegistry>()
        .and_then(|registry| registry.get_by_name(&name))
        .filter(|entry| entry.handle != Handle::default())
        .map(|entry| entry.handle.clone().untyped());
    let named = world
        .get_resource::<crate::asset_catalog::AssetCatalog>()
        .and_then(|catalog| catalog.handles.get(&format!("@{name}")).cloned());
    let handle = listed
        .or(named)
        .filter(|handle| handle.type_id() == std::any::TypeId::of::<StandardMaterial>())?;
    let claimed = world
        .get_resource::<AssetIndex>()
        .is_some_and(|index| index.by_handle(&handle).is_some());
    (!claimed).then_some(handle)
}

/// Read a changed file back into the handle the loaded one already has, so
/// everything holding that handle sees the new value.
fn reload_in_place(
    world: &mut World,
    kind: &AssetKind,
    path: &Path,
    held: &AssetValue,
) -> Option<AssetValue> {
    let fresh = load_asset_value(world, kind, path)?;
    let (Some(held), Some(fresh_handle)) = (held.handle(), fresh.handle()) else {
        return Some(fresh);
    };
    Some(move_value(world, kind, fresh_handle, held))
}

/// Move a freshly read value into the handle the editor is already handing
/// out, leaving the fresh one empty.
fn move_value(
    world: &mut World,
    kind: &AssetKind,
    fresh: &UntypedHandle,
    into: &UntypedHandle,
) -> AssetValue {
    if fresh.id() == into.id() {
        return AssetValue::Handle(into.clone());
    }
    let registry = world.resource::<AppTypeRegistry>().clone();
    let reflect_asset = registry
        .read()
        .get_with_type_path(&kind.type_path)
        .and_then(|registration| registration.data::<ReflectAsset>())
        .cloned();
    let moved = reflect_asset.and_then(|reflect_asset| {
        let value = reflect_asset.remove(world, fresh.id())?;
        reflect_asset
            .insert(world, into.id(), value.as_partial_reflect())
            .ok()
    });
    match moved {
        Some(()) => AssetValue::Handle(into.clone()),
        None => AssetValue::Handle(fresh.clone()),
    }
}

/// Walk the project's assets and bring the index up to what is on disk.
///
/// A file that appeared is read and indexed. A file that changed is read again
/// into the handle it already had, unless the card editing it has unsaved
/// edits, which are what the user meant to keep. A file that has gone leaves
/// the index and closes its card, unless that card holds unsaved edits and so
/// has somewhere to write the file back from; its handle stays alive so
/// whatever already references it keeps rendering.
pub fn rescan_asset_index(world: &mut World) -> AssetRescan {
    let mut scan = AssetRescan::default();
    let Some(assets) = assets_dir(world) else {
        return scan;
    };
    world.get_resource_or_init::<AssetKindCache>();
    if !world.contains_resource::<AssetIndex>() {
        world.init_resource::<AssetIndex>();
    }

    let found = world.resource_scope(|world, mut cache: Mut<AssetKindCache>| {
        let Some(kinds) = world.get_resource::<AssetKinds>() else {
            return Vec::new();
        };
        walk_document_files(&assets)
            .into_iter()
            .filter_map(|path| {
                let relative = path.strip_prefix(&assets).ok()?.to_path_buf();
                if is_catalog_file(&relative) {
                    return None;
                }
                let kind = kind_of_file(&path, kinds, &mut cache)?;
                let mtime = std::fs::metadata(&path)
                    .and_then(|meta| meta.modified())
                    .ok()?;
                Some((relative, kind, mtime))
            })
            .collect::<Vec<_>>()
    });

    let gone: Vec<PathBuf> = world
        .resource::<AssetIndex>()
        .iter()
        .map(|entry| entry.path.clone())
        .filter(|path| !found.iter().any(|(relative, _, _)| relative == path))
        .collect();
    for path in gone {
        if crate::definition_assets::card_has_unsaved_edits(world, &path) {
            continue;
        }
        if assets.join(&path).is_file() {
            warn!(
                "{} no longer holds a type this project knows",
                path.display()
            );
        }
        world.resource_mut::<AssetIndex>().remove(&path);
        crate::definition_assets::close_card_for(world, &path);
        scan.removed.push(path);
    }

    for (relative, kind, mtime) in found {
        let held = world
            .resource::<AssetIndex>()
            .get(&relative)
            .map(|entry| (entry.mtime, entry.value.clone(), entry.kind.clone()));
        let file = assets.join(&relative);
        let value = match held {
            Some((known, _, _)) if known == mtime => continue,
            Some((_, _, known_kind)) if known_kind != kind.kind => {
                world.resource_mut::<AssetIndex>().remove(&relative);
                let Some(value) = load_asset_value(world, &kind, &file) else {
                    continue;
                };
                scan.added.push(relative.clone());
                value
            }
            Some((_, held, _)) => {
                if crate::definition_assets::card_has_unsaved_edits(world, &relative) {
                    continue;
                }
                let Some(value) = reload_in_place(world, &kind, &file, &held) else {
                    continue;
                };
                scan.reloaded.push(relative.clone());
                value
            }
            None => {
                let Some(value) = load_asset_value(world, &kind, &file) else {
                    continue;
                };
                scan.added.push(relative.clone());
                value
            }
        };
        publish_name(world, &relative, &kind, &value);
        world.resource_mut::<AssetIndex>().insert(AssetEntry {
            path: relative,
            kind: kind.kind.clone(),
            type_path: kind.type_path.clone(),
            file_kind: AssetFileKind::Asset {
                type_path: kind.type_path.clone(),
            },
            value,
            mtime,
        });
    }
    publish_reference_map(world);
    scan
}

/// Keep the bare name a material file's stem gives it resolving, for the brush
/// faces, terrain slots and scenes written before references were paths.
fn publish_name(world: &mut World, path: &Path, kind: &AssetKind, value: &AssetValue) {
    if kind.kind != MATERIAL_KIND {
        return;
    }
    let Some(handle) = value.handle() else {
        return;
    };
    let name = definition_name_of(path);
    world
        .resource_mut::<crate::asset_catalog::AssetCatalog>()
        .insert(format!("@{name}"), handle.clone());
}

/// Publish what the index holds as the references a document reads and writes:
/// every loaded file under the path that names it, under the bare name it was
/// spelled by before paths when no other file shares that stem, and every
/// loaded handle under the path it is emitted as.
///
/// The open scene resolves those as well as what it embeds, and what it embeds
/// wins the spellings they share. Only its `#` entries are the document's own,
/// so a name published for a file that has since gone is not carried over.
pub fn publish_reference_map(world: &mut World) {
    let mut references: bevy::platform::collections::HashMap<String, UntypedHandle> =
        bevy::platform::collections::HashMap::default();
    let mut paths: bevy::platform::collections::HashMap<UntypedAssetId, String> =
        bevy::platform::collections::HashMap::default();
    let mut stems: HashMap<String, usize> = HashMap::new();
    let index = world.resource::<AssetIndex>();
    for entry in index.iter() {
        *stems.entry(entry.name()).or_default() += 1;
    }
    for entry in index.iter() {
        let Some(handle) = entry.value.handle() else {
            continue;
        };
        let path = entry.path.to_slash_lossy().into_owned();
        paths.insert(handle.id(), path.clone());
        references.insert(path, handle.clone());
        let name = entry.name();
        if stems.get(&name) == Some(&1) {
            references.insert(format!("@{name}"), handle.clone());
            references.entry(name).or_insert_with(|| handle.clone());
        }
    }
    let mut scene = references.clone();
    if let Some(embedded) = world.get_resource::<jackdaw_bsn::BsnSceneAssets>() {
        for (reference, handle) in &embedded.0 {
            let Some(name) = reference.strip_prefix('#') else {
                continue;
            };
            scene.insert(reference.clone(), handle.clone());
            scene.insert(format!("@{name}"), handle.clone());
        }
    }
    world.insert_resource(jackdaw_bsn::BsnProjectAssets(references));
    world.insert_resource(jackdaw_bsn::BsnSceneAssets(scene));
    world.insert_resource(jackdaw_bsn::BsnAssetPaths(paths));
}

/// Record a file the editor itself wrote, holding the value it wrote, and
/// report the path the index keys it by.
pub fn index_written(
    world: &mut World,
    file: &Path,
    kind: &AssetKind,
    value: AssetValue,
) -> Option<PathBuf> {
    let indexed = indexed_path(world, file)?;
    let mtime = std::fs::metadata(file)
        .and_then(|meta| meta.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    publish_name(world, &indexed, kind, &value);
    world.resource_mut::<AssetIndex>().insert(AssetEntry {
        path: indexed.clone(),
        kind: kind.kind.clone(),
        type_path: kind.type_path.clone(),
        file_kind: AssetFileKind::Asset {
            type_path: kind.type_path.clone(),
        },
        value,
        mtime,
    });
    publish_reference_map(world);
    Some(indexed)
}

/// Record that a file was written, so the next scan does not read back what
/// the editor itself just wrote.
pub fn note_written(world: &mut World, path: &Path) {
    let Some(relative) = indexed_path(world, path) else {
        return;
    };
    let Ok(mtime) = std::fs::metadata(path).and_then(|meta| meta.modified()) else {
        return;
    };
    if let Some(entry) = world.resource_mut::<AssetIndex>().get_mut(&relative) {
        entry.mtime = mtime;
    }
}

/// Watches the project's assets so a file written by another tool is indexed
/// without reopening the project.
#[derive(Resource)]
struct AssetFileWatcher {
    _watcher: notify::RecommendedWatcher,
    receiver: Mutex<mpsc::Receiver<()>>,
}

#[derive(Resource, Default)]
struct AssetScanPending(bool);

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<AssetIndex>()
        .init_resource::<AssetKindCache>()
        .init_resource::<AssetScanPending>()
        .add_systems(OnEnter(crate::AppState::Editor), open_asset_index)
        .add_systems(
            Update,
            (
                follow_asset_kinds.run_if(resource_changed::<AssetKinds>),
                poll_asset_watcher,
                apply_asset_scan,
            )
                .chain()
                .run_if(in_state(crate::AppState::Editor)),
        );
}

/// Index the open project's assets and start watching them.
pub fn open_asset_index(world: &mut World) {
    watch_asset_files(world);
    let scan = rescan_asset_index(world);
    if !scan.added.is_empty() {
        info!("Indexed {} asset files", scan.added.len());
    }
}

fn watch_asset_files(world: &mut World) {
    let Some(assets) = assets_dir(world) else {
        return;
    };
    let (sender, receiver) = mpsc::channel();
    let watcher =
        notify::recommended_watcher(move |event: Result<notify::Event, notify::Error>| {
            use notify::EventKind;
            if let Ok(event) = event
                && matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(_)
                )
            {
                let _ = sender.send(());
            }
        });
    let Ok(mut watcher) = watcher else { return };
    use notify::Watcher as _;
    if watcher
        .watch(&assets, notify::RecursiveMode::Recursive)
        .is_ok()
    {
        world.insert_resource(AssetFileWatcher {
            _watcher: watcher,
            receiver: Mutex::new(receiver),
        });
    }
}

fn poll_asset_watcher(
    watcher: Option<Res<AssetFileWatcher>>,
    mut pending: ResMut<AssetScanPending>,
) {
    let Some(watcher) = watcher else { return };
    let Ok(receiver) = watcher.receiver.lock() else {
        return;
    };
    if receiver.try_recv().is_ok() {
        while receiver.try_recv().is_ok() {}
        pending.0 = true;
    }
}

/// Follow the registered kinds: a kind that has gone takes its entries and its
/// open card with it, and a kind that has arrived gets the assets walked again.
fn follow_asset_kinds(world: &mut World) {
    let kinds: Vec<String> = world
        .get_resource::<AssetKinds>()
        .map(|kinds| kinds.iter().map(|kind| kind.kind.clone()).collect())
        .unwrap_or_default();
    world.resource_mut::<AssetIndex>().retain_kinds(&kinds);
    crate::definition_assets::close_card_of_missing_kind(world, &kinds);
    world.resource_mut::<AssetScanPending>().0 = true;
}

fn apply_asset_scan(world: &mut World) {
    if !std::mem::take(&mut world.resource_mut::<AssetScanPending>().0) {
        return;
    }
    rescan_asset_index(world);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stem_two_files_share_resolves_to_neither_and_names_both() {
        let mut index = AssetIndex::default();
        for folder in ["content/items", "content/props"] {
            index.insert(AssetEntry {
                path: PathBuf::from(folder).join("torch.bsn"),
                kind: "item".to_string(),
                type_path: "my_game::ItemDef".to_string(),
                file_kind: AssetFileKind::Asset {
                    type_path: "my_game::ItemDef".to_string(),
                },
                value: AssetValue::Unloaded,
                mtime: SystemTime::UNIX_EPOCH,
            });
        }

        assert!(
            index.by_stem("torch").is_none(),
            "a name two files answer to names neither"
        );
        assert_eq!(
            index.stem_paths("torch"),
            vec![
                PathBuf::from("content/items/torch.bsn"),
                PathBuf::from("content/props/torch.bsn"),
            ],
            "and both files are there to be named by their paths"
        );
    }

    #[test]
    fn a_lone_stem_still_resolves() {
        let mut index = AssetIndex::default();
        index.insert(AssetEntry {
            path: PathBuf::from("anywhere/torch.item.bsn"),
            kind: "item".to_string(),
            type_path: "my_game::ItemDef".to_string(),
            file_kind: AssetFileKind::Asset {
                type_path: "my_game::ItemDef".to_string(),
            },
            value: AssetValue::Unloaded,
            mtime: SystemTime::UNIX_EPOCH,
        });

        assert_eq!(
            index.by_stem("torch").map(|entry| entry.path.clone()),
            Some(PathBuf::from("anywhere/torch.item.bsn"))
        );
    }

    #[test]
    fn a_file_outside_the_projects_assets_is_not_indexed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut world = World::new();
        world.insert_resource(ProjectRoot {
            root: tmp.path().to_path_buf(),
            config: crate::project::ProjectConfig::default(),
        });
        let assets = tmp.path().join("assets");

        assert_eq!(
            indexed_path(&world, &assets.join("materials/slate.bsn")),
            Some(PathBuf::from("materials/slate.bsn"))
        );
        assert_eq!(
            indexed_path(&world, Path::new("materials/slate.bsn")),
            Some(PathBuf::from("materials/slate.bsn")),
            "a path already relative to the assets is taken as it stands"
        );
        assert_eq!(
            indexed_path(&world, Path::new("/elsewhere/slate.bsn")),
            None
        );
        assert_eq!(
            indexed_path(&world, Path::new("../slate.bsn")),
            None,
            "a relative path that walks out of the assets names nothing here"
        );
    }
}
