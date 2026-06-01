//! Modal operators for the per-element brush drags: face / vertex /
//! edge. Each one wraps the corresponding interaction state machine
//! that used to run as an unconditional system in
//! `brush::interaction`. The drag math itself is unchanged; this file
//! owns the modal lifecycle (trigger on click, per-frame invoke,
//! release commit, Escape cancel) and Right-click cancel.
//!
//! Constraint cycling (X / Y / Z) for vertex / edge drag is handled
//! inline in the operator body. Escape goes through the global
//! `modal.cancel` chain.

use bevy::feathers::cursor::{EntityCursor, OverrideCursor};
use bevy::prelude::*;
use bevy::window::SystemCursorIcon;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;
use jackdaw_jsn::Brush;

use crate::brush::interaction::{
    FaceExtrudeMode, PendingSubDrag, VertexDragConstraint, compute_brush_drag_offset,
};
use crate::brush::box_select::BrushBoxSelectState;
use crate::brush::{
    BrushDragCapture, BrushDragState, BrushEditMode, BrushFaceEntity, BrushMeshCache,
    BrushSelection, BrushSubSelection, EdgeDragState, EditMode, VertexDragState,
    rebuild_brush_from_vertices,
};
use crate::draw_brush::DrawBrushState;
use crate::keybind_focus::KeybindFocus;
use crate::modal_transform::ModalTransformState;
use crate::selection::{Selected, Selection};
use crate::snapping::SnapSettings;
use crate::viewport::ViewportCursor;
use crate::viewport_util::{point_in_polygon_2d, point_to_segment_dist};

/// Minimum extrude depth before commit pushes a new brush.
const MIN_EXTRUDE_DEPTH: f32 = 0.01;

/// Pixels the cursor must travel after a press to promote pending -> active.
const DRAG_THRESHOLD: f32 = 5.0;

/// Show the grabbing cursor while a geometry drag is active.
fn set_grab_cursor(override_cursor: &mut OverrideCursor) {
    override_cursor.0 = Some(EntityCursor::System(SystemCursorIcon::Grabbing));
}

/// Clear the grabbing cursor when a geometry drag ends, leaving any cursor
/// owned by another system untouched.
fn clear_grab_cursor(override_cursor: &mut OverrideCursor) {
    if override_cursor.0 == Some(EntityCursor::System(SystemCursorIcon::Grabbing)) {
        override_cursor.0 = None;
    }
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushFaceDragOp>()
        .register_operator::<BrushVertexDragOp>()
        .register_operator::<BrushEdgeDragOp>();
}

/// True when no other modal/drag/draw is active and the cursor isn't in a text field.
fn drag_environment_ok(
    keybind_focus: &KeybindFocus,
    modal: &ModalTransformState,
    draw_state: &DrawBrushState,
) -> bool {
    !keybind_focus.is_typing() && modal.active.is_none() && draw_state.active.is_none()
}

/// Returns true if the cursor is over any face polygon of `brush_entity`.
///
/// Same face hit-test as `brush_face_drag`. Used by other invoke
/// triggers (notably box-select) that must yield to face-drag when
/// the user shift-clicks on a brush face. Without
/// this guard, box-select races face-drag for the same `Shift + LMB`
/// chord and wins because face-drag's hit-test runs inside the
/// operator, which dispatches a frame later.
pub(crate) fn cursor_over_brush_face(
    brush_entity: Entity,
    viewport_cursor: Vec2,
    camera: &Camera,
    cam_tf: &GlobalTransform,
    face_entities: &Query<(Entity, &BrushFaceEntity, &GlobalTransform)>,
    brush_caches: &Query<&BrushMeshCache>,
) -> bool {
    let Ok(cache) = brush_caches.get(brush_entity) else {
        return false;
    };
    for (_, face_ent, face_global) in face_entities {
        if face_ent.brush_entity != brush_entity {
            continue;
        }
        let polygon = &cache.face_polygons[face_ent.face_index];
        if polygon.len() < 3 {
            continue;
        }
        let screen_verts: Vec<Vec2> = polygon
            .iter()
            .filter_map(|&vi| {
                camera
                    .world_to_viewport(cam_tf, face_global.transform_point(cache.vertices[vi]))
                    .ok()
            })
            .collect();
        if screen_verts.len() < 3 {
            continue;
        }
        if point_in_polygon_2d(viewport_cursor, &screen_verts) {
            return true;
        }
    }
    false
}

// =====================================================================
// Face drag
// =====================================================================

/// Mouse-down dispatches `brush.face.drag` whenever the gesture is one
/// of: LMB while in face-edit mode, or Shift / Alt + LMB in object
/// mode (auto-enters face-edit as a "quick action").
pub(crate) fn face_drag_invoke_trigger(
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    edit_mode: Res<EditMode>,
    drag_state: Res<BrushDragState>,
    keybind_focus: KeybindFocus,
    modal: Res<ModalTransformState>,
    draw_state: Res<DrawBrushState>,
    vp: ViewportCursor,
    gizmo_hover: Res<crate::gizmos::GizmoHoverState>,
    mut commands: Commands,
) {
    if !mouse.just_pressed(MouseButton::Left) || drag_state.active || drag_state.pending.is_some() {
        return;
    }

    // Only trigger when inside the viewport
    if vp.viewport_entity().is_none() {
        return;
    }

    let in_face_edit = matches!(*edit_mode, EditMode::BrushEdit(BrushEditMode::Face));

    // In face-edit mode a hovered gizmo axis owns the click; the sub-element
    // gizmo drag takes it via `gizmo.drag_edit`. (Object-mode extrude via
    // shift/alt is unaffected.)
    if in_face_edit && gizmo_hover.hovered_axis.is_some() {
        return;
    }
    let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let alt = keyboard.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    if !(in_face_edit || shift || alt) {
        return;
    }
    if !drag_environment_ok(&keybind_focus, &modal, &draw_state) && !in_face_edit {
        return;
    }
    commands.queue(|world: &mut World| {
        let _ = world
            .operator(BrushFaceDragOp::ID)
            .settings(CallOperatorSettings {
                execution_context: ExecutionContext::Invoke,
                creates_history_entry: true,
            })
            .call();
    });
}

