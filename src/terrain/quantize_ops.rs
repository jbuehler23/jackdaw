//! Operators for a terrain's grid quantization.
//!
//! Quantization is a three-field descriptor on the `Terrain` component --
//! small, human-edited, inline in the scene document -- so every mutation
//! here syncs the component back to the AST. What it *governs* is bulk
//! data, and that still travels the sidecar route: the apply operator
//! hands the snapped array to [`SetTerrainHeights`] and never puts a
//! height near the document.
//!
//! Turning quantization off is deliberately inert. It changes how the
//! terrain is meshed and stops future strokes snapping; it does not
//! un-snap anything already stored. A user who terraced a hillside and
//! then switched the setting off still has the hillside they made.

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
/// The inspector's numeric fields and the on/off toggle both come
/// through here so there is one place that knows a descriptor edit has
/// to reach the AST, and that flipping `enabled` changes which mesher
/// runs and therefore needs a full rebuild.
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
/// Sculpting, generating and eroding all snap as they go once
/// quantization is on, so this operator is for the terrain that already
/// existed when the setting was turned on, or whose step has since
/// changed.
///
/// `allows_undo = false` because it pushes its own [`SetTerrainHeights`]
/// entry; letting the framework also capture a diff would double-record
/// the change.
#[operator(
    id = "terrain.quantize.apply",
    label = "Apply Quantization",
    description = "Snap every stored height to the elevation step and resize the \
                   terrain so its vertex spacing equals the cell size.",
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
/// The heights and the world size move together: pinning vertex spacing
/// to a metric cell size is a resize, and undoing the snapped heights
/// without the resize would leave the terrain at a spacing its heights
/// were never snapped for.
fn apply_quantization(world: &mut World, entity: Entity) {
    let Some(terrain) = world.get::<jackdaw_scene_types::Terrain>(entity) else {
        return;
    };
    let terrain = terrain.clone();
    if !terrain.quantization.enabled {
        return;
    }

    let old_size = terrain.size;
    let new_size = terrain
        .quantization
        .world_size_for(terrain.resolution)
        .unwrap_or(old_size);

    let Some(old_heights) = world
        .resource_mut::<TerrainDataStore>()
        .entry_for(&terrain)
        .map(|data| data.heights.clone())
    else {
        return;
    };
    let mut new_heights = old_heights.clone();
    if let Some(step) = terrain.quantization.active_height_step() {
        jackdaw_terrain::quantize_heights(&mut new_heights, step);
    }

    // An already-conforming terrain should not leave an undo entry that
    // does nothing when the user presses Ctrl+Z.
    if new_heights == old_heights && new_size == old_size {
        return;
    }

    let command = SetTerrainHeights {
        entity,
        old_heights,
        new_heights,
        label: "Quantize Terrain".to_string(),
        resize: (new_size != old_size).then_some((old_size, new_size)),
    };
    world.resource_scope(|world, mut history: Mut<CommandHistory>| {
        history.execute(Box::new(command), world);
    });
}
