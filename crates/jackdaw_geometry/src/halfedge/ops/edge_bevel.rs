//! Edge bevel: chamfer each selected edge into a quad, or round it into a
//! strip of quads with more segments.
//!
//! For each selected edge `e` with endpoints `v0`, `v1` and two adjacent quad
//! (`loop_count` >= 4) faces `fA`, `fB`:
//!   - Replace `v0` with two vertices `v0_A` and `v0_B`, offset along the
//!     parallel-edge direction in `fA` and `fB` by `width`. Same for `v1`.
//!   - The original edge `e` is removed; in its place a new chamfer quad
//!     `(v0_A, v0_B, v1_B, v1_A)` is created, plus two new rail edges
//!     `(v0_A, v1_A)` (in `fA`) and `(v0_B, v1_B)` (in `fB`).
//!   - Faces `fA` and `fB` (and the third face at each endpoint, for the
//!     standard cube-corner topology) are rebuilt with the new vertices and
//!     edges.
//!
//! With `segments` > 1 the chamfer becomes a strip of quads whose inner rails
//! follow a quarter-ellipse tangent to both faces (a circular arc on a square
//! corner), and each end cap gains the in-between vertices.
//!
//! Limitations:
//!   - The profile shape is fixed; only the segment count varies.
//!   - Vertex bevel is a separate op.
//!   - The in-between vertices only reach end caps at degree-3 endpoints.
//!   - If two beveled edges share a vertex, they are processed independently;
//!     overlapping bevels at that shared vert may produce garbage but will not
//!     crash.
//!   - Endpoints with degree != 3 are not specially handled. The
//!     delete-and-recreate path will still rebuild the faces it touches, but
//!     the resulting topology at high-degree (or degree-2) verts may be
//!     non-manifold. The cube case (every vert degree 3) is fully supported.

use std::collections::HashSet;

use crate::halfedge::cycles::{
    disk_remove_edge, disk_walk, edge_between, radial_remove_loop, radial_walk,
};
use crate::halfedge::ops::face_create::create_face_from_verts_with_material;
use crate::halfedge::types::*;
use crate::newell::newell_normal;

#[derive(Debug)]
pub enum BevelError {
    EmptyInput,
    WidthTooSmall,
    /// Reserved for callers that want to know an edge was rejected. The
    /// current implementation skips non-manifold edges silently and never
    /// returns this variant from the public entry point.
    NonManifoldEdge,
    /// More than one segment at an endpoint that does not meet exactly three
    /// edges, where the rounded profile has no end cap to close it.
    ProfileWithoutCap,
    /// A face could not be rebuilt; the mesh is left as it was.
    RebuildFailed,
}

#[derive(Debug)]
pub struct BevelResult {
    /// The chamfer quad faces created, one per beveled edge.
    pub new_faces: Vec<FaceKey>,
    /// The new rail edges, two per beveled edge: `(v0_A, v1_A)` and
    /// `(v0_B, v1_B)`. Each pair appears consecutively in the order edges
    /// were processed.
    pub new_edges: Vec<EdgeKey>,
}

/// Bevel each edge in `edges` into a quad of width `width`.
///
/// Returns `EmptyInput` if `edges` is empty, `WidthTooSmall` if `width < 1e-6`.
/// Per-edge non-manifold (not exactly 2 adjacent faces with `loop_count` >= 4)
/// is silently skipped; the result reflects whatever edges did succeed. A
/// face that cannot be rebuilt fails the whole call with `RebuildFailed` and
/// leaves `mesh` untouched.
pub fn edge_bevel(
    mesh: &mut HalfedgeMesh,
    edges: &[EdgeKey],
    width: f32,
) -> Result<BevelResult, BevelError> {
    edge_bevel_segments(mesh, edges, width, 1)
}

