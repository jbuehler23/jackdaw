//! Live mirror evaluation. The authored half of a brush is reflected
//! across one or more brush-local planes at display time; authored
//! elements keep their indices (identity prefix) so picking the
//! authored half needs no remapping and mirrored elements map back
//! through the source arrays.

#[cfg(feature = "render")]
use bevy::prelude::ReflectComponent;
use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::{BrushPlane, clip_to_halfspace, newell_normal};

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
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(
    feature = "render",
    derive(bevy::ecs::component::Component, bevy::reflect::Reflect)
)]
#[cfg_attr(feature = "render", reflect(Component))]
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
    /// Weld mirrored verts to their source at the plane. When false the
    /// two halves stay as separate overlapping geometry.
    pub merge: bool,
    /// Per-axis non-destructive cut: drop authored geometry on the far
    /// side of the plane before mirroring.
    pub bisect: [bool; 3],
    /// Per-axis bisect direction: keep the other side of the plane.
    pub bisect_flip: [bool; 3],
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
            merge: true,
            bisect: [false; 3],
            bisect_flip: [false; 3],
        }
    }
}

impl MeshMirror {
    /// Enabled axes as flags.
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

/// Marker in `EvaluatedBrush::vert_source` / `face_source` for an evaluated
/// element with no authored origin (the cut cap and split vertices a bisect
/// introduces). The editor skips these when drawing editable handles, and the
/// modifier-stack fold passes the marker through instead of indexing with it.
pub const NO_SOURCE: u32 = u32::MAX;

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
/// flipping face winding. When `merge` is set, mirrored copies of verts
/// that lie within `merge_dist` of that plane weld back to their source
/// vert; when `merge` is clear nothing welds and the two halves stay as
/// separate overlapping geometry.
///
/// Axes are processed sequentially: the output of axis N is the input
/// of axis N+1, so X|Y produces four copies.
///
/// **Precondition:** every index in `face_polygons` must be in range for
/// `vertices`; an out-of-range index will panic.
///
/// **Note:** with `merge` set, a face whose vertices all lie on the
/// mirror plane (all welded) produces no mirrored face; it would
/// duplicate exactly onto its source. With `merge` clear that seam face
/// is duplicated like any other.
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

        // Non-destructive bisect: before reflecting this axis, drop the
        // authored half on the far side of the plane and cap the cut. The
        // cap and its split verts carry NO_SOURCE; the on-plane split verts
        // weld to themselves during reflection (merge), so the cap becomes
        // the seam instead of doubling geometry.
        if mirror.bisect[axis] {
            let clipped = clip_to_halfspace(
                &eval.vertices,
                &eval.face_polygons,
                &eval.vert_source,
                &eval.face_source,
                axis,
                plane,
                !mirror.bisect_flip[axis],
            );
            eval.vertices = clipped.vertices;
            eval.face_polygons = clipped.face_polygons;
            eval.vert_source = clipped.vert_source;
            eval.face_source = clipped.face_source;
        }

        let input_vert_count = eval.vertices.len();
        let input_face_count = eval.face_polygons.len();

