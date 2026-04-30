use bevy::{
    prelude::*,
    ui::UiGlobalTransform,
    window::{CursorGrabMode, CursorOptions},
};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;

use crate::default_style;
use crate::{
    commands::{CommandHistory, SetTransform},
    modal_transform::ModalTransformState,
    selection::{Selected, Selection},
    snapping::SnapSettings,
    viewport::{MainViewportCamera, SceneViewport},
    viewport_util::{point_to_segment_dist, window_to_viewport_cursor},
};

/// Gizmo group for transform gizmos, rendered on top of all geometry.
#[derive(Default, Reflect, GizmoConfigGroup)]
struct TransformGizmoGroup;

const AXIS_LENGTH: f32 = 1.0;
const AXIS_TIP_LENGTH: f32 = 0.25;
const AXIS_START_OFFSET: f32 = 0.2;
const ROTATE_RING_RADIUS: f32 = 1.0;
const SCALE_CUBE_SIZE: f32 = 0.07;
/// World units per unit of camera distance. Controls the gizmo's constant screen-space size.
const GIZMO_SCREEN_SCALE: f32 = 0.1;
const INACTIVE_ALPHA: f32 = 0.15;
const ROTATE_SENSITIVITY: f32 = 0.01;
const SCALE_SENSITIVITY: f32 = 0.005;
const MIN_SCALE: f32 = 0.01;
const AXIS_HIT_DISTANCE: f32 = 35.0;
const EPSILON: f32 = 1e-6;

#[derive(Resource, Default, PartialEq, Eq, Clone, Copy, Debug)]
pub enum GizmoMode {
    #[default]
    Translate,
    Rotate,
    Scale,
}

#[derive(Resource, Default, PartialEq, Eq, Clone, Copy, Debug)]
pub enum GizmoSpace {
    #[default]
    World,
    Local,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GizmoAxis {
    X,
    Y,
    Z,
}

#[derive(Resource, Default)]
pub struct GizmoDragState {
    pub active: bool,
    pub axis: Option<GizmoAxis>,
    pub drag_start_screen: Vec2,
    pub start_transform: Transform,
    pub entity: Option<Entity>,
    pub accumulated_delta: f32,
}

#[derive(Resource, Default)]
pub struct GizmoHoverState {
    pub hovered_axis: Option<GizmoAxis>,
}

pub struct TransformGizmosPlugin;

impl Plugin for TransformGizmosPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GizmoMode>()
            .init_resource::<GizmoSpace>()
            .init_resource::<GizmoDragState>()
            .init_resource::<GizmoHoverState>()
            .init_gizmo_group::<TransformGizmoGroup>()
            .add_systems(Startup, configure_transform_gizmos)
            .add_systems(
                Update,
                (handle_gizmo_hover, gizmo_drag_invoke_trigger)
                    .chain()
                    .in_set(crate::EditorInteractionSystems),
            )
            .add_systems(
                Update,
                draw_gizmos
                    .after(gizmo_drag_invoke_trigger)
                    .run_if(in_state(crate::AppState::Editor)),
            );
    }
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<GizmoDragOp>();
}

fn configure_transform_gizmos(mut config_store: ResMut<GizmoConfigStore>) {
    let (config, _) = config_store.config_mut::<TransformGizmoGroup>();
    config.depth_bias = -1.0;
    config.line.width = 3.0;
}

