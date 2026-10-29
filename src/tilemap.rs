//! Tilemap cells in the editor: one store for every open tab, keyed by where
//! each map's `.jdtiles` sidecar sits and recording which document owns each
//! key, read from and written to the folder of that document.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use bevy::ecs::change_detection::Tick;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use jackdaw_api_internal::document::SceneResource;
use jackdaw_scene_types::{LayerTerrainMatch, Tilemap, TilemapLayer};
use jackdaw_tilemap::{TerrainSet, TilemapData};
use jackdaw_tileset::{TerrainMatch, Tileset};

use crate::prefab::{IsA, Prefab, PrefabEntityId};
use crate::scene_io::SidecarImport;

pub fn plugin(app: &mut App) {
    app.init_resource::<TilemapDataStore>()
        .init_resource::<TilemapDocument>()
        .add_systems(
            Update,
            ensure_tilemap_data_path.run_if(in_state(crate::AppState::Editor)),
        );
    app.world_mut()
        .spawn(SceneResource::of::<TilemapDocument>());
}

/// The document the tab in front keys its maps under, kept per tab.
#[derive(Resource, Default)]
pub struct TilemapDocument {
    /// The file the tab was last loaded from or saved to.
    file: Option<String>,
    /// Names the tab while it has no file; 0 until first needed.
    unsaved: u64,
}

/// Decoded tilemap sidecars of every open tab, keyed by [`store_key_of`]: the
/// sidecar's path under the project, or absolute; `unsaved-N/...` for a tab
/// with no file.
#[derive(Resource, Default)]
pub struct TilemapDataStore {
    entries: HashMap<String, TilemapData>,
    /// Keys whose file exists but could not be read, with the reason. Their
    /// files are never written.
    load_failed: HashMap<String, String>,
    read_mtimes: HashMap<String, SystemTime>,
    /// Keys edited since they were last read or written.
    changed: HashSet<String>,
    /// The document whose maps live at each key.
    owners: HashMap<String, String>,
    next_unsaved: u64,
}

impl TilemapDataStore {
    pub fn get(&self, key: &str) -> Option<&TilemapData> {
        self.entries.get(key)
    }

    /// The map for editing; the next save of its owner writes it.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut TilemapData> {
        self.changed.insert(key.to_string());
        self.entries.get_mut(key)
    }

    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// Puts an edited map in place; the next save of its owner writes it.
    pub fn insert(&mut self, key: impl Into<String>, data: TilemapData) {
        let key = key.into();
        self.changed.insert(key.clone());
        self.entries.insert(key, data);
    }

    pub fn remove(&mut self, key: &str) -> Option<TilemapData> {
        self.read_mtimes.remove(key);
        self.load_failed.remove(key);
        self.changed.remove(key);
        self.entries.remove(key)
    }

    pub fn load_failed_reason(&self, key: &str) -> Option<&str> {
        self.load_failed.get(key).map(String::as_str)
    }

    /// Lifts the quarantine on a key, for a read that decoded.
    pub fn clear_load_failed(&mut self, key: &str) {
        self.load_failed.remove(key);
    }

    /// The document whose maps live at `key`, if one has claimed it.
    pub fn owner(&self, key: &str) -> Option<&str> {
        self.owners.get(key).map(String::as_str)
    }

    fn holds(&self, key: &str) -> bool {
        self.entries.contains_key(key) || self.load_failed.contains_key(key)
    }

    fn quarantine(&mut self, key: &str, reason: String) {
        self.entries.remove(key);
        self.changed.remove(key);
        self.load_failed.insert(key.to_string(), reason);
    }

    fn note_read(&mut self, key: &str, full: &Path) {
        match std::fs::metadata(full).and_then(|meta| meta.modified()) {
            Ok(mtime) => self.read_mtimes.insert(key.to_string(), mtime),
            Err(_) => self.read_mtimes.remove(key),
        };
    }

    /// Whether the file has changed since the store read it, or appeared
    /// after the store found it missing.
    fn is_stale(&self, key: &str, full: &Path) -> bool {
        let Ok(mtime) = std::fs::metadata(full).and_then(|meta| meta.modified()) else {
            return false;
        };
        self.read_mtimes.get(key).is_none_or(|read| *read != mtime)
    }
}

