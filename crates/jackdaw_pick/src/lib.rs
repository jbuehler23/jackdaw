//! Engine-agnostic picking queries.
//!
//! These functions answer "which face does this ray hit" and "is this point
//! inside this face" from plain brush geometry: vertex positions, per-face
//! polygon rings, and face planes. They are pure math over [`glam`] vectors
//! and [`jackdaw_geometry`] plane data, with no engine dependency, so the same
//! picking drives any host. The editor unwraps its ray type into an
//! origin-and-direction pair at the call boundary and feeds in the brush cache;
//! a different host does the same with its own ray.
//!
//! The kernel's [`jackdaw_geometry::ray_plane_intersection`] primitive does the
//! ray-versus-plane step; this crate builds the nearest-face and containment
//! queries on top of it.

use glam::{Vec2, Vec3};
use jackdaw_geometry::{BrushPlane, ray_plane_intersection};

/// One face of a brush, as picking sees it: the polygon ring (indices into a
/// shared vertex array, in winding order) plus the face's supporting plane.
///
/// This is the engine-free view of a face the editor already has in its mesh
/// cache (a `face_polygons` entry paired with the authored face plane); a
/// different host supplies the same two pieces.
pub struct Face<'a> {
    /// Vertex indices forming this face's boundary ring, in winding order.
    pub ring: &'a [usize],
    /// The face's supporting plane (normal and signed distance from origin).
    pub plane: BrushPlane,
}

