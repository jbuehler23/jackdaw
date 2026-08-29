//! In-memory source of truth for terrain bulk data.
//!
//! Heights, control words, the material list and paint-channel values never
//! enter the scene document. They live here as the same region document the
//! sidecar holds, keyed by the scene-relative sidecar path the `Terrain`
//! component carries, and are written to that sidecar when the scene is saved.
//!
//! This resource holds the active tab's decoded data. Inactive
//! [`crate::scenes::SceneTab`]s own their stores directly: tab capture moves the
//! live resource into the outgoing tab and activation restores the incoming
//! tab's store before importing sidecars (`src/scenes/swap.rs`), which keeps
//! identical scene-relative paths isolated between tabs.

use std::borrow::Cow;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
#[cfg(test)]
use jackdaw_terrain::region::RegionSize;
use jackdaw_terrain::sidecar::{self, AutoTerrainSettings, RegionTerrainData, TerrainMaterialSlot};
use jackdaw_terrain::texture_set::MAX_TEXTURES;
use jackdaw_terrain::{Control, GridRect, GridShape};

/// Why a change to a terrain's material list was refused.
///
/// A terrain stores material *names*, so every refusal is about a name that
/// could not be stored durably, not about pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerrainMaterialError {
    /// The material has never been saved, so it has no durable identity to
    /// reference. Its textures exist only for as long as this editor runs.
    Unsaved(String),
    /// No material of that name exists at all.
    Unknown(String),
    /// The name could not address a material file, so storing it would
    /// make every later save of this scene fail.
    InvalidName {
        name: String,
        reason: sidecar::MaterialNameError,
    },
    /// The material already holds a texture id on this terrain. One
    /// material per id: a second slot would burn an id and an array layer.
    AlreadyUsed(String),
    /// The list is already at [`MAX_TEXTURES`].
    Full,
    /// No such slot on this terrain.
    NoSuchSlot(usize),
    /// The slot is a vacated texture id, held open by a removed material.
    /// There is no material there to act on.
    EmptySlot(usize),
    /// The terrain's data failed to load, so there is no document to edit
    /// and nothing this store may write for that path.
    LoadFailed,
}

impl core::fmt::Display for TerrainMaterialError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Unsaved(name) => write!(
                f,
                "'{name}' is not saved yet; save it in the Materials panel first, \
                 so this terrain keeps a name it can still resolve after a restart"
            ),
            Self::Unknown(name) => write!(f, "no material named '{name}'"),
            Self::InvalidName { name, reason } => {
                write!(f, "'{name}' cannot be referenced: {reason}")
            }
            Self::AlreadyUsed(name) => write!(
                f,
                "'{name}' already has a texture id on this terrain; each id needs \
                 its own material"
            ),
            Self::Full => write!(
                f,
                "this terrain already has {MAX_TEXTURES} materials, the limit"
            ),
            Self::NoSuchSlot(index) => write!(f, "this terrain has no material slot {index}"),
            Self::EmptySlot(index) => write!(
                f,
                "texture id {index} has no material; add one to fill it back in"
            ),
            Self::LoadFailed => write!(
                f,
                "this terrain's data failed to load; fix or replace the sidecar file \
                 and reload the scene"
            ),
        }
    }
}

impl core::error::Error for TerrainMaterialError {}

/// The active scene's terrain bulk data, keyed by
/// [`jackdaw_scene_types::Terrain::data_path`].
///
/// One [`RegionTerrainData`] per terrain: the same document the sidecar holds,
/// so heights, control words, the material list and the paint channels all
/// reach disk from one place.
#[derive(Resource, Default)]
pub struct TerrainDataStore {
    entries: HashMap<String, RegionTerrainData>,
    /// Sidecar paths whose most recent read attempt found a file but could not
    /// decode it (corrupt bytes, a newer format version), mapped to the reason.
    ///
    /// Distinct from a path with no entry, which means never loaded or created
    /// and stays lenient. Real data exists on disk for a load-failed path, and
    /// this store must not fabricate a zeroed replacement for it, either for
    /// editing or for save to write back over the original. The reason is kept
    /// so the Textures tab and the inspector can show why the terrain is
    /// quarantined.
    load_failed: HashMap<String, String>,
    /// Which block of each terrain's control map has been written since the
    /// renderer last uploaded it.
    ///
    /// Kept beside [`Self::entries`] rather than in the document, which is what
    /// gets persisted. Behind a lock for the reason [`Self::heightmaps`] is:
    /// the renderer asks every frame, and asking must not read as writing.
    control_dirty: Mutex<HashMap<String, GridRect>>,
    /// Sampling views handed out by [`Self::heightmap`], keyed the same way
    /// [`Self::entries`] is.
    ///
    /// Behind a lock so that reading one takes `&self`. Through `&mut` it would
    /// flag the whole resource written on every frame a brush hovers a terrain,
    /// and everything gated on `is_changed`, such as the material tiling
    /// fields, would run against a store nobody had written.
    heightmaps: Mutex<HashMap<String, CachedHeightmap>>,
}

/// A terrain's heights ready to sample, with the height range a raycast needs
/// available on demand.
///
/// Shared rather than owned: the brush target, the ring gizmo and the meshers
/// all read the same allocation and none of them may write it. A reader takes a
/// clone for the frame it needs and drops it again; one held across frames
/// keeps the store from reusing the allocation and from patching it in place,
/// forcing a full copy on every write.
#[derive(Clone)]
pub struct SharedHeightmap {
    pub map: Arc<jackdaw_terrain::Heightmap>,
    /// [`jackdaw_terrain::Heightmap::height_bounds`] of [`Self::map`],
    /// behind [`Self::bounds`].
    ///
    /// Derived rather than stored, so it cannot drift from the map it
    /// describes, and scanned lazily: only the brush-target raycast wants it,
    /// and a drag writes the heights far more often than a raycast asks their
    /// range.
    bounds: Arc<OnceLock<(f32, f32)>>,
}

impl SharedHeightmap {
    /// Lowest and highest height in [`Self::map`], for
    /// [`jackdaw_terrain::Heightmap::raycast_within`]. Scanned on first ask and
    /// kept for as long as this cache generation lives.
    pub fn bounds(&self) -> (f32, f32) {
        *self.bounds.get_or_init(|| self.map.height_bounds())
    }

    /// Whether the range has been scanned yet.
    #[cfg(test)]
    fn bounds_scanned(&self) -> bool {
        self.bounds.get().is_some()
    }
}

/// A bounds cell already holding `value`.
fn scanned(value: (f32, f32)) -> Arc<OnceLock<(f32, f32)>> {
    let cell = OnceLock::new();
    let _ = cell.set(value);
    Arc::new(cell)
}

struct CachedHeightmap {
    /// Set once this terrain's document has been handed out to be written
    /// through, so what is cached may not be what is stored.
    stale: bool,
    resolution: u32,
    size: Vec2,
    origin: Vec2,
    max_height: f32,
    shared: SharedHeightmap,
}

impl CachedHeightmap {
    /// Carries `rect` of a freshly written grid into the cached map, so a
    /// stroke costs its brush footprint rather than a whole rebuild.
    ///
    /// `false` when the map cannot be written, because a reader is still
    /// holding it or it was built at another length, leaving the caller to
    /// retire it instead.
    fn patch(&mut self, heights: &[f32], rect: GridRect, resolution: u32) -> bool {
        let Some(map) = Arc::get_mut(&mut self.shared.map) else {
            return false;
        };
        if map.heights.len() != heights.len() {
            return false;
        }
        rect.copy_into(heights, &mut map.heights, resolution);
        // An already-scanned range widens over the cells that moved; an
        // unscanned one stays unscanned and pays for the whole scan the first
        // time a raycast asks.
        if let Some((low, high)) = self.shared.bounds.get().copied() {
            let (mut low, mut high) = (low, high);
            for row in rect.rows(resolution) {
                for height in
                    &map.heights[row.start.min(map.heights.len())..row.end.min(map.heights.len())]
                {
                    if height.is_finite() {
                        low = low.min(*height);
                        high = high.max(*height);
                    }
                }
            }
            self.shared.bounds = scanned((low, high));
        }
        true
    }

    /// Whether this still describes the terrain it was built for.
    ///
    /// The shape is compared as well as the stale mark. Cell size and max
    /// height live on the component, which the store never sees change; the
    /// resolution and origin come from the stored regions, so allocating one by
    /// sculpting past the edge retires the map too, or the added ground would
    /// never be gathered into it.
    fn matches(&self, terrain: &jackdaw_scene_types::Terrain, shape: GridShape) -> bool {
        !self.stale
            && self.resolution == shape.resolution
            && self.size == shape.size
            && self.origin == shape.origin
            && self.max_height == terrain.max_height
    }
}

impl TerrainDataStore {
    /// Data for a sidecar path, if this store has any.
    pub fn get(&self, data_path: &str) -> Option<&RegionTerrainData> {
        self.entries.get(data_path)
    }

    /// The geometry a document's cells are drawn at, or `None` when this
    /// store has no such document or it came from a file too old to say.
    pub fn grid(&self, data_path: &str) -> Option<jackdaw_terrain::sidecar::GridGeometry> {
        self.entries.get(data_path).and_then(|data| data.grid)
    }

