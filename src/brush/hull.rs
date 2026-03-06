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

/// Rebuild a `Brush` from a new set of vertices using convex hull.
/// Attempts to match new faces to old faces for material/UV preservation.
pub(super) fn rebuild_brush_from_vertices(
    old_brush: &Brush,
    _old_vertices: &[Vec3],
    old_face_polygons: &[Vec<usize>],
    new_vertices: &[Vec3],
) -> Option<Brush> {
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

        let old_face = &old_brush.faces[best_old];
        faces.push(BrushFaceData {
            plane: BrushPlane {
                normal: hull_face.normal,
                distance: hull_face.distance,
            },
            material_index: old_face.material_index,
            texture_path: old_face.texture_path.clone(),
            material_name: old_face.material_name.clone(),
            uv_offset: old_face.uv_offset,
            uv_scale: old_face.uv_scale,
            uv_rotation: old_face.uv_rotation,
        });
    }

    Some(Brush { faces })
}
