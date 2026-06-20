//! Engine-agnostic convex-hull math for the jackdaw editor.
//!
//! This crate holds the pure geometry that the editor uses to turn loose point
//! sets into convex brushes: merging hull triangles into coplanar polygons,
//! rebuilding a brush's faces from a new vertex set (with material and UV
//! preservation), and computing the planar convex hull of coplanar points.
//!
//! It depends only on `glam`, `parry3d`, and the bevy-free build of
//! `jackdaw_geometry`, so nothing here pulls in a renderer or an ECS.

use std::collections::HashSet;

use glam::{Vec2, Vec3};

use jackdaw_geometry::{
    BrushFaceData, BrushPlane, BrushTopology, EPSILON, FaceMaterial, compute_brush_topology,
    compute_face_tangent_axes, sort_face_vertices_by_winding,
};

/// A coplanar polygon face recovered from a set of convex-hull triangles.
pub struct HullFace {
    pub normal: Vec3,
    pub distance: f32,
    pub vertex_indices: Vec<usize>,
}

/// Merge the triangles from a convex hull into coplanar polygon faces.
pub fn merge_hull_triangles(vertices: &[Vec3], triangles: &[[u32; 3]]) -> Vec<HullFace> {
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

/// Rebuild a brush's faces from a new set of vertices using convex hull.
///
/// Attempts to match new faces to old faces for material/UV preservation.
/// Implements texture lock: preserves UV axes from old faces and adjusts
/// scale/offset to maintain consistent texel density.
///
/// Returns the rebuilt faces, their freshly computed half-edge topology, and an
/// `old_to_new` map (indexed by old face index) so callers can carry per-face
/// selection state across the rebuild. Returns `None` when there are too few
/// input vertices or the hull degenerates below a closed solid.
pub fn rebuild_brush_from_vertices(
    old_faces: &[BrushFaceData],
    old_vertices: &[Vec3],
    old_face_polygons: &[Vec<usize>],
    new_vertices: &[Vec3],
) -> Option<(Vec<BrushFaceData>, BrushTopology, Vec<usize>)> {
    if new_vertices.len() < 4 {
        return None;
    }

    // parry 0.26 takes / returns plain `Vec3` for vertex inputs.
    let (hull_positions, hull_tris) = parry3d::transformation::convex_hull(new_vertices);

    if hull_positions.len() < 4 || hull_tris.is_empty() {
        return None;
    }

    let hull_faces = merge_hull_triangles(&hull_positions, &hull_tris);

    if hull_faces.len() < 4 {
        return None;
    }

    // Map hull vertex indices to input vertex indices (closest position match)
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
            let normal_sim = hull_face.normal.dot(old_faces[old_idx].plane.normal);
            let score = overlap + normal_sim * 0.1;
            if score > best_score {
                best_score = score;
                best_old = old_idx;
            }
        }
        best_old_per_new.push(best_old);

        let old_face = &old_faces[best_old];

        // Resolve UV axes from old face (texture lock: preserve axes)
        let (u_axis, v_axis) =
            if old_face.uv_u_axis != Vec3::ZERO && old_face.uv_v_axis != Vec3::ZERO {
                (old_face.uv_u_axis, old_face.uv_v_axis)
            } else {
                compute_face_tangent_axes(old_face.plane.normal)
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

        // Preserve scale: texels-per-world-unit stays constant.
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
            ..Default::default()
        });
    }

    // Build old-to-new face index mapping by inverting best_old_per_new
    let mut old_to_new = vec![0usize; old_faces.len()];
    for (new_idx, &old_idx) in best_old_per_new.iter().enumerate() {
        old_to_new[old_idx] = new_idx;
    }

    let topology = compute_brush_topology(&faces);
    Some((faces, topology, old_to_new))
}