    /// Vertices per edge of the dense grid a terrain presents.
    ///
    /// However far this document's stored regions reach, squared off to the
    /// longer axis so the dense machinery downstream keeps to a single number.
    /// This *is* the terrain's extent, since nothing declares one, so a
    /// document holding no regions has no grid and reads as a terrain with no
    /// ground until an edit allocates the first region.
    pub fn grid_resolution(&self, data_path: &str) -> u32 {
        self.entries
            .get(data_path)
            .and_then(|data| data.regions.stored_extent())
            .map_or(0, |(x, z)| x.max(z))
    }

    /// The dense grid a terrain presents: its stored extent, placed and scaled
    /// by the geometry its cells are drawn at.
    ///
    /// The one place the two halves are put together; extent comes from the
    /// regions and placement from the sidecar.
    pub fn grid_shape(&self, terrain: &jackdaw_scene_types::Terrain) -> GridShape {
        match self.entries.get(&terrain.data_path) {
            Some(data) => Self::shape_of(data, terrain),
            None => GridShape {
                resolution: 0,
                size: Vec2::ZERO,
                origin: Vec2::ZERO,
            },
        }
    }

    /// [`Self::grid_shape`] for a document already in hand.
    fn shape_of(data: &RegionTerrainData, terrain: &jackdaw_scene_types::Terrain) -> GridShape {
        data.grid_shape(terrain.size, terrain.resolution)
    }

    /// Sets how far apart a document's cells sit, keeping where they sit: a
    /// lateral rescale of the same cells over more or less ground.
    pub fn set_cell_size(&mut self, data_path: &str, cell_size: f32) {
        let anchor = self
            .grid(data_path)
            .map(|grid| grid.anchor)
            .unwrap_or_default();
        self.set_grid(
            data_path,
            jackdaw_terrain::sidecar::GridGeometry { cell_size, anchor },
        );
        self.retire_heightmap(data_path);
    }

    /// Records the geometry a document's cells are drawn at.
    ///
    /// The load path writes down what a sidecar without stored geometry had its
    /// cells placed by, so every later reader takes it from the document rather
    /// than re-deriving it.
    pub fn set_grid(&mut self, data_path: &str, grid: jackdaw_terrain::sidecar::GridGeometry) {
        if let Some(data) = self.entries.get_mut(data_path) {
            data.grid = Some(grid);
        }
    }

