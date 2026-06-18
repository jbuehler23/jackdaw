//! Accumulates brush faces into per-material render-mesh buffers.
//!
//! The render build emits one Bevy mesh per chunk instead of one per
//! face; `face_of_tri` maps every triangle back to its authored face
//! index so raycast hits can resolve faces.

use bevy::prelude::*;
use jackdaw_geometry::{BrushFaceData, compute_face_tangent_axes, triangulate_polygon};

/// CPU-side buffers for one material chunk of a brush's render mesh.
pub(crate) struct ChunkBuffers {
    /// The shared face material; `Handle::default()` for the
    /// default-palette chunk.
    pub material: Handle<StandardMaterial>,
    /// True when this is the default-palette chunk, making it eligible
    /// for the per-frame selection/preview material swap.
    pub uses_default_material: bool,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub tangents: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    /// Authored face index for each triangle (3 consecutive indices).
    pub face_of_tri: Vec<u32>,
}

impl ChunkBuffers {
    fn new(material: Handle<StandardMaterial>) -> Self {
        let uses_default_material = material == Handle::default();
        Self {
            material,
            uses_default_material,
            positions: Vec::new(),
            normals: Vec::new(),
            uvs: Vec::new(),
            tangents: Vec::new(),
            indices: Vec::new(),
            face_of_tri: Vec::new(),
        }
    }
}