#[operator(
    id = "brush.face.drag",
    label = "Drag Face",
    description = "Pick a brush face under the cursor and drag it (push/pull or \
                   shift+extrude). Modal: commits on LMB release, cancels on \
                   Escape or right-click. Auto-enters face-edit mode from object \
                   mode as a quick action; the drag-end / cancel restores Object \
                   mode in that case.",
    modal = true,
    cancel = cancel_face_drag,
)]
pub fn brush_face_drag(
    _: In<OperatorParameters>,
    mut edit_mode: ResMut<EditMode>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    vp: ViewportCursor,
    face_entities: Query<(Entity, &BrushFaceEntity, &GlobalTransform)>,
    brush_caches: Query<&BrushMeshCache>,
    selection: Res<Selection>,
    snap_settings: Res<SnapSettings>,
    mut brush_selection: ResMut<BrushSelection>,
    mut brush_box_state: ResMut<BrushBoxSelectState>,
    mut brushes: Query<(&mut Brush, &GlobalTransform)>,
    mut drag_state: ResMut<BrushDragState>,
    mut commands: Commands,
    modal: Option<Single<Entity, With<ActiveModalOperator>>>,
    mut halfedge_q: Query<&mut crate::brush::BrushHalfedge>,
    mut override_cursor: ResMut<OverrideCursor>,
) -> OperatorResult {
    let cursor_pos = vp.cursor()?;
    // First invoke uses the hovered viewport; subsequent invokes use
    // the captured one so the drag stays bound to its origin panel.
    let (camera_entity, viewport_entity) = if modal.is_none() {
        let camera_entity = vp.camera_entity()?;
        let viewport_entity = vp.viewport_entity()?;
        (camera_entity, viewport_entity)
    } else {
        match (drag_state.drag_camera, drag_state.drag_viewport) {
            (Some(c), Some(v)) => (c, v),
            _ => {
                let c = vp.camera_entity()?;
                let v = vp.viewport_entity()?;
                (c, v)
            }
        }
    };
    let (camera, cam_tf) = vp.camera_for(camera_entity)?;
    let viewport_cursor = vp.viewport_cursor_for(camera, viewport_entity, cursor_pos)?;

    let in_face_edit = matches!(*edit_mode, EditMode::BrushEdit(BrushEditMode::Face));
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let alt = keyboard.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);

    if modal.is_none() {
        // First-invoke: pick a face under the cursor, set pending /
        // selection / quick-action.
        //
        // In face-edit mode, hit-test across all edit brushes and pick the
        // nearest face. A plain click clears other brushes' sub-selections.
        // Ctrl extends (toggle) on the clicked brush. In object mode the
        // single primary-selected brush is used as before.
        if in_face_edit {
            let candidates: Vec<Entity> = brush_selection.edit_brushes().collect();

            let mut best: Option<(Entity, usize)> = None;
            let mut best_depth = f32::MAX;

            for candidate in &candidates {
                let brush_entity = *candidate;
                let Ok(cache) = brush_caches.get(brush_entity) else {
                    continue;
                };
                for (_, face_ent, face_global) in &face_entities {
                    if face_ent.brush_entity != brush_entity {
                        continue;
                    }
                    let polygon = &cache.face_polygons[face_ent.face_index];
                    if polygon.len() < 3 {
                        continue;
                    }
                    let screen_verts: Vec<Vec2> = polygon
                        .iter()
                        .filter_map(|&vi| {
                            camera
                                .world_to_viewport(
                                    cam_tf,
                                    face_global.transform_point(cache.vertices[vi]),
                                )
                                .ok()
                        })
                        .collect();
                    if screen_verts.len() < 3 {
                        continue;
                    }
                    if point_in_polygon_2d(viewport_cursor, &screen_verts) {
                        let centroid: Vec3 =
                            polygon.iter().map(|&vi| cache.vertices[vi]).sum::<Vec3>()
                                / polygon.len() as f32;
                        let depth = (cam_tf.translation()
                            - face_global.transform_point(centroid))
                        .length_squared();
                        if depth < best_depth {
                            best_depth = depth;
                            best = Some((brush_entity, face_ent.face_index));
                        }
                    }
                }
            }

            let Some((brush_entity, face_idx)) = best else {
                // No face hit. Ctrl is additive-toggle, so an empty
                // Ctrl-click is a no-op. Otherwise hand the press to the
                // edit-mode box-select (Shift records an add-select);
                // staying in edit mode either way.
                if !ctrl {
                    brush_box_state.pending = Some(cursor_pos);
                    brush_box_state.shift = shift;
                }
                return OperatorResult::Cancelled;
            };

            brush_selection.active_brush = Some(brush_entity);
            drag_state.quick_action = false;

            if ctrl {
                // Ctrl+click: toggle on the clicked brush; leave other brushes untouched.
                let sub = brush_selection.sub_mut(brush_entity);
                if let Some(pos) = sub.faces.iter().position(|&f| f == face_idx) {
                    sub.faces.remove(pos);
                } else {
                    sub.faces.push(face_idx);
                }
                return OperatorResult::Cancelled;
            }

            // Plain click: clear every brush's sub-selection, then select the clicked face.
            brush_selection.clear_sub_selections();
            brush_selection.sub_mut(brush_entity).faces = vec![face_idx];
            drag_state.extrude_mode = FaceExtrudeMode::Merge;
            drag_state.pending = Some(PendingSubDrag {
                click_pos: cursor_pos,
            });
            drag_state.drag_camera = Some(camera_entity);
            drag_state.drag_viewport = Some(viewport_entity);
            return OperatorResult::Running;
        }

        // Object-mode quick-action: single brush path (unchanged).
        let brush_entity =
            selection.primary().filter(|&e| brushes.contains(e))?;
        let cache = brush_caches.get(brush_entity)?;

        let mut best_face = None;
        let mut best_depth = f32::MAX;
        for (_, face_ent, face_global) in &face_entities {
            if face_ent.brush_entity != brush_entity {
                continue;
            }
            let polygon = &cache.face_polygons[face_ent.face_index];
            if polygon.len() < 3 {
                continue;
            }
            let screen_verts: Vec<Vec2> = polygon
                .iter()
                .filter_map(|&vi| {
                    camera
                        .world_to_viewport(cam_tf, face_global.transform_point(cache.vertices[vi]))
                        .ok()
                })
                .collect();
            if screen_verts.len() < 3 {
                continue;
            }
            if point_in_polygon_2d(viewport_cursor, &screen_verts) {
                let centroid: Vec3 = polygon.iter().map(|&vi| cache.vertices[vi]).sum::<Vec3>()
                    / polygon.len() as f32;
                let depth =
                    (cam_tf.translation() - face_global.transform_point(centroid)).length_squared();
                if depth < best_depth {
                    best_depth = depth;
                    best_face = Some(face_ent.face_index);
                }
            }
        }

        let Some(face_idx) = best_face else {
            return OperatorResult::Cancelled;
        };

        *edit_mode = EditMode::BrushEdit(BrushEditMode::Face);
        brush_selection.active_brush = Some(brush_entity);
        brush_selection.sub_mut(brush_entity).clear();
        drag_state.quick_action = true;

        brush_selection.sub_mut(brush_entity).faces = vec![face_idx];
        drag_state.extrude_mode = if alt {
            FaceExtrudeMode::Extend
        } else {
            FaceExtrudeMode::Merge
        };
        drag_state.pending = Some(PendingSubDrag {
            click_pos: cursor_pos,
        });
        drag_state.drag_camera = Some(camera_entity);
        drag_state.drag_viewport = Some(viewport_entity);
        return OperatorResult::Running;
    }

    // Subsequent invoke: handle right-click cancel, release commit,
    // pending -> active promotion, and per-frame drag math.
    if drag_state.active && mouse.just_pressed(MouseButton::Right) {
        return OperatorResult::Cancelled;
    }

    if mouse.just_released(MouseButton::Left) {
        if drag_state.active {
            match drag_state.extrude_mode {
                FaceExtrudeMode::Merge => {}
                FaceExtrudeMode::Extend => {
                    if drag_state.extend_depth.abs() > MIN_EXTRUDE_DEPTH {
                        spawn_extruded_brush(
                            &drag_state.extend_face_polygon,
                            drag_state.extend_face_normal,
                            drag_state.extend_depth,
                            &mut commands,
                        );
                    }
                }
            }
        }
        let was_quick = drag_state.quick_action;
        clear_face_drag_state(&mut drag_state);
        clear_grab_cursor(&mut override_cursor);
        if was_quick {
            *edit_mode = EditMode::Object;
            brush_selection.clear();
        }
        return OperatorResult::Finished;
    }

    if let Some(ref pending) = drag_state.pending
        && mouse.pressed(MouseButton::Left)
        && !drag_state.active
        && (cursor_pos - pending.click_pos).length() > DRAG_THRESHOLD
        && let Some(brush_entity) = brush_selection.active_brush
        && let Ok((brush, brush_global)) = brushes.get(brush_entity)
    {
        drag_state.active = true;
        set_grab_cursor(&mut override_cursor);
        drag_state.start_cursor = viewport_cursor;
        let active_faces: Vec<usize> = brush_selection
            .sub(brush_entity)
            .map(|s| s.faces.clone())
            .unwrap_or_default();
        if let Some(&face_idx) = active_faces.first()
            && face_idx < brush.faces.len()
        {
            drag_state.drag_face_normal = brush.faces[face_idx].plane.normal;
        }
        match drag_state.extrude_mode {
            FaceExtrudeMode::Merge => {
                drag_state.start_brush = Some(brush.clone());
            }
            FaceExtrudeMode::Extend => {
                let (_, brush_rot, _) = brush_global.to_scale_rotation_translation();
                drag_state.extend_face_normal =
                    (brush_rot * drag_state.drag_face_normal).normalize();
                if let Ok(cache) = brush_caches.get(brush_entity)
                    && let Some(&face_idx) = active_faces.first()
                {
                    drag_state.extend_face_polygon = cache.face_polygons[face_idx]
                        .iter()
                        .map(|&vi| brush_global.transform_point(cache.vertices[vi]))
                        .collect();
                }
                drag_state.extend_depth = 0.0;
            }
        }
    }

    if drag_state.active {
        let brush_entity = brush_selection.active_brush?;
        let drag_faces: Vec<usize> = brush_selection
            .sub(brush_entity)
            .map(|s| s.faces.clone())
            .unwrap_or_default();
        match drag_state.extrude_mode {
            FaceExtrudeMode::Merge => {
                let (mut brush, brush_global) = brushes.get_mut(brush_entity)?;
                let start = drag_state.start_brush.as_ref()?;
                let brush_pos = brush_global.translation();
                let Ok(origin_screen) = camera.world_to_viewport(cam_tf, brush_pos) else {
                    return OperatorResult::Running;
                };
                let Ok(normal_screen) =
                    camera.world_to_viewport(cam_tf, brush_pos + drag_state.drag_face_normal)
                else {
                    return OperatorResult::Running;
                };
                let screen_dir = (normal_screen - origin_screen).normalize_or_zero();
                let mouse_delta = viewport_cursor - drag_state.start_cursor;
                let projected = mouse_delta.dot(screen_dir);
                let cam_dist = (cam_tf.translation() - brush_pos).length();
                let drag_amount =
                    snap_translate(projected * cam_dist * 0.003, &snap_settings, ctrl);
                if let Ok(mut halfedge) = halfedge_q.get_mut(brush_entity) {
                    // HalfedgeMesh path: translate each selected face's ring vertices along the face normal.
                    let face_keys = halfedge.face_keys.clone();
                    let start_positions: Vec<bevy::math::Vec3> =
                        if !start.topology.vertices.is_empty() {
                            start.topology.vertices.iter().map(|v| v.position).collect()
                        } else {
                            halfedge.mesh.verts.values().map(|v| v.co).collect()
                        };
                    use std::collections::HashSet;
                    let mut translated: HashSet<jackdaw_geometry::halfedge::VertKey> =
                        HashSet::new();
                    for &face_idx in &drag_faces {
                        if face_idx >= face_keys.len() {
                            continue;
                        }
                        let fk = face_keys[face_idx];
                        let face_normal = if face_idx < start.faces.len() {
                            start.faces[face_idx].plane.normal
                        } else {
                            halfedge.mesh.faces[fk].normal_cache
                        };
                        let face = &halfedge.mesh.faces[fk];
                        let mut cur = face.loop_first;
                        let mut ring_keys: Vec<jackdaw_geometry::halfedge::VertKey> =
                            Vec::with_capacity(face.loop_count as usize);
                        for _ in 0..face.loop_count {
                            ring_keys.push(halfedge.mesh.loops[cur].vert);
                            cur = halfedge.mesh.loops[cur].next;
                        }
                        for vk in ring_keys {
                            if translated.contains(&vk) {
                                continue;
                            }
                            translated.insert(vk);
                            let start_pos_for_key = halfedge
                                .vert_keys
                                .iter()
                                .position(|&k| k == vk)
                                .and_then(|idx| start_positions.get(idx).copied());
                            let new_co = match start_pos_for_key {
                                Some(start_co) => start_co + face_normal * drag_amount,
                                None => halfedge.mesh.verts[vk].co + face_normal * drag_amount,
                            };
                            halfedge.mesh.verts[vk].co = new_co;
                        }
                    }
                    // Recompute all face normals.
                    let face_keys_all: Vec<_> = halfedge.mesh.faces.keys().collect();
                    for fk in face_keys_all {
                        let face = &halfedge.mesh.faces[fk];
                        let mut ring_positions = Vec::with_capacity(face.loop_count as usize);
                        let mut cur = face.loop_first;
                        for _ in 0..face.loop_count {
                            let lp = &halfedge.mesh.loops[cur];
                            ring_positions.push(halfedge.mesh.verts[lp.vert].co);
                            cur = lp.next;
                        }
                        let new_normal = jackdaw_geometry::newell_normal(&ring_positions);
                        halfedge.mesh.faces[fk].normal_cache = new_normal;
                    }
                    // Sync brush.faces[i].plane and brush.topology.
                    let new_topology = halfedge.mesh.flatten_to_topology();
                    let positions: Vec<bevy::math::Vec3> =
                        new_topology.vertices.iter().map(|v| v.position).collect();
                    for (face_idx, face_data) in brush.faces.iter_mut().enumerate() {
                        if face_idx < new_topology.polygons.len() {
                            let normal = new_topology.face_normal_with(&positions, face_idx);
                            let v0_idx = new_topology.loops
                                [new_topology.polygons[face_idx].loop_start as usize]
                                .vert as usize;
                            let distance = positions[v0_idx].dot(normal);
                            face_data.plane.normal = normal;
                            face_data.plane.distance = distance;
                        }
                    }
                    brush.topology = new_topology;
                } else {
                    // Legacy convex path: just shift plane.distance.
                    for &face_idx in &drag_faces {
                        if face_idx < start.faces.len() && face_idx < brush.faces.len() {
                            brush.faces[face_idx].plane.distance =
                                start.faces[face_idx].plane.distance + drag_amount;
                        }
                    }
                }
            }
            FaceExtrudeMode::Extend => {
                if drag_state.extend_face_polygon.is_empty() {
                    return OperatorResult::Cancelled;
                }
                let face_centroid: Vec3 = drag_state.extend_face_polygon.iter().sum::<Vec3>()
                    / drag_state.extend_face_polygon.len() as f32;
                let world_normal = drag_state.extend_face_normal;
                let Ok(origin_screen) = camera.world_to_viewport(cam_tf, face_centroid) else {
                    return OperatorResult::Running;
                };
                let Ok(normal_screen) =
                    camera.world_to_viewport(cam_tf, face_centroid + world_normal)
                else {
                    return OperatorResult::Running;
                };
                let screen_dir = (normal_screen - origin_screen).normalize_or_zero();
                let mouse_delta = viewport_cursor - drag_state.start_cursor;
                let projected = mouse_delta.dot(screen_dir);
                let cam_dist = (cam_tf.translation() - face_centroid).length();
                drag_state.extend_depth =
                    snap_translate(projected * cam_dist * 0.003, &snap_settings, ctrl);
            }
        }
    }

    OperatorResult::Running
}

