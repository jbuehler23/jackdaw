//! Scatter kept in the terrain's document rather than as scene entities.
//!
//! A stored placement is twenty-four bytes in the sidecar and one batched
//! draw among thousands. An instance entity is a name, a scene node, an
//! AST row, an undo snapshot, a glTF scene spawn and an outliner line. The
//! editor's scatter operators write the former; the latter is what a hand
//! places, and what [`nth_in_group`] and [`remove`] turn one placement back
//! into when a hand wants to edit it.
//!
//! An older scene whose scatter is entities keeps working: nothing here
//! touches those groups until [`adopt`] converts one.

use bevy::prelude::*;
use jackdaw_terrain::region::RegionCoord;
use jackdaw_terrain::render::{ScatterChunk, ScatterDirty, TerrainScatter};
use jackdaw_terrain::{RegionTerrainData, ScatterPalette, ScatterPlacement};

use crate::commands::{CommandGroup, CommandHistory, DespawnEntity, EditorCommand};

use super::TerrainDataStore;

/// A terrain's whole stored scatter, as an undo entry holds it.
///
/// Whole rather than per region: a run replaces one group across every
/// region it reaches, so the regions an entry would have to name are the
/// ones it changed, and at twenty-four bytes a placement the copy is
/// cheaper than working out which those were.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScatterSnapshot {
    palette: ScatterPalette,
    regions: Vec<(RegionCoord, Vec<ScatterPlacement>)>,
}

impl ScatterSnapshot {
    pub fn capture(data: &RegionTerrainData) -> Self {
        Self {
            palette: data.scatter.clone(),
            regions: data
                .regions
                .iter_sorted()
                .map(|(coord, region)| (coord, region.placements().to_vec()))
                .collect(),
        }
    }

    fn apply(&self, data: &mut RegionTerrainData) {
        data.scatter = self.palette.clone();
        // A region the snapshot does not name is one an edit since it
        // allocated. Allocating is how a stamp stores a placement past the
        // terrain's edge, so leaving it behind would let an undone stamp
        // grow the terrain's extent, its mesh and its file for good.
        let extra: Vec<RegionCoord> = data
            .regions
            .iter_sorted()
            .map(|(coord, _)| coord)
            .filter(|coord| !self.regions.iter().any(|(known, _)| known == coord))
            .collect();
        for coord in extra {
            data.regions.remove_region(coord);
        }
        for (_, region) in data.regions.iter_sorted_mut() {
            region.placements_mut().clear();
        }
        // A snapshot region the document does not hold is one an undone
        // edit took away: a placement lives in the region covering it, so
        // putting the placements back puts the region back with them.
        for (coord, placements) in &self.regions {
            if placements.is_empty() {
                continue;
            }
            *data.regions.ensure_region(*coord).placements_mut() = placements.clone();
        }
    }

    /// What this entry costs the undo stack: the placements, the palette
    /// strings, and the per-region list headers behind them.
    fn heap_bytes(&self) -> usize {
        let palette: usize = self
            .palette
            .assets
            .iter()
            .map(|entry| entry.asset.capacity() + size_of::<jackdaw_terrain::ScatterPaletteEntry>())
            .chain(
                self.palette
                    .groups
                    .iter()
                    .map(|key| key.capacity() + size_of::<String>()),
            )
            .sum();
        palette
            + self
                .regions
                .iter()
                .map(|(_, placements)| {
                    size_of::<(RegionCoord, Vec<ScatterPlacement>)>()
                        + placements.len() * size_of::<ScatterPlacement>()
                })
                .sum::<usize>()
    }
}

/// Swap a terrain's stored scatter for another, and back again on undo.
pub struct SetScatterData {
    pub data_path: String,
    pub before: ScatterSnapshot,
    pub after: ScatterSnapshot,
    pub label: String,
}

impl SetScatterData {
    fn write(&self, world: &mut World, snapshot: &ScatterSnapshot) {
        let mut store = world.resource_mut::<TerrainDataStore>();
        let Some(data) = store.document_mut(&self.data_path) else {
            return;
        };
        snapshot.apply(data);
        store.mark_scatter_dirty(&self.data_path, None);
    }
}

impl EditorCommand for SetScatterData {
    fn execute(&mut self, world: &mut World) {
        let after = self.after.clone();
        self.write(world, &after);
    }

    fn undo(&mut self, world: &mut World) {
        let before = self.before.clone();
        self.write(world, &before);
    }

    fn description(&self) -> &str {
        &self.label
    }

