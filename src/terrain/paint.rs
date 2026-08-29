//! Painting per-cell data into a terrain: scatter-mask values (invisible
//! gameplay data) or splat control words (base/overlay texture ids and
//! blend).
//!
//! One modal operator drives both, switching on
//! [`TerrainPaintState::domain`] (`terrain.paint.target`,
//! `texture_ops.rs`): driven by LMB, one history entry pushed on release,
//! Escape restoring the pre-stroke snapshot, Shift+scroll resizing the
//! brush.
//!
//! [`PaintDomain::Channels`] is the "Scatter Masks" target: gameplay data
//! deciding where scatter places objects, which the terrain does not
//! render. Values are integers, so the brush is a threshold stamp rather
//! than an accumulation: a cell inside the falloff is written, a cell
//! outside is left alone. Holding Ctrl paints 0 (erase).
//!
//! [`PaintDomain::Textures`] control words blend continuously, so the brush
//! nudges each cell every frame it is held rather than stamping it
//! (`jackdaw_terrain::apply_control_brush`). Primary paints the base
//! texture id and lowers blend toward it; Ctrl paints the overlay id and
//! raises blend toward it rather than erasing.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_terrain::{Control, GridRect};

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

/// Which domain the Paint tool's brush writes to. The options bar's
/// target picker switches this.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaintDomain {
    #[default]
    Channels,
    Textures,
}

/// Which channel/value or texture/opacity the paint brush writes, and
/// whether the viewport is showing what has been painted.
#[derive(Resource)]
pub struct TerrainPaintState {
    /// Which domain the brush writes: channels or textures.
    pub domain: PaintDomain,
    /// Index into the terrain's channel table.
    pub active_channel: usize,
    /// Index into that channel's palette.
    pub active_entry: usize,
    /// Tint the terrain by the active channel. On by default; without it,
    /// painted data is invisible in the viewport.
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
    /// Texture id the brush paints in [`PaintDomain::Textures`], selected
    /// from the Textures tab's thumbnail grid.
    pub active_texture_id: u8,
    /// Blend range crossed per second at full brush strength, in
    /// [`PaintDomain::Textures`]. `0.0..=1.0`; scaled by falloff and frame
    /// `dt` at the call site (see `jackdaw_terrain::apply_control_brush`).
    pub texture_opacity: f32,
    /// Control words at stroke start, for undo, in [`PaintDomain::Textures`].
    pub stroke_control_snapshot: Vec<Control>,
    /// Whether the brush hands cells back to autoterrain instead of
    /// painting a texture into them. The paint bar's Restore Auto
    /// checkbox switches this.
    pub restore_auto: bool,
    /// What the in-progress stroke is doing, captured at its start, so
    /// toggling the checkbox mid-stroke does not turn half a drag into the
    /// other gesture.
    pub stroke_restores: bool,
}

