//! In-memory source of truth for terrain bulk data.
//!
//! Heights and paint-channel values do not live on the `Terrain`
//! component and never enter the scene document. They live here, keyed by
//! the scene-relative sidecar path the component carries, and they are
//! written to that sidecar when the scene is saved.
//!
//! The active tab's decoded data lives in this resource. Inactive
//! [`crate::scenes::SceneTab`]s own their stores directly; tab capture moves
//! the live resource into the outgoing tab and activation restores the
//! incoming tab's store before importing sidecars (`src/scenes/swap.rs`).
//! This keeps identical scene-relative paths isolated between tabs while
//! entities continue to round-trip through BSN text.

use bevy::log::warn_once;
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use jackdaw_terrain::TerrainData;
use jackdaw_terrain::sidecar;

/// The active scene's terrain bulk data, keyed by
/// [`jackdaw_scene_types::Terrain::data_path`].
#[derive(Resource, Default)]
pub struct TerrainDataStore {
    entries: HashMap<String, TerrainData>,
    /// Sidecar paths whose most recent read attempt found a file but
    /// could not decode it (corrupt bytes, a newer format version, ...).
    ///
    /// Distinct from a path with no entry at all, which just means
    /// "never loaded or created" -- that case is the ordinary flat-
    /// terrain-on-a-missing-sidecar story and stays lenient. A
    /// load-failed path is different: real data exists on disk and this
    /// store must not fabricate a zeroed replacement for it, either for
    /// editing or for save to write back over the original.
    load_failed: HashSet<String>,
}

impl TerrainDataStore {
    /// Data for a sidecar path, if this store has any.
    pub fn get(&self, data_path: &str) -> Option<&TerrainData> {
        self.entries.get(data_path)
    }

    /// Mutable data for a sidecar path, if this store has any.
    pub fn get_mut(&mut self, data_path: &str) -> Option<&mut TerrainData> {
        self.entries.get_mut(data_path)
    }

    /// Heights for a sidecar path, or an empty slice when absent.
    ///
    /// Callers that only sample heights want a slice, not an `Option`,
    /// and a terrain with no data reads as flat -- which is exactly what
    /// a missing sidecar is supposed to look like.
    pub fn heights(&self, data_path: &str) -> &[f32] {
        self.entries
            .get(data_path)
            .map(|data| data.heights.as_slice())
            .unwrap_or(&[])
    }

    /// Install data for a sidecar path, replacing anything already there
    /// and clearing any load-failed mark -- a fresh, successfully decoded
    /// read supersedes a previous failure for the same path.
    pub fn insert(&mut self, data_path: impl Into<String>, data: TerrainData) {
        let data_path = data_path.into();
        self.load_failed.remove(&data_path);
        self.entries.insert(data_path, data);
    }

    /// Drop a sidecar path's data.
    pub fn remove(&mut self, data_path: &str) -> Option<TerrainData> {
        self.entries.remove(data_path)
    }

    /// Whether a sidecar path has data.
    pub fn contains(&self, data_path: &str) -> bool {
        self.entries.contains_key(data_path)
    }

    /// Mark a sidecar path as failed to load: a file exists but its
    /// contents could not be decoded. See the `load_failed` field's docs
    /// (above, on this struct) for why this is kept separate from "no
    /// entry".
    pub fn mark_load_failed(&mut self, data_path: impl Into<String>) {
        self.load_failed.insert(data_path.into());
    }

    /// Whether a sidecar path's last read attempt failed to decode.
    pub fn is_load_failed(&self, data_path: &str) -> bool {
        self.load_failed.contains(data_path)
    }