/// [`edge_bevel`] with `segments` quads across each chamfer. `segments` below
/// 1 counts as 1. The rail edges in the result run from one face side to the
/// other, two outer rails then the inner ones. More than one segment needs
/// every endpoint of a bevelable edge to meet exactly three edges, else
/// `ProfileWithoutCap` and `mesh` untouched.
pub fn edge_bevel_segments(
    mesh: &mut HalfedgeMesh,
    edges: &[EdgeKey],
    width: f32,
    segments: u32,
) -> Result<BevelResult, BevelError> {
    if edges.is_empty() {
        return Err(BevelError::EmptyInput);
    }
    if width < 1e-6 {
        return Err(BevelError::WidthTooSmall);
    }

    let mut new_faces: Vec<FaceKey> = Vec::new();
    let mut new_edges: Vec<EdgeKey> = Vec::new();

    let segments = segments.max(1);
    if segments > 1
        && edges
            .iter()
            .filter(|&&edge| bevelable(mesh, edge))
            .flat_map(|&edge| mesh.edges[edge].v)
            .any(|vert| disk_walk(mesh, vert).count() != 3)
    {
        return Err(BevelError::ProfileWithoutCap);
    }
    let mut work = mesh.clone();
    for &edge_key in edges {
        bevel_one_edge(
            &mut work,
            edge_key,
            width,
            segments,
            &mut new_faces,
            &mut new_edges,
        )?;
    }
    *mesh = work;

    Ok(BevelResult {
        new_faces,
        new_edges,
    })
}

/// Whether `edge` has exactly two adjacent faces of four or more sides.
fn bevelable(mesh: &HalfedgeMesh, edge: EdgeKey) -> bool {
    if !mesh.edges.contains_key(edge) {
        return false;
    }
    let radial: Vec<LoopKey> = radial_walk(mesh, edge).collect();
    radial.len() == 2
        && radial
            .iter()
            .all(|&lp| mesh.faces[mesh.loops[lp].face].loop_count >= 4)
}

