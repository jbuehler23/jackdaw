//! Tilemap cells in the editor: one store for every open tab, keyed by where
//! each map's `.jdtiles` sidecar sits and recording which document owns each
//! key, read from and written to the folder of that document.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use bevy::ecs::change_detection::Tick;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use jackdaw_api_internal::document::SceneResource;
use jackdaw_scene_types::{LayerTerrainMatch, Tilemap};
use jackdaw_tilemap::{
    CHUNK_AREA, CellCoord, ChunkCoord, ChunkedLayer, LayerData, LayerKind, TerrainSet, TileRef,
    TilemapData, resolve_region,
};
use jackdaw_tileset::TerrainMatch;

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
///
/// Every write goes through [`insert`](Self::insert), [`remove`](Self::remove)
/// or [`get_mut`](Self::get_mut), which record what the drawn chunks of the
/// key must catch up on.
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
    /// Keys whose cells changed since their maps were last drawn.
    redraw: HashMap<String, Redraw>,
    /// The terrain set each terrain layer resolves with, by key and layer
    /// index. A layer listed here holds the tiles its authoring resolves to,
    /// and an edit resolves the cells it touched with it.
    sets: HashMap<String, BTreeMap<usize, Arc<TerrainSet>>>,
    /// The map each key holds, by an id no other map is ever given. A save
    /// that moves the cells to a new key moves the id with them.
    ids: HashMap<String, u64>,
    next_id: u64,
    /// Terrain cells edits have resolved.
    pub(crate) resolved_cells: usize,
    /// Advances on every write of any key's cells.
    revision: u64,
    /// Maps whose layer count and cells were found to disagree, warned once.
    pub(crate) mismatch_warned: HashSet<u64>,
}

/// What the drawn chunks of a key must catch up on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Redraw {
    /// The cells were put or dropped whole.
    Whole,
    /// Only these chunks, by layer index, were edited.
    Chunks(BTreeSet<(usize, ChunkCoord)>),
}

impl TilemapDataStore {
    pub fn get(&self, key: &str) -> Option<&TilemapData> {
        self.entries.get(key)
    }