    /// Data for a terrain, created zeroed at the terrain's resolution if
    /// absent, resized if the resolution changed underneath, and
    /// reconciled against the terrain's declared channels.
    ///
    /// Every mutating path goes through here, so a channel or height
    /// array can never be a different length than the terrain claims, and
    /// a channel the user just added is already zeroed at the right
    /// length by the time anything tries to paint it.
    ///
    /// Returns `None` and refuses the edit for a path marked
    /// [`Self::is_load_failed`]: minting zeroed data here is exactly the
    /// silent-data-loss path this store exists to prevent, since the
    /// next save would otherwise write that zeroed data over the real
    /// file.
    pub fn entry_for(
        &mut self,
        terrain: &jackdaw_scene_types::Terrain,
    ) -> Option<&mut TerrainData> {
        if terrain.data_path.is_empty() {
            return None;
        }
        if self.load_failed.contains(&terrain.data_path) {
            warn_once!(
                "Refusing to edit terrain data that failed to load; \
                 fix or replace the sidecar file and reload the scene"
            );
            return None;
        }
        let data = self
            .entries
            .entry(terrain.data_path.clone())
            .or_insert_with(|| TerrainData {
                resolution: terrain.resolution,
                ..default()
            });
        if data.resolution != terrain.resolution {
            data.resolution = terrain.resolution;
        }
        if data.heights.len() != terrain.cell_count() {
            data.normalize();
        }
        reconcile_channels(data, terrain);
        Some(data)
    }

    /// Every (sidecar path, data) pair currently held.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &TerrainData)> {
        self.entries.iter()
    }
}

/// Bring a terrain's stored channel values in line with the channel
/// descriptors on its component.
///
/// The component owns the channel table -- names, widths, order -- and
/// the sidecar owns the values. When the user adds, removes, renames or
/// reorders a channel in the inspector, this is what carries the values
/// along: matched by name, so a rename keeps what was painted and a
/// reorder moves it rather than shuffling it into the wrong layer.
///
/// A channel added to a terrain that already has heights is initialised
/// to zero at `resolution^2` here, which is the only place that can
/// happen without the caller having to remember it.
fn reconcile_channels(data: &mut TerrainData, terrain: &jackdaw_scene_types::Terrain) {
    use jackdaw_terrain::{ChannelData, ChannelElement};

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
        for channel in &mut data.channels {
            channel.resize_to(terrain.resolution);
        }
        return;
    }

    let mut previous = std::mem::take(&mut data.channels);
    data.channels = terrain
        .channels
        .iter()
        .map(|descriptor| {
            let element = wanted(descriptor.element);
            let taken = previous
                .iter()
                .position(|had| had.name == descriptor.name)
                .map(|at| previous.remove(at));
            let mut channel = taken.unwrap_or_else(|| {
                ChannelData::new(descriptor.name.clone(), element, terrain.resolution)
            });
            channel.name = descriptor.name.clone();
            channel.element = element;
            channel.resize_to(terrain.resolution);
            channel
        })
        .collect();
}

