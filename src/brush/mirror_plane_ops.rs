//! The `mirror.plane.set` operator writes a mirror modifier's plane coordinate
//! on one brush-local axis. The `mirror.plane.drag` modal operator grabs a
//! hovered plane handle (see [`crate::brush::mirror_plane_overlay`]) and slides
//! the plane along its axis with snapping, writing `offset[axis]` live each
//! frame. Mutating the `ModifierStack` marks it `Changed`, which the existing
//! re-fold + AST sync + undo capture systems react to, so both operators only
//! write `offset` and return.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;
use jackdaw_geometry::ModifierStack;
use jackdaw_jsn::Brush;

use crate::brush::interaction::{VertexDragConstraint, compute_brush_drag_offset};
use crate::brush::mirror_plane_overlay::{MirrorPlaneHover, plane_handle_world};
use crate::core_extension::CoreExtensionInputContext;
use crate::draw_brush::{DrawBrushState, env_allows_brush_op};
use crate::keybind_focus::KeybindFocus;
use crate::selection::Selection;
use crate::snapping::SnapSettings;
use crate::viewport::ViewportCursor;

/// Set the plane coordinate of the first editor-enabled Mirror modifier on
/// `axis` for every selected brush. Available with a selected brush whose
/// stack carries an enabled Mirror entry.
#[operator(
    id = "mirror.plane.set",
    label = "Set Mirror Plane",
    is_available = can_edit_mirror_plane,
    allows_undo = true,
    params(
        axis(i64, doc = "Mirror axis: 0=x, 1=y, 2=z."),
        value(f64, doc = "Plane coordinate on that axis, in brush-local space."),
    ),
)]
pub(crate) fn mirror_plane_set(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    mut brushes: Query<&mut ModifierStack, With<Brush>>,
) -> OperatorResult {
    let (Some(axis), Some(value)) = (params.as_int("axis"), params.as_float("value")) else {
        return OperatorResult::Cancelled;
    };
    let axis = axis as usize;
    if axis > 2 {
        return OperatorResult::Cancelled;
    }
    let mut set = false;
    for &entity in &selection.entities {
        let Ok(mut stack) = brushes.get_mut(entity) else {
            continue;
        };
        let Some(mirror) = stack.first_enabled_mirror_mut() else {
            continue;
        };
        mirror.offset[axis] = value as f32;
        set = true;
    }
    if set {
        OperatorResult::Finished
    } else {
        OperatorResult::Cancelled
    }
}

/// A selected brush whose stack carries an editor-enabled Mirror entry, the
/// gate for `mirror.plane.set`.
pub(crate) fn can_edit_mirror_plane(
    keybind_focus: KeybindFocus,
    modal: Res<crate::modal_transform::ModalTransformState>,
    draw_state: Res<DrawBrushState>,
    selection: Res<Selection>,
    candidates: Query<&ModifierStack, With<Brush>>,
) -> bool {
    env_allows_brush_op(&keybind_focus, &modal, &draw_state)
        && selection.entities.iter().any(|&e| {
            candidates
                .get(e)
                .is_ok_and(|stack| stack.first_enabled_mirror().is_some())
        })
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<MirrorPlaneSetOp>();
    ctx.register_operator::<MirrorPlaneDragOp>();
    // No default keybind; the unbound action entities keep the ops bindable by
    // presets and user rebinds. `mirror.plane.drag` is dispatched by its invoke
    // trigger on a press over a handle, so it never needs a key of its own, but
    // the action keeps it discoverable in the rebind UI.
    ctx.action_for::<CoreExtensionInputContext, MirrorPlaneSetOp>();
    ctx.action_for::<CoreExtensionInputContext, MirrorPlaneDragOp>();
}

/// Nearest vertex coordinate on `axis` to `coord`, within `tolerance`
/// (world units, brush-local). `None` when no vertex is close enough.
pub fn resolve_axis_snap_to_geometry(
    vertices: &[Vec3],
    axis: usize,
    coord: f32,
    tolerance: f32,
) -> Option<f32> {
    vertices
        .iter()
        .map(|v| v[axis])
        .filter(|&c| (c - coord).abs() <= tolerance)
        .min_by(|a, b| (a - coord).abs().total_cmp(&(b - coord).abs()))
}

