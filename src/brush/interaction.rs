use bevy::{input_focus::InputFocus, prelude::*};

use crate::default_style;
use crate::{
    selection::Selection, viewport::MainViewportCamera, viewport_util::point_in_polygon_2d,
};

use super::{BrushEditMode, BrushMeshCache, BrushSelection, EditMode};
use jackdaw_geometry::{brush_planes_to_world, compute_brush_geometry};
use jackdaw_jsn::{Brush, BrushFaceData, BrushPlane};

/// Reactive cleanup: when the active brush entity is no longer
/// selected, drop out of brush-edit mode. The digit-key mode switches
/// (1/2/3/4) and the Escape exit-to-Object live in
/// [`crate::edit_mode_ops`].
pub(super) fn drop_brush_edit_on_deselect(
    input_focus: Res<InputFocus>,
    selection: Res<Selection>,
    mut edit_mode: ResMut<EditMode>,
    mut brush_selection: ResMut<BrushSelection>,
    modal: Res<crate::modal_transform::ModalTransformState>,
) {
    if input_focus.0.is_some() || modal.active.is_some() {
        return;
    }

    if let EditMode::BrushEdit(_) = *edit_mode
        && let Some(brush_entity) = brush_selection.entity
        && selection.primary() != Some(brush_entity)
    {
        // Save last selected face for extend-to-brush fallback
        if !brush_selection.faces.is_empty() {
            brush_selection.last_face_entity = Some(brush_entity);
            brush_selection.last_face_index = brush_selection.faces.last().copied();
        }
        *edit_mode = EditMode::Object;
        brush_selection.clear();
    }
}

