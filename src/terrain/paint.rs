//! Painting per-cell integer values into a terrain's channels.
//!
//! Mirrors [`super::sculpt`] exactly: a modal operator driven by LMB, one
//! history entry pushed on release, Escape restoring the pre-stroke
//! snapshot, and Shift+scroll resizing the same brush. Sculpt and paint
//! share one brush idiom because that is what the tools people arrive
//! from do -- Unity, Unreal and `Terrain3D` all use one set of brush
//! controls across their terrain modes.
//!
//! The values are integers, so the brush is a threshold stamp rather than
//! an accumulation: a cell inside the falloff is written, a cell outside
//! is left alone. Holding Ctrl paints 0 -- the erase gesture every one of
//! those tools has and jackdaw did not.

use bevy::prelude::*;
use jackdaw_api::prelude::*;

use super::{
    CHUNK_SIZE, TerrainBrushSettings, TerrainDataStore, TerrainDirtyChunks, TerrainEditMode,
};
use crate::commands::{CommandHistory, EditorCommand};
use crate::default_style;
use crate::selection::Selection;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<TerrainPaintState>().add_systems(
        Update,
        (
            update_paint_brush_position,
            paint_invoke_trigger,
            handle_paint_resize_scroll,
            draw_paint_brush_gizmo,
        )
            .chain()
            .run_if(in_state(crate::AppState::Editor)),
    );
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TerrainPaintOp>();
    ctx.register_operator::<TerrainToolPaintOp>();
}

/// Which channel and value the paint brush writes, and whether the
/// viewport is showing what has been painted.
#[derive(Resource)]
pub struct TerrainPaintState {
    /// Index into the terrain's channel table.
    pub active_channel: usize,
    /// Index into that channel's palette.
    pub active_entry: usize,
    /// Tint the terrain by the active channel.
    ///
    /// `Terrain3D` calls this a control-texture debug view and it is the
    /// single most borrowable thing in the reference set: without it a
    /// user is painting invisible data.
    ///
    /// On by default. A terrain with no channels is unaffected, and for
    /// one that has them the alternative is opening a painted scene,
    /// seeing flat grey, and concluding nothing was ever painted.
    pub show_channel: bool,
    /// The terrain the brush is over.
    pub target: Option<Entity>,
    /// Whether a stroke is in progress (LMB held).
    pub active: bool,
    /// Values of the painted channel at stroke start, for undo.
    pub stroke_snapshot: Vec<u16>,
    /// Which channel the in-progress stroke is writing.
    pub stroke_channel: usize,
    /// Brush position in grid space.
    pub brush_position: Option<Vec2>,
}

impl Default for TerrainPaintState {
    fn default() -> Self {
        Self {
            active_channel: 0,
            active_entry: 0,
            show_channel: true,
            target: None,
            active: false,
            stroke_snapshot: Vec::new(),
            stroke_channel: 0,
            brush_position: None,
        }
    }
}

impl TerrainPaintState {
    /// The value the brush writes for a terrain, honouring the erase
    /// modifier. `None` when the terrain declares no usable channel.
    pub fn value_for(&self, terrain: &jackdaw_scene_types::Terrain, erase: bool) -> Option<u16> {
        let channel = terrain.channels.get(self.active_channel)?;
        if erase {
            return Some(0);
        }
        Some(
            channel
                .palette
                .get(self.active_entry)
                .map(|entry| entry.value)
                .unwrap_or(0),
        )
    }
}

/// Undo command for a paint stroke.
///
/// Deliberately does not touch the scene document. Channel values are
/// bulk per-cell data and live in the sidecar store, the same as heights;
/// pushing them through the BSN AST is the defect the sidecar fixes.
pub struct SetTerrainChannel {
    pub entity: Entity,
    pub channel: usize,
    pub old_values: Vec<u16>,
    pub new_values: Vec<u16>,
    pub label: String,
}

