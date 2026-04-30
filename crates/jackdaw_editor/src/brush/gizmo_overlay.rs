use bevy::prelude::*;

use super::interaction::{
    BrushDragState, EdgeDragState, FaceExtrudeMode, VertexDragConstraint, VertexDragState,
};
use super::{BrushEditMode, BrushMeshCache, BrushSelection, EditMode};
use crate::default_style;
use crate::face_grid::BrushOutlineSelectedGizmoGroup;
use jackdaw_jsn::Brush;

pub(super) fn draw_brush_edit_gizmos(
    edit_mode: Res<EditMode>,
    brush_selection: Res<BrushSelection>,
    brush_caches: Query<&BrushMeshCache>,
    brush_transforms: Query<&GlobalTransform>,
    brushes: Query<&Brush>,
    vertex_drag: Res<VertexDragState>,
    edge_drag: Res<EdgeDragState>,
    face_drag: Res<BrushDragState>,
    hover: Res<super::BrushFaceHover>,
    mut gizmos: Gizmos<BrushOutlineSelectedGizmoGroup>,
) {
    // Draw hover face outline (works in both Object and Edit modes)
    if let (Some(hover_entity), Some(hover_face)) = (hover.entity, hover.face_index)
        && let Ok(cache) = brush_caches.get(hover_entity)
        && let Ok(brush_global) = brush_transforms.get(hover_entity)
    {
        let polygon = &cache.face_polygons[hover_face];
        if polygon.len() >= 3 {
            // Skip if face is already selected (avoid double highlight)
            let is_selected = brush_selection.faces.contains(&hover_face)
                && brush_selection.entity == Some(hover_entity);
            if !is_selected {
                let color = default_style::EDIT_AVAILABLE_COLOR;
                for i in 0..polygon.len() {
                    let a = brush_global.transform_point(cache.vertices[polygon[i]]);
                    let b = brush_global
                        .transform_point(cache.vertices[polygon[(i + 1) % polygon.len()]]);
                    gizmos.line(a, b, color);
                }
            }
        }
    }

    let EditMode::BrushEdit(mode) = *edit_mode else {
        return;
    };

    let Some(brush_entity) = brush_selection.entity else {
        return;
    };
    let Ok(cache) = brush_caches.get(brush_entity) else {
        return;
    };
    let Ok(brush_global) = brush_transforms.get(brush_entity) else {
        return;
    };

    // In Clip mode, hide edge/vertex overlays on default-material brushes so the
    // clip plane and cut preview are clearly visible.
    let all_faces_default = brushes
        .get(brush_entity)
        .is_ok_and(|b| b.faces.iter().all(|f| f.material == Handle::default()));
    let skip_wireframe = mode == BrushEditMode::Clip && all_faces_default;

    if !skip_wireframe {
        // Collect unique edges and track selected state
        let mut drawn_edges: Vec<(usize, usize, bool)> = Vec::new();
        for polygon in &cache.face_polygons {
            if polygon.len() < 2 {
                continue;
            }
            for i in 0..polygon.len() {
                let a = polygon[i];
                let b = polygon[(i + 1) % polygon.len()];
                let edge = (a.min(b), a.max(b));
                if !drawn_edges
                    .iter()
                    .any(|(ea, eb, _)| *ea == edge.0 && *eb == edge.1)
                {
                    let selected = brush_selection.edges.contains(&edge);
                    drawn_edges.push((edge.0, edge.1, selected));
                }
            }
        }

        // Draw all edges
        for &(a, b, selected) in &drawn_edges {
            let wa = brush_global.transform_point(cache.vertices[a]);
            let wb = brush_global.transform_point(cache.vertices[b]);
            let color = if selected {
                default_style::EDIT_SELECTED_COLOR
            } else {
                default_style::EDIT_AVAILABLE_COLOR
            };
            if selected || mode == BrushEditMode::Edge {
                gizmos.line(wa, wb, color);
            }
        }

        // Draw vertices as small spheres
        for (vi, v) in cache.vertices.iter().enumerate() {
            let world_pos = brush_global.transform_point(*v);
            let selected = brush_selection.vertices.contains(&vi);
            let color = if selected {
                default_style::EDIT_SELECTED_COLOR
            } else {
                default_style::EDIT_AVAILABLE_COLOR
            };
            if selected || mode == BrushEditMode::Vertex {
                gizmos.sphere(
                    Isometry3d::from_translation(world_pos),
                    default_style::EDIT_VERTEX_RADIUS,
                    color,
                );
            }
        }
    }

    // Highlight selected faces
    if mode == BrushEditMode::Face {
        for &face_idx in &brush_selection.faces {
            let polygon = &cache.face_polygons[face_idx];
            if polygon.len() < 3 {
                continue;
            }
            // Draw face outline in bright color
            for i in 0..polygon.len() {
                let a = brush_global.transform_point(cache.vertices[polygon[i]]);
                let b =
                    brush_global.transform_point(cache.vertices[polygon[(i + 1) % polygon.len()]]);
                gizmos.line(a, b, default_style::EDIT_SELECTED_COLOR);
            }
        }
    }

    // Draw extend mode wireframe preview
    if face_drag.active && face_drag.extrude_mode == FaceExtrudeMode::Extend {
        let polygon = &face_drag.extend_face_polygon;
        let depth = face_drag.extend_depth;
        let normal = face_drag.extend_face_normal;
        let offset = normal * depth;
        let preview_color = default_style::FACE_EXTRUDE_PREVIEW;

        if polygon.len() >= 3 {
            // Base polygon edges
            for i in 0..polygon.len() {
                let a = polygon[i];
                let b = polygon[(i + 1) % polygon.len()];
                gizmos.line(a, b, preview_color);
            }
            // Top polygon edges (base + offset)
            for i in 0..polygon.len() {
                let a = polygon[i] + offset;
                let b = polygon[(i + 1) % polygon.len()] + offset;
                gizmos.line(a, b, preview_color);
            }
            // Connecting edges
            for &v in polygon {
                gizmos.line(v, v + offset, preview_color);
            }
        }
    }

    // Draw drag constraint line (vertex or edge drag)
    let active_constraint = if vertex_drag.active {
        Some(vertex_drag.constraint)
    } else if edge_drag.active {
        Some(edge_drag.constraint)
    } else {
        None
    };
    if let Some(constraint) = active_constraint
        && constraint != VertexDragConstraint::Free
    {
        let (axis_dir, color) = match constraint {
            VertexDragConstraint::AxisX => (Vec3::X, default_style::AXIS_X),
            VertexDragConstraint::AxisY => (Vec3::Y, default_style::AXIS_Y),
            VertexDragConstraint::AxisZ => (Vec3::Z, default_style::AXIS_Z),
            VertexDragConstraint::Free => unreachable!(),
        };
        let (_, brush_rot, _) = brush_global.to_scale_rotation_translation();
        let world_axis = brush_rot * axis_dir;
        let center = brush_global.translation();
        gizmos.line(
            center - world_axis * 50.0,
            center + world_axis * 50.0,
            color,
        );
    }
}