fn cancel_face_drag(
    mut edit_mode: ResMut<EditMode>,
    mut brush_selection: ResMut<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut drag_state: ResMut<BrushDragState>,
    mut halfedge_q: Query<&mut crate::brush::BrushHalfedge>,
    mut override_cursor: ResMut<OverrideCursor>,
) {
    if drag_state.extrude_mode == FaceExtrudeMode::Merge
        && let Some(brush_entity) = brush_selection.active_brush
        && let Some(ref start) = drag_state.start_brush
        && let Ok(mut brush) = brushes.get_mut(brush_entity)
    {
        *brush = start.clone();
        // If HalfedgeMesh was active, re-lift from the restored topology.
        if let Ok(mut halfedge) = halfedge_q.get_mut(brush_entity) {
            *halfedge = crate::brush::BrushHalfedge::from_topology(&start.topology);
        }
    }
    let was_quick = drag_state.quick_action;
    clear_face_drag_state(&mut drag_state);
    clear_grab_cursor(&mut override_cursor);
    if was_quick {
        *edit_mode = EditMode::Object;
        brush_selection.clear();
    }
}

fn clear_face_drag_state(drag_state: &mut BrushDragState) {
    drag_state.active = false;
    drag_state.pending = None;
    drag_state.extend_face_polygon.clear();
    drag_state.extend_depth = 0.0;
    drag_state.start_brush = None;
    drag_state.quick_action = false;
    drag_state.drag_camera = None;
    drag_state.drag_viewport = None;
}