    /// The map for editing; the next save of its owner writes it, and the
    /// chunks the edit touched are drawn again once the edit is dropped.
    pub fn get_mut(&mut self, key: &str) -> Option<TilemapEdit<'_>> {
        let data = self.entries.get_mut(key)?;
        self.revision += 1;
        self.changed.insert(key.to_string());
        let redraw =
            (self.redraw.entry(key.to_string())).or_insert_with(|| Redraw::Chunks(BTreeSet::new()));
        let sets = self.sets.get(key);
        Some(TilemapEdit::new(
            data,
            redraw,
            sets,
            &mut self.resolved_cells,
        ))
    }

    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// Puts an edited map in place; the next save of its owner writes it. Its
    /// maps draw it whole, so the layers' dirty chunks are dropped. The map
    /// keeps its id: it is still the same document's map.
    pub fn insert(&mut self, key: impl Into<String>, mut data: TilemapData) {
        let key = key.into();
        data.layers.iter_mut().for_each(clear_dirty);
        self.revision += 1;
        self.changed.insert(key.clone());
        self.redraw.insert(key.clone(), Redraw::Whole);
        self.sets.remove(&key);
        if !self.ids.contains_key(&key) {
            self.next_id += 1;
            self.ids.insert(key.clone(), self.next_id);
        }
        self.entries.insert(key, data);
    }

    /// Records `doc` as the document whose map lives at `key`. A map another
    /// document held there is a different map, so the key gets a new map id.
    fn set_owner(&mut self, key: &str, doc: &str) {
        let before = self.owners.insert(key.to_string(), doc.to_string());
        if before.is_some_and(|before| before != doc) {
            self.next_id += 1;
            self.ids.insert(key.to_string(), self.next_id);
        }
    }

    pub fn remove(&mut self, key: &str) -> Option<TilemapData> {
        self.revision += 1;
        self.redraw.insert(key.to_string(), Redraw::Whole);
        self.sets.remove(key);
        self.ids.remove(key);
        self.read_mtimes.remove(key);
        self.load_failed.remove(key);
        self.changed.remove(key);
        self.entries.remove(key)
    }

    /// Takes data layer `index` out when `remove`, then puts `insert` in its
    /// place, keeping each other layer's resolved set under its new index.
    /// Returns the layer taken out. A layer past the end of the data is left
    /// for padding to add.
    pub(crate) fn splice_layer(
        &mut self,
        key: &str,
        index: usize,
        remove: bool,
        insert: Option<LayerData>,
    ) -> Option<LayerData> {
        let data = self.entries.get_mut(key)?;
        let remove = remove && index < data.layers.len();
        let insert = insert.filter(|_| index <= data.layers.len() - usize::from(remove));
        if !remove && insert.is_none() {
            return None;
        }
        let removed = remove.then(|| data.layers.remove(index));
        let added = insert.is_some();
        if let Some(mut layer) = insert {
            clear_dirty(&mut layer);
            data.layers.insert(index, layer);
        }
        if let Some(sets) = self.sets.get_mut(key) {
            *sets = std::mem::take(sets)
                .into_iter()
                .filter(|(at, _)| *at != index || !remove)
                .map(|(at, set)| {
                    let shift = isize::from(added) - isize::from(remove);
                    (
                        if at < index {
                            at
                        } else {
                            at.saturating_add_signed(shift)
                        },
                        set,
                    )
                })
                .collect();
        }
        self.revision += 1;
        self.changed.insert(key.to_string());
        self.redraw.insert(key.to_string(), Redraw::Whole);
        removed
    }

    /// Whether the cells at `key` changed since its maps were last drawn.
    pub fn needs_redraw(&self, key: &str) -> bool {
        self.redraw.contains_key(key)
    }

    /// What the maps at `key` must draw again, clearing it.
    pub fn take_redraw(&mut self, key: &str) -> Option<Redraw> {
        self.redraw.remove(key)
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

    /// Whether the cells at `key` were edited since they were last read or
    /// written. Tiles derived from the authoring do not count.
    pub fn has_unsaved_edits(&self, key: &str) -> bool {
        self.changed.contains(key)
    }

    /// A count that advances whenever any key's cells are written.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Terrain cells edits have resolved since the store was made.
    pub fn resolved_cells(&self) -> usize {
        self.resolved_cells
    }

    /// The id of the map held at `key`.
    pub fn map_id(&self, key: &str) -> Option<u64> {
        self.ids.get(key).copied()
    }

    /// Where the map with id `id` is held now, if it is still open.
    pub fn key_of_map(&self, id: u64) -> Option<&str> {
        (self.ids.iter()).find_map(|(key, at)| (*at == id).then_some(key.as_str()))
    }

    /// The terrain set terrain layer `layer` at `key` resolves with, if its
    /// tiles are known to be resolved.
    pub fn resolved_with(&self, key: &str, layer: usize) -> Option<&Arc<TerrainSet>> {
        self.sets.get(key)?.get(&layer)
    }

    pub(crate) fn note_resolved(&mut self, key: &str, layer: usize, set: Arc<TerrainSet>) {
        (self.sets.entry(key.to_string()).or_default()).insert(layer, set);
    }

    /// Marks a layer's tiles as resolved with no known set: edits leave them
    /// as they are, and the next refresh that has a set resolves them whole.
    pub(crate) fn forget_resolved(&mut self, key: &str, layer: usize) {
        if let Some(layers) = self.sets.get_mut(key) {
            layers.remove(&layer);
        }
    }

    /// Whether data layer `layer` at `key` differs in kind from `kind`, the
    /// kind its scene layer has.
    pub(crate) fn kind_differs(&self, key: &str, layer: usize, kind: LayerKind) -> bool {
        (self.entries.get(key))
            .and_then(|data| data.layers.get(layer))
            .is_some_and(|data| data.kind != kind)
    }

    /// Gives data layer `layer` at `key` the kind of its scene layer. The
    /// scene is the source of the kind, so this is no edit of the cells.
    pub(crate) fn sync_layer_kind(&mut self, key: &str, layer: usize, kind: LayerKind) {
        if let Some(data) = (self.entries.get_mut(key)).and_then(|data| data.layers.get_mut(layer))
        {
            data.kind = kind;
        }
    }

    /// Makes a layer's tiles equal `fresh`, derived from its authoring. The
    /// changed chunks are drawn again, but it is no edit: the key does not
    /// become unsaved.
    pub(crate) fn put_derived_tiles(
        &mut self,
        key: &str,
        layer: usize,
        fresh: &ChunkedLayer<TileRef>,
    ) {
        let Some(tiles) = (self.entries.get_mut(key))
            .and_then(|data| data.layers.get_mut(layer))
            .map(|layer| &mut layer.tiles)
        else {
            return;
        };
        self.revision += 1;
        let coords: BTreeSet<ChunkCoord> = (tiles.chunks().map(|(c, _)| c))
            .chain(fresh.chunks().map(|(c, _)| c))
            .collect();
        for coord in coords {
            let want = fresh.chunk(coord);
            if tiles.chunk(coord) == want {
                continue;
            }
            for local in 0..CHUNK_AREA {
                tiles.set(coord.cell(local), want.map_or(TileRef::EMPTY, |c| c[local]));
            }
        }
        let dirty = tiles.take_dirty();
        let redraw =
            (self.redraw.entry(key.to_string())).or_insert_with(|| Redraw::Chunks(BTreeSet::new()));
        if let Redraw::Chunks(chunks) = redraw {
            chunks.extend(dirty.into_iter().map(|chunk| (layer, chunk)));
        }
    }

    fn holds(&self, key: &str) -> bool {
        self.entries.contains_key(key) || self.load_failed.contains_key(key)
    }

    /// Puts cells just read from or written to the file at `key`.
    fn put_unchanged(&mut self, key: &str, data: TilemapData) {
        self.insert(key, data);
        self.changed.remove(key);
    }

    fn quarantine(&mut self, key: &str, reason: String) {
        self.remove(key);
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

/// A map open for editing. Dropping it resolves the terrain cells the edit
/// touched, and the ring around them, into the tiles of each layer that has a
/// set, then records the chunks the edit touched, from the layers' dirty
/// chunks and the overrides it changed.
///
/// The cells an edit touched are the ones it reports with
/// [`mark_written`](Self::mark_written), or else every cell of the chunks it
/// dirtied, plus the overrides it changed.
pub struct TilemapEdit<'a> {
    data: &'a mut TilemapData,
    redraw: &'a mut Redraw,
    sets: Option<&'a BTreeMap<usize, Arc<TerrainSet>>>,
    resolved_cells: &'a mut usize,
    /// Each layer's overrides as the edit found them.
    overrides: Vec<Option<BTreeMap<CellCoord, TileRef>>>,
    /// The authoring cells the edit reports it wrote, by layer.
    written: BTreeMap<usize, Vec<CellCoord>>,
}

impl<'a> TilemapEdit<'a> {
    fn new(
        data: &'a mut TilemapData,
        redraw: &'a mut Redraw,
        sets: Option<&'a BTreeMap<usize, Arc<TerrainSet>>>,
        resolved_cells: &'a mut usize,
    ) -> Self {
        let overrides = (data.layers.iter())
            .map(|layer| (layer.authoring.as_ref()).map(|a| a.overrides.clone()))
            .collect();
        Self {
            data,
            redraw,
            sets,
            resolved_cells,
            overrides,
            written: BTreeMap::new(),
        }
    }

    /// Reports the authoring cells of `layer` this edit wrote, so dropping it
    /// resolves only those and their ring rather than their whole chunks.
    pub fn mark_written(&mut self, layer: usize, cells: impl IntoIterator<Item = CellCoord>) {
        self.written.entry(layer).or_default().extend(cells);
    }
}

impl std::ops::Deref for TilemapEdit<'_> {
    type Target = TilemapData;
    fn deref(&self) -> &TilemapData {
        self.data
    }
}

