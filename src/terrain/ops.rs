//! Operators for the terrain tool palette, contextual options bar, and
//! Terrain panel's Generation section.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;

use super::paint::TerrainToolPaintOp;
use super::panel::TerrainGenerateState;
use super::sculpt::SetTerrainHeights;
use super::{TerrainDataStore, TerrainDirtyChunks, TerrainEditMode, TerrainPaintState};
use crate::commands::CommandHistory;
use crate::core_extension::CoreExtensionInputContext;
use crate::selection::Selection;

/// Regions per axis a generate lays down on a terrain that holds none.
///
/// Four is a kilometre a side at the default cell size. Nothing caps a
/// terrain at it.
const FRESH_TERRAIN_REGIONS: u32 = 4;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TerrainToolRaiseOp>()
        .register_operator::<TerrainToolLowerOp>()
        .register_operator::<TerrainToolFlattenOp>()
        .register_operator::<TerrainToolSmoothOp>()
        .register_operator::<TerrainToolNoiseOp>()
        .register_operator::<TerrainToolQuantizeOp>()
        .register_operator::<TerrainToolNavmeshOp>()
        .register_operator::<TerrainToolExitToSelectOp>()
        .register_operator::<TerrainGenerateOp>()
        .register_operator::<TerrainErodeOp>();

    // Palette shortcuts, Alt+1-9 down the palette's left-to-right,
    // top-to-bottom order (Raise, Lower, Flatten, Smooth, Noise, Paint,
    // Quantize, Navmesh, Regions; the last is bound in `regions.rs` beside
    // its operator). Plain Digit1-4 dispatch mesh edit-mode switches on
    // this input context (`edit_mode_ops.rs`), so the terrain palette takes
    // the Alt chord rather than colliding with them.
    ctx.bind_operator::<CoreExtensionInputContext, TerrainToolRaiseOp>([PresetInput::key(
        "Digit1",
    )
    .alt()]);
    ctx.bind_operator::<CoreExtensionInputContext, TerrainToolLowerOp>([PresetInput::key(
        "Digit2",
    )
    .alt()]);
    ctx.bind_operator::<CoreExtensionInputContext, TerrainToolFlattenOp>([PresetInput::key(
        "Digit3",
    )
    .alt()]);
    ctx.bind_operator::<CoreExtensionInputContext, TerrainToolSmoothOp>([PresetInput::key(
        "Digit4",
    )
    .alt()]);
    ctx.bind_operator::<CoreExtensionInputContext, TerrainToolNoiseOp>([PresetInput::key(
        "Digit5",
    )
    .alt()]);
    ctx.bind_operator::<CoreExtensionInputContext, TerrainToolPaintOp>([PresetInput::key(
        "Digit6",
    )
    .alt()]);
    ctx.bind_operator::<CoreExtensionInputContext, TerrainToolQuantizeOp>([PresetInput::key(
        "Digit7",
    )
    .alt()]);
    ctx.bind_operator::<CoreExtensionInputContext, TerrainToolNavmeshOp>([PresetInput::key(
        "Digit8",
    )
    .alt()]);
    // Esc leaves the active tool for no-tool. Mid-stroke the sculpt/paint
    // modal's cancel binding (Escape) takes precedence: this op's
    // `is_available` refuses to fire while a stroke is in progress.
    ctx.bind_operator::<CoreExtensionInputContext, TerrainToolExitToSelectOp>([PresetInput::key(
        "Escape",
    )]);
}

fn toggle_to(mode: &mut TerrainEditMode, target: TerrainEditMode) {
    *mode = if *mode == target {
        TerrainEditMode::None
    } else {
        target
    };
}

/// Tool-toggle ops require a terrain to be selected; without one the
/// palette and options bar that host these buttons are hidden.
pub(super) fn has_selected_terrain(
    selection: Res<Selection>,
    terrains: Query<(), With<jackdaw_scene_types::Terrain>>,
) -> bool {
    selection.primary().is_some_and(|e| terrains.contains(e))
}