pub(crate) fn handle_gizmo_hover(
    selection: Res<Selection>,
    transforms: Query<&GlobalTransform, With<Selected>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    windows: Query<&Window>,
    mode: Res<GizmoMode>,
    space: Res<GizmoSpace>,
    mut hover: ResMut<GizmoHoverState>,
    drag_state: Res<GizmoDragState>,
    modal: Res<ModalTransformState>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    edit_mode: Res<crate::brush::EditMode>,
    draw_state: Res<crate::draw_brush::DrawBrushState>,
) {
    hover.hovered_axis = None;

    if drag_state.active || modal.active.is_some() || draw_state.active.is_some() {
        return;
    }

    // Don't show gizmo hover in brush edit mode
    if *edit_mode != crate::brush::EditMode::Object {
        return;
    }

    let Some(primary) = selection.primary() else {
        return;
    };
    let Ok(global_tf) = transforms.get(primary) else {
        return;
    };
    let Ok((camera, cam_tf)) = camera_query.single() else {
        return;
    };
    let Ok(window) = windows.single() else {
        return;
    };

    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };

    // Convert window cursor to viewport-local coordinates
    let Some(viewport_cursor) = window_to_viewport_cursor(cursor_pos, camera, &viewport_query)
    else {
        return;
    };

    let gizmo_pos = global_tf.translation();
    // Scale is inherently local, so force local orientation so handles match transform.scale axes
    let effective_space = if *mode == GizmoMode::Scale {
        &GizmoSpace::Local
    } else {
        &space
    };
    let rotation = gizmo_rotation(global_tf, effective_space);

    let cam_dist = (cam_tf.translation() - gizmo_pos).length();
    let scale = cam_dist * GIZMO_SCREEN_SCALE;

    let axes = [
        (GizmoAxis::X, rotation * Vec3::X),
        (GizmoAxis::Y, rotation * Vec3::Y),
        (GizmoAxis::Z, rotation * Vec3::Z),
    ];

    let mut best_axis = None;
    let mut best_dist = f32::MAX;
    let threshold = AXIS_HIT_DISTANCE;

    for (axis, dir) in &axes {
        let dist = match *mode {
            GizmoMode::Translate | GizmoMode::Scale => {
                let start = gizmo_pos + *dir * (AXIS_START_OFFSET * scale);
                let endpoint = gizmo_pos + *dir * (AXIS_LENGTH * scale);
                let Some(start_screen) = camera.world_to_viewport(cam_tf, start).ok() else {
                    continue;
                };
                let Some(end_screen) = camera.world_to_viewport(cam_tf, endpoint).ok() else {
                    continue;
                };
                point_to_segment_dist(viewport_cursor, start_screen, end_screen)
            }
            GizmoMode::Rotate => point_to_ring_screen_dist(
                viewport_cursor,
                camera,
                cam_tf,
                gizmo_pos,
                *dir,
                ROTATE_RING_RADIUS * scale,
            ),
        };
        if dist < threshold && dist < best_dist {
            best_dist = dist;
            best_axis = Some(*axis);
        }
    }

    hover.hovered_axis = best_axis;
}

/// LMB on a hovered gizmo axis dispatches `gizmo.drag`. Mouse-button
/// gestures aren't expressible as BEI key bindings.
fn gizmo_drag_invoke_trigger(
    mouse: Res<ButtonInput<MouseButton>>,
    selection: Res<Selection>,
    hover: Res<GizmoHoverState>,
    drag_state: Res<GizmoDragState>,
    modal: Res<ModalTransformState>,
    edit_mode: Res<crate::brush::EditMode>,
    draw_state: Res<crate::draw_brush::DrawBrushState>,
    mut commands: Commands,
) {
    if drag_state.active
        || !mouse.just_pressed(MouseButton::Left)
        || hover.hovered_axis.is_none()
        || selection.primary().is_none()
        || modal.active.is_some()
        || *edit_mode != crate::brush::EditMode::Object
        || draw_state.active.is_some()
    {
        return;
    }
    commands.queue(|world: &mut World| {
        let _ = world
            .operator(GizmoDragOp::ID)
            .settings(CallOperatorSettings {
                execution_context: ExecutionContext::Invoke,
                creates_history_entry: true,
            })
            .call();
    });
}

