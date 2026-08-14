use bevy::prelude::*;
use jackdaw_api::prelude::*;

use super::{
    CHUNK_SIZE, TerrainBrushSettings, TerrainDataStore, TerrainDirtyChunks, TerrainEditMode,
    TerrainSculptState,
};
use crate::commands::{CommandHistory, EditorCommand};
use crate::default_style;
use crate::selection::Selection;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (
            update_terrain_brush_position,
            sculpt_invoke_trigger,
            handle_brush_resize_scroll,
            draw_terrain_brush_gizmo,
        )
            .chain()
            .run_if(in_state(crate::AppState::Editor)),
    );
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TerrainSculptOp>();
}

/// Undo command for terrain height changes.
///
/// Heights are written to [`TerrainDataStore`], not to the component and
/// not to the scene document. Pushing 262,144 floats through the BSN AST
/// on every stroke is the defect the sidecar exists to fix, so this
/// command deliberately does not sync the heights to the document.
///
/// Memory model: each entry holds two full heightmap copies (`old_heights`
/// and `new_heights`), and [`CommandHistory`]'s `undo_stack` is a plain,
/// uncapped `Vec`. A long sculpting session on a large terrain therefore
/// accumulates memory proportional to `resolution^2 * strokes` for as
/// long as the tab stays open -- there is currently no depth cap or
/// delta compression, and none is planned this round. If this becomes a
/// real problem, the fix belongs at
/// `CommandHistory` (a shared depth cap for every command type), not as
/// special-casing here.
pub struct SetTerrainHeights {
    pub entity: Entity,
    pub old_heights: Vec<f32>,
    pub new_heights: Vec<f32>,
    pub label: String,
    /// Optional `(old, new)` world size to move with the heights.
    ///
    /// Only the quantize operator sets this. Pinning a terrain's vertex
    /// spacing to a metric cell size resizes it and re-snaps its heights
    /// in one gesture, and undoing half of that leaves the terrain at a
    /// spacing its heights were never snapped for. `size` is three
    /// floats, not a per-cell array, so it does reach the document --
    /// the rule this command exists to keep is about bulk data, not
    /// about every field.
    pub resize: Option<(Vec2, Vec2)>,
}

impl SetTerrainHeights {
    /// Construct a heights-only entry. The overwhelmingly common case:
    /// a stroke, a generate, an erode.
    pub fn new(
        entity: Entity,
        old_heights: Vec<f32>,
        new_heights: Vec<f32>,
        label: String,
    ) -> Self {
        Self {
            entity,
            old_heights,
            new_heights,
            label,
            resize: None,
        }
    }

    fn apply(&self, world: &mut World, heights: &[f32], size: Option<Vec2>) {
        if let Some(size) = size {
            let resized = match world.get_mut::<jackdaw_scene_types::Terrain>(self.entity) {
                Some(mut terrain) if terrain.size != size => {
                    terrain.size = size;
                    Some(terrain.clone())
                }
                _ => None,
            };
            if let Some(terrain) = resized {
                crate::commands::sync_component_to_ast(
                    world,
                    self.entity,
                    "jackdaw_scene_types::types::Terrain",
                    &terrain,
                );
            }
        }
        let Some(terrain) = world.get::<jackdaw_scene_types::Terrain>(self.entity) else {
            return;
        };
        let terrain = terrain.clone();
        if let Some(data) = world.resource_mut::<TerrainDataStore>().entry_for(&terrain) {
            data.heights = heights.to_vec();
            data.normalize();
        }
        if let Some(mut dirty) = world.get_mut::<TerrainDirtyChunks>(self.entity) {
            dirty.rebuild_all = true;
        }
    }
}

impl EditorCommand for SetTerrainHeights {
    fn execute(&mut self, world: &mut World) {
        // `apply` already copies its `heights` slice into the store
        // (`data.heights = heights.to_vec()`), so cloning `new_heights`
        // here first was a second full-array copy for nothing -- a
        // 512-resolution terrain is 262,144 floats, so that was 1 MiB
        // wasted per undo/redo click. `&self.new_heights` borrows
        // straight from the command.
        self.apply(world, &self.new_heights, self.resize.map(|(_, new)| new));
    }

    fn undo(&mut self, world: &mut World) {
        self.apply(world, &self.old_heights, self.resize.map(|(old, _)| old));
    }

    fn description(&self) -> &str {
        &self.label
    }
}

