use std::collections::HashSet;

use bevy::prelude::*;

use jackdaw_jsn::BrushGroup;

use crate::brush::{Brush, BrushEditMode, BrushMeshCache, EditMode};
use crate::draw_brush::{CutPreviewFace, CutPreviewHidden, CutResultPreviewMesh};
use crate::selection::Selected;
use crate::snapping::SnapSettings;
use crate::viewport_overlays::OverlaySettings;
use crate::viewport_select::GroupEditState;
use crate::{JackdawDrawSystems, default_style};

/// Gizmo group for face grid lines. Rendered slightly in front of geometry.
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct FaceGridGizmoGroup;

/// Gizmo group for unselected brush edge wireframes. Rendered in front of both geometry
/// and face grid lines to ensure edges are always clearly visible.
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct BrushWireframeUnselectedGizmoGroup;

/// Gizmo group for selecetd brush edge wireframes. Rendered in front of both geometry
/// and face grid lines to ensure edges are always clearly visible.
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct BrushWireframeSelectedGizmoGroup;

/// Gizmo group for unselected brush outlines. Like wireframe, but not rendered in front of geometry.
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct BrushOutlineUnselectedGizmoGroup;

/// Gizmo group for selected brush outlines. Like wireframe, but not rendered in front of geometry.
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct BrushOutlineSelectedGizmoGroup;

pub struct FaceGridPlugin;

impl Plugin for FaceGridPlugin {
    fn build(&self, app: &mut App) {
        app.init_gizmo_group::<FaceGridGizmoGroup>()
            .init_gizmo_group::<BrushWireframeUnselectedGizmoGroup>()
            .init_gizmo_group::<BrushWireframeSelectedGizmoGroup>()
            .init_gizmo_group::<BrushOutlineUnselectedGizmoGroup>()
            .init_gizmo_group::<BrushOutlineSelectedGizmoGroup>()
            .add_systems(Startup, configure_face_grid_gizmos)
            .add_systems(
                PostUpdate,
                (
                    draw_brush_wireframe,
                    draw_face_grids,
                    draw_cut_preview_edges,
                    draw_cut_preview_grids,
                )
                    .in_set(JackdawDrawSystems),
            );
    }
}

fn configure_face_grid_gizmos(mut config_store: ResMut<GizmoConfigStore>) {
    // configured in draw order

    let (config, _) = config_store.config_mut::<BrushWireframeSelectedGizmoGroup>();
    config.depth_bias = -1.0;
    config.line = default_style::WIREFRAME_LINE_SELECTED;

    let (config, _) = config_store.config_mut::<BrushWireframeUnselectedGizmoGroup>();
    config.depth_bias = -0.9999;
    config.line = default_style::WIREFRAME_LINE_UNSELECTED;

    let (config, _) = config_store.config_mut::<BrushOutlineSelectedGizmoGroup>();
    config.depth_bias = -0.0012;
    config.line = default_style::OUTLINE_LINE_SELECTED;

    let (config, _) = config_store.config_mut::<BrushOutlineUnselectedGizmoGroup>();
    config.depth_bias = -0.0011;
    config.line = default_style::OUTLINE_LINE_UNSELECTED;

    let (config, _) = config_store.config_mut::<FaceGridGizmoGroup>();
    config.depth_bias = -0.001;
    config.line = default_style::FACE_GRID_LINE;
}