impl Default for TerrainPaintState {
    fn default() -> Self {
        Self {
            domain: PaintDomain::default(),
            active_channel: 0,
            active_entry: 0,
            show_channel: true,
            target: None,
            active: false,
            stroke_snapshot: Vec::new(),
            stroke_channel: 0,
            brush_position: None,
            active_texture_id: 0,
            texture_opacity: 0.5,
            stroke_control_snapshot: Vec::new(),
            restore_auto: false,
            stroke_restores: false,
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
/// Does not touch the scene document: channel values are bulk per-cell data
/// living in the sidecar store, the same as heights.
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
        if let Some(mut data) = world.resource_mut::<TerrainDataStore>().entry_for(&terrain)
            && self.channel < data.channels().len()
        {
            data.set_channel_values(self.channel, values);
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

    fn heap_bytes(&self) -> usize {
        (self.old_values.capacity() + self.new_values.capacity()) * size_of::<u16>()
    }
}

/// Undo command for a texture-paint stroke: writes the whole control-word
/// layer back to a snapshot.
///
/// Whole-layer rather than rect-scoped, unlike
/// [`super::sculpt::SetTerrainHeights`]: a resolution-256 terrain's control
/// layer is 256 KiB (`256 * 256 * 4` bytes), so an entry holds 512 KiB.
/// [`CommandHistory`]'s byte budget is what bounds that.
///
/// Does not touch the scene document, for the same reason
/// [`SetTerrainChannel`] does not: control words are bulk per-cell data
/// living in the sidecar store.
pub struct SetTerrainControl {
    pub entity: Entity,
    pub old_control: Vec<Control>,
    pub new_control: Vec<Control>,
    pub label: String,
}

impl SetTerrainControl {
    fn apply(&self, world: &mut World, values: &[Control]) {
        let Some(terrain) = world.get::<jackdaw_scene_types::Terrain>(self.entity) else {
            return;
        };
        let terrain = terrain.clone();
        if let Some(mut control) = world
            .resource_mut::<TerrainDataStore>()
            .control_mut(&terrain)
        {
            let len = control.len().min(values.len());
            control[..len].copy_from_slice(&values[..len]);
        }
    }
}

impl EditorCommand for SetTerrainControl {
    fn execute(&mut self, world: &mut World) {
        self.apply(world, &self.new_control.clone());
    }

    fn undo(&mut self, world: &mut World) {
        self.apply(world, &self.old_control.clone());
    }

    fn description(&self) -> &str {
        &self.label
    }

    fn heap_bytes(&self) -> usize {
        (self.old_control.capacity() + self.new_control.capacity()) * size_of::<Control>()
    }
}

/// Pick the paint tool. Pressing again puts the brush away.
#[operator(
    id = "terrain.tool.paint",
    label = "Paint",
    description = "Paint the active scatter mask's selected value onto the terrain.",
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
    store: Res<TerrainDataStore>,
    mut paint_state: ResMut<TerrainPaintState>,
) {
    if *edit_mode != TerrainEditMode::Paint {
        if paint_state.brush_position.is_some() || paint_state.target.is_some() {
            paint_state.brush_position = None;
            paint_state.target = None;
        }
        return;
    }
    match super::sculpt::terrain_brush_hit(&vp, &terrain_query, &selection, &store) {
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
    description = "Paint the active scatter mask under the brush (erase with Ctrl), or \
                   in Texture mode paint the selected texture (Ctrl paints the overlay).",
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
    mut terrain_query: Query<(&jackdaw_scene_types::Terrain, &mut TerrainDirtyChunks)>,
    mut store: ResMut<TerrainDataStore>,
    mut history: ResMut<CommandHistory>,
    time: Res<Time>,
    active: ActiveModalQuery,
) -> OperatorResult {
    if *edit_mode != TerrainEditMode::Paint {
        return OperatorResult::Cancelled;
    }
    let target = paint_state.target?;

    match paint_state.domain {
        PaintDomain::Channels => {
            let (terrain, mut dirty) = terrain_query.get_mut(target)?;
            let terrain = terrain.clone();

            let erase = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
            let value = paint_state.value_for(&terrain, erase)?;
            let channel_index = if active.is_modal_running() {
                paint_state.stroke_channel
            } else {
                paint_state.active_channel
            };

            let mut data = store.entry_for(&terrain)?;
            let descriptor = data.channel_mut(channel_index)?;
            let (element, name) = (descriptor.element, descriptor.name.clone());
            // Gathered from the regions, brushed, and scattered back, the
            // same shape a height stroke takes, so a stroke reaching a cell
            // no region holds allocates one on the write.
            let resolution = data.document().grid_resolution();
            let mut values = data.channel_values(channel_index);

            if !active.is_modal_running() {
                paint_state.active = true;
                paint_state.stroke_channel = channel_index;
                paint_state.stroke_snapshot = values.clone();
            }

            // See `super::stroke_should_end`: checked every frame,
            // including the modal's first.
            if super::stroke_should_end(&mouse) {
                paint_state.active = false;
                let old_values = std::mem::take(&mut paint_state.stroke_snapshot);
                // A stroke that changed nothing, such as repainting a value
                // onto itself, leaves no history entry.
                if old_values != values {
                    history.push_executed(Box::new(SetTerrainChannel {
                        entity: target,
                        channel: channel_index,
                        old_values,
                        new_values: values.clone(),
                        label: format!("Paint {name}"),
                    }));
                }
                return OperatorResult::Finished;
            }

            if let Some(grid_pos) = paint_state.brush_position {
                let changed = jackdaw_terrain::apply_channel_brush(
                    &mut values,
                    resolution,
                    element,
                    grid_pos,
                    brush_settings.radius,
                    brush_settings.falloff,
                    PAINT_THRESHOLD,
                    value,
                );
                if changed > 0 {
                    data.set_channel_values(channel_index, &values);
                    // Only the chunks the stroke touched, never `rebuild_all`.
                    for chunk in jackdaw_terrain::affected_chunks_at(
                        resolution,
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
        PaintDomain::Textures => {
            let (terrain, _dirty) = terrain_query.get_mut(target)?;
            let terrain = terrain.clone();
            // The stroke lands on the cells the terrain holds, so the brush
            // reaches wherever ground has been allocated.
            let resolution = store.grid_shape(&terrain).resolution;
            let secondary = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
            let texture_id = paint_state.active_texture_id;
            let slot_count = store.materials(&terrain.data_path).len();

            if !active.is_modal_running() {
                // `control_mut` marks the whole map for upload, which puts
                // paint that arrived by a load or an undo on screen, and is
                // where a terrain the store refuses turns the stroke away
                // before it starts.
                paint_state.stroke_control_snapshot = store.control_mut(&terrain)?.to_vec();
                paint_state.active = true;
                paint_state.stroke_restores = paint_state.restore_auto;
            }
            let restoring = paint_state.stroke_restores;

            if super::stroke_should_end(&mouse) {
                paint_state.active = false;
                let old_control = std::mem::take(&mut paint_state.stroke_control_snapshot);
                let new_control = store.control(&terrain.data_path);
                if old_control.as_slice() != new_control.as_ref() {
                    history.push_executed(Box::new(SetTerrainControl {
                        entity: target,
                        old_control,
                        new_control: new_control.to_vec(),
                        label: if restoring {
                            "Restore Auto".to_string()
                        } else {
                            "Paint Texture".to_string()
                        },
                    }));
                }
                return OperatorResult::Finished;
            }

            // `active_texture_id` can be stale against the live list, from
            // a removed slot or from painting before the terrain has any,
            // and `terrain.texture.select`'s clamp (`MAX_TEXTURE_ID`, 31)
            // is looser than the list ceiling (`texture_set::MAX_TEXTURES`,
            // 16), so it cannot catch this. The write is refused rather
            // than clamped to an id other than the one displayed; the
            // options bar's texture hint reads "(pick one in the Terrain
            // panel)" for this id.
            //
            // A restoring stroke lays down no texture, so it has no id to
            // be stale about: it clears the manual bit and leaves every
            // cell's ids and blend alone.
            let texture_valid = restoring || (texture_id as usize) < slot_count;

            if texture_valid
                && let Some(grid_pos) = paint_state.brush_position
                // The cells this frame writes, and the only cells the
                // renderer has to rebuild texels for.
                && let Some(rect) =
                    GridRect::brush(resolution, grid_pos, brush_settings.radius)
                && let Some(mut control) = store.control_rect_mut(&terrain, rect)
            {
                if restoring {
                    jackdaw_terrain::apply_restore_brush(
                        &mut control,
                        resolution,
                        grid_pos,
                        brush_settings.radius,
                        brush_settings.falloff,
                        PAINT_THRESHOLD,
                    );
                } else {
                    jackdaw_terrain::apply_control_brush(
                        &mut control,
                        resolution,
                        grid_pos,
                        brush_settings.radius,
                        brush_settings.falloff,
                        paint_state.texture_opacity,
                        time.delta_secs(),
                        secondary,
                        texture_id,
                    );
                }
            }
            OperatorResult::Running
        }
    }
}

/// Falloff a cell must clear to be written. Integer channels cannot blend,
/// so the brush edge needs a hard cutoff; half-strength matches the visible
/// ring's half-intensity contour.
const PAINT_THRESHOLD: f32 = 0.5;

fn cancel_terrain_paint(
    mut paint_state: ResMut<TerrainPaintState>,
    mut terrain_query: Query<(&jackdaw_scene_types::Terrain, &mut TerrainDirtyChunks)>,
    mut store: ResMut<TerrainDataStore>,
) {
    if !paint_state.active {
        return;
    }
    paint_state.active = false;

    if paint_state.domain == PaintDomain::Textures {
        let snapshot = std::mem::take(&mut paint_state.stroke_control_snapshot);
        let Some(target) = paint_state.target else {
            return;
        };
        let Ok((terrain, _dirty)) = terrain_query.get_mut(target) else {
            return;
        };
        let terrain = terrain.clone();
        if let Some(mut control) = store.control_mut(&terrain)
            && control.len() == snapshot.len()
        {
            control.copy_from_slice(&snapshot);
        }
        return;
    }

    let snapshot = std::mem::take(&mut paint_state.stroke_snapshot);
    let channel_index = paint_state.stroke_channel;
    if let Some(target) = paint_state.target
        && let Ok((terrain, mut dirty)) = terrain_query.get_mut(target)
    {
        let terrain = terrain.clone();
        if let Some(mut data) = store.entry_for(&terrain)
            && channel_index < data.channels().len()
        {
            data.set_channel_values(channel_index, &snapshot);
        }
        dirty.rebuild_all = true;
    }
}

/// Shift+scroll resizes the paint brush as it resizes the sculpt brush, so
/// the gesture keeps its meaning between modes.
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

/// Ring gizmo following the terrain surface, in the active palette colour
/// so the user sees what the brush lays down.
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

    let heightmap = store.heightmap(terrain);
    let origin = terrain_tf.translation();
    let ring = super::brush_ring_points(&heightmap.map, grid_pos, brush_settings.radius);

    for pair in ring.windows(2) {
        gizmos.line(origin + pair[0], origin + pair[1], color);
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

    /// Ctrl-held erases, writing 0 whatever entry is selected.
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

    /// An out-of-range palette index reads as 0 rather than panicking, so
    /// removing the selected entry does not take the editor down on the
    /// next brush move.
    #[test]
    fn a_stale_palette_selection_falls_back_to_zero() {
        let state = TerrainPaintState {
            active_entry: 99,
            ..default()
        };
        assert_eq!(state.value_for(&two_entry_terrain(), false), Some(0));
    }

    // --- SetTerrainControl ---

    fn control_test_world() -> (World, Entity) {
        let mut world = World::new();
        world.init_resource::<TerrainDataStore>();
        let terrain = jackdaw_scene_types::Terrain {
            resolution: 4,
            data_path: "zone.terrain-0.jdterrain".to_string(),
            ..default()
        };
        // Ground to paint on: nothing allocates implicitly, and the regions
        // are sized so this fixture is sixteen cells.
        let mut regions = jackdaw_terrain::TerrainRegions::new(
            jackdaw_terrain::RegionSize::new(4).expect("a power of two"),
        );
        regions.ensure_grid(4).expect("inside the region cap");
        world.resource_mut::<TerrainDataStore>().insert(
            terrain.data_path.clone(),
            jackdaw_terrain::RegionTerrainData {
                regions,
                ..default()
            },
        );
        let entity = world.spawn(terrain).id();
        (world, entity)
    }

    /// Execute writes the painted result and undo restores the layer as it
    /// stood before the stroke.
    #[test]
    fn execute_and_undo_round_trip_the_whole_control_layer() {
        let (mut world, entity) = control_test_world();
        let terrain = world
            .get::<jackdaw_scene_types::Terrain>(entity)
            .unwrap()
            .clone();

        let old_control = world
            .resource_mut::<TerrainDataStore>()
            .control_mut(&terrain)
            .expect("keyed")
            .to_vec();
        assert!(old_control.iter().all(|c| *c == Control::default()));

        let mut new_control = old_control.clone();
        new_control[5] = Control::default().with_base_id(3).with_blend(120);

        let mut command = SetTerrainControl {
            entity,
            old_control: old_control.clone(),
            new_control: new_control.clone(),
            label: "Paint Texture".to_string(),
        };

        command.execute(&mut world);
        assert_eq!(
            world
                .resource::<TerrainDataStore>()
                .control("zone.terrain-0.jdterrain"),
            new_control.as_slice(),
        );

        command.undo(&mut world);
        assert_eq!(
            world
                .resource::<TerrainDataStore>()
                .control("zone.terrain-0.jdterrain"),
            old_control.as_slice(),
        );
    }

    /// A control layer that shrank between capture and apply, such as when
    /// the terrain's resolution changed, must not panic writing a longer
    /// snapshot back over a shorter live layer.
    #[test]
    fn a_mismatched_snapshot_length_does_not_panic() {
        let (mut world, entity) = control_test_world();
        let mut command = SetTerrainControl {
            entity,
            old_control: vec![Control::default(); 4],
            new_control: vec![Control::default(); 4],
            label: "Paint Texture".to_string(),
        };
        command.execute(&mut world);
    }

    // --- terrain_paint operator ---

    /// A world with every resource `terrain_paint` reads and one terrain
    /// entity, mouse held down so the first tick does not read as a stroke
    /// end (see `super::stroke_should_end`), and no active modal entity, so
    /// the operator starts a new stroke on the first call.
    fn paint_op_world() -> (World, Entity) {
        let mut world = World::new();
        world.insert_resource({
            let mut mouse = ButtonInput::<MouseButton>::default();
            mouse.press(MouseButton::Left);
            mouse
        });
        world.init_resource::<ButtonInput<KeyCode>>();
        world.insert_resource(TerrainEditMode::Paint);
        world.init_resource::<TerrainBrushSettings>();
        world.init_resource::<TerrainPaintState>();
        world.init_resource::<TerrainDataStore>();
        world.init_resource::<CommandHistory>();
        world.init_resource::<crate::terrain::splat::TerrainSplatMaterials>();
        let mut time: Time = Time::default();
        time.advance_by(std::time::Duration::from_millis(100));
        world.insert_resource(time);

        let terrain = jackdaw_scene_types::Terrain {
            resolution: 8,
            data_path: "zone.terrain-0.jdterrain".to_string(),
            ..default()
        };
        // Ground to paint on, in regions sized to keep the fixture small.
        let mut regions = jackdaw_terrain::TerrainRegions::new(
            jackdaw_terrain::RegionSize::new(8).expect("a power of two"),
        );
        regions.ensure_grid(8).expect("inside the region cap");
        world.resource_mut::<TerrainDataStore>().insert(
            terrain.data_path.clone(),
            jackdaw_terrain::RegionTerrainData {
                regions,
                ..default()
            },
        );
        let entity = world.spawn((terrain, TerrainDirtyChunks::default())).id();
        (world, entity)
    }

    fn call_terrain_paint(world: &mut World) -> OperatorResult {
        world
            .run_system_cached_with(
                terrain_paint,
                OperatorParameters(std::collections::BTreeMap::new()),
            )
            .expect("system runs")
    }

    /// An `active_texture_id` past the terrain's slot count does not
    /// paint: the brush goes quiet rather than writing an id the terrain
    /// has no material for.
    #[test]
    fn painting_with_an_out_of_range_active_texture_id_leaves_the_control_map_untouched() {
        let (mut world, entity) = paint_op_world();
        world
            .resource_mut::<TerrainDataStore>()
            .set_materials(
                "zone.terrain-0.jdterrain",
                vec![jackdaw_terrain::sidecar::TerrainMaterialSlot::new("grass")],
            )
            .expect("accepted");
        {
            let mut paint_state = world.resource_mut::<TerrainPaintState>();
            paint_state.domain = PaintDomain::Textures;
            paint_state.target = Some(entity);
            paint_state.brush_position = Some(Vec2::new(4.0, 4.0));
            // The list above has exactly one slot (id 0); 5 is out of range.
            paint_state.active_texture_id = 5;
        }

        let result = call_terrain_paint(&mut world);
        assert_eq!(result, OperatorResult::Running);

        let control = world
            .resource::<TerrainDataStore>()
            .control("zone.terrain-0.jdterrain");
        assert!(
            control.iter().all(|c| *c == Control::default()),
            "an out-of-range texture id must not paint any cell",
        );
    }

    /// An id the terrain has a slot for paints normally, so the bounds
    /// check refuses only the stale case.
    #[test]
    fn painting_with_an_in_range_active_texture_id_paints_normally() {
        let (mut world, entity) = paint_op_world();
        world
            .resource_mut::<TerrainDataStore>()
            .set_materials(
                "zone.terrain-0.jdterrain",
                vec![
                    jackdaw_terrain::sidecar::TerrainMaterialSlot::new("grass"),
                    jackdaw_terrain::sidecar::TerrainMaterialSlot::new("rock"),
                ],
            )
            .expect("accepted");
        {
            let mut paint_state = world.resource_mut::<TerrainPaintState>();
            paint_state.domain = PaintDomain::Textures;
            paint_state.target = Some(entity);
            paint_state.brush_position = Some(Vec2::new(4.0, 4.0));
            paint_state.active_texture_id = 1;
        }

        let result = call_terrain_paint(&mut world);
        assert_eq!(result, OperatorResult::Running);

        let control = world
            .resource::<TerrainDataStore>()
            .control("zone.terrain-0.jdterrain");
        assert!(
            control.iter().any(|c| *c != Control::default()),
            "an in-range texture id must still paint",
        );
    }

    /// The restore brush hands cells back to autoterrain by clearing the
    /// manual bit and touching nothing else, so the paint underneath
    /// remains to be claimed again.
    #[test]
    fn a_restoring_stroke_releases_cells_without_touching_their_paint() {
        let (mut world, entity) = paint_op_world();
        let painted = Control::default()
            .with_base_id(0)
            .with_overlay_id(0)
            .with_blend(90)
            .with_manual(true);
        {
            let terrain = world
                .get::<jackdaw_scene_types::Terrain>(entity)
                .unwrap()
                .clone();
            let mut store = world.resource_mut::<TerrainDataStore>();
            let mut control = store.control_mut(&terrain).expect("keyed");
            control.fill(painted);
        }
        world
            .resource_mut::<TerrainDataStore>()
            .set_materials(
                "zone.terrain-0.jdterrain",
                vec![jackdaw_terrain::sidecar::TerrainMaterialSlot::new("grass")],
            )
            .expect("accepted");
        {
            let mut paint_state = world.resource_mut::<TerrainPaintState>();
            paint_state.domain = PaintDomain::Textures;
            paint_state.target = Some(entity);
            paint_state.brush_position = Some(Vec2::new(4.0, 4.0));
            paint_state.restore_auto = true;
        }

        assert_eq!(call_terrain_paint(&mut world), OperatorResult::Running);

        let control = world
            .resource::<TerrainDataStore>()
            .control("zone.terrain-0.jdterrain");
        let under_brush = control[4 * 8 + 4];
        assert!(!under_brush.manual(), "the cell is back to autoterrain");
        assert_eq!(under_brush.blend(), 90, "its paint is untouched");
        assert_eq!(under_brush.base_id(), 0);
        assert!(
            control[0].manual(),
            "a cell outside the brush keeps its claim",
        );
    }

    /// Restoring lays no texture down, so it has no texture id to be stale
    /// about, and the bounds check that silences an ordinary stroke leaves
    /// it alone.
    #[test]
    fn a_restoring_stroke_works_on_a_terrain_with_no_materials_at_all() {
        let (mut world, entity) = paint_op_world();
        {
            let terrain = world
                .get::<jackdaw_scene_types::Terrain>(entity)
                .unwrap()
                .clone();
            let mut store = world.resource_mut::<TerrainDataStore>();
            let mut control = store.control_mut(&terrain).expect("keyed");
            control.fill(Control::default().with_manual(true));
        }
        {
            let mut paint_state = world.resource_mut::<TerrainPaintState>();
            paint_state.domain = PaintDomain::Textures;
            paint_state.target = Some(entity);
            paint_state.brush_position = Some(Vec2::new(4.0, 4.0));
            paint_state.restore_auto = true;
            paint_state.active_texture_id = 5;
        }

        assert_eq!(call_terrain_paint(&mut world), OperatorResult::Running);

        let control = world
            .resource::<TerrainDataStore>()
            .control("zone.terrain-0.jdterrain");
        assert!(!control[4 * 8 + 4].manual());
    }

    /// Flipping the checkbox mid-drag leaves the rest of the stroke doing
    /// what it started as.
    #[test]
    fn a_stroke_keeps_doing_what_it_started_as() {
        let (mut world, entity) = paint_op_world();
        world
            .resource_mut::<TerrainDataStore>()
            .set_materials(
                "zone.terrain-0.jdterrain",
                vec![jackdaw_terrain::sidecar::TerrainMaterialSlot::new("grass")],
            )
            .expect("accepted");
        {
            let mut paint_state = world.resource_mut::<TerrainPaintState>();
            paint_state.domain = PaintDomain::Textures;
            paint_state.target = Some(entity);
            paint_state.brush_position = Some(Vec2::new(4.0, 4.0));
            paint_state.restore_auto = false;
        }

        let _ = call_terrain_paint(&mut world);
        assert!(!world.resource::<TerrainPaintState>().stroke_restores);
        world.resource_mut::<TerrainPaintState>().restore_auto = true;

        // The stroke is running, so the flag it captured stands.
        assert!(!world.resource::<TerrainPaintState>().stroke_restores);
    }

    /// A quarantined (load-failed) terrain refuses every write, so
    /// `terrain_paint` is a no-op through the same `TerrainDataStore`
    /// refusal every other terrain edit goes through (see `store.rs`'s
    /// `document_in`).
    #[test]
    fn painting_a_quarantined_terrain_is_a_no_op() {
        let (mut world, entity) = paint_op_world();
        {
            // A quarantined terrain is one whose sidecar never decoded, so
            // the store holds nothing for it.
            let mut store = world.resource_mut::<TerrainDataStore>();
            store.remove("zone.terrain-0.jdterrain");
            store.mark_load_failed("zone.terrain-0.jdterrain", "unreadable bytes");
        }
        {
            let mut paint_state = world.resource_mut::<TerrainPaintState>();
            paint_state.domain = PaintDomain::Textures;
            paint_state.target = Some(entity);
            paint_state.brush_position = Some(Vec2::new(4.0, 4.0));
            paint_state.active_texture_id = 0;
        }

        let result = call_terrain_paint(&mut world);
        assert_eq!(
            result,
            OperatorResult::Cancelled,
            "a quarantined terrain has no document to paint into",
        );
        assert!(
            world
                .resource::<TerrainDataStore>()
                .control("zone.terrain-0.jdterrain")
                .is_empty(),
            "nothing was minted for the quarantined path",
        );
        assert!(
            !world.resource::<TerrainPaintState>().active,
            "no stroke was started"
        );
    }
}