impl std::ops::DerefMut for TilemapEdit<'_> {
    fn deref_mut(&mut self) -> &mut TilemapData {
        self.data
    }
}

impl Drop for TilemapEdit<'_> {
    fn drop(&mut self) {
        let mut whole = self.data.layers.len() != self.overrides.len();
        let mut touched = BTreeSet::new();
        for (index, layer) in self.data.layers.iter_mut().enumerate() {
            let before = self.overrides.get(index).and_then(Option::as_ref);
            let mut dirty: BTreeSet<ChunkCoord> = BTreeSet::new();
            let Some(authoring) = layer.authoring.as_mut() else {
                whole |= before.is_some();
                touched.extend(layer.tiles.take_dirty().into_iter().map(|c| (index, c)));
                continue;
            };
            dirty.extend(authoring.terrain.take_dirty());
            dirty.extend(authoring.links.take_dirty());
            let mut cells: Vec<CellCoord> = match self.written.remove(&index) {
                Some(written) => written,
                None => (dirty.iter())
                    .flat_map(|chunk| (0..CHUNK_AREA).map(|local| chunk.cell(local)))
                    .collect(),
            };
            let after = &authoring.overrides;
            match before {
                Some(before) => {
                    let moved = (before.iter().filter(|(c, t)| after.get(c) != Some(t)))
                        .chain(after.iter().filter(|(c, t)| before.get(c) != Some(t)))
                        .map(|(cell, _)| *cell);
                    cells.extend(moved);
                }
                None => {
                    whole = true;
                    cells.extend(after.keys().copied());
                }
            }
            dirty.extend(cells.iter().map(|cell| cell.chunk()));
            let set = (self.sets.and_then(|sets| sets.get(&index)))
                .filter(|_| layer.kind == LayerKind::Terrain && !cells.is_empty());
            if let Some(set) = set {
                *self.resolved_cells += resolve_into(layer, set, cells).0;
            }
            dirty.extend(layer.tiles.take_dirty());
            touched.extend(dirty.into_iter().map(|chunk| (index, chunk)));
        }
        match self.redraw {
            Redraw::Whole => {}
            Redraw::Chunks(_) if whole => *self.redraw = Redraw::Whole,
            Redraw::Chunks(chunks) => chunks.extend(touched),
        }
    }
}