/// Draw brush wireframe and outlines. Will do special treatment for the currently selected brushes.
fn draw_brush_wireframe(
    mut wireframe_unselected: Gizmos<BrushWireframeUnselectedGizmoGroup>,
    mut wireframe_selected: Gizmos<BrushWireframeSelectedGizmoGroup>,
    mut outline_unselected: Gizmos<BrushOutlineUnselectedGizmoGroup>,
    mut outline_selected: Gizmos<BrushOutlineSelectedGizmoGroup>,
    settings: Res<OverlaySettings>,
    edit_mode: Res<EditMode>,
    brushes: Query<
        (
            Entity,
            &Brush,
            &BrushMeshCache,
            &GlobalTransform,
            Has<Selected>,
            &InheritedVisibility,
        ),
        Without<CutPreviewHidden>,
    >,
    parents: Query<&ChildOf>,
    selected_query: Query<(), With<Selected>>,
    group_edit: Res<GroupEditState>,
    brush_groups: Query<(), With<BrushGroup>>,
) {
    let in_clip_mode = matches!(*edit_mode, EditMode::BrushEdit(BrushEditMode::Clip));

    for (entity, brush, cache, global_tf, is_brush_selected, inherited_vis) in &brushes {
        if !inherited_vis.get() {
            continue;
        }

        // In Clip mode, hide wireframe on selected default-material brushes so
        // the clip plane and cut preview are clearly visible.
        if in_clip_mode {
            let all_default = brush.faces.iter().all(|f| f.material == Handle::default());
            if all_default {
                continue;
            }
        }

        let is_parent_selected = parents
            .get(entity)
            .is_ok_and(|child_of| selected_query.contains(child_of.0));
        let in_active_group = group_edit
            .active_group
            .is_some_and(|group| parents.get(entity).is_ok_and(|c| c.0 == group));
        let is_selected = is_brush_selected || in_active_group || is_parent_selected;

        // we use the same color for wireframe and outline, outlines are just thicker and not drawn in front of geo
        let color: Color = if is_brush_selected {
            if in_clip_mode {
                default_style::WIREFRAME_OUTLINE_SELECTED_CLIP
            } else {
                default_style::WIREFRAME_OUTLINE_SELECTED
            }
        } else if in_active_group {
            default_style::WIREFRAME_OUTLINE_GROUP_EDIT
        } else if is_parent_selected {
            if in_clip_mode {
                default_style::WIREFRAME_OUTLINE_SELECTED_CLIP
            } else {
                default_style::WIREFRAME_OUTLINE_SELECTED
            }
        } else {
            default_style::WIREFRAME_OUTLINE_UNSELECTED
        };

        // Determine if we should hide cap-only edges (internal cut boundaries)
        let in_brush_group = parents
            .get(entity)
            .is_ok_and(|child_of| brush_groups.contains(child_of.0));
        let hide_cap_edges = in_brush_group && !is_brush_selected;

        // Pre-collect non-cap edges (edges on at least one original face)
        let non_cap_edges: Option<HashSet<(usize, usize)>> = if hide_cap_edges {
            let mut set = HashSet::new();
            for (fi, polygon) in cache.face_polygons.iter().enumerate() {
                if !brush.faces.get(fi).is_some_and(|f| f.is_cap) {
                    for i in 0..polygon.len() {
                        let a = polygon[i];
                        let b = polygon[(i + 1) % polygon.len()];
                        set.insert((a.min(b), a.max(b)));
                    }
                }
            }
            Some(set)
        } else {
            None
        };

        // Draw edges
        let mut drawn_edges = HashSet::new();
        for polygon in &cache.face_polygons {
            for i in 0..polygon.len() {
                let a = polygon[i];
                let b = polygon[(i + 1) % polygon.len()];
                let edge = (a.min(b), a.max(b));
                if drawn_edges.insert(edge) {
                    if let Some(ref nce) = non_cap_edges
                        && !nce.contains(&edge)
                    {
                        continue;
                    }
                    let wa = global_tf.transform_point(cache.vertices[a]);
                    let wb = global_tf.transform_point(cache.vertices[b]);
                    if is_selected {
                        // selected brushes *always* draw their outlines
                        outline_selected.line(wa, wb, color);
                        if settings.show_brush_wireframe {
                            wireframe_selected.line(wa, wb, color);
                        }
                    } else {
                        if settings.show_brush_outline {
                            outline_unselected.line(wa, wb, color);
                        }
                        if settings.show_brush_wireframe {
                            wireframe_unselected.line(wa, wb, color);
                        }
                    }
                }
            }
        }
    }
}