/// The solver set terrain layer `layer` resolves with, or a message saying
/// what the user has to fix.
pub fn layer_terrain_set(tileset: &Tileset, layer: &TilemapLayer) -> Result<TerrainSet, String> {
    let mode = match layer.terrain_match {
        LayerTerrainMatch::Auto => None,
        LayerTerrainMatch::Corners => Some(TerrainMatch::Corners),
        LayerTerrainMatch::Sides => Some(TerrainMatch::Sides),
        LayerTerrainMatch::CornersAndSides => Some(TerrainMatch::CornersAndSides),
    };
    tileset
        .terrain_set(mode)
        .map_err(|err| format!("Tilemap layer {:?}: {err}", layer.name))
}

fn parent_dir(path: &Path) -> PathBuf {
    path.parent().map(Path::to_path_buf).unwrap_or_default()
}

/// A path's store form: under the project root when one is open and holds
/// it, else the path itself, with `.` and `..` folded away.
fn store_key(world: &World, full: &Path) -> String {
    let full = jackdaw_prefab::normalize_path(full);
    let root = world.get_resource::<crate::project::ProjectRoot>();
    let under = root.and_then(|root| {
        full.strip_prefix(jackdaw_prefab::normalize_path(&root.root))
            .ok()
    });
    under.unwrap_or(&full).to_string_lossy().replace('\\', "/")
}

/// A document maps can belong to: a scene or prefab file, or an unsaved tab.
#[derive(Clone)]
struct Doc {
    id: String,
    /// The file's folder; `None` for an unsaved tab.
    dir: Option<PathBuf>,
    stem: String,
}

impl Doc {
    fn file(world: &World, path: &Path) -> Self {
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        Self {
            id: store_key(world, path),
            dir: Some(parent_dir(path)),
            stem: (stem.chars())
                .map(|c| {
                    if matches!(c, ':' | '\\' | '/') {
                        '_'
                    } else {
                        c
                    }
                })
                .collect(),
        }
    }

    /// Where `data_path` of one of this document's maps sits (no file for an
    /// unsaved tab), and its store key.
    fn locate(&self, world: &World, data_path: &str) -> Option<(Option<PathBuf>, String)> {
        match &self.dir {
            Some(dir) => {
                let full = jackdaw_tilemap::resolve_path(dir, data_path).ok()?;
                let key = store_key(world, &full);
                Some((Some(full), key))
            }
            None => {
                jackdaw_tilemap::resolve_path(Path::new(""), data_path).ok()?;
                Some((None, format!("{}/{data_path}", self.id)))
            }
        }
    }
}

/// The document of the tab in front.
fn current_doc(world: &mut World) -> Doc {
    let (file, unsaved) = (world.get_resource::<TilemapDocument>())
        .map_or((None, 0), |doc| (doc.file.clone(), doc.unsaved));
    if let Some(file) = file {
        return Doc::file(world, Path::new(&file));
    }
    let id = if unsaved != 0 {
        unsaved
    } else {
        let mut store = world.resource_mut::<TilemapDataStore>();
        store.next_unsaved += 1;
        let id = store.next_unsaved;
        if let Some(mut doc) = world.get_resource_mut::<TilemapDocument>() {
            doc.unsaved = id;
        }
        id
    };
    Doc {
        id: format!("unsaved-{id}"),
        dir: None,
        stem: "untitled".to_string(),
    }
}

/// The prefab instance an inherited entity came from, if it is one. The walk
/// stops at a prefab's own root, whose document (a variant's, say) owns it.
fn owning_instance(world: &World, entity: Entity) -> Option<&IsA> {
    world.get::<PrefabEntityId>(entity)?;
    let mut current = Some(entity);
    while let Some(e) = current {
        if world.get::<Prefab>(e).is_some() {
            return None;
        }
        if let Some(isa) = world.get::<IsA>(e) {
            return Some(isa);
        }
        current = world.get::<ChildOf>(e).map(ChildOf::parent);
    }
    None
}

struct LiveMap {
    entity: Entity,
    data_path: String,
    /// The document the map belongs to: the tab's, or the prefab it is
    /// inherited from.
    owner: Doc,
    inherited: bool,
    /// The sidecar and its key, or `None` for an empty or invalid path.
    located: Option<(Option<PathBuf>, String)>,
    changed: Tick,
}

impl LiveMap {
    fn key(&self) -> Option<&str> {
        self.located.as_ref().map(|(_, key)| key.as_str())
    }
}

