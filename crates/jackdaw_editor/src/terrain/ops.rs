//! Operators for the terrain contextual toolbar and inspector.

use bevy::prelude::*;
use jackdaw_api::prelude::*;

use super::inspector::TerrainGenerateState;
use super::sculpt::SetTerrainHeights;
use super::{TerrainDirtyChunks, TerrainEditMode};
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
fn has_selected_terrain(
    selection: Res<Selection>,
    terrains: Query<(), With<jackdaw_jsn::Terrain>>,
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
/// `allows_undo = false` because this op pushes its own
/// [`SetTerrainHeights`] history entry; letting the framework also
/// capture a diff would double-record the change.
#[operator(
    id = "terrain.generate",
    label = "Generate Terrain",
    description = "Generate a fresh heightmap for the selected terrain.",
    is_available = has_selected_terrain
)]
pub(crate) fn terrain_generate(
    _: In<OperatorParameters>,
    selection: Res<Selection>,
    mut terrains: Query<(&mut jackdaw_jsn::Terrain, &mut TerrainDirtyChunks)>,
    gen_state: Res<TerrainGenerateState>,
    mut history: ResMut<CommandHistory>,
) -> OperatorResult {
    let Some(entity) = selection.primary() else {
        return OperatorResult::Cancelled;
    };
    let Ok((mut terrain, mut dirty)) = terrains.get_mut(entity) else {
        return OperatorResult::Cancelled;
    };

    let old_heights = terrain.heights.clone();
    let new_heights = jackdaw_terrain::generate_heightmap(terrain.resolution, &gen_state.settings);
    terrain.heights = new_heights.clone();
    dirty.rebuild_all = true;
    history.push_executed(Box::new(SetTerrainHeights {
        entity,
        old_heights,
        new_heights,
        label: "Generate Terrain".to_string(),
    }));
    OperatorResult::Finished
}

/// Apply hydraulic erosion to the selected terrain.
///
/// Uses the erosion settings from the inspector's generation panel
/// ([`TerrainGenerateState::erosion`]).
///
/// `allows_undo = false` because this op pushes its own
/// [`SetTerrainHeights`] history entry; letting the framework also
/// capture a diff would double-record the change.
#[operator(
    id = "terrain.erode",
    label = "Erode Terrain",
    description = "Apply hydraulic erosion to the selected terrain.",
    is_available = has_selected_terrain
)]
pub(crate) fn terrain_erode(
    _: In<OperatorParameters>,
    selection: Res<Selection>,
    mut terrains: Query<(&mut jackdaw_jsn::Terrain, &mut TerrainDirtyChunks)>,
    gen_state: Res<TerrainGenerateState>,
    mut history: ResMut<CommandHistory>,
) -> OperatorResult {
    let Some(entity) = selection.primary() else {
        return OperatorResult::Cancelled;
    };
    let Ok((mut terrain, mut dirty)) = terrains.get_mut(entity) else {
        return OperatorResult::Cancelled;
    };

    let old_heights = terrain.heights.clone();
    let mut new_heights = terrain.heights.clone();
    jackdaw_terrain::hydraulic_erosion(&mut new_heights, terrain.resolution, &gen_state.erosion);
    terrain.heights = new_heights.clone();
    dirty.rebuild_all = true;
    history.push_executed(Box::new(SetTerrainHeights {
        entity,
        old_heights,
        new_heights,
        label: "Erode Terrain".to_string(),
    }));
    OperatorResult::Finished
}