pub(crate) struct PendingSubDrag {
    pub click_pos: Vec2,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum FaceExtrudeMode {
    #[default]
    Merge, // Push/pull existing face plane
    Extend, // Create new brush from face extrusion
}

#[derive(Resource, Default)]
pub(crate) struct BrushDragState {
    pub pending: Option<PendingSubDrag>,
    pub active: bool,
    pub extrude_mode: FaceExtrudeMode,
    /// When true, exits to Object mode when drag completes or is cancelled.
    pub quick_action: bool,
    pub(crate) start_brush: Option<Brush>,
    pub(crate) start_cursor: Vec2,
    pub(crate) drag_face_normal: Vec3,
    /// World-space face polygon vertices for extend preview.
    pub extend_face_polygon: Vec<Vec3>,
    /// World-space face normal for extend preview.
    pub extend_face_normal: Vec3,
    /// Current extrude depth during extend drag.
    pub extend_depth: f32,
    /// Multi-viewport: camera + UI-node entities captured at drag
    /// start so the drag stays bound to its origin viewport even if
    /// the cursor wanders into another panel.
    pub(crate) drag_camera: Option<Entity>,
    pub(crate) drag_viewport: Option<Entity>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum VertexDragConstraint {
    #[default]
    Free,
    AxisX,
    AxisY,
    AxisZ,
}

#[derive(Resource, Default)]
pub(crate) struct VertexDragState {
    pub pending: Option<PendingSubDrag>,
    pub active: bool,
    pub constraint: VertexDragConstraint,
    pub(crate) start_brush: Option<Brush>,
    pub(crate) start_cursor: Vec2,
    pub(crate) start_vertex_positions: Vec<Vec3>,
    /// Full vertex list at drag start (for hull rebuild).
    pub(crate) start_all_vertices: Vec<Vec3>,
    /// Per-face polygon indices at drag start (for hull rebuild).
    pub(crate) start_face_polygons: Vec<Vec<usize>>,
    /// New vertex position for Shift+drag split (edge midpoint or face center).
    pub(crate) split_vertex: Option<Vec3>,
    /// Multi-viewport: see [`BrushDragState::drag_camera`].
    pub(crate) drag_camera: Option<Entity>,
    pub(crate) drag_viewport: Option<Entity>,
}

/// Compute a local-space offset for brush vertex/edge drag based on mouse movement.
pub(crate) fn compute_brush_drag_offset(
    constraint: VertexDragConstraint,
    mouse_delta: Vec2,
    cam_tf: &GlobalTransform,
    camera: &Camera,
    brush_global: &GlobalTransform,
) -> Option<Vec3> {
    let brush_pos = brush_global.translation();
    let cam_dist = (cam_tf.translation() - brush_pos).length();
    let scale = cam_dist * 0.003;

    let offset = match constraint {
        VertexDragConstraint::Free => {
            let cam_right = cam_tf.right().as_vec3();
            let cam_up = cam_tf.up().as_vec3();
            let world_offset =
                cam_right * mouse_delta.x * scale + cam_up * (-mouse_delta.y) * scale;
            let (_, brush_rot, _) = brush_global.to_scale_rotation_translation();
            brush_rot.inverse() * world_offset
        }
        constraint => {
            let axis_dir = match constraint {
                VertexDragConstraint::AxisX => Vec3::X,
                VertexDragConstraint::AxisY => Vec3::Y,
                VertexDragConstraint::AxisZ => Vec3::Z,
                VertexDragConstraint::Free => unreachable!(),
            };
            let origin_screen = camera.world_to_viewport(cam_tf, brush_pos).ok()?;
            let (_, brush_rot, _) = brush_global.to_scale_rotation_translation();
            let world_axis = brush_rot * axis_dir;
            let axis_screen = camera
                .world_to_viewport(cam_tf, brush_pos + world_axis)
                .ok()?;
            let screen_axis = (axis_screen - origin_screen).normalize_or_zero();
            let projected = mouse_delta.dot(screen_axis);
            axis_dir * projected * scale
        }
    };
    Some(offset)
}

#[derive(Resource, Default)]
pub(crate) struct EdgeDragState {
    pub pending: Option<PendingSubDrag>,
    pub active: bool,
    pub constraint: VertexDragConstraint,
    pub(crate) start_brush: Option<Brush>,
    pub(crate) start_cursor: Vec2,
    /// Start positions for each selected edge's two endpoints (vertex indices + positions).
    pub(crate) start_edge_vertices: Vec<(usize, Vec3)>,
    /// Full vertex list at drag start (for hull rebuild).
    pub(crate) start_all_vertices: Vec<Vec3>,
    /// Per-face polygon indices at drag start (for hull rebuild).
    pub(crate) start_face_polygons: Vec<Vec<usize>>,
    /// Multi-viewport: see [`BrushDragState::drag_camera`].
    pub(crate) drag_camera: Option<Entity>,
    pub(crate) drag_viewport: Option<Entity>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ClipMode {
    #[default]
    KeepFront,
    KeepBack,
    Split,
}

#[derive(Resource, Default)]
pub(crate) struct ClipState {
    pub points: Vec<Vec3>,
    pub preview_plane: Option<BrushPlane>,
    pub mode: ClipMode,
}

/// Recompute the clip preview plane from `ClipState.points` and draw
/// the clip overlay (points + front/back wireframes). Mutations
/// (placing points, cycling mode, applying, clearing) live in the
/// `brush.clip.*` operators in [`crate::clip_ops`].
pub(super) fn handle_clip_mode(
    edit_mode: Res<EditMode>,
    camera_query: Query<(Entity, &Camera, &GlobalTransform), With<MainViewportCamera>>,
    active: Res<crate::viewport::ActiveViewport>,
    brush_selection: Res<BrushSelection>,
    brushes: Query<&Brush>,
    brush_transforms: Query<&GlobalTransform>,
    mut clip_state: ResMut<ClipState>,
    mut gizmos: Gizmos,
) {
    let EditMode::BrushEdit(BrushEditMode::Clip) = *edit_mode else {
        // Clear clip state when not in clip mode
        if !clip_state.points.is_empty() || clip_state.mode != ClipMode::KeepFront {
            *clip_state = ClipState::default();
        }
        return;
    };

    let Some(brush_entity) = brush_selection.entity else {
        return;
    };
    let Ok(brush_global) = brush_transforms.get(brush_entity) else {
        return;
    };
    // Multi-viewport: use the hovered viewport's camera for clip-plane
    // orientation; fall back to any camera so the preview keeps working
    // when no viewport is focused.
    let cam_tf = active
        .camera
        .and_then(|e| camera_query.get(e).ok())
        .or_else(|| camera_query.iter().next())
        .map(|(_, _, tf)| tf);
    let Some(cam_tf) = cam_tf else {
        return;
    };

    // Compute preview plane from collected points
    clip_state.preview_plane = match clip_state.points.len() {
        2 => {
            // Two points + camera forward for orientation
            let dir = clip_state.points[1] - clip_state.points[0];
            let (_, brush_rot, _) = brush_global.to_scale_rotation_translation();
            let local_cam_fwd = brush_rot.inverse() * cam_tf.forward().as_vec3();
            let normal = dir.cross(local_cam_fwd).normalize_or_zero();
            if normal.length_squared() > 0.5 {
                let distance = normal.dot(clip_state.points[0]);
                Some(BrushPlane { normal, distance })
            } else {
                None
            }
        }
        3 => {
            let a = clip_state.points[0];
            let b = clip_state.points[1];
            let c = clip_state.points[2];
            let normal = (b - a).cross(c - a).normalize_or_zero();
            if normal.length_squared() > 0.5 {
                let distance = normal.dot(a);
                Some(BrushPlane { normal, distance })
            } else {
                None
            }
        }
        _ => None,
    };

    let Ok(brush_ref) = brushes.get(brush_entity) else {
        return;
    };

    // Draw clip points and preview
    for (i, point) in clip_state.points.iter().enumerate() {
        let world_pos = brush_global.transform_point(*point);
        let color = default_style::CLIP_POINT;
        gizmos.sphere(Isometry3d::from_translation(world_pos), 0.06, color);
        // Draw connecting lines between points
        if i > 0 {
            let prev_world = brush_global.transform_point(clip_state.points[i - 1]);
            gizmos.line(prev_world, world_pos, color);
        }
    }

    // Draw clipped geometry preview
    if let Some(ref plane) = clip_state.preview_plane {
        let (_, brush_rot, brush_trans) = brush_global.to_scale_rotation_translation();

        let world_faces = brush_planes_to_world(&brush_ref.faces, brush_rot, brush_trans);

        // Transform clip plane to world space (same formula as brush_planes_to_world)
        let world_clip_normal = (brush_rot * plane.normal).normalize();
        let world_clip_distance = plane.distance + world_clip_normal.dot(brush_trans);

        // Front half faces (brush + clip plane)
        let front_clip = BrushFaceData {
            plane: BrushPlane {
                normal: world_clip_normal,
                distance: world_clip_distance,
            },
            uv_scale: Vec2::ONE,
            ..default()
        };
        let mut front_faces = world_faces.clone();
        front_faces.push(front_clip);

        // Back half faces (brush + flipped clip plane)
        let back_clip = BrushFaceData {
            plane: BrushPlane {
                normal: -world_clip_normal,
                distance: -world_clip_distance,
            },
            uv_scale: Vec2::ONE,
            ..default()
        };
        let mut back_faces = world_faces;
        back_faces.push(back_clip);

        let (front_color, back_color) = match clip_state.mode {
            ClipMode::KeepFront => (default_style::CLIP_KEEP, default_style::CLIP_DISCARD),
            ClipMode::KeepBack => (default_style::CLIP_DISCARD, default_style::CLIP_KEEP),
            ClipMode::Split => (default_style::CLIP_KEEP, default_style::CLIP_SPLIT_BACK),
        };

        // Draw front half wireframe
        let (verts, polys) = compute_brush_geometry(&front_faces);
        if verts.len() >= 4 {
            for polygon in &polys {
                for i in 0..polygon.len() {
                    let a = verts[polygon[i]];
                    let b = verts[polygon[(i + 1) % polygon.len()]];
                    gizmos.line(a, b, front_color);
                }
            }
        }

        // Draw back half wireframe
        let (verts, polys) = compute_brush_geometry(&back_faces);
        if verts.len() >= 4 {
            for polygon in &polys {
                for i in 0..polygon.len() {
                    let a = verts[polygon[i]];
                    let b = verts[polygon[(i + 1) % polygon.len()]];
                    gizmos.line(a, b, back_color);
                }
            }
        }
    }
}

/// Pick the closest face under the cursor on a given brush entity.
fn pick_face_under_cursor(
    viewport_cursor: Vec2,
    brush_entity: Entity,
    camera: &Camera,
    cam_tf: &GlobalTransform,
    cache: &BrushMeshCache,
    face_entities: &Query<(Entity, &super::BrushFaceEntity, &GlobalTransform)>,
) -> Option<usize> {
    let mut best_face = None;
    let mut best_depth = f32::MAX;

    for (_, face_ent, face_global) in face_entities {
        if face_ent.brush_entity != brush_entity {
            continue;
        }
        let face_idx = face_ent.face_index;
        let polygon = &cache.face_polygons[face_idx];
        if polygon.len() < 3 {
            continue;
        }
        let screen_verts: Vec<Vec2> = polygon
            .iter()
            .filter_map(|&vi| {
                let world = face_global.transform_point(cache.vertices[vi]);
                camera.world_to_viewport(cam_tf, world).ok()
            })
            .collect();
        if screen_verts.len() < 3 {
            continue;
        }
        if point_in_polygon_2d(viewport_cursor, &screen_verts) {
            let centroid: Vec3 =
                polygon.iter().map(|&vi| cache.vertices[vi]).sum::<Vec3>() / polygon.len() as f32;
            let world_centroid = face_global.transform_point(centroid);
            let depth = (cam_tf.translation() - world_centroid).length_squared();
            if depth < best_depth {
                best_depth = depth;
                best_face = Some(face_idx);
            }
        }
    }
    best_face
}

/// Updates the hover resource each frame to track which face the cursor is over.
pub(super) fn brush_face_hover(
    edit_mode: Res<EditMode>,
    keyboard: Res<ButtonInput<KeyCode>>,
    vp: crate::viewport::ViewportCursor,
    face_entities: Query<(Entity, &super::BrushFaceEntity, &GlobalTransform)>,
    brush_selection: Res<BrushSelection>,
    brush_caches: Query<&BrushMeshCache>,
    selection: Res<Selection>,
    drag_state: Res<BrushDragState>,
    mut hover: ResMut<super::BrushFaceHover>,
    brushes: Query<(), With<Brush>>,
) {
    let in_face_edit = matches!(*edit_mode, EditMode::BrushEdit(BrushEditMode::Face));
    let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let alt = keyboard.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);