/// Draw grid lines on each face of all brushes (brighter on selected).
fn draw_face_grids(
    mut gizmos: Gizmos<FaceGridGizmoGroup>,
    settings: Res<OverlaySettings>,
    snap: Res<SnapSettings>,
    brushes: Query<
        (
            Entity,
            &Brush,
            &BrushMeshCache,
            &GlobalTransform,
            Has<Selected>,
            &InheritedVisibility,
        ),
        Without<CutPreviewHidden>,
    >,
    parents: Query<&ChildOf>,
    selected_query: Query<(), With<Selected>>,
    group_edit: Res<GroupEditState>,
) {
    if !settings.show_face_grid {
        return;
    }

    let grid_size = snap.grid_size();

    for (entity, brush, cache, global_tf, is_selected, inherited_vis) in &brushes {
        if !inherited_vis.get() {
            continue;
        }
        let in_active_group = group_edit
            .active_group
            .is_some_and(|group| parents.get(entity).is_ok_and(|c| c.0 == group));
        let parent_selected = !in_active_group
            && parents
                .get(entity)
                .is_ok_and(|child_of| selected_query.contains(child_of.0));
        let effectively_selected = is_selected || parent_selected;
        let color = if effectively_selected {
            default_style::FACE_GRID_SELECTED
        } else {
            default_style::FACE_GRID_UNSELECTED
        };
        for (face_idx, face_data) in brush.faces.iter().enumerate() {
            if face_data.material == Handle::default() {
                continue; // Skip default-material faces, checkerboard provides its own structure.
            }
            let Some(polygon_indices) = cache.face_polygons.get(face_idx) else {
                continue;
            };
            if polygon_indices.len() < 3 {
                continue;
            }

            // Get world-space face vertices
            let world_verts: Vec<Vec3> = polygon_indices
                .iter()
                .map(|&i| global_tf.transform_point(cache.vertices[i]))
                .collect();

            // Compute world normal
            let world_normal = global_tf
                .compute_transform()
                .rotation
                .mul_vec3(face_data.plane.normal)
                .normalize();

            // Select 2D coordinate axes (TrenchBroom style):
            // Pick the two axes perpendicular to the dominant normal component
            let abs_n = world_normal.abs();
            let (axis_u, axis_v, plane_axis) = if abs_n.x >= abs_n.y && abs_n.x >= abs_n.z {
                // Dominant X: use Y, Z
                (1usize, 2usize, 0usize)
            } else if abs_n.y >= abs_n.x && abs_n.y >= abs_n.z {
                // Dominant Y: use X, Z
                (0, 2, 1)
            } else {
                // Dominant Z: use X, Y
                (0, 1, 2)
            };

            // Project face vertices to 2D using selected axes
            let polygon_2d: Vec<Vec2> = world_verts
                .iter()
                .map(|v| {
                    let arr = v.to_array();
                    Vec2::new(arr[axis_u], arr[axis_v])
                })
                .collect();

            // Find bounding rect of 2D polygon
            let mut min_2d = Vec2::splat(f32::MAX);
            let mut max_2d = Vec2::splat(f32::MIN);
            for &p in &polygon_2d {
                min_2d = min_2d.min(p);
                max_2d = max_2d.max(p);
            }

            // Snap bounds to grid
            let grid_min_u = (min_2d.x / grid_size).floor() * grid_size;
            let grid_max_u = (max_2d.x / grid_size).ceil() * grid_size;
            let grid_min_v = (min_2d.y / grid_size).floor() * grid_size;
            let grid_max_v = (max_2d.y / grid_size).ceil() * grid_size;

            // Plane equation for reconstructing 3rd axis:
            // normal . point = d (world-space)
            let plane_d = world_normal.dot(world_verts[0]);
            let normal_arr = world_normal.to_array();

            // Draw lines at constant U values (vertical lines in 2D)
            let mut u = grid_min_u;
            while u <= grid_max_u + grid_size * 0.01 {
                if let Some((p0_2d, p1_2d)) = clip_line_to_convex_polygon(&polygon_2d, true, u) {
                    let a = reconstruct_3d(p0_2d, axis_u, axis_v, plane_axis, plane_d, normal_arr);
                    let b = reconstruct_3d(p1_2d, axis_u, axis_v, plane_axis, plane_d, normal_arr);
                    if let (Some(a), Some(b)) = (a, b) {
                        gizmos.line(a, b, color);
                    }
                }
                u += grid_size;
            }

            // Draw lines at constant V values (horizontal lines in 2D)
            let mut v = grid_min_v;
            while v <= grid_max_v + grid_size * 0.01 {
                if let Some((p0_2d, p1_2d)) = clip_line_to_convex_polygon(&polygon_2d, false, v) {
                    let a = reconstruct_3d(p0_2d, axis_u, axis_v, plane_axis, plane_d, normal_arr);
                    let b = reconstruct_3d(p1_2d, axis_u, axis_v, plane_axis, plane_d, normal_arr);
                    if let (Some(a), Some(b)) = (a, b) {
                        gizmos.line(a, b, color);
                    }
                }
                v += grid_size;
            }
        }
    }
}

