//! Operators for the terrain contextual toolbar and inspector.

use bevy::prelude::*;
use jackdaw_api::prelude::*;

use super::inspector::TerrainGenerateState;
use super::sculpt::SetTerrainHeights;
use super::{TerrainDataStore, TerrainDirtyChunks, TerrainEditMode};
use crate::commands::CommandHistory;
use crate::selection::Selection;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TerrainToolRaiseOp>()
        .register_operator::<TerrainToolLowerOp>()
        .register_operator::<TerrainToolFlattenOp>()
        .register_operator::<TerrainToolSmoothOp>()
        .register_operator::<TerrainToolNoiseOp>()
        .register_operator::<TerrainToolGenerateOp>()
        .register_operator::<TerrainGenerateOp>()
        .register_operator::<TerrainErodeOp>();
}

fn toggle_to(mode: &mut TerrainEditMode, target: TerrainEditMode) {
    *mode = if *mode == target {
        TerrainEditMode::None
    } else {
        target
    };
}

/// Tool-toggle ops require a terrain to be selected; otherwise the
/// toolbar that hosts these buttons is hidden anyway.
pub(super) fn has_selected_terrain(
    selection: Res<Selection>,
    terrains: Query<(), With<jackdaw_scene_types::Terrain>>,
) -> bool {
    selection.primary().is_some_and(|e| terrains.contains(e))
}

/// Pick the raise sculpt tool. Pressing again puts the brush away.
#[operator(
    id = "terrain.tool.raise",
    label = "Raise",
    description = "Pick the raise sculpt tool.",
    is_available = has_selected_terrain
)]
pub(crate) fn terrain_tool_raise(
    _: In<OperatorParameters>,
    mut mode: ResMut<TerrainEditMode>,
) -> OperatorResult {
    toggle_to(
        &mut mode,
        TerrainEditMode::Sculpt(jackdaw_terrain::SculptTool::Raise),
    );
    OperatorResult::Finished
}

/// Pick the lower sculpt tool. Pressing again puts the brush away.
#[operator(
    id = "terrain.tool.lower",
    label = "Lower",
    description = "Pick the lower sculpt tool.",
    is_available = has_selected_terrain
)]
pub(crate) fn terrain_tool_lower(
    _: In<OperatorParameters>,
    mut mode: ResMut<TerrainEditMode>,
) -> OperatorResult {
    toggle_to(
        &mut mode,
        TerrainEditMode::Sculpt(jackdaw_terrain::SculptTool::Lower),
    );
    OperatorResult::Finished
}

/// Pick the flatten sculpt tool. Pressing again puts the brush away.
#[operator(
    id = "terrain.tool.flatten",
    label = "Flatten",
    description = "Pick the flatten sculpt tool.",
    is_available = has_selected_terrain
)]
pub(crate) fn terrain_tool_flatten(
    _: In<OperatorParameters>,
    mut mode: ResMut<TerrainEditMode>,
) -> OperatorResult {
    toggle_to(
        &mut mode,
        TerrainEditMode::Sculpt(jackdaw_terrain::SculptTool::Flatten),
    );
    OperatorResult::Finished
}

/// Pick the smooth sculpt tool. Pressing again puts the brush away.
#[operator(
    id = "terrain.tool.smooth",
    label = "Smooth",
    description = "Pick the smooth sculpt tool.",
    is_available = has_selected_terrain
)]
pub(crate) fn terrain_tool_smooth(
    _: In<OperatorParameters>,
    mut mode: ResMut<TerrainEditMode>,
) -> OperatorResult {
    toggle_to(
        &mut mode,
        TerrainEditMode::Sculpt(jackdaw_terrain::SculptTool::Smooth),
    );
    OperatorResult::Finished
}

/// Pick the noise sculpt tool. Pressing again puts the brush away.
#[operator(
    id = "terrain.tool.noise",
    label = "Noise",
    description = "Pick the noise sculpt tool.",
    is_available = has_selected_terrain
)]
pub(crate) fn terrain_tool_noise(
    _: In<OperatorParameters>,
    mut mode: ResMut<TerrainEditMode>,
) -> OperatorResult {
    toggle_to(
        &mut mode,
        TerrainEditMode::Sculpt(jackdaw_terrain::SculptTool::Noise),
    );
    OperatorResult::Finished
}

