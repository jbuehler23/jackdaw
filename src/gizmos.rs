use bevy::{
    ecs::system::SystemParam,
    prelude::*,
    ui::UiGlobalTransform,
    window::{CursorGrabMode, CursorOptions},
};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;

use crate::active_tool::ActiveTool;
use crate::default_style;
use crate::{
    modal_transform::ModalTransformState,
    selection::{Selected, Selection},
    snapping::SnapSettings,
    viewport::{MainViewportCamera, SceneViewport},
    viewport_util::{
        point_to_segment_dist, window_to_viewport_cursor_for,
        window_to_viewport_cursor_for_unbounded,
    },
};

/// Gizmo group for transform gizmos, rendered on top of all geometry.
#[derive(Default, Reflect, GizmoConfigGroup)]
struct TransformGizmoGroup;

/// Bundles viewport-pick params used by `gizmo_drag` (the system has
/// many params; bundling keeps it under Bevy's system-param ceiling).
#[derive(SystemParam)]
struct GizmoViewportCtx<'w, 's> {
    active_viewport: Res<'w, crate::viewport::ActiveViewport>,
    viewport_query:
        Query<'w, 's, (&'static ComputedNode, &'static UiGlobalTransform), With<SceneViewport>>,
    cursor: crate::viewport::UiCursorPos<'w, 's>,
}

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

pub struct GizmoTarget {
    pub entity: Entity,
    pub start_transform: Transform,
}

#[derive(Resource, Default)]
pub struct GizmoDragState {
    pub active: bool,
    pub axis: Option<GizmoAxis>,
    pub drag_start_screen: Vec2,
    pub targets: Vec<GizmoTarget>,
    /// World-space centroid, used to draw the gizmo and project the drag axis.
    pub pivot: Vec3,
    /// Centroid of the targets' local start translations, used as the orbit
    /// center for rotate/scale. For a single target (or a group under one
    /// parent) this keeps the transform in the targets' own space so parented
    /// entities are not displaced; only mixed-parent groups are approximate.
    pub pivot_local: Vec3,
    pub accumulated_delta: f32,
    /// Camera entity of the viewport this drag was started in.
    /// Captured at modal start so subsequent frames keep referring to
    /// the same viewport even if the cursor wanders into a different
    /// one (multi-viewport setups).
    pub camera: Option<Entity>,
    /// `SceneViewport` UI-node entity of the same viewport.
    pub viewport: Option<Entity>,
}

#[derive(Resource, Default)]
pub struct GizmoHoverState {
    pub hovered_axis: Option<GizmoAxis>,
}

pub struct TransformGizmosPlugin;

impl Plugin for TransformGizmosPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActiveTool>()
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

/// World-space size for the transform gizmo, picked so it occupies a
/// roughly constant slice of the viewport regardless of zoom.
///
/// In perspective the apparent size of a fixed world-space object falls
/// off with camera distance, so we pre-multiply by `cam_dist` to keep
/// it constant. In orthographic the camera distance is meaningless
/// (`ORTHO_DISTANCE` is parked at 50 to stay outside scene geometry);
/// the visible extent is set by `OrthographicProjection::area`, which
/// shrinks as the user zooms in. Multiplying that height by the same
/// `GIZMO_SCREEN_SCALE` keeps the gizmo at the same on-screen fraction
/// in both projections.
fn gizmo_world_scale(projection: &Projection, cam_tf: &GlobalTransform, gizmo_pos: Vec3) -> f32 {
    match projection {
        Projection::Orthographic(ortho) => ortho.area.height() * GIZMO_SCREEN_SCALE,
        _ => (cam_tf.translation() - gizmo_pos).length() * GIZMO_SCREEN_SCALE,
    }
}