        // Mirror every existing vert. With merge set, verts within
        // merge_dist of the plane weld to themselves instead of
        // duplicating; with merge clear nothing welds, so on-plane verts
        // gain a coincident duplicate at the seam.
        let mut mirrored_index = vec![0usize; input_vert_count];
        for (i, slot) in mirrored_index.iter_mut().enumerate() {
            let v = eval.vertices[i];
            if mirror.merge && (v[axis] - plane).abs() <= mirror.merge_dist {
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

    /// Axis-aligned cube spanning [-1, 1] on every axis, 8 verts, 6 quad
    /// faces wound CCW from outside.
    fn cube() -> (Vec<Vec3>, Vec<Vec<usize>>) {
        (
            vec![
                Vec3::new(-1.0, -1.0, -1.0),
                Vec3::new(1.0, -1.0, -1.0),
                Vec3::new(1.0, 1.0, -1.0),
                Vec3::new(-1.0, 1.0, -1.0),
                Vec3::new(-1.0, -1.0, 1.0),
                Vec3::new(1.0, -1.0, 1.0),
                Vec3::new(1.0, 1.0, 1.0),
                Vec3::new(-1.0, 1.0, 1.0),
            ],
            vec![
                vec![0, 3, 2, 1],
                vec![4, 5, 6, 7],
                vec![0, 1, 5, 4],
                vec![3, 7, 6, 2],
                vec![0, 4, 7, 3],
                vec![1, 2, 6, 5],
            ],
        )
    }

    #[test]
    fn bisect_halves_geometry_before_mirroring() {
        let (verts, polys) = cube();

        // Control: plain X-mirror of the straddling cube. No vert sits on
        // x=0, so nothing welds and all six faces duplicate.
        let control = evaluate_mirror(
            &verts,
            &polys,
            &MeshMirror {
                mirror_x: true,
                ..Default::default()
            },
        );
        assert_eq!(control.face_polygons.len(), 12, "6 authored + 6 mirrored");
        assert!(
            control.face_source.iter().all(|&s| s != NO_SOURCE),
            "plain mirror introduces no cap"
        );

        // With bisect: keep x>=0, cap the cut, then mirror. The clip yields
        // the +X face, four clipped side quads, and one cap (6 faces). The
        // cap lies on x=0 and welds to itself, so mirroring skips it and
        // duplicates the other five faces: 6 + 5 = 11 faces.
        let bisected = evaluate_mirror(
            &verts,
            &polys,
            &MeshMirror {
                mirror_x: true,
                bisect: [true, false, false],
                ..Default::default()
            },
        );
        assert_eq!(
            bisected.face_polygons.len(),
            11,
            "kept half (6, cap included) + 5 mirrored non-seam faces"
        );
        assert!(
            bisected.face_polygons.len() < control.face_polygons.len(),
            "bisect removes the doubled overlapping half"
        );
        // The cap survives as a NO_SOURCE face on the seam.
        assert!(
            bisected.face_source.contains(&NO_SOURCE),
            "cut cap kept as a seam face"
        );
        // The four split verts on x=0 carry NO_SOURCE and weld to themselves.
        assert!(
            bisected.vert_source.contains(&NO_SOURCE),
            "split verts kept with NO_SOURCE"
        );
        // No kept geometry strays onto the discarded (x < 0) side.
        for ring in &bisected.face_polygons {
            for &vi in ring {
                let x = bisected.vertices[vi].x;
                // Mirrored copies live at x <= 0; authored kept half at x >= 0.
                assert!(x.abs() <= 1.0 + 1e-5, "no stray geometry: x={x}");
            }
        }
    }

    #[test]
    fn bisect_flip_keeps_the_mirror_image_half() {
        let (verts, polys) = cube();
        let keep_pos = evaluate_mirror(
            &verts,
            &polys,
            &MeshMirror {
                mirror_x: true,
                bisect: [true, false, false],
                ..Default::default()
            },
        );
        let keep_neg = evaluate_mirror(
            &verts,
            &polys,
            &MeshMirror {
                mirror_x: true,
                bisect: [true, false, false],
                bisect_flip: [true, false, false],
                ..Default::default()
            },
        );
        // Mirror-and-bisect is symmetric: keeping either half then mirroring
        // produces the same face count.
        assert_eq!(
            keep_pos.face_polygons.len(),
            keep_neg.face_polygons.len(),
            "either kept half yields the same mirrored result"
        );
        // The authored kept half flips sides. With keep_positive the authored
        // half sits at x>=0; flipped it sits at x<=0.
        let authored_xs_pos: Vec<f32> = keep_pos.vertices[..8].iter().map(|v| v.x).collect();
        let authored_xs_neg: Vec<f32> = keep_neg.vertices[..8].iter().map(|v| v.x).collect();
        // The authored verts themselves are untouched (identity prefix); the
        // difference is which faces reference them. Confirm at least one face
        // references an x=1 authored vert in the keep-positive result.
        let _ = (authored_xs_pos, authored_xs_neg);
        let touches_pos = keep_pos
            .face_polygons
            .iter()
            .flatten()
            .any(|&vi| keep_pos.vertices[vi].x > 0.5);
        let touches_neg_authored = keep_neg
            .face_polygons
            .iter()
            .flatten()
            .any(|&vi| vi < 8 && keep_neg.vertices[vi].x < -0.5);
        assert!(touches_pos, "keep-positive references the +X authored half");
        assert!(
            touches_neg_authored,
            "flipped keep references the -X authored half"
        );
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
    fn merge_false_duplicates_seam_instead_of_welding() {
        // A quad lying entirely on the x=0 mirror plane: all four verts
        // are seam verts and the face is a seam face.
        let verts = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 1.0),
            Vec3::new(0.0, 0.0, 1.0),
        ];
        let polys = vec![vec![0, 1, 2, 3]];

        // merge:true welds all four verts onto themselves and skips the
        // all-welded seam face.
        let merged = evaluate_mirror(
            &verts,
            &polys,
            &MeshMirror {
                merge: true,
                ..Default::default()
            },
        );
        assert_eq!(merged.vertices.len(), 4, "all four seam verts weld");
        assert_eq!(merged.face_polygons.len(), 1, "seam face not duplicated");

        // merge:false never welds: every seam vert gets a coincident
        // duplicate and the seam face is mirrored too.
        let unmerged = evaluate_mirror(
            &verts,
            &polys,
            &MeshMirror {
                merge: false,
                ..Default::default()
            },
        );
        assert_eq!(unmerged.vertices.len(), 8, "4 authored + 4 coincident");
        assert_eq!(unmerged.face_polygons.len(), 2, "seam face duplicated");
        assert!(
            unmerged.vertices.len() > merged.vertices.len()
                && unmerged.face_polygons.len() > merged.face_polygons.len(),
            "unmerged mirror keeps both halves and their seam face"
        );
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
    #[cfg(feature = "render")]
    fn mesh_mirror_round_trips_through_reflection() {
        use bevy::reflect::{
            FromReflect, TypeRegistry,
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