/// Process a single edge. Skips an edge it cannot bevel; fails when a face
/// cannot be rebuilt, leaving `mesh` half-edited for the caller to discard.
fn bevel_one_edge(
    mesh: &mut HalfedgeMesh,
    edge_key: EdgeKey,
    width: f32,
    segments: u32,
    new_faces: &mut Vec<FaceKey>,
    new_edges: &mut Vec<EdgeKey>,
) -> Result<(), BevelError> {
    if !bevelable(mesh, edge_key) {
        return Ok(());
    }
    let radial: Vec<LoopKey> = radial_walk(mesh, edge_key).collect();
    let l_a = radial[0];
    let l_b = radial[1];
    let face_a = mesh.loops[l_a].face;
    let face_b = mesh.loops[l_b].face;

    let v0 = mesh.edges[edge_key].v[0];
    let v1 = mesh.edges[edge_key].v[1];

    // Capture the original adjacent face normals before any teardown. The
    // chamfer face's outward direction is the average of these two normals;
    // we'll use it later to verify the chamfer winding produces a Newell
    // normal that faces outward, reversing the ring if not.
    let outward_expected = {
        let n_a = mesh.faces[face_a].normal_cache;
        let n_b = mesh.faces[face_b].normal_cache;
        (n_a + n_b).normalize_or_zero()
    };

    // 2. Find each face's loop at v0 and at v1 (could be the radial loop itself
    //    or its `next`, depending on which way `edge_key` walks in that face).
    // We only need the loops at v0 in each face: the in-face perpendicular
    // computed below is valid for both endpoints of the selected edge in that
    // face (the walk direction on `edge_key` is fixed per face, so the perp is
    // fixed too).
    let l_a_v0 = loop_at_vert(mesh, l_a, v0);
    let l_b_v0 = loop_at_vert(mesh, l_b, v0);

    // 3. Compute the in-face perpendicular at the selected edge for each
    //    adjacent face. This is "perpendicular to the selected edge, in the
    //    face's plane, pointing into the face from the edge". Both endpoints
    //    of the selected edge use the SAME perpendicular within a given face,
    //    so the new chamfer's rail in that face is parallel to the original
    //    edge. For axis-aligned cube faces this matches "walk the parallel
    //    edge"; for chamfer-adjacent faces it produces a clean parallelogram
    //    even when the adjacent face is slanted (chained bevels).
    let v0_pos = mesh.verts[v0].co;
    let v1_pos = mesh.verts[v1].co;
    let walk_a = walk_dir_along_edge_in_face(mesh, l_a_v0, v0_pos, v1_pos, edge_key);
    let walk_b = walk_dir_along_edge_in_face(mesh, l_b_v0, v0_pos, v1_pos, edge_key);
    let perp_a = mesh.faces[face_a]
        .normal_cache
        .cross(walk_a)
        .normalize_or_zero();
    let perp_b = mesh.faces[face_b]
        .normal_cache
        .cross(walk_b)
        .normalize_or_zero();
    if perp_a.length_squared() < 1e-12 || perp_b.length_squared() < 1e-12 {
        return Ok(());
    }
    let dir_a_v0 = perp_a;
    let dir_a_v1 = perp_a;
    let dir_b_v0 = perp_b;
    let dir_b_v1 = perp_b;

    // 5. Allocate the 4 offset verts.
    let v0_a = mesh.add_vert(v0_pos + dir_a_v0 * width);
    let v0_b = mesh.add_vert(v0_pos + dir_b_v0 * width);
    let v1_a = mesh.add_vert(v1_pos + dir_a_v1 * width);
    let v1_b = mesh.add_vert(v1_pos + dir_b_v1 * width);
    let mids_0 = profile_verts(mesh, v0_pos, v0_a, v0_b, segments);
    let mids_1 = profile_verts(mesh, v1_pos, v1_a, v1_b, segments);

    // 6. Collect every face incident at v0 or v1. These are the faces whose
    //    rings reference v0 or v1 and therefore need rebuilding (or
    //    substitution). For the typical degree-3 endpoint, that's fA, fB, plus
    //    one "third" face per endpoint (fC at v0, fD at v1; the same face if
    //    v0 and v1 happen to share an extra face, which a manifold cube does
    //    not).
    let mut affected: HashSet<FaceKey> = HashSet::new();
    collect_faces_at_vert(mesh, v0, &mut affected);
    collect_faces_at_vert(mesh, v1, &mut affected);

    // 7. Build the new vert ring for each affected face by walking its old
    //    ring and applying the substitutions:
    //      - In fA: v0 -> v0_A, v1 -> v1_A.
    //      - In fB: v0 -> v0_B, v1 -> v1_B.
    //      - In any other face touching v0: substitute v0 with [v0_A, v0_B]
    //        (or just one of them, or no change) based on which of its two
    //        ring-adjacent edges at v0 are the parallel edges.
    //      - Likewise for v1.
    let mut rebuilds: Vec<(Vec<VertKey>, u32)> = Vec::with_capacity(affected.len());
    for &fk in &affected {
        let ring = build_new_ring(
            mesh,
            fk,
            [face_a, face_b],
            [v0, v1],
            [v0_a, v0_b, v1_a, v1_b],
            [&mids_0, &mids_1],
        );
        let mat = mesh.faces[fk].material_idx;
        rebuilds.push((ring, mat));
    }

    // 8. Tear down the affected faces (loops + face entries) and the beveled
    //    edge `e` itself. Edges other than `e` are left in place; they will be
    //    looked up (or rebuilt with new endpoints) when the new faces are
    //    created. The original vertices v0 and v1 are also removed: any edge
    //    still referencing them is purged from the disk + slotmap so that
    //    `create_face_from_verts_with_material` below doesn't accidentally
    //    reuse a stale edge whose endpoint is gone.
    for &fk in &affected {
        tear_down_face(mesh, fk);
    }

    disk_remove_edge(mesh, edge_key);
    mesh.edges.remove(edge_key);

    // Remove any remaining edges at v0 / v1; they'll be re-created with the
    // offset verts as endpoints. (For the cube case, the parallel edges
    // `e_*_par_v*` are the only ones left at v0 / v1 by this point.)
    purge_edges_at_vert(mesh, v0);
    purge_edges_at_vert(mesh, v1);
    if mesh.verts.contains_key(v0) {
        mesh.verts.remove(v0);
    }
    if mesh.verts.contains_key(v1) {
        mesh.verts.remove(v1);
    }

    // 9. Re-create every affected face from the new rings. Each face is
    //    recreated with its original material_idx so the post-flatten slot
    //    order is preserved.
    for (ring, mat) in &rebuilds {
        if ring.len() < 3 {
            continue;
        }
        let face = create_face_from_verts_with_material(mesh, ring, Some(*mat))
            .map_err(|_| BevelError::RebuildFailed)?;
        let positions: Vec<glam::Vec3> = ring.iter().map(|&k| mesh.verts[k].co).collect();
        mesh.faces[face].normal_cache = newell_normal(&positions);
    }

    // 10. Create the chamfer quad. Winding `(v0_A, v0_B, v1_B, v1_A)` is the
    //     CCW order viewed from outside (i.e. from the side opposite to where
    //     the original edge was buried). The chamfer's edges:
    //        (v0_A, v0_B): shared with fC's rebuilt ring (degree-3 case) or
    //                      with whatever face received the v0-side end cap.
    //        (v0_B, v1_B): the fB-side rail edge.
    //        (v1_B, v1_A): shared with fD's rebuilt ring at v1.
    //        (v1_A, v0_A): the fA-side rail edge.
    //     `create_face_from_verts_with_material` is idempotent on edge lookup,
    //     so any edge already created by the rebuilds in step 9 will be
    //     reused; only the 0-2 edges that don't yet exist get inserted.
    // Pick winding so each strip quad's Newell normal faces outward (matching
    // the average of the two adjacent faces' normals). Without this check the
    // ring can end up wound clockwise viewed from outside, the face renders as
    // a backface, and the chamfer looks dark / inverted.
    let side_0: Vec<VertKey> = std::iter::once(v0_a)
        .chain(mids_0.iter().copied())
        .chain(std::iter::once(v0_b))
        .collect();
    let side_1: Vec<VertKey> = std::iter::once(v1_a)
        .chain(mids_1.iter().copied())
        .chain(std::iter::once(v1_b))
        .collect();
    for k in 0..side_0.len() - 1 {
        let candidate_ring: [VertKey; 4] = [side_0[k], side_0[k + 1], side_1[k + 1], side_1[k]];
        let candidate_positions: Vec<glam::Vec3> =
            candidate_ring.iter().map(|&v| mesh.verts[v].co).collect();
        let candidate_normal = newell_normal(&candidate_positions);
        let strip_ring: [VertKey; 4] = if outward_expected.length_squared() > 1e-6
            && candidate_normal.dot(outward_expected) < 0.0
        {
            [side_0[k], side_1[k], side_1[k + 1], side_0[k + 1]]
        } else {
            candidate_ring
        };
        let strip_mat = next_material_idx(mesh);
        let strip_face = create_face_from_verts_with_material(mesh, &strip_ring, Some(strip_mat))
            .map_err(|_| BevelError::RebuildFailed)?;
        let positions: Vec<glam::Vec3> = strip_ring.iter().map(|&v| mesh.verts[v].co).collect();
        mesh.faces[strip_face].normal_cache = newell_normal(&positions);
        new_faces.push(strip_face);
    }

    let rails = [(v0_a, v1_a), (v0_b, v1_b)]
        .into_iter()
        .chain(mids_0.iter().copied().zip(mids_1.iter().copied()));
    new_edges.extend(rails.filter_map(|(a, b)| edge_between(mesh, a, b)));
    Ok(())
}

