//! Live mirror evaluation. The authored half of a brush is reflected
//! across one or more brush-local planes at display time; authored
//! elements keep their indices (identity prefix) so picking the
//! authored half needs no remapping and mirrored elements map back
//! through the source arrays.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{BrushPlane, newell_normal};

bitflags::bitflags! {
    /// Which brush-local axes mirror. Combinations compose (X|Y mirrors
    /// into four quadrants).
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct MirrorAxes: u8 {
        const X = 1 << 0;
        const Y = 1 << 1;
        const Z = 1 << 2;
    }
}

impl Serialize for MirrorAxes {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.bits().serialize(s)
    }
}

impl<'de> Deserialize<'de> for MirrorAxes {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(MirrorAxes::from_bits_truncate(u8::deserialize(d)?))
    }
}

/// Live mirror settings for a brush. The plane for each enabled axis
/// passes through `offset` perpendicular to that brush-local axis.
///
/// Three bools so the fields reflect, serialize, and render as
/// inspector checkboxes.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component)]
pub struct MeshMirror {
    pub mirror_x: bool,
    pub mirror_y: bool,
    pub mirror_z: bool,
    /// Plane point in brush-local space.
    pub offset: Vec3,
    /// Pin near-plane verts to the plane and stop others crossing it
    /// during transforms.
    pub clip: bool,
    /// Mirrored copies of verts within this distance of the plane weld
    /// to their source vert, closing the center seam. Uses `<=`
    /// comparison: a vert welds when
    /// `|v[axis] - offset[axis]| <= merge_dist`. At `merge_dist = 0.0`
    /// only exact-plane verts weld.
    pub merge_dist: f32,
}

impl Default for MeshMirror {
    fn default() -> Self {
        Self {
            mirror_x: true,
            mirror_y: false,
            mirror_z: false,
            offset: Vec3::ZERO,
            clip: true,
            merge_dist: 0.001,
        }
    }
}

impl MeshMirror {
    /// Enabled axes as flags; `mirror_x/y/z` stay plain bools so they
    /// reflect, serialize, and render as inspector checkboxes.
    pub fn axes(&self) -> MirrorAxes {
        let mut a = MirrorAxes::empty();
        if self.mirror_x {
            a |= MirrorAxes::X;
        }
        if self.mirror_y {
            a |= MirrorAxes::Y;
        }
        if self.mirror_z {
            a |= MirrorAxes::Z;
        }
        a
    }
}

/// Mirror-evaluated geometry. Indices `0..authored_len` are the
/// authored elements unchanged (identity prefix); appended elements
/// map back through the source arrays.
#[derive(Debug)]
pub struct EvaluatedBrush {
    pub vertices: Vec<Vec3>,
    pub face_polygons: Vec<Vec<usize>>,
    /// Evaluated face index -> authored face index.
    pub face_source: Vec<u32>,
    /// Evaluated vertex index -> authored vertex index.
    pub vert_source: Vec<u32>,
}

/// Reflect the authored geometry across each enabled axis plane,
/// flipping face winding and welding mirrored copies of verts that lie
/// within `merge_dist` of that plane back to their source vert.
///
/// Axes are processed sequentially: the output of axis N is the input
/// of axis N+1, so X|Y produces four copies.
///
/// **Precondition:** every index in `face_polygons` must be in range for
/// `vertices`; an out-of-range index will panic.
///
/// **Note:** a face whose vertices all lie on the mirror plane (all welded)
/// produces no mirrored face; it would duplicate exactly onto its source.
pub fn evaluate_mirror(
    vertices: &[Vec3],
    face_polygons: &[Vec<usize>],
    mirror: &MeshMirror,
) -> EvaluatedBrush {
    let axes = mirror.axes();
    let mut eval = EvaluatedBrush {
        vertices: vertices.to_vec(),
        face_polygons: face_polygons.to_vec(),
        face_source: (0..face_polygons.len() as u32).collect(),
        vert_source: (0..vertices.len() as u32).collect(),
    };

    for (bit, axis) in [
        (MirrorAxes::X, 0usize),
        (MirrorAxes::Y, 1),
        (MirrorAxes::Z, 2),
    ] {
        if !axes.contains(bit) {
            continue;
        }
        let plane = mirror.offset[axis];
        let input_vert_count = eval.vertices.len();
        let input_face_count = eval.face_polygons.len();

        // Mirror every existing vert; verts within merge_dist of the
        // plane weld to themselves instead of duplicating.
        let mut mirrored_index = vec![0usize; input_vert_count];
        for (i, slot) in mirrored_index.iter_mut().enumerate() {
            let v = eval.vertices[i];
            if (v[axis] - plane).abs() <= mirror.merge_dist {
                *slot = i;
            } else {
                let mut m = v;
                m[axis] = 2.0 * plane - v[axis];
                *slot = eval.vertices.len();
                eval.vertices.push(m);
                let src = eval.vert_source[i];
                eval.vert_source.push(src);
            }
        }

        // Mirror every existing face with reversed winding. A face
        // whose verts ALL welded would duplicate exactly onto its
        // source (a face lying in the plane); skip those.
        for f in 0..input_face_count {
            let ring = &eval.face_polygons[f];
            if ring.iter().all(|&vi| mirrored_index[vi] == vi) {
                continue;
            }
            let mirrored_ring: Vec<usize> =
                ring.iter().rev().map(|&vi| mirrored_index[vi]).collect();
            let src = eval.face_source[f];
            eval.face_polygons.push(mirrored_ring);
            eval.face_source.push(src);
        }
    }

    eval
}