    /// Heights for a sidecar path, or an empty slice when absent.
    ///
    /// A terrain with no data reads as flat, which is what a missing sidecar
    /// looks like.
    ///
    /// Borrowed whenever the grid is one whole region, which is every
    /// power-of-two resolution. A grid that runs past its region, such as the
    /// `129` shape, is gathered from the regions holding it, and the caller
    /// cannot tell the two apart.
    pub fn heights(&self, data_path: &str) -> Cow<'_, [f32]> {
        match self.entries.get(data_path) {
            Some(data) => match data.contiguous_grid() {
                Some(region) => Cow::Borrowed(region.heights()),
                None => Cow::Owned(data.grid_heights()),
            },
            None => Cow::Borrowed(&[]),
        }
    }

    /// A terrain's heights as a heightmap to sample and raycast against.
    ///
    /// Heights come from this store, never from the component, whose `heights`
    /// field is empty except on a scene old enough to still be migrated. A
    /// terrain missing from the store reads as flat.
    ///
    /// Rebuilt only when the document was written or the terrain's dimensions
    /// moved. The per-frame brush work, the target raycast and the ring
    /// following the surface, reads this rather than copying a large terrain's
    /// heights out and rescanning them for the raycast's bounds.
    pub fn heightmap(&self, terrain: &jackdaw_scene_types::Terrain) -> SharedHeightmap {
        let shape = self.grid_shape(terrain);
        let mut cache = self.cached();
        if let Some(cached) = cache.get(&terrain.data_path)
            && cached.matches(terrain, shape)
        {
            return cached.shared.clone();
        }

        // The retired map's allocation is reused when nothing else holds it,
        // which is the common case: readers take a clone for a frame.
        let mut retired = cache
            .remove(&terrain.data_path)
            .map(|cached| cached.shared.map);
        let cells = (shape.resolution as usize) * (shape.resolution as usize);
        let reusable = retired
            .as_mut()
            .and_then(Arc::get_mut)
            .is_some_and(|map| map.heights.len() == cells);

        // Row by row across whatever regions the grid spans, with the seam
        // clamped: a vertex past the last region repeats its neighbour rather
        // than dropping the terrain's border to zero.
        let gather = |heights: &mut Vec<f32>| match self.entries.get(&terrain.data_path) {
            Some(data) => data
                .regions
                .sample_grid_heights_into(shape.resolution, heights),
            None => {
                heights.clear();
                heights.resize(cells, 0.0);
            }
        };
        let map = if reusable {
            let mut map = retired.expect("reusable means there is a map to reuse");
            {
                let inner = Arc::get_mut(&mut map).expect("reusable means uniquely held");
                inner.resolution = shape.resolution;
                inner.size = shape.size;
                inner.origin = shape.origin;
                inner.max_height = terrain.max_height;
                gather(&mut inner.heights);
            }
            map
        } else {
            let mut heights = Vec::new();
            gather(&mut heights);
            Arc::new(jackdaw_terrain::Heightmap {
                resolution: shape.resolution,
                size: shape.size,
                origin: shape.origin,
                max_height: terrain.max_height,
                heights,
            })
        };

        let shared = SharedHeightmap {
            map,
            bounds: Arc::new(OnceLock::new()),
        };
        cache.insert(
            terrain.data_path.clone(),
            CachedHeightmap {
                stale: false,
                resolution: shape.resolution,
                size: shape.size,
                origin: shape.origin,
                max_height: terrain.max_height,
                shared: shared.clone(),
            },
        );
        shared
    }

    /// Brushes one frame of a stroke into a terrain's heights.
    ///
    /// `edit` is handed the dense height grid to write where it lives, so a
    /// frame of a drag neither copies the heights out nor copies them back, and
    /// the shared heightmap is patched over `rect` instead of being retired.
    ///
    /// `rect` must cover every cell `edit` writes ([`GridRect::brush`] gives
    /// the brush's own bounds); cells outside it keep whatever the cached map
    /// already held.
    ///
    /// A cached height range is only widened here, never narrowed, so a stroke
    /// that flattens the terrain's one peak leaves the range taller than the
    /// terrain. That costs the brush-target raycast marching steps, since it
    /// marches the slab the range spans, but only a range thousands of cells
    /// tall would lose a hit. The next full rebuild tightens it again.
    ///
    /// `false` when the terrain has no document to write: the cases
    /// [`Self::entry_for`] refuses, and a terrain with no editable window.
    pub fn brush_heights(
        &mut self,
        terrain: &jackdaw_scene_types::Terrain,
        rect: GridRect,
        edit: impl FnOnce(&mut [f32]),
    ) -> bool {
        let Self {
            entries,
            load_failed,
            heightmaps,
            ..
        } = self;
        let Some(data) = Self::document_in(entries, load_failed, terrain) else {
            return false;
        };
        // A grid that is one whole region is brushed where it lives. One that
        // spans regions has no single slice to hand out, so it is gathered,
        // brushed and scattered back, charging the copy to the terrains whose
        // resolution runs past a region rather than to every stroke.
        let gathered = match data.contiguous_grid_mut() {
            Some(region) => {
                edit(region.heights_mut());
                None
            }
            None => {
                let mut heights = data.grid_heights();
                edit(&mut heights);
                data.set_grid_heights(&heights);
                Some(heights)
            }
        };
        let heights = match &gathered {
            Some(heights) => heights.as_slice(),
            None => data
                .contiguous_grid()
                .map(jackdaw_terrain::Region::heights)
                .unwrap_or(&[]),
        };

        // The gathered heights are a `channel_resolution` grid, which need not
        // be the grid the cached map was built at: the map covers however far
        // the regions reach. When the two disagree the patch refuses and the
        // map is retired, so the next reader rebuilds it over the wider ground.
        let shape = Self::shape_of(data, terrain);
        let stride = data.grid_resolution();
        let cache = heightmaps.get_mut().unwrap_or_else(PoisonError::into_inner);
        if let Some(cached) = cache.get_mut(&terrain.data_path)
            && !(cached.matches(terrain, shape) && cached.patch(heights, rect, stride))
        {
            cached.stale = true;
        }
        true
    }

    /// Whether a terrain's cached heightmap still stands, rather than
    /// having been retired and needing a full rebuild.
    #[cfg(test)]
    fn heightmap_is_current(&self, terrain: &jackdaw_scene_types::Terrain) -> bool {
        self.cached()
            .get(&terrain.data_path)
            .is_some_and(|cached| cached.matches(terrain, self.grid_shape(terrain)))
    }

    /// Drops cached heightmaps for every path `live` does not name.
    ///
    /// The documents stay, because an undo can bring a deleted terrain back and
    /// its heights have to still be there. The cached map is derived data,
    /// costs `resolution^2 * 4` bytes (a megabyte for a 512-resolution
    /// terrain), and nothing else frees it.
    pub fn retain_heightmaps(&self, live: impl Fn(&str) -> bool) {
        self.cached().retain(|path, _| live(path));
    }

    /// How many terrains this store is holding a sampling view for.
    pub fn cached_heightmap_count(&self) -> usize {
        self.cached().len()
    }

    /// A terrain's document for reading.
    ///
    /// Shaped and reconciled the way [`Self::entry_for`] shapes it, but the
    /// cached heightmap is left alone: nothing reachable from here can write a
    /// height.
    pub fn read_for(&mut self, terrain: &jackdaw_scene_types::Terrain) -> Option<TerrainRead<'_>> {
        Self::document_in(&mut self.entries, &self.load_failed, terrain)
            .map(|data| TerrainRead::new(data))
    }

    /// Marks one terrain's cached heightmap as not describing what is stored.
    /// Called wherever that terrain's bulk data may have been written.
    ///
    /// Per path, not global: sculpting one terrain says nothing about any other
    /// terrain's heights, and retiring theirs would charge every open terrain a
    /// full rebuild for a stroke on one.
    fn retire_heightmap(&self, data_path: &str) {
        if let Some(cached) = self.cached().get_mut(data_path) {
            cached.stale = true;
        }
    }

    /// The heightmap cache, taking a poisoned lock back rather than propagating
    /// it: this is derived data with no invariant a panic elsewhere can leave
    /// half-written.
    fn cached(&self) -> MutexGuard<'_, HashMap<String, CachedHeightmap>> {
        self.heightmaps
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Installs data for a sidecar path, replacing anything already there and
    /// clearing any load-failed mark, which a successfully decoded read for the
    /// same path supersedes.
    ///
    /// The control map is marked for upload: a document that arrives with paint
    /// on it has to reach the GPU without the user touching a brush first.
    pub fn insert(&mut self, data_path: impl Into<String>, data: RegionTerrainData) {
        let data_path = data_path.into();
        self.retire_heightmap(&data_path);
        self.load_failed.remove(&data_path);
        self.mark_control_dirty(&data_path, GridRect::whole(data.grid_resolution()));
        self.entries.insert(data_path, data);
    }

    /// Marks a sidecar path as failed to load: a file exists but its contents
    /// could not be decoded. The `load_failed` field's docs cover why this is
    /// kept separate from having no entry.
    pub fn mark_load_failed(&mut self, data_path: impl Into<String>, reason: impl Into<String>) {
        self.load_failed.insert(data_path.into(), reason.into());
    }

    /// Whether a sidecar path's last read attempt failed to decode.
    pub fn is_load_failed(&self, data_path: &str) -> bool {
        self.load_failed.contains_key(data_path)
    }

    /// Why a sidecar path is quarantined, if it is. `None` for a path that has
    /// never failed to load, including one never loaded at all.
    pub fn load_failed_reason(&self, data_path: &str) -> Option<&str> {
        self.load_failed.get(data_path).map(String::as_str)
    }

    /// Drops a sidecar path's data.
    pub fn remove(&mut self, data_path: &str) -> Option<RegionTerrainData> {
        self.cached().remove(data_path);
        self.entries.remove(data_path)
    }

    /// Whether a sidecar path has data.
    pub fn contains(&self, data_path: &str) -> bool {
        self.entries.contains_key(data_path)
    }

    /// The document for a terrain, shaped so its dense editing window is the
    /// region the editor's tools address, and reconciled against the terrain's
    /// declared channels.
    ///
    /// Every mutating path goes through here, so a channel or height array can
    /// never be a different length than the terrain claims, and a newly added
    /// channel is zeroed at the right length before anything paints it.
    ///
    /// See [`Self::entry_for`] for the cases this refuses. Every refusal
    /// happens before anything is written, leaving the document as it loaded.
    ///
    /// Takes its fields rather than `&mut self` so a caller can hold the
    /// returned borrow and still touch `control_dirty`.
    fn document_in<'a>(
        entries: &'a mut HashMap<String, RegionTerrainData>,
        load_failed: &HashMap<String, String>,
        terrain: &jackdaw_scene_types::Terrain,
    ) -> Option<&'a mut RegionTerrainData> {
        if terrain.data_path.is_empty() {
            return None;
        }
        if load_failed.contains_key(&terrain.data_path) {
            warn!(
                "Refusing to edit terrain data {:?}: it failed to load; fix or replace \
                 the sidecar file and reload the scene",
                terrain.data_path
            );
            return None;
        }

        // Nothing here resizes the document. A terrain's extent is the regions
        // it holds, so a document arrives at whatever it was stored at and
        // grows only where an edit allocates a region.
        let data = entries.entry(terrain.data_path.clone()).or_default();

        reconcile_channels(data, terrain);
        Some(data)
    }

    /// The dense editing view of a terrain's document.
    ///
    /// `None`, refusing the edit, for a terrain with no sidecar path and for
    /// one whose data failed to load. Nothing here reshapes a document to fit a
    /// declared grid: a terrain's extent is the regions it holds. A refusal
    /// leaves the document, and the file behind it, as it was; minting or
    /// trimming data here would lose data silently.
    pub fn entry_for(
        &mut self,
        terrain: &jackdaw_scene_types::Terrain,
    ) -> Option<TerrainEntry<'_>> {
        // An entry is handed out to be written through, so this terrain's
        // cached heightmap is stale from here whether the caller writes or not.
        self.retire_heightmap(&terrain.data_path);
        Self::document_in(&mut self.entries, &self.load_failed, terrain).map(TerrainEntry::new)
    }

    /// Every (sidecar path, document) pair currently held.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &RegionTerrainData)> {
        self.entries.iter()
    }

    /// The materials a terrain paints with, in texture-id order. Empty for
    /// a terrain that has none, which is what a fresh one looks like.
    pub fn materials(&self, data_path: &str) -> &[TerrainMaterialSlot] {
        self.entries
            .get(data_path)
            .map(|data| data.materials.as_slice())
            .unwrap_or(&[])
    }

    /// Replaces a terrain's whole material list.
    ///
    /// Whole-list rather than per-slot: order *is* the id space, so every
    /// mutation (add, remove, reorder, retile) is a list rewrite and an undo
    /// entry only has to hold the two lists.
    ///
    /// A name the sidecar encoder would reject is refused rather than stored,
    /// since it would otherwise sit in the store and fail every later save.
    /// Whether a name is *saved* is checked by the caller, which is what can
    /// see the material registry.
    ///
    /// Nothing here rewrites paint or renumbers an id: a removed material
    /// leaves a tombstone ([`TerrainMaterialSlot::tombstone`]) rather than
    /// closing the gap, so every cell in the control map keeps drawing what it
    /// drew. A reorder is the exception, being a renumbering asked for by name.
    ///
    /// Trailing tombstones are dropped: an id past the end of the list is no
    /// different from an id held open at the end of it, and keeping them would
    /// let the list grow against [`MAX_TEXTURES`] on nothing.
    pub fn set_materials(
        &mut self,
        data_path: impl Into<String>,
        materials: Vec<TerrainMaterialSlot>,
    ) -> Result<(), TerrainMaterialError> {
        let data_path = data_path.into();
        let mut materials = materials;
        while materials
            .last()
            .is_some_and(TerrainMaterialSlot::is_tombstone)
        {
            materials.pop();
        }
        if materials.len() > MAX_TEXTURES {
            return Err(TerrainMaterialError::Full);
        }
        for (index, slot) in materials.iter().enumerate() {
            sidecar::validate_material_name(&slot.material).map_err(|reason| {
                TerrainMaterialError::InvalidName {
                    name: slot.material.clone(),
                    reason,
                }
            })?;
            // Tombstones all answer to the empty name, so only real
            // materials can collide.
            if slot.is_tombstone() {
                continue;
            }
            if materials[..index]
                .iter()
                .any(|earlier| earlier.material == slot.material)
            {
                return Err(TerrainMaterialError::AlreadyUsed(slot.material.clone()));
            }
        }
        if self.load_failed.contains_key(&data_path) {
            return Err(TerrainMaterialError::LoadFailed);
        }
        match self.entries.get_mut(&data_path) {
            Some(data) => data.materials = materials,
            // A list can be set before the terrain has been edited or
            // loaded, so it mints the document the next edit will shape.
            None if !materials.is_empty() => {
                self.entries.insert(
                    data_path,
                    RegionTerrainData {
                        materials,
                        ..default()
                    },
                );
            }
            None => {}
        }
        Ok(())
    }

    /// How a terrain textures the cells no hand has claimed. A terrain with no
    /// document reads as the default, off.
    pub fn autoterrain(&self, data_path: &str) -> AutoTerrainSettings {
        self.entries
            .get(data_path)
            .map(|data| data.autoterrain)
            .unwrap_or_default()
    }

    /// Replaces a terrain's autoterrain settings.
    ///
    /// Refused for a quarantined path, like every other write. A path with no
    /// document mints one only for non-default settings, so reading a terrain's
    /// settings and writing them straight back never creates a document.
    pub fn set_autoterrain(
        &mut self,
        data_path: impl Into<String>,
        settings: AutoTerrainSettings,
    ) -> Result<(), TerrainMaterialError> {
        let data_path = data_path.into();
        if self.load_failed.contains_key(&data_path) {
            return Err(TerrainMaterialError::LoadFailed);
        }
        let settings = settings.sanitized();
        match self.entries.get_mut(&data_path) {
            Some(data) => data.autoterrain = settings,
            None if settings != AutoTerrainSettings::default() => {
                self.entries.insert(
                    data_path,
                    RegionTerrainData {
                        autoterrain: settings,
                        ..default()
                    },
                );
            }
            None => {}
        }
        Ok(())
    }

    /// Control words for a sidecar path, or an empty slice when absent.
    ///
    /// Empty is not an error: an unpainted terrain has no window, and every
    /// cell of it reads as the default word.
    pub fn control(&self, data_path: &str) -> Cow<'_, [Control]> {
        match self.entries.get(data_path) {
            Some(data) => match data.contiguous_grid() {
                Some(region) => Cow::Borrowed(region.control_words()),
                None => Cow::Owned(data.grid_control()),
            },
            None => Cow::Borrowed(&[]),
        }
    }

    /// One rect of a terrain's control words, row-major over the rect.
    ///
    /// What the renderer patches an uploaded control map from after a stroke.
    /// Reading the rect rather than [`Self::control`]'s whole grid is the
    /// difference between a hundred cells and a million on the default terrain.
    /// Cells outside the terrain's own grid read as the default word.
    pub fn control_rect(&self, data_path: &str, resolution: u32, rect: GridRect) -> Vec<Control> {
        let Some(data) = self.entries.get(data_path) else {
            return vec![Control::default(); rect.cells()];
        };
        let mut out = Vec::with_capacity(rect.cells());
        for gz in rect.z..rect.z + rect.height {
            for gx in rect.x..rect.x + rect.width {
                out.push(if gx < resolution && gz < resolution {
                    data.regions.grid_control(gx, gz)
                } else {
                    Control::default()
                });
            }
        }
        out
    }

    /// Control words for a terrain, sized to its resolution. `None` wherever
    /// [`Self::entry_for`] is `None`, and for the same reasons: paint is stored
    /// in the same document as everything else.
    ///
    /// Handing out `&mut` marks the map dirty whether or not the caller writes,
    /// the alternative being a per-cell setter a brush would call thousands of
    /// times a stroke. A spurious re-upload costs one texture write; a missed
    /// one leaves the terrain rendering paint that is not in the document.
    pub fn control_mut(
        &mut self,
        terrain: &jackdaw_scene_types::Terrain,
    ) -> Option<ControlGrid<'_>> {
        let resolution = self.grid_shape(terrain).resolution;
        self.control_rect_mut(terrain, GridRect::whole(resolution))
    }

    /// [`Self::control_mut`] for a caller that knows which block it is
    /// about to write.
    ///
    /// Only `rect` is marked for upload, so a paint stroke costs the renderer
    /// its brush footprint per frame rather than the whole map. A write outside
    /// `rect` leaves that part of the uploaded texture unchanged, so the rect
    /// has to cover every cell the caller touches.
    pub fn control_rect_mut(
        &mut self,
        terrain: &jackdaw_scene_types::Terrain,
        rect: GridRect,
    ) -> Option<ControlGrid<'_>> {
        // The cached heightmap stands. A resolution change is caught by
        // `CachedHeightmap::matches`, which compares the dimensions it was
        // built for. At an unchanged resolution the only heights `document_in`
        // can move are the ones it mints, a freshly ensured region or a window
        // carried across by prefix copy, and both leave zeroes where
        // `heightmap` already pads with them.
        let Self {
            entries,
            load_failed,
            control_dirty,
            ..
        } = self;
        let document = Self::document_in(entries, load_failed, terrain)?;
        let dirty = control_dirty
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        dirty
            .entry(terrain.data_path.clone())
            .and_modify(|pending| *pending = pending.union(rect))
            .or_insert(rect);
        Some(ControlGrid::new(document, rect))
    }

    /// Records that `rect` of a terrain's control map needs uploading.
    fn mark_control_dirty(&self, data_path: &str, rect: GridRect) {
        let mut dirty = self
            .control_dirty
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match dirty.get_mut(data_path) {
            Some(pending) => *pending = pending.union(rect),
            None => {
                dirty.insert(data_path.to_string(), rect);
            }
        }
    }

    /// Which block of a terrain's control map has been written since this
    /// was last called, clearing the mark.
    ///
    /// The renderer calls this only once it has somewhere to upload to, so a
    /// write that lands before the material exists keeps its mark until then.
    pub fn take_control_dirty(&self, data_path: &str) -> Option<GridRect> {
        self.control_dirty
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(data_path)
    }
}