/// Open the heightmap-generation panel. Pressing again closes it.
#[operator(
    id = "terrain.tool.generate",
    label = "Generate",
    description = "Open the heightmap-generation panel.",
    is_available = has_selected_terrain
)]
pub(crate) fn terrain_tool_generate(
    _: In<OperatorParameters>,
    mut mode: ResMut<TerrainEditMode>,
) -> OperatorResult {
    toggle_to(&mut mode, TerrainEditMode::Generate);
    OperatorResult::Finished
}

/// Generate a fresh heightmap for the selected terrain.
///
/// Reads the noise/octaves/etc. settings from the inspector's
/// generation panel ([`TerrainGenerateState`]).
///
/// `allows_undo` is left at its default (`true`), not set to `false`:
/// this op pushes its own [`SetTerrainHeights`] history entry, but that
/// entry only ever touches heights, which live outside the AST (see
/// `store.rs`'s module doc). The framework's automatic before/after
/// snapshot diff sees no change there and skips recording a duplicate,
/// per CONTRIBUTING's note on `push_executed`.
#[operator(
    id = "terrain.generate",
    label = "Generate Terrain",
    description = "Generate a fresh heightmap for the selected terrain.",
    is_available = has_selected_terrain
)]
pub(crate) fn terrain_generate(
    _: In<OperatorParameters>,
    selection: Res<Selection>,
    mut terrains: Query<(&jackdaw_scene_types::Terrain, &mut TerrainDirtyChunks)>,
    mut store: ResMut<TerrainDataStore>,
    gen_state: Res<TerrainGenerateState>,
    mut history: ResMut<CommandHistory>,
) -> OperatorResult {
    let entity = selection.primary()?;
    let (terrain, mut dirty) = terrains.get_mut(entity)?;

    let mut new_heights =
        jackdaw_terrain::generate_heightmap(terrain.resolution, &gen_state.settings);
    // Snap before the array is stored or recorded, so the terrain is
    // never briefly off-lattice and undo never restores an unsnapped
    // intermediate that was not on screen.
    if let Some(step) = terrain.quantization.active_height_step() {
        jackdaw_terrain::quantize_heights(&mut new_heights, step);
    }
    let data = store.entry_for(terrain)?;
    let old_heights = std::mem::replace(&mut data.heights, new_heights.clone());
    dirty.rebuild_all = true;
    history.push_executed(Box::new(SetTerrainHeights::new(
        entity,
        old_heights,
        new_heights,
        "Generate Terrain".to_string(),
    )));
    OperatorResult::Finished
}

/// Apply hydraulic erosion to the selected terrain.
///
/// Uses the erosion settings from the inspector's generation panel
/// ([`TerrainGenerateState::erosion`]).
///
/// `allows_undo` is left at its default (`true`), not set to `false`:
/// this op pushes its own [`SetTerrainHeights`] history entry, but that
/// entry only ever touches heights, which live outside the AST (see
/// `store.rs`'s module doc). The framework's automatic before/after
/// snapshot diff sees no change there and skips recording a duplicate,
/// per CONTRIBUTING's note on `push_executed`.
#[operator(
    id = "terrain.erode",
    label = "Erode Terrain",
    description = "Apply hydraulic erosion to the selected terrain.",
    is_available = has_selected_terrain
)]
pub(crate) fn terrain_erode(
    _: In<OperatorParameters>,
    selection: Res<Selection>,
    mut terrains: Query<(&jackdaw_scene_types::Terrain, &mut TerrainDirtyChunks)>,
    mut store: ResMut<TerrainDataStore>,
    gen_state: Res<TerrainGenerateState>,
    mut history: ResMut<CommandHistory>,
) -> OperatorResult {
    let entity = selection.primary()?;
    let (terrain, mut dirty) = terrains.get_mut(entity)?;
    let resolution = terrain.resolution;
    let step = terrain.quantization.active_height_step();
    let data = store.entry_for(terrain)?;

    let old_heights = data.heights.clone();
    let mut new_heights = data.heights.clone();
    jackdaw_terrain::hydraulic_erosion(&mut new_heights, resolution, &gen_state.erosion);
    // Erosion moves sediment in continuous amounts, so on a quantized
    // terrain it is re-snapped whole: without this a single erode pass
    // silently takes every cell off the lattice.
    if let Some(step) = step {
        jackdaw_terrain::quantize_heights(&mut new_heights, step);
    }
    data.heights = new_heights.clone();
    dirty.rebuild_all = true;
    history.push_executed(Box::new(SetTerrainHeights::new(
        entity,
        old_heights,
        new_heights,
        "Erode Terrain".to_string(),
    )));
    OperatorResult::Finished
}