/// Bound to Escape: leaves the active tool for no-tool, restoring
/// click-select and gizmo interaction in the viewport. Available only when
/// a tool is active and no stroke is running, so it does not fight the
/// modal's stroke-cancel handling. There is no palette button for it: the
/// main toolbar's Select tool and pressing an active tool's button again
/// (see `toggle_to`) both leave a terrain tool.
#[operator(
    id = "terrain.tool.exit_to_select",
    label = "Exit to Select",
    description = "Stop editing and select entities in the viewport.",
    is_available = can_exit_to_select,
    allows_undo = false
)]
pub(crate) fn terrain_tool_exit_to_select(
    _: In<OperatorParameters>,
    mut mode: ResMut<TerrainEditMode>,
) -> OperatorResult {
    *mode = TerrainEditMode::None;
    OperatorResult::Finished
}

/// Escape cancels an in-progress stroke rather than switching tools, so
/// this refuses while `TerrainSculptState` or `TerrainPaintState` reports
/// one running.
fn can_exit_to_select(
    selection: Res<Selection>,
    terrains: Query<(), With<jackdaw_scene_types::Terrain>>,
    mode: Res<TerrainEditMode>,
    sculpt: Res<super::TerrainSculptState>,
    paint: Res<TerrainPaintState>,
) -> bool {
    selection.primary().is_some_and(|e| terrains.contains(e))
        && *mode != TerrainEditMode::None
        && !sculpt.active
        && !paint.active
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

/// Pick the Quantize tool, which brings up the quantization options bar
/// (cell size, height step, on/off, Apply). Pressing again puts it away.
#[operator(
    id = "terrain.tool.quantize",
    label = "Quantize",
    description = "Show the terrain's grid-quantization settings.",
    is_available = has_selected_terrain
)]
pub(crate) fn terrain_tool_quantize(
    _: In<OperatorParameters>,
    mut mode: ResMut<TerrainEditMode>,
) -> OperatorResult {
    toggle_to(&mut mode, TerrainEditMode::Quantize);
    OperatorResult::Finished
}

/// Pick the Navmesh tool, which shows the bake params, the Bake action and
/// the overlay toggle in the options bar. Claims no viewport input.
#[operator(
    id = "terrain.tool.navmesh",
    label = "Navmesh",
    description = "Bake a navigation mesh from this terrain.",
    is_available = has_selected_terrain
)]
pub(crate) fn terrain_tool_navmesh(
    _: In<OperatorParameters>,
    mut mode: ResMut<TerrainEditMode>,
) -> OperatorResult {
    toggle_to(&mut mode, TerrainEditMode::Navmesh);
    OperatorResult::Finished
}

/// Generate a fresh heightmap for the selected terrain.
///
/// Reads the noise and octave settings from the Terrain panel's Generation
/// section ([`TerrainGenerateState`]).
///
/// `allows_undo` stays at its default of `true`: this op pushes its own
/// [`SetTerrainHeights`] entry, and that entry touches only heights, which
/// live outside the AST, so the framework's snapshot diff sees no change
/// and records no duplicate.
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
    mut commands: Commands,
) -> OperatorResult {
    let entity = selection.primary()?;
    let (terrain, mut dirty) = terrains.get_mut(entity)?;

    let mut data = store.entry_for(terrain)?;
    // An unsculpted terrain holds no regions and so has nothing to
    // generate over. A generate lays down a footprint of four regions a
    // side, a kilometre of ground at the default cell size; sculpting past
    // it allocates more.
    let mut refused = None;
    if data.document().grid_resolution() == 0
        && let Err(err) =
            data.ensure_extent(FRESH_TERRAIN_REGIONS * jackdaw_terrain::RegionSize::DEFAULT.get())
    {
        refused = Some(err.to_string());
    }
    if let Some(message) = refused {
        warn!("{message}");
        commands.queue(move |world: &mut World| {
            crate::terrain::toast_terrain_notice(world, &message);
        });
        return OperatorResult::Cancelled;
    }
    let resolution = data.document().grid_resolution();

    let mut new_heights = jackdaw_terrain::generate_heightmap(resolution, &gen_state.settings);
    // Snap before the array is stored or recorded, so the terrain is never
    // briefly off-lattice and undo restores no unsnapped intermediate.
    if let Some(step) = terrain.quantization.active_height_step() {
        jackdaw_terrain::quantize_heights(&mut new_heights, step);
    }
    let old_heights = data.heights().to_vec();
    data.set_heights(&new_heights);
    dirty.rebuild_all = true;
    history.push_executed(Box::new(SetTerrainHeights::whole(
        entity,
        old_heights,
        new_heights,
        "Generate Terrain".to_string(),
    )));
    OperatorResult::Finished
}