#[operator(
    id = "gizmo.drag",
    label = "Gizmo Drag",
    description = "Drag the active transform gizmo to translate / rotate / scale the \
                   primary selection. Modal: commits on LMB release, cancels on \
                   Escape (restoring the start transform). Mode and axis come from \
                   the toolbar's `GizmoMode` resource and the click-time \
                   `GizmoHoverState`.",
    modal = true,
    allows_undo = false,
    cancel = cancel_gizmo_drag,
)]
pub fn gizmo_drag(
    _: In<OperatorParameters>,
    selection: Res<Selection>,
    mut transforms: Query<(&GlobalTransform, &mut Transform), With<Selected>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    windows: Query<&Window>,
    mut cursor_query: Query<&mut CursorOptions, With<Window>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mode: Res<GizmoMode>,
    space: Res<GizmoSpace>,
    hover: Res<GizmoHoverState>,
    mut drag_state: ResMut<GizmoDragState>,
    mut history: ResMut<CommandHistory>,
    snap_settings: Res<SnapSettings>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    modal: Option<Single<Entity, With<ActiveModalOperator>>>,
) -> OperatorResult {
    let Ok(window) = windows.single() else {
        return OperatorResult::Cancelled;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return OperatorResult::Cancelled;
    };
    let Ok((camera, cam_tf)) = camera_query.single() else {
        return OperatorResult::Cancelled;
    };
    let Some(viewport_cursor) = window_to_viewport_cursor(cursor_pos, camera, &viewport_query)
    else {
        return OperatorResult::Cancelled;
    };

    if modal.is_none() {
        let Some(primary) = selection.primary() else {
            return OperatorResult::Cancelled;
        };
        let Some(axis) = hover.hovered_axis else {
            return OperatorResult::Cancelled;
        };
        let Ok((_, transform)) = transforms.get(primary) else {
            return OperatorResult::Cancelled;
        };
        drag_state.active = true;
        drag_state.axis = Some(axis);
        drag_state.drag_start_screen = viewport_cursor;
        drag_state.start_transform = *transform;
        drag_state.entity = Some(primary);
        drag_state.accumulated_delta = 0.0;
        if let Ok(mut cursor_opts) = cursor_query.single_mut() {
            cursor_opts.grab_mode = CursorGrabMode::Confined;
        }
        return OperatorResult::Running;
    }

    if mouse.just_released(MouseButton::Left) {
        if let Some(entity) = drag_state.entity
            && let Ok((_, transform)) = transforms.get(entity)
        {
            history.push_executed(Box::new(SetTransform {
                entity,
                old_transform: drag_state.start_transform,
                new_transform: *transform,
            }));
        }
        clear_gizmo_drag_state(&mut drag_state, &mut cursor_query);
        return OperatorResult::Finished;
    }

    let Some(entity) = drag_state.entity else {
        return OperatorResult::Finished;
    };
    let Ok((global_tf, mut transform)) = transforms.get_mut(entity) else {
        return OperatorResult::Finished;
    };
    let Some(axis) = drag_state.axis else {
        return OperatorResult::Finished;
    };

    let effective_space = if *mode == GizmoMode::Scale {
        &GizmoSpace::Local
    } else {
        &space
    };
    let rotation = gizmo_rotation(global_tf, effective_space);
    let axis_dir = match axis {
        GizmoAxis::X => rotation * Vec3::X,
        GizmoAxis::Y => rotation * Vec3::Y,
        GizmoAxis::Z => rotation * Vec3::Z,
    };
    let gizmo_pos = global_tf.translation();
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let mouse_delta = viewport_cursor - drag_state.drag_start_screen;

    match *mode {
        GizmoMode::Translate => {
            let drag_start_pos = drag_state.start_transform.translation;
            let Ok(origin_screen) = camera.world_to_viewport(cam_tf, drag_start_pos) else {
                return OperatorResult::Running;
            };
            let Ok(axis_screen) = camera.world_to_viewport(cam_tf, drag_start_pos + axis_dir)
            else {
                return OperatorResult::Running;
            };
            let screen_axis = axis_screen - origin_screen;
            let len_sq = screen_axis.length_squared();
            if len_sq < EPSILON {
                return OperatorResult::Running;
            }
            let projected = mouse_delta.dot(screen_axis) / len_sq;
            let snapped = snap_settings.snap_translate_vec3_if(axis_dir * projected, ctrl);
            transform.translation = drag_state.start_transform.translation + snapped;
        }
        GizmoMode::Rotate => {
            let screen_axis = match axis {
                GizmoAxis::X => Vec2::Y,
                GizmoAxis::Y => Vec2::X,
                GizmoAxis::Z => -Vec2::X,
            };
            let raw_angle = mouse_delta.dot(screen_axis) * ROTATE_SENSITIVITY;
            let angle = snap_settings.snap_rotate_if(raw_angle, ctrl);
            transform.rotation =
                Quat::from_axis_angle(axis_dir, angle) * drag_state.start_transform.rotation;
        }
        GizmoMode::Scale => {
            let Ok(origin_screen) = camera.world_to_viewport(cam_tf, gizmo_pos) else {
                return OperatorResult::Running;
            };
            let Ok(axis_screen) = camera.world_to_viewport(cam_tf, gizmo_pos + axis_dir) else {
                return OperatorResult::Running;
            };
            let screen_axis = (axis_screen - origin_screen).normalize_or_zero();
            let projected = mouse_delta.dot(screen_axis) * SCALE_SENSITIVITY;
            let mut new_scale = drag_state.start_transform.scale;
            match axis {
                GizmoAxis::X => new_scale.x = f32::max(new_scale.x + projected, MIN_SCALE),
                GizmoAxis::Y => new_scale.y = f32::max(new_scale.y + projected, MIN_SCALE),
                GizmoAxis::Z => new_scale.z = f32::max(new_scale.z + projected, MIN_SCALE),
            }
            transform.scale = snap_settings.snap_scale_vec3_if(new_scale, ctrl);
        }
    }
    OperatorResult::Running
}