/// Snap a vertex/edge/face-drag's local-space offset so the dragged
/// geometry lands on the world grid. Same snap convention as
/// elsewhere: translate-snap is on/off via `SnapSettings`, with Ctrl
/// flipping the
/// state for the current gesture. For an unconstrained drag we snap the
/// primary vertex's final world position on all three axes; for an axis-
/// constrained drag we snap the scalar offset along the local constraint
/// axis. Returns `local_offset` unchanged when snap is disabled.
pub(crate) fn snap_drag_local_offset(
    local_offset: Vec3,
    primary_start_local: Vec3,
    constraint: VertexDragConstraint,
    brush_global: &GlobalTransform,
    snap: &SnapSettings,
    ctrl: bool,
) -> Vec3 {
    if !snap.translate_active(ctrl) || snap.translate_increment <= 0.0 {
        return local_offset;
    }
    let inc = snap.translate_increment;
    let snap_to = |v: f32| (v / inc).round() * inc;
    match constraint {
        VertexDragConstraint::Free => {
            let world_end = brush_global.transform_point(primary_start_local + local_offset);
            let snapped_world = Vec3::new(
                snap_to(world_end.x),
                snap_to(world_end.y),
                snap_to(world_end.z),
            );
            let inv = brush_global.affine().inverse();
            let snapped_local_end = inv.transform_point3(snapped_world);
            snapped_local_end - primary_start_local
        }
        VertexDragConstraint::AxisX | VertexDragConstraint::AxisY | VertexDragConstraint::AxisZ => {
            let axis_local = match constraint {
                VertexDragConstraint::AxisX => Vec3::X,
                VertexDragConstraint::AxisY => Vec3::Y,
                VertexDragConstraint::AxisZ => Vec3::Z,
                VertexDragConstraint::Free => unreachable!(),
            };
            axis_local * snap_to(local_offset.dot(axis_local))
        }
    }
}

fn snap_translate(value: f32, snap: &SnapSettings, ctrl: bool) -> f32 {
    if snap.translate_active(ctrl) && snap.translate_increment > 0.0 {
        (value / snap.translate_increment).round() * snap.translate_increment
    } else {
        value
    }
}

fn spawn_extruded_brush(
    face_polygon_world: &[Vec3],
    world_normal: Vec3,
    depth: f32,
    commands: &mut Commands,
) {
    if face_polygon_world.len() < 3 || depth.abs() < MIN_EXTRUDE_DEPTH {
        return;
    }

    let face_polygon = face_polygon_world.to_vec();
    let normal = world_normal;

    commands.queue(move |world: &mut World| {
        let face_centroid: Vec3 = face_polygon.iter().sum::<Vec3>() / face_polygon.len() as f32;
        let center = face_centroid + normal * depth / 2.0;

        let rotation = if normal == Vec3::Y {
            Quat::IDENTITY
        } else if normal == Vec3::NEG_Y {
            Quat::from_rotation_x(std::f32::consts::PI)
        } else {
            let (u, _v) = jackdaw_geometry::compute_face_tangent_axes(normal);
            let target_mat = Mat3::from_cols(u, normal, -normal.cross(u).normalize());
            Quat::from_mat3(&target_mat)
        };
        let inv_rotation = rotation.inverse();

        let local_verts: Vec<Vec3> = face_polygon
            .iter()
            .map(|&v| inv_rotation * (v - center))
            .collect();

        let Some(mut brush) = Brush::prism(&local_verts, Vec3::Y, depth) else {
            return;
        };

        let last_mat = world
            .resource::<crate::brush::LastUsedMaterial>()
            .material
            .clone();
        if let Some(ref mat) = last_mat {
            for face in &mut brush.faces {
                face.material = mat.clone();
            }
        }

        let entity = world
            .spawn((
                Name::new("Brush"),
                brush,
                Transform {
                    translation: center,
                    rotation,
                    scale: Vec3::ONE,
                },
                Visibility::default(),
            ))
            .id();

        let selection = world.resource::<Selection>();
        let old_selected: Vec<Entity> = selection.entities.clone();
        for &e in &old_selected {
            if let Ok(mut ec) = world.get_entity_mut(e) {
                ec.remove::<Selected>();
            }
        }
        let mut selection = world.resource_mut::<Selection>();
        selection.entities = vec![entity];
        world.entity_mut(entity).insert(Selected);
        // No manual undo push: the enclosing `brush.face.drag` modal
        // has `allows_undo = true` and its SnapshotDiff captures the
        // spawn.
    });
}

// =====================================================================
// Vertex drag
// =====================================================================

pub(crate) fn vertex_drag_invoke_trigger(
    mouse: Res<ButtonInput<MouseButton>>,
    edit_mode: Res<EditMode>,
    drag_state: Res<VertexDragState>,
    keybind_focus: KeybindFocus,
    vp: ViewportCursor,
    gizmo_hover: Res<crate::gizmos::GizmoHoverState>,
    mut commands: Commands,
) {
    if !mouse.just_pressed(MouseButton::Left)
        || !matches!(*edit_mode, EditMode::BrushEdit(BrushEditMode::Vertex))
        || drag_state.active
        || drag_state.pending.is_some()
        || keybind_focus.is_typing()
        || vp.viewport_entity().is_none()
        // A hovered gizmo axis owns the click; the sub-element gizmo drag
        // takes it via `gizmo.drag_edit`.
        || gizmo_hover.hovered_axis.is_some()
    {
        return;
    }
    commands.queue(|world: &mut World| {
        let _ = world
            .operator(BrushVertexDragOp::ID)
            .settings(CallOperatorSettings {
                execution_context: ExecutionContext::Invoke,
                creates_history_entry: true,
            })
            .call();
    });
}