/// The `segments - 1` in-between vertices of the rounded profile from `a` to
/// `b` around the corner at `corner`, in order from `a`. The profile is the
/// quarter-ellipse through `a` and `b` tangent to both faces, centred on the
/// corner's opposite in the parallelogram `a`, `corner`, `b`.
fn profile_verts(
    mesh: &mut HalfedgeMesh,
    corner: glam::Vec3,
    a: VertKey,
    b: VertKey,
    segments: u32,
) -> Vec<VertKey> {
    let a_pos = mesh.verts[a].co;
    let b_pos = mesh.verts[b].co;
    let centre = a_pos + b_pos - corner;
    (1..segments)
        .map(|k| {
            let angle = std::f32::consts::FRAC_PI_2 * k as f32 / segments as f32;
            let pos = centre + (a_pos - centre) * angle.cos() + (b_pos - centre) * angle.sin();
            mesh.add_vert(pos)
        })
        .collect()
}

/// Walk direction along the beveled edge `e` in the face containing `lp_at_v0`.
/// Returns a vector pointing along `e` in the order the face's loop traverses
/// it, i.e. v0 -> v1 if the loop continues from v0 along e, or v1 -> v0 if
/// the loop arrived at v0 via e. The returned vector is NOT normalized.
///
/// Used to compute the in-face perpendicular at the selected edge via
/// `face_normal x walk_dir`, which gives a direction perpendicular to `e`,
/// lying in the face plane, pointing into the face from the edge (assuming
/// CCW winding viewed from the face normal).
fn walk_dir_along_edge_in_face(
    mesh: &HalfedgeMesh,
    lp_at_v0: LoopKey,
    v0_pos: glam::Vec3,
    v1_pos: glam::Vec3,
    e: EdgeKey,
) -> glam::Vec3 {
    if mesh.loops[lp_at_v0].edge == e {
        // The face's walk leaves v0 along e, heading toward v1.
        v1_pos - v0_pos
    } else {
        // The face's walk arrived at v0 via e from v1, so the walk direction
        // along e (in the face's winding order) is v1 -> v0.
        v0_pos - v1_pos
    }
}