/// Raycast the cursor against the selected terrain's XZ plane and
/// return the (entity, grid coordinate) that the brush should target.
pub(super) fn terrain_brush_hit(
    vp: &crate::viewport::ViewportCursor,
    terrain_query: &Query<(Entity, &jackdaw_scene_types::Terrain, &GlobalTransform)>,
    selection: &Selection,
) -> Option<(Entity, Vec2)> {
    let selected = selection.primary()?;
    let (terrain_entity, terrain, terrain_tf) = terrain_query.get(selected).ok()?;

    let cursor_pos = vp.cursor()?;

    let camera_entity = vp.camera_entity()?;
    let viewport_entity = vp.viewport_entity()?;
    let (camera, cam_tf) = vp.camera_for(camera_entity)?;
    let local_cursor = vp.viewport_cursor_for(camera, viewport_entity, cursor_pos)?;
    let ray = camera.viewport_to_world(cam_tf, local_cursor).ok()?;

    let terrain_origin = terrain_tf.translation();
    let denom = ray.direction.y;
    if denom.abs() <= 1e-6 {
        return None;
    }
    let t = (terrain_origin.y - ray.origin.y) / denom;
    if t <= 0.0 {
        return None;
    }
    let world_hit = ray.origin + ray.direction * t;
    let local = world_hit - terrain_origin;
    let half = terrain.size / 2.0;
    if local.x.abs() > half.x || local.z.abs() > half.y {
        return None;
    }

    Some((terrain_entity, local_to_grid(terrain, local.xz())))
}

/// Grid coordinate of a terrain-local XZ position.
///
/// Pure geometry, so the brush cursor does not copy a 512-resolution
/// heightmap out of the store every frame just to learn which cell it is
/// hovering. Mirrors `Heightmap::world_to_grid`.
fn local_to_grid(terrain: &jackdaw_scene_types::Terrain, local: Vec2) -> Vec2 {
    let cell = terrain.size / (terrain.resolution.max(2) - 1) as f32;
    (local + terrain.size / 2.0) / cell
}

/// Track the brush-target grid position so the overlay gizmo follows
/// the cursor even when no stroke is in progress.
fn update_terrain_brush_position(
    edit_mode: Res<TerrainEditMode>,
    vp: crate::viewport::ViewportCursor,
    terrain_query: Query<(Entity, &jackdaw_scene_types::Terrain, &GlobalTransform)>,
    selection: Res<Selection>,
    mut sculpt_state: ResMut<TerrainSculptState>,
) {
    if !matches!(*edit_mode, TerrainEditMode::Sculpt(_)) {
        if sculpt_state.brush_position.is_some() || sculpt_state.target.is_some() {
            sculpt_state.brush_position = None;
            sculpt_state.target = None;
        }
        return;
    }
    match terrain_brush_hit(&vp, &terrain_query, &selection) {
        Some((entity, grid)) => {
            sculpt_state.target = Some(entity);
            sculpt_state.brush_position = Some(grid);
        }
        None => sculpt_state.brush_position = None,
    }
}

/// LMB in sculpt mode (with the brush over the terrain) dispatches
/// `terrain.sculpt`. Mouse-button gestures aren't expressible as BEI
/// key bindings.
fn sculpt_invoke_trigger(
    mouse: Res<ButtonInput<MouseButton>>,
    edit_mode: Res<TerrainEditMode>,
    sculpt_state: Res<TerrainSculptState>,
    mut commands: Commands,
) {
    if sculpt_state.active
        || !mouse.just_pressed(MouseButton::Left)
        || !matches!(*edit_mode, TerrainEditMode::Sculpt(_))
        || sculpt_state.brush_position.is_none()
        || sculpt_state.target.is_none()
    {
        return;
    }
    commands.queue(|world: &mut World| {
        let _ = world
            .operator(TerrainSculptOp::ID)
            .settings(CallOperatorSettings {
                execution_context: ExecutionContext::Invoke,
                creates_history_entry: false,
            })
            .call();
    });
}

#[operator(
    id = "terrain.sculpt",
    label = "Sculpt Terrain",
    description = "Sculpt the terrain under the brush while the mouse button is held.",
    modal = true,
    allows_undo = false,
    cancel = cancel_terrain_sculpt,
)]
pub fn terrain_sculpt(
    _: In<OperatorParameters>,
    mouse: Res<ButtonInput<MouseButton>>,
    edit_mode: Res<TerrainEditMode>,
    brush_settings: Res<TerrainBrushSettings>,
    mut sculpt_state: ResMut<TerrainSculptState>,
    mut terrain_query: Query<(&jackdaw_scene_types::Terrain, &mut TerrainDirtyChunks)>,
    mut store: ResMut<TerrainDataStore>,
    mut history: ResMut<CommandHistory>,
    time: Res<Time>,
    active: ActiveModalQuery,
) -> OperatorResult {
    let TerrainEditMode::Sculpt(tool) = *edit_mode else {
        return OperatorResult::Cancelled;
    };
    let target = sculpt_state.target?;
    let (terrain, mut dirty) = terrain_query.get_mut(target)?;
    let data = store.entry_for(terrain)?;

    if !active.is_modal_running() {
        sculpt_state.active = true;
        sculpt_state.stroke_snapshot = data.heights.clone();
    }

    // See `super::stroke_should_end` doc: checked every frame, including
    // the modal's first, not gated behind `else`/`modal.is_some()`.
    if super::stroke_should_end(&mouse) {
        sculpt_state.active = false;
        history.push_executed(Box::new(SetTerrainHeights::new(
            target,
            std::mem::take(&mut sculpt_state.stroke_snapshot),
            data.heights.clone(),
            format!("Terrain {tool:?}"),
        )));
        return OperatorResult::Finished;
    }

    if let Some(grid_pos) = sculpt_state.brush_position {
        let mut hm = jackdaw_terrain::Heightmap {
            resolution: terrain.resolution,
            size: terrain.size,
            max_height: terrain.max_height,
            heights: std::mem::take(&mut data.heights),
        };
        jackdaw_terrain::apply_brush(
            &mut hm,
            tool,
            grid_pos,
            brush_settings.radius,
            brush_settings.strength,
            brush_settings.falloff,
            time.delta_secs(),
            None,
        );
        // Snap inside the stroke, not on release: the user has to watch
        // the terraces form while dragging, or they are sculpting one
        // surface and getting another. Only the cells the brush touched
        // are rewritten, so this costs the brush footprint per frame
        // rather than the whole array.
        if let Some(step) = terrain.quantization.active_height_step() {
            jackdaw_terrain::quantize_region(
                &mut hm.heights,
                terrain.resolution,
                grid_pos,
                brush_settings.radius,
                step,
            );
        }
        let affected =
            jackdaw_terrain::affected_chunks(&hm, grid_pos, brush_settings.radius, CHUNK_SIZE);
        data.heights = hm.heights;
        for chunk in affected {
            dirty.dirty.insert(chunk);
        }
    }
    OperatorResult::Running
}

