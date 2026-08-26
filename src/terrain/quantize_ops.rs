//! Operators for a terrain's grid quantization.
//!
//! Quantization is a three-field descriptor on the `Terrain` component,
//! small and inline in the scene document, so every mutation here syncs the
//! component back to the AST. The heights it governs are bulk data and
//! travel the sidecar route: the apply operator hands the snapped array to
//! [`SetTerrainHeights`] and puts no height near the document.
//!
//! Turning quantization off changes how the terrain is meshed and stops
//! later strokes snapping; it un-snaps nothing already stored.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_scene_types::TerrainQuantization;

use super::ops::has_selected_terrain;
use super::sculpt::SetTerrainHeights;
use super::{TerrainDataStore, TerrainDirtyChunks};
use crate::commands::CommandHistory;
use crate::selection::Selection;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TerrainQuantizeToggleOp>()
        .register_operator::<TerrainQuantizeApplyOp>();
}

/// Edit a terrain's quantization descriptor and push it to the document.
///
/// The inspector's numeric fields and the on/off toggle both come through
/// here: a descriptor edit reaches the AST, and flipping `enabled` changes
/// which mesher runs and needs a full rebuild.
pub(super) fn commit_quantization(
    world: &mut World,
    entity: Entity,
    edit: impl FnOnce(&mut TerrainQuantization),
) {
    let Some(mut terrain) = world.get_mut::<jackdaw_scene_types::Terrain>(entity) else {
        return;
    };
    let before = terrain.quantization.clone();
    edit(&mut terrain.quantization);
    if terrain.quantization == before {
        return;
    }
    let shading_changed = terrain.quantization.enabled != before.enabled;
    let terrain = terrain.clone();
    crate::commands::sync_component_to_ast(
        world,
        entity,
        "jackdaw_scene_types::types::Terrain",
        &terrain,
    );
    if shading_changed && let Some(mut dirty) = world.get_mut::<TerrainDirtyChunks>(entity) {
        dirty.rebuild_all = true;
    }
}

/// Turn quantization on or off for the selected terrain.
#[operator(
    id = "terrain.quantize.toggle",
    label = "Toggle Quantization",
    description = "Snap the terrain to a metric grid and show its surface as flat cells.",
    is_available = has_selected_terrain
)]
pub(crate) fn terrain_quantize_toggle(
    _: In<OperatorParameters>,
    selection: Res<Selection>,
    mut commands: Commands,
) -> OperatorResult {
    let entity = selection.primary()?;
    commands.queue(move |world: &mut World| {
        commit_quantization(world, entity, |q| q.enabled = !q.enabled);
    });
    OperatorResult::Finished
}

/// Snap the selected terrain's existing heights and pin its cell size.
///
/// Sculpting, generating and eroding snap as they go while quantization is
/// on, so this operator covers heights stored before it was turned on or
/// under a different step.
///
/// `allows_undo = false` because it pushes its own [`SetTerrainHeights`]
/// entry, and a framework diff would double-record the change.
#[operator(
    id = "terrain.quantize.apply",
    label = "Apply Quantization",
    description = "Snap every stored height to the elevation step and pin the \
                   terrain's cells to the quantized cell size.",
    is_available = has_selected_terrain,
    allows_undo = false
)]
pub(crate) fn terrain_quantize_apply(
    _: In<OperatorParameters>,
    selection: Res<Selection>,
    mut commands: Commands,
) -> OperatorResult {
    let entity = selection.primary()?;
    commands.queue(move |world: &mut World| apply_quantization(world, entity));
    OperatorResult::Finished
}