/// Triangulate every face and accumulate buffers per material chunk.
/// Chunk order follows first appearance of each material so rebuilds
/// are deterministic. Faces with fewer than three vertices are skipped.
pub(crate) fn build_mesh_chunks(
    vertices: &[Vec3],
    face_polygons: &[Vec<usize>],
    faces: &[BrushFaceData],
) -> Vec<ChunkBuffers> {
    let mut chunks: Vec<ChunkBuffers> = Vec::new();

    for (face_idx, face_data) in faces.iter().enumerate() {
        let Some(indices) = face_polygons.get(face_idx) else {
            continue;
        };
        if indices.len() < 3 {
            continue;
        }

        let chunk_idx = match chunks.iter().position(|c| c.material == face_data.material) {
            Some(i) => i,
            None => {
                chunks.push(ChunkBuffers::new(face_data.material.clone()));
                chunks.len() - 1
            }
        };
        let chunk = &mut chunks[chunk_idx];

        // Build per-triangle (flat-shaded) buffers so non-planar faces
        // render correctly. Each triangle gets its own computed normal;
        // vertex positions are duplicated (3 per tri) so every vertex
        // can carry an independent normal.
        let (u_axis, v_axis) =
            if face_data.uv_u_axis != Vec3::ZERO && face_data.uv_v_axis != Vec3::ZERO {
                (face_data.uv_u_axis, face_data.uv_v_axis)
            } else {
                compute_face_tangent_axes(face_data.plane.normal)
            };

        // Concave / annulus-aware triangulation via earcut. Fan
        // triangulation would silently mis-triangulate concave faces
        // and fill keyhole-bridged holes with bogus geometry.
        let ring_u32: Vec<u32> = indices.iter().map(|&i| i as u32).collect();
        let tris = triangulate_polygon(vertices, &ring_u32, face_data.plane.normal);

        let cos_r = face_data.uv_rotation.cos();
        let sin_r = face_data.uv_rotation.sin();

        for tri in &tris {
            let p_a = vertices[tri[0] as usize];
            let p_b = vertices[tri[1] as usize];
            let p_c = vertices[tri[2] as usize];

            let cross = (p_b - p_a).cross(p_c - p_a);
            let tri_normal = if cross.length_squared() > 1e-10 {
                cross.normalize()
            } else {
                face_data.plane.normal
            };
            let tri_normal_arr = tri_normal.to_array();

            // Tangent sign uses the face u/v axes (UV continuity is per-face).
            let w = tri_normal.dot(u_axis.cross(v_axis)).signum();
            let tangent = [u_axis.x, u_axis.y, u_axis.z, w];

            let base = chunk.positions.len() as u32;
            for &vert_pos in &[p_a, p_b, p_c] {
                chunk.positions.push(vert_pos.to_array());
                chunk.normals.push(tri_normal_arr);

                // UV math matches compute_face_uvs exactly:
                // project -> rotate -> scale -> offset.
                let u = vert_pos.dot(u_axis);
                let v = vert_pos.dot(v_axis);
                let ru = u * cos_r - v * sin_r;
                let rv = u * sin_r + v * cos_r;
                let su = ru / face_data.uv_scale.x.max(0.001) + face_data.uv_offset.x;
                let sv = rv / face_data.uv_scale.y.max(0.001) + face_data.uv_offset.y;
                chunk.uvs.push([su, sv]);
                chunk.tangents.push(tangent);
            }
            chunk.indices.push(base);
            chunk.indices.push(base + 1);
            chunk.indices.push(base + 2);
            chunk.face_of_tri.push(face_idx as u32);
        }
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::uuid_handle;

    /// (`vertices`, `face_polygons`, `faces`) for a unit cube, all faces on
    /// the default palette material.
    fn cube_inputs() -> (Vec<Vec3>, Vec<Vec<usize>>, Vec<BrushFaceData>) {
        let brush = jackdaw_jsn::Brush::cuboid(0.5, 0.5, 0.5);
        let vertices: Vec<Vec3> = brush.topology.vertices.iter().map(|v| v.position).collect();
        let face_polygons: Vec<Vec<usize>> = (0..brush.topology.polygons.len())
            .map(|i| brush.topology.face_ring(i).map(|v| v as usize).collect())
            .collect();
        (vertices, face_polygons, brush.faces)
    }

    #[test]
    fn uniform_cube_builds_one_chunk() {
        let (vertices, face_polygons, faces) = cube_inputs();
        let chunks = build_mesh_chunks(&vertices, &face_polygons, &faces);

        assert_eq!(chunks.len(), 1);
        let chunk = &chunks[0];
        assert!(chunk.uses_default_material);
        // 6 quad faces, earcut yields 2 triangles each.
        assert_eq!(chunk.face_of_tri.len(), 12);
        assert_eq!(chunk.indices.len(), 36);
        assert_eq!(chunk.positions.len(), 36);
        assert_eq!(chunk.normals.len(), 36);
        assert_eq!(chunk.uvs.len(), 36);
        assert_eq!(chunk.tangents.len(), 36);
        // Every face contributes exactly 2 consecutive triangles.
        for face_idx in 0..6u32 {
            let count = chunk.face_of_tri.iter().filter(|&&f| f == face_idx).count();
            assert_eq!(count, 2, "face {face_idx} should map to 2 triangles");
        }
    }

    #[test]
    fn explicit_material_splits_into_second_chunk() {
        let (vertices, face_polygons, mut faces) = cube_inputs();
        let red: Handle<StandardMaterial> = uuid_handle!("8e6c3d2a-5b14-4f9e-9a77-c01d54a3b681");
        faces[0].material = red.clone();

        let chunks = build_mesh_chunks(&vertices, &face_polygons, &faces);

        // First-seen order: face 0 (red) starts chunk 0, face 1 starts
        // the default chunk.
        assert_eq!(chunks.len(), 2);
        assert!(!chunks[0].uses_default_material);
        assert_eq!(chunks[0].material, red);
        assert_eq!(chunks[0].face_of_tri, vec![0, 0]);
        assert!(chunks[1].uses_default_material);
        assert_eq!(chunks[1].face_of_tri, vec![1, 1, 2, 2, 3, 3, 4, 4, 5, 5]);

        // Indices must ramp from 0 within EACH chunk (a shared global
        // base counter would start chunk 1 at 6).
        assert!(
            chunks[1]
                .indices
                .iter()
                .enumerate()
                .all(|(i, &v)| v == i as u32)
        );
        assert_eq!(chunks[1].positions.len(), 30);
    }

    #[test]
    fn degenerate_faces_are_skipped() {
        let (vertices, mut face_polygons, faces) = cube_inputs();
        // Truncate face 2's ring below a triangle.
        face_polygons[2] = vec![0, 1];

        let chunks = build_mesh_chunks(&vertices, &face_polygons, &faces);

        assert_eq!(chunks.len(), 1);
        // 5 remaining quads, 2 triangles each.
        assert_eq!(chunks[0].face_of_tri.len(), 10);
        assert!(!chunks[0].face_of_tri.contains(&2));
    }

    #[test]
    fn triangle_winding_and_uvs_match_per_face_build() {
        // The buffers must reproduce the exact per-face math the old
        // build used: positions in earcut order, UV = project onto
        // face axes, rotate, scale, offset.
        let (vertices, face_polygons, faces) = cube_inputs();
        let chunks = build_mesh_chunks(&vertices, &face_polygons, &faces);
        let chunk = &chunks[0];

        // Reproduce face 0's first triangle independently.
        let face_data = &faces[0];
        let ring_u32: Vec<u32> = face_polygons[0].iter().map(|&i| i as u32).collect();
        let tris = triangulate_polygon(&vertices, &ring_u32, face_data.plane.normal);
        let expected_first = vertices[tris[0][0] as usize];
        assert_eq!(chunk.positions[0], expected_first.to_array());

        // Indices are a plain 0..n ramp (positions are deduplicated
        // nowhere; every triangle owns 3 verts for flat shading).
        assert!(
            chunk
                .indices
                .iter()
                .enumerate()
                .all(|(i, &v)| v == i as u32)
        );
    }

    #[test]
    fn mirrored_face_with_recomputed_plane_flips_chunk_normals() {
        use jackdaw_geometry::{BrushPlane, MeshMirror, evaluate_mirror, reflected_face_plane};

        // A single +X cap quad at x=1, mirrored across the default x=0 plane.
        let vertices = vec![
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(1.0, 0.0, 1.0),
        ];
        let face_polygons = vec![vec![0, 1, 2, 3]];
        let (u, v) = compute_face_tangent_axes(Vec3::X);
        let faces = [BrushFaceData {
            plane: BrushPlane {
                normal: Vec3::X,
                distance: 1.0,
            },
            uv_scale: Vec2::ONE,
            uv_u_axis: u,
            uv_v_axis: v,
            ..Default::default()
        }];

        let eval = evaluate_mirror(&vertices, &face_polygons, &MeshMirror::default());
        assert_eq!(eval.face_polygons.len(), 2);

        // Same construction as regenerate_brush_meshes: mirrored entries
        // (past the identity prefix) clone the authored data and recompute
        // the plane from the evaluated ring.
        let evaluated_faces: Vec<BrushFaceData> = eval
            .face_source
            .iter()
            .enumerate()
            .map(|(i, &src)| {
                let mut face = faces[src as usize].clone();
                if src as usize != i
                    && let Some(plane) =
                        reflected_face_plane(&eval.vertices, &eval.face_polygons[i])
                {
                    face.plane = plane;
                }
                face
            })
            .collect();

        let chunks = build_mesh_chunks(&eval.vertices, &eval.face_polygons, &evaluated_faces);
        assert_eq!(chunks.len(), 1);
        let chunk = &chunks[0];

        // Flat-shaded normals come from each emitted triangle's winding,
        // so this checks the triangulator wound the mirrored face toward
        // -X, not merely that the hint normal was copied through.
        for (tri_idx, &face_idx) in chunk.face_of_tri.iter().enumerate() {
            for n in &chunk.normals[tri_idx * 3..tri_idx * 3 + 3] {
                if face_idx == 0 {
                    assert!(n[0] > 0.0, "authored cap must face +X, got {n:?}");
                } else {
                    assert!(n[0] < 0.0, "mirrored cap must face -X, got {n:?}");
                }
            }
        }
    }

    #[test]
    fn uv_math_matches_reference_formula() {
        let (vertices, face_polygons, mut faces) = cube_inputs();
        // Non-trivial transform so each term of the formula matters.
        faces[0].uv_rotation = 0.5;
        faces[0].uv_scale = Vec2::new(2.0, 4.0);
        faces[0].uv_offset = Vec2::new(0.25, -0.75);

        let chunks = build_mesh_chunks(&vertices, &face_polygons, &faces);
        let chunk = &chunks[0];

        // Recompute the expected UV of the first emitted vertex of face 0
        // straight from the documented formula: project -> rotate ->
        // scale -> offset.
        let face_data = &faces[0];
        let ring_u32: Vec<u32> = face_polygons[0].iter().map(|&i| i as u32).collect();
        let tris = triangulate_polygon(&vertices, &ring_u32, face_data.plane.normal);
        let p = vertices[tris[0][0] as usize];
        // cuboid populates uv_u_axis/uv_v_axis via compute_face_tangent_axes,
        // so they are non-zero and the builder takes the direct branch.
        let (u_axis, v_axis) = (face_data.uv_u_axis, face_data.uv_v_axis);
        let (u, v) = (p.dot(u_axis), p.dot(v_axis));
        let (cos_r, sin_r) = (face_data.uv_rotation.cos(), face_data.uv_rotation.sin());
        let ru = u * cos_r - v * sin_r;
        let rv = u * sin_r + v * cos_r;
        let expected = [
            ru / face_data.uv_scale.x.max(0.001) + face_data.uv_offset.x,
            rv / face_data.uv_scale.y.max(0.001) + face_data.uv_offset.y,
        ];
        assert_eq!(chunk.uvs[0], expected);
    }
}