/// Given a loop on the beveled edge, return the loop in the same face whose
/// `.vert` equals `target`. The radial loop either already starts at `target`,
/// or its `.next` does.
fn loop_at_vert(mesh: &HalfedgeMesh, loop_on_e: LoopKey, target: VertKey) -> LoopKey {
    if mesh.loops[loop_on_e].vert == target {
        loop_on_e
    } else {
        debug_assert_eq!(mesh.loops[mesh.loops[loop_on_e].next].vert, target);
        mesh.loops[loop_on_e].next
    }
}

/// Insert every face incident at `v` into `out`. Walks the disk cycle of `v`
/// and the radial cycle of every incident edge.
fn collect_faces_at_vert(mesh: &HalfedgeMesh, v: VertKey, out: &mut HashSet<FaceKey>) {
    for e in disk_walk(mesh, v).collect::<Vec<_>>() {
        for lp in radial_walk(mesh, e).collect::<Vec<_>>() {
            out.insert(mesh.loops[lp].face);
        }
    }
}

/// Build the new vert ring for `face` given the bevel substitutions. Walks the
/// existing ring; each non-{v0, v1} vert is kept as-is, each v0 is replaced
/// per the face-specific rule (a single offset vert, two offset verts, or no
/// change), and likewise for v1.
///
/// The offset vertices come as `[v0_a, v0_b, v1_a, v1_b]` and `mids` holds the
/// profile vertices at `v0` and at `v1`, each ordered from the A offset to the B.
fn build_new_ring(
    mesh: &HalfedgeMesh,
    face: FaceKey,
    [face_a, face_b]: [FaceKey; 2],
    [v0, v1]: [VertKey; 2],
    [v0_a, v0_b, v1_a, v1_b]: [VertKey; 4],
    mids: [&[VertKey]; 2],
) -> Vec<VertKey> {
    let face_data = &mesh.faces[face];
    let n = face_data.loop_count as usize;
    let mut ring: Vec<VertKey> = Vec::with_capacity(n + 2);

    let mut cur = face_data.loop_first;
    for _ in 0..n {
        let lp = &mesh.loops[cur];
        let v = lp.vert;
        if v != v0 && v != v1 {
            ring.push(v);
            cur = lp.next;
            continue;
        }

        // Identify which endpoint we're at, and which adjacent ring edges
        // anchor this loop.
        let endpoint = if v == v0 { Endpoint::V0 } else { Endpoint::V1 };
        let prev_edge = mesh.loops[lp.prev].edge;
        let next_edge = lp.edge;

        if face == face_a {
            // In fA: the loop walks across the beveled edge. Replace v with v_A.
            ring.push(endpoint.pick(v0_a, v1_a));
        } else if face == face_b {
            // In fB: same idea, with v_B.
            ring.push(endpoint.pick(v0_b, v1_b));
        } else {
            // In any other face at v: substitute based on which ring edges
            // here are the parallel edges in fA / fB. The parallel edge in fA
            // at this endpoint is identified by being on `face_a`'s radial
            // cycle (alongside the loop in fA we already located); we can
            // detect it locally by walking each candidate edge's radial cycle
            // and checking whether `face_a` (or `face_b`) appears there.
            let prev_par_a = edge_touches_face(mesh, prev_edge, face_a);
            let prev_par_b = edge_touches_face(mesh, prev_edge, face_b);
            let next_par_a = edge_touches_face(mesh, next_edge, face_a);
            let next_par_b = edge_touches_face(mesh, next_edge, face_b);

            let (offset_a, offset_b) = match endpoint {
                Endpoint::V0 => (v0_a, v0_b),
                Endpoint::V1 => (v1_a, v1_b),
            };

            // Build the substitution sequence for this v-slot. The "prev"
            // side maps to `offset_X` if the prev_edge is the parallel in
            // face_X; otherwise the original v stays for that side. Same for
            // "next".
            let left = if prev_par_a {
                Some(offset_a)
            } else if prev_par_b {
                Some(offset_b)
            } else {
                None
            };
            let right = if next_par_a {
                Some(offset_a)
            } else if next_par_b {
                Some(offset_b)
            } else {
                None
            };

            match (left, right) {
                (None, None) => {
                    // Neither adjacent edge is a parallel of fA/fB. Keep v.
                    ring.push(v);
                }
                (Some(l), None) => {
                    // prev edge is parallel: the previous ring vert is now
                    // connected via the relabeled parallel edge to `l`. v
                    // itself stays in the ring (connected to next_edge, which
                    // is unchanged).
                    ring.push(l);
                    ring.push(v);
                }
                (None, Some(r)) => {
                    // next edge is parallel.
                    ring.push(v);
                    ring.push(r);
                }
                (Some(l), Some(r)) => {
                    // Both adjacent edges are parallels (degree-3 cube-corner
                    // case): v is fully replaced by [l, profile, r].
                    ring.push(l);
                    if l != r {
                        let profile = endpoint.pick(mids[0], mids[1]);
                        if l == offset_a {
                            ring.extend(profile.iter().copied());
                        } else {
                            ring.extend(profile.iter().rev().copied());
                        }
                        ring.push(r);
                    }
                }
            }
        }

        cur = lp.next;
    }

    ring
}