pub(crate) fn handle_gizmo_hover(
    selection: Res<Selection>,
    transforms: Query<&GlobalTransform, With<Selected>>,
    parents: Query<&ChildOf>,
    camera_query: Query<(&Camera, &GlobalTransform, &Projection), With<MainViewportCamera>>,
    cursor: crate::viewport::UiCursorPos,
    mode: Res<ActiveTool>,
    space: Res<GizmoSpace>,
    mut hover: ResMut<GizmoHoverState>,
    drag_state: Res<GizmoDragState>,
    modal: Res<ModalTransformState>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    active: Res<crate::viewport::ActiveViewport>,
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

    // Hover-routed: only the camera + viewport pair under the cursor
    // gets gizmo hover treatment (multi-viewport).
    let Some(camera_entity) = active.camera else {
        return;
    };
    let Some(viewport_entity) = active.ui_node else {
        return;
    };
    let Ok((camera, cam_tf, projection)) = camera_query.get(camera_entity) else {
        return;
    };

    let Some(cursor_pos) = cursor.get() else {
        return;
    };

    let Some(viewport_cursor) =
        window_to_viewport_cursor_for(cursor_pos, camera, viewport_entity, &viewport_query)
    else {
        return;
    };

    if matches!(*mode, ActiveTool::Select) {
        return;
    }

    let topmost = topmost_selected(
        &selection.entities,
        |e| parents.get(e).ok().map(|c| c.0),
        |e| selection.entities.contains(&e),
    );
    let positions: Vec<Vec3> = topmost
        .iter()
        .filter_map(|&e| transforms.get(e).ok().map(GlobalTransform::translation))
        .collect();
    if positions.is_empty() {
        return;
    }
    let gizmo_pos = centroid(&positions);

    // Scale is inherently local, so force local orientation so handles match transform.scale axes
    let effective_space = if *mode == ActiveTool::Scale {
        &GizmoSpace::Local
    } else {
        &space
    };
    let rotation = if topmost.len() == 1 {
        transforms
            .get(topmost[0])
            .ok()
            .map(|gt| gizmo_rotation(gt, effective_space))
            .unwrap_or(Quat::IDENTITY)
    } else {
        Quat::IDENTITY
    };

    let scale = gizmo_world_scale(projection, cam_tf, gizmo_pos);

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
            ActiveTool::Translate | ActiveTool::Scale => {
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
            ActiveTool::Rotate => point_to_ring_screen_dist(
                viewport_cursor,
                camera,
                cam_tf,
                gizmo_pos,
                *dir,
                ROTATE_RING_RADIUS * scale,
            ),
            ActiveTool::Select => continue,
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
                   the toolbar's `ActiveTool` resource and the click-time \
                   `GizmoHoverState`.",
    modal = true,
    allows_undo = true,
    cancel = cancel_gizmo_drag,
)]
pub fn gizmo_drag(
    _: In<OperatorParameters>,
    selection: Res<Selection>,
    mut transforms: Query<(&GlobalTransform, &mut Transform), With<Selected>>,
    parents: Query<&ChildOf>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_ctx: GizmoViewportCtx,
    mut cursor_query: Query<&mut CursorOptions, With<Window>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mode: Res<ActiveTool>,
    space: Res<GizmoSpace>,
    hover: Res<GizmoHoverState>,
    mut drag_state: ResMut<GizmoDragState>,
    snap_settings: Res<SnapSettings>,
    modal: Option<Single<Entity, With<ActiveModalOperator>>>,
) -> OperatorResult {
    let cursor_pos = viewport_ctx.cursor.get()?;
    // First-frame: pick the active (hovered) viewport. Subsequent
    // frames: use the captured one so the drag stays attached even
    // if the cursor strays into a different viewport.
    let (camera_entity, viewport_entity) = if modal.is_none() {
        let camera_entity = viewport_ctx.active_viewport.camera?;
        let viewport_entity = viewport_ctx.active_viewport.ui_node?;
        (camera_entity, viewport_entity)
    } else {
        let Some(camera_entity) = drag_state.camera else {
            return OperatorResult::Finished;
        };
        let Some(viewport_entity) = drag_state.viewport else {
            return OperatorResult::Finished;
        };
        (camera_entity, viewport_entity)
    };
    let (camera, cam_tf) = camera_query.get(camera_entity)?;
    // Bounds-check on the first frame so a press that misses the
    // viewport doesn't grab the gizmo. Once the modal is running the
    // cursor belongs to the drag, so accept positions outside the
    // viewport rectangle. Otherwise dragging across into a sibling
    // viewport (or off the panel entirely) returns `None` here, the
    // operator cancels, and the cancel handler restores the start
    // transform, which the user sees as a snap-back.
    let viewport_cursor = if modal.is_none() {
        window_to_viewport_cursor_for(
            cursor_pos,
            camera,
            viewport_entity,
            &viewport_ctx.viewport_query,
        )?
    } else {
        window_to_viewport_cursor_for_unbounded(
            cursor_pos,
            camera,
            viewport_entity,
            &viewport_ctx.viewport_query,
        )?
    };

    if modal.is_none() {
        let axis = hover.hovered_axis?;
        let target_entities = topmost_selected(
            &selection.entities,
            |e| parents.get(e).ok().map(|c| c.0),
            |e| selection.entities.contains(&e),
        );
        let mut targets = Vec::new();
        let mut positions = Vec::new();
        let mut local_positions = Vec::new();
        for e in target_entities {
            if let Ok((gt, t)) = transforms.get(e) {
                targets.push(GizmoTarget { entity: e, start_transform: *t });
                positions.push(gt.translation());
                local_positions.push(t.translation);
            }
        }
        if targets.is_empty() {
            return OperatorResult::Finished;
        }
        drag_state.active = true;
        drag_state.axis = Some(axis);
        drag_state.drag_start_screen = viewport_cursor;
        drag_state.targets = targets;
        drag_state.pivot = centroid(&positions);
        drag_state.pivot_local = centroid(&local_positions);
        drag_state.camera = Some(camera_entity);
        drag_state.viewport = Some(viewport_entity);
        drag_state.accumulated_delta = 0.0;
        if let Ok(mut cursor_opts) = cursor_query.single_mut() {
            cursor_opts.grab_mode = CursorGrabMode::Confined;
        }
        return OperatorResult::Running;
    }

    if mouse.just_released(MouseButton::Left) {
        // Undo is handled by the framework: the modal captured a
        // before-snapshot on start; returning Finished triggers an
        // after-snapshot + SnapshotDiff push.
        clear_gizmo_drag_state(&mut drag_state, &mut cursor_query);
        return OperatorResult::Finished;
    }

    if drag_state.targets.is_empty() {
        return OperatorResult::Finished;
    }
    let Some(axis) = drag_state.axis else {
        return OperatorResult::Finished;
    };

    // Compute axis direction from the single target's frame (local/world),
    // or world axes for multi-target drags. The immutable borrow via .get
    // is released before the mutation loop below.
    let rotation = if drag_state.targets.len() == 1 {
        let single_entity = drag_state.targets[0].entity;
        transforms
            .get(single_entity)
            .ok()
            .map(|(g, _)| gizmo_rotation(g, if *mode == ActiveTool::Scale { &GizmoSpace::Local } else { &space }))
            .unwrap_or(Quat::IDENTITY)
    } else {
        Quat::IDENTITY
    };
    let axis_dir = match axis {
        GizmoAxis::X => rotation * Vec3::X,
        GizmoAxis::Y => rotation * Vec3::Y,
        GizmoAxis::Z => rotation * Vec3::Z,
    };
    let pivot = drag_state.pivot;
    let pivot_local = drag_state.pivot_local;
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let mouse_delta = viewport_cursor - drag_state.drag_start_screen;

    if matches!(*mode, ActiveTool::Select) {
        return OperatorResult::Finished;
    }

    match *mode {
        ActiveTool::Translate => {
            // Use the pivot as the reference point for projecting the axis onto screen.
            let Ok(origin_screen) = camera.world_to_viewport(cam_tf, pivot) else {
                return OperatorResult::Running;
            };
            let Ok(axis_screen) = camera.world_to_viewport(cam_tf, pivot + axis_dir) else {
                return OperatorResult::Running;
            };
            let screen_axis = axis_screen - origin_screen;
            let len_sq = screen_axis.length_squared();
            if len_sq < EPSILON {
                return OperatorResult::Running;
            }
            let projected = mouse_delta.dot(screen_axis) / len_sq;
            let snapped = snap_settings.snap_translate_vec3_if(axis_dir * projected, ctrl);
            for t in &drag_state.targets {
                if let Ok((_, mut tf)) = transforms.get_mut(t.entity) {
                    tf.translation = t.start_transform.translation + snapped;
                }
            }
        }
        ActiveTool::Rotate => {
            let screen_axis = match axis {
                GizmoAxis::X => Vec2::Y,
                GizmoAxis::Y => Vec2::X,
                GizmoAxis::Z => -Vec2::X,
            };
            let raw_angle = mouse_delta.dot(screen_axis) * ROTATE_SENSITIVITY;
            let angle = snap_settings.snap_rotate_if(raw_angle, ctrl);
            let r = Quat::from_axis_angle(axis_dir, angle);
            for t in &drag_state.targets {
                if let Ok((_, mut tf)) = transforms.get_mut(t.entity) {
                    tf.translation = pivot_local + r * (t.start_transform.translation - pivot_local);
                    tf.rotation = r * t.start_transform.rotation;
                }
            }
        }
        ActiveTool::Scale => {
            let Ok(origin_screen) = camera.world_to_viewport(cam_tf, pivot) else {
                return OperatorResult::Running;
            };
            let Ok(axis_screen) = camera.world_to_viewport(cam_tf, pivot + axis_dir) else {
                return OperatorResult::Running;
            };
            let screen_axis = (axis_screen - origin_screen).normalize_or_zero();
            let projected = mouse_delta.dot(screen_axis) * SCALE_SENSITIVITY;
            let mut factor = Vec3::ONE;
            match axis {
                GizmoAxis::X => factor.x = 1.0 + projected,
                GizmoAxis::Y => factor.y = 1.0 + projected,
                GizmoAxis::Z => factor.z = 1.0 + projected,
            }
            for t in &drag_state.targets {
                if let Ok((_, mut tf)) = transforms.get_mut(t.entity) {
                    let offset = t.start_transform.translation - pivot_local;
                    tf.translation = pivot_local + factor * offset;
                    // Clamp the final scale (not the factor) so small start
                    // scales cannot be driven below the floor.
                    let scaled = (t.start_transform.scale * factor).max(Vec3::splat(MIN_SCALE));
                    tf.scale = snap_settings.snap_scale_vec3_if(scaled, ctrl);
                }
            }
        }
        ActiveTool::Select => {}
    }
    OperatorResult::Running
}

