use std::collections::HashSet;

use avian3d::parry::math::Point as ParryPoint;
use avian3d::parry::transformation::convex_hull;
use bevy::prelude::*;

use jackdaw_geometry::{EPSILON, sort_face_vertices_by_winding};
use jackdaw_jsn::{Brush, BrushFaceData, BrushPlane};

fn vec3_to_point(v: Vec3) -> ParryPoint<f32> {
    ParryPoint::new(v.x, v.y, v.z)
}

fn point_to_vec3(p: &ParryPoint<f32>) -> Vec3 {
    Vec3::new(p.x, p.y, p.z)
}

pub struct HullFace {
    pub normal: Vec3,
    pub distance: f32,
    pub vertex_indices: Vec<usize>,
}

/// Merge the triangles from a convex hull into coplanar polygon faces.
pub(crate) fn merge_hull_triangles(vertices: &[Vec3], triangles: &[[u32; 3]]) -> Vec<HullFace> {
    // Compute normal + distance for each triangle, group coplanar ones.
    let mut face_groups: Vec<(Vec3, f32, HashSet<usize>)> = Vec::new();

    for tri in triangles {
        let a = vertices[tri[0] as usize];
        let b = vertices[tri[1] as usize];
        let c = vertices[tri[2] as usize];
        let normal = (b - a).cross(c - a).normalize_or_zero();
        if normal.length_squared() < 0.5 {
            continue; // degenerate triangle
        }
        let distance = normal.dot(a);

        // Find existing group with matching plane
        let mut found = false;
        for (gn, gd, gverts) in &mut face_groups {
            if gn.dot(normal) > 1.0 - EPSILON && (distance - *gd).abs() < EPSILON {
                gverts.insert(tri[0] as usize);
                gverts.insert(tri[1] as usize);
                gverts.insert(tri[2] as usize);
                found = true;
                break;
            }
        }
        if !found {
            let mut verts = HashSet::new();
            verts.insert(tri[0] as usize);
            verts.insert(tri[1] as usize);
            verts.insert(tri[2] as usize);
            face_groups.push((normal, distance, verts));
        }
    }

    face_groups
        .into_iter()
        .map(|(normal, distance, vert_set)| {
            let mut vertex_indices: Vec<usize> = vert_set.into_iter().collect();
            sort_face_vertices_by_winding(vertices, &mut vertex_indices, normal);
            HullFace {
                normal,
                distance,
                vertex_indices,
            }
        })
        .collect()
}

/// Compute span and centroid projection of a face's vertices along given UV axes.
fn compute_face_uv_metrics(
    vertices: &[Vec3],
    face_vert_indices: &[usize],
    u_axis: Vec3,
    v_axis: Vec3,
) -> (Vec2, Vec2) {
    let (mut min_u, mut max_u) = (f32::MAX, f32::MIN);
    let (mut min_v, mut max_v) = (f32::MAX, f32::MIN);
    let mut sum_u = 0.0_f32;
    let mut sum_v = 0.0_f32;
    for &vi in face_vert_indices {
        let pos = vertices[vi];
        let u = pos.dot(u_axis);
        let v = pos.dot(v_axis);
        min_u = min_u.min(u);
        max_u = max_u.max(u);
        min_v = min_v.min(v);
        max_v = max_v.max(v);
        sum_u += u;
        sum_v += v;
    }
    let n = face_vert_indices.len() as f32;
    (
        Vec2::new((max_u - min_u).max(0.001), (max_v - min_v).max(0.001)),
        Vec2::new(sum_u / n, sum_v / n),
    )
}