impl SetTerrainChannel {
    fn apply(&self, world: &mut World, values: &[u16]) {
        let Some(terrain) = world.get::<jackdaw_scene_types::Terrain>(self.entity) else {
            return;
        };
        let terrain = terrain.clone();
        if let Some(data) = world.resource_mut::<TerrainDataStore>().entry_for(&terrain)
            && let Some(channel) = data.channels.get_mut(self.channel)
        {
            channel.values = values.to_vec();
            channel.resize_to(terrain.resolution);
        }
        if let Some(mut dirty) = world.get_mut::<TerrainDirtyChunks>(self.entity) {
            dirty.rebuild_all = true;
        }
    }
}

impl EditorCommand for SetTerrainChannel {
    fn execute(&mut self, world: &mut World) {
        self.apply(world, &self.new_values.clone());
    }

    fn undo(&mut self, world: &mut World) {
        self.apply(world, &self.old_values.clone());
    }

    fn description(&self) -> &str {
        &self.label
    }
}

/// Pick the paint tool. Pressing again puts the brush away.
#[operator(
    id = "terrain.tool.paint",
    label = "Paint",
    description = "Paint the active channel's selected value onto the terrain.",
    is_available = super::ops::has_selected_terrain
)]
pub(crate) fn terrain_tool_paint(
    _: In<OperatorParameters>,
    mut mode: ResMut<TerrainEditMode>,
) -> OperatorResult {
    *mode = if *mode == TerrainEditMode::Paint {
        TerrainEditMode::None
    } else {
        TerrainEditMode::Paint
    };
    OperatorResult::Finished
}

/// Track the brush target so the ring gizmo follows the cursor even
/// between strokes.
fn update_paint_brush_position(
    edit_mode: Res<TerrainEditMode>,
    vp: crate::viewport::ViewportCursor,
    terrain_query: Query<(Entity, &jackdaw_scene_types::Terrain, &GlobalTransform)>,
    selection: Res<Selection>,
    mut paint_state: ResMut<TerrainPaintState>,
) {
    if *edit_mode != TerrainEditMode::Paint {
        if paint_state.brush_position.is_some() || paint_state.target.is_some() {
            paint_state.brush_position = None;
            paint_state.target = None;
        }
        return;
    }
    match super::sculpt::terrain_brush_hit(&vp, &terrain_query, &selection) {
        Some((entity, grid)) => {
            paint_state.target = Some(entity);
            paint_state.brush_position = Some(grid);
        }
        None => paint_state.brush_position = None,
    }
}

/// LMB in paint mode dispatches `terrain.paint`. Mouse-button gestures
/// are not expressible as BEI key bindings.
fn paint_invoke_trigger(
    mouse: Res<ButtonInput<MouseButton>>,
    edit_mode: Res<TerrainEditMode>,
    paint_state: Res<TerrainPaintState>,
    mut commands: Commands,
) {
    if paint_state.active
        || !mouse.just_pressed(MouseButton::Left)
        || *edit_mode != TerrainEditMode::Paint
        || paint_state.brush_position.is_none()
        || paint_state.target.is_none()
    {
        return;
    }
    commands.queue(|world: &mut World| {
        let _ = world
            .operator(TerrainPaintOp::ID)
            .settings(CallOperatorSettings {
                execution_context: ExecutionContext::Invoke,
                creates_history_entry: false,
            })
            .call();
    });
}