/// One history entry covering both halves of the snap.
///
/// The heights and the cell size move together: undoing the snapped
/// heights without the spacing would leave the terrain at a spacing its
/// heights were not snapped for.
///
/// The spacing half is `super::shape_ops::SetTerrainShape`, the same
/// command the cell size chip pushes, so a cell size changes by one path.
fn apply_quantization(world: &mut World, entity: Entity) {
    let Some(terrain) = world.get::<jackdaw_scene_types::Terrain>(entity) else {
        return;
    };
    let terrain = terrain.clone();
    if !terrain.quantization.enabled {
        return;
    }

    let old_cell_size = terrain.cell_size;
    let new_cell_size = terrain
        .quantization
        .active_cell_size()
        .map(super::shape_ops::clamp_cell_size)
        .unwrap_or(old_cell_size);

    let Some(old_heights) = world
        .resource_mut::<TerrainDataStore>()
        .entry_for(&terrain)
        .map(|data| data.heights().to_vec())
    else {
        return;
    };
    let mut new_heights = old_heights.clone();
    if let Some(step) = terrain.quantization.active_height_step() {
        jackdaw_terrain::quantize_heights(&mut new_heights, step);
    }

    // An already-conforming terrain leaves no undo entry.
    if new_heights == old_heights && new_cell_size == old_cell_size {
        return;
    }

    let label = "Quantize Terrain".to_string();
    let mut commands: Vec<Box<dyn jackdaw_commands::EditorCommand>> = Vec::new();
    if new_heights != old_heights {
        commands.push(Box::new(SetTerrainHeights::whole(
            entity,
            old_heights,
            new_heights,
            label.clone(),
        )));
    }
    if new_cell_size != old_cell_size {
        commands.push(Box::new(super::shape_ops::SetTerrainShape::new(
            entity,
            terrain.data_path.clone(),
            old_cell_size,
            new_cell_size,
            label.clone(),
        )));
    }
    let group = crate::commands::CommandGroup { commands, label };
    world.resource_scope(|world, mut history: Mut<CommandHistory>| {
        history.execute(Box::new(group), world);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATA_PATH: &str = "zone.terrain-0.jdterrain";

    fn world_with_terrain(quantization: TerrainQuantization) -> (World, Entity) {
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
        world.init_resource::<CommandHistory>();
        let mut store = TerrainDataStore::default();
        store.insert(
            DATA_PATH.to_string(),
            jackdaw_terrain::RegionTerrainData::from_legacy_v1(&jackdaw_terrain::TerrainData {
                resolution: 4,
                heights: vec![0.3; 16],
                channels: vec![],
            })
            .expect("a power-of-two resolution migrates"),
        );
        world.insert_resource(store);
        let entity = world
            .spawn(jackdaw_scene_types::Terrain {
                cell_size: 1.0,
                data_path: DATA_PATH.to_string(),
                quantization,
                ..default()
            })
            .id();
        (world, entity)
    }

    /// Pinning a terrain to a metric spacing changes its cell size, the one
    /// thing a terrain declares about its grid. The component's rectangle
    /// is a load-only inlet: writing one back would put a fabricated shape
    /// in the scene text for a later load to believe.
    #[test]
    fn apply_pins_the_cell_size_and_writes_no_rectangle() {
        let (mut world, entity) = world_with_terrain(TerrainQuantization {
            enabled: true,
            cell_size: 2.0,
            height_step: 0.25,
        });
        let inlets = {
            let terrain = world
                .get::<jackdaw_scene_types::Terrain>(entity)
                .expect("the fixture spawns a terrain");
            (terrain.size, terrain.resolution)
        };

        apply_quantization(&mut world, entity);

        let terrain = world
            .get::<jackdaw_scene_types::Terrain>(entity)
            .expect("the terrain survives the apply");
        assert_eq!(terrain.cell_size, 2.0);
        assert_eq!((terrain.size, terrain.resolution), inlets);
        assert_eq!(
            world
                .resource::<TerrainDataStore>()
                .grid(DATA_PATH)
                .map(|grid| grid.cell_size),
            Some(2.0),
            "the document carries the spacing too, so a half-written save still draws",
        );
    }

    /// One undo puts back both halves, the spacing and the snapped
    /// heights.
    #[test]
    fn undoing_an_apply_restores_the_cell_size_it_replaced() {
        let (mut world, entity) = world_with_terrain(TerrainQuantization {
            enabled: true,
            cell_size: 2.0,
            height_step: 0.25,
        });

        apply_quantization(&mut world, entity);
        world.resource_scope(|world, mut history: Mut<CommandHistory>| {
            history.undo(world);
        });

        assert_eq!(
            world
                .get::<jackdaw_scene_types::Terrain>(entity)
                .expect("the terrain survives the undo")
                .cell_size,
            1.0,
        );
    }
}