    fn heap_bytes(&self) -> usize {
        self.before.heap_bytes() + self.after.heap_bytes()
    }
}

/// One placement a run produced, ready to be stored.
pub struct PendingPlacement {
    /// Terrain-local position.
    pub position: Vec3,
    pub yaw: f32,
    pub scale: f32,
    /// Index into the run's asset list, which the stamp interns into the
    /// document's palette.
    pub asset: usize,
}

/// Why a run could not be stored on the terrain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScatterStoreError {
    /// The terrain has no sidecar document to store placements in.
    NoDocument,
    /// The group table already names every index a placement could carry.
    GroupTableFull,
    /// The asset palette already names every index a placement could
    /// carry.
    PaletteFull,
}

impl ScatterStoreError {
    /// What to tell whoever asked for the run.
    pub fn message(self) -> &'static str {
        match self {
            Self::NoDocument => "this terrain has no sidecar to store its scatter in",
            Self::GroupTableFull => {
                "this terrain already holds as many scatter groups as a placement can name; \
                 clear one and try again"
            }
            Self::PaletteFull => {
                "this terrain's scatter palette already holds as many assets as a placement \
                 can name"
            }
        }
    }
}

/// Replace one group's stored placements, as one undo entry.
///
/// Returns how many were stored, or why they could not be.
pub fn stamp(
    world: &mut World,
    data_path: &str,
    key: &str,
    assets: &[String],
    placements: Vec<PendingPlacement>,
) -> Result<usize, ScatterStoreError> {
    let (before, after, stored) = {
        let mut store = world.resource_mut::<TerrainDataStore>();
        let data = store
            .document_mut(data_path)
            .ok_or(ScatterStoreError::NoDocument)?;
        let before = ScatterSnapshot::capture(data);

        // The key keeps the row it already had, so a group re-run a
        // hundred times is one row rather than a hundred.
        let group = data
            .scatter
            .intern_group(key)
            .map_err(|_| ScatterStoreError::GroupTableFull)?;
        data.clear_group(group);
        let palette = assets
            .iter()
            .map(|asset| data.scatter.intern_asset(asset))
            .collect::<Result<Vec<u16>, _>>()
            .map_err(|_| ScatterStoreError::PaletteFull)?;

        let mut stored = 0;
        for placement in placements {
            let Some(asset) = palette.get(placement.asset) else {
                continue;
            };
            if data
                .add_placement(
                    placement.position,
                    group,
                    *asset,
                    placement.yaw,
                    placement.scale,
                )
                .is_some()
            {
                stored += 1;
            }
        }
        (before, ScatterSnapshot::capture(data), stored)
    };

    push(
        world,
        SetScatterData {
            data_path: data_path.to_string(),
            before,
            after,
            label: format!("Scatter {key}"),
        },
    );
    Ok(stored)
}

/// Drop one group's stored placements, as one undo entry. Returns how many
/// went.
pub fn clear(world: &mut World, data_path: &str, key: &str) -> Option<usize> {
    let (before, after, removed) = {
        let mut store = world.resource_mut::<TerrainDataStore>();
        let data = store.document_mut(data_path)?;
        let before = ScatterSnapshot::capture(data);
        let group = data.scatter.group_index(key)?;
        let removed = data.remove_group(group);
        (before, ScatterSnapshot::capture(data), removed)
    };
    if removed == 0 {
        return Some(0);
    }
    push(
        world,
        SetScatterData {
            data_path: data_path.to_string(),
            before,
            after,
            label: format!("Clear Scatter {key}"),
        },
    );
    Some(removed)
}

