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

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;
use jackdaw_jsn::Brush;

use crate::brush::interaction::{
    FaceExtrudeMode, PendingSubDrag, VertexDragConstraint, compute_brush_drag_offset,
};
use crate::brush::{
    BrushDragState, BrushEditMode, BrushFaceEntity, BrushMeshCache, BrushSelection, EdgeDragState,
    EditMode, SetBrush, VertexDragState, rebuild_brush_from_vertices,
};
use crate::commands::CommandHistory;
use crate::draw_brush::{CreateBrushCommand, DrawBrushState, brush_data_from_entity};
use crate::keybind_focus::KeybindFocus;
use crate::modal_transform::ModalTransformState;
use crate::selection::{Selected, Selection};
use crate::snapping::SnapSettings;
use crate::viewport::ViewportCursor;
use crate::viewport_util::{point_in_polygon_2d, point_to_segment_dist};

/// Minimum extrude depth before commit pushes a new brush.
const MIN_EXTRUDE_DEPTH: f32 = 0.01;
/// Pixels the cursor must travel after a press to promote pending → active.
const DRAG_THRESHOLD: f32 = 5.0;

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
/// Mirrors the face hit-test in `brush_face_drag`. Used by other
/// invoke triggers (notably box-select) that must yield to
/// face-drag when the user shift-clicks on a brush face. Without
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
    mut commands: Commands,
) {
    if !mouse.just_pressed(MouseButton::Left) || drag_state.active || drag_state.pending.is_some() {
        return;
    }
    let in_face_edit = matches!(*edit_mode, EditMode::BrushEdit(BrushEditMode::Face));
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
                creates_history_entry: false,
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
    allows_undo = false,
    cancel = cancel_face_drag,
)]
pub fn brush_face_drag(
    _: In<OperatorParameters>,
    mut edit_mode: ResMut<EditMode>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    vp: ViewportCursor,
    face_entities: Query<(Entity, &BrushFaceEntity, &GlobalTransform)>,
    mut brush_selection: ResMut<BrushSelection>,
    brush_caches: Query<&BrushMeshCache>,
    selection: Res<Selection>,
    mut brushes: Query<(&mut Brush, &GlobalTransform)>,
    mut drag_state: ResMut<BrushDragState>,
    mut history: ResMut<CommandHistory>,
    mut commands: Commands,
    snap_settings: Res<SnapSettings>,
    modal: Option<Single<Entity, With<ActiveModalOperator>>>,
) -> OperatorResult {
    let Ok(window) = vp.windows.single() else {
        return OperatorResult::Cancelled;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return OperatorResult::Cancelled;
    };
    // First invoke uses the hovered viewport; subsequent invokes use
    // the captured one so the drag stays bound to its origin panel.
    let (camera_entity, viewport_entity) = if modal.is_none() {
        let Some(camera_entity) = vp.camera_entity() else {
            return OperatorResult::Cancelled;
        };
        let Some(viewport_entity) = vp.viewport_entity() else {
            return OperatorResult::Cancelled;
        };
        (camera_entity, viewport_entity)
    } else {
        match (drag_state.drag_camera, drag_state.drag_viewport) {
            (Some(c), Some(v)) => (c, v),
            _ => {
                let Some(c) = vp.camera_entity() else {
                    return OperatorResult::Cancelled;
                };
                let Some(v) = vp.viewport_entity() else {
                    return OperatorResult::Cancelled;
                };
                (c, v)
            }
        }
    };
    let Some((camera, cam_tf)) = vp.camera_for(camera_entity) else {
        return OperatorResult::Cancelled;
    };
    let Some(viewport_cursor) = vp.viewport_cursor_for(camera, viewport_entity, cursor_pos) else {
        return OperatorResult::Cancelled;
    };

    let in_face_edit = matches!(*edit_mode, EditMode::BrushEdit(BrushEditMode::Face));
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let alt = keyboard.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);

    if modal.is_none() {
        // First-invoke: pick a face under the cursor, set pending /
        // selection / quick-action.
        let brush_entity = if in_face_edit {
            brush_selection.entity
        } else {
            selection.primary().filter(|&e| brushes.contains(e))
        };
        let Some(brush_entity) = brush_entity else {
            return OperatorResult::Cancelled;
        };
        let Ok(cache) = brush_caches.get(brush_entity) else {
            return OperatorResult::Cancelled;
        };

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
            // No face hit. If we were in face mode and not Ctrl-clicking,
            // exit to Object as the legacy click-out behavior.
            if in_face_edit && !ctrl {
                *edit_mode = EditMode::Object;
                brush_selection.clear();
            }
            return OperatorResult::Cancelled;
        };

        if !in_face_edit {
            *edit_mode = EditMode::BrushEdit(BrushEditMode::Face);
            brush_selection.entity = Some(brush_entity);
            brush_selection.faces.clear();
            brush_selection.vertices.clear();
            brush_selection.edges.clear();
            drag_state.quick_action = true;
        }

        if in_face_edit && ctrl {
            // Ctrl+click in face mode: toggle multi-select (no drag).
            if let Some(pos) = brush_selection.faces.iter().position(|&f| f == face_idx) {
                brush_selection.faces.remove(pos);
            } else {
                brush_selection.faces.push(face_idx);
            }
            return OperatorResult::Cancelled;
        }

        brush_selection.faces = vec![face_idx];
        drag_state.extrude_mode = if !in_face_edit && alt {
            FaceExtrudeMode::Extend
        } else {
            if in_face_edit {
                drag_state.quick_action = false;
            }
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
    // pending → active promotion, and per-frame drag math.
    if drag_state.active && mouse.just_pressed(MouseButton::Right) {
        return OperatorResult::Cancelled;
    }

    if mouse.just_released(MouseButton::Left) {
        if drag_state.active {
            match drag_state.extrude_mode {
                FaceExtrudeMode::Merge => {
                    if let Some(brush_entity) = brush_selection.entity
                        && let Some(ref start) = drag_state.start_brush
                        && let Ok((brush, _)) = brushes.get(brush_entity)
                    {
                        history.push_executed(Box::new(SetBrush {
                            entity: brush_entity,
                            old: start.clone(),
                            new: brush.clone(),
                            label: "Move brush face".to_string(),
                        }));
                    }
                }
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
        && let Some(brush_entity) = brush_selection.entity
        && let Ok((brush, brush_global)) = brushes.get(brush_entity)
    {
        drag_state.active = true;
        drag_state.start_cursor = viewport_cursor;
        if let Some(&face_idx) = brush_selection.faces.first()
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
                    && let Some(&face_idx) = brush_selection.faces.first()
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
        let Some(brush_entity) = brush_selection.entity else {
            return OperatorResult::Cancelled;
        };
        match drag_state.extrude_mode {
            FaceExtrudeMode::Merge => {
                let Ok((mut brush, brush_global)) = brushes.get_mut(brush_entity) else {
                    return OperatorResult::Cancelled;
                };
                let Some(ref start) = drag_state.start_brush else {
                    return OperatorResult::Cancelled;
                };
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
                for &face_idx in &brush_selection.faces {
                    if face_idx < start.faces.len() && face_idx < brush.faces.len() {
                        brush.faces[face_idx].plane.distance =
                            start.faces[face_idx].plane.distance + drag_amount;
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
) {
    if drag_state.extrude_mode == FaceExtrudeMode::Merge
        && let Some(brush_entity) = brush_selection.entity
        && let Some(ref start) = drag_state.start_brush
        && let Ok(mut brush) = brushes.get_mut(brush_entity)
    {
        *brush = start.clone();
    }
    let was_quick = drag_state.quick_action;
    clear_face_drag_state(&mut drag_state);
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

        let cmd = CreateBrushCommand {
            data: brush_data_from_entity(world, entity),
        };
        world
            .resource_mut::<CommandHistory>()
            .push_executed(Box::new(cmd));
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
    mut commands: Commands,
) {
    if !mouse.just_pressed(MouseButton::Left)
        || !matches!(*edit_mode, EditMode::BrushEdit(BrushEditMode::Vertex))
        || drag_state.active
        || drag_state.pending.is_some()
        || keybind_focus.is_typing()
    {
        return;
    }
    commands.queue(|world: &mut World| {
        let _ = world
            .operator(BrushVertexDragOp::ID)
            .settings(CallOperatorSettings {
                execution_context: ExecutionContext::Invoke,
                creates_history_entry: false,
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
    allows_undo = false,
    cancel = cancel_vertex_drag,
)]
pub fn brush_vertex_drag(
    _: In<OperatorParameters>,
    mut edit_mode: ResMut<EditMode>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    vp: ViewportCursor,
    brush_transforms: Query<&GlobalTransform>,
    mut brush_selection: ResMut<BrushSelection>,
    brush_caches: Query<&BrushMeshCache>,
    mut brushes: Query<&mut Brush>,
    mut drag_state: ResMut<VertexDragState>,
    mut history: ResMut<CommandHistory>,
    modal: Option<Single<Entity, With<ActiveModalOperator>>>,
) -> OperatorResult {
    let Some(brush_entity) = brush_selection.entity else {
        return OperatorResult::Cancelled;
    };
    let Ok(window) = vp.windows.single() else {
        return OperatorResult::Cancelled;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return OperatorResult::Cancelled;
    };
    let (camera_entity, viewport_entity) = if modal.is_none() {
        let Some(camera_entity) = vp.camera_entity() else {
            return OperatorResult::Cancelled;
        };
        let Some(viewport_entity) = vp.viewport_entity() else {
            return OperatorResult::Cancelled;
        };
        (camera_entity, viewport_entity)
    } else {
        match (drag_state.drag_camera, drag_state.drag_viewport) {
            (Some(c), Some(v)) => (c, v),
            _ => {
                let Some(c) = vp.camera_entity() else {
                    return OperatorResult::Cancelled;
                };
                let Some(v) = vp.viewport_entity() else {
                    return OperatorResult::Cancelled;
                };
                (c, v)
            }
        }
    };
    let Some((camera, cam_tf)) = vp.camera_for(camera_entity) else {
        return OperatorResult::Cancelled;
    };
    let Some(viewport_cursor) = vp.viewport_cursor_for(camera, viewport_entity, cursor_pos) else {
        return OperatorResult::Cancelled;
    };
    let Ok(brush_global) = brush_transforms.get(brush_entity) else {
        return OperatorResult::Cancelled;
    };

    let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);

    if modal.is_none() {
        // First invoke: pick vertex / split vertex.
        let Ok(cache) = brush_caches.get(brush_entity) else {
            return OperatorResult::Cancelled;
        };

        if shift && !ctrl {
            // Shift+click: pick edge midpoint or face center to split.
            let mut unique_edges: Vec<(usize, usize)> = Vec::new();
            for polygon in &cache.face_polygons {
                if polygon.len() < 2 {
                    continue;
                }
                for i in 0..polygon.len() {
                    let a = polygon[i];
                    let b = polygon[(i + 1) % polygon.len()];
                    let edge = (a.min(b), a.max(b));
                    if !unique_edges.contains(&edge) {
                        unique_edges.push(edge);
                    }
                }
            }
            let mut best_split: Option<Vec3> = None;
            let mut best_dist = 20.0_f32;
            for &(a, b) in &unique_edges {
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
            let Some(split_pos) = best_split else {
                return OperatorResult::Cancelled;
            };
            let new_idx = cache.vertices.len();
            brush_selection.vertices = vec![new_idx];
            drag_state.split_vertex = Some(split_pos);
            drag_state.pending = Some(PendingSubDrag {
                click_pos: cursor_pos,
            });
            return OperatorResult::Running;
        }

        let mut best_vert = None;
        let mut best_dist = 20.0_f32;
        for (vi, v) in cache.vertices.iter().enumerate() {
            if let Ok(screen) = camera.world_to_viewport(cam_tf, brush_global.transform_point(*v)) {
                let dist = (screen - viewport_cursor).length();
                if dist < best_dist {
                    best_dist = dist;
                    best_vert = Some(vi);
                }
            }
        }
        let Some(vi) = best_vert else {
            if !ctrl {
                *edit_mode = EditMode::Object;
                brush_selection.clear();
            }
            return OperatorResult::Cancelled;
        };
        if ctrl {
            if let Some(pos) = brush_selection.vertices.iter().position(|&v| v == vi) {
                brush_selection.vertices.remove(pos);
            } else {
                brush_selection.vertices.push(vi);
            }
            return OperatorResult::Cancelled;
        }
        brush_selection.vertices = vec![vi];
        drag_state.pending = Some(PendingSubDrag {
            click_pos: cursor_pos,
        });
        drag_state.drag_camera = Some(camera_entity);
        drag_state.drag_viewport = Some(viewport_entity);
        return OperatorResult::Running;
    }

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
        if drag_state.active
            && let Some(ref start) = drag_state.start_brush
            && let Ok(brush) = brushes.get(brush_entity)
        {
            let label = if drag_state.split_vertex.is_some() {
                "Split brush vertex"
            } else {
                "Move brush vertex"
            };
            history.push_executed(Box::new(SetBrush {
                entity: brush_entity,
                old: start.clone(),
                new: brush.clone(),
                label: label.to_string(),
            }));
        }
        clear_vertex_drag_state(&mut drag_state);
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
        drag_state.start_vertex_positions = brush_selection
            .vertices
            .iter()
            .map(|&vi| all_verts.get(vi).copied().unwrap_or(Vec3::ZERO))
            .collect();
        drag_state.start_all_vertices = all_verts;
        drag_state.start_face_polygons = cache.face_polygons.clone();
    }

    if drag_state.active {
        let Ok(mut brush) = brushes.get_mut(brush_entity) else {
            return OperatorResult::Cancelled;
        };
        let Some(ref start) = drag_state.start_brush else {
            return OperatorResult::Cancelled;
        };
        let mouse_delta = viewport_cursor - drag_state.start_cursor;
        let Some(local_offset) = compute_brush_drag_offset(
            drag_state.constraint,
            mouse_delta,
            cam_tf,
            camera,
            brush_global,
        ) else {
            return OperatorResult::Running;
        };
        let mut new_verts = drag_state.start_all_vertices.clone();
        for (sel_idx, &vert_idx) in brush_selection.vertices.iter().enumerate() {
            if sel_idx < drag_state.start_vertex_positions.len() && vert_idx < new_verts.len() {
                new_verts[vert_idx] = drag_state.start_vertex_positions[sel_idx] + local_offset;
            }
        }
        if let Some((new_brush, _)) = rebuild_brush_from_vertices(
            start,
            &drag_state.start_all_vertices,
            &drag_state.start_face_polygons,
            &new_verts,
        ) {
            *brush = new_brush;
        }
    }
    OperatorResult::Running
}

fn cancel_vertex_drag(
    brush_selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut drag_state: ResMut<VertexDragState>,
) {
    if let Some(brush_entity) = brush_selection.entity
        && let Some(ref start) = drag_state.start_brush
        && let Ok(mut brush) = brushes.get_mut(brush_entity)
    {
        *brush = start.clone();
    }
    clear_vertex_drag_state(&mut drag_state);
}

fn clear_vertex_drag_state(drag_state: &mut VertexDragState) {
    drag_state.active = false;
    drag_state.pending = None;
    drag_state.constraint = VertexDragConstraint::Free;
    drag_state.split_vertex = None;
    drag_state.start_brush = None;
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
    mut commands: Commands,
) {
    if !mouse.just_pressed(MouseButton::Left)
        || !matches!(*edit_mode, EditMode::BrushEdit(BrushEditMode::Edge))
        || drag_state.active
        || drag_state.pending.is_some()
        || keybind_focus.is_typing()
    {
        return;
    }
    commands.queue(|world: &mut World| {
        let _ = world
            .operator(BrushEdgeDragOp::ID)
            .settings(CallOperatorSettings {
                execution_context: ExecutionContext::Invoke,
                creates_history_entry: false,
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
    allows_undo = false,
    cancel = cancel_edge_drag,
)]
pub fn brush_edge_drag(
    _: In<OperatorParameters>,
    mut edit_mode: ResMut<EditMode>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    vp: ViewportCursor,
    brush_transforms: Query<&GlobalTransform>,
    mut brush_selection: ResMut<BrushSelection>,
    brush_caches: Query<&BrushMeshCache>,
    mut brushes: Query<&mut Brush>,
    mut drag_state: ResMut<EdgeDragState>,
    mut history: ResMut<CommandHistory>,
    modal: Option<Single<Entity, With<ActiveModalOperator>>>,
) -> OperatorResult {
    let Some(brush_entity) = brush_selection.entity else {
        return OperatorResult::Cancelled;
    };
    let Ok(window) = vp.windows.single() else {
        return OperatorResult::Cancelled;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return OperatorResult::Cancelled;
    };
    let (camera_entity, viewport_entity) = if modal.is_none() {
        let Some(camera_entity) = vp.camera_entity() else {
            return OperatorResult::Cancelled;
        };
        let Some(viewport_entity) = vp.viewport_entity() else {
            return OperatorResult::Cancelled;
        };
        (camera_entity, viewport_entity)
    } else {
        match (drag_state.drag_camera, drag_state.drag_viewport) {
            (Some(c), Some(v)) => (c, v),
            _ => {
                let Some(c) = vp.camera_entity() else {
                    return OperatorResult::Cancelled;
                };
                let Some(v) = vp.viewport_entity() else {
                    return OperatorResult::Cancelled;
                };
                (c, v)
            }
        }
    };
    let Some((camera, cam_tf)) = vp.camera_for(camera_entity) else {
        return OperatorResult::Cancelled;
    };
    let Some(viewport_cursor) = vp.viewport_cursor_for(camera, viewport_entity, cursor_pos) else {
        return OperatorResult::Cancelled;
    };
    let Ok(brush_global) = brush_transforms.get(brush_entity) else {
        return OperatorResult::Cancelled;
    };
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);

    if modal.is_none() {
        // First invoke: pick edge.
        let Ok(cache) = brush_caches.get(brush_entity) else {
            return OperatorResult::Cancelled;
        };

        let mut unique_edges: Vec<(usize, usize)> = Vec::new();
        for polygon in &cache.face_polygons {
            if polygon.len() < 2 {
                continue;
            }
            for i in 0..polygon.len() {
                let a = polygon[i];
                let b = polygon[(i + 1) % polygon.len()];
                let edge = (a.min(b), a.max(b));
                if !unique_edges.contains(&edge) {
                    unique_edges.push(edge);
                }
            }
        }

        let mut best_edge = None;
        let mut best_dist = 20.0_f32;
        for &(a, b) in &unique_edges {
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
                best_edge = Some((a, b));
            }
        }

        let Some(edge) = best_edge else {
            if !ctrl {
                *edit_mode = EditMode::Object;
                brush_selection.clear();
            }
            return OperatorResult::Cancelled;
        };
        if ctrl {
            if let Some(pos) = brush_selection.edges.iter().position(|e| *e == edge) {
                brush_selection.edges.remove(pos);
            } else {
                brush_selection.edges.push(edge);
            }
            return OperatorResult::Cancelled;
        }
        brush_selection.edges = vec![edge];
        drag_state.pending = Some(PendingSubDrag {
            click_pos: cursor_pos,
        });
        drag_state.drag_camera = Some(camera_entity);
        drag_state.drag_viewport = Some(viewport_entity);
        return OperatorResult::Running;
    }

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
        if drag_state.active
            && let Some(ref start) = drag_state.start_brush
            && let Ok(brush) = brushes.get(brush_entity)
        {
            history.push_executed(Box::new(SetBrush {
                entity: brush_entity,
                old: start.clone(),
                new: brush.clone(),
                label: "Move brush edge".to_string(),
            }));
        }
        clear_edge_drag_state(&mut drag_state);
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

        let mut seen = std::collections::HashSet::new();
        let mut edge_verts = Vec::new();
        for &(a, b) in &brush_selection.edges {
            if seen.insert(a) {
                edge_verts.push((a, cache.vertices.get(a).copied().unwrap_or(Vec3::ZERO)));
            }
            if seen.insert(b) {
                edge_verts.push((b, cache.vertices.get(b).copied().unwrap_or(Vec3::ZERO)));
            }
        }
        drag_state.start_edge_vertices = edge_verts;
    }

    if drag_state.active {
        let Ok(mut brush) = brushes.get_mut(brush_entity) else {
            return OperatorResult::Cancelled;
        };
        let Some(ref start) = drag_state.start_brush else {
            return OperatorResult::Cancelled;
        };
        let mouse_delta = viewport_cursor - drag_state.start_cursor;
        let Some(local_offset) = compute_brush_drag_offset(
            drag_state.constraint,
            mouse_delta,
            cam_tf,
            camera,
            brush_global,
        ) else {
            return OperatorResult::Running;
        };
        let mut new_verts = drag_state.start_all_vertices.clone();
        for &(vi, start_pos) in &drag_state.start_edge_vertices {
            if vi < new_verts.len() {
                new_verts[vi] = start_pos + local_offset;
            }
        }
        if let Some((new_brush, _)) = rebuild_brush_from_vertices(
            start,
            &drag_state.start_all_vertices,
            &drag_state.start_face_polygons,
            &new_verts,
        ) {
            *brush = new_brush;
        }
    }
    OperatorResult::Running
}

fn cancel_edge_drag(
    brush_selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut drag_state: ResMut<EdgeDragState>,
) {
    if let Some(brush_entity) = brush_selection.entity
        && let Some(ref start) = drag_state.start_brush
        && let Ok(mut brush) = brushes.get_mut(brush_entity)
    {
        *brush = start.clone();
    }
    clear_edge_drag_state(&mut drag_state);
}

fn clear_edge_drag_state(drag_state: &mut EdgeDragState) {
    drag_state.active = false;
    drag_state.pending = None;
    drag_state.constraint = VertexDragConstraint::Free;
    drag_state.start_brush = None;
    drag_state.drag_camera = None;
    drag_state.drag_viewport = None;
}
