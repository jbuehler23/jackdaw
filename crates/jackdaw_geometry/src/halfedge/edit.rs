//! Apply a half-edge edit to a brush and reconcile its derived data.

use crate::halfedge::{FaceKey, HalfedgeMesh, VertKey};
use crate::{BrushFaceData, BrushTopology, newell_normal};

impl HalfedgeMesh {
    /// Recompute every face's cached normal from its current loop ring.
    /// Half-edge ops leave `normal_cache` stale; the reconcile refreshes it
    /// before flattening so downstream consumers see the post-edit normals.
    pub fn recache_face_normals(&mut self) {
        let face_keys: Vec<FaceKey> = self.faces.keys().collect();
        for fk in face_keys {
            let face = &self.faces[fk];
            let mut ring = Vec::with_capacity(face.loop_count as usize);
            let mut cur = face.loop_first;
            for _ in 0..face.loop_count {
                let lp = &self.loops[cur];
                ring.push(self.verts[lp.vert].co);
                cur = lp.next;
            }
            self.faces[fk].normal_cache = newell_normal(&ring);
        }
    }
}

/// A half-edge edit mesh plus the index tables binding it back to authored
/// topology order. `vert_keys` is parallel to `BrushTopology::vertices`;
/// `face_keys` is indexed by `material_idx`.
#[derive(Clone, Default)]
pub struct HalfedgeBinding {
    pub mesh: HalfedgeMesh,
    pub vert_keys: Vec<VertKey>,
    pub face_keys: Vec<FaceKey>,
}

impl HalfedgeBinding {
    /// Lift a brush topology into an edit-time binding.
    pub fn lift_from_topology(topology: &BrushTopology) -> Self {
        let mesh = HalfedgeMesh::lift_from_topology(topology);
        let vert_keys: Vec<VertKey> = mesh.verts.keys().collect();
        let mut face_keys: Vec<FaceKey> = vec![FaceKey::default(); mesh.faces.len()];
        for (k, f) in mesh.faces.iter() {
            let slot = f.material_idx as usize;
            if slot < face_keys.len() {
                face_keys[slot] = k;
            }
        }
        Self {
            mesh,
            vert_keys,
            face_keys,
        }
    }
}

/// Apply a half-edge `edit` to `binding`, then reconcile a brush's derived
/// data: recache normals, flatten to topology, resize `faces` to the new
/// polygon count (truncating extras, default-filling new slots), recompute
/// every plane, and re-lift the binding. Returns the edit's result.
///
/// Owns structure only. New faces are default-filled; the caller, which knows
/// which new face derives from which source, propagates appearance afterward
/// via [`BrushFaceData::copy_appearance_from`].
pub fn apply_topology_edit<R>(
    faces: &mut Vec<BrushFaceData>,
    topology: &mut BrushTopology,
    binding: &mut HalfedgeBinding,
    edit: impl FnOnce(&mut HalfedgeMesh) -> R,
) -> R {
    let result = edit(&mut binding.mesh);
    binding.mesh.recache_face_normals();

    let new_topology = binding.mesh.flatten_to_topology();
    let count = new_topology.polygons.len();
    faces.truncate(count);
    faces.resize_with(count, BrushFaceData::default);
    new_topology.recompute_face_planes(faces);
    *topology = new_topology;
    *binding = HalfedgeBinding::lift_from_topology(topology);

    result
}

#[cfg(test)]
mod tests {
    use crate::halfedge::HalfedgeMesh;
    use crate::{compute_brush_topology, cuboid_faces};
    use glam::Vec3;

    #[test]
    fn recache_face_normals_restores_axis_aligned_normals() {
        let topo = compute_brush_topology(&cuboid_faces(Vec3::splat(1.0)));
        let mut mesh = HalfedgeMesh::lift_from_topology(&topo);

        // Scramble every cached normal, then rebuild from the loop rings.
        let keys: Vec<_> = mesh.faces.keys().collect();
        for k in &keys {
            mesh.faces[*k].normal_cache = Vec3::ZERO;
        }
        mesh.recache_face_normals();

        for (_, f) in mesh.faces.iter() {
            assert!((f.normal_cache.length() - 1.0).abs() < 1e-4, "unit length");
            let n = f.normal_cache.abs();
            let axis_aligned = (n - Vec3::X).length() < 1e-3
                || (n - Vec3::Y).length() < 1e-3
                || (n - Vec3::Z).length() < 1e-3;
            assert!(
                axis_aligned,
                "cube face normal axis-aligned, got {:?}",
                f.normal_cache
            );
        }
    }
}