/// Find the brush face a ray hits, nearest first.
///
/// For each face the ray is intersected with the face's supporting plane; among
/// the forward hits, the nearest along the ray is kept, provided it lies inside
/// the solid (inside every face half-space, matching the convex-solid surface).
/// `vertices` and the face planes must be expressed in the same space as the
/// ray. `dir` is assumed normalized so the returned ordering is by true
/// distance; it need not be for correctness of the nearest selection.
///
/// Returns the hit face's index into `faces` together with the hit point, or
/// `None` if the ray misses every face. Faces with fewer than three ring
/// vertices are skipped.
pub fn face_from_ray(
    origin: Vec3,
    dir: Vec3,
    vertices: &[Vec3],
    faces: &[Face<'_>],
) -> Option<(usize, Vec3)> {
    let mut best_t = f32::MAX;
    let mut best: Option<(usize, Vec3)> = None;

    for (face_idx, face) in faces.iter().enumerate() {
        if face.ring.len() < 3 {
            continue;
        }
        let normal = face.plane.normal;
        let centroid = ring_centroid(vertices, face.ring);
        let Some(hit) = ray_plane_intersection(origin, dir, centroid, normal) else {
            continue;
        };
        // With a unit-length direction this dot is the distance along the ray.
        let t = (hit - origin).dot(dir);
        if t < best_t && point_inside_all_planes(hit, faces) {
            best_t = t;
            best = Some((face_idx, hit));
        }
    }

    best
}

/// Test whether a point lies inside a planar face ring.
///
/// The ring and the test point are projected onto the plane spanned by the
/// polygon (with `normal` as the plane normal), then a 2D ray-cast decides
/// containment. A point off the plane projects onto it first, so a point on the
/// plane but beyond an edge reads as outside and a point past an edge in any
/// direction reads as outside. Returns `false` for a degenerate ring (fewer
/// than three vertices) or a zero normal.
pub fn point_in_polygon(point: Vec3, ring: &[Vec3], normal: Vec3) -> bool {
    if ring.len() < 3 {
        return false;
    }
    let n = normal.normalize_or_zero();
    if n == Vec3::ZERO {
        return false;
    }
    // A 2D basis in the face plane.
    let u_seed = if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let u = (u_seed - n * u_seed.dot(n)).normalize_or_zero();
    if u == Vec3::ZERO {
        return false;
    }
    let v = n.cross(u);
    let origin = ring[0];
    let to_2d = |p: Vec3| -> Vec2 {
        let d = p - origin;
        Vec2::new(d.dot(u), d.dot(v))
    };
    let ring_2d: Vec<Vec2> = ring.iter().map(|&p| to_2d(p)).collect();
    point_in_polygon_2d(to_2d(point), &ring_2d)
}

/// Centroid of a face ring: the mean of its vertex positions.
fn ring_centroid(vertices: &[Vec3], ring: &[usize]) -> Vec3 {
    let sum: Vec3 = ring.iter().map(|&vi| vertices[vi]).sum();
    sum / ring.len() as f32
}

/// Whether `point` lies on the solid side of every face plane (inside the
/// convex half-space intersection), within a small tolerance.
fn point_inside_all_planes(point: Vec3, faces: &[Face<'_>]) -> bool {
    const EPSILON: f32 = 1e-4;
    for face in faces {
        if face.plane.normal.dot(point) > face.plane.distance + EPSILON {
            return false;
        }
    }
    true
}

/// Even-odd ray-cast point-in-polygon test in 2D. Handles convex and concave
/// rings; returns `false` for fewer than three vertices.
fn point_in_polygon_2d(point: Vec2, polygon: &[Vec2]) -> bool {
    let n = polygon.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let pi = polygon[i];
        let pj = polygon[j];
        if ((pi.y > point.y) != (pj.y > point.y))
            && (point.x < (pj.x - pi.x) * (point.y - pi.y) / (pj.y - pi.y) + pi.x)
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;
    use jackdaw_geometry::{BrushTopology, compute_brush_topology, cuboid_faces};

    /// A unit cuboid centered at the origin, in the shape picking consumes:
    /// vertex positions plus per-face rings (indices into those positions) and
    /// the face planes. Built the way the editor builds its mesh cache, from
    /// `cuboid_faces` -> `compute_brush_topology`. 8 verts, 6 quad faces with
    /// axis-aligned planes at +/-0.5 on each axis.
    struct Cuboid {
        vertices: Vec<Vec3>,
        rings: Vec<Vec<usize>>,
        planes: Vec<BrushPlane>,
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
        let planes: Vec<BrushPlane> = faces.iter().map(|f| f.plane.clone()).collect();
        Cuboid {
            vertices,
            rings,
            planes,
        }
    }

    impl Cuboid {
        fn faces(&self) -> Vec<Face<'_>> {
            self.rings
                .iter()
                .zip(self.planes.iter())
                .map(|(ring, plane)| Face {
                    ring,
                    plane: plane.clone(),
                })
                .collect()
        }

        /// The index of the face whose plane normal matches `normal`.
        fn face_index_with_normal(&self, normal: Vec3) -> usize {
            self.planes
                .iter()
                .position(|p| p.normal.dot(normal) > 0.99)
                .expect("cuboid has a face with this normal")
        }
    }

    #[test]
    fn cuboid_fixture_has_expected_counts() {
        let c = cuboid();
        assert_eq!(c.vertices.len(), 8);
        assert_eq!(c.rings.len(), 6);
        assert_eq!(c.planes.len(), 6);
    }

    #[test]
    fn downward_ray_hits_the_top_face() {
        let c = cuboid();
        // A ray starting above the cube, pointing down, hits the +Y (top) face
        // at y ~= +0.5.
        let (face_idx, hit) = face_from_ray(
            Vec3::new(0.0, 5.0, 0.0),
            Vec3::NEG_Y,
            &c.vertices,
            &c.faces(),
        )
        .expect("a downward ray hits the cube");
        assert_eq!(face_idx, c.face_index_with_normal(Vec3::Y), "top face");
        assert!(
            (hit.y - 0.5).abs() < 1e-4,
            "hit near the top plane: {hit:?}"
        );
        assert!(
            hit.x.abs() < 1e-4 && hit.z.abs() < 1e-4,
            "hit centered: {hit:?}"
        );
    }

    #[test]
    fn ray_chooses_the_nearest_of_two_candidate_faces() {
        let c = cuboid();
        // A ray along -X from far +X passes through both the +X and -X faces.
        // The nearest is the +X (near) face at x ~= +0.5, not the far -X face.
        let (face_idx, hit) = face_from_ray(
            Vec3::new(5.0, 0.0, 0.0),
            Vec3::NEG_X,
            &c.vertices,
            &c.faces(),
        )
        .expect("a side-on ray hits the cube");
        assert_eq!(
            face_idx,
            c.face_index_with_normal(Vec3::X),
            "nearest is the +X face"
        );
        assert!((hit.x - 0.5).abs() < 1e-4, "hit on the near plane: {hit:?}");
    }

    #[test]
    fn ray_that_misses_returns_none() {
        let c = cuboid();
        // A downward ray well outside the cube's x-extent misses every face's
        // solid surface.
        let result = face_from_ray(
            Vec3::new(5.0, 5.0, 0.0),
            Vec3::NEG_Y,
            &c.vertices,
            &c.faces(),
        );
        assert!(
            result.is_none(),
            "ray outside the footprint misses: {result:?}"
        );
    }

    #[test]
    fn point_at_face_center_is_inside() {
        let c = cuboid();
        // Center of the top face: on its plane, well within the ring.
        let top = c.face_index_with_normal(Vec3::Y);
        let ring: Vec<Vec3> = c.rings[top].iter().map(|&i| c.vertices[i]).collect();
        assert!(point_in_polygon(Vec3::new(0.0, 0.5, 0.0), &ring, Vec3::Y));
    }

    #[test]
    fn point_outside_the_ring_is_outside() {
        let c = cuboid();
        // A point on the top plane but far past the +X edge of the top face.
        let top = c.face_index_with_normal(Vec3::Y);
        let ring: Vec<Vec3> = c.rings[top].iter().map(|&i| c.vertices[i]).collect();
        assert!(!point_in_polygon(Vec3::new(5.0, 0.5, 0.0), &ring, Vec3::Y));
    }

    #[test]
    fn point_on_plane_beyond_an_edge_is_outside() {
        let c = cuboid();
        // Just outside the +X edge of the top face (face spans x in [-0.5, 0.5]).
        let top = c.face_index_with_normal(Vec3::Y);
        let ring: Vec<Vec3> = c.rings[top].iter().map(|&i| c.vertices[i]).collect();
        assert!(!point_in_polygon(Vec3::new(0.51, 0.5, 0.0), &ring, Vec3::Y));
        // And just inside that edge is contained.
        assert!(point_in_polygon(Vec3::new(0.49, 0.5, 0.0), &ring, Vec3::Y));
    }
}