fn live_maps(world: &mut World, doc: &Doc) -> Vec<LiveMap> {
    let scene_dir = doc.dir.clone().unwrap_or_default();
    let mut query = world.query::<(Entity, Ref<Tilemap>)>();
    let world: &World = world;
    let mut maps: Vec<LiveMap> = (query.iter(world))
        .map(|(entity, tilemap)| {
            let owner = match owning_instance(world, entity) {
                Some(isa) => {
                    let root = crate::prefab::save_load::source_root(world, &scene_dir);
                    let source = crate::prefab::save_load::resolve_source_path(
                        &isa.source,
                        &root,
                        &scene_dir,
                    );
                    Doc::file(world, &source)
                }
                None => doc.clone(),
            };
            LiveMap {
                entity,
                data_path: tilemap.data_path.clone(),
                located: owner.locate(world, &tilemap.data_path),
                inherited: owner.id != doc.id,
                owner,
                changed: tilemap.last_changed(),
            }
        })
        .collect();
    maps.sort_by_key(|map| map.entity.index());
    maps
}

/// The store key of `entity`'s map, resolved against the document it
/// belongs to.
pub fn store_key_of(world: &mut World, entity: Entity) -> Option<String> {
    let doc = current_doc(world);
    (live_maps(world, &doc).into_iter())
        .find(|map| map.entity == entity)
        .and_then(|map| map.located.map(|(_, key)| key))
}

/// A data path for a map of `doc` that no other document owns, no map or
/// entry uses, and no file of another document holds.
fn mint(world: &World, doc: &Doc, taken: &HashSet<String>) -> String {
    let store = world.resource::<TilemapDataStore>();
    jackdaw_tilemap::mint_data_path(&doc.stem, |candidate| {
        let Some((full, key)) = doc.locate(world, candidate) else {
            return true;
        };
        let on_disk = full.is_some_and(|full| full.exists());
        taken.contains(&key)
            || match store.owner(&key) {
                Some(owner) => owner != doc.id || store.holds(&key),
                None => store.holds(&key) || on_disk,
            }
    })
}

/// Gives every changed map of the tab's own document a data path of its own.
pub fn ensure_tilemap_data_path(maps: Query<Entity, Changed<Tilemap>>, mut commands: Commands) {
    let batch: Vec<Entity> = maps.iter().collect();
    if !batch.is_empty() {
        commands.queue(move |world: &mut World| ensure_paths(world, &batch));
    }
}

/// Gives each changed map of the tab's own document a data path of its own:
/// - none or an invalid one: a fresh one;
/// - a key another document owns (a paste, an undo to an old path, an edit to
///   another map's path), or a file this scene never loaded: a fresh one
///   holding a copy of those cells;
/// - the key of another map of this document that changed earlier (a
///   duplicate): the newer map moves, with a copy of the cells.
///
/// Maps inherited from a prefab keep the prefab's key and are never minted.
fn ensure_paths(world: &mut World, batch: &[Entity]) {
    let doc = current_doc(world);
    let live = live_maps(world, &doc);
    let now = world.change_tick();
    let mut taken: HashSet<String> = live
        .iter()
        .filter_map(|m| m.key().map(String::from))
        .collect();
    for map in live.iter().filter(|m| batch.contains(&m.entity)) {
        if map.inherited {
            if map.data_path.is_empty() {
                warn!("An inherited tilemap has no data path; its prefab must give it one");
            }
            continue;
        }
        let Some((full, key)) = map.located.clone() else {
            if !map.data_path.is_empty() {
                warn!(
                    "Tilemap data path {:?} is invalid; the map gets a new one",
                    map.data_path
                );
            }
            let minted = mint(world, &doc, &taken);
            claim(world, &doc, &minted, None);
            set_data_path(world, map.entity, &minted, &mut taken, &doc);
            continue;
        };
        let store = world.resource::<TilemapDataStore>();
        let foreign = store.owner(&key).is_some_and(|owner| owner != doc.id);
        let newer_twin = live.iter().any(|other| {
            other.entity != map.entity
                && !other.inherited
                && other.key() == Some(key.as_str())
                && (map.changed.is_newer_than(other.changed, now)
                    || (map.changed == other.changed && map.entity.index() > other.entity.index()))
        });
        let unloaded = store.owner(&key).is_none()
            && !store.holds(&key)
            && full.as_ref().is_some_and(|full| full.exists());
        if !(foreign || newer_twin || unloaded) {
            if store.owner(&key).is_none() {
                claim(world, &doc, &map.data_path, None);
            }
            continue;
        }
        let cells = copy_source(world, &key, full.as_deref());
        let minted = mint(world, &doc, &taken);
        match cells {
            Ok(cells) => claim(world, &doc, &minted, Some(cells)),
            Err(why) => warn!("Copied tilemap {:?} gets no cells: {why}", map.data_path),
        }
        set_data_path(world, map.entity, &minted, &mut taken, &doc);
    }
}