/// The region side a terrain of `resolution` vertices per edge is stored
/// at: [`RegionSize::for_resolution`], never coarser than
/// [`RegionSize::DEFAULT`].
///
/// `for_resolution` alone gives a power-of-two grid exactly one region, which
/// leaves a terrain nothing to author region by region. The cap puts a grid of
/// regions under a large terrain (a 1024 grid is 4x4 of them) and is a no-op at
/// every resolution of 256 or below, where the seam rule decides the size.
#[cfg(test)]
fn editor_region_size(resolution: u32) -> RegionSize {
    let derived = RegionSize::for_resolution(resolution);
    if derived.get() > RegionSize::DEFAULT.get() {
        RegionSize::DEFAULT
    } else {
        derived
    }
}

/// One terrain's control map as a dense grid, writable.
///
/// A grid that is one whole region is written where it lives. One that spans
/// regions is gathered here and scattered back on drop, so a caller writes a
/// dense grid either way without knowing which shape it got.
///
/// The gather and the scatter both cover `rect` only, which for a stroke is its
/// brush footprint: a hundred cells against a million on the default terrain.
/// Outside `rect` the dense view reads as the default word rather than as what
/// is stored, so `rect` has to cover every cell the caller reads as well as
/// every cell it writes.
pub struct ControlGrid<'a> {
    words: ControlWords<'a>,
}

enum ControlWords<'a> {
    Borrowed(&'a mut [Control]),
    Gathered {
        document: &'a mut RegionTerrainData,
        /// Dense `resolution^2`, but only `rect` is populated and only
        /// `rect` is written back.
        words: Vec<Control>,
        rect: GridRect,
        resolution: u32,
    },
}

impl<'a> ControlGrid<'a> {
    fn new(document: &'a mut RegionTerrainData, rect: GridRect) -> Self {
        // The row stride is the document's own grid: the words handed out have
        // to be indexable at the same resolution the caller brushes at.
        let resolution = document.grid_resolution();
        // Split in two steps because the contiguous case reborrows the document
        // for the whole guard, so the gathered case cannot also hold it in the
        // same match.
        if document.contiguous_grid().is_some() {
            let words = document
                .contiguous_grid_mut()
                .expect("just checked")
                .control_words_mut();
            return Self {
                words: ControlWords::Borrowed(words),
            };
        }
        let mut words = vec![Control::default(); (resolution as usize) * (resolution as usize)];
        for row in rect.rows(resolution) {
            for index in row {
                let x = (index % resolution as usize) as u32;
                let z = (index / resolution as usize) as u32;
                words[index] = document.regions.grid_control(x, z);
            }
        }
        Self {
            words: ControlWords::Gathered {
                document,
                words,
                rect,
                resolution,
            },
        }
    }
}

impl core::ops::Deref for ControlGrid<'_> {
    type Target = [Control];

    fn deref(&self) -> &[Control] {
        match &self.words {
            ControlWords::Borrowed(words) => words,
            ControlWords::Gathered { words, .. } => words,
        }
    }
}

impl core::ops::DerefMut for ControlGrid<'_> {
    fn deref_mut(&mut self) -> &mut [Control] {
        match &mut self.words {
            ControlWords::Borrowed(words) => words,
            ControlWords::Gathered { words, .. } => words,
        }
    }
}

impl Drop for ControlGrid<'_> {
    fn drop(&mut self) {
        if let ControlWords::Gathered {
            document,
            words,
            rect,
            resolution,
        } = &mut self.words
        {
            for row in rect.rows(*resolution) {
                for index in row {
                    let x = (index % *resolution as usize) as i32;
                    let z = (index / *resolution as usize) as i32;
                    document.regions.set_control(x, z, words[index]);
                }
            }
        }
    }
}

/// The dense `resolution^2` view of one terrain's document.
///
/// The editor's sculpt, generate, erosion, quantize, scatter and channel paths
/// all address a terrain as a single dense grid, whatever the document stores
/// it as.
///
/// Which shape that is comes from the document's own region size, not from
/// the resolution alone. A terrain of 256 vertices per edge or fewer is one
/// region, so [`RegionTerrainData::contiguous_grid`] is `Some` once the
/// origin region exists (true of every such document reached through
/// [`TerrainDataStore::entry_for`], which allocates it) and the dense grid is
/// that region's own layer, handed over without a copy. A whole-array write
/// goes through [`Self::set_heights`], which fills or truncates rather than
/// letting the region change shape.
///
/// Anything wider spans regions, and the same calls gather and scatter through
/// [`RegionTerrainData::grid_heights`] and
/// [`RegionTerrainData::set_grid_heights`] instead. Those walk the sparse
/// addressing cell by cell, so a whole-grid gather is a million lookups on the
/// default terrain: affordable for a load or a generate, not per frame. The
/// paths a stroke runs every frame take a rect instead
/// ([`TerrainDataStore::control_rect_mut`], [`TerrainDataStore::control_rect`])
/// and touch only the cells the brush covers.
///
/// A zero-resolution terrain has no grid; its heights read as empty and writes
/// to them go nowhere.
pub struct TerrainEntry<'a> {
    data: &'a mut RegionTerrainData,
}

impl<'a> TerrainEntry<'a> {
    fn new(data: &'a mut RegionTerrainData) -> Self {
        Self { data }
    }