/// Apply hydraulic erosion to the selected terrain.
///
/// Uses the erosion settings from the Terrain panel's Generation section
/// ([`TerrainGenerateState::erosion`]).
///
/// `allows_undo` stays at its default of `true`: this op pushes its own
/// [`SetTerrainHeights`] entry, and that entry touches only heights, which
/// live outside the AST, so the framework's snapshot diff sees no change
/// and records no duplicate.
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
    let step = terrain.quantization.active_height_step();
    let mut data = store.entry_for(terrain)?;
    // Erosion strides the document's heights, so this is the grid those
    // heights are stored on.
    let resolution = data.document().grid_resolution();

    let old_heights = data.heights().to_vec();
    let mut new_heights = old_heights.clone();
    jackdaw_terrain::hydraulic_erosion(&mut new_heights, resolution, &gen_state.erosion);
    // Erosion moves sediment in continuous amounts, so a quantized terrain
    // is re-snapped whole; one pass would otherwise take every cell off the
    // lattice.
    if let Some(step) = step {
        jackdaw_terrain::quantize_heights(&mut new_heights, step);
    }
    data.set_heights(&new_heights);
    dirty.rebuild_all = true;
    history.push_executed(Box::new(SetTerrainHeights::whole(
        entity,
        old_heights,
        new_heights,
        "Erode Terrain".to_string(),
    )));
    OperatorResult::Finished
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plain Digit1-4 are bound to mesh edit-mode switches on
    /// `CoreExtensionInputContext` (`edit_mode_ops.rs`), so the terrain
    /// palette does not reuse them.
    #[test]
    fn palette_keybinds_use_alt_not_plain_digits() {
        let alt_binding = PresetInput::key("Digit1").alt();
        let plain_binding = PresetInput::key("Digit1");
        assert_ne!(alt_binding, plain_binding);
    }

    /// `can_exit_to_select` keeps `terrain.tool.exit_to_select` from firing
    /// while a stroke runs, so Escape stays the sculpt/paint modal's
    /// mid-stroke cancel.
    #[test]
    fn exit_to_select_is_unavailable_mid_stroke() {
        let mut world = World::new();
        world.init_resource::<Selection>();
        world.init_resource::<TerrainEditMode>();
        world.init_resource::<super::super::TerrainSculptState>();
        world.init_resource::<TerrainPaintState>();

        let terrain = world.spawn(jackdaw_scene_types::Terrain::default()).id();
        world.resource_mut::<Selection>().entities = vec![terrain];
        *world.resource_mut::<TerrainEditMode>() =
            TerrainEditMode::Sculpt(jackdaw_terrain::SculptTool::Raise);

        assert!(
            world
                .run_system_cached(can_exit_to_select)
                .expect("system runs"),
            "available once a tool is active and nothing is mid-stroke",
        );

        world
            .resource_mut::<super::super::TerrainSculptState>()
            .active = true;
        assert!(
            !world
                .run_system_cached(can_exit_to_select)
                .expect("system runs"),
            "must be unavailable while a stroke is in progress",
        );
    }

    /// An unsculpted terrain holds no regions, so a generate lays ground
    /// down rather than running over nothing.
    #[test]
    fn generating_on_a_fresh_terrain_lays_down_ground() {
        let mut store = TerrainDataStore::default();
        let terrain = jackdaw_scene_types::Terrain {
            data_path: "zone.terrain-0.jdterrain".to_string(),
            ..default()
        };
        assert_eq!(store.grid_shape(&terrain).resolution, 0, "nothing yet");

        let side = jackdaw_terrain::RegionSize::DEFAULT.get();
        store
            .entry_for(&terrain)
            .expect("keyed")
            .ensure_extent(FRESH_TERRAIN_REGIONS * side)
            .expect("the fresh footprint is far inside the cap");

        let shape = store.grid_shape(&terrain);
        assert_eq!(shape.resolution, FRESH_TERRAIN_REGIONS * side);
        assert_eq!(
            store
                .get(&terrain.data_path)
                .expect("keyed")
                .regions
                .region_count(),
            (FRESH_TERRAIN_REGIONS * FRESH_TERRAIN_REGIONS) as usize,
        );
        // A kilometre a side at the default cell size.
        assert_eq!(shape.size.x, (shape.resolution - 1) as f32);
    }
}