#[operator(
    id = "brush.vertex.drag",
    label = "Drag Vertex",
    description = "Pick a brush vertex (or shift-pick a midpoint to split) and drag \
                   it. Modal: X / Y / Z toggle axis constraints during the drag, \
                   LMB release commits, Escape or right-click cancels.",
    modal = true,
    cancel = cancel_vertex_drag,
)]
pub fn brush_vertex_drag(
    _: In<OperatorParameters>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    vp: ViewportCursor,
    brush_transforms: Query<&GlobalTransform>,
    mut brush_selection: ResMut<BrushSelection>,
    mut brush_box_state: ResMut<BrushBoxSelectState>,
    brush_caches: Query<&BrushMeshCache>,
    mut brushes: Query<&mut Brush>,
    mut drag_state: ResMut<VertexDragState>,
    modal: Option<Single<Entity, With<ActiveModalOperator>>>,
    mut halfedge_q: Query<&mut crate::brush::BrushHalfedge>,
    snap_settings: Res<SnapSettings>,
    mut override_cursor: ResMut<OverrideCursor>,
) -> OperatorResult {
    let cursor_pos = vp.cursor()?;
    let (camera_entity, viewport_entity) = if modal.is_none() {
        let camera_entity = vp.camera_entity()?;
        let viewport_entity = vp.viewport_entity()?;
        (camera_entity, viewport_entity)
    } else {
        match (drag_state.drag_camera, drag_state.drag_viewport) {
            (Some(c), Some(v)) => (c, v),
            _ => {
                let c = vp.camera_entity()?;
                let v = vp.viewport_entity()?;
                (c, v)
            }
        }
    };
    let (camera, cam_tf) = vp.camera_for(camera_entity)?;
    let viewport_cursor = vp.viewport_cursor_for(camera, viewport_entity, cursor_pos)?;

    let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);

    if modal.is_none() {
        // First invoke: pick vertex / split vertex across all edit brushes.
        let candidates: Vec<Entity> = brush_selection.edit_brushes().collect();
        if candidates.is_empty() {
            return OperatorResult::Cancelled;
        }

        if shift && !ctrl {
            // Shift+click: pick edge midpoint or face center to split (active brush only).
            let brush_entity = brush_selection.active_brush?;
            let cache = brush_caches.get(brush_entity)?;
            let brush_global = brush_transforms.get(brush_entity)?;

            let mut best_split: Option<Vec3> = None;
            let mut best_dist = 20.0_f32;
            for (a, b) in cache.unique_edges() {
                let midpoint = (cache.vertices[a] + cache.vertices[b]) * 0.5;
                if let Ok(screen) =
                    camera.world_to_viewport(cam_tf, brush_global.transform_point(midpoint))
                {
                    let dist = (screen - viewport_cursor).length();
                    if dist < best_dist {
                        best_dist = dist;
                        best_split = Some(midpoint);
                    }
                }
            }
            if best_split.is_none() {
                best_dist = 20.0;
                for polygon in &cache.face_polygons {
                    if polygon.len() < 3 {
                        continue;
                    }
                    let centroid: Vec3 = polygon.iter().map(|&vi| cache.vertices[vi]).sum::<Vec3>()
                        / polygon.len() as f32;
                    if let Ok(screen) =
                        camera.world_to_viewport(cam_tf, brush_global.transform_point(centroid))
                    {
                        let dist = (screen - viewport_cursor).length();
                        if dist < best_dist {
                            best_dist = dist;
                            best_split = Some(centroid);
                        }
                    }
                }
            }
            let split_pos = best_split?;
            let new_idx = cache.vertices.len();
            brush_selection.sub_mut(brush_entity).vertices = vec![new_idx];
            drag_state.split_vertex = Some(split_pos);
            drag_state.pending = Some(PendingSubDrag {
                click_pos: cursor_pos,
            });
            return OperatorResult::Running;
        }

        // Plain or Ctrl+click: find the nearest vertex across all edit brushes.
        let mut best: Option<(Entity, usize)> = None;
        let mut best_dist = 20.0_f32;
        for &brush_entity in &candidates {
            let Ok(cache) = brush_caches.get(brush_entity) else {
                continue;
            };
            let Ok(brush_global) = brush_transforms.get(brush_entity) else {
                continue;
            };
            for (vi, v) in cache.vertices.iter().enumerate() {
                if let Ok(screen) =
                    camera.world_to_viewport(cam_tf, brush_global.transform_point(*v))
                {
                    let dist = (screen - viewport_cursor).length();
                    if dist < best_dist {
                        best_dist = dist;
                        best = Some((brush_entity, vi));
                    }
                }
            }
        }
        let Some((brush_entity, vi)) = best else {
            // No vertex hit. Ctrl is additive-toggle, so an empty
            // Ctrl-click is a no-op. Otherwise hand the press to the
            // edit-mode box-select (Shift records an add-select); staying
            // in edit mode either way.
            if !ctrl {
                brush_box_state.pending = Some(cursor_pos);
                brush_box_state.shift = shift;
            }
            return OperatorResult::Cancelled;
        };

        brush_selection.active_brush = Some(brush_entity);

        if ctrl {
            let sub = brush_selection.sub_mut(brush_entity);
            if let Some(pos) = sub.vertices.iter().position(|&v| v == vi) {
                sub.vertices.remove(pos);
            } else {
                sub.vertices.push(vi);
            }
            return OperatorResult::Cancelled;
        }

        // Plain click on an already-selected vertex: keep the whole multi-brush
        // selection and drag it as a group. Plain click on an unselected vertex:
        // clear every brush's sub-selection and select just this one.
        let already_selected = brush_selection
            .sub(brush_entity)
            .is_some_and(|s| s.vertices.contains(&vi));
        if !already_selected {
            brush_selection.clear_sub_selections();
            brush_selection.sub_mut(brush_entity).vertices = vec![vi];
        }
        drag_state.pending = Some(PendingSubDrag {
            click_pos: cursor_pos,
        });
        drag_state.drag_camera = Some(camera_entity);
        drag_state.drag_viewport = Some(viewport_entity);
        return OperatorResult::Running;
    }

    // Subsequent invokes need the active brush.
    let brush_entity = brush_selection.active_brush?;
    let brush_global = brush_transforms.get(brush_entity)?;

    // Subsequent invokes: constraint cycling, RMB cancel, release commit, drag math.
    if drag_state.active {
        if keyboard.just_pressed(KeyCode::KeyX) {
            drag_state.constraint =
                toggle_constraint(drag_state.constraint, VertexDragConstraint::AxisX);
        } else if keyboard.just_pressed(KeyCode::KeyY) {
            drag_state.constraint =
                toggle_constraint(drag_state.constraint, VertexDragConstraint::AxisY);
        } else if keyboard.just_pressed(KeyCode::KeyZ) {
            drag_state.constraint =
                toggle_constraint(drag_state.constraint, VertexDragConstraint::AxisZ);
        }
    }

    if drag_state.active && mouse.just_pressed(MouseButton::Right) {
        return OperatorResult::Cancelled;
    }

    if mouse.just_released(MouseButton::Left) {
        clear_vertex_drag_state(&mut drag_state);
        clear_grab_cursor(&mut override_cursor);
        return OperatorResult::Finished;
    }

    if let Some(ref pending) = drag_state.pending
        && mouse.pressed(MouseButton::Left)
        && !drag_state.active
        && (cursor_pos - pending.click_pos).length() > DRAG_THRESHOLD
        && let Ok(cache) = brush_caches.get(brush_entity)
        && let Ok(brush) = brushes.get(brush_entity)
    {
        drag_state.active = true;
        drag_state.constraint = VertexDragConstraint::Free;
        drag_state.start_brush = Some(brush.clone());
        drag_state.start_cursor = viewport_cursor;
        let mut all_verts = cache.vertices.clone();
        if let Some(split_pos) = drag_state.split_vertex {
            all_verts.push(split_pos);
        }
        let vertex_indices: Vec<usize> = brush_selection
            .sub(brush_entity)
            .map(|s| s.vertices.clone())
            .unwrap_or_default();
        drag_state.start_vertex_positions = vertex_indices
            .iter()
            .map(|&vi| all_verts.get(vi).copied().unwrap_or(Vec3::ZERO))
            .collect();
        drag_state.start_all_vertices = all_verts;
        drag_state.start_face_polygons = cache.face_polygons.clone();

        // Capture every edit brush with selected vertices so one shared world
        // offset moves them all.
        let mut captures = capture_edit_brushes(
            &brush_selection,
            &brushes,
            &brush_caches,
            &brush_transforms,
            |sub, _| sub.vertices.clone(),
        );
        // A Shift-split appends one vertex to the active brush only. The shared
        // capture builds from the live cache, which does not contain that
        // vertex, so re-derive the active brush's capture from the
        // split-extended vertex list set above.
        if drag_state.split_vertex.is_some()
            && let Some(capture) = captures.iter_mut().find(|c| c.entity == brush_entity)
        {
            let extended = &drag_state.start_all_vertices;
            capture.start_world = capture
                .indices
                .iter()
                .map(|&vi| {
                    let local = extended.get(vi).copied().unwrap_or(Vec3::ZERO);
                    brush_global.transform_point(local)
                })
                .collect();
            capture.start_all_vertices = extended.clone();
        }
        drag_state.brush_captures = captures;
        set_grab_cursor(&mut override_cursor);
    }

    if drag_state.active {
        let mouse_delta = viewport_cursor - drag_state.start_cursor;
        let primary_start = drag_state
            .start_vertex_positions
            .first()
            .copied()
            .unwrap_or(Vec3::ZERO);
        apply_shared_drag(
            drag_state.constraint,
            mouse_delta,
            primary_start,
            cam_tf,
            camera,
            brush_global,
            &snap_settings,
            ctrl,
            &drag_state.brush_captures,
            &mut brushes,
            &mut halfedge_q,
        );
    }
    OperatorResult::Running
}

