//! The operator for a terrain's shape: how far apart its cells sit.
//!
//! Cell spacing is the whole of a terrain's shape. How many cells there
//! are is not authored: the allocated regions are the terrain, and
//! sculpting past the last one allocates another.
//!
//! A cell size reaches two places. The component carries it as the
//! authoring surface, what the user scrubs and what an undo restores; the
//! document carries it because a save writes the sidecar before the scene
//! text, so the geometry travels with the cells it describes and a
//! half-written save still draws. Both are written together here, and on
//! load the document's copy wins.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_commands::EditorCommand;

use super::ops::has_selected_terrain;
use super::{TerrainDataStore, TerrainDirtyChunks};
use crate::commands::CommandHistory;
use crate::selection::Selection;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TerrainShapeCellSizeOp>();
}

/// Closest and furthest apart a terrain's cells may sit, in metres.
///
/// A quarter metre resolves a kerb; four metres is a coarse landscape
/// terrain. The bounds keep a scrub drag or a script from collapsing the
/// terrain to zero cell size or pushing its float positions past what the
/// clipmap resolves.
pub const MIN_CELL_SIZE: f32 = 0.25;
pub const MAX_CELL_SIZE: f32 = 4.0;

/// A cell size brought inside the allowed range.
///
/// Clamped rather than refused: these values arrive from a scrub drag,
/// where a refusal reads as the field being stuck. Non-finite takes the
/// floor rather than letting a NaN reach the grid.
pub fn clamp_cell_size(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(MIN_CELL_SIZE, MAX_CELL_SIZE)
    } else {
        MIN_CELL_SIZE
    }
}

/// One undo entry for a terrain's cell size.
///
/// Restores the component, the scene document node and the terrain
/// document together, since a cell size that reached only some of them
/// leaves the three disagreeing. The stored cells are not part of it:
/// changing cell size moves no height.
pub struct SetTerrainShape {
    entity: Entity,
    data_path: String,
    old: f32,
    new: f32,
    label: String,
}

impl SetTerrainShape {
    pub fn new(entity: Entity, data_path: String, old: f32, new: f32, label: String) -> Self {
        Self {
            entity,
            data_path,
            old,
            new,
            label,
        }
    }

    fn restore(&self, world: &mut World, old: bool) {
        let cell_size = if old { self.old } else { self.new };
        let Some(mut terrain) = world.get_mut::<jackdaw_scene_types::Terrain>(self.entity) else {
            return;
        };
        terrain.cell_size = cell_size;
        let terrain = terrain.clone();
        if let Some(mut store) = world.get_resource_mut::<TerrainDataStore>() {
            store.set_cell_size(&self.data_path, cell_size);
        }
        crate::commands::sync_component_to_ast(
            world,
            self.entity,
            "jackdaw_scene_types::types::Terrain",
            &terrain,
        );
        if let Some(mut dirty) = world.get_mut::<TerrainDirtyChunks>(self.entity) {
            dirty.rebuild_all = true;
        }
    }
}

impl EditorCommand for SetTerrainShape {
    fn execute(&mut self, world: &mut World) {
        self.restore(world, false);
    }

    fn undo(&mut self, world: &mut World) {
        self.restore(world, true);
    }

    fn description(&self) -> &str {
        &self.label
    }
}

/// Apply a cell size change. A change that moves nothing leaves no undo
/// entry.
pub(super) fn commit_shape(world: &mut World, entity: Entity, cell_size: f32) {
    let Some(terrain) = world.get::<jackdaw_scene_types::Terrain>(entity) else {
        return;
    };
    let (old, data_path) = (terrain.cell_size, terrain.data_path.clone());
    if old == cell_size {
        return;
    }
    world.resource_scope(|world, mut history: Mut<CommandHistory>| {
        history.execute(
            Box::new(SetTerrainShape::new(
                entity,
                data_path,
                old,
                cell_size,
                "Set Terrain Cell Size".to_string(),
            )),
            world,
        );
    });
}

/// Set how far apart the selected terrain's cells sit.
///
/// A lateral rescale: the same cells cover more or less ground and no
/// height moves, so a terrain sculpted at one cell size and scrubbed to
/// another keeps its shape. Nothing is resampled in either direction.
///
/// `allows_undo = false` because it pushes its own [`SetTerrainShape`]
/// entry, and a framework diff would double-record the change.
#[operator(
    id = "terrain.shape.cell_size",
    label = "Terrain Cell Size",
    description = "Set how far apart the selected terrain's cells sit, in metres.",
    is_available = has_selected_terrain,
    allows_undo = false,
    params(value(f64, doc = "Metres per cell edge."),),
)]
pub(crate) fn terrain_shape_cell_size(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    mut commands: Commands,
) -> OperatorResult {
    let entity = selection.primary()?;
    let cell_size = clamp_cell_size(params.as_float("value")? as f32);
    commands.queue(move |world: &mut World| {
        commit_shape(world, entity, cell_size);
    });
    OperatorResult::Finished
}

#[cfg(test)]
mod tests {
    use jackdaw_terrain::{
        RegionTerrainData,
        region::{RegionCoord, RegionSize, TerrainRegions},
    };

    use super::*;

