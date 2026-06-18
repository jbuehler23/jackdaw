//! Per-face render buffers for a brush. Triangulate one face ring, flat-shade
//! it with the face plane normal, project its UVs, and derive a tangent. Plain
//! float buffers only; the editor and runtime assemble Bevy meshes from these.

use glam::Vec3;

use crate::{BrushFaceData, compute_face_tangent_axes, compute_face_uvs, triangulate_polygon};

/// One face's render geometry. The ring is emitted once (per-face layout); all
/// vertices share the face normal (flat shading). `indices` are local, ramped
/// from 0.
pub struct FaceRenderBuffers {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub tangents: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

impl FaceRenderBuffers {
    pub fn tri_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// Build one face's render buffers. `ring` indexes into `vertices`. Returns
/// empty buffers for a degenerate ring (fewer than three vertices).
pub fn build_face_render_buffers(
    vertices: &[Vec3],
    ring: &[usize],
    face: &BrushFaceData,
) -> FaceRenderBuffers {
    if ring.len() < 3 {
        return FaceRenderBuffers {
            positions: Vec::new(),
            normals: Vec::new(),
            uvs: Vec::new(),
            tangents: Vec::new(),
            indices: Vec::new(),
        };
    }

    let positions: Vec<[f32; 3]> = ring.iter().map(|&vi| vertices[vi].to_array()).collect();
    let normal = face.plane.normal;
    let normals: Vec<[f32; 3]> = vec![normal.to_array(); ring.len()];

    let (u_axis, v_axis) = if face.uv_u_axis != Vec3::ZERO && face.uv_v_axis != Vec3::ZERO {
        (face.uv_u_axis, face.uv_v_axis)
    } else {
        compute_face_tangent_axes(normal)
    };
    let uvs = compute_face_uvs(
        vertices,
        ring,
        u_axis,
        v_axis,
        face.uv_offset,
        face.uv_scale,
        face.uv_rotation,
    );
    let w = normal.dot(u_axis.cross(v_axis)).signum();
    let tangents: Vec<[f32; 4]> = vec![[u_axis.x, u_axis.y, u_axis.z, w]; ring.len()];

    // Concave / keyhole-bridged faces need a real triangulator; fan
    // triangulation fills holes and mis-tiles L-shapes. Indices are local to
    // the ring (0..ring.len()).
    let ring_verts: Vec<Vec3> = ring.iter().map(|&vi| vertices[vi]).collect();
    let identity: Vec<u32> = (0..ring.len() as u32).collect();
    let tris = triangulate_polygon(&ring_verts, &identity, normal);
    let indices: Vec<u32> = tris.iter().flat_map(|t| t.iter().copied()).collect();

    FaceRenderBuffers {
        positions,
        normals,
        uvs,
        tangents,
        indices,
    }
}

use crate::reflected_face_plane;

/// Resolve the per-face data to pair with each evaluated polygon. `face_source`
/// maps an evaluated face index back to an authored face (`NO_SOURCE` for cut
/// geometry with no origin). Mirrored faces (where `face_source[i] != i`) clone
/// their authored data but get the plane recomputed from the evaluated ring,
/// since the authored normal is un-reflected and would wind the triangulation
/// inside out. Faces without authored data fall back to default.
pub fn resolve_evaluated_faces(
    face_source: &[u32],
    vertices: &[Vec3],
    face_polygons: &[Vec<usize>],
    authored_faces: &[BrushFaceData],
) -> Vec<BrushFaceData> {
    face_source
        .iter()
        .enumerate()
        .map(|(evaluated_idx, &src)| {
            let mut face = authored_faces
                .get(src as usize)
                .cloned()
                .unwrap_or_default();
            if src as usize != evaluated_idx
                && let Some(plane) = reflected_face_plane(vertices, &face_polygons[evaluated_idx])
            {
                face.plane = plane;
            }
            face
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BrushFaceData, BrushPlane, compute_face_tangent_axes};
    use glam::{Vec2, Vec3};

    fn quad() -> (Vec<Vec3>, Vec<usize>, BrushFaceData) {
        let verts = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        ];
        let (u, v) = compute_face_tangent_axes(Vec3::Z);
        let face = BrushFaceData {
            plane: BrushPlane {
                normal: Vec3::Z,
                distance: 0.0,
            },
            uv_scale: Vec2::ONE,
            uv_u_axis: u,
            uv_v_axis: v,
            ..Default::default()
        };
        (verts, vec![0, 1, 2, 3], face)
    }

    #[test]
    fn quad_builds_two_triangles_flat_shaded() {
        let (verts, ring, face) = quad();
        let buf = build_face_render_buffers(&verts, &ring, &face);
        assert_eq!(buf.indices.len(), 6, "quad earcuts to 2 triangles");
        assert_eq!(
            buf.positions.len(),
            4,
            "ring emitted once (per-face layout)"
        );
        for n in &buf.normals {
            assert!((Vec3::from_array(*n) - Vec3::Z).length() < 1e-5);
        }
        assert_eq!(buf.normals.len(), buf.positions.len());
        assert_eq!(buf.uvs.len(), buf.positions.len());
        assert_eq!(buf.tangents.len(), buf.positions.len());
    }

    #[test]
    fn pentagon_earcuts_to_three_triangles() {
        let verts = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(2.5, 1.5, 0.0),
            Vec3::new(1.0, 2.5, 0.0),
            Vec3::new(-0.5, 1.5, 0.0),
        ];
        let (u, v) = compute_face_tangent_axes(Vec3::Z);
        let face = BrushFaceData {
            plane: BrushPlane {
                normal: Vec3::Z,
                distance: 0.0,
            },
            uv_scale: Vec2::ONE,
            uv_u_axis: u,
            uv_v_axis: v,
            ..Default::default()
        };
        let buf = build_face_render_buffers(&verts, &[0, 1, 2, 3, 4], &face);
        assert_eq!(
            buf.indices.len(),
            9,
            "a convex pentagon earcuts to 3 triangles"
        );
        assert_eq!(buf.positions.len(), 5);
    }

    #[test]
    fn resolve_evaluated_faces_recomputes_mirrored_plane() {
        use crate::{BrushPlane, MeshMirror, compute_face_tangent_axes, evaluate_mirror};
        use glam::{Vec2, Vec3};

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
        let resolved = resolve_evaluated_faces(
            &eval.face_source,
            &eval.vertices,
            &eval.face_polygons,
            &faces,
        );

        assert_eq!(resolved.len(), 2);
        assert!(resolved[0].plane.normal.x > 0.0);
        assert!(
            resolved[1].plane.normal.x < 0.0,
            "mirrored plane recomputed from reflected ring"
        );
    }
}