fn cancel_terrain_sculpt(
    mut sculpt_state: ResMut<TerrainSculptState>,
    mut terrain_query: Query<(&jackdaw_scene_types::Terrain, &mut TerrainDirtyChunks)>,
    mut store: ResMut<TerrainDataStore>,
) {
    if !sculpt_state.active {
        return;
    }
    sculpt_state.active = false;
    let snapshot = std::mem::take(&mut sculpt_state.stroke_snapshot);
    if let Some(target) = sculpt_state.target
        && let Ok((terrain, mut dirty)) = terrain_query.get_mut(target)
        && let Some(data) = store.entry_for(terrain)
    {
        data.heights = snapshot;
        data.normalize();
        dirty.rebuild_all = true;
    }
}

fn handle_brush_resize_scroll(
    keyboard: Res<ButtonInput<KeyCode>>,
    nav_scroll: crate::modal_inputs::NavScrollInputs,
    edit_mode: Res<TerrainEditMode>,
    mut brush_settings: ResMut<TerrainBrushSettings>,
) {
    if !matches!(*edit_mode, TerrainEditMode::Sculpt(_)) {
        return;
    }

    // Resize only fires when Shift is held. The camera system already skips
    // zoom when Shift is down, so resize and zoom remain mutually exclusive
    // exactly as they were before the BEI migration.
    let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    if !shift {
        return;
    }

    if nav_scroll.resize_up() {
        brush_settings.radius = f32::min(brush_settings.radius * 1.15, 50.0);
    } else if nav_scroll.resize_down() {
        brush_settings.radius = f32::max(brush_settings.radius * 0.87, 1.0);
    }
}

fn draw_terrain_brush_gizmo(
    sculpt_state: Res<TerrainSculptState>,
    brush_settings: Res<TerrainBrushSettings>,
    edit_mode: Res<TerrainEditMode>,
    terrains: Query<(&jackdaw_scene_types::Terrain, &GlobalTransform)>,
    store: Res<TerrainDataStore>,
    mut gizmos: Gizmos,
) {
    if !matches!(*edit_mode, TerrainEditMode::Sculpt(_)) {
        return;
    }

    let Some(target) = sculpt_state.target else {
        return;
    };
    let Some(grid_pos) = sculpt_state.brush_position else {
        return;
    };

    let Ok((terrain, terrain_tf)) = terrains.get(target) else {
        return;
    };

    let heightmap = super::mesh::heightmap_from_terrain(terrain, &store);

    let segments = 32;
    let radius = brush_settings.radius;
    let origin = terrain_tf.translation();
    let cell = heightmap.cell_size();

    for i in 0..segments {
        let a0 = (i as f32 / segments as f32) * std::f32::consts::TAU;
        let a1 = ((i + 1) as f32 / segments as f32) * std::f32::consts::TAU;

        let gx0 = grid_pos.x + a0.cos() * radius;
        let gz0 = grid_pos.y + a0.sin() * radius;
        let gx1 = grid_pos.x + a1.cos() * radius;
        let gz1 = grid_pos.y + a1.sin() * radius;

        let h0 = heightmap.sample_bilinear(gx0, gz0);
        let h1 = heightmap.sample_bilinear(gx1, gz1);

        let half = terrain.size / 2.0;
        let p0 = origin + Vec3::new(gx0 * cell.x - half.x, h0 + 0.1, gz0 * cell.y - half.y);
        let p1 = origin + Vec3::new(gx1 * cell.x - half.x, h1 + 0.1, gz1 * cell.y - half.y);

        gizmos.line(p0, p1, default_style::TERRAIN_SCULPT_GIZMO);
    }
}
