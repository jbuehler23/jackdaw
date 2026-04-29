use bevy::{
    input::mouse::{MouseScrollUnit, MouseWheel},
    prelude::*,
    ui::UiGlobalTransform,
};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;

use super::{
    CHUNK_SIZE, TerrainBrushSettings, TerrainDirtyChunks, TerrainEditMode, TerrainSculptState,
};
use crate::commands::{CommandHistory, EditorCommand};
use crate::default_style;
use crate::selection::Selection;
use crate::viewport::{MainViewportCamera, SceneViewport};

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
pub struct SetTerrainHeights {
    pub entity: Entity,
    pub old_heights: Vec<f32>,
    pub new_heights: Vec<f32>,
    pub label: String,
}

impl EditorCommand for SetTerrainHeights {
    fn execute(&mut self, world: &mut World) {
        if let Some(mut terrain) = world.get_mut::<jackdaw_jsn::Terrain>(self.entity) {
            terrain.heights = self.new_heights.clone();
        }
        if let Some(mut dirty) = world.get_mut::<TerrainDirtyChunks>(self.entity) {
            dirty.rebuild_all = true;
        }
        sync_terrain_heights_to_ast(world, self.entity);
    }

    fn undo(&mut self, world: &mut World) {
        if let Some(mut terrain) = world.get_mut::<jackdaw_jsn::Terrain>(self.entity) {
            terrain.heights = self.old_heights.clone();
        }
        if let Some(mut dirty) = world.get_mut::<TerrainDirtyChunks>(self.entity) {
            dirty.rebuild_all = true;
        }
        sync_terrain_heights_to_ast(world, self.entity);
    }

    fn description(&self) -> &str {
        &self.label
    }
}

fn sync_terrain_heights_to_ast(world: &mut World, entity: Entity) {
    if let Some(terrain) = world.get::<jackdaw_jsn::Terrain>(entity) {
        let terrain = terrain.clone();
        crate::commands::sync_component_to_ast(
            world,
            entity,
            "jackdaw_jsn::types::terrain::Terrain",
            &terrain,
        );
    }
}

/// Raycast the cursor against the selected terrain's XZ plane and
/// return the (entity, grid coordinate) that the brush should target.
fn terrain_brush_hit(
    windows: &Query<&Window>,
    camera_query: &Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: &Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    terrain_query: &Query<(Entity, &jackdaw_jsn::Terrain, &GlobalTransform)>,
    selection: &Selection,
) -> Option<(Entity, Vec2)> {
    let selected = selection.primary()?;
    let (terrain_entity, terrain, terrain_tf) = terrain_query.get(selected).ok()?;

    let window = windows.single().ok()?;
    let cursor_pos = window.cursor_position()?;

    let (vp_computed, vp_tf) = viewport_query.single().ok()?;
    let scale = vp_computed.inverse_scale_factor();
    let vp_size = vp_computed.size() * scale;
    let vp_top_left = vp_tf.translation * scale - vp_size / 2.0;
    let local_cursor = cursor_pos - vp_top_left;
    if local_cursor.x < 0.0
        || local_cursor.y < 0.0
        || local_cursor.x > vp_size.x
        || local_cursor.y > vp_size.y
    {
        return None;
    }

    let (camera, cam_tf) = camera_query.single().ok()?;
    let target_size = camera.logical_viewport_size().unwrap_or(vp_size);
    let local_cursor = local_cursor * target_size / vp_size;
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

    let heightmap = jackdaw_terrain::Heightmap {
        resolution: terrain.resolution,
        size: terrain.size,
        max_height: terrain.max_height,
        heights: terrain.heights.clone(),
    };
    Some((
        terrain_entity,
        heightmap.world_to_grid(Vec2::new(local.x, local.z)),
    ))
}