/// Rebuild a `Brush` from a new set of vertices using convex hull.
/// Attempts to match new faces to old faces for material/UV preservation.
/// Implements texture lock: preserves UV axes from old faces and adjusts
/// scale/offset to maintain consistent texel density.
pub(crate) fn rebuild_brush_from_vertices(
    old_brush: &Brush,
    old_vertices: &[Vec3],
    old_face_polygons: &[Vec<usize>],
    new_vertices: &[Vec3],
) -> Option<(Brush, Vec<usize>)> {
    if new_vertices.len() < 4 {
        return None;
    }

    let points: Vec<ParryPoint<f32>> = new_vertices.iter().map(|v| vec3_to_point(*v)).collect();
    let (hull_verts, hull_tris) = convex_hull(&points);

    if hull_verts.len() < 4 || hull_tris.is_empty() {
        return None;
    }

    let hull_positions: Vec<Vec3> = hull_verts.iter().map(point_to_vec3).collect();
    let hull_faces = merge_hull_triangles(&hull_positions, &hull_tris);

    if hull_faces.len() < 4 {
        return None;
    }

    // Map hull vertex indices → input vertex indices (closest position match)
    let hull_to_input: Vec<usize> = hull_positions
        .iter()
        .map(|hp| {
            new_vertices
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    (**a - *hp)
                        .length_squared()
                        .partial_cmp(&(**b - *hp).length_squared())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(i, _)| i)
                .unwrap_or(0)
        })
        .collect();

    let mut faces = Vec::with_capacity(hull_faces.len());
    let mut best_old_per_new = Vec::with_capacity(hull_faces.len());
    for hull_face in &hull_faces {
        // Remap vertex indices from hull-local to input-local
        let input_verts: HashSet<usize> = hull_face
            .vertex_indices
            .iter()
            .map(|&hi| hull_to_input[hi])
            .collect();

        // Match to best old face by vertex overlap + normal similarity
        let mut best_old = 0usize;
        let mut best_score = -1.0_f32;
        for (old_idx, old_polygon) in old_face_polygons.iter().enumerate() {
            let old_set: HashSet<usize> = old_polygon.iter().copied().collect();
            let overlap = input_verts.intersection(&old_set).count() as f32;
            let normal_sim = hull_face.normal.dot(old_brush.faces[old_idx].plane.normal);
            let score = overlap + normal_sim * 0.1;
            if score > best_score {
                best_score = score;
                best_old = old_idx;
            }
        }
        best_old_per_new.push(best_old);

        let old_face = &old_brush.faces[best_old];

        // Resolve UV axes from old face (texture lock: preserve axes)
        let (u_axis, v_axis) =
            if old_face.uv_u_axis != Vec3::ZERO && old_face.uv_v_axis != Vec3::ZERO {
                (old_face.uv_u_axis, old_face.uv_v_axis)
            } else {
                jackdaw_geometry::compute_face_tangent_axes(old_face.plane.normal)
            };

        // Remap hull vertex indices to input indices for metric computation
        let remapped_indices: Vec<usize> = hull_face
            .vertex_indices
            .iter()
            .map(|&hi| hull_to_input[hi])
            .collect();

        // Compute UV centroids using the preserved axes
        let old_polygon = &old_face_polygons[best_old];
        let (_, old_centroid) = compute_face_uv_metrics(old_vertices, old_polygon, u_axis, v_axis);
        let (_, new_centroid) =
            compute_face_uv_metrics(new_vertices, &remapped_indices, u_axis, v_axis);

        // Preserve scale (Valve 220-style: scale = texels-per-world-unit, stays constant)
        let new_scale = old_face.uv_scale;

        // Adjust offset to anchor texture position
        let safe_scale = new_scale.max(Vec2::splat(0.001));
        let old_uv_center = old_centroid / safe_scale;
        let new_uv_center = new_centroid / safe_scale;
        let new_offset = old_face.uv_offset + (old_uv_center - new_uv_center);

        faces.push(BrushFaceData {
            plane: BrushPlane {
                normal: hull_face.normal,
                distance: hull_face.distance,
            },
            material: old_face.material.clone(),
            uv_offset: new_offset,
            uv_scale: new_scale,
            uv_rotation: old_face.uv_rotation,
            uv_u_axis: u_axis,
            uv_v_axis: v_axis,
            ..default()
        });
    }

    // Build old→new face index mapping by inverting best_old_per_new
    let mut old_to_new = vec![0usize; old_brush.faces.len()];
    for (new_idx, &old_idx) in best_old_per_new.iter().enumerate() {
        old_to_new[old_idx] = new_idx;
    }

    Some((Brush { faces }, old_to_new))
}