/// Draw wireframe edges on cut-preview fragment faces.
fn draw_cut_preview_edges(
    mut gizmos: Gizmos<BrushWireframeUnselectedGizmoGroup>,
    settings: Res<OverlaySettings>,
    previews: Query<&CutPreviewFace, With<CutResultPreviewMesh>>,
) {
    if !settings.show_brush_wireframe {
        return;
    }

    let color: Color = default_style::WIREFRAME_OUTLINE_CUT_PREVIEW;

    for face in &previews {
        if face.is_default_material || face.is_cap {
            continue;
        }
        let verts = &face.world_vertices;
        if verts.len() < 3 {
            continue;
        }
        for i in 0..verts.len() {
            let a = verts[i];
            let b = verts[(i + 1) % verts.len()];
            gizmos.line(a, b, color);
        }
    }
}

/// Draw grid lines on cut-preview fragment faces.
fn draw_cut_preview_grids(
    mut gizmos: Gizmos<FaceGridGizmoGroup>,
    settings: Res<OverlaySettings>,
    snap: Res<SnapSettings>,
    previews: Query<&CutPreviewFace, With<CutResultPreviewMesh>>,
) {
    if !settings.show_face_grid {
        return;
    }

    let grid_size = snap.grid_size();
    let color = default_style::FACE_GRID_SELECTED;

    for face in &previews {
        if face.is_default_material || face.is_cap {
            continue;
        }
        let world_verts = &face.world_vertices;
        if world_verts.len() < 3 {
            continue;
        }
        let world_normal = face.world_normal;

        let abs_n = world_normal.abs();
        let (axis_u, axis_v, plane_axis) = if abs_n.x >= abs_n.y && abs_n.x >= abs_n.z {
            (1usize, 2usize, 0usize)
        } else if abs_n.y >= abs_n.x && abs_n.y >= abs_n.z {
            (0, 2, 1)
        } else {
            (0, 1, 2)
        };

        let polygon_2d: Vec<Vec2> = world_verts
            .iter()
            .map(|v| {
                let arr = v.to_array();
                Vec2::new(arr[axis_u], arr[axis_v])
            })
            .collect();

        let mut min_2d = Vec2::splat(f32::MAX);
        let mut max_2d = Vec2::splat(f32::MIN);
        for &p in &polygon_2d {
            min_2d = min_2d.min(p);
            max_2d = max_2d.max(p);
        }

        let grid_min_u = (min_2d.x / grid_size).floor() * grid_size;
        let grid_max_u = (max_2d.x / grid_size).ceil() * grid_size;
        let grid_min_v = (min_2d.y / grid_size).floor() * grid_size;
        let grid_max_v = (max_2d.y / grid_size).ceil() * grid_size;

        let plane_d = world_normal.dot(world_verts[0]);
        let normal_arr = world_normal.to_array();

        let mut u = grid_min_u;
        while u <= grid_max_u + grid_size * 0.01 {
            if let Some((p0_2d, p1_2d)) = clip_line_to_convex_polygon(&polygon_2d, true, u) {
                let a = reconstruct_3d(p0_2d, axis_u, axis_v, plane_axis, plane_d, normal_arr);
                let b = reconstruct_3d(p1_2d, axis_u, axis_v, plane_axis, plane_d, normal_arr);
                if let (Some(a), Some(b)) = (a, b) {
                    let offset = world_normal * 0.002;
                    gizmos.line(a + offset, b + offset, color);
                }
            }
            u += grid_size;
        }

        let mut v = grid_min_v;
        while v <= grid_max_v + grid_size * 0.01 {
            if let Some((p0_2d, p1_2d)) = clip_line_to_convex_polygon(&polygon_2d, false, v) {
                let a = reconstruct_3d(p0_2d, axis_u, axis_v, plane_axis, plane_d, normal_arr);
                let b = reconstruct_3d(p1_2d, axis_u, axis_v, plane_axis, plane_d, normal_arr);
                if let (Some(a), Some(b)) = (a, b) {
                    let offset = world_normal * 0.002;
                    gizmos.line(a + offset, b + offset, color);
                }
            }
            v += grid_size;
        }
    }
}