#[operator(
    id = "terrain.paint",
    label = "Paint Terrain",
    description = "Paint the active channel under the brush, or erase it while holding Ctrl.",
    modal = true,
    allows_undo = false,
    cancel = cancel_terrain_paint,
)]
pub fn terrain_paint(
    _: In<OperatorParameters>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    edit_mode: Res<TerrainEditMode>,
    brush_settings: Res<TerrainBrushSettings>,
    mut paint_state: ResMut<TerrainPaintState>,
    terrain_query: Query<(&jackdaw_scene_types::Terrain, &mut TerrainDirtyChunks)>,
    mut store: ResMut<TerrainDataStore>,
    mut history: ResMut<CommandHistory>,
    active: ActiveModalQuery,
) -> OperatorResult {
    if *edit_mode != TerrainEditMode::Paint {
        return OperatorResult::Cancelled;
    }
    let mut terrain_query = terrain_query;
    let target = paint_state.target?;
    let (terrain, mut dirty) = terrain_query.get_mut(target)?;
    let terrain = terrain.clone();

    let erase = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let value = paint_state.value_for(&terrain, erase)?;
    let channel_index = if active.is_modal_running() {
        paint_state.stroke_channel
    } else {
        paint_state.active_channel
    };

    let data = store.entry_for(&terrain)?;
    let channel = data.channels.get_mut(channel_index)?;

    if !active.is_modal_running() {
        paint_state.active = true;
        paint_state.stroke_channel = channel_index;
        paint_state.stroke_snapshot = channel.values.clone();
    }

    // See `super::stroke_should_end` doc: checked every frame, including
    // the modal's first, not gated behind `else`/`modal.is_some()`.
    if super::stroke_should_end(&mouse) {
        paint_state.active = false;
        let old_values = std::mem::take(&mut paint_state.stroke_snapshot);
        // A stroke that changed nothing (repainting a value onto itself)
        // must not leave an empty entry cluttering the undo stack.
        if old_values != channel.values {
            history.push_executed(Box::new(SetTerrainChannel {
                entity: target,
                channel: channel_index,
                old_values,
                new_values: channel.values.clone(),
                label: format!("Paint {}", channel.name),
            }));
        }
        return OperatorResult::Finished;
    }

    if let Some(grid_pos) = paint_state.brush_position {
        let changed = jackdaw_terrain::apply_channel_brush(
            &mut channel.values,
            terrain.resolution,
            channel.element,
            grid_pos,
            brush_settings.radius,
            brush_settings.falloff,
            PAINT_THRESHOLD,
            value,
        );
        if changed > 0 {
            // Only the chunks the stroke touched, never `rebuild_all`.
            for chunk in jackdaw_terrain::affected_chunks_at(
                terrain.resolution,
                grid_pos,
                brush_settings.radius,
                CHUNK_SIZE,
            ) {
                dirty.dirty.insert(chunk);
            }
        }
    }
    OperatorResult::Running
}

/// Falloff a cell must clear to be written.
///
/// Integer channels cannot blend, so a soft brush edge has to become a
/// hard decision somewhere. Half-strength is the natural place: the
/// painted disc matches the visible ring's half-intensity contour, which
/// is what a user reads the ring as meaning.
const PAINT_THRESHOLD: f32 = 0.5;

fn cancel_terrain_paint(
    mut paint_state: ResMut<TerrainPaintState>,
    terrain_query: Query<(&jackdaw_scene_types::Terrain, &mut TerrainDirtyChunks)>,
    mut store: ResMut<TerrainDataStore>,
) {
    if !paint_state.active {
        return;
    }
    paint_state.active = false;
    let snapshot = std::mem::take(&mut paint_state.stroke_snapshot);
    let channel_index = paint_state.stroke_channel;
    let mut terrain_query = terrain_query;
    if let Some(target) = paint_state.target
        && let Ok((terrain, mut dirty)) = terrain_query.get_mut(target)
    {
        let terrain = terrain.clone();
        if let Some(data) = store.entry_for(&terrain)
            && let Some(channel) = data.channels.get_mut(channel_index)
        {
            channel.values = snapshot;
            channel.resize_to(terrain.resolution);
        }
        dirty.rebuild_all = true;
    }
}

/// Shift+scroll resizes the paint brush exactly as it resizes the sculpt
/// brush, so the gesture does not change meaning between modes.
fn handle_paint_resize_scroll(
    keyboard: Res<ButtonInput<KeyCode>>,
    nav_scroll: crate::modal_inputs::NavScrollInputs,
    edit_mode: Res<TerrainEditMode>,
    mut brush_settings: ResMut<TerrainBrushSettings>,
) {
    if *edit_mode != TerrainEditMode::Paint
        || !keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight])
    {
        return;
    }
    if nav_scroll.resize_up() {
        brush_settings.radius = f32::min(brush_settings.radius * 1.15, 50.0);
    } else if nav_scroll.resize_down() {
        brush_settings.radius = f32::max(brush_settings.radius * 0.87, 1.0);
    }
}

