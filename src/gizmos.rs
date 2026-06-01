use bevy::{
    ecs::system::SystemParam,
    feathers::cursor::{EntityCursor, OverrideCursor},
    prelude::*,
    ui::UiGlobalTransform,
    window::{CursorGrabMode, CursorOptions, SystemCursorIcon},
};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;

use crate::active_tool::ActiveTool;
use crate::brush::BrushDragCapture;
use crate::brush_drag_ops::restore_captures;
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

/// Read params for the brush-edit gizmo, shared by hover and draw so both
/// place the gizmo at the same point. Bundling keeps each system under
/// Bevy's system-param ceiling.
#[derive(SystemParam)]
pub(crate) struct EditGizmoCtx<'w, 's> {
    caches: Query<'w, 's, &'static crate::brush::BrushMeshCache>,
    globals: Query<'w, 's, &'static GlobalTransform>,
    selection: Res<'w, crate::brush::BrushSelection>,
    drag_state: Res<'w, EditGizmoDragState>,
}

impl EditGizmoCtx<'_, '_> {
    /// World centroid of the selected sub-elements across every edit brush,
    /// or `None` if nothing is selected.
    fn sub_element_centroid(&self) -> Option<Vec3> {
        let positions = sub_element_world_positions(self.selection.edit_brushes(), |e| {
            let cache = self.caches.get(e).ok()?;
            let global = self.globals.get(e).ok()?;
            let sub = self.selection.sub(e)?;
            Some((
                cache.vertices.as_slice(),
                cache.face_polygons.as_slice(),
                sub,
                *global,
            ))
        });
        if positions.is_empty() {
            None
        } else {
            Some(centroid(&positions))
        }
    }
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
/// Pixel radius around the projected gizmo center that grabs the uniform
/// scale handle. Smaller than `AXIS_START_OFFSET` projected, so it never
/// competes with the axis arms.
const UNIFORM_HANDLE_RADIUS: f32 = 12.0;
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
    /// Center handle that scales every axis by the same factor about the
    /// pivot. Only hovered and dragged while the Scale tool is active.
    Uniform,
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

/// Drag state for the central gizmo while in brush-edit mode. Transforms the
/// selected sub-elements across every edit brush about their shared world
/// centroid. Separate from [`GizmoDragState`] so object-mode dragging is
/// untouched.
#[derive(Resource, Default)]
pub(crate) struct EditGizmoDragState {
    pub(crate) active: bool,
    axis: Option<GizmoAxis>,
    drag_start_screen: Vec2,
    /// World centroid of all captured start positions across every brush.
    pivot: Vec3,
    /// Where the gizmo is drawn this frame. Held stable at the pivot during
    /// scale / rotate (so grid-snapped vertices jumping between grid lines do
    /// not make the centroid, and thus the gizmo, bounce); follows the
    /// translation delta during a translate drag.
    pub(crate) draw_pos: Vec3,
    /// Camera entity of the viewport this drag started in.
    camera: Option<Entity>,
    /// `SceneViewport` UI-node entity of the same viewport.
    viewport: Option<Entity>,
    captures: Vec<BrushDragCapture>,
}

pub struct TransformGizmosPlugin;

impl Plugin for TransformGizmosPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActiveTool>()
            .init_resource::<GizmoSpace>()
            .init_resource::<GizmoDragState>()
            .init_resource::<EditGizmoDragState>()
            .init_resource::<GizmoHoverState>()
            .init_gizmo_group::<TransformGizmoGroup>()
            .add_systems(Startup, configure_transform_gizmos)
            .add_systems(
                Update,
                (
                    handle_gizmo_hover,
                    gizmo_drag_invoke_trigger,
                    gizmo_drag_edit_invoke_trigger,
                )
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
    ctx.register_operator::<GizmoDragOp>()
        .register_operator::<GizmoDragEditOp>();
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
    edit_ctx: EditGizmoCtx,
) {
    hover.hovered_axis = None;

    if drag_state.active
        || edit_ctx.drag_state.active
        || modal.active.is_some()
        || draw_state.active.is_some()
    {
        return;
    }

    // Object mode hovers the object gizmo; brush-edit mode hovers the
    // same gizmo positioned at the selected sub-element centroid. Other
    // edit modes (Physics) show no central gizmo.
    let in_brush_edit = matches!(*edit_mode, crate::brush::EditMode::BrushEdit(_));
    if *edit_mode != crate::brush::EditMode::Object && !in_brush_edit {
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

    let (gizmo_pos, rotation) = if in_brush_edit {
        // Sub-elements are points with no per-target frame, so world axes.
        let Some(pos) = edit_ctx.sub_element_centroid() else {
            return;
        };
        (pos, Quat::IDENTITY)
    } else {
        let Some(placement) =
            object_gizmo_placement(&selection, &transforms, &parents, &mode, &space)
        else {
            return;
        };
        placement
    };

    let scale = gizmo_world_scale(projection, cam_tf, gizmo_pos);

    // The center handle exists only for scaling. It takes priority over the
    // axis arms: the arms start `AXIS_START_OFFSET` away from the center, so
    // the small center radius never overlaps them. A hit here short-circuits
    // the per-axis test below.
    if matches!(*mode, ActiveTool::Scale)
        && let Ok(center_screen) = camera.world_to_viewport(cam_tf, gizmo_pos)
        && (viewport_cursor - center_screen).length() <= UNIFORM_HANDLE_RADIUS
    {
        hover.hovered_axis = Some(GizmoAxis::Uniform);
        return;
    }

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

/// LMB on a hovered gizmo axis in brush-edit mode dispatches
/// `gizmo.drag_edit`, which transforms the selected sub-elements. Object-mode
/// dispatch stays in [`gizmo_drag_invoke_trigger`]; this runs after it so the
/// two never fire on the same press (the mode gates are mutually exclusive).
fn gizmo_drag_edit_invoke_trigger(
    mouse: Res<ButtonInput<MouseButton>>,
    hover: Res<GizmoHoverState>,
    drag_state: Res<EditGizmoDragState>,
    object_drag: Res<GizmoDragState>,
    modal: Res<ModalTransformState>,
    edit_mode: Res<crate::brush::EditMode>,
    draw_state: Res<crate::draw_brush::DrawBrushState>,
    mode: Res<ActiveTool>,
    brush_selection: Res<crate::brush::BrushSelection>,
    mut commands: Commands,
) {
    let has_sub_selection = brush_selection.brushes.values().any(|s| {
        !s.vertices.is_empty() || !s.edges.is_empty() || !s.faces.is_empty()
    });
    if drag_state.active
        || object_drag.active
        || !mouse.just_pressed(MouseButton::Left)
        || hover.hovered_axis.is_none()
        || matches!(*mode, ActiveTool::Select)
        || !matches!(*edit_mode, crate::brush::EditMode::BrushEdit(_))
        || !has_sub_selection
        || modal.active.is_some()
        || draw_state.active.is_some()
    {
        return;
    }
    commands.queue(|world: &mut World| {
        let _ = world
            .operator(GizmoDragEditOp::ID)
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
    // if the cursor strays into a different viewport. A captured
    // viewport that is no longer available finishes the drag; a missing
    // active viewport on the first frame cancels it. Dragging across
    // into a sibling viewport (or off the panel) yields no viewport
    // cursor, cancelling the drag so the cancel handler restores the
    // start transform, which the user sees as a snap-back.
    let Some((camera_entity, viewport_entity)) = resolve_drag_viewport(
        modal.is_some(),
        &viewport_ctx,
        drag_state.camera,
        drag_state.viewport,
    ) else {
        return if modal.is_some() {
            OperatorResult::Finished
        } else {
            OperatorResult::Cancelled
        };
    };
    let (camera, cam_tf) = camera_query.get(camera_entity)?;
    let viewport_cursor = resolve_viewport_cursor(
        modal.is_some(),
        cursor_pos,
        camera,
        viewport_entity,
        &viewport_ctx.viewport_query,
    )?;

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
        // Uniform is scale-only and orientation-independent; it never reaches
        // the directional translate/rotate paths.
        GizmoAxis::Uniform => Vec3::ZERO,
    };
    let pivot = drag_state.pivot;
    let pivot_local = drag_state.pivot_local;
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let mouse_delta = viewport_cursor - drag_state.drag_start_screen;

    match *mode {
        ActiveTool::Translate => {
            // Use the pivot as the reference point for projecting the axis onto screen.
            let Some(projected) =
                translate_axis_amount(mouse_delta, camera, cam_tf, pivot, axis_dir)
            else {
                return OperatorResult::Running;
            };
            let snapped = snap_settings.snap_translate_vec3_if(axis_dir * projected, ctrl);
            for t in &drag_state.targets {
                if let Ok((_, mut tf)) = transforms.get_mut(t.entity) {
                    tf.translation = t.start_transform.translation + snapped;
                }
            }
        }
        ActiveTool::Rotate => {
            let screen_axis = rotate_screen_axis(axis);
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
            let Some(factor) = scale_factor(axis, mouse_delta, camera, cam_tf, pivot, axis_dir)
            else {
                return OperatorResult::Running;
            };
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
        ActiveTool::Select => return OperatorResult::Finished,
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

/// Read-only params the edit-drag operator needs on the first frame to
/// capture each brush's sub-selection and on cancel to rebuild meshes.
/// Bundled to keep `gizmo_drag_edit` under Bevy's system-param ceiling.
#[derive(SystemParam)]
struct EditGizmoBrushParams<'w, 's> {
    caches: Query<'w, 's, &'static crate::brush::BrushMeshCache>,
    globals: Query<'w, 's, &'static GlobalTransform>,
    brushes: Query<'w, 's, &'static mut jackdaw_jsn::Brush>,
    halfedges: Query<'w, 's, &'static mut crate::brush::BrushHalfedge>,
}

#[operator(
    id = "gizmo.drag_edit",
    label = "Gizmo Drag (Edit Mode)",
    description = "Drag the central transform gizmo to translate / rotate / scale the \
                   selected vertices, edges, and faces across every edit brush about \
                   their shared centroid. Modal: commits on LMB release, cancels on \
                   Escape (restoring every brush). Tool and axis come from the \
                   toolbar's `ActiveTool` resource and the click-time \
                   `GizmoHoverState`.",
    modal = true,
    allows_undo = true,
    cancel = cancel_gizmo_edit_drag,
)]
pub fn gizmo_drag_edit(
    _: In<OperatorParameters>,
    brush_selection: Res<crate::brush::BrushSelection>,
    mut brush_params: EditGizmoBrushParams,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_ctx: GizmoViewportCtx,
    mut cursor_query: Query<&mut CursorOptions, With<Window>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mode: Res<ActiveTool>,
    hover: Res<GizmoHoverState>,
    mut drag_state: ResMut<EditGizmoDragState>,
    snap_settings: Res<SnapSettings>,
    modal: Option<Single<Entity, With<ActiveModalOperator>>>,
    mut override_cursor: ResMut<OverrideCursor>,
) -> OperatorResult {
    let cursor_pos = viewport_ctx.cursor.get()?;
    // First frame picks the active (hovered) viewport; subsequent frames use
    // the captured one so the drag stays attached even if the cursor strays
    // into a sibling viewport. A captured viewport that is no longer available
    // finishes the drag; a missing active viewport on the first frame cancels
    // it.
    let Some((camera_entity, viewport_entity)) = resolve_drag_viewport(
        modal.is_some(),
        &viewport_ctx,
        drag_state.camera,
        drag_state.viewport,
    ) else {
        return if modal.is_some() {
            OperatorResult::Finished
        } else {
            OperatorResult::Cancelled
        };
    };
    let (camera, cam_tf) = camera_query.get(camera_entity)?;
    let viewport_cursor = resolve_viewport_cursor(
        modal.is_some(),
        cursor_pos,
        camera,
        viewport_entity,
        &viewport_ctx.viewport_query,
    )?;

    if modal.is_none() {
        let axis = hover.hovered_axis?;
        let captures = crate::brush_drag_ops::capture_edit_brushes(
            &brush_selection,
            &brush_params.brushes,
            &brush_params.caches,
            &brush_params.globals,
            selected_sub_vertices,
        );
        if captures.is_empty() {
            return OperatorResult::Finished;
        }
        let all_start_world: Vec<Vec3> =
            captures.iter().flat_map(|c| c.start_world.iter().copied()).collect();
        drag_state.active = true;
        drag_state.axis = Some(axis);
        drag_state.drag_start_screen = viewport_cursor;
        drag_state.pivot = centroid(&all_start_world);
        drag_state.draw_pos = drag_state.pivot;
        drag_state.camera = Some(camera_entity);
        drag_state.viewport = Some(viewport_entity);
        drag_state.captures = captures;
        if let Ok(mut cursor_opts) = cursor_query.single_mut() {
            cursor_opts.grab_mode = CursorGrabMode::Confined;
        }
        override_cursor.0 = Some(EntityCursor::System(SystemCursorIcon::Grabbing));
        return OperatorResult::Running;
    }

    if mouse.just_released(MouseButton::Left) {
        // The framework captured a before-snapshot on start; returning
        // Finished triggers the after-snapshot + SnapshotDiff push, so one
        // Undo restores every brush touched.
        clear_gizmo_edit_drag_state(&mut drag_state, &mut cursor_query);
        clear_gizmo_grab_cursor(&mut override_cursor);
        return OperatorResult::Finished;
    }

    if drag_state.captures.is_empty() {
        return OperatorResult::Finished;
    }
    let Some(axis) = drag_state.axis else {
        return OperatorResult::Finished;
    };

    // Sub-elements are points with no per-target frame, so the gizmo always
    // uses world axes (matching the multi-target object case).
    let axis_dir = match axis {
        GizmoAxis::X => Vec3::X,
        GizmoAxis::Y => Vec3::Y,
        GizmoAxis::Z => Vec3::Z,
        // Uniform is scale-only and orientation-independent; it never reaches
        // the directional translate/rotate paths.
        GizmoAxis::Uniform => Vec3::ZERO,
    };
    let pivot = drag_state.pivot;
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let mouse_delta = viewport_cursor - drag_state.drag_start_screen;

    // Map each captured start world position to its new world position, then
    // back to that brush's local space. The plan is built from immutable
    // capture data first; the mutable Brush / Halfedge writes happen after.
    let mut plan: Vec<(Entity, Vec<Vec3>)> = Vec::with_capacity(drag_state.captures.len());
    // Where to draw the gizmo this frame. Scale / rotate keep it at the
    // pivot; translate moves it with the selection.
    let mut new_draw_pos = pivot;

    match *mode {
        ActiveTool::Translate => {
            let Some(projected) =
                translate_axis_amount(mouse_delta, camera, cam_tf, pivot, axis_dir)
            else {
                return OperatorResult::Running;
            };
            let world_delta = snap_settings.snap_translate_vec3_if(axis_dir * projected, ctrl);
            new_draw_pos = pivot + world_delta;
            for capture in &drag_state.captures {
                let new_local: Vec<Vec3> = capture
                    .start_world
                    .iter()
                    .map(|&w| capture.start_world_to_local.transform_point3(w + world_delta))
                    .collect();
                plan.push((capture.entity, new_local));
            }
        }
        ActiveTool::Rotate => {
            let screen_axis = rotate_screen_axis(axis);
            let raw_angle = mouse_delta.dot(screen_axis) * ROTATE_SENSITIVITY;
            let angle = snap_settings.snap_rotate_if(raw_angle, ctrl);
            let r = Quat::from_axis_angle(axis_dir, angle);
            for capture in &drag_state.captures {
                let new_local: Vec<Vec3> = capture
                    .start_world
                    .iter()
                    .map(|&w| {
                        let new_world = pivot + r * (w - pivot);
                        capture.start_world_to_local.transform_point3(new_world)
                    })
                    .collect();
                plan.push((capture.entity, new_local));
            }
        }
        ActiveTool::Scale => {
            let Some(factor) = scale_factor(axis, mouse_delta, camera, cam_tf, pivot, axis_dir)
            else {
                return OperatorResult::Running;
            };
            // When scale snapping is active (Ctrl flips it), land the
            // resulting vertex positions on the grid rather than snapping the
            // scale factor, so edited geometry aligns to the visible grid.
            let snap_to_grid = snap_settings.scale_active(ctrl);
            for capture in &drag_state.captures {
                let new_local: Vec<Vec3> = capture
                    .start_world
                    .iter()
                    .map(|&w| {
                        let mut new_world = pivot + factor * (w - pivot);
                        if snap_to_grid {
                            new_world = snap_settings.snap_position_to_grid(new_world);
                        }
                        capture.start_world_to_local.transform_point3(new_world)
                    })
                    .collect();
                plan.push((capture.entity, new_local));
            }
        }
        ActiveTool::Select => return OperatorResult::Finished,
    }

    drag_state.draw_pos = new_draw_pos;

    for (entity, new_local_positions) in plan {
        let Some(capture) = drag_state.captures.iter().find(|c| c.entity == entity) else {
            continue;
        };
        let Ok(mut brush) = brush_params.brushes.get_mut(entity) else {
            continue;
        };
        let mut halfedge_opt = brush_params.halfedges.get_mut(entity).ok();
        crate::brush_drag_ops::apply_vertex_deltas(
            &mut brush,
            halfedge_opt.as_deref_mut(),
            &capture.start_brush,
            &capture.start_all_vertices,
            &capture.start_face_polygons,
            &capture.indices,
            &new_local_positions,
        );
    }
    OperatorResult::Running
}

/// Restore every captured brush to its drag-start topology, then clear state.
/// Shares [`restore_captures`] with the direct vertex / edge drag cancels.
fn cancel_gizmo_edit_drag(
    mut drag_state: ResMut<EditGizmoDragState>,
    mut brushes: Query<&mut jackdaw_jsn::Brush>,
    mut halfedges: Query<&mut crate::brush::BrushHalfedge>,
    mut cursor_query: Query<&mut CursorOptions, With<Window>>,
    mut override_cursor: ResMut<OverrideCursor>,
) {
    restore_captures(&drag_state.captures, &mut brushes, &mut halfedges);
    clear_gizmo_edit_drag_state(&mut drag_state, &mut cursor_query);
    clear_gizmo_grab_cursor(&mut override_cursor);
}

/// Clear the grabbing cursor when an edit-gizmo drag ends, leaving any cursor
/// owned by another system untouched.
fn clear_gizmo_grab_cursor(override_cursor: &mut OverrideCursor) {
    if override_cursor.0 == Some(EntityCursor::System(SystemCursorIcon::Grabbing)) {
        override_cursor.0 = None;
    }
}

fn clear_gizmo_edit_drag_state(
    drag_state: &mut EditGizmoDragState,
    cursor_query: &mut Query<&mut CursorOptions, With<Window>>,
) {
    drag_state.active = false;
    drag_state.axis = None;
    drag_state.captures.clear();
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
    edit_ctx: EditGizmoCtx,
) {
    if matches!(*mode, ActiveTool::Select) {
        return;
    }

    let in_brush_edit = matches!(*edit_mode, crate::brush::EditMode::BrushEdit(_));
    // Object mode draws the object gizmo; brush-edit mode draws the same gizmo
    // at the selected sub-element centroid, coexisting with the per-element
    // handles from `draw_brush_edit_gizmos`. Other modes draw nothing.
    if modal.active.is_some() || (*edit_mode != crate::brush::EditMode::Object && !in_brush_edit) {
        return;
    }

    // Live centroid every frame so the gizmo tracks the selection during a
    // drag (translate follows; rotate/scale keep the centroid at the pivot).
    // Reading the targets' GlobalTransform / live cache glues the gizmo to
    // the meshes, which read the same data.
    let (gizmo_pos, rotation) = if in_brush_edit {
        // Sub-elements are points with no per-target frame, so world axes.
        // During a drag use the operator's stable draw position (held at the
        // pivot for scale / rotate) so grid-snapped vertices do not make the
        // live centroid, and the gizmo, bounce; otherwise track the live
        // sub-element centroid.
        let pos = if edit_ctx.drag_state.active {
            edit_ctx.drag_state.draw_pos
        } else {
            match edit_ctx.sub_element_centroid() {
                Some(p) => p,
                None => return,
            }
        };
        (pos, Quat::IDENTITY)
    } else {
        let Some(placement) =
            object_gizmo_placement(&selection, &transforms, &parents, &mode, &space)
        else {
            return;
        };
        placement
    };

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

    let scale = gizmo_world_scale(projection, cam_tf, gizmo_pos);

    let right = rotation * Vec3::X;
    let up = rotation * Vec3::Y;
    let forward = rotation * Vec3::Z;

    let active_axis = if drag_state.active {
        drag_state.axis
    } else if edit_ctx.drag_state.active {
        edit_ctx.drag_state.axis
    } else {
        hover.hovered_axis
    };

    let dragging = drag_state.active || edit_ctx.drag_state.active;
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
            // Draw scale handles: lines with cubes at the end.
            let cube_half = SCALE_CUBE_SIZE * scale;
            for (dir, color) in [(right, x_color), (up, y_color), (forward, z_color)] {
                let end = gizmo_pos + dir * (AXIS_LENGTH * scale);
                gizmos.line(gizmo_pos + dir * (AXIS_START_OFFSET * scale), end, color);
                draw_wire_cube(&mut gizmos, end, cube_half, color);
            }
            // Center handle for uniform scale, world-axis aligned (it is
            // orientation-independent) and the same size as the axis tips.
            let uniform_color = axis_color(GizmoAxis::Uniform, active_axis, dragging);
            draw_wire_cube(&mut gizmos, gizmo_pos, cube_half, uniform_color);
        }
        ActiveTool::Select => {}
    }
}

/// Draw a world-axis-aligned wireframe cube centered at `center` with the
/// given half-extent, using gizmo line segments.
fn draw_wire_cube(
    gizmos: &mut Gizmos<TransformGizmoGroup>,
    center: Vec3,
    half: f32,
    color: Color,
) {
    let x = Vec3::X * half;
    let y = Vec3::Y * half;
    let z = Vec3::Z * half;
    let corners = [
        center - x - y - z,
        center + x - y - z,
        center + x + y - z,
        center - x + y - z,
        center - x - y + z,
        center + x - y + z,
        center + x + y + z,
        center - x + y + z,
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
        // Center handle: neutral light gray at rest, white when active so it
        // brightens on hover/drag like the axis handles.
        GizmoAxis::Uniform => (
            Color::srgba(0.8, 0.8, 0.8, 0.6),
            Color::srgba(1.0, 1.0, 1.0, 1.0),
        ),
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

/// Topology vertex indices touched by a sub-selection, given the brush's
/// face polygons. Unions the selected vertices, both ends of each selected
/// edge, and every vertex of each selected face. Deduplicated, order-stable
/// (first appearance wins).
pub fn selected_sub_vertices(
    sub: &crate::brush::BrushSubSelection,
    face_polygons: &[Vec<usize>],
) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    let push = |v: usize, out: &mut Vec<usize>| {
        if !out.contains(&v) {
            out.push(v);
        }
    };
    for &v in &sub.vertices {
        push(v, &mut out);
    }
    for &(a, b) in &sub.edges {
        push(a, &mut out);
        push(b, &mut out);
    }
    for &f in &sub.faces {
        if let Some(polygon) = face_polygons.get(f) {
            for &v in polygon {
                push(v, &mut out);
            }
        }
    }
    out
}

/// World positions of every selected sub-element vertex across the edit
/// brushes. For each brush, `brush_data(e)` yields its local vertices,
/// face polygons, sub-selection, and world transform; brushes that yield
/// `None` or have an empty sub-selection contribute nothing. Used by the
/// edit-mode gizmo's hover, draw, and drag-start so all three agree on
/// the gizmo position.
fn sub_element_world_positions<'a>(
    edit_brushes: impl IntoIterator<Item = Entity>,
    brush_data: impl Fn(
        Entity,
    ) -> Option<(
        &'a [Vec3],
        &'a [Vec<usize>],
        &'a crate::brush::BrushSubSelection,
        GlobalTransform,
    )>,
) -> Vec<Vec3> {
    let mut positions = Vec::new();
    for e in edit_brushes {
        let Some((vertices, face_polygons, sub, global)) = brush_data(e) else {
            continue;
        };
        for vi in selected_sub_vertices(sub, face_polygons) {
            if let Some(local) = vertices.get(vi) {
                positions.push(global.transform_point(*local));
            }
        }
    }
    positions
}

/// Maps a gizmo axis to the 2D screen-space direction whose signed projection
/// of the mouse delta drives a rotation about that axis.
fn rotate_screen_axis(axis: GizmoAxis) -> Vec2 {
    match axis {
        GizmoAxis::X => Vec2::Y,
        GizmoAxis::Y => Vec2::X,
        GizmoAxis::Z => -Vec2::X,
        // Uniform is scale-only and never reaches the rotate path.
        GizmoAxis::Uniform => Vec2::ZERO,
    }
}

/// Per-component scale factor for a gizmo drag. `Uniform` scales every axis by
/// the same amount, driven by a diagonal screen drag (up / right grows). For a
/// directional axis the mouse delta is projected onto the axis as seen on
/// screen. Returns `None` when the pivot or axis endpoint fails to project into
/// the viewport.
fn scale_factor(
    axis: GizmoAxis,
    mouse_delta: Vec2,
    camera: &Camera,
    cam_tf: &GlobalTransform,
    pivot: Vec3,
    axis_dir: Vec3,
) -> Option<Vec3> {
    if axis == GizmoAxis::Uniform {
        // Diagonal drag drives uniform scale so both horizontal and
        // vertical movement respond: up / right grows, down / left
        // shrinks. Signed and monotonic straight through the gizmo
        // center (a radial distance would re-grow once the cursor
        // passed back through the center). Screen Y is down-positive,
        // so up is negative Y.
        let dir = Vec2::new(1.0, -1.0).normalize_or_zero();
        let projected = mouse_delta.dot(dir) * SCALE_SENSITIVITY;
        Some(Vec3::splat(1.0 + projected))
    } else {
        let origin_screen = camera.world_to_viewport(cam_tf, pivot).ok()?;
        let axis_screen = camera.world_to_viewport(cam_tf, pivot + axis_dir).ok()?;
        let screen_axis = (axis_screen - origin_screen).normalize_or_zero();
        let projected = mouse_delta.dot(screen_axis) * SCALE_SENSITIVITY;
        let mut factor = Vec3::ONE;
        match axis {
            GizmoAxis::X => factor.x = 1.0 + projected,
            GizmoAxis::Y => factor.y = 1.0 + projected,
            GizmoAxis::Z => factor.z = 1.0 + projected,
            // Handled by the Uniform branch above.
            GizmoAxis::Uniform => {}
        }
        Some(factor)
    }
}

/// Signed distance the selection should move along `axis_dir` for the current
/// mouse delta, found by projecting the delta onto the axis as it appears on
/// screen. Returns `None` when the pivot or axis endpoint fails to project, or
/// when the projected axis is degenerate (zero length on screen).
fn translate_axis_amount(
    mouse_delta: Vec2,
    camera: &Camera,
    cam_tf: &GlobalTransform,
    pivot: Vec3,
    axis_dir: Vec3,
) -> Option<f32> {
    let origin_screen = camera.world_to_viewport(cam_tf, pivot).ok()?;
    let axis_screen = camera.world_to_viewport(cam_tf, pivot + axis_dir).ok()?;
    let screen_axis = axis_screen - origin_screen;
    let len_sq = screen_axis.length_squared();
    if len_sq < EPSILON {
        return None;
    }
    Some(mouse_delta.dot(screen_axis) / len_sq)
}

/// Camera and `SceneViewport` UI-node entities a gizmo drag should use this
/// frame. On the first frame (`modal_active` false) this is the active
/// (hovered) viewport; once the modal is running it is the viewport captured at
/// drag start, so the drag stays attached even if the cursor strays into a
/// sibling viewport. Returns `None` when the required viewport is unavailable.
fn resolve_drag_viewport(
    modal_active: bool,
    viewport_ctx: &GizmoViewportCtx,
    stored_camera: Option<Entity>,
    stored_viewport: Option<Entity>,
) -> Option<(Entity, Entity)> {
    if !modal_active {
        let camera_entity = viewport_ctx.active_viewport.camera?;
        let viewport_entity = viewport_ctx.active_viewport.ui_node?;
        Some((camera_entity, viewport_entity))
    } else {
        let camera_entity = stored_camera?;
        let viewport_entity = stored_viewport?;
        Some((camera_entity, viewport_entity))
    }
}

/// Viewport-local cursor position for a gizmo drag. The first frame is
/// bounds-checked so a press that misses the viewport does not grab the gizmo;
/// once the modal is running the cursor belongs to the drag, so positions
/// outside the viewport rectangle are accepted. Returns `None` when the cursor
/// is rejected.
fn resolve_viewport_cursor(
    modal_active: bool,
    cursor_pos: Vec2,
    camera: &Camera,
    viewport_entity: Entity,
    viewport_query: &Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
) -> Option<Vec2> {
    if !modal_active {
        window_to_viewport_cursor_for(cursor_pos, camera, viewport_entity, viewport_query)
    } else {
        window_to_viewport_cursor_for_unbounded(cursor_pos, camera, viewport_entity, viewport_query)
    }
}

/// Gizmo position and orientation for object mode: the centroid of the topmost
/// selected entities' world translations, with the orientation of the single
/// selected target (or identity for a multi-target group). The Scale tool
/// forces local orientation so handles align with the entity's scale axes.
/// Returns `None` when nothing selected contributes a position.
fn object_gizmo_placement(
    selection: &Selection,
    transforms: &Query<&GlobalTransform, With<Selected>>,
    parents: &Query<&ChildOf>,
    mode: &ActiveTool,
    space: &GizmoSpace,
) -> Option<(Vec3, Quat)> {
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
        return None;
    }
    // Scale is inherently local, so force local orientation so handles match transform.scale axes
    let effective_space = if *mode == ActiveTool::Scale {
        &GizmoSpace::Local
    } else {
        space
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
    Some((centroid(&positions), rotation))
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

    #[test]
    fn selected_sub_vertices_expands_and_dedups() {
        use crate::brush::BrushSubSelection;
        // Two triangular faces sharing edge (1, 2).
        let face_polygons = vec![vec![0, 1, 2], vec![2, 1, 3]];

        // A lone vertex.
        let sub = BrushSubSelection {
            vertices: vec![5],
            ..Default::default()
        };
        assert_eq!(selected_sub_vertices(&sub, &face_polygons), vec![5]);

        // An edge yields both ends.
        let sub = BrushSubSelection {
            edges: vec![(1, 2)],
            ..Default::default()
        };
        assert_eq!(selected_sub_vertices(&sub, &face_polygons), vec![1, 2]);

        // A face expands to its polygon's vertices.
        let sub = BrushSubSelection {
            faces: vec![0],
            ..Default::default()
        };
        assert_eq!(selected_sub_vertices(&sub, &face_polygons), vec![0, 1, 2]);

        // Overlap across the three lists is deduplicated, order-stable.
        let sub = BrushSubSelection {
            vertices: vec![1],
            edges: vec![(1, 2)],
            faces: vec![0, 1],
        };
        // vertices: 1; edge (1,2): 2; face 0: 0; face 1 (2,1,3): 3.
        assert_eq!(selected_sub_vertices(&sub, &face_polygons), vec![1, 2, 0, 3]);

        // Out-of-range face index is skipped without panicking.
        let sub = BrushSubSelection {
            faces: vec![99],
            ..Default::default()
        };
        assert!(selected_sub_vertices(&sub, &face_polygons).is_empty());
    }
}