/// Every stored group under a terrain, with how many placements each holds.
pub fn group_counts(store: &TerrainDataStore, data_path: &str) -> Vec<(String, usize)> {
    let Some(data) = store.get(data_path) else {
        return Vec::new();
    };
    let counts = data.group_counts();
    let mut rows: Vec<(String, usize)> = data
        .scatter
        .groups
        .iter()
        .enumerate()
        .filter(|(_, key)| !key.is_empty())
        .map(|(index, key)| (key.clone(), counts.get(index).copied().unwrap_or(0)))
        .filter(|(_, count)| *count > 0)
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

/// Where one placement sits and what it draws: what a promotion needs to
/// spawn an entity in its place.
pub struct PromotedPlacement {
    pub asset: String,
    pub region: RegionCoord,
    pub index: usize,
    pub transform: Transform,
}

/// The `index`th placement of `key`, in the order the document holds them.
pub fn nth_in_group(
    store: &TerrainDataStore,
    data_path: &str,
    key: &str,
    index: usize,
) -> Option<PromotedPlacement> {
    let data = store.get(data_path)?;
    let group = data.scatter.group_index(key)?;
    let (region, at, placement) = data.group_placements(group).nth(index)?;
    let asset = data.scatter.asset(placement.asset)?.asset.clone();
    Some(PromotedPlacement {
        asset,
        region,
        index: at,
        transform: Transform {
            translation: data.placement_position(region, placement),
            rotation: Quat::from_rotation_y(placement.yaw),
            scale: Vec3::splat(placement.scale),
        },
    })
}

/// Remove one placement from the document, as one undo entry.
pub fn remove(world: &mut World, data_path: &str, region: RegionCoord, index: usize) -> bool {
    let Some((before, after)) = ({
        let mut store = world.resource_mut::<TerrainDataStore>();
        store.document_mut(data_path).and_then(|data| {
            let before = ScatterSnapshot::capture(data);
            data.remove_placement(region, index)?;
            Some((before, ScatterSnapshot::capture(data)))
        })
    }) else {
        return false;
    };
    push(
        world,
        SetScatterData {
            data_path: data_path.to_string(),
            before,
            after,
            label: "Remove Placement".to_string(),
        },
    );
    true
}

/// Store a group of model entities as placements and despawn them, as one
/// undo entry.
///
/// Returns how many were stored. Undo puts the entities back and takes the
/// placements away again, because the two halves are one command group.
pub fn adopt(
    world: &mut World,
    data_path: &str,
    key: &str,
    group_entity: Entity,
    models: Vec<(String, Transform)>,
) -> Result<usize, ScatterStoreError> {
    let (before, after, stored) = {
        let mut store = world.resource_mut::<TerrainDataStore>();
        let data = store
            .document_mut(data_path)
            .ok_or(ScatterStoreError::NoDocument)?;
        let before = ScatterSnapshot::capture(data);
        let group = data
            .scatter
            .intern_group(key)
            .map_err(|_| ScatterStoreError::GroupTableFull)?;
        data.clear_group(group);

        let mut stored = 0;
        for (asset, transform) in &models {
            let asset_index = data
                .scatter
                .intern_asset(asset)
                .map_err(|_| ScatterStoreError::PaletteFull)?;
            let (_, yaw, _) = transform.rotation.to_euler(EulerRot::YXZ);
            let scale = transform.scale.max_element();
            if data
                .add_placement(transform.translation, group, asset_index, yaw, scale)
                .is_some()
            {
                stored += 1;
            }
        }
        (before, ScatterSnapshot::capture(data), stored)
    };

    let mut commands: Vec<Box<dyn EditorCommand>> = vec![Box::new(SetScatterData {
        data_path: data_path.to_string(),
        before,
        after,
        label: format!("Adopt Scatter {key}"),
    })];
    // The group's own despawn takes its subtree with it, and puts the
    // subtree back on undo, so the models are not despawned separately.
    commands.push(Box::new(DespawnEntity::from_world(world, group_entity)));

    let mut command: Box<dyn EditorCommand> = Box::new(CommandGroup {
        commands,
        label: format!("Adopt Scatter {key}"),
    });
    command.execute(world);
    world
        .resource_mut::<CommandHistory>()
        .push_executed(command);
    Ok(stored)
}

fn push(world: &mut World, command: SetScatterData) {
    let mut command: Box<dyn EditorCommand> = Box::new(command);
    command.execute(world);
    world
        .resource_mut::<CommandHistory>()
        .push_executed(command);
}

/// Keep what the renderer draws out of the outliner and out of the saved
/// scene.
///
/// A drawn placement is derived from the terrain's document the same way a
/// surface level is derived from its heights, so it is marked the same
/// way: the document already holds it, and a second copy in the scene text
/// would be spawned beside it on the next load.
///
/// The chunk alone carries the mark. Its children are unnamed, so neither
/// the outliner nor the scene writer reaches them on their own, and a mark
/// per drawn entity would be two component inserts and an archetype move
/// per placement on every rebuild.
pub fn hide_drawn_scatter(add: On<Add, ScatterChunk>, mut commands: Commands) {
    commands
        .entity(add.entity)
        .insert((crate::EditorHidden, crate::NonSerializable));
}

/// Keep each terrain's drawn scatter in step with its document.
///
/// The renderer works off components on the terrain entity; the store is
/// where the editor's documents live. This is the one place the two meet.
/// A terrain with neither is brought up to date whatever the store says,
/// so a scene reopened in the same tab draws its scatter without waiting
/// for another edit to mark it.
pub fn sync_terrain_scatter(
    mut commands: Commands,
    store: Res<TerrainDataStore>,
    terrains: Query<(
        Entity,
        &jackdaw_scene_types::Terrain,
        Option<&TerrainScatter>,
        Option<&ScatterDirty>,
    )>,
) {
    for (entity, terrain, drawn, pending) in &terrains {
        let dirty = store.take_scatter_dirty(&terrain.data_path);
        if dirty.is_none() && drawn.is_some() {
            continue;
        }
        let scatter = store
            .get(&terrain.data_path)
            .map(TerrainScatter::from_document)
            .unwrap_or_default();
        // Merged rather than replaced: a mark the renderer has not run
        // against yet is still owed a rebuild, and this frame's edit is
        // not the only region waiting.
        let mut mark = dirty.unwrap_or_else(ScatterDirty::all);
        if let Some(pending) = pending {
            mark.merge(pending);
        }
        commands.entity(entity).insert((scatter, mark));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jackdaw_terrain::ScatterPaletteEntry;
    use jackdaw_terrain::region::{RegionSize, TerrainRegions};

    fn document() -> RegionTerrainData {
        let mut data = RegionTerrainData {
            regions: TerrainRegions::new(RegionSize::new(8).unwrap()),
            ..RegionTerrainData::default()
        };
        data.regions.set_height(0, 0, 0.0);
        data
    }

    fn store_with(data: RegionTerrainData) -> World {
        let mut world = World::new();
        let mut store = TerrainDataStore::default();
        store.insert("t.jdterrain", data);
        world.insert_resource(store);
        world.insert_resource(CommandHistory::default());
        world
    }

    fn placements(count: usize) -> Vec<PendingPlacement> {
        (0..count)
            .map(|at| PendingPlacement {
                position: Vec3::new(at as f32, 0.0, 1.0),
                yaw: 0.0,
                scale: 1.0,
                asset: 0,
            })
            .collect()
    }

    #[test]
    fn a_stamp_stores_its_placements_and_undo_takes_them_away_again() {
        let mut world = store_with(document());
        let stored = stamp(
            &mut world,
            "t.jdterrain",
            "woods",
            &["models/tree.gltf".to_string()],
            placements(3),
        );
        assert_eq!(stored, Ok(3));
        assert_eq!(
            world
                .resource::<TerrainDataStore>()
                .get("t.jdterrain")
                .unwrap()
                .placement_count(),
            3
        );

        let mut history = world.remove_resource::<CommandHistory>().unwrap();
        history.undo(&mut world);
        world.insert_resource(history);
        assert_eq!(
            world
                .resource::<TerrainDataStore>()
                .get("t.jdterrain")
                .unwrap()
                .placement_count(),
            0
        );
    }

    #[test]
    fn re_running_a_group_replaces_its_placements_rather_than_adding_to_them() {
        let mut world = store_with(document());
        let assets = vec!["models/tree.gltf".to_string()];
        stamp(&mut world, "t.jdterrain", "woods", &assets, placements(3)).expect("stores");
        stamp(&mut world, "t.jdterrain", "woods", &assets, placements(2)).expect("stores");
        let store = world.resource::<TerrainDataStore>();
        assert_eq!(store.get("t.jdterrain").unwrap().placement_count(), 2);
        assert_eq!(
            group_counts(store, "t.jdterrain"),
            vec![("woods".to_string(), 2)]
        );
    }

    #[test]
    fn clearing_a_group_leaves_the_others_alone() {
        let mut world = store_with(document());
        let assets = vec!["models/tree.gltf".to_string()];
        stamp(&mut world, "t.jdterrain", "woods", &assets, placements(3)).expect("stores");
        stamp(&mut world, "t.jdterrain", "meadow", &assets, placements(2)).expect("stores");
        assert_eq!(clear(&mut world, "t.jdterrain", "woods"), Some(3));
        assert_eq!(
            group_counts(world.resource::<TerrainDataStore>(), "t.jdterrain"),
            vec![("meadow".to_string(), 2)]
        );
    }

    #[test]
    fn a_promoted_placement_reports_where_it_stood_and_leaves_the_document() {
        let mut world = store_with(document());
        stamp(
            &mut world,
            "t.jdterrain",
            "woods",
            &["models/tree.gltf".to_string()],
            placements(3),
        )
        .expect("stores");
        let promoted = nth_in_group(
            world.resource::<TerrainDataStore>(),
            "t.jdterrain",
            "woods",
            1,
        )
        .expect("the group holds three");
        assert_eq!(promoted.asset, "models/tree.gltf");
        assert_eq!(promoted.transform.translation, Vec3::new(1.0, 0.0, 1.0));

        assert!(remove(
            &mut world,
            "t.jdterrain",
            promoted.region,
            promoted.index
        ));
        assert_eq!(
            group_counts(world.resource::<TerrainDataStore>(), "t.jdterrain"),
            vec![("woods".to_string(), 2)]
        );
    }

    /// A stamp that reached past the terrain's edge allocated the region
    /// it landed in. Undo takes the ground back with the placements, or
    /// the terrain's extent, its mesh and its file grow for good.
    #[test]
    fn undoing_a_stamp_gives_back_the_ground_it_allocated() {
        let mut world = store_with(document());
        let before = world
            .resource::<TerrainDataStore>()
            .get("t.jdterrain")
            .unwrap()
            .regions
            .region_count();
        let stored = stamp(
            &mut world,
            "t.jdterrain",
            "woods",
            &["models/tree.gltf".to_string()],
            vec![PendingPlacement {
                position: Vec3::new(20.0, 0.0, 20.0),
                yaw: 0.0,
                scale: 1.0,
                asset: 0,
            }],
        );
        assert_eq!(stored, Ok(1));
        assert!(
            world
                .resource::<TerrainDataStore>()
                .get("t.jdterrain")
                .unwrap()
                .regions
                .region_count()
                > before
        );

        let mut history = world.remove_resource::<CommandHistory>().unwrap();
        history.undo(&mut world);
        world.insert_resource(history);
        assert_eq!(
            world
                .resource::<TerrainDataStore>()
                .get("t.jdterrain")
                .unwrap()
                .regions
                .region_count(),
            before
        );
    }

    /// An undo entry is the placements and the palette, not the ground:
    /// the heights of even a small terrain dwarf what a stamp changed.
    #[test]
    fn an_undo_entry_costs_the_placements_rather_than_the_document() {
        let mut data = document();
        for x in 0..64 {
            for z in 0..64 {
                data.regions.set_height(x, z, 1.0);
            }
        }
        let ground = data.regions.region_count() * 64 * 64 * size_of::<f32>();
        let mut world = store_with(data);
        stamp(
            &mut world,
            "t.jdterrain",
            "woods",
            &["models/tree.gltf".to_string()],
            placements(64),
        )
        .expect("the terrain has a document");

        let stored = ScatterSnapshot::capture(
            world
                .resource::<TerrainDataStore>()
                .get("t.jdterrain")
                .expect("the document is there"),
        )
        .heap_bytes();
        assert!(
            stored < ground / 4,
            "a snapshot of {stored} bytes against {ground} bytes of ground"
        );
    }

    #[test]
    fn a_group_re_run_keeps_the_row_it_already_had() {
        let mut world = store_with(document());
        let assets = vec!["models/tree.gltf".to_string()];
        for _ in 0..8 {
            stamp(&mut world, "t.jdterrain", "woods", &assets, placements(2))
                .expect("the terrain has a document");
        }
        let store = world.resource::<TerrainDataStore>();
        let data = store.get("t.jdterrain").unwrap();
        assert_eq!(data.scatter.groups, vec!["woods".to_string()]);
        assert_eq!(data.scatter.assets.len(), 1);
        assert_eq!(data.placement_count(), 2);
    }

    #[test]
    fn a_terrain_with_no_document_says_so_rather_than_storing_nothing_quietly() {
        let mut world = store_with(document());
        assert_eq!(
            stamp(
                &mut world,
                "missing.jdterrain",
                "woods",
                &["models/tree.gltf".to_string()],
                placements(1),
            ),
            Err(ScatterStoreError::NoDocument)
        );
    }

    #[test]
    fn a_snapshot_round_trips_a_document_it_was_taken_from() {
        let mut data = document();
        data.scatter.assets.push(ScatterPaletteEntry::new("a.glb"));
        data.scatter.groups.push("g".to_string());
        data.add_placement(Vec3::new(1.0, 0.0, 1.0), 0, 0, 0.0, 1.0)
            .expect("the region is allocated");
        let snapshot = ScatterSnapshot::capture(&data);

        let mut cleared = data.clone();
        cleared.remove_group(0);
        assert_ne!(ScatterSnapshot::capture(&cleared), snapshot);
        snapshot.apply(&mut cleared);
        assert_eq!(ScatterSnapshot::capture(&cleared), snapshot);
    }
}