/// Ring gizmo following the terrain surface, in the active palette
/// colour so the user can see what they are about to lay down.
fn draw_paint_brush_gizmo(
    paint_state: Res<TerrainPaintState>,
    brush_settings: Res<TerrainBrushSettings>,
    edit_mode: Res<TerrainEditMode>,
    keyboard: Res<ButtonInput<KeyCode>>,
    terrains: Query<(&jackdaw_scene_types::Terrain, &GlobalTransform)>,
    store: Res<TerrainDataStore>,
    mut gizmos: Gizmos,
) {
    if *edit_mode != TerrainEditMode::Paint {
        return;
    }
    let (Some(target), Some(grid_pos)) = (paint_state.target, paint_state.brush_position) else {
        return;
    };
    let Ok((terrain, terrain_tf)) = terrains.get(target) else {
        return;
    };

    let erase = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let color = if erase {
        default_style::TERRAIN_SCULPT_GIZMO
    } else {
        terrain
            .channels
            .get(paint_state.active_channel)
            .and_then(|channel| channel.palette.get(paint_state.active_entry))
            .map(|entry| entry.color)
            .unwrap_or(default_style::TERRAIN_SCULPT_GIZMO)
    };

    let heightmap = super::mesh::heightmap_from_terrain(terrain, &store);
    let segments = 32;
    let radius = brush_settings.radius;
    let origin = terrain_tf.translation();
    let cell = heightmap.cell_size();
    let half = terrain.size / 2.0;

    for i in 0..segments {
        let a0 = (i as f32 / segments as f32) * std::f32::consts::TAU;
        let a1 = ((i + 1) as f32 / segments as f32) * std::f32::consts::TAU;

        let gx0 = grid_pos.x + a0.cos() * radius;
        let gz0 = grid_pos.y + a0.sin() * radius;
        let gx1 = grid_pos.x + a1.cos() * radius;
        let gz1 = grid_pos.y + a1.sin() * radius;

        let h0 = heightmap.sample_bilinear(gx0, gz0);
        let h1 = heightmap.sample_bilinear(gx1, gz1);

        let p0 = origin + Vec3::new(gx0 * cell.x - half.x, h0 + 0.1, gz0 * cell.y - half.y);
        let p1 = origin + Vec3::new(gx1 * cell.x - half.x, h1 + 0.1, gz1 * cell.y - half.y);
        gizmos.line(p0, p1, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jackdaw_scene_types::{TerrainChannel, TerrainChannelElement, TerrainPaletteEntry};

    fn two_entry_terrain() -> jackdaw_scene_types::Terrain {
        jackdaw_scene_types::Terrain {
            channels: vec![TerrainChannel {
                name: "biome".to_string(),
                element: TerrainChannelElement::U8,
                palette: vec![
                    TerrainPaletteEntry {
                        value: 0,
                        label: "unset".to_string(),
                        color: Color::BLACK,
                    },
                    TerrainPaletteEntry {
                        value: 7,
                        label: "marsh".to_string(),
                        color: Color::WHITE,
                    },
                ],
            }],
            ..default()
        }
    }

    #[test]
    fn the_brush_writes_the_selected_palette_entrys_value() {
        let state = TerrainPaintState {
            active_entry: 1,
            ..default()
        };
        assert_eq!(state.value_for(&two_entry_terrain(), false), Some(7));
    }

    /// Ctrl-held erases. Every reference tool binds Ctrl to the inverse
    /// of the current brush; for an integer channel the inverse of
    /// "write this value" is "write nothing here".
    #[test]
    fn ctrl_erases_regardless_of_the_selected_entry() {
        let state = TerrainPaintState {
            active_entry: 1,
            ..default()
        };
        assert_eq!(state.value_for(&two_entry_terrain(), true), Some(0));
    }

    #[test]
    fn a_terrain_with_no_channels_has_nothing_to_paint() {
        let state = TerrainPaintState::default();
        let bare = jackdaw_scene_types::Terrain::default();
        assert_eq!(state.value_for(&bare, false), None);
    }

    /// An out-of-range palette index reads as 0 rather than panicking:
    /// removing the selected entry from the palette must not take the
    /// editor down on the next brush move.
    #[test]
    fn a_stale_palette_selection_falls_back_to_zero() {
        let state = TerrainPaintState {
            active_entry: 99,
            ..default()
        };
        assert_eq!(state.value_for(&two_entry_terrain(), false), Some(0));
    }
}