/// What the plane snapped to this frame, for viewport feedback.
pub enum PlaneSnap {
    /// Brush-local position of the snapped vertex.
    Vertex(Vec3),
    /// Brush-local position of the snapped edge midpoint.
    EdgeMidpoint(Vec3),
    Grid,
    Free,
}

/// Resolve a tentative brush-local plane coordinate on `axis` to a snapped
/// value, in priority order: nearest brush geometry (vertex, then edge
/// midpoint) within `tolerance` -> grid (when grid snapping is active) ->
/// free. Returns the resolved coordinate and what it locked to.
pub fn resolve_plane_snap(
    authored_vertices: &[Vec3],
    edges: &[(usize, usize)],
    axis: usize,
    coord: f32,
    tolerance: f32,
    grid_active: bool,
    grid: &SnapSettings,
) -> (f32, PlaneSnap) {
    if let Some(snapped) = resolve_axis_snap_to_geometry(authored_vertices, axis, coord, tolerance)
    {
        let vert = authored_vertices
            .iter()
            .filter(|v| (v[axis] - coord).abs() <= tolerance)
            .min_by(|a, b| (a[axis] - coord).abs().total_cmp(&(b[axis] - coord).abs()))
            .copied()
            .unwrap_or_else(|| {
                let mut v = Vec3::ZERO;
                v[axis] = snapped;
                v
            });
        return (snapped, PlaneSnap::Vertex(vert));
    }

    let nearest_midpoint = edges
        .iter()
        .filter_map(|&(a, b)| {
            let va = authored_vertices.get(a)?;
            let vb = authored_vertices.get(b)?;
            Some((*va + *vb) * 0.5)
        })
        .filter(|mid| (mid[axis] - coord).abs() <= tolerance)
        .min_by(|a, b| (a[axis] - coord).abs().total_cmp(&(b[axis] - coord).abs()));
    if let Some(mid) = nearest_midpoint {
        return (mid[axis], PlaneSnap::EdgeMidpoint(mid));
    }

    if grid_active {
        let mut v = Vec3::ZERO;
        v[axis] = coord;
        let snapped = grid.snap_position_to_grid(v)[axis];
        return (snapped, PlaneSnap::Grid);
    }

    (coord, PlaneSnap::Free)
}

// =====================================================================
// Plane drag
// =====================================================================

/// Screen radius, in pixels, within which the dragged plane locks to a nearby
/// vertex or edge midpoint. Converted to world units at the handle depth each
/// frame so the snap feel is the same at any zoom, matching the cursor-relative
/// thresholds the brush vertex / edge picks use.
const PLANE_SNAP_PIXELS: f32 = 10.0;

/// Maps an axis index to the matching brush-local axis constraint, so the drag
/// calibration moves the plane only along its own normal.
fn axis_constraint(axis: usize) -> VertexDragConstraint {
    match axis {
        0 => VertexDragConstraint::AxisX,
        1 => VertexDragConstraint::AxisY,
        _ => VertexDragConstraint::AxisZ,
    }
}

/// World units that one cursor pixel spans along the brush-local `axis` at
/// `anchor_world`, measured against the live projection the same way
/// [`compute_brush_drag_offset`]'s axis branch calibrates the drag. Used to
/// turn [`PLANE_SNAP_PIXELS`] into a world-space snap tolerance so the lock
/// distance reads the same at any zoom or brush scale.
fn world_per_pixel_along_axis(
    axis: usize,
    cam_tf: &GlobalTransform,
    camera: &Camera,
    brush_global: &GlobalTransform,
    anchor_world: Vec3,
) -> Option<f32> {
    let origin_screen = camera.world_to_viewport(cam_tf, anchor_world).ok()?;
    let axis_dir = match axis {
        0 => Vec3::X,
        1 => Vec3::Y,
        _ => Vec3::Z,
    };
    let (_, brush_rot, _) = brush_global.to_scale_rotation_translation();
    let world_axis = brush_rot * axis_dir;
    let axis_screen = camera
        .world_to_viewport(cam_tf, anchor_world + world_axis)
        .ok()?;
    let px_per_world = (axis_screen - origin_screen).length();
    if px_per_world < 1e-4 {
        return None;
    }
    Some(1.0 / px_per_world)
}