/// Records the key of `data_path` as `doc`'s, with `cells` there when given
/// and an empty map when the key holds nothing yet.
fn claim(world: &mut World, doc: &Doc, data_path: &str, cells: Option<TilemapData>) {
    let Some((_, key)) = doc.locate(world, data_path) else {
        return;
    };
    let mut store = world.resource_mut::<TilemapDataStore>();
    store.owners.insert(key.clone(), doc.id.clone());
    match cells {
        Some(cells) => store.insert(key, cells),
        None if !store.holds(&key) => store.insert(key, TilemapData::default()),
        None => {}
    }
}

/// The cells a copy of the map at `key` takes: the store's, or the file's
/// when the store never read it. A quarantined or unreadable source gives
/// none.
fn copy_source(world: &World, key: &str, full: Option<&Path>) -> Result<TilemapData, String> {
    let store = world.resource::<TilemapDataStore>();
    if let Some(reason) = store.load_failed_reason(key) {
        return Err(format!("its source could not be loaded ({reason})"));
    }
    if let Some(cells) = store.get(key) {
        return Ok(cells.clone());
    }
    let Some(full) = full else {
        return Ok(TilemapData::default());
    };
    match std::fs::read(full) {
        Ok(bytes) => jackdaw_tilemap::decode(&bytes).map_err(|err| err.to_string()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(TilemapData::default()),
        Err(err) => Err(err.to_string()),
    }
}

/// Points `entity`'s map at `data_path`, in the world and the document.
fn set_data_path(
    world: &mut World,
    entity: Entity,
    data_path: &str,
    taken: &mut HashSet<String>,
    doc: &Doc,
) {
    if let Some((_, key)) = doc.locate(world, data_path) {
        taken.insert(key);
    }
    let Some(mut tilemap) = world.get_mut::<Tilemap>(entity) else {
        return;
    };
    tilemap.data_path = data_path.to_string();
    let tilemap = tilemap.clone();
    crate::commands::sync_component_to_ast(world, entity, Tilemap::type_path(), &tilemap);
}

/// The data path changes a save to a new file makes, applied to the maps
/// and the document ahead of the write and kept only if it succeeds.
pub(crate) struct SaveRenames {
    /// The document being written.
    doc: Option<Doc>,
    /// Entity, old and new data path, old and new key.
    renames: Vec<(Entity, String, String, String, String)>,
    /// The file the tab was keyed under before, for a rollback.
    previous_file: Option<String>,
}

impl SaveRenames {
    /// The file a renamed map's cells were kept in before the save.
    fn old_file(&self, key: &str) -> Option<PathBuf> {
        let (_, from, ..) = self.renames.iter().find(|rename| rename.4 == key)?;
        let previous = self.previous_file.as_ref()?;
        jackdaw_tilemap::resolve_path(&parent_dir(Path::new(previous)), from).ok()
    }

    /// The key the cells now saved at `key` were held under.
    fn source_of<'a>(&'a self, key: &'a str) -> &'a str {
        (self.renames.iter())
            .find(|rename| rename.4 == key)
            .map_or(key, |rename| rename.3.as_str())
    }
}

/// Moves the tab's maps to the file `path` it is about to be saved to. A map
/// whose data path is not beside the new file under its stem gets a fresh
/// one; the old file stays with the document that owns it.
pub(crate) fn begin_save(world: &mut World, path: &str) -> SaveRenames {
    let mut renames = SaveRenames {
        doc: None,
        renames: Vec::new(),
        previous_file: None,
    };
    if !world.contains_resource::<TilemapDataStore>() {
        return renames;
    }
    let old = current_doc(world);
    let new = Doc::file(world, Path::new(path));
    renames.previous_file = world
        .get_resource::<TilemapDocument>()
        .and_then(|d| d.file.clone());
    renames.doc = Some(new.clone());
    if old.id == new.id {
        return renames;
    }
    let live = live_maps(world, &old);
    let mut taken = HashSet::new();
    let mut moved: HashMap<String, String> = HashMap::new();
    for map in live.iter().filter(|m| !m.inherited) {
        let Some(old_key) = map.key().map(String::from) else {
            continue;
        };
        let keep = old.dir == new.dir && map.data_path.starts_with(&format!("{}.", new.stem));
        let to = match moved.get(&map.data_path) {
            Some(to) => to.clone(),
            None if keep => map.data_path.clone(),
            None => mint(world, &new, &taken),
        };
        moved.insert(map.data_path.clone(), to.clone());
        let Some((_, new_key)) = new.locate(world, &to) else {
            continue;
        };
        taken.insert(new_key.clone());
        renames.renames.push((
            map.entity,
            map.data_path.clone(),
            to.clone(),
            old_key,
            new_key,
        ));
        if to != map.data_path {
            set_data_path(world, map.entity, &to, &mut taken, &new);
        }
    }
    if let Some(mut doc) = world.get_resource_mut::<TilemapDocument>() {
        doc.file = Some(path.to_string());
    }
    renames
}

/// Keeps the renames of a save that wrote everything, moving the cells and
/// ownership to the new keys, or undoes them for one that failed.
pub(crate) fn finish_save(world: &mut World, renames: SaveRenames, written: bool) {
    let Some(doc) = renames.doc else {
        return;
    };
    if !written {
        if let Some(mut tab_doc) = world.get_resource_mut::<TilemapDocument>() {
            tab_doc.file = renames.previous_file;
        }
        let old = current_doc(world);
        let mut taken = HashSet::new();
        for (entity, from, to, ..) in &renames.renames {
            if from != to {
                set_data_path(world, *entity, from, &mut taken, &old);
            }
        }
        return;
    }
    let mut store = world.resource_mut::<TilemapDataStore>();
    for (_, _, _, from_key, to_key) in &renames.renames {
        if from_key == to_key {
            store.owners.insert(to_key.clone(), doc.id.clone());
            continue;
        }
        if let Some(cells) = store.entries.remove(from_key) {
            store.entries.insert(to_key.clone(), cells);
        }
        if let Some(reason) = store.load_failed.remove(from_key) {
            store.load_failed.insert(to_key.clone(), reason);
        }
        store.changed.remove(from_key);
        store.read_mtimes.remove(from_key);
        if store
            .owner(from_key)
            .is_some_and(|owner| owner.starts_with("unsaved-"))
        {
            store.owners.remove(from_key);
        }
        store.owners.insert(to_key.clone(), doc.id.clone());
    }
}

/// Reads the sidecar of every tilemap in the world into the store and claims
/// the keys for the documents the maps belong to, returning a warning per map
/// that loaded empty or not at all.
///
/// A missing file loads an empty map. A file that cannot be read or decoded
/// leaves no entry and is quarantined, so a save keeps the file instead of
/// writing over it; it is read again only once its file changes or goes. A
/// key with unsaved edits is only re-read by an explicit load of its owner.
pub(crate) fn import_tilemap_sidecars(
    world: &mut World,
    scene_path: &str,
    mode: SidecarImport,
) -> Vec<String> {
    if !world.contains_resource::<TilemapDataStore>() {
        return Vec::new();
    }
    if let Some(mut doc) = world.get_resource_mut::<TilemapDocument>() {
        doc.file = Some(scene_path.to_string());
    }
    let doc = current_doc(world);
    let mut warnings = Vec::new();
    let mut seen = HashSet::new();
    for map in live_maps(world, &doc) {
        let Some((Some(full), key)) = map.located else {
            if !map.data_path.is_empty() {
                warnings.push(format!("Tilemap data path {:?} is invalid", map.data_path));
            }
            continue;
        };
        if !seen.insert(key.clone()) {
            continue;
        }
        let mut store = world.resource_mut::<TilemapDataStore>();
        let owner = store
            .owners
            .entry(key.clone())
            .or_insert(map.owner.id.clone());
        if *owner != map.owner.id {
            continue;
        }
        let stale = store.is_stale(&key, &full) && !store.changed.contains(&key);
        let wanted = match mode {
            SidecarImport::Reload if !map.inherited => true,
            SidecarImport::Reload | SidecarImport::FillMissing => !store.contains(&key),
            SidecarImport::RefreshChanged => !store.contains(&key) || stale,
        };
        let quarantined = store.load_failed_reason(&key).is_some();
        let gone = !full.exists();
        if !(wanted || quarantined && gone) || (quarantined && !stale && !gone) {
            continue;
        }
        let read = std::fs::read(&full);
        match read.map(|bytes| jackdaw_tilemap::decode(&bytes)) {
            Ok(Ok(data)) => {
                store.entries.insert(key.clone(), data);
                store.changed.remove(&key);
                store.clear_load_failed(&key);
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                store.clear_load_failed(&key);
                store.entries.insert(key.clone(), TilemapData::default());
                warnings.push(format!(
                    "Tilemap data {} is missing; loading an empty map",
                    full.display()
                ));
            }
            Ok(Err(err)) => {
                store.quarantine(&key, err.to_string());
                warnings.push(format!(
                    "Tilemap data {} is unreadable ({err}); saving keeps the file \
                     until it is fixed",
                    full.display()
                ));
            }
            Err(err) => {
                store.quarantine(&key, err.to_string());
                warnings.push(format!(
                    "Tilemap data {} cannot be read ({err}); saving keeps the file \
                     until it is fixed",
                    full.display()
                ));
            }
        }
        store.note_read(&key, &full);
    }
    for warning in &warnings {
        warn!("{warning}");
    }
    if let Some(first) = warnings.first() {
        crate::status_bar::notify_warn(world, first.clone());
    }
    warnings
}

/// Writes each live map of the document being saved whose cells changed
/// since they were read or written, or whose file is missing. Maps of other
/// documents (inherited from a prefab, say) are written by their own saves.
///
/// An invalid path and a quarantined key are skipped with a warning and never
/// fail the save; a quarantined file moving with a Save As is copied as it is.
pub(crate) fn export_tilemap_sidecars(
    world: &mut World,
    scene_path: &str,
    renames: &SaveRenames,
) -> Result<(), BevyError> {
    if !world.contains_resource::<TilemapDataStore>() {
        return Ok(());
    }
    let doc = match &renames.doc {
        Some(doc) => doc.clone(),
        None => Doc::file(world, Path::new(scene_path)),
    };
    let mut writes = Vec::new();
    let mut seen = HashSet::new();
    for map in live_maps(world, &doc) {
        if map.inherited {
            continue;
        }
        let Some((Some(full), key)) = map.located else {
            warn!(
                "Not saving tilemap data {:?}: the path is invalid",
                map.data_path
            );
            continue;
        };
        if !seen.insert(key.clone()) {
            continue;
        }
        let source = renames.source_of(&key);
        let store = world.resource::<TilemapDataStore>();
        let owner = store.owner(&key);
        let renamed = source != key;
        if !renamed && owner.is_some_and(|owner| owner != doc.id) {
            continue;
        }
        if let Some(reason) = store.load_failed_reason(source) {
            warn!("Not saving tilemap data {key:?}: it could not be loaded ({reason})");
            if let Some(old_full) = renamed.then(|| renames.old_file(&key)).flatten()
                && let Err(err) = std::fs::copy(&old_full, &full)
            {
                warn!("Tilemap data {} was not copied: {err}", old_full.display());
            }
            continue;
        }
        let unchanged = !renamed && !store.changed.contains(&key) && full.exists();
        let Some(data) = store.get(source).filter(|_| !unchanged) else {
            continue;
        };
        writes.push((key, full, jackdaw_tilemap::encode(data), renamed));
    }
    for (key, full, bytes, renamed) in writes {
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("cannot make {}: {err}", parent.display()))?;
        }
        jackdaw_api_internal::document::write_atomic(&full, &bytes)
            .map_err(|err| format!("failed to write tilemap data {}: {err}", full.display()))?;
        if !renamed {
            let mut store = world.resource_mut::<TilemapDataStore>();
            store.changed.remove(&key);
            store.owners.insert(key.clone(), doc.id.clone());
            store.note_read(&key, &full);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jackdaw_tileset::TilesetTerrainSet;

    #[test]
    fn a_layer_on_a_mixed_mode_tileset_is_told_to_pick_a_mode() {
        let set = |inner, mode| TilesetTerrainSet {
            inner,
            outer: 1,
            mode,
            ..Default::default()
        };
        let tileset = Tileset {
            terrain_sets: vec![set(2, TerrainMatch::Sides), set(3, TerrainMatch::Corners)],
            ..Default::default()
        };
        let mut layer = TilemapLayer {
            name: "Ground".into(),
            ..Default::default()
        };
        let warning = layer_terrain_set(&tileset, &layer).expect_err("ambiguous");
        assert!(warning.contains("Ground") && warning.contains("pick one match mode"));
        layer.terrain_match = LayerTerrainMatch::Sides;
        assert!(layer_terrain_set(&tileset, &layer).is_ok());
    }
}