/// Reconstruct a 3D point from 2D coordinates + plane equation.
/// Returns None if the plane normal component along `plane_axis` is ~zero.
fn reconstruct_3d(
    point_2d: Vec2,
    axis_u: usize,
    axis_v: usize,
    plane_axis: usize,
    plane_d: f32,
    normal: [f32; 3],
) -> Option<Vec3> {
    if normal[plane_axis].abs() < 1e-6 {
        return None;
    }
    let mut arr = [0.0f32; 3];
    arr[axis_u] = point_2d.x;
    arr[axis_v] = point_2d.y;
    // Solve: normal[plane_axis] * arr[plane_axis] = plane_d - normal[axis_u]*arr[axis_u] - normal[axis_v]*arr[axis_v]
    arr[plane_axis] = (plane_d - normal[axis_u] * arr[axis_u] - normal[axis_v] * arr[axis_v])
        / normal[plane_axis];
    Some(Vec3::from_array(arr))
}

/// Clip a horizontal or vertical line to a convex polygon.
///
/// If `is_u_constant` is true, clips the line `u = val` (finds min/max v intersections).
/// If false, clips the line `v = val` (finds min/max u intersections).
///
/// Returns the two intersection endpoints, or None if the line doesn't cross the polygon.
fn clip_line_to_convex_polygon(
    polygon: &[Vec2],
    is_u_constant: bool,
    val: f32,
) -> Option<(Vec2, Vec2)> {
    let n = polygon.len();
    let mut intersections = Vec::new();

    for i in 0..n {
        let a = polygon[i];
        let b = polygon[(i + 1) % n];

        let (a_coord, b_coord, a_other, b_other) = if is_u_constant {
            (a.x, b.x, a.y, b.y)
        } else {
            (a.y, b.y, a.x, b.x)
        };

        // Check if edge crosses the line
        let min_c = a_coord.min(b_coord);
        let max_c = a_coord.max(b_coord);
        if val < min_c - 1e-6 || val > max_c + 1e-6 {
            continue;
        }

        let denom = b_coord - a_coord;
        let other = if denom.abs() < 1e-6 {
            // Edge is parallel to the line, use both endpoints.
            intersections.push(a_other);
            b_other
        } else {
            let t = (val - a_coord) / denom;
            a_other + t * (b_other - a_other)
        };
        intersections.push(other);
    }

    if intersections.len() < 2 {
        return None;
    }

    let min_other = intersections.iter().copied().fold(f32::MAX, f32::min);
    let max_other = intersections.iter().copied().fold(f32::MIN, f32::max);

    if (max_other - min_other).abs() < 1e-6 {
        return None;
    }

    let (p0, p1) = if is_u_constant {
        (Vec2::new(val, min_other), Vec2::new(val, max_other))
    } else {
        (Vec2::new(min_other, val), Vec2::new(max_other, val))
    };

    Some((p0, p1))
}