/// Per-drag state for `mirror.plane.drag`: the grabbed handle, the offset and
/// anchor captured at grab, and the cursor / viewport the drag is bound to.
#[derive(Resource, Default)]
pub struct MirrorPlaneDragState {
    pub active: bool,
    pub entity: Option<Entity>,
    pub axis: usize,
    /// `offset[axis]` at grab, the baseline the drag delta adds to and the
    /// value cancel restores.
    pub start_offset: f32,
    /// `plane_handle_world` at grab; the depth the pixel-to-world drag
    /// calibration measures against.
    pub anchor_world: Vec3,
    pub start_cursor: Vec2,
    /// Multi-viewport: camera + UI-node entities captured at grab so the drag
    /// stays bound to its origin viewport even if the cursor wanders.
    pub camera_entity: Option<Entity>,
    pub viewport_entity: Option<Entity>,
    /// Brush-local position of the geometry element the plane is currently
    /// locked to (a vertex or edge midpoint), for the snap highlight. `None`
    /// when snapping to the grid or nothing.
    pub snap_target: Option<Vec3>,
}

/// On LMB just-pressed while a mirror-plane handle is hovered and no modal is
/// running, dispatch `mirror.plane.drag`. Mirrors
/// [`crate::brush_drag_ops::vertex_drag_invoke_trigger`]: the operator captures
/// its start state on the first invoke and runs modal from there.
pub(crate) fn mirror_plane_drag_invoke_trigger(
    pointer: crate::modal_inputs::PointerInputs,
    hover: Res<MirrorPlaneHover>,
    drag_state: Res<MirrorPlaneDragState>,
    keybind_focus: KeybindFocus,
    active_modal: jackdaw_api_internal::lifecycle::ActiveModalQuery,
    vp: ViewportCursor,
    mut commands: Commands,
) {
    if !pointer.pointer_primary_just_pressed()
        || hover.target.is_none()
        || drag_state.active
        || keybind_focus.is_typing()
        || vp.viewport_entity().is_none()
        || active_modal.is_modal_running()
    {
        return;
    }
    commands.queue(|world: &mut World| {
        let _ = world
            .operator(MirrorPlaneDragOp::ID)
            .settings(CallOperatorSettings {
                execution_context: ExecutionContext::Invoke,
                creates_history_entry: true,
            })
            .call();
    });
}