fn cancel_vertex_drag(
    mut brushes: Query<&mut Brush>,
    mut halfedge_q: Query<&mut crate::brush::BrushHalfedge>,
    mut drag_state: ResMut<VertexDragState>,
    mut override_cursor: ResMut<OverrideCursor>,
) {
    restore_captures(&drag_state.brush_captures, &mut brushes, &mut halfedge_q);
    clear_vertex_drag_state(&mut drag_state);
    clear_grab_cursor(&mut override_cursor);
}

fn clear_vertex_drag_state(drag_state: &mut VertexDragState) {
    drag_state.active = false;
    drag_state.pending = None;
    drag_state.constraint = VertexDragConstraint::Free;
    drag_state.split_vertex = None;
    drag_state.start_brush = None;
    drag_state.brush_captures.clear();
    drag_state.drag_camera = None;
    drag_state.drag_viewport = None;
}

fn toggle_constraint(
    current: VertexDragConstraint,
    target: VertexDragConstraint,
) -> VertexDragConstraint {
    if current == target {
        VertexDragConstraint::Free
    } else {
        target
    }
}

// =====================================================================
// Edge drag
// =====================================================================

pub(crate) fn edge_drag_invoke_trigger(
    mouse: Res<ButtonInput<MouseButton>>,
    edit_mode: Res<EditMode>,
    drag_state: Res<EdgeDragState>,
    keybind_focus: KeybindFocus,
    vp: ViewportCursor,
    gizmo_hover: Res<crate::gizmos::GizmoHoverState>,
    mut commands: Commands,
) {
    if !mouse.just_pressed(MouseButton::Left)
        || !matches!(*edit_mode, EditMode::BrushEdit(BrushEditMode::Edge))
        || drag_state.active
        || drag_state.pending.is_some()
        || keybind_focus.is_typing()
        || vp.viewport_entity().is_none()
        // A hovered gizmo axis owns the click; the sub-element gizmo drag
        // takes it via `gizmo.drag_edit`.
        || gizmo_hover.hovered_axis.is_some()
    {
        return;
    }
    commands.queue(|world: &mut World| {
        let _ = world
            .operator(BrushEdgeDragOp::ID)
            .settings(CallOperatorSettings {
                execution_context: ExecutionContext::Invoke,
                creates_history_entry: true,
            })
            .call();
    });
}

#[operator(
    id = "brush.edge.drag",
    label = "Drag Edge",
    description = "Pick a brush edge and drag it. Modal: X / Y / Z toggle axis \
                   constraints, LMB release commits, Escape or right-click \
                   cancels.",
    modal = true,
    cancel = cancel_edge_drag,
)]
pub fn brush_edge_drag(
    _: In<OperatorParameters>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    vp: ViewportCursor,
    brush_transforms: Query<&GlobalTransform>,
    mut brush_selection: ResMut<BrushSelection>,
    mut brush_box_state: ResMut<BrushBoxSelectState>,
    brush_caches: Query<&BrushMeshCache>,
    mut brushes: Query<&mut Brush>,
    mut drag_state: ResMut<EdgeDragState>,
    modal: Option<Single<Entity, With<ActiveModalOperator>>>,
    mut halfedge_q: Query<&mut crate::brush::BrushHalfedge>,
    snap_settings: Res<SnapSettings>,
    mut override_cursor: ResMut<OverrideCursor>,
) -> OperatorResult {
    let cursor_pos = vp.cursor()?;
    let (camera_entity, viewport_entity) = if modal.is_none() {
        let camera_entity = vp.camera_entity()?;
        let viewport_entity = vp.viewport_entity()?;
        (camera_entity, viewport_entity)
    } else {
        match (drag_state.drag_camera, drag_state.drag_viewport) {
            (Some(c), Some(v)) => (c, v),
            _ => {
                let c = vp.camera_entity()?;
                let v = vp.viewport_entity()?;
                (c, v)
            }
        }
    };
    let (camera, cam_tf) = vp.camera_for(camera_entity)?;
    let viewport_cursor = vp.viewport_cursor_for(camera, viewport_entity, cursor_pos)?;
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);

    if modal.is_none() {
        // First invoke: pick the nearest edge across all edit brushes.
        let candidates: Vec<Entity> = brush_selection.edit_brushes().collect();
        if candidates.is_empty() {
            return OperatorResult::Cancelled;
        }

        let mut best: Option<(Entity, (usize, usize))> = None;
        let mut best_dist = 20.0_f32;

        for &brush_entity in &candidates {
            let Ok(cache) = brush_caches.get(brush_entity) else {
                continue;
            };
            let Ok(brush_global) = brush_transforms.get(brush_entity) else {
                continue;
            };

            for (a, b) in cache.unique_edges() {
                let wa = brush_global.transform_point(cache.vertices[a]);
                let wb = brush_global.transform_point(cache.vertices[b]);
                let Ok(sa) = camera.world_to_viewport(cam_tf, wa) else {
                    continue;
                };
                let Ok(sb) = camera.world_to_viewport(cam_tf, wb) else {
                    continue;
                };
                let dist = point_to_segment_dist(viewport_cursor, sa, sb);
                if dist < best_dist {
                    best_dist = dist;
                    best = Some((brush_entity, (a, b)));
                }
            }
        }

        let Some((brush_entity, edge)) = best else {
            // No edge hit. Ctrl is additive-toggle, so an empty
            // Ctrl-click is a no-op. Otherwise hand the press to the
            // edit-mode box-select (Shift records an add-select); staying
            // in edit mode either way.
            if !ctrl {
                brush_box_state.pending = Some(cursor_pos);
                brush_box_state.shift = shift;
            }
            return OperatorResult::Cancelled;
        };

        brush_selection.active_brush = Some(brush_entity);

        if ctrl {
            let sub = brush_selection.sub_mut(brush_entity);
            if let Some(pos) = sub.edges.iter().position(|e| *e == edge) {
                sub.edges.remove(pos);
            } else {
                sub.edges.push(edge);
            }
            return OperatorResult::Cancelled;
        }

        // Plain click on an already-selected edge: keep the whole multi-brush
        // selection and drag it as a group. Plain click on an unselected edge:
        // clear every brush's sub-selection and select just this one.
        let already_selected = brush_selection
            .sub(brush_entity)
            .is_some_and(|s| s.edges.contains(&edge));
        if !already_selected {
            brush_selection.clear_sub_selections();
            brush_selection.sub_mut(brush_entity).edges = vec![edge];
        }
        drag_state.pending = Some(PendingSubDrag {
            click_pos: cursor_pos,
        });
        drag_state.drag_camera = Some(camera_entity);
        drag_state.drag_viewport = Some(viewport_entity);
        return OperatorResult::Running;
    }

    // Subsequent invokes need the active brush.
    let brush_entity = brush_selection.active_brush?;
    let brush_global = brush_transforms.get(brush_entity)?;

    if drag_state.active {
        if keyboard.just_pressed(KeyCode::KeyX) {
            drag_state.constraint =
                toggle_constraint(drag_state.constraint, VertexDragConstraint::AxisX);
        } else if keyboard.just_pressed(KeyCode::KeyY) {
            drag_state.constraint =
                toggle_constraint(drag_state.constraint, VertexDragConstraint::AxisY);
        } else if keyboard.just_pressed(KeyCode::KeyZ) {
            drag_state.constraint =
                toggle_constraint(drag_state.constraint, VertexDragConstraint::AxisZ);
        }
    }

    if drag_state.active && mouse.just_pressed(MouseButton::Right) {
        return OperatorResult::Cancelled;
    }

    if mouse.just_released(MouseButton::Left) {
        clear_edge_drag_state(&mut drag_state);
        clear_grab_cursor(&mut override_cursor);
        return OperatorResult::Finished;
    }

    if let Some(ref pending) = drag_state.pending
        && mouse.pressed(MouseButton::Left)
        && !drag_state.active
        && (cursor_pos - pending.click_pos).length() > DRAG_THRESHOLD
        && let Ok(cache) = brush_caches.get(brush_entity)
        && let Ok(brush) = brushes.get(brush_entity)
    {
        drag_state.active = true;
        drag_state.constraint = VertexDragConstraint::Free;
        drag_state.start_brush = Some(brush.clone());
        drag_state.start_cursor = viewport_cursor;
        drag_state.start_all_vertices = cache.vertices.clone();
        drag_state.start_face_polygons = cache.face_polygons.clone();

        let drag_edges: Vec<(usize, usize)> = brush_selection
            .sub(brush_entity)
            .map(|s| s.edges.clone())
            .unwrap_or_default();
        let mut seen = std::collections::HashSet::new();
        let mut edge_verts = Vec::new();
        for &(a, b) in &drag_edges {
            if seen.insert(a) {
                edge_verts.push((a, cache.vertices.get(a).copied().unwrap_or(Vec3::ZERO)));
            }
            if seen.insert(b) {
                edge_verts.push((b, cache.vertices.get(b).copied().unwrap_or(Vec3::ZERO)));
            }
        }
        drag_state.start_edge_vertices = edge_verts;

        // Capture every edit brush with selected edges so one shared world
        // offset moves them all. `indices` are the deduped endpoint vertices of
        // that brush's selected edges.
        drag_state.brush_captures = capture_edit_brushes(
            &brush_selection,
            &brushes,
            &brush_caches,
            &brush_transforms,
            |sub, _| {
                let mut seen = std::collections::HashSet::new();
                let mut indices = Vec::new();
                for &(a, b) in &sub.edges {
                    if seen.insert(a) {
                        indices.push(a);
                    }
                    if seen.insert(b) {
                        indices.push(b);
                    }
                }
                indices
            },
        );
        set_grab_cursor(&mut override_cursor);
    }

    if drag_state.active {
        let mouse_delta = viewport_cursor - drag_state.start_cursor;
        let primary_start = drag_state
            .start_edge_vertices
            .first()
            .map(|&(_, p)| p)
            .unwrap_or(Vec3::ZERO);
        apply_shared_drag(
            drag_state.constraint,
            mouse_delta,
            primary_start,
            cam_tf,
            camera,
            brush_global,
            &snap_settings,
            ctrl,
            &drag_state.brush_captures,
            &mut brushes,
            &mut halfedge_q,
        );
    }
    OperatorResult::Running
}

