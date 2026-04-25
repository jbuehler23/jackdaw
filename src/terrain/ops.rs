//! Operators for the terrain contextual toolbar and inspector.
//!
//! Six tool-toggle ops flip `TerrainEditMode` between `Sculpt(tool)` /
//! `Generate` and `None`; `terrain.generate` and `terrain.erode`
//! apply the corresponding heightmap transform and push a single
//! [`SetTerrainHeights`] history entry.

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

#[operator(
    id = "terrain.tool.raise",
    label = "Raise",
    description = "Activate the raise sculpt tool, or deactivate if already active.",
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

#[operator(
    id = "terrain.tool.lower",
    label = "Lower",
    description = "Activate the lower sculpt tool, or deactivate if already active.",
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

#[operator(
    id = "terrain.tool.flatten",
    label = "Flatten",
    description = "Activate the flatten sculpt tool, or deactivate if already active.",
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

#[operator(
    id = "terrain.tool.smooth",
    label = "Smooth",
    description = "Activate the smooth sculpt tool, or deactivate if already active.",
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

#[operator(
    id = "terrain.tool.noise",
    label = "Noise",
    description = "Activate the noise sculpt tool, or deactivate if already active.",
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

#[operator(
    id = "terrain.tool.generate",
    label = "Generate",
    description = "Activate the generate-heightmap mode, or deactivate if already active.",
    is_available = has_selected_terrain
)]
pub(crate) fn terrain_tool_generate(
    _: In<OperatorParameters>,
    mut mode: ResMut<TerrainEditMode>,
) -> OperatorResult {
    toggle_to(&mut mode, TerrainEditMode::Generate);
    OperatorResult::Finished
}

#[operator(
    id = "terrain.generate",
    label = "Generate Terrain",
    description = "Replace the selected terrain's heights with a generated heightmap from \
                   `TerrainGenerateState.settings`. Pushes a single `SetTerrainHeights` \
                   history entry.",
    is_available = has_selected_terrain,
    allows_undo = false
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

#[operator(
    id = "terrain.erode",
    label = "Erode Terrain",
    description = "Run hydraulic erosion in-place on the selected terrain's heights and push \
                   a `SetTerrainHeights` history entry.",
    is_available = has_selected_terrain,
    allows_undo = false
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