/// Mint a sidecar path for a new terrain in `scene_stem`'s scene, unique
/// against everything the store already holds.
///
/// Uniqueness is checked against the whole store, not just the terrains
/// currently spawned in the scene: the store can hold entries for a
/// terrain the scene does not currently have live, e.g. one an undo
/// brought back or a delete has not yet been redone past. Checking only
/// live terrains could mint a path that collides with one of those once
/// it resurfaces. This does not, on its own, prevent a collision with a
/// path minted in a *different* open tab -- each inactive
/// [`crate::scenes::SceneTab`] owns its own separate
/// [`TerrainDataStore`], so this function only ever sees one tab's store
/// at a time.
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

    #[test]
    fn entry_for_creates_a_zeroed_array_at_the_terrain_resolution() {
        let mut store = TerrainDataStore::default();
        let data = store.entry_for(&terrain(4, "a.jdterrain")).expect("keyed");
        assert_eq!(data.heights.len(), 16);
        assert!(data.heights.iter().all(|h| *h == 0.0));
    }

    #[test]
    fn entry_for_resizes_when_the_resolution_changed_underneath() {
        let mut store = TerrainDataStore::default();
        store
            .entry_for(&terrain(4, "a.jdterrain"))
            .expect("keyed")
            .heights[0] = 3.0;
        let grown = store.entry_for(&terrain(8, "a.jdterrain")).expect("keyed");
        assert_eq!(grown.resolution, 8);
        assert_eq!(grown.heights.len(), 64);
        assert_eq!(grown.heights[0], 3.0, "existing cells survive the resize");
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

    /// The headline acceptance for channels: adding one to a terrain that
    /// already has heights must give it `resolution^2` zeroes, without
    /// the caller having to remember to size it.
    #[test]
    fn a_channel_added_to_a_sculpted_terrain_is_zeroed_at_the_right_length() {
        use jackdaw_scene_types::{TerrainChannel, TerrainChannelElement};

        let mut store = TerrainDataStore::default();
        let mut sculpted = terrain(8, "a.jdterrain");
        store.entry_for(&sculpted).expect("keyed").heights = vec![2.0; 64];

        sculpted.channels.push(TerrainChannel {
            name: "biome".to_string(),
            element: TerrainChannelElement::U8,
            palette: vec![],
        });
        let data = store.entry_for(&sculpted).expect("keyed");
        assert_eq!(data.channels.len(), 1);
        assert_eq!(data.channels[0].values.len(), 64);
        assert!(data.channels[0].values.iter().all(|v| *v == 0));
        assert_eq!(data.heights, vec![2.0; 64], "heights are untouched");
    }

    /// Renaming a channel must carry what was painted into it. Matching
    /// by name means a reorder moves values with their layer instead of
    /// shuffling them into the wrong one.
    #[test]
    fn reordering_channels_moves_their_values_rather_than_shuffling_them() {
        use jackdaw_scene_types::{TerrainChannel, TerrainChannelElement};

        let channel = |name: &str| TerrainChannel {
            name: name.to_string(),
            element: TerrainChannelElement::U8,
            palette: vec![],
        };

        let mut store = TerrainDataStore::default();
        let mut two = terrain(4, "a.jdterrain");
        two.channels = vec![channel("biome"), channel("walkable")];
        {
            let data = store.entry_for(&two).expect("keyed");
            data.channels[0].values[0] = 11;
            data.channels[1].values[0] = 22;
        }

        two.channels.swap(0, 1);
        let data = store.entry_for(&two).expect("keyed");
        assert_eq!(data.channels[0].name, "walkable");
        assert_eq!(data.channels[0].values[0], 22);
        assert_eq!(data.channels[1].name, "biome");
        assert_eq!(data.channels[1].values[0], 11);
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

        let mut store = TerrainDataStore::default();
        let mut two = terrain(4, "a.jdterrain");
        two.channels = vec![channel("biome"), channel("walkable")];
        store.entry_for(&two).expect("keyed").channels[1].values[3] = 9;

        two.channels.remove(0);
        let data = store.entry_for(&two).expect("keyed");
        assert_eq!(data.channels.len(), 1);
        assert_eq!(data.channels[0].name, "walkable");
        assert_eq!(data.channels[0].values[3], 9);
    }

    /// A `u8` channel widened to `u16` keeps what was painted; only its
    /// ceiling changes.
    #[test]
    fn widening_a_channel_keeps_its_values() {
        use jackdaw_scene_types::{TerrainChannel, TerrainChannelElement};

        let mut store = TerrainDataStore::default();
        let mut narrow = terrain(4, "a.jdterrain");
        narrow.channels = vec![TerrainChannel {
            name: "biome".to_string(),
            element: TerrainChannelElement::U8,
            palette: vec![],
        }];
        store.entry_for(&narrow).expect("keyed").channels[0].values[2] = 200;

        narrow.channels[0].element = TerrainChannelElement::U16;
        let data = store.entry_for(&narrow).expect("keyed");
        assert_eq!(
            data.channels[0].element,
            jackdaw_terrain::ChannelElement::U16
        );
        assert_eq!(data.channels[0].values[2], 200);
    }

    #[test]
    fn minted_paths_do_not_collide_with_what_the_store_already_holds() {
        let mut store = TerrainDataStore::default();
        let first = mint_data_path(&store, "level");
        assert_eq!(first, format!("level.terrain-0.{}", sidecar::EXTENSION));
        store.insert(first.clone(), TerrainData::default());
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

    /// C1: a path marked load-failed must refuse edits rather than mint
    /// zeroed data that a later save would write over the real file.
    #[test]
    fn entry_for_refuses_a_load_failed_path() {
        let mut store = TerrainDataStore::default();
        store.mark_load_failed("a.jdterrain");
        assert!(store.entry_for(&terrain(4, "a.jdterrain")).is_none());
        assert!(!store.contains("a.jdterrain"));
    }

    #[test]
    fn inserting_data_clears_a_previous_load_failed_mark() {
        let mut store = TerrainDataStore::default();
        store.mark_load_failed("a.jdterrain");
        store.insert("a.jdterrain", TerrainData::default());
        assert!(!store.is_load_failed("a.jdterrain"));
        assert!(store.entry_for(&terrain(4, "a.jdterrain")).is_some());
    }
}