fn cancel_edge_drag(
    mut brushes: Query<&mut Brush>,
    mut halfedge_q: Query<&mut crate::brush::BrushHalfedge>,
    mut drag_state: ResMut<EdgeDragState>,
    mut override_cursor: ResMut<OverrideCursor>,
) {
    restore_captures(&drag_state.brush_captures, &mut brushes, &mut halfedge_q);
    clear_edge_drag_state(&mut drag_state);
    clear_grab_cursor(&mut override_cursor);
}

fn clear_edge_drag_state(drag_state: &mut EdgeDragState) {
    drag_state.active = false;
    drag_state.pending = None;
    drag_state.constraint = VertexDragConstraint::Free;
    drag_state.start_brush = None;
    drag_state.brush_captures.clear();
    drag_state.drag_camera = None;
    drag_state.drag_viewport = None;
}

/// Build a [`BrushDragCapture`] for every edit brush whose sub-selection
/// resolves to a non-empty set of topology vertex indices. `indices_of` maps a
/// brush's sub-selection and face polygons to the vertex indices the drag
/// should move (vertex drag: the selected vertices; edge drag: the deduped
/// edge endpoints; gizmo: every selected sub-element vertex). Each capture's
/// `start_world` is the world position of those vertices at drag start, taken
/// from the live cache; callers that need a split-extended vertex list (the
/// vertex drag) adjust the active brush's capture afterward.
pub(crate) fn capture_edit_brushes(
    brush_selection: &BrushSelection,
    brushes: &Query<&mut Brush>,
    caches: &Query<&BrushMeshCache>,
    transforms: &Query<&GlobalTransform>,
    indices_of: impl Fn(&BrushSubSelection, &[Vec<usize>]) -> Vec<usize>,
) -> Vec<BrushDragCapture> {
    let mut captures = Vec::new();
    for e in brush_selection.edit_brushes() {
        let Some(sub) = brush_selection.sub(e) else {
            continue;
        };
        let Ok(e_cache) = caches.get(e) else {
            continue;
        };
        let indices = indices_of(sub, &e_cache.face_polygons);
        if indices.is_empty() {
            continue;
        }
        let Ok(e_brush) = brushes.get(e) else {
            continue;
        };
        let Ok(e_global) = transforms.get(e) else {
            continue;
        };
        let start_world: Vec<Vec3> = indices
            .iter()
            .map(|&vi| {
                let local = e_cache.vertices.get(vi).copied().unwrap_or(Vec3::ZERO);
                e_global.transform_point(local)
            })
            .collect();
        captures.push(BrushDragCapture {
            entity: e,
            start_brush: e_brush.clone(),
            start_all_vertices: e_cache.vertices.clone(),
            start_face_polygons: e_cache.face_polygons.clone(),
            indices,
            start_world,
            start_world_to_local: e_global.affine().inverse(),
        });
    }
    captures
}

/// Resolve the current mouse delta into a snapped world displacement and
/// broadcast it to every captured brush. Shared by the vertex and edge drag
/// per-frame tails, which differ only in how `primary_start` (the local start
/// position the free-drag snap rounds against) is sourced. Does nothing when
/// the offset cannot be projected this frame, matching the per-operator early
/// return.
fn apply_shared_drag(
    constraint: VertexDragConstraint,
    mouse_delta: Vec2,
    primary_start: Vec3,
    cam_tf: &GlobalTransform,
    camera: &Camera,
    brush_global: &GlobalTransform,
    snap_settings: &SnapSettings,
    ctrl: bool,
    captures: &[BrushDragCapture],
    brushes: &mut Query<&mut Brush>,
    halfedge_q: &mut Query<&mut crate::brush::BrushHalfedge>,
) {
    let Some(local_offset) =
        compute_brush_drag_offset(constraint, mouse_delta, cam_tf, camera, brush_global)
    else {
        return;
    };
    let local_offset = snap_drag_local_offset(
        local_offset,
        primary_start,
        constraint,
        brush_global,
        snap_settings,
        ctrl,
    );
    // The snapped offset is in the active brush's local space; lift it to a
    // world displacement so every captured brush can re-express it in its own
    // local space. `transform_vector3` applies rotation + scale only.
    let world_offset = brush_global.affine().transform_vector3(local_offset);
    broadcast_drag_to_captures(captures, world_offset, brushes, halfedge_q);
}