fn cancel_gizmo_drag(
    mut drag_state: ResMut<GizmoDragState>,
    mut transforms: Query<&mut Transform, With<Selected>>,
    mut cursor_query: Query<&mut CursorOptions, With<Window>>,
) {
    for t in &drag_state.targets {
        if let Ok(mut transform) = transforms.get_mut(t.entity) {
            *transform = t.start_transform;
        }
    }
    clear_gizmo_drag_state(&mut drag_state, &mut cursor_query);
}

fn clear_gizmo_drag_state(
    drag_state: &mut GizmoDragState,
    cursor_query: &mut Query<&mut CursorOptions, With<Window>>,
) {
    drag_state.active = false;
    drag_state.axis = None;
    drag_state.targets.clear();
    drag_state.camera = None;
    drag_state.viewport = None;
    if let Ok(mut cursor_opts) = cursor_query.single_mut() {
        cursor_opts.grab_mode = CursorGrabMode::None;
    }
}

fn draw_gizmos(
    mut gizmos: Gizmos<TransformGizmoGroup>,
    selection: Res<Selection>,
    transforms: Query<&GlobalTransform, With<Selected>>,
    parents: Query<&ChildOf>,
    camera_query: Query<(Entity, &GlobalTransform, &Projection), With<MainViewportCamera>>,
    active: Res<crate::viewport::ActiveViewport>,
    mode: Res<ActiveTool>,
    space: Res<GizmoSpace>,
    hover: Res<GizmoHoverState>,
    drag_state: Res<GizmoDragState>,
    modal: Res<ModalTransformState>,
    edit_mode: Res<crate::brush::EditMode>,
) {
    if matches!(*mode, ActiveTool::Select) {
        return;
    }

    // Hide gizmo during modal operations or brush edit mode
    if modal.active.is_some() || *edit_mode != crate::brush::EditMode::Object {
        return;
    }

    let topmost = topmost_selected(
        &selection.entities,
        |e| parents.get(e).ok().map(|c| c.0),
        |e| selection.entities.contains(&e),
    );
    let positions: Vec<Vec3> = topmost
        .iter()
        .filter_map(|&e| transforms.get(e).ok().map(GlobalTransform::translation))
        .collect();
    if positions.is_empty() {
        return;
    }
    // Live centroid every frame so the gizmo tracks the selection during a
    // drag (translate follows; rotate/scale keep the centroid at the pivot).
    // Reading the targets' GlobalTransform glues the gizmo to the meshes,
    // which read the same transforms.
    let gizmo_pos = centroid(&positions);

    // Multi-viewport: scale the gizmo by the active (hovered)
    // viewport's camera, falling back to any camera. The single
    // Gizmos pass renders into every viewport, so the size will be
    // visually correct in the hovered viewport and approximate in
    // the others until the cursor moves.
    let cam = active
        .camera
        .and_then(|e| camera_query.get(e).ok())
        .or_else(|| camera_query.iter().next());
    let Some((_, cam_tf, projection)) = cam else {
        return;
    };

    let effective_space = if *mode == ActiveTool::Scale {
        &GizmoSpace::Local
    } else {
        &space
    };
    let rotation = if topmost.len() == 1 {
        transforms
            .get(topmost[0])
            .ok()
            .map(|gt| gizmo_rotation(gt, effective_space))
            .unwrap_or(Quat::IDENTITY)
    } else {
        Quat::IDENTITY
    };

    let scale = gizmo_world_scale(projection, cam_tf, gizmo_pos);

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
        ActiveTool::Translate => {
            gizmos
                .arrow(
                    gizmo_pos + right * (AXIS_START_OFFSET * scale),
                    gizmo_pos + right * (AXIS_LENGTH * scale),
                    x_color,
                )
                .with_tip_length(AXIS_TIP_LENGTH * scale);
            gizmos
                .arrow(
                    gizmo_pos + up * (AXIS_START_OFFSET * scale),
                    gizmo_pos + up * (AXIS_LENGTH * scale),
                    y_color,
                )
                .with_tip_length(AXIS_TIP_LENGTH * scale);
            gizmos
                .arrow(
                    gizmo_pos + forward * (AXIS_START_OFFSET * scale),
                    gizmo_pos + forward * (AXIS_LENGTH * scale),
                    z_color,
                )
                .with_tip_length(AXIS_TIP_LENGTH * scale);
        }
        ActiveTool::Rotate => {
            // Draw rotation rings
            gizmos.circle(
                Isometry3d::new(gizmo_pos, Quat::from_rotation_arc(Vec3::Z, right)),
                ROTATE_RING_RADIUS * scale,
                x_color,
            );
            gizmos.circle(
                Isometry3d::new(gizmo_pos, Quat::from_rotation_arc(Vec3::Z, up)),
                ROTATE_RING_RADIUS * scale,
                y_color,
            );
            gizmos.circle(
                Isometry3d::new(gizmo_pos, Quat::from_rotation_arc(Vec3::Z, forward)),
                ROTATE_RING_RADIUS * scale,
                z_color,
            );
        }
        ActiveTool::Scale => {
            // Draw scale handles: lines with cubes at the end
            let cube_half = SCALE_CUBE_SIZE * scale;
            for (dir, color) in [(right, x_color), (up, y_color), (forward, z_color)] {
                let end = gizmo_pos + dir * (AXIS_LENGTH * scale);
                gizmos.line(gizmo_pos + dir * (AXIS_START_OFFSET * scale), end, color);
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
        ActiveTool::Select => {}
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

/// Entities to transform as a group: the selected entities minus any whose
/// ancestor is also selected (so a child moves once, via its parent, not
/// twice). `ancestor_of` yields an entity's parent if any; `is_selected`
/// reports selection membership.
fn topmost_selected(
    selected: &[Entity],
    ancestor_of: impl Fn(Entity) -> Option<Entity>,
    is_selected: impl Fn(Entity) -> bool,
) -> Vec<Entity> {
    selected
        .iter()
        .copied()
        .filter(|&e| {
            let mut cur = ancestor_of(e);
            while let Some(a) = cur {
                if is_selected(a) {
                    return false;
                }
                cur = ancestor_of(a);
            }
            true
        })
        .collect()
}

/// Mean of a set of world positions. Empty input returns the origin.
fn centroid(positions: &[Vec3]) -> Vec3 {
    if positions.is_empty() {
        return Vec3::ZERO;
    }
    positions.iter().copied().sum::<Vec3>() / positions.len() as f32
}

#[cfg(test)]
mod central_gizmo_tests {
    use super::*;

    #[test]
    fn topmost_excludes_selected_descendants() {
        let parent = Entity::from_raw_u32(1).unwrap();
        let child = Entity::from_raw_u32(2).unwrap();
        let other = Entity::from_raw_u32(3).unwrap();
        let selected = [parent, child, other];
        let is_selected = |e: Entity| selected.contains(&e);
        let ancestor = |e: Entity| if e == child { Some(parent) } else { None };
        let out = topmost_selected(&selected, ancestor, is_selected);
        // Order-independent: child is excluded (its parent is selected); the
        // other two survive. Avoids depending on `Entity`'s sort order.
        assert_eq!(out.len(), 2);
        assert!(out.contains(&parent));
        assert!(out.contains(&other));
        assert!(!out.contains(&child));
    }

    #[test]
    fn centroid_is_mean_of_positions() {
        let c = centroid(&[Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0), Vec3::new(0.0, 3.0, 0.0)]);
        assert!((c - Vec3::new(2.0 / 3.0, 1.0, 0.0)).length() < 1e-6);
    }
}