/// Track the brush-target grid position so the overlay gizmo follows
/// the cursor even when no stroke is in progress.
fn update_terrain_brush_position(
    edit_mode: Res<TerrainEditMode>,
    windows: Query<&Window>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    terrain_query: Query<(Entity, &jackdaw_jsn::Terrain, &GlobalTransform)>,
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
    match terrain_brush_hit(
        &windows,
        &camera_query,
        &viewport_query,
        &terrain_query,
        &selection,
    ) {
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
    description = "Apply the active sculpt tool while LMB is held. Modal: commits \
                   the height delta as a single undo entry on release; Escape \
                   restores the pre-stroke heights.",
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
    mut terrain_query: Query<(&mut jackdaw_jsn::Terrain, &mut TerrainDirtyChunks)>,
    mut history: ResMut<CommandHistory>,
    time: Res<Time>,
    modal: Option<Single<Entity, With<ActiveModalOperator>>>,
) -> OperatorResult {
    let TerrainEditMode::Sculpt(tool) = *edit_mode else {
        return OperatorResult::Cancelled;
    };
    let Some(target) = sculpt_state.target else {
        return OperatorResult::Cancelled;
    };
    let Ok((mut terrain, mut dirty)) = terrain_query.get_mut(target) else {
        return OperatorResult::Cancelled;
    };

    if modal.is_none() {
        sculpt_state.active = true;
        sculpt_state.stroke_snapshot = terrain.heights.clone();
    } else if mouse.just_released(MouseButton::Left) {
        sculpt_state.active = false;
        history.push_executed(Box::new(SetTerrainHeights {
            entity: target,
            old_heights: std::mem::take(&mut sculpt_state.stroke_snapshot),
            new_heights: terrain.heights.clone(),
            label: format!("Terrain {tool:?}"),
        }));
        return OperatorResult::Finished;
    }

    if let Some(grid_pos) = sculpt_state.brush_position {
        let mut hm = jackdaw_terrain::Heightmap {
            resolution: terrain.resolution,
            size: terrain.size,
            max_height: terrain.max_height,
            heights: terrain.heights.clone(),
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
        let affected =
            jackdaw_terrain::affected_chunks(&hm, grid_pos, brush_settings.radius, CHUNK_SIZE);
        terrain.heights = hm.heights;
        for chunk in affected {
            dirty.dirty.insert(chunk);
        }
    }
    OperatorResult::Running
}

fn cancel_terrain_sculpt(
    mut sculpt_state: ResMut<TerrainSculptState>,
    mut terrain_query: Query<(&mut jackdaw_jsn::Terrain, &mut TerrainDirtyChunks)>,
) {
    if !sculpt_state.active {
        return;
    }
    sculpt_state.active = false;
    let snapshot = std::mem::take(&mut sculpt_state.stroke_snapshot);
    if let Some(target) = sculpt_state.target
        && let Ok((mut terrain, mut dirty)) = terrain_query.get_mut(target)
    {
        terrain.heights = snapshot;
        dirty.rebuild_all = true;
    }
}

fn handle_brush_resize_scroll(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut scroll_events: MessageReader<MouseWheel>,
    edit_mode: Res<TerrainEditMode>,
    mut brush_settings: ResMut<TerrainBrushSettings>,
) {
    if !matches!(*edit_mode, TerrainEditMode::Sculpt(_)) {
        return;
    }

    let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    if !shift {
        return;
    }

    for event in scroll_events.read() {
        let delta = match event.unit {
            MouseScrollUnit::Line => event.y,
            MouseScrollUnit::Pixel => event.y * 0.01,
        };
        if delta > 0.0 {
            brush_settings.radius = f32::min(brush_settings.radius * 1.15, 50.0);
        } else if delta < 0.0 {
            brush_settings.radius = f32::max(brush_settings.radius * 0.87, 1.0);
        }
    }
}

fn draw_terrain_brush_gizmo(
    sculpt_state: Res<TerrainSculptState>,
    brush_settings: Res<TerrainBrushSettings>,
    edit_mode: Res<TerrainEditMode>,
    terrains: Query<(&jackdaw_jsn::Terrain, &GlobalTransform)>,
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

    let heightmap = jackdaw_terrain::Heightmap {
        resolution: terrain.resolution,
        size: terrain.size,
        max_height: terrain.max_height,
        heights: terrain.heights.clone(),
    };

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