/// Move every captured brush's selected vertices by one shared world
/// displacement. Each capture offsets its start world positions, converts the
/// result back to its own local space via the inverse affine taken at drag
/// start, and rebuilds through [`apply_vertex_deltas`].
fn broadcast_drag_to_captures(
    captures: &[BrushDragCapture],
    world_offset: Vec3,
    brushes: &mut Query<&mut Brush>,
    halfedge_q: &mut Query<&mut crate::brush::BrushHalfedge>,
) {
    for capture in captures {
        let new_positions: Vec<Vec3> = capture
            .start_world
            .iter()
            .map(|&w| capture.start_world_to_local.transform_point3(w + world_offset))
            .collect();
        let Ok(mut brush) = brushes.get_mut(capture.entity) else {
            continue;
        };
        let mut halfedge_opt = halfedge_q.get_mut(capture.entity).ok();
        apply_vertex_deltas(
            &mut brush,
            halfedge_opt.as_deref_mut(),
            &capture.start_brush,
            &capture.start_all_vertices,
            &capture.start_face_polygons,
            &capture.indices,
            &new_positions,
        );
    }
}

/// Restore every captured brush to its drag-start topology and re-lift its
/// half-edge mesh. Shared by the vertex, edge, and edit-gizmo drag cancel
/// handlers.
pub(crate) fn restore_captures(
    captures: &[BrushDragCapture],
    brushes: &mut Query<&mut Brush>,
    halfedge_q: &mut Query<&mut crate::brush::BrushHalfedge>,
) {
    for capture in captures {
        let Ok(mut brush) = brushes.get_mut(capture.entity) else {
            continue;
        };
        *brush = capture.start_brush.clone();
        if let Ok(mut halfedge) = halfedge_q.get_mut(capture.entity) {
            *halfedge = crate::brush::BrushHalfedge::from_topology(&capture.start_brush.topology);
        }
    }
}

// =====================================================================
// Reusable vertex-delta application
// =====================================================================

/// Move a subset of brush vertices to new absolute brush-local positions and
/// rebuild the geometry.
///
/// `indices[i]` is the topology vertex index (into `all_vertices` and, when
/// present, `halfedge.vert_keys`) that should move. `new_positions[i]` is the
/// absolute brush-local destination for that vertex.
///
/// Two code paths, matching the runtime behaviour in `brush_vertex_drag`:
///
/// - If `halfedge` is `Some`, the `HalfedgeMesh` path is taken: vertex
///   coordinates are mutated directly, face normals are recomputed from the
///   new ring positions, and the result is flattened back into
///   `brush.topology` and `brush.faces[*].plane`.
/// - If `halfedge` is `None`, the legacy Quickhull path is taken: a new
///   vertex list is built from `all_vertices` with the selected positions
///   overwritten, then `rebuild_brush_from_vertices` derives the convex hull.
///   `start_brush`, `all_vertices`, and `face_polygons` are only read on this
///   path; they may be empty slices when the `HalfedgeMesh` path is guaranteed.
pub fn apply_vertex_deltas(
    brush: &mut Brush,
    halfedge: Option<&mut crate::brush::BrushHalfedge>,
    start_brush: &Brush,
    all_vertices: &[Vec3],
    face_polygons: &[Vec<usize>],
    indices: &[usize],
    new_positions: &[Vec3],
) {
    if let Some(halfedge) = halfedge {
        let vert_keys = halfedge.vert_keys.clone();
        for (sel_idx, &vert_idx) in indices.iter().enumerate() {
            if sel_idx < new_positions.len() && vert_idx < vert_keys.len() {
                let key = vert_keys[vert_idx];
                if let Some(v) = halfedge.mesh.verts.get_mut(key) {
                    v.co = new_positions[sel_idx];
                }
            }
        }
        let face_keys: Vec<_> = halfedge.mesh.faces.keys().collect();
        for fk in face_keys {
            let face = &halfedge.mesh.faces[fk];
            let mut ring_positions = Vec::with_capacity(face.loop_count as usize);
            let mut cur = face.loop_first;
            for _ in 0..face.loop_count {
                let lp = &halfedge.mesh.loops[cur];
                ring_positions.push(halfedge.mesh.verts[lp.vert].co);
                cur = lp.next;
            }
            let new_normal = jackdaw_geometry::newell_normal(&ring_positions);
            halfedge.mesh.faces[fk].normal_cache = new_normal;
        }
        let new_topology = halfedge.mesh.flatten_to_topology();
        let positions: Vec<Vec3> = new_topology.vertices.iter().map(|v| v.position).collect();
        for (face_idx, face_data) in brush.faces.iter_mut().enumerate() {
            if face_idx < new_topology.polygons.len() {
                let normal = new_topology.face_normal_with(&positions, face_idx);
                let v0_idx =
                    new_topology.loops[new_topology.polygons[face_idx].loop_start as usize].vert
                        as usize;
                let distance = positions[v0_idx].dot(normal);
                face_data.plane.normal = normal;
                face_data.plane.distance = distance;
            }
        }
        brush.topology = new_topology;
    } else {
        let mut new_verts = all_vertices.to_vec();
        for (sel_idx, &vert_idx) in indices.iter().enumerate() {
            if sel_idx < new_positions.len() && vert_idx < new_verts.len() {
                new_verts[vert_idx] = new_positions[sel_idx];
            }
        }
        if let Some((new_brush, _)) =
            rebuild_brush_from_vertices(start_brush, all_vertices, face_polygons, &new_verts)
        {
            *brush = new_brush;
        }
    }
}

#[cfg(test)]
mod apply_vertex_deltas_tests {
    use super::*;
    use jackdaw_geometry::halfedge::HalfedgeMesh;

    fn make_halfedge(brush: &Brush) -> crate::brush::BrushHalfedge {
        let mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
        let vert_keys: Vec<_> = mesh.verts.keys().collect();
        let mut face_keys: Vec<jackdaw_geometry::halfedge::FaceKey> =
            vec![Default::default(); mesh.faces.len()];
        for (k, f) in mesh.faces.iter() {
            let slot = f.material_idx as usize;
            if slot < face_keys.len() {
                face_keys[slot] = k;
            }
        }
        crate::brush::BrushHalfedge {
            mesh,
            vert_keys,
            face_keys,
        }
    }

    #[test]
    fn moving_a_vertex_lands_it_and_preserves_faces() {
        let mut brush = Brush::cuboid(1.0, 1.0, 1.0);
        let mut halfedge = make_halfedge(&brush);
        let faces_before = brush.faces.len();

        // Read vertex 0's current position from the topology.
        let v0_before = brush.topology.vertices[0].position;
        let delta = Vec3::new(0.5, 0.0, 0.0);
        let new_pos = v0_before + delta;

        let start_brush = brush.clone();
        apply_vertex_deltas(
            &mut brush,
            Some(&mut halfedge),
            &start_brush,
            &[],
            &[],
            &[0],
            &[new_pos],
        );

        // The moved vertex should appear somewhere in the refreshed topology.
        let found = brush
            .topology
            .vertices
            .iter()
            .any(|v| (v.position - new_pos).length() < 1e-3);
        assert!(
            found,
            "no vertex near {new_pos:?} after move; topology = {:?}",
            brush.topology.vertices.iter().map(|v| v.position).collect::<Vec<_>>()
        );
        assert_eq!(
            brush.faces.len(),
            faces_before,
            "face count should not change"
        );
    }
}