/// Resolves `cells` of a terrain layer and the ring around them into its
/// tiles with `set`. Returns how many cells were resolved and each cell whose
/// tile changed, with its new tile. Every edit and the tile tools' preview
/// resolve through it.
pub fn resolve_into(
    layer: &mut LayerData,
    set: &TerrainSet,
    cells: impl IntoIterator<Item = CellCoord>,
) -> (usize, Vec<(CellCoord, TileRef)>) {
    let Some(a) = layer.authoring.as_ref() else {
        return (0, Vec::new());
    };
    let resolved = resolve_region(set, &a.terrain, &a.links, &a.overrides, cells);
    let count = resolved.len();
    let changed = (resolved.into_iter())
        .filter(|&(cell, tile)| layer.tiles.set(cell, tile) != tile)
        .collect();
    (count, changed)
}

/// The match mode a layer names, or `None` for the tileset's only one.
pub fn layer_mode(terrain_match: LayerTerrainMatch) -> Option<TerrainMatch> {
    match terrain_match {
        LayerTerrainMatch::Auto => None,
        LayerTerrainMatch::Corners => Some(TerrainMatch::Corners),
        LayerTerrainMatch::Sides => Some(TerrainMatch::Sides),
        LayerTerrainMatch::CornersAndSides => Some(TerrainMatch::CornersAndSides),
    }
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
            let expanded = |source: &Path| {
                (world.get_resource::<crate::prefab::PrefabAstCache>())
                    .is_none_or(|cache| cache.get(source).is_some())
            };
            let owner = match jackdaw_prefab::owning_source(world, entity, expanded) {
                Some(source) => {
                    let root = crate::prefab::save_load::source_root(world, &scene_dir);
                    let source =
                        crate::prefab::save_load::resolve_source_path(source, &root, &scene_dir);
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

/// The store key of `entity`'s map if the tab in front may edit its cells,
/// or why not: its path is invalid, its file could not be read, or it is
/// inherited from a prefab that no open tab holds, whose sidecar no save
/// would write.
pub fn editable_key(world: &mut World, entity: Entity) -> Result<String, String> {
    let doc = current_doc(world);
    let map = (live_maps(world, &doc).into_iter())
        .find(|map| map.entity == entity)
        .ok_or("the entity is not a tilemap")?;
    let key = map
        .key()
        .ok_or("the map's data path is invalid")?
        .to_string();
    let store = world.resource::<TilemapDataStore>();
    if let Some(reason) = store.load_failed_reason(&key) {
        return Err(format!("the map's cells could not be loaded ({reason})"));
    }
    if !store.contains(&key) {
        return Err("the map's cells are not loaded".into());
    }
    if map.inherited {
        let tabs = world.get_resource::<crate::scenes::Scenes>();
        let paths: Vec<PathBuf> = tabs.map_or(Vec::new(), |scenes| {
            (scenes.tabs.iter())
                .filter_map(|tab| tab.path.clone())
                .collect()
        });
        if !(paths.iter()).any(|path| Doc::file(world, path).id == map.owner.id) {
            return Err(format!(
                "the map is inherited from {}; open the prefab in a tab to edit its cells",
                map.owner.id
            ));
        }
    }
    Ok(key)
}

/// The store key of every live map with a valid data path.
pub fn store_keys(world: &mut World) -> HashMap<Entity, String> {
    let doc = current_doc(world);
    (live_maps(world, &doc).into_iter())
        .filter_map(|map| Some((map.entity, map.located?.1)))
        .collect()
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

fn clear_dirty(layer: &mut LayerData) {
    layer.tiles.take_dirty();
    if let Some(authoring) = &mut layer.authoring {
        authoring.terrain.take_dirty();
        authoring.links.take_dirty();
    }
}

/// A data path no map, entry or file of the tab in front holds.
pub(crate) fn mint_map_path(world: &mut World) -> String {
    let doc = current_doc(world);
    let taken = (live_maps(world, &doc).iter())
        .filter_map(|map| map.key().map(String::from))
        .collect();
    mint(world, &doc, &taken)
}

/// Puts the cells of a new map at `data_path` of the tab in front: `held`
/// with its id when a redo brings them back, else an empty map. Returns the
/// map id.
pub(crate) fn claim_map(
    world: &mut World,
    data_path: &str,
    held: Option<(TilemapData, u64)>,
) -> Option<u64> {
    let doc = current_doc(world);
    let (_, key) = doc.locate(world, data_path)?;
    let mut store = world.resource_mut::<TilemapDataStore>();
    store.set_owner(&key, &doc.id);
    let (cells, id) = held.unzip();
    store.insert(key.clone(), cells.unwrap_or_default());
    if let Some(id) = id {
        store.ids.insert(key.clone(), id);
    }
    store.map_id(&key)
}

/// Drops the cells of map `id` wherever they are held now, returning them.
pub(crate) fn release_map(world: &mut World, id: u64) -> Option<(TilemapData, u64)> {
    let mut store = world.resource_mut::<TilemapDataStore>();
    let key = store.key_of_map(id)?.to_owned();
    Some((store.remove(&key)?, id))
}

/// The cells kind of a scene layer kind.
pub(crate) fn data_kind(kind: jackdaw_scene_types::TilemapLayerKind) -> LayerKind {
    match kind {
        jackdaw_scene_types::TilemapLayerKind::Terrain => LayerKind::Terrain,
        jackdaw_scene_types::TilemapLayerKind::Tile => LayerKind::Tile,
    }
}

/// Records the key of `data_path` as `doc`'s, with `cells` there when given
/// and an empty map when the key holds nothing yet.
fn claim(world: &mut World, doc: &Doc, data_path: &str, cells: Option<TilemapData>) {
    let Some((_, key)) = doc.locate(world, data_path) else {
        return;
    };
    let mut store = world.resource_mut::<TilemapDataStore>();
    store.set_owner(&key, &doc.id);
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
            store.set_owner(to_key, &doc.id);
            continue;
        }
        let reason = store.load_failed.remove(from_key);
        let (id, sets) = (
            store.ids.get(from_key).copied(),
            store.sets.remove(from_key),
        );
        if let Some(cells) = store.remove(from_key) {
            store.put_unchanged(to_key, cells);
            if let Some(id) = id {
                store.ids.insert(to_key.clone(), id);
            }
            if let Some(sets) = sets {
                store.sets.insert(to_key.clone(), sets);
            }
        }
        if let Some(reason) = reason {
            store.load_failed.insert(to_key.clone(), reason);
        }
        if store
            .owner(from_key)
            .is_some_and(|owner| owner.starts_with("unsaved-"))
        {
            store.owners.remove(from_key);
        }
        store.set_owner(to_key, &doc.id);
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
                store.put_unchanged(&key, data);
                store.clear_load_failed(&key);
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                store.clear_load_failed(&key);
                store.put_unchanged(&key, TilemapData::default());
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
        let mut store = world.resource_mut::<TilemapDataStore>();
        store.note_read(&key, &full);
        if !renamed {
            store.changed.remove(&key);
            store.set_owner(&key, &doc.id);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jackdaw_tileset::{Tileset, TilesetTerrainSet};

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
        let warning = (tileset.terrain_set(layer_mode(LayerTerrainMatch::Auto)))
            .expect_err("ambiguous")
            .to_string();
        assert!(warning.contains("pick one match mode"));
        assert!((tileset.terrain_set(layer_mode(LayerTerrainMatch::Sides))).is_ok());
    }
}
