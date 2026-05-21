//! Face poke: insert a new vertex at a point inside a face, then replace the
//! face with a fan of triangles from the new vertex to each ring edge.
//!
//! Used by the knife operator for face-interior cuts. Only supports fan
//! triangulation; if the input face is concave, the resulting fan triangles
//! may overlap. For convex faces (the common case for brush geometry, where
//! faces are convex polygons by construction) the result is a proper
//! triangulation with N tris for an N-ring face.
//!
//! The new center vertex is connected to every ring vertex by a new edge; the
//! original ring edges are reused.
//!
//! No concave-face support (no ear-cutting). The op is intentionally limited
//! to fan triangulation per the spec; callers must ensure the face is convex
//! if they need non-overlapping output triangles.
//!
//! Material inheritance: every new triangle face inherits the original face's
//! `material_idx`. After `flatten_to_topology`, faces are sorted by
//! `material_idx`, so the N fan triangles end up as adjacent polygon slots.
//!
//! Errors:
//!   - `FaceNotFound`         if `face` is not in the mesh.
//!   - `Degenerate`           if the face has fewer than 3 loops.
//!   - `PointNotInFacePlane`  if `|face.normal_cache.dot(point - centroid)| > 1e-3`.

use bevy_math::Vec3;

use crate::halfedge::cycles::radial_remove_loop;
use crate::halfedge::ops::face_create::create_face_from_verts_with_material;
use crate::halfedge::types::*;

#[derive(Debug)]
pub enum PokeError {
    FaceNotFound,
    Degenerate,
    PointNotInFacePlane,
}

#[derive(Debug)]
pub struct PokeResult {
    /// The new center vertex inserted into the face.
    pub center_vert: VertKey,
    /// The N triangle faces created from the fan triangulation
    /// (one per edge of the original face's ring).
    pub new_faces: Vec<FaceKey>,
    /// The N new edges from the center vert to each original ring vert.
    pub new_edges: Vec<EdgeKey>,
}

/// Plane-distance tolerance for `point` vs. the face plane (world units).
const PLANE_TOLERANCE: f32 = 1e-3;

pub fn face_poke(
    mesh: &mut HalfedgeMesh,
    face: FaceKey,
    point: Vec3,
) -> Result<PokeResult, PokeError> {
    // 1. Look up `face`; bail if missing or below the triangle threshold.
    let face_data = mesh
        .faces
        .get(face)
        .cloned()
        .ok_or(PokeError::FaceNotFound)?;
    let n = face_data.loop_count as usize;
    if n < 3 {
        return Err(PokeError::Degenerate);
    }

    // 2. Walk the face ring to collect its N vertices in order.
    let mut ring: Vec<VertKey> = Vec::with_capacity(n);
    {
        let mut cur = face_data.loop_first;
        for _ in 0..n {
            ring.push(mesh.loops[cur].vert);
            cur = mesh.loops[cur].next;
        }
    }

    // 3. Verify `point` lies in the face plane.
    //    Use centroid as a plane anchor; signed plane distance is
    //    `normal.dot(point - centroid)`.
    let centroid: Vec3 = ring.iter().map(|&v| mesh.verts[v].co).sum::<Vec3>() / n as f32;
    let plane_dist = face_data.normal_cache.dot(point - centroid);
    if plane_dist.abs() > PLANE_TOLERANCE {
        return Err(PokeError::PointNotInFacePlane);
    }

    let original_material_idx = face_data.material_idx;

    // 4. Insert the new center vertex.
    let center_vert = mesh.add_vert(point);

    // 5. Tear down the original face: free its loops (removing them from radial
    //    cycles) and remove the face entry. Ring verts and ring edges remain
    //    in place; they will be reused by the fan triangles below.
    tear_down_face(mesh, face);

    // 6. Build the fan triangles. For each consecutive ring pair
    //    `(ring[i], ring[(i+1) % N])`, create a triangle
    //    `[center_vert, ring[i], ring[(i+1) % N]]`.
    //
    //    `create_face_from_verts_with_material` is idempotent on edge lookup:
    //    the original ring edge `(ring[i], ring[(i+1) % N])` is reused, and
    //    new spoke edges from `center_vert` to each ring vert are created on
    //    first reference and reused on subsequent calls.
    let mut new_faces: Vec<FaceKey> = Vec::with_capacity(n);
    for i in 0..n {
        let tri_verts = [center_vert, ring[i], ring[(i + 1) % n]];
        let tri_face =
            create_face_from_verts_with_material(mesh, &tri_verts, Some(original_material_idx))
                .expect("fan triangle has exactly 3 verts");
        new_faces.push(tri_face);
    }

    // 7. Collect the N new spoke edges `(center_vert, ring[i])`.
    let mut new_edges: Vec<EdgeKey> = Vec::with_capacity(n);
    for &rv in &ring {
        let edge = find_edge_between(mesh, center_vert, rv)
            .expect("spoke edge must exist after fan construction");
        new_edges.push(edge);
    }

    Ok(PokeResult {
        center_vert,
        new_faces,
        new_edges,
    })
}

/// Free `face`'s loops (removing them from radial cycles) and remove the face
/// entry. Edges and verts are NOT touched; callers must reuse them.
fn tear_down_face(mesh: &mut HalfedgeMesh, face: FaceKey) {
    let face_data = mesh.faces[face].clone();
    let n = face_data.loop_count as usize;
    let mut loops: Vec<LoopKey> = Vec::with_capacity(n);
    let mut cur = face_data.loop_first;
    for _ in 0..n {
        loops.push(cur);
        cur = mesh.loops[cur].next;
    }
    for lp in loops {
        radial_remove_loop(mesh, lp);
        mesh.loops.remove(lp);
    }
    mesh.faces.remove(face);
}

/// Linear scan of `mesh.edges` for an edge connecting `va` and `vb`.
fn find_edge_between(mesh: &HalfedgeMesh, va: VertKey, vb: VertKey) -> Option<EdgeKey> {
    mesh.edges
        .iter()
        .find(|(_, e)| (e.v[0] == va && e.v[1] == vb) || (e.v[0] == vb && e.v[1] == va))
        .map(|(k, _)| k)
}