    // Clear hover during active drag
    if drag_state.active {
        hover.entity = None;
        hover.face_index = None;
        return;
    }

    // Determine if we should show hover
    let should_hover = in_face_edit || (*edit_mode == EditMode::Object && (shift || alt));

    if !should_hover {
        hover.entity = None;
        hover.face_index = None;
        return;
    }

    let intent = if alt {
        super::HoverIntent::Extend
    } else {
        super::HoverIntent::PushPull
    };

    let Ok(window) = vp.windows.single() else {
        hover.entity = None;
        hover.face_index = None;
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        hover.entity = None;
        hover.face_index = None;
        return;
    };
    let Some(camera_entity) = vp.camera_entity() else {
        hover.entity = None;
        hover.face_index = None;
        return;
    };
    let Some(viewport_entity) = vp.viewport_entity() else {
        hover.entity = None;
        hover.face_index = None;
        return;
    };
    let Some((camera, cam_tf)) = vp.camera_for(camera_entity) else {
        hover.entity = None;
        hover.face_index = None;
        return;
    };
    let Some(viewport_cursor) = vp.viewport_cursor_for(camera, viewport_entity, cursor_pos) else {
        hover.entity = None;
        hover.face_index = None;
        return;
    };

    let brush_entity = if in_face_edit {
        brush_selection.entity
    } else {
        selection.primary().filter(|&e| brushes.contains(e))
    };

    let Some(brush_entity) = brush_entity else {
        hover.entity = None;
        hover.face_index = None;
        return;
    };

    let Ok(cache) = brush_caches.get(brush_entity) else {
        hover.entity = None;
        hover.face_index = None;
        return;
    };

    if let Some(face_idx) = pick_face_under_cursor(
        viewport_cursor,
        brush_entity,
        camera,
        cam_tf,
        cache,
        &face_entities,
    ) {
        hover.entity = Some(brush_entity);
        hover.face_index = Some(face_idx);
        hover.intent = intent;
    } else {
        hover.entity = None;
        hover.face_index = None;
    }
}