/// Plane of an evaluated face: Newell normal over the ring, distance from
/// the first ring vertex. Mirrored faces carry their authored source's
/// `BrushFaceData` whose plane normal is un-reflected; building meshes
/// from it triangulates and shades those faces inside out, so builders
/// replace the cloned plane with this one. Returns `None` for degenerate
/// rings (fewer than 3 verts or zero area); callers keep the authored
/// plane.
pub fn reflected_face_plane(vertices: &[Vec3], ring: &[usize]) -> Option<BrushPlane> {
    if ring.len() < 3 {
        return None;
    }
    let positions: Vec<Vec3> = ring.iter().map(|&vi| vertices[vi]).collect();
    let normal = newell_normal(&positions);
    if normal == Vec3::ZERO {
        return None;
    }
    Some(BrushPlane {
        normal,
        distance: positions[0].dot(normal),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Open half-quad straddling nothing: a single square face on the
    /// +X side with two verts ON the X plane.
    fn half_quad() -> (Vec<Vec3>, Vec<Vec<usize>>) {
        (
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(1.0, 1.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
            ],
            vec![vec![0, 1, 2, 3]],
        )
    }

    #[test]
    fn identity_prefix_holds() {
        let (verts, polys) = half_quad();
        let eval = evaluate_mirror(&verts, &polys, &MeshMirror::default());
        assert_eq!(&eval.vertices[..4], &verts[..]);
        assert_eq!(eval.face_polygons[0], polys[0]);
        assert_eq!(&eval.vert_source[..4], &[0, 1, 2, 3]);
        assert_eq!(eval.face_source[0], 0);
    }

    #[test]
    fn x_mirror_welds_plane_verts_and_flips_winding() {
        let (verts, polys) = half_quad();
        let eval = evaluate_mirror(&verts, &polys, &MeshMirror::default());
        // Verts 0 and 1 sit on the plane: welded, not duplicated.
        assert_eq!(eval.vertices.len(), 6, "4 authored + 2 mirrored off-plane");
        assert_eq!(eval.face_polygons.len(), 2);
        // Mirrored verts have negated X.
        assert_eq!(eval.vertices[4].x, -1.0);
        assert_eq!(eval.vertices[5].x, -1.0);
        // Mirrored face maps to authored face 0 and reuses welded verts.
        assert_eq!(eval.face_source[1], 0);
        let m = &eval.face_polygons[1];
        assert!(
            m.contains(&0) && m.contains(&1),
            "welded plane verts reused"
        );
        // Winding flipped: the mirrored ring traverses in reverse
        // orientation. Verify via the polygon normal (Newell) flipping
        // its X-free axes consistently: compute signed area normals of
        // both faces and assert their Z components have the same sign
        // (a reflected ring with reversed order keeps facing the same
        // world direction for a plane-orthogonal quad).
        let normal = |ring: &Vec<usize>, vs: &Vec<Vec3>| {
            let mut n = Vec3::ZERO;
            for i in 0..ring.len() {
                let a = vs[ring[i]];
                let b = vs[ring[(i + 1) % ring.len()]];
                n += a.cross(b);
            }
            n
        };
        let n0 = normal(&eval.face_polygons[0], &eval.vertices);
        let n1 = normal(&eval.face_polygons[1], &eval.vertices);
        assert!(
            n0.z * n1.z > 0.0,
            "mirrored face must keep outward orientation: {n0:?} vs {n1:?}"
        );
    }

    #[test]
    fn offset_moves_the_plane() {
        let (verts, polys) = half_quad();
        let mirror = MeshMirror {
            offset: Vec3::new(1.0, 0.0, 0.0),
            ..Default::default()
        };
        let eval = evaluate_mirror(&verts, &polys, &mirror);
        // Plane at x=1: verts 2 and 3 weld; verts 0/1 mirror to x=2.
        assert_eq!(eval.vertices.len(), 6);
        assert!(eval.vertices[4..].iter().all(|v| (v.x - 2.0).abs() < 1e-6));
    }

    #[test]
    fn two_axes_compose_to_four_copies() {
        // One triangle fully off both planes.
        let verts = vec![
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(2.0, 1.0, 0.0),
            Vec3::new(1.0, 2.0, 0.0),
        ];
        let polys = vec![vec![0, 1, 2]];
        let mirror = MeshMirror {
            mirror_x: true,
            mirror_y: true,
            ..Default::default()
        };
        let eval = evaluate_mirror(&verts, &polys, &mirror);
        assert_eq!(eval.face_polygons.len(), 4);
        assert_eq!(eval.vertices.len(), 12);
        // Every appended face maps to the single authored face.
        assert!(eval.face_source.iter().all(|&f| f == 0));
    }

    #[test]
    fn zero_merge_dist_never_welds() {
        // Verts at x=1e-9 are NOT on the plane (x=0.0): with merge_dist=0.0
        // the <= comparison gives 1e-9 <= 0.0 = false, so no welding.
        let verts = vec![
            Vec3::new(1e-9, 0.0, 0.0),
            Vec3::new(1e-9, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
        ];
        let polys = vec![vec![0, 1, 2, 3]];
        let mirror = MeshMirror {
            merge_dist: 0.0,
            ..Default::default()
        };
        let eval = evaluate_mirror(&verts, &polys, &mirror);
        assert_eq!(eval.vertices.len(), 8, "no welding at zero tolerance");
    }

    #[test]
    fn reflected_face_plane_flips_a_mirrored_cap() {
        // A +X cap quad at x=1, fully off the default x=0 mirror plane.
        let verts = vec![
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(1.0, 0.0, 1.0),
        ];
        let polys = vec![vec![0, 1, 2, 3]];
        assert_eq!(
            reflected_face_plane(&verts, &polys[0])
                .expect("planar quad")
                .normal,
            Vec3::X
        );

        let eval = evaluate_mirror(&verts, &polys, &MeshMirror::default());
        assert_eq!(eval.face_polygons.len(), 2);
        let plane = reflected_face_plane(&eval.vertices, &eval.face_polygons[1])
            .expect("mirrored quad is planar");
        assert!(
            plane.normal.distance(Vec3::NEG_X) < 1e-6,
            "mirrored cap must face -X, got {:?}",
            plane.normal
        );
        // Plane x = -1 with normal -X: n.dot(p) = 1.
        assert!((plane.distance - 1.0).abs() < 1e-6);
    }

    #[test]
    fn reflected_face_plane_rejects_degenerate_rings() {
        let verts = vec![Vec3::ZERO, Vec3::X, Vec3::new(2.0, 0.0, 0.0)];
        assert!(
            reflected_face_plane(&verts, &[0, 1]).is_none(),
            "too few verts"
        );
        assert!(
            reflected_face_plane(&verts, &[0, 1, 2]).is_none(),
            "collinear ring has zero area"
        );
    }

    #[test]
    fn mesh_mirror_round_trips_through_reflection() {
        use bevy::reflect::{
            TypeRegistry,
            serde::{TypedReflectDeserializer, TypedReflectSerializer},
        };
        use serde::de::DeserializeSeed;

        let mut registry = TypeRegistry::default();
        registry.register::<MeshMirror>();
        registry.register::<Vec3>();

        let original = MeshMirror {
            mirror_y: true,
            offset: Vec3::new(0.5, 0.0, 0.0),
            ..Default::default()
        };
        let serializer = TypedReflectSerializer::new(&original, &registry);
        let json = serde_json::to_string(&serializer).expect("serialize");

        let registration = registry
            .get(std::any::TypeId::of::<MeshMirror>())
            .expect("MeshMirror registered");
        let mut de = serde_json::Deserializer::from_str(&json);
        let reflected = TypedReflectDeserializer::new(registration, &registry)
            .deserialize(&mut de)
            .expect("deserialize");
        let back = MeshMirror::from_reflect(reflected.as_partial_reflect()).expect("from_reflect");
        assert_eq!(back, original);
        assert!(back.mirror_x && back.mirror_y && !back.mirror_z);
    }
}