    fn document(span: i32) -> RegionTerrainData {
        let mut regions = TerrainRegions::new(RegionSize::DEFAULT);
        for rz in 0..span {
            for rx in 0..span {
                regions.ensure_region(RegionCoord::new(rx, rz));
            }
        }
        RegionTerrainData {
            regions,
            ..default()
        }
    }

    /// A world holding one selected terrain registered in the document,
    /// with `span` regions of stored ground behind it.
    fn world_with_terrain(span: i32) -> (World, Entity) {
        use bevy::ecs::reflect::AppTypeRegistry;

        let mut world = World::new();
        world.init_resource::<AppTypeRegistry>();
        {
            let registry = world.resource::<AppTypeRegistry>().clone();
            let mut writer = registry.write();
            writer.register::<Name>();
            writer.register::<jackdaw_scene_types::Terrain>();
            writer.register::<jackdaw_scene_types::SceneNodeId>();
        }
        world.init_resource::<jackdaw_bsn::SceneBsnAst>();
        world.insert_resource(CommandHistory::default());
        let mut store = TerrainDataStore::default();
        store.insert("zone.terrain-0.jdterrain".to_string(), document(span));
        world.insert_resource(store);

        let entity = world
            .spawn((
                Name::new("Terrain"),
                jackdaw_scene_types::Terrain {
                    data_path: "zone.terrain-0.jdterrain".to_string(),
                    ..default()
                },
            ))
            .id();
        crate::scene_io::register_entity_in_ast(&mut world, entity);
        world.insert_resource(Selection {
            entities: vec![entity],
        });
        (world, entity)
    }

    fn cell_size(world: &World, entity: Entity) -> f32 {
        world
            .get::<jackdaw_scene_types::Terrain>(entity)
            .expect("the terrain is still there")
            .cell_size
    }

    fn saved(world: &mut World) -> String {
        crate::scene_io::emit_bsn_scene_with_inline_assets(world, std::path::Path::new("."))
    }

    /// The component, the terrain document and the scene text end up
    /// agreeing, or the next load reconciles them apart again.
    #[test]
    fn a_cell_size_change_reaches_the_component_the_document_and_undo() {
        let (mut world, entity) = world_with_terrain(4);

        commit_shape(&mut world, entity, 2.0);

        assert_eq!(cell_size(&world, entity), 2.0);
        assert_eq!(
            world
                .resource::<TerrainDataStore>()
                .grid("zone.terrain-0.jdterrain")
                .expect("stated")
                .cell_size,
            2.0,
        );
        let text = saved(&mut world);
        assert!(
            text.contains("cell_size: 2.0"),
            "the scene text has to carry it, or a save loses it:\n{text}"
        );

        world.resource_scope(|world, mut history: Mut<CommandHistory>| {
            history.undo(world);
        });
        assert_eq!(cell_size(&world, entity), 1.0);
    }

    /// A redo puts the change back everywhere the undo took it from.
    #[test]
    fn redo_puts_a_cell_size_change_back() {
        let (mut world, entity) = world_with_terrain(4);

        commit_shape(&mut world, entity, 0.5);
        world.resource_scope(|world, mut history: Mut<CommandHistory>| {
            history.undo(world);
        });
        assert_eq!(cell_size(&world, entity), 1.0);

        world.resource_scope(|world, mut history: Mut<CommandHistory>| {
            history.redo(world);
        });

        assert_eq!(cell_size(&world, entity), 0.5);
        assert_eq!(
            world
                .resource::<TerrainDataStore>()
                .grid("zone.terrain-0.jdterrain")
                .expect("stated")
                .cell_size,
            0.5,
        );
    }

    /// A lateral rescale: the same cells, further apart. Nothing is
    /// resampled and no height moves.
    #[test]
    fn changing_the_cell_size_rescales_without_touching_a_height() {
        let (mut world, entity) = world_with_terrain(1);
        let terrain = world
            .get::<jackdaw_scene_types::Terrain>(entity)
            .expect("spawned")
            .clone();

        let before = world.resource::<TerrainDataStore>().grid_shape(&terrain);
        let heights = world
            .resource::<TerrainDataStore>()
            .heights("zone.terrain-0.jdterrain")
            .to_vec();

        commit_shape(&mut world, entity, 2.0);

        let terrain = world
            .get::<jackdaw_scene_types::Terrain>(entity)
            .expect("still there")
            .clone();
        let after = world.resource::<TerrainDataStore>().grid_shape(&terrain);
        assert_eq!(after.resolution, before.resolution, "the same cells");
        assert_eq!(after.size, before.size * 2.0, "over twice the ground");
        assert_eq!(
            *world
                .resource::<TerrainDataStore>()
                .heights("zone.terrain-0.jdterrain"),
            *heights.as_slice(),
            "no height moved",
        );
    }

    #[test]
    fn a_cell_size_outside_the_range_is_clamped_rather_than_refused() {
        assert_eq!(clamp_cell_size(0.0), MIN_CELL_SIZE);
        assert_eq!(clamp_cell_size(1000.0), MAX_CELL_SIZE);
        assert_eq!(clamp_cell_size(f32::NAN), MIN_CELL_SIZE);
        assert_eq!(clamp_cell_size(1.5), 1.5);
    }
}