    pub fn heights(&self) -> Cow<'_, [f32]> {
        match self.data.contiguous_grid() {
            Some(region) => Cow::Borrowed(region.heights()),
            None => Cow::Owned(self.data.grid_heights()),
        }
    }

    /// Test-only: the editor's sculpt and generate paths write whole arrays
    /// through [`Self::set_heights`] rather than single cells in place.
    #[cfg(test)]
    pub fn heights_mut(&mut self) -> &mut [f32] {
        self.data
            .contiguous_grid_mut()
            .map(jackdaw_terrain::Region::heights_mut)
            .unwrap_or(&mut [])
    }

    /// Overwrites every height, zero-filling a short source and ignoring the
    /// tail of a long one.
    ///
    /// A dense grid states its own extent: handing over `n * n` heights says
    /// the terrain has `n` cells a side, so writing one to a terrain holding no
    /// regions allocates the regions to hold it. A grid smaller than what is
    /// stored overwrites its corner and leaves the rest; nothing here removes
    /// ground.
    pub fn set_heights(&mut self, heights: &[f32]) {
        let side = (heights.len() as f64).sqrt().round() as u32;
        if side * side == heights.len() as u32 {
            // Past the cap this allocates nothing and the write lands in
            // whatever regions already exist.
            let _ = self.ensure_extent(side);
        }
        match self.data.contiguous_grid_mut() {
            Some(region) => region.copy_heights_from(heights),
            None => self.data.set_grid_heights(heights),
        }
    }

    /// Overwrites one rectangle of the height grid, row by row.
    ///
    /// `values` is the row-major run [`GridRect::read`] produces for the same
    /// rect. A stroke undo restores through this rather than writing back a
    /// whole terrain to put a few hundred brushed cells back.
    pub fn set_heights_rect(&mut self, rect: GridRect, values: &[f32]) {
        let stride = self.data.grid_resolution();
        match self.data.contiguous_grid_mut() {
            Some(region) => rect.write(region.heights_mut(), stride, values),
            None => {
                let mut heights = self.data.grid_heights();
                rect.write(&mut heights, stride, values);
                self.data.set_grid_heights(&heights);
            }
        }
    }

    pub fn channels(&self) -> &[jackdaw_terrain::ChannelDescriptor] {
        &self.data.channels
    }

    pub fn channels_mut(&mut self) -> &mut Vec<jackdaw_terrain::ChannelDescriptor> {
        &mut self.data.channels
    }

    pub fn channel_mut(&mut self, index: usize) -> Option<&mut jackdaw_terrain::ChannelDescriptor> {
        self.data.channels.get_mut(index)
    }

    /// Puts a whole channel's values back across the regions holding them, as
    /// undoing the removal of that channel does.
    pub fn restore_channel(&mut self, index: usize, values: &[u16]) {
        let resolution = self.data.grid_resolution();
        self.data
            .regions
            .write_grid_channel(index, resolution, values);
    }

    /// Brings `cells` per axis into being, allocating every region holding one
    /// of them.
    ///
    /// Nothing allocates implicitly: a terrain is the regions it holds, so a
    /// first edit on a terrain with no regions goes through here.
    ///
    /// Refused as a whole when it would take the terrain past the region cap,
    /// leaving the document as it was.
    pub fn ensure_extent(
        &mut self,
        cells: u32,
    ) -> Result<(), jackdaw_terrain::region::RegionCapError> {
        if cells == 0 {
            return Ok(());
        }
        self.data.regions.ensure_grid(cells)
    }

    /// One channel's values as a dense grid, gathered from the regions.
    ///
    /// A stroke brushes this and hands it back through
    /// [`Self::set_channel_values`], the same gather-brush-scatter shape the
    /// heights use.
    pub fn channel_values(&self, index: usize) -> Vec<u16> {
        self.data
            .regions
            .read_grid_channel(index, self.data.grid_resolution())
    }

    /// Scatters a brushed channel grid back across the regions.
    pub fn set_channel_values(&mut self, index: usize, values: &[u16]) {
        self.restore_channel(index, values);
    }

    /// The whole document, for the paths that persist or copy it.
    pub fn document(&self) -> &RegionTerrainData {
        self.data
    }
}

/// One terrain's document, for a caller that only reads it.
///
/// The read half of [`TerrainEntry`]. The scatter operator wants a terrain's
/// channels reconciled against its component, which only
/// `TerrainDataStore::document_in` does, but it writes no heights and so must
/// not cost the shared heightmap a rebuild.
pub struct TerrainRead<'a> {
    data: &'a RegionTerrainData,
}

impl<'a> TerrainRead<'a> {
    fn new(data: &'a RegionTerrainData) -> Self {
        Self { data }
    }

    pub fn heights(&self) -> Cow<'_, [f32]> {
        match self.data.contiguous_grid() {
            Some(region) => Cow::Borrowed(region.heights()),
            None => Cow::Owned(self.data.grid_heights()),
        }
    }

    pub fn channels(&self) -> &[jackdaw_terrain::ChannelDescriptor] {
        &self.data.channels
    }

    /// The whole document, for readers that want more than one layer.
    pub fn document(&self) -> &RegionTerrainData {
        self.data
    }
}

/// Brings a terrain's stored channel values in line with the channel
/// descriptors on its component.
///
/// The component owns the channel table (names, widths, order) and the sidecar
/// owns the values. When the user adds, removes, renames or reorders a channel
/// in the inspector, this carries the values along, matched by name, so a
/// rename keeps what was painted and a reorder moves it rather than shuffling
/// it into the wrong layer.
///
/// A channel added to a terrain that already has heights is initialised to zero
/// at `resolution^2` here.
fn reconcile_channels(data: &mut RegionTerrainData, terrain: &jackdaw_scene_types::Terrain) {
    use jackdaw_terrain::ChannelElement;

    let wanted = |element: jackdaw_scene_types::TerrainChannelElement| match element {
        jackdaw_scene_types::TerrainChannelElement::U8 => ChannelElement::U8,
        jackdaw_scene_types::TerrainChannelElement::U16 => ChannelElement::U16,
    };

    if data.channels.len() == terrain.channels.len()
        && data
            .channels
            .iter()
            .zip(&terrain.channels)
            .all(|(have, want)| have.name == want.name && have.element == wanted(want.element))
    {
        return;
    }

    // Matching on name keeps a channel's painted values across a rename-free
    // reorder; a newly declared channel starts zeroed.
    let sources: Vec<Option<usize>> = terrain
        .channels
        .iter()
        .map(|descriptor| {
            data.channels
                .iter()
                .position(|had| had.name == descriptor.name)
        })
        .collect();

    data.channels = terrain
        .channels
        .iter()
        .map(|descriptor| {
            jackdaw_terrain::ChannelDescriptor::new(
                descriptor.name.clone(),
                wanted(descriptor.element),
            )
        })
        .collect();
    data.regions.remap_channels(&sources);
}