/// Build brush faces for the convex hull of `all_local_verts`, matching each
/// resulting face back to an existing face so material and UV projection carry
/// over.
///
/// The first `existing_count` entries of `all_local_verts` are the original
/// brush's vertices; the remainder are newly added points. A hull face is
/// matched to an old face only when it contains at least one original vertex,
/// scoring candidates by shared-vertex overlap plus a small normal-similarity
/// term. Faces made purely of new vertices, and faces with no match, take
/// `default_material` and freshly computed tangent axes.
///
/// `old_faces` and `old_face_polygons` are the existing faces and their vertex
/// index rings (indices into the original `existing_count` vertices). Returns
/// `None` when there are fewer than four input vertices or the hull is
/// degenerate (fewer than four hull faces).
pub fn build_hull_faces_matching(
    all_local_verts: &[Vec3],
    existing_count: usize,
    old_faces: &[BrushFaceData],
    old_face_polygons: &[Vec<usize>],
    default_material: FaceMaterial,
) -> Option<Vec<BrushFaceData>> {
    if all_local_verts.len() < 4 {
        return None;
    }
    let (hull_positions, hull_tris) = parry3d::transformation::convex_hull(all_local_verts);
    if hull_positions.len() < 4 || hull_tris.is_empty() {
        return None;
    }
    let hull_faces = merge_hull_triangles(&hull_positions, &hull_tris);
    if hull_faces.len() < 4 {
        return None;
    }

    // Map each hull vertex back to the nearest input vertex index.
    let hull_to_input: Vec<usize> = hull_positions
        .iter()
        .map(|hp| {
            all_local_verts
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

    let mut new_faces = Vec::with_capacity(hull_faces.len());
    for hull_face in &hull_faces {
        let input_verts: Vec<usize> = hull_face
            .vertex_indices
            .iter()
            .map(|&hi| hull_to_input[hi])
            .collect();
        let has_original = input_verts.iter().any(|&i| i < existing_count);

        let mut best_old = None;
        let mut best_score = -1.0_f32;
        if has_original {
            for (old_idx, old_polygon) in old_face_polygons.iter().enumerate() {
                let old_set: HashSet<usize> = old_polygon.iter().copied().collect();
                let overlap = input_verts
                    .iter()
                    .filter(|&&i| i < existing_count && old_set.contains(&i))
                    .count() as f32;
                let normal_sim = hull_face.normal.dot(old_faces[old_idx].plane.normal);
                let score = overlap + normal_sim * 0.1;
                if score > best_score {
                    best_score = score;
                    best_old = Some(old_idx);
                }
            }
        }

        let face_data = if let Some(old_idx) = best_old {
            let old_face = &old_faces[old_idx];
            BrushFaceData {
                plane: BrushPlane {
                    normal: hull_face.normal,
                    distance: hull_face.distance,
                },
                material: old_face.material.clone(),
                uv_offset: old_face.uv_offset,
                uv_scale: old_face.uv_scale,
                uv_rotation: old_face.uv_rotation,
                uv_u_axis: old_face.uv_u_axis,
                uv_v_axis: old_face.uv_v_axis,
                ..Default::default()
            }
        } else {
            let (u, v) = compute_face_tangent_axes(hull_face.normal);
            BrushFaceData {
                plane: BrushPlane {
                    normal: hull_face.normal,
                    distance: hull_face.distance,
                },
                material: default_material.clone(),
                uv_scale: Vec2::ONE,
                uv_u_axis: u,
                uv_v_axis: v,
                ..Default::default()
            }
        };
        new_faces.push(face_data);
    }

    Some(new_faces)
}

/// Compute the 2D convex hull of coplanar points projected onto the given axes.
///
/// Returns the subset of input points forming the hull, in CCW winding order.
/// Uses Andrew's monotone chain on the 2D projection. Fewer than three points
/// are returned unchanged.
pub fn convex_hull_on_plane(points: &[Vec3], axis_u: Vec3, axis_v: Vec3) -> Vec<Vec3> {
    if points.len() < 3 {
        return points.to_vec();
    }

    // Project to 2D
    let pts2d: Vec<Vec2> = points
        .iter()
        .map(|p| Vec2::new(p.dot(axis_u), p.dot(axis_v)))
        .collect();

    // Andrew's monotone chain algorithm
    let mut indexed: Vec<usize> = (0..pts2d.len()).collect();
    indexed.sort_by(|&a, &b| {
        pts2d[a]
            .x
            .partial_cmp(&pts2d[b].x)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                pts2d[a]
                    .y
                    .partial_cmp(&pts2d[b].y)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });

    let cross = |o: Vec2, a: Vec2, b: Vec2| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);

    let mut hull: Vec<usize> = Vec::new();
    // Lower hull
    for &i in &indexed {
        while hull.len() >= 2
            && cross(
                pts2d[hull[hull.len() - 2]],
                pts2d[hull[hull.len() - 1]],
                pts2d[i],
            ) <= 0.0
        {
            hull.pop();
        }
        hull.push(i);
    }
    // Upper hull
    let lower_len = hull.len() + 1;
    for &i in indexed.iter().rev() {
        while hull.len() >= lower_len
            && cross(
                pts2d[hull[hull.len() - 2]],
                pts2d[hull[hull.len() - 1]],
                pts2d[i],
            ) <= 0.0
        {
            hull.pop();
        }
        hull.push(i);
    }
    hull.pop(); // remove duplicate of first point

    hull.iter().map(|&i| points[i]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jackdaw_geometry::{BrushTopology, compute_brush_topology, cuboid_faces};

    /// A unit cuboid centered at the origin: vertex positions plus per-face
    /// rings (indices into those positions) and the face data. Built from
    /// `cuboid_faces` -> `compute_brush_topology`, the way the editor builds
    /// its mesh cache. 8 verts, 6 quad faces with axis-aligned planes at
    /// +/-0.5 on each axis.
    struct Cuboid {
        vertices: Vec<Vec3>,
        rings: Vec<Vec<usize>>,
        faces: Vec<BrushFaceData>,
    }

    fn cuboid() -> Cuboid {
        let faces = cuboid_faces(Vec3::splat(1.0));
        let topo: BrushTopology = compute_brush_topology(&faces);
        let vertices: Vec<Vec3> = topo.vertices.iter().map(|v| v.position).collect();
        let rings: Vec<Vec<usize>> = topo
            .polygons
            .iter()
            .map(|poly| {
                let start = poly.loop_start as usize;
                let total = poly.loop_total as usize;
                topo.loops[start..start + total]
                    .iter()
                    .map(|l| l.vert as usize)
                    .collect()
            })
            .collect();
        Cuboid {
            vertices,
            rings,
            faces,
        }
    }

    fn axis_aligned_normal(n: Vec3) -> bool {
        let abs = n.abs();
        let on_axis = |a: f32, b: f32, c: f32| (a - 1.0).abs() < 1e-4 && b < 1e-4 && c < 1e-4;
        on_axis(abs.x, abs.y, abs.z) || on_axis(abs.y, abs.x, abs.z) || on_axis(abs.z, abs.x, abs.y)
    }

    // --- convex_hull_on_plane -------------------------------------------------

    #[test]
    fn planar_hull_fewer_than_three_points_returns_input() {
        let two = vec![Vec3::ZERO, Vec3::X];
        let out = convex_hull_on_plane(&two, Vec3::X, Vec3::Y);
        assert_eq!(out, two);

        let one = vec![Vec3::new(3.0, 0.0, 0.0)];
        assert_eq!(convex_hull_on_plane(&one, Vec3::X, Vec3::Y), one);

        let none: Vec<Vec3> = Vec::new();
        assert_eq!(convex_hull_on_plane(&none, Vec3::X, Vec3::Y), none);
    }

    #[test]
    fn planar_hull_drops_interior_point() {
        // A unit square in the XY plane plus an interior point.
        let pts = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.5, 0.5, 0.0), // interior
        ];
        let hull = convex_hull_on_plane(&pts, Vec3::X, Vec3::Y);
        assert_eq!(hull.len(), 4, "interior point dropped: {hull:?}");
        // The interior point must not appear in the hull.
        assert!(
            !hull
                .iter()
                .any(|p| (*p - Vec3::new(0.5, 0.5, 0.0)).length() < 1e-5),
            "interior point leaked into hull"
        );
        // All four corners are present.
        for corner in [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        ] {
            assert!(
                hull.iter().any(|p| (*p - corner).length() < 1e-5),
                "missing corner {corner:?}"
            );
        }
    }

    #[test]
    fn planar_hull_winds_counter_clockwise() {
        let pts = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(2.0, 2.0, 0.0),
            Vec3::new(0.0, 2.0, 0.0),
        ];
        let hull = convex_hull_on_plane(&pts, Vec3::X, Vec3::Y);
        assert_eq!(hull.len(), 4);
        // Signed area (shoelace) of the projection is positive for CCW.
        let mut area = 0.0_f32;
        for i in 0..hull.len() {
            let a = hull[i];
            let b = hull[(i + 1) % hull.len()];
            area += a.x * b.y - b.x * a.y;
        }
        assert!(area > 0.0, "expected CCW winding, signed area = {area}");
    }

    #[test]
    fn planar_hull_drops_collinear_midpoints() {
        // Triangle with extra points sitting on its edges; the hull keeps only
        // the three corners (monotone chain rejects collinear midpoints).
        let pts = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0), // on bottom edge
            Vec3::new(1.0, 2.0, 0.0),
        ];
        let hull = convex_hull_on_plane(&pts, Vec3::X, Vec3::Y);
        assert_eq!(hull.len(), 3, "collinear midpoint dropped: {hull:?}");
        assert!(
            !hull
                .iter()
                .any(|p| (*p - Vec3::new(1.0, 0.0, 0.0)).length() < 1e-5),
            "collinear midpoint leaked into hull"
        );
    }

    #[test]
    fn planar_hull_respects_axes() {
        // The same square projected onto the XZ plane (axis_v = Z). The hull
        // still recovers the four corners.
        let pts = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.5, 0.0, 0.5), // interior in XZ
        ];
        let hull = convex_hull_on_plane(&pts, Vec3::X, Vec3::Z);
        assert_eq!(hull.len(), 4, "{hull:?}");
    }

    // --- merge_hull_triangles -------------------------------------------------

    /// The 8 corners and 12 triangles (two per face) of a unit cube.
    fn cube_corners_and_tris() -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let c = [
            Vec3::new(-0.5, -0.5, -0.5), // 0
            Vec3::new(0.5, -0.5, -0.5),  // 1
            Vec3::new(0.5, 0.5, -0.5),   // 2
            Vec3::new(-0.5, 0.5, -0.5),  // 3
            Vec3::new(-0.5, -0.5, 0.5),  // 4
            Vec3::new(0.5, -0.5, 0.5),   // 5
            Vec3::new(0.5, 0.5, 0.5),    // 6
            Vec3::new(-0.5, 0.5, 0.5),   // 7
        ];
        let tris = vec![
            // -Z face (outward normal -Z): 0,3,2 / 0,2,1
            [0, 3, 2],
            [0, 2, 1],
            // +Z face: 4,5,6 / 4,6,7
            [4, 5, 6],
            [4, 6, 7],
            // -Y face: 0,1,5 / 0,5,4
            [0, 1, 5],
            [0, 5, 4],
            // +Y face: 3,7,6 / 3,6,2
            [3, 7, 6],
            [3, 6, 2],
            // -X face: 0,4,7 / 0,7,3
            [0, 4, 7],
            [0, 7, 3],
            // +X face: 1,2,6 / 1,6,5
            [1, 2, 6],
            [1, 6, 5],
        ];
        (c.to_vec(), tris)
    }

    #[test]
    fn merge_recovers_six_cube_faces() {
        let (verts, tris) = cube_corners_and_tris();
        let faces = merge_hull_triangles(&verts, &tris);
        assert_eq!(faces.len(), 6, "cube has six coplanar faces");
        for f in &faces {
            assert_eq!(f.vertex_indices.len(), 4, "each face is a quad");
            assert!(
                axis_aligned_normal(f.normal),
                "face normal should be axis aligned: {:?}",
                f.normal
            );
            // plane distance is +/-0.5 for a unit cube centered at origin
            assert!(
                (f.distance.abs() - 0.5).abs() < 1e-4,
                "face plane distance {} should be 0.5",
                f.distance
            );
        }
        // All six axis directions are represented exactly once.
        for dir in [
            Vec3::X,
            Vec3::NEG_X,
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::Z,
            Vec3::NEG_Z,
        ] {
            let count = faces.iter().filter(|f| f.normal.dot(dir) > 0.99).count();
            assert_eq!(count, 1, "exactly one face with normal {dir:?}");
        }
    }

    #[test]
    fn merge_skips_degenerate_triangles() {
        let (mut verts, mut tris) = cube_corners_and_tris();
        // Append a duplicate vertex and a zero-area triangle referencing it.
        verts.push(verts[0]);
        let dup = (verts.len() - 1) as u32;
        tris.push([0, dup, 1]); // collinear-with-zero-area (two identical points)
        let faces = merge_hull_triangles(&verts, &tris);
        // The degenerate triangle contributes no new face group.
        assert_eq!(faces.len(), 6, "degenerate triangle skipped");
    }

    #[test]
    fn merge_winds_face_vertices_consistently() {
        let (verts, tris) = cube_corners_and_tris();
        let faces = merge_hull_triangles(&verts, &tris);
        // For each face, consecutive ring edges should trace a convex quad:
        // the polygon's signed area in its own plane is non-zero.
        for f in &faces {
            let n = f.vertex_indices.len();
            let mut area = 0.0_f32;
            for i in 0..n {
                let a = verts[f.vertex_indices[i]];
                let b = verts[f.vertex_indices[(i + 1) % n]];
                area += a.cross(b).dot(f.normal);
            }
            assert!(
                area.abs() > 1e-3,
                "face winding traces a non-degenerate loop"
            );
        }
    }

    // --- rebuild_brush_from_vertices -----------------------------------------

    #[test]
    fn rebuild_too_few_vertices_is_none() {
        let c = cuboid();
        let verts = vec![Vec3::ZERO, Vec3::X, Vec3::Y];
        let out = rebuild_brush_from_vertices(&c.faces, &c.vertices, &c.rings, &verts);
        assert!(out.is_none(), "fewer than four input vertices returns None");
    }

    #[test]
    fn rebuild_from_cube_corners_yields_six_faces() {
        let c = cuboid();
        let (faces, topology, old_to_new) =
            rebuild_brush_from_vertices(&c.faces, &c.vertices, &c.rings, &c.vertices)
                .expect("rebuilding from the cube corners succeeds");
        assert_eq!(faces.len(), 6, "rebuilt brush has six faces");
        assert_eq!(topology.polygons.len(), 6, "topology has six polygons");
        assert_eq!(
            old_to_new.len(),
            c.faces.len(),
            "one map entry per old face"
        );
        for f in &faces {
            assert!(
                axis_aligned_normal(f.plane.normal),
                "rebuilt face normal axis aligned: {:?}",
                f.plane.normal
            );
        }
    }

    #[test]
    fn rebuild_carries_over_material_and_uv() {
        let mut c = cuboid();
        // Stamp a distinctive UV scale on every old face so we can detect that
        // the rebuilt faces inherited it.
        for (i, f) in c.faces.iter_mut().enumerate() {
            f.uv_scale = Vec2::new(2.0 + i as f32, 3.0 + i as f32);
            f.uv_rotation = 0.25 * i as f32;
        }
        let (faces, _topology, _old_to_new) =
            rebuild_brush_from_vertices(&c.faces, &c.vertices, &c.rings, &c.vertices)
                .expect("rebuild succeeds");
        // Every rebuilt face's scale must equal some old face's stamped scale
        // (scale is preserved verbatim from the matched old face).
        for f in &faces {
            let matched = c
                .faces
                .iter()
                .any(|of| (of.uv_scale - f.uv_scale).length() < 1e-5);
            assert!(
                matched,
                "rebuilt uv_scale {:?} came from an old face",
                f.uv_scale
            );
            // uv axes are non-zero (resolved from the matched face or tangent).
            assert!(f.uv_u_axis.length() > 0.5 && f.uv_v_axis.length() > 0.5);
        }
    }

    // --- build_hull_faces_matching -------------------------------------------

    #[test]
    fn build_hull_too_few_vertices_is_none() {
        let c = cuboid();
        let verts = vec![Vec3::ZERO, Vec3::X, Vec3::Y];
        let out = build_hull_faces_matching(&verts, 0, &c.faces, &c.rings, FaceMaterial::default());
        assert!(out.is_none(), "fewer than four input vertices returns None");
    }

    #[test]
    fn build_hull_carries_matched_uv() {
        // `FaceMaterial` is `String` here but `Handle<StandardMaterial>` under
        // the editor's `render` feature, so material identity is not assertable
        // across both builds. UV carry-over is, and it is the signal that proves
        // the matched branch fired (the fallback branch resets uv_scale to one).
        let mut c = cuboid();
        for (i, f) in c.faces.iter_mut().enumerate() {
            f.uv_scale = Vec2::new(2.0 + i as f32, 3.0 + i as f32);
            f.uv_rotation = 0.25 * i as f32;
        }
        let faces = build_hull_faces_matching(
            &c.vertices,
            c.vertices.len(),
            &c.faces,
            &c.rings,
            FaceMaterial::default(),
        )
        .expect("hull of the cube corners succeeds");
        assert_eq!(faces.len(), 6, "cube hull has six faces");
        for f in &faces {
            // Every face had original vertices, so it matched an old face: its
            // uv_scale came from one of the stamped old faces, never the unit
            // fallback scale.
            assert_ne!(f.uv_scale, Vec2::ONE, "matched face kept the fallback uv");
            let matched = c
                .faces
                .iter()
                .any(|of| (of.uv_scale - f.uv_scale).length() < 1e-5);
            assert!(
                matched,
                "rebuilt uv_scale {:?} came from an old face",
                f.uv_scale
            );
        }
    }

    #[test]
    fn build_hull_all_new_uses_default_material() {
        let c = cuboid();
        // With existing_count == 0 no vertex is "original", so every hull face
        // falls back to the default material and fresh tangent axes.
        let faces =
            build_hull_faces_matching(&c.vertices, 0, &c.faces, &c.rings, FaceMaterial::default())
                .expect("hull succeeds");
        assert_eq!(faces.len(), 6);
        for f in &faces {
            assert_eq!(
                f.material,
                FaceMaterial::default(),
                "all-new face uses default material"
            );
            assert_eq!(f.uv_scale, Vec2::ONE, "all-new face uses unit uv scale");
            assert!(
                f.uv_u_axis.length() > 0.5 && f.uv_v_axis.length() > 0.5,
                "fresh tangent axes are non-zero"
            );
        }
    }
}