/// Returns true iff some loop in `e`'s radial cycle belongs to `face`.
fn edge_touches_face(mesh: &HalfedgeMesh, e: EdgeKey, face: FaceKey) -> bool {
    for lp in radial_walk(mesh, e) {
        if mesh.loops[lp].face == face {
            return true;
        }
    }
    false
}

/// Free `face`'s loops (removing them from radial cycles) and remove the face
/// entry. Edges are NOT touched here; they remain in the slotmap and disk
/// cycles for downstream cleanup.
fn tear_down_face(mesh: &mut HalfedgeMesh, face: FaceKey) {
    if !mesh.faces.contains_key(face) {
        return;
    }
    let face_data = mesh.faces[face].clone();
    let n = face_data.loop_count as usize;
    let mut cur = face_data.loop_first;
    let mut loops: Vec<LoopKey> = Vec::with_capacity(n);
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

/// Remove every remaining edge incident at `v` (after faces have been torn
/// down). Used to scrub orphaned edges that would otherwise leak when v is
/// deleted, AND to make sure `create_face_from_verts_with_material` doesn't
/// reuse a stale edge whose endpoint is about to vanish.
fn purge_edges_at_vert(mesh: &mut HalfedgeMesh, v: VertKey) {
    if !mesh.verts.contains_key(v) {
        return;
    }
    let incident: Vec<EdgeKey> = disk_walk(mesh, v).collect();
    for e in incident {
        disk_remove_edge(mesh, e);
        mesh.edges.remove(e);
    }
}

/// Compute the next `material_idx` for a newly created face: one above
/// the current maximum. Same convention as
/// `create_face_from_verts_with_material` when called with `None`,
/// but lets the caller hold onto the chosen index for later lookup.
fn next_material_idx(mesh: &HalfedgeMesh) -> u32 {
    mesh.faces
        .values()
        .map(|f| f.material_idx)
        .max()
        .map_or(0, |m| m + 1)
}

#[derive(Clone, Copy)]
enum Endpoint {
    V0,
    V1,
}

impl Endpoint {
    fn pick<T>(self, a: T, b: T) -> T {
        match self {
            Endpoint::V0 => a,
            Endpoint::V1 => b,
        }
    }
}