/// Mints a sidecar path for a new terrain in `scene_stem`'s scene, unique
/// against everything the store already holds.
///
/// Uniqueness is checked against the whole store, not just the terrains spawned
/// in the scene, because the store can hold an entry for a terrain the scene
/// does not have live: one an undo brought back, or a delete not yet redone
/// past. Checking only live terrains could mint a path that collides with one
/// of those once it resurfaces. Collisions across tabs are not covered: each
/// inactive [`crate::scenes::SceneTab`] owns a separate [`TerrainDataStore`],
/// so this sees one tab's store at a time.
pub fn mint_data_path(store: &TerrainDataStore, scene_stem: &str) -> String {
    let stem = if scene_stem.is_empty() {
        "untitled"
    } else {
        scene_stem
    };
    for n in 0.. {
        let candidate = format!("{stem}.terrain-{n}.{}", sidecar::EXTENSION);
        if !store.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!("the candidate space is unbounded")
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn terrain(resolution: u32, data_path: &str) -> jackdaw_scene_types::Terrain {
        jackdaw_scene_types::Terrain {
            resolution,
            data_path: data_path.to_string(),
            ..default()
        }
    }

    /// A store already holding a document for `terrain`, stored in regions
    /// small enough that the terrain is a handful of cells.
    ///
    /// Region size is fixed when a document is created and a real one uses
    /// [`RegionSize::DEFAULT`], which would make every fixture here a quarter
    /// of a million cells.
    fn store_for(terrain: &jackdaw_scene_types::Terrain) -> TerrainDataStore {
        let mut regions =
            jackdaw_terrain::TerrainRegions::new(editor_region_size(terrain.resolution));
        // Nothing allocates implicitly, so a terrain these tests write to has
        // to have been given its cells.
        regions
            .ensure_grid(terrain.resolution)
            .expect("inside the region cap");
        let mut store = TerrainDataStore::default();
        store.insert(
            terrain.data_path.clone(),
            RegionTerrainData {
                regions,
                ..default()
            },
        );
        // Arriving in the store marks everything for upload; a test that
        // watches what an edit dirties starts from a settled store.
        store.take_control_dirty(&terrain.data_path);
        store
    }

    /// Two readers in one frame must not each pay for a copy of a large
    /// terrain's heights.
    #[test]
    fn two_reads_of_unchanged_data_share_one_heightmap() {
        let subject = terrain(4, "a.jdterrain");
        let mut store = store_for(&subject);
        store
            .entry_for(&subject)
            .expect("keyed")
            .set_heights(&[2.0; 16]);

        let first = store.heightmap(&subject);
        let second = store.heightmap(&subject);
        assert!(std::sync::Arc::ptr_eq(&first.map, &second.map));
    }

    /// The brush raycasts against the heights a stroke wrote, not the ones it
    /// replaced.
    #[test]
    fn a_write_retires_the_shared_heightmap() {
        let subject = terrain(4, "a.jdterrain");
        let mut store = store_for(&subject);
        assert_eq!(store.heightmap(&subject).map.heights, vec![0.0; 16]);

        store
            .entry_for(&subject)
            .expect("keyed")
            .set_heights(&[5.0; 16]);

        let fresh = store.heightmap(&subject);
        assert_eq!(fresh.map.heights, vec![5.0; 16]);
        assert_eq!(fresh.bounds(), (0.0, 5.0));
    }

    /// A frame of a drag must patch the shared heightmap where the brush
    /// wrote, not retire it: rebuilding is a whole-terrain copy, on every
    /// frame the button is held.
    #[test]
    fn brushing_patches_the_shared_heightmap_instead_of_retiring_it() {
        let subject = terrain(4, "a.jdterrain");
        let mut store = store_for(&subject);
        store.entry_for(&subject).expect("keyed");
        let _ = store.heightmap(&subject);

        let rect = GridRect {
            x: 1,
            z: 1,
            width: 2,
            height: 2,
        };
        assert!(store.brush_heights(&subject, rect, |heights| {
            heights[5] = 9.0;
        }));

        assert!(
            store.heightmap_is_current(&subject),
            "the cached map was retired rather than patched",
        );
        assert_eq!(store.heightmap(&subject).map.heights[5], 9.0);
        assert_eq!(store.heights("a.jdterrain")[5], 9.0);
    }

    /// A reader that kept its clone past the frame it needed it makes the
    /// map unpatchable, so the store falls back to a rebuild rather than
    /// letting the two disagree.
    #[test]
    fn a_held_heightmap_forces_a_rebuild_rather_than_a_stale_patch() {
        let subject = terrain(4, "a.jdterrain");
        let mut store = store_for(&subject);
        store.entry_for(&subject).expect("keyed");
        let held = store.heightmap(&subject);

        let rect = GridRect::whole(4);
        store.brush_heights(&subject, rect, |heights| heights[5] = 9.0);

        assert!(!store.heightmap_is_current(&subject));
        assert_eq!(held.map.heights[5], 0.0, "the held clone was written under");
        assert_eq!(store.heightmap(&subject).map.heights[5], 9.0);
    }

    /// Only the brush-target raycast wants the height range, and a drag
    /// writes far more often than a raycast asks, so the scan waits until
    /// something asks for it.
    #[test]
    fn a_height_range_is_scanned_on_first_ask_and_widened_by_a_stroke() {
        let subject = terrain(4, "a.jdterrain");
        let mut store = store_for(&subject);
        store.entry_for(&subject).expect("keyed");
        assert!(!store.heightmap(&subject).bounds_scanned());

        assert_eq!(store.heightmap(&subject).bounds(), (0.0, 0.0));
        assert!(store.heightmap(&subject).bounds_scanned());

        let rect = GridRect {
            x: 1,
            z: 1,
            width: 2,
            height: 2,
        };
        store.brush_heights(&subject, rect, |heights| heights[5] = 9.0);
        assert_eq!(store.heightmap(&subject).bounds(), (0.0, 9.0));
    }

    /// Nothing else ever frees a cached heightmap, so a tab that visited
    /// a hundred terrains would hold a hundred of them.
    #[test]
    fn a_cached_heightmap_is_freed_once_its_terrain_is_gone() {
        let kept = terrain(4, "a.jdterrain");
        let mut store = store_for(&kept);
        let gone = terrain(4, "b.jdterrain");
        store.entry_for(&kept).expect("keyed");
        store.entry_for(&gone).expect("keyed");
        let _ = store.heightmap(&kept);
        let _ = store.heightmap(&gone);
        assert_eq!(store.cached_heightmap_count(), 2);

        store.retain_heightmaps(|path| path == "a.jdterrain");
        assert_eq!(store.cached_heightmap_count(), 1);
        assert!(
            store.contains("b.jdterrain"),
            "the document stays; an undo can bring the terrain back",
        );
    }

    /// Scattering samples a terrain and writes none of it, so it must
    /// leave the shared heightmap where it found it.
    #[test]
    fn reading_a_document_leaves_the_shared_heightmap_alone() {
        let subject = terrain(4, "a.jdterrain");
        let mut store = store_for(&subject);
        store
            .entry_for(&subject)
            .expect("keyed")
            .set_heights(&[3.0; 16]);
        let held = store.heightmap(&subject);

        let read = store.read_for(&subject).expect("keyed");
        assert_eq!(*read.heights(), [3.0; 16]);
        assert!(read.channels().is_empty());

        assert!(std::sync::Arc::ptr_eq(
            &held.map,
            &store.heightmap(&subject).map
        ));
    }

    /// Reading the shared heightmap must not read as a write. Systems gated on
    /// `Res<TerrainDataStore>::is_changed`, such as the Textures tab's tiling
    /// fields, would otherwise run on every frame a brush hovered a terrain,
    /// inserting slider values and flushing commands for an untouched store.
    #[test]
    fn reading_a_heightmap_does_not_flag_the_store_written() {
        #[derive(Resource, Default)]
        struct Flagged(bool);

        let mut app = App::new();
        app.init_resource::<TerrainDataStore>();
        app.init_resource::<Flagged>();
        app.add_systems(
            Update,
            |store: Res<TerrainDataStore>, mut flagged: ResMut<Flagged>| {
                let _ = store.heightmap(&terrain(4, "a.jdterrain"));
                flagged.0 = store.is_changed();
            },
        );

        app.update();
        app.update();
        assert!(
            !app.world().resource::<Flagged>().0,
            "a hover frame flagged the store written",
        );

        app.world_mut()
            .resource_mut::<TerrainDataStore>()
            .entry_for(&terrain(4, "a.jdterrain"))
            .expect("keyed")
            .set_heights(&[1.0; 16]);
        app.update();
        assert!(
            app.world().resource::<Flagged>().0,
            "a real write must still flag the store",
        );
    }

    /// Writing one terrain says nothing about what another terrain's
    /// heights are, so a stroke on one must not charge every other open
    /// terrain a full rebuild.
    #[test]
    fn writing_one_terrain_keeps_another_terrains_shared_heightmap() {
        let first = terrain(4, "a.jdterrain");
        let mut store = store_for(&first);
        let second = terrain(4, "b.jdterrain");
        let held = store.heightmap(&second);

        store
            .entry_for(&first)
            .expect("keyed")
            .set_heights(&[7.0; 16]);

        assert!(
            std::sync::Arc::ptr_eq(&held.map, &store.heightmap(&second).map),
            "b's cached heightmap was retired by a write to a",
        );
        assert_eq!(store.heightmap(&first).map.heights, vec![7.0; 16]);
    }

    /// Cell size lives on the component, which this store never sees
    /// change, so a cached map has to notice for itself that it was built
    /// at another scale.
    #[test]
    fn a_changed_cell_size_retires_the_shared_heightmap() {
        let subject = terrain(4, "a.jdterrain");
        let mut store = store_for(&subject);
        store
            .entry_for(&subject)
            .expect("keyed")
            .set_heights(&[0.0; 16]);

        let one_metre = store.heightmap(&subject).map.size;

        // The document's geometry is the authority, so a cell size change
        // is committed there; the component carries the same number as
        // the authoring surface.
        store.set_grid(
            &subject.data_path,
            jackdaw_terrain::sidecar::GridGeometry {
                cell_size: 4.0,
                anchor: Vec2::ZERO,
            },
        );
        let coarse = jackdaw_scene_types::Terrain {
            cell_size: 4.0,
            ..subject.clone()
        };
        assert_eq!(store.heightmap(&coarse).map.size, one_metre * 4.0);
        // Same cells, further apart: a lateral rescale, no resampling.
        assert_eq!(store.heightmap(&coarse).map.heights.len(), 16);
    }

    /// Extent is the stored regions, so allocating one grows the map, and a
    /// cached map built before the allocation has to be retired or the added
    /// ground would never be gathered into it.
    #[test]
    fn allocating_a_region_grows_the_shared_heightmap() {
        let subject = terrain(4, "a.jdterrain");
        let mut store = store_for(&subject);
        store
            .entry_for(&subject)
            .expect("keyed")
            .set_heights(&[0.0; 16]);
        assert_eq!(store.heightmap(&subject).map.resolution, 4);

        store.retire_heightmap(&subject.data_path);
        store
            .entries
            .get_mut("a.jdterrain")
            .expect("keyed")
            .regions
            .ensure_region(jackdaw_terrain::RegionCoord::new(1, 0));

        assert_eq!(store.heightmap(&subject).map.resolution, 8);
    }

    /// A terrain with no document has no cells, and therefore no ground: there
    /// is no declared rectangle to pad out to.
    #[test]
    fn a_terrain_with_nothing_stored_has_no_ground() {
        let store = TerrainDataStore::default();
        let map = store.heightmap(&terrain(4, "nothing-here.jdterrain")).map;
        assert!(map.heights.is_empty());
        assert_eq!(map.resolution, 0);
    }

    /// A terrain wider than one region is stored as a grid of them, so there is
    /// something to author region by region. The default terrain, 1024 vertices
    /// per edge, is 4x4.
    #[test]
    fn a_terrain_wider_than_one_region_is_stored_as_a_grid_of_them() {
        let mut store = TerrainDataStore::default();
        let mut data = store
            .entry_for(&terrain(1024, "a.jdterrain"))
            .expect("keyed");
        // A kilometre of ground, which is four default-sized regions a
        // side. Nothing lays it down until something asks for it.
        data.ensure_extent(1024).expect("inside the region cap");
        let document = data.document();
        assert_eq!(document.regions.region_size().get(), 256);
        assert_eq!(document.regions.region_count(), 16);
    }

    /// A document held as one region the size of the whole grid, with
    /// non-trivial heights and control words.
    fn single_region_document(resolution: u32) -> RegionTerrainData {
        let mut regions = jackdaw_terrain::TerrainRegions::new(
            jackdaw_terrain::RegionSize::new(resolution).expect("a power of two"),
        );
        regions
            .ensure_grid(resolution)
            .expect("inside the region cap");
        for z in 0..resolution as i32 {
            for x in 0..resolution as i32 {
                regions.set_height(x, z, (x * 7 + z * 13) as f32 * 0.25);
                regions.set_control(
                    x,
                    z,
                    Control::default()
                        .with_base_id(((x + z) % 8) as u8)
                        .with_blend(((x * 3 + z) % 256) as u8),
                );
            }
        }
        RegionTerrainData {
            regions,
            ..RegionTerrainData::default()
        }
    }

    /// A document with no colour layer must not grow one by being opened for
    /// editing: carrying the default colour is not a paint.
    #[test]
    fn opening_a_document_does_not_mint_a_colour_layer_it_never_had() {
        let mut store = TerrainDataStore::default();
        store.insert("a.jdterrain", single_region_document(512));
        let data = store
            .entry_for(&terrain(512, "a.jdterrain"))
            .expect("the document opens");
        assert!(
            data.document()
                .regions
                .iter_sorted()
                .all(|(_, region)| region.color().is_none()),
            "no region should have been given a colour layer",
        );
    }

    /// The cap only makes regions finer, so every resolution the seam rule
    /// sizes keeps that size.
    #[test]
    fn a_resolution_of_one_region_or_less_keeps_its_derived_size() {
        for resolution in [2u32, 4, 129, 256] {
            assert_eq!(
                editor_region_size(resolution),
                jackdaw_terrain::RegionSize::for_resolution(resolution),
                "resolution {resolution}",
            );
        }
    }

    /// Nothing resizes a document. Its extent is the regions it holds, and the
    /// component carries no resolution to reconcile it against, so opening it
    /// for editing leaves its ground as it was.
    #[test]
    fn entry_for_leaves_the_extent_it_found_alone() {
        let subject = terrain(4, "a.jdterrain");
        let mut store = store_for(&subject);
        store.entry_for(&subject).expect("keyed").heights_mut()[0] = 3.0;

        // Reopened under a component claiming another shape entirely.
        let reopened = store.entry_for(&terrain(8, "a.jdterrain")).expect("keyed");
        assert_eq!(reopened.document().grid_resolution(), 4);
        assert_eq!(reopened.heights().len(), 16);
        assert_eq!(reopened.heights()[0], 3.0);
    }

    /// Sculpting near the origin leaves ground stored far away alone:
    /// there is no window, so nothing is outside one, and an edit touches
    /// only the regions it reaches.
    #[test]
    fn an_edit_near_the_origin_leaves_distant_ground_alone() {
        use jackdaw_terrain::{RegionCoord, TerrainRegions};

        let mut regions = TerrainRegions::new(RegionSize::new(4).unwrap());
        regions.set_height(20, 20, 2.0);
        regions.ensure_grid(4).expect("inside the region cap");
        let mut store = TerrainDataStore::default();
        store.insert(
            "a.jdterrain",
            RegionTerrainData {
                regions,
                ..default()
            },
        );

        let subject = terrain(4, "a.jdterrain");
        let rect = GridRect {
            x: 0,
            z: 0,
            width: 1,
            height: 1,
        };
        store.brush_heights(&subject, rect, |heights| heights[0] = 9.0);

        let document = store.get("a.jdterrain").expect("kept");
        // The distant ground is untouched. Writing a dense grid does bring the
        // regions between into being, since the grid spans them.
        assert_eq!(document.regions.height_at(20, 20), 2.0);
        assert_eq!(document.regions.height_at(0, 0), 9.0);
        assert!(document.regions.region(RegionCoord::ORIGIN).is_some());
    }

    /// A zero-resolution terrain has no cells to allocate a region for, but
    /// still keeps its document and its channel table.
    #[test]
    fn a_zero_resolution_terrain_has_a_document_but_no_window() {
        let mut store = TerrainDataStore::default();
        let data = store.entry_for(&terrain(0, "a.jdterrain")).expect("keyed");
        assert!(data.heights().is_empty());
        assert_eq!(data.document().regions.region_count(), 0);
        assert!(store.contains("a.jdterrain"));
    }

    #[test]
    fn a_terrain_without_a_data_path_has_no_entry() {
        let mut store = TerrainDataStore::default();
        assert!(store.entry_for(&terrain(4, "")).is_none());
        assert_eq!(store.iter().count(), 0);
    }

    #[test]
    fn heights_of_an_unknown_path_read_as_flat_rather_than_panicking() {
        let store = TerrainDataStore::default();
        assert!(store.heights("nothing-here.jdterrain").is_empty());
    }

    /// Adding a channel to a terrain that already has heights gives it
    /// `resolution^2` zeroes without the caller sizing it.
    #[test]
    fn a_channel_added_to_a_sculpted_terrain_is_zeroed_at_the_right_length() {
        use jackdaw_scene_types::{TerrainChannel, TerrainChannelElement};

        let mut sculpted = terrain(8, "a.jdterrain");
        let mut store = store_for(&sculpted);
        store
            .entry_for(&sculpted)
            .expect("keyed")
            .set_heights(&[2.0; 64]);

        sculpted.channels.push(TerrainChannel {
            name: "biome".to_string(),
            element: TerrainChannelElement::U8,
            palette: vec![],
        });
        let data = store.entry_for(&sculpted).expect("keyed");
        assert_eq!(data.channels().len(), 1);
        // A newly declared channel reads as zero on every region.
        assert!((0..8).all(|x| data.document().regions.channel_at(0, x, 0) == 0));
        assert_eq!(*data.heights(), [2.0; 64], "heights are untouched");
    }

    /// Matching by name means a reorder moves values with their layer instead
    /// of shuffling them into the wrong one.
    #[test]
    fn reordering_channels_moves_their_values_rather_than_shuffling_them() {
        use jackdaw_scene_types::{TerrainChannel, TerrainChannelElement};

        let channel = |name: &str| TerrainChannel {
            name: name.to_string(),
            element: TerrainChannelElement::U8,
            palette: vec![],
        };

        let mut two = terrain(4, "a.jdterrain");
        let mut store = store_for(&two);
        two.channels = vec![channel("biome"), channel("walkable")];
        {
            let mut data = store.entry_for(&two).expect("keyed");
            data.ensure_extent(4).expect("inside the region cap");
            data.set_channel_values(0, &{
                let mut v = vec![0u16; 16];
                v[0] = 11;
                v
            });
            data.set_channel_values(1, &{
                let mut v = vec![0u16; 16];
                v[0] = 22;
                v
            });
        }

        two.channels.swap(0, 1);
        let data = store.entry_for(&two).expect("keyed");
        assert_eq!(data.channels()[0].name, "walkable");
        assert_eq!(data.document().regions.channel_at(0, 0, 0), 22);
        assert_eq!(data.channels()[1].name, "biome");
        assert_eq!(data.document().regions.channel_at(1, 0, 0), 11);
    }

    /// Removing a channel drops its values with it, and leaves the
    /// survivors alone.
    #[test]
    fn removing_a_channel_drops_only_its_own_values() {
        use jackdaw_scene_types::{TerrainChannel, TerrainChannelElement};

        let channel = |name: &str| TerrainChannel {
            name: name.to_string(),
            element: TerrainChannelElement::U8,
            palette: vec![],
        };

        let mut two = terrain(4, "a.jdterrain");
        let mut store = store_for(&two);
        two.channels = vec![channel("biome"), channel("walkable")];
        {
            let mut data = store.entry_for(&two).expect("keyed");
            data.ensure_extent(4).expect("inside the region cap");
            let mut values = vec![0u16; 16];
            values[3] = 9;
            data.set_channel_values(1, &values);
        }

        two.channels.remove(0);
        let data = store.entry_for(&two).expect("keyed");
        assert_eq!(data.channels().len(), 1);
        assert_eq!(data.channels()[0].name, "walkable");
        assert_eq!(data.document().regions.channel_at(0, 3, 0), 9);
    }

    /// A `u8` channel widened to `u16` keeps what was painted; only its
    /// ceiling changes.
    #[test]
    fn widening_a_channel_keeps_its_values() {
        use jackdaw_scene_types::{TerrainChannel, TerrainChannelElement};

        let mut narrow = terrain(4, "a.jdterrain");
        let mut store = store_for(&narrow);
        narrow.channels = vec![TerrainChannel {
            name: "biome".to_string(),
            element: TerrainChannelElement::U8,
            palette: vec![],
        }];
        {
            let mut data = store.entry_for(&narrow).expect("keyed");
            data.ensure_extent(4).expect("inside the region cap");
            let mut values = vec![0u16; 16];
            values[2] = 200;
            data.set_channel_values(0, &values);
        }

        narrow.channels[0].element = TerrainChannelElement::U16;
        let data = store.entry_for(&narrow).expect("keyed");
        assert_eq!(
            data.channels()[0].element,
            jackdaw_terrain::ChannelElement::U16
        );
        assert_eq!(data.document().regions.channel_at(0, 2, 0), 200);
    }

    #[test]
    fn minted_paths_do_not_collide_with_what_the_store_already_holds() {
        let mut store = TerrainDataStore::default();
        let first = mint_data_path(&store, "level");
        assert_eq!(first, format!("level.terrain-0.{}", sidecar::EXTENSION));
        store.insert(first.clone(), RegionTerrainData::default());
        let second = mint_data_path(&store, "level");
        assert_ne!(second, first);
        assert_eq!(second, format!("level.terrain-1.{}", sidecar::EXTENSION));
    }

    #[test]
    fn an_unnamed_scene_still_mints_a_usable_path() {
        let store = TerrainDataStore::default();
        let path = mint_data_path(&store, "");
        assert!(path.starts_with("untitled.terrain-"));
        assert!(path.ends_with(sidecar::EXTENSION));
    }

    /// A path marked load-failed refuses every write rather than minting data
    /// that a later save would write over the real file. Paint and the material
    /// list live in that same document, so they are refused on the same terms
    /// as sculpting.
    #[test]
    fn a_load_failed_path_refuses_every_write() {
        let mut store = TerrainDataStore::default();
        store.mark_load_failed("a.jdterrain", "unreadable bytes");

        assert!(store.entry_for(&terrain(4, "a.jdterrain")).is_none());
        assert!(store.control_mut(&terrain(4, "a.jdterrain")).is_none());
        assert_eq!(
            store.set_materials("a.jdterrain", vec![TerrainMaterialSlot::new("grass")]),
            Err(TerrainMaterialError::LoadFailed),
        );
        assert_eq!(
            store.load_failed_reason("a.jdterrain"),
            Some("unreadable bytes")
        );

        assert!(!store.contains("a.jdterrain"));
        assert!(store.materials("a.jdterrain").is_empty());
    }

    /// A name the sidecar encoder would reject must not reach the store, where
    /// it would fail every later save of the scene.
    #[test]
    fn a_name_that_could_not_address_a_material_file_is_refused() {
        let mut store = TerrainDataStore::default();
        for bad in ["../escape", "/abs/grass", "my material"] {
            assert!(
                matches!(
                    store.set_materials("a.jdterrain", vec![TerrainMaterialSlot::new(bad)]),
                    Err(TerrainMaterialError::InvalidName { .. }),
                ),
                "{bad:?} must not be storable",
            );
        }
        assert!(store.materials("a.jdterrain").is_empty());
        assert!(!store.contains("a.jdterrain"));

        store
            .set_materials("a.jdterrain", vec![TerrainMaterialSlot::new("grass_05")])
            .expect("a plain material name is accepted");
        assert_eq!(store.materials("a.jdterrain")[0].material, "grass_05");
    }

    /// The empty name is the tombstone, not a bad name: it is how a vacated
    /// texture id spells itself, and it has to survive the store or a removal
    /// could not hold an id open.
    #[test]
    fn a_tombstone_stores_and_keeps_the_ids_above_it_in_place() {
        let mut store = TerrainDataStore::default();
        store
            .set_materials(
                "a.jdterrain",
                vec![
                    TerrainMaterialSlot::new("grass"),
                    TerrainMaterialSlot::tombstone(),
                    TerrainMaterialSlot::new("sand"),
                ],
            )
            .expect("a vacated id is storable");
        let materials = store.materials("a.jdterrain");
        assert_eq!(materials.len(), 3);
        assert!(materials[1].is_tombstone());
        assert_eq!(materials[2].material, "sand", "texture id 2 stayed id 2");
    }

    /// Several ids can be vacant at once. They all spell themselves the same
    /// way, which is not two materials colliding.
    #[test]
    fn tombstones_do_not_collide_with_each_other() {
        let mut store = TerrainDataStore::default();
        store
            .set_materials(
                "a.jdterrain",
                vec![
                    TerrainMaterialSlot::tombstone(),
                    TerrainMaterialSlot::tombstone(),
                    TerrainMaterialSlot::new("sand"),
                ],
            )
            .expect("vacated ids share the empty name by construction");
        assert_eq!(store.materials("a.jdterrain").len(), 3);
    }

    /// A vacated id at the end holds nothing open, an id past the end of the
    /// list being the same as one held open at the end of it, so it is dropped
    /// rather than counted against the ceiling.
    #[test]
    fn trailing_tombstones_are_dropped_and_interior_ones_are_kept() {
        let mut store = TerrainDataStore::default();
        store
            .set_materials(
                "a.jdterrain",
                vec![
                    TerrainMaterialSlot::new("grass"),
                    TerrainMaterialSlot::tombstone(),
                    TerrainMaterialSlot::new("sand"),
                    TerrainMaterialSlot::tombstone(),
                    TerrainMaterialSlot::tombstone(),
                ],
            )
            .expect("accepted");
        let materials = store.materials("a.jdterrain");
        assert_eq!(materials.len(), 3);
        assert!(materials[1].is_tombstone(), "the interior hole stays");
        assert_eq!(materials[2].material, "sand");
    }

    /// One material per texture id: a second slot on the same material
    /// would burn an id and an array layer for nothing.
    #[test]
    fn the_same_material_twice_in_one_list_is_refused() {
        let mut store = TerrainDataStore::default();
        assert_eq!(
            store.set_materials(
                "a.jdterrain",
                vec![
                    TerrainMaterialSlot::new("grass"),
                    TerrainMaterialSlot::new("rock"),
                    TerrainMaterialSlot::new("grass"),
                ],
            ),
            Err(TerrainMaterialError::AlreadyUsed("grass".to_string())),
        );
        assert!(store.materials("a.jdterrain").is_empty());
    }

    #[test]
    fn a_list_past_the_id_ceiling_is_refused_whole() {
        let mut store = TerrainDataStore::default();
        let over: Vec<_> = (0..MAX_TEXTURES + 1)
            .map(|i| TerrainMaterialSlot::new(format!("m{i}")))
            .collect();
        assert_eq!(
            store.set_materials("a.jdterrain", over),
            Err(TerrainMaterialError::Full)
        );
        assert!(store.materials("a.jdterrain").is_empty());
    }

    #[test]
    fn a_terrain_starts_with_no_materials_and_no_paint() {
        let store = TerrainDataStore::default();
        assert!(store.materials("a.jdterrain").is_empty());
        assert!(store.control("a.jdterrain").is_empty());
    }

    #[test]
    fn setting_and_emptying_a_material_list_round_trips() {
        let mut store = TerrainDataStore::default();
        store
            .set_materials(
                "a.jdterrain",
                vec![TerrainMaterialSlot {
                    material: "grass".to_string(),
                    uv_scale: 0.25,
                    detile: 0.0,
                }],
            )
            .expect("accepted");
        assert_eq!(store.materials("a.jdterrain").len(), 1);
        assert_eq!(store.materials("a.jdterrain")[0].uv_scale, 0.25);
        store
            .set_materials("a.jdterrain", Vec::new())
            .expect("emptying is always accepted");
        assert!(store.materials("a.jdterrain").is_empty());
    }

    /// Emptying the list keeps what was painted: the ids still mean something
    /// once materials are added back.
    #[test]
    fn emptying_the_material_list_keeps_the_control_map() {
        let terrain = terrain(4, "a.jdterrain");
        let mut store = store_for(&terrain);
        store.control_mut(&terrain).expect("keyed")[3] = Control::default().with_base_id(2);
        store
            .set_materials("a.jdterrain", vec![TerrainMaterialSlot::new("grass")])
            .expect("accepted");
        store
            .set_materials("a.jdterrain", Vec::new())
            .expect("accepted");
        assert_eq!(store.control("a.jdterrain")[3].base_id(), 2);
    }

    /// A paint stroke tells the renderer which cells it wrote, so the
    /// texel rebuild costs the brush footprint instead of the terrain.
    #[test]
    fn painting_marks_only_the_brushed_rect_for_upload() {
        let subject = terrain(16, "a.jdterrain");
        let mut store = store_for(&subject);
        let first = GridRect {
            x: 2,
            z: 2,
            width: 3,
            height: 3,
        };
        let second = GridRect {
            x: 6,
            z: 3,
            width: 2,
            height: 2,
        };
        store.control_rect_mut(&subject, first).expect("keyed");
        store.control_rect_mut(&subject, second).expect("keyed");

        assert_eq!(
            store.take_control_dirty("a.jdterrain"),
            Some(first.union(second)),
            "a stroke's frames have to accumulate into one block",
        );
        assert_eq!(store.take_control_dirty("a.jdterrain"), None);
    }

    /// Undo, cancel and a freshly loaded document have no rect to name, so they
    /// say the whole map: a partial upload would leave paint on screen that the
    /// document does not hold.
    #[test]
    fn a_whole_map_write_marks_the_whole_map_for_upload() {
        let subject = terrain(16, "a.jdterrain");
        let mut store = store_for(&subject);
        store.control_mut(&subject).expect("keyed");
        assert_eq!(
            store.take_control_dirty("a.jdterrain"),
            Some(GridRect::whole(16)),
        );
    }

    #[test]
    fn a_terrain_without_a_data_path_has_no_control_layer() {
        let mut store = TerrainDataStore::default();
        assert!(store.control_mut(&terrain(4, "")).is_none());
    }

    #[test]
    fn inserting_data_clears_a_previous_load_failed_mark() {
        let mut store = TerrainDataStore::default();
        store.mark_load_failed("a.jdterrain", "bad magic");
        store.insert("a.jdterrain", RegionTerrainData::default());
        assert!(!store.is_load_failed("a.jdterrain"));
        assert_eq!(store.load_failed_reason("a.jdterrain"), None);
        assert!(store.entry_for(&terrain(4, "a.jdterrain")).is_some());
    }
}
