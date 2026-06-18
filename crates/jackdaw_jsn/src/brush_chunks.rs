//! Group a brush's faces into per-material render chunks. One place builds the
//! brush render mesh for both the editor viewport and the runtime.

use glam::Vec3;
use jackdaw_geometry::{FaceMaterial, build_face_render_buffers};

use crate::types::BrushFaceData;

/// CPU-side buffers for one material chunk of a brush's render mesh.
pub struct MeshChunk {
    pub material: FaceMaterial,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub tangents: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    /// Authored face index for each triangle (parallel to `indices` in groups of 3).
    pub face_of_tri: Vec<u32>,
}

/// Group a brush's already-evaluated faces into per-material render chunks.
/// `vertices` / `face_polygons` / `faces` are post-modifier geometry: the caller
/// folds the modifier stack (`evaluate_modifier_stack`) and resolves evaluated
/// face data (`resolve_evaluated_faces`) first, since the editor also needs the
/// fold's source maps for its picker cache. Chunk order follows first appearance
/// of each material; faces with fewer than three vertices are skipped.
pub fn build_brush_chunks(
    vertices: &[Vec3],
    face_polygons: &[Vec<usize>],
    faces: &[BrushFaceData],
) -> Vec<MeshChunk> {
    let mut chunks: Vec<MeshChunk> = Vec::new();
    for (face_idx, face) in faces.iter().enumerate() {
        let Some(ring) = face_polygons.get(face_idx) else {
            continue;
        };
        if ring.len() < 3 {
            continue;
        }
        let buf = build_face_render_buffers(vertices, ring, face);
        if buf.indices.is_empty() {
            continue;
        }

        let chunk_idx = match chunks.iter().position(|c| c.material == face.material) {
            Some(i) => i,
            None => {
                chunks.push(MeshChunk {
                    material: face.material.clone(),
                    positions: Vec::new(),
                    normals: Vec::new(),
                    uvs: Vec::new(),
                    tangents: Vec::new(),
                    indices: Vec::new(),
                    face_of_tri: Vec::new(),
                });
                chunks.len() - 1
            }
        };
        let chunk = &mut chunks[chunk_idx];
        let base = chunk.positions.len() as u32;
        chunk.positions.extend_from_slice(&buf.positions);
        chunk.normals.extend_from_slice(&buf.normals);
        chunk.uvs.extend_from_slice(&buf.uvs);
        chunk.tangents.extend_from_slice(&buf.tangents);
        for &i in &buf.indices {
            chunk.indices.push(base + i);
        }
        for _ in 0..buf.indices.len() / 3 {
            chunk.face_of_tri.push(face_idx as u32);
        }
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Brush;
    use glam::Vec3;

    fn cube_inputs() -> (Vec<Vec3>, Vec<Vec<usize>>, Vec<crate::types::BrushFaceData>) {
        let brush = Brush::cuboid(0.5, 0.5, 0.5);
        let vertices: Vec<Vec3> = brush.topology.vertices.iter().map(|v| v.position).collect();
        let face_polygons: Vec<Vec<usize>> = (0..brush.topology.polygons.len())
            .map(|i| brush.topology.face_ring(i).map(|v| v as usize).collect())
            .collect();
        (vertices, face_polygons, brush.faces)
    }

    #[test]
    fn uniform_cube_builds_one_chunk() {
        let (v, fp, faces) = cube_inputs();
        let chunks = build_brush_chunks(&v, &fp, &faces);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].face_of_tri.len(), 12); // 6 quads * 2 tris
        assert_eq!(chunks[0].indices.len(), 36);
        for face_idx in 0..6u32 {
            assert_eq!(
                chunks[0]
                    .face_of_tri
                    .iter()
                    .filter(|&&f| f == face_idx)
                    .count(),
                2
            );
        }
    }

    #[test]
    fn ngon_face_round_trips_as_one_face() {
        // Replace face 0's ring with a hexagon of new verts; it must earcut to
        // 4 triangles, every one mapping back to face index 0.
        let (mut v, mut fp, mut faces) = cube_inputs();
        let base = v.len();
        for k in 0..6 {
            let a = std::f32::consts::TAU * k as f32 / 6.0;
            v.push(Vec3::new(a.cos(), a.sin(), 2.0));
        }
        fp[0] = (base..base + 6).collect();
        faces[0] = crate::types::BrushFaceData {
            plane: crate::types::BrushPlane {
                normal: Vec3::Z,
                ..Default::default()
            },
            ..Default::default()
        };

        let chunks = build_brush_chunks(&v, &fp, &faces);
        let tris_for_face0 = chunks
            .iter()
            .flat_map(|c| c.face_of_tri.iter())
            .filter(|&&f| f == 0)
            .count();
        assert_eq!(tris_for_face0, 4, "a hexagon renders as 4 tris, all face 0");
    }

    /// Build the `red` face material in either feature config: a distinct
    /// `Handle<StandardMaterial>` under `render`, a non-default string id
    /// without it. Both differ from the default material, so face 0 splits
    /// into its own chunk.
    #[cfg(feature = "render")]
    fn red_material() -> FaceMaterial {
        bevy::asset::uuid_handle!("8e6c3d2a-5b14-4f9e-9a77-c01d54a3b681")
    }
    #[cfg(not(feature = "render"))]
    fn red_material() -> FaceMaterial {
        "red".into()
    }

    #[test]
    fn explicit_material_splits_into_second_chunk() {
        let (v, fp, mut faces) = cube_inputs();
        faces[0].material = red_material();

        let chunks = build_brush_chunks(&v, &fp, &faces);

        // First-seen order: face 0 (red) starts chunk 0, faces 1..6 share the
        // default chunk.
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].material, red_material());
        assert_eq!(chunks[0].face_of_tri, vec![0, 0]);
        assert_eq!(chunks[1].face_of_tri, vec![1, 1, 2, 2, 3, 3, 4, 4, 5, 5]);

        // Each chunk's indices are local to its own vertex block; a shared
        // global base counter would push chunk 1's indices past chunk 0's
        // vertices (the red quad contributes four).
        assert_eq!(chunks[1].indices.iter().min(), Some(&0));
        assert!(
            (*chunks[1].indices.iter().max().unwrap() as usize) < chunks[1].positions.len(),
            "chunk 1 indices must reference only its own vertices"
        );
    }

    #[test]
    fn mirrored_face_with_recomputed_plane_flips_chunk_normals() {
        use glam::Vec2;
        use jackdaw_geometry::{
            BrushPlane, MeshMirror, compute_face_tangent_axes, evaluate_mirror,
        };

        // A single +X cap quad at x=1, mirrored across the default x=0 plane.
        let vertices = vec![
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(1.0, 0.0, 1.0),
        ];
        let face_polygons = vec![vec![0, 1, 2, 3]];
        let (u, v) = compute_face_tangent_axes(Vec3::X);
        let faces = vec![BrushFaceData {
            plane: BrushPlane {
                normal: Vec3::X,
                distance: 1.0,
            },
            uv_scale: Vec2::ONE,
            uv_u_axis: u,
            uv_v_axis: v,
            ..Default::default()
        }];

        // Evaluate the mirror to get the two-face geometry, then let
        // build_brush_chunks resolve the evaluated faces (it recomputes the
        // mirrored cap's plane from its ring) and build the chunk.
        let eval = evaluate_mirror(&vertices, &face_polygons, &MeshMirror::default());
        assert_eq!(eval.face_polygons.len(), 2);
        let resolved = jackdaw_geometry::resolve_evaluated_faces(
            &eval.face_source,
            &eval.vertices,
            &eval.face_polygons,
            &faces,
        );

        let chunks = build_brush_chunks(&eval.vertices, &eval.face_polygons, &resolved);
        assert_eq!(chunks.len(), 1);
        let chunk = &chunks[0];

        // Per-vertex normals come from the resolved face plane, and each
        // triangle's winding is independent. Walk the triangles (3 indices
        // each) and, using `face_of_tri`, check both: the authored cap (face 0)
        // shades and winds toward +X, the mirrored cap (face 1) toward -X. This
        // confirms `resolve_evaluated_faces` recomputed the mirrored plane (it
        // would otherwise inherit the authored +X) and the triangulator wound
        // the mirrored ring the other way, not merely copied the hint normal.
        for (tri_idx, &face_idx) in chunk.face_of_tri.iter().enumerate() {
            let i0 = chunk.indices[tri_idx * 3] as usize;
            let i1 = chunk.indices[tri_idx * 3 + 1] as usize;
            let i2 = chunk.indices[tri_idx * 3 + 2] as usize;
            let p = |i: usize| Vec3::from_array(chunk.positions[i]);
            let winding_x = (p(i1) - p(i0)).cross(p(i2) - p(i0)).normalize().x;
            for &i in &[i0, i1, i2] {
                let n = chunk.normals[i];
                if face_idx == 0 {
                    assert!(n[0] > 0.0, "authored cap normal must face +X, got {n:?}");
                } else {
                    assert!(n[0] < 0.0, "mirrored cap normal must face -X, got {n:?}");
                }
            }
            if face_idx == 0 {
                assert!(
                    winding_x > 0.0,
                    "authored cap must wind +X, got {winding_x}"
                );
            } else {
                assert!(
                    winding_x < 0.0,
                    "mirrored cap must wind -X, got {winding_x}"
                );
            }
        }
    }

    #[test]
    fn uv_math_matches_reference_formula() {
        use glam::Vec2;

        let (vertices, face_polygons, mut faces) = cube_inputs();
        // Non-trivial transform so each term of the formula matters.
        faces[0].uv_rotation = 0.5;
        faces[0].uv_scale = Vec2::new(2.0, 4.0);
        faces[0].uv_offset = Vec2::new(0.25, -0.75);

        let chunks = build_brush_chunks(&vertices, &face_polygons, &faces);
        let chunk = &chunks[0];

        // The chunk emits one vertex per ring vertex in ring order, so the
        // first UV belongs to face 0's first ring vertex. Recompute it straight
        // from the documented formula: project -> rotate -> scale -> offset.
        let face_data = &faces[0];
        let p = vertices[face_polygons[0][0]];
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