fn cancel_gizmo_drag(
    mut drag_state: ResMut<GizmoDragState>,
    mut transforms: Query<&mut Transform, With<Selected>>,
    mut cursor_query: Query<&mut CursorOptions, With<Window>>,
) {
    if let Some(entity) = drag_state.entity
        && let Ok(mut transform) = transforms.get_mut(entity)
    {
        *transform = drag_state.start_transform;
    }
    clear_gizmo_drag_state(&mut drag_state, &mut cursor_query);
}

fn clear_gizmo_drag_state(
    drag_state: &mut GizmoDragState,
    cursor_query: &mut Query<&mut CursorOptions, With<Window>>,
) {
    drag_state.active = false;
    drag_state.axis = None;
    drag_state.entity = None;
    if let Ok(mut cursor_opts) = cursor_query.single_mut() {
        cursor_opts.grab_mode = CursorGrabMode::None;
    }
}

fn draw_gizmos(
    mut gizmos: Gizmos<TransformGizmoGroup>,
    selection: Res<Selection>,
    transforms: Query<&GlobalTransform, With<Selected>>,
    camera_query: Query<&GlobalTransform, With<MainViewportCamera>>,
    mode: Res<GizmoMode>,
    space: Res<GizmoSpace>,
    hover: Res<GizmoHoverState>,
    drag_state: Res<GizmoDragState>,
    modal: Res<ModalTransformState>,
    edit_mode: Res<crate::brush::EditMode>,
) {
    // Hide gizmo during modal operations or brush edit mode
    if modal.active.is_some() || *edit_mode != crate::brush::EditMode::Object {
        return;
    }

    let Some(primary) = selection.primary() else {
        return;
    };
    let Ok(global_tf) = transforms.get(primary) else {
        return;
    };
    let Ok(cam_tf) = camera_query.single() else {
        return;
    };

    let pos = global_tf.translation();
    let effective_space = if *mode == GizmoMode::Scale {
        &GizmoSpace::Local
    } else {
        &space
    };
    let rotation = gizmo_rotation(global_tf, effective_space);

    let cam_dist = (cam_tf.translation() - pos).length();
    let scale = cam_dist * GIZMO_SCREEN_SCALE;

    let right = rotation * Vec3::X;
    let up = rotation * Vec3::Y;
    let forward = rotation * Vec3::Z;

    let active_axis = if drag_state.active {
        drag_state.axis
    } else {
        hover.hovered_axis
    };

    let dragging = drag_state.active;
    let x_color = axis_color(GizmoAxis::X, active_axis, dragging);
    let y_color = axis_color(GizmoAxis::Y, active_axis, dragging);
    let z_color = axis_color(GizmoAxis::Z, active_axis, dragging);

    match *mode {
        GizmoMode::Translate => {
            gizmos
                .arrow(
                    pos + right * (AXIS_START_OFFSET * scale),
                    pos + right * (AXIS_LENGTH * scale),
                    x_color,
                )
                .with_tip_length(AXIS_TIP_LENGTH * scale);
            gizmos
                .arrow(
                    pos + up * (AXIS_START_OFFSET * scale),
                    pos + up * (AXIS_LENGTH * scale),
                    y_color,
                )
                .with_tip_length(AXIS_TIP_LENGTH * scale);
            gizmos
                .arrow(
                    pos + forward * (AXIS_START_OFFSET * scale),
                    pos + forward * (AXIS_LENGTH * scale),
                    z_color,
                )
                .with_tip_length(AXIS_TIP_LENGTH * scale);
        }
        GizmoMode::Rotate => {
            // Draw rotation rings
            gizmos.circle(
                Isometry3d::new(pos, Quat::from_rotation_arc(Vec3::Z, right)),
                ROTATE_RING_RADIUS * scale,
                x_color,
            );
            gizmos.circle(
                Isometry3d::new(pos, Quat::from_rotation_arc(Vec3::Z, up)),
                ROTATE_RING_RADIUS * scale,
                y_color,
            );
            gizmos.circle(
                Isometry3d::new(pos, Quat::from_rotation_arc(Vec3::Z, forward)),
                ROTATE_RING_RADIUS * scale,
                z_color,
            );
        }
        GizmoMode::Scale => {
            // Draw scale handles: lines with cubes at the end
            let cube_half = SCALE_CUBE_SIZE * scale;
            for (dir, color) in [(right, x_color), (up, y_color), (forward, z_color)] {
                let end = pos + dir * (AXIS_LENGTH * scale);
                gizmos.line(pos + dir * (AXIS_START_OFFSET * scale), end, color);
                // Draw a small cube at the end using lines
                let x = Vec3::X * cube_half;
                let y = Vec3::Y * cube_half;
                let z = Vec3::Z * cube_half;
                let corners = [
                    end - x - y - z,
                    end + x - y - z,
                    end + x + y - z,
                    end - x + y - z,
                    end - x - y + z,
                    end + x - y + z,
                    end + x + y + z,
                    end - x + y + z,
                ];
                // Bottom face
                gizmos.line(corners[0], corners[1], color);
                gizmos.line(corners[1], corners[2], color);
                gizmos.line(corners[2], corners[3], color);
                gizmos.line(corners[3], corners[0], color);
                // Top face
                gizmos.line(corners[4], corners[5], color);
                gizmos.line(corners[5], corners[6], color);
                gizmos.line(corners[6], corners[7], color);
                gizmos.line(corners[7], corners[4], color);
                // Verticals
                gizmos.line(corners[0], corners[4], color);
                gizmos.line(corners[1], corners[5], color);
                gizmos.line(corners[2], corners[6], color);
                gizmos.line(corners[3], corners[7], color);
            }
        }
    }
}