#[operator(
    id = "mirror.plane.drag",
    label = "Drag Mirror Plane",
    description = "Grab the hovered mirror-plane handle and slide the plane along \
                   its axis, snapping to nearby brush vertices / edge midpoints \
                   (then the grid when grid snapping is on). Modal: LMB release \
                   commits, Escape or right-click cancels (restoring the start \
                   offset).",
    modal = true,
    allows_undo = true,
    cancel = cancel_mirror_plane_drag,
)]
pub fn mirror_plane_drag(
    _: In<OperatorParameters>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    vp: ViewportCursor,
    hover: Res<MirrorPlaneHover>,
    mut brushes: Query<(&Brush, &GlobalTransform, &mut ModifierStack)>,
    snap_settings: Res<SnapSettings>,
    mut drag_state: ResMut<MirrorPlaneDragState>,
    modal: Option<Single<Entity, With<ActiveModalOperator>>>,
) -> OperatorResult {
    let cursor_pos = vp.cursor()?;

    if modal.is_none() {
        // First invoke: grab the hovered handle and capture the baseline.
        let Some((entity, axis)) = hover.target else {
            return OperatorResult::Cancelled;
        };
        let Ok((brush, global_tf, stack)) = brushes.get(entity) else {
            return OperatorResult::Cancelled;
        };
        let Some(mirror) = stack.first_enabled_mirror() else {
            return OperatorResult::Cancelled;
        };
        let Some(anchor_world) = plane_handle_world(brush, global_tf, mirror, axis) else {
            return OperatorResult::Cancelled;
        };
        // Bind the drag to the hovered viewport, like the other modal drags, so
        // it keeps tracking even if the cursor strays into another panel.
        let camera_entity = vp.camera_entity()?;
        let viewport_entity = vp.viewport_entity()?;
        let (camera, _) = vp.camera_for(camera_entity)?;
        let start_cursor = vp.viewport_cursor_for(camera, viewport_entity, cursor_pos)?;

        drag_state.active = true;
        drag_state.entity = Some(entity);
        drag_state.axis = axis;
        drag_state.start_offset = mirror.offset[axis];
        drag_state.anchor_world = anchor_world;
        drag_state.start_cursor = start_cursor;
        drag_state.camera_entity = Some(camera_entity);
        drag_state.viewport_entity = Some(viewport_entity);
        return OperatorResult::Running;
    }

    // Subsequent invokes: RMB cancel, release commit, per-frame plane slide.
    if mouse.just_pressed(MouseButton::Right) {
        return OperatorResult::Cancelled;
    }
    if mouse.just_released(MouseButton::Left) {
        clear_drag_state(&mut drag_state);
        return OperatorResult::Finished;
    }

    let (Some(entity), Some(camera_entity), Some(viewport_entity)) = (
        drag_state.entity,
        drag_state.camera_entity,
        drag_state.viewport_entity,
    ) else {
        return OperatorResult::Running;
    };
    let axis = drag_state.axis;
    let (camera, cam_tf) = vp.camera_for(camera_entity)?;
    let Some(viewport_cursor) = vp.viewport_cursor_for(camera, viewport_entity, cursor_pos) else {
        return OperatorResult::Running;
    };
    let Ok((brush, brush_global, mut stack)) = brushes.get_mut(entity) else {
        return OperatorResult::Running;
    };

    let mouse_delta = viewport_cursor - drag_state.start_cursor;
    let Some(local_delta) = compute_brush_drag_offset(
        axis_constraint(axis),
        mouse_delta,
        cam_tf,
        camera,
        brush_global,
        drag_state.anchor_world,
    ) else {
        return OperatorResult::Running;
    };
    let tentative = drag_state.start_offset + local_delta[axis];

    // Snap against the authored geometry: vertices and edge midpoints first,
    // then the grid when grid snapping is on. Ctrl inverts the grid toggle for
    // this gesture, matching the other drags.
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let grid_active = snap_settings.translate_active(ctrl);
    let authored_vertices = brush
        .topology
        .vertices
        .iter()
        .map(|v| v.position)
        .collect::<Vec<_>>();
    // `topology.edges` already lists the authored unique edges; the cache's
    // `unique_edges` walks evaluated rings (mirror copies included), so read
    // them here to keep the snap in authored space.
    let edges = brush
        .topology
        .edges
        .iter()
        .map(|e| (e.v[0] as usize, e.v[1] as usize))
        .collect::<Vec<_>>();
    let tolerance =
        world_per_pixel_along_axis(axis, cam_tf, camera, brush_global, drag_state.anchor_world)
            .map(|wpp| wpp * PLANE_SNAP_PIXELS)
            .unwrap_or_else(|| snap_settings.grid_size());
    let (resolved, snap) = resolve_plane_snap(
        &authored_vertices,
        &edges,
        axis,
        tentative,
        tolerance,
        grid_active,
        &snap_settings,
    );

    if let Some(mirror) = stack.first_enabled_mirror_mut() {
        mirror.offset[axis] = resolved;
    }
    drag_state.snap_target = match snap {
        PlaneSnap::Vertex(v) | PlaneSnap::EdgeMidpoint(v) => Some(v),
        PlaneSnap::Grid | PlaneSnap::Free => None,
    };
    OperatorResult::Running
}

fn cancel_mirror_plane_drag(
    mut brushes: Query<&mut ModifierStack, With<Brush>>,
    mut drag_state: ResMut<MirrorPlaneDragState>,
) {
    if let Some(entity) = drag_state.entity
        && let Ok(mut stack) = brushes.get_mut(entity)
        && let Some(mirror) = stack.first_enabled_mirror_mut()
    {
        mirror.offset[drag_state.axis] = drag_state.start_offset;
    }
    clear_drag_state(&mut drag_state);
}

fn clear_drag_state(drag_state: &mut MirrorPlaneDragState) {
    drag_state.active = false;
    drag_state.entity = None;
    drag_state.axis = 0;
    drag_state.start_offset = 0.0;
    drag_state.anchor_world = Vec3::ZERO;
    drag_state.start_cursor = Vec2::ZERO;
    drag_state.camera_entity = None;
    drag_state.viewport_entity = None;
    drag_state.snap_target = None;
}