fn gizmo_rotation(global_tf: &GlobalTransform, space: &GizmoSpace) -> Quat {
    match space {
        GizmoSpace::World => Quat::IDENTITY,
        GizmoSpace::Local => {
            let (_, rotation, _) = global_tf.to_scale_rotation_translation();
            rotation
        }
    }
}

fn axis_color(axis: GizmoAxis, active: Option<GizmoAxis>, dragging: bool) -> Color {
    let is_active = active == Some(axis);
    let (normal, bright) = match axis {
        GizmoAxis::X => (default_style::AXIS_X, default_style::AXIS_X_BRIGHT),
        GizmoAxis::Y => (default_style::AXIS_Y, default_style::AXIS_Y_BRIGHT),
        GizmoAxis::Z => (default_style::AXIS_Z, default_style::AXIS_Z_BRIGHT),
    };

    if is_active {
        bright
    } else if dragging {
        // Dim non-active axes during drag
        normal.with_alpha(INACTIVE_ALPHA)
    } else {
        normal
    }
}

fn point_to_ring_screen_dist(
    cursor: Vec2,
    camera: &Camera,
    cam_tf: &GlobalTransform,
    center: Vec3,
    normal: Vec3,
    radius: f32,
) -> f32 {
    const RING_SAMPLES: usize = 16;
    let rot = Quat::from_rotation_arc(Vec3::Z, normal);
    let mut min_dist = f32::MAX;
    let mut prev_screen = None;

    for i in 0..=RING_SAMPLES {
        let angle = (i % RING_SAMPLES) as f32 * std::f32::consts::TAU / RING_SAMPLES as f32;
        let local = Vec3::new(angle.cos() * radius, angle.sin() * radius, 0.0);
        let world = center + rot * local;
        let Some(screen) = camera.world_to_viewport(cam_tf, world).ok() else {
            prev_screen = None;
            continue;
        };
        if let Some(prev) = prev_screen {
            let dist = point_to_segment_dist(cursor, prev, screen);
            if dist < min_dist {
                min_dist = dist;
            }
        }
        prev_screen = Some(screen);
    }

    min_dist
}
