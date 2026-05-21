//! Retriangulate a face with Steiner points and constraint edges.
//!
//! Used by the knife operator's commit pipeline to turn a clicked polyline
//! path into a proper cut through a face. Given a face plus a list of
//! interior points (Steiner verts) and a list of constraint edges
//! (indices into `ring_verts ++ interior_points`), the op:
//!
//! 1. Adds the interior points as new verts.
//! 2. Projects the combined point set to 2D via the face plane.
//! 3. Runs constrained Delaunay triangulation, using `cdt`. The face ring
//!    is passed as a closed boundary so the flood-fill keeps only
//!    triangles inside the face. Each constraint edge is passed TWICE so
//!    its `toggle_lock_sign` ends up `Some(false)`: a fixed edge that
//!    does NOT count as an inside/outside transition. This makes the
//!    polyline's segments appear as triangle edges without erasing
//!    triangles on either side of the cut.
//! 4. Runs a "tris-to-quads" merge pass that recombines adjacent CDT
//!    triangles into convex quads where possible. Boundary edges (face
//!    ring) and constraint edges (user path) are never merged across;
//!    only edges introduced by CDT are candidates. The merge is greedy:
//!    candidates with the best aspect ratio win first.
//! 5. Tears down the original face and creates one new face per
//!    output polygon (triangle or quad), inheriting the original face's
//!    `material_idx`.
//!
//! Self-intersecting polylines are rejected upstream; this op assumes
//! the caller has validated. CDT failures bubble up as
//! `RetriangulateError::CdtFailed`.
//!
//! Assumptions:
//!   - Face is convex (the brush case). A concave face would still earcut
//!     correctly via cdt, but the face-ring closed-contour pass assumes
//!     no self-intersection in the boundary.
//!   - All constraint edges lie on the face plane (caller projects).
//!   - Interior points are on or near the face plane (caller ensures).

use bevy_math::{Vec2, Vec3};

use crate::compute_face_tangent_axes;
use crate::halfedge::cycles::radial_remove_loop;
use crate::halfedge::ops::face_create::create_face_from_verts_with_material;
use crate::halfedge::types::*;

#[derive(Debug)]
pub enum RetriangulateError {
    FaceNotFound,
    Degenerate,
    InvalidConstraintIndex,
    CdtFailed(String),
}

#[derive(Debug)]
pub struct RetriangulateResult {
    /// New polygon faces created from the retriangulation (a mix of
    /// triangles and quads after the merge pass).
    pub new_faces: Vec<FaceKey>,
    /// New verts added as Steiner points (one per `interior_points` entry,
    /// in the same order).
    pub new_verts: Vec<VertKey>,
}

/// Retriangulate `face` honoring the supplied Steiner points and
/// constraint edges.
///
/// `interior_points`: 3D positions on the face plane. Each is added as a
/// fresh vert in the mesh.
///
/// `constraint_edges`: pairs of indices into the combined list
/// `ring_verts ++ interior_points`. Indices in `0..ring_len` refer to
/// existing ring verts; indices in `ring_len..ring_len+interior_len`
/// refer to the new Steiner verts in `new_verts` order.
///
/// Each output triangle inherits the original face's `material_idx`. The
/// original face is removed; its edges are reused where possible.
pub fn face_retriangulate(
    mesh: &mut HalfedgeMesh,
    face: FaceKey,
    interior_points: Vec<Vec3>,
    constraint_edges: Vec<(usize, usize)>,
) -> Result<RetriangulateResult, RetriangulateError> {
    // 1. Snapshot the face's ring + metadata.
    let face_data = mesh
        .faces
        .get(face)
        .cloned()
        .ok_or(RetriangulateError::FaceNotFound)?;
    let ring_len = face_data.loop_count as usize;
    if ring_len < 3 {
        return Err(RetriangulateError::Degenerate);
    }

    let mut ring_verts: Vec<VertKey> = Vec::with_capacity(ring_len);
    {
        let mut cur = face_data.loop_first;
        for _ in 0..ring_len {
            ring_verts.push(mesh.loops[cur].vert);
            cur = mesh.loops[cur].next;
        }
    }
    let ring_positions: Vec<Vec3> = ring_verts.iter().map(|&v| mesh.verts[v].co).collect();

    let total = ring_len + interior_points.len();
    if total < 3 {
        return Err(RetriangulateError::Degenerate);
    }

    // Validate constraint indices.
    for &(a, b) in &constraint_edges {
        if a >= total || b >= total || a == b {
            return Err(RetriangulateError::InvalidConstraintIndex);
        }
    }

    let original_material_idx = face_data.material_idx;
    let face_flag = face_data.flag;
    let face_normal = face_data.normal_cache;

    // Fast path: no Steiner points + exactly one constraint edge between
    // two non-adjacent ring verts. This is the common "edge-to-edge cut"
    // case after `split_edge` placed both endpoints on the ring. CDT
    // misbehaves on this shape (ring verts read as PointOnFixedEdge
    // against the boundary loop); route straight to `split_face` which
    // is purpose-built for "chord between two ring verts" and produces
    // a clean 2-quad result instead of 4 CDT-triangles.
    if interior_points.is_empty() && constraint_edges.len() == 1 {
        let (a, b) = constraint_edges[0];
        if a < ring_len && b < ring_len {
            let dist = (a as isize - b as isize).rem_euclid(ring_len as isize) as usize;
            let non_adjacent = dist > 1 && dist < ring_len - 1;
            if non_adjacent {
                let va = ring_verts[a];
                let vb = ring_verts[b];
                let _new_edge = super::face_split::split_face(mesh, face, va, vb)
                    .map_err(|e| RetriangulateError::CdtFailed(format!("{:?}", e)))?;
                // After split_face, the original face was replaced by two
                // sub-faces. Find them by their material_idx (preserved by
                // split_face).
                let new_faces: Vec<FaceKey> = mesh
                    .faces
                    .iter()
                    .filter(|(_, f)| f.material_idx == original_material_idx)
                    .map(|(k, _)| k)
                    .collect();
                let _ = face_flag;
                return Ok(RetriangulateResult {
                    new_faces,
                    new_verts: Vec::new(),
                });
            }
        }
    }

    // 2. Project the combined point set to 2D.
    let (u_axis, v_axis) = compute_face_tangent_axes(face_normal);
    // Anchor at ring[0] for numerical stability.
    let origin = ring_positions[0];
    let project = |p: Vec3| -> (f64, f64) {
        let d = p - origin;
        (d.dot(u_axis) as f64, d.dot(v_axis) as f64)
    };

    let mut points_2d: Vec<(f64, f64)> = Vec::with_capacity(total);
    for r in &ring_positions {
        points_2d.push(project(*r));
    }
    for p in &interior_points {
        points_2d.push(project(*p));
    }

    // 3. Add Steiner verts to the mesh now (before tearing down the
    // original face, so a later failure leaves the mesh in a clean state
    // would require rollback; here we accept that on CDT failure the
    // Steiner verts remain orphaned -- callers must validate path
    // upstream).
    let new_verts: Vec<VertKey> = interior_points.iter().map(|&p| mesh.add_vert(p)).collect();

    // 4. Build the edge list for cdt:
    //    - Face ring edges once (closed boundary).
    //    - Constraint edges twice (so flood-fill ignores them).
    let mut cdt_edges: Vec<(usize, usize)> =
        Vec::with_capacity(ring_len + constraint_edges.len() * 2);
    for i in 0..ring_len {
        cdt_edges.push((i, (i + 1) % ring_len));
    }
    for &(a, b) in &constraint_edges {
        cdt_edges.push((a, b));
        cdt_edges.push((a, b));
    }

    // 5. Run constrained Delaunay.
    let triangles = cdt::triangulate_with_edges(&points_2d, &cdt_edges)
        .map_err(|e| RetriangulateError::CdtFailed(format!("{:?}", e)))?;

    if triangles.is_empty() {
        return Err(RetriangulateError::CdtFailed(
            "no triangles produced".to_string(),
        ));
    }

    // Combined vert lookup: index 0..ring_len -> ring_verts; rest ->
    // new_verts.
    let resolve_vert = |idx: usize| -> VertKey {
        if idx < ring_len {
            ring_verts[idx]
        } else {
            new_verts[idx - ring_len]
        }
    };

    // 6. Validate orientation. cdt's output triangle winding may be CW or
    // CCW depending on the 2D projection sign. We ensure each output
    // triangle is CCW relative to the face normal by checking the 2D
    // signed area: positive area means CCW in our (u, v) basis. If
    // negative (CW), swap two verts.
    let to_ccw = |a: usize, b: usize, c: usize| -> (usize, usize, usize) {
        let pa = points_2d[a];
        let pb = points_2d[b];
        let pc = points_2d[c];
        let cross = (pb.0 - pa.0) * (pc.1 - pa.1) - (pb.1 - pa.1) * (pc.0 - pa.0);
        if cross < 0.0 { (a, c, b) } else { (a, b, c) }
    };

    let ccw_triangles: Vec<[usize; 3]> = triangles
        .into_iter()
        .map(|(a, b, c)| {
            let (a, b, c) = to_ccw(a, b, c);
            [a, b, c]
        })
        .collect();

    // 6a. Merge adjacent polygons into larger convex polygons where
    // possible. Multi-pass: first pass produces quads (from tri + tri),
    // subsequent passes extend to pentagons, hexagons, etc. as long as
    // every merge stays convex.
    let polygons = merge_polygons(&ccw_triangles, &points_2d, ring_len, &constraint_edges);

    // 7. Tear down the original face: free its loops (removing them from
    // radial cycles) and remove the face entry. Ring verts and ring
    // edges remain in place; they will be reused by the new triangles
    // below.
    tear_down_face(mesh, face);

    // 8. Build a new face per output polygon (triangle or quad).
    let mut new_faces: Vec<FaceKey> = Vec::with_capacity(polygons.len());
    for poly in polygons {
        let poly_verts: Vec<VertKey> = poly.iter().map(|&i| resolve_vert(i)).collect();
        let new_face =
            create_face_from_verts_with_material(mesh, &poly_verts, Some(original_material_idx))
                .map_err(|_| RetriangulateError::CdtFailed("face create failed".to_string()))?;
        mesh.faces[new_face].flag = face_flag;
        mesh.faces[new_face].normal_cache = face_normal;
        new_faces.push(new_face);
    }

    Ok(RetriangulateResult {
        new_faces,
        new_verts,
    })
}

/// Free `face`'s loops and remove the face entry. Edges and verts are
/// NOT touched; callers must reuse them. Mirrors `face_poke`'s helper of
/// the same name.
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

fn _project_2d(p: Vec3, origin: Vec3, u_axis: Vec3, v_axis: Vec3) -> Vec2 {
    let d = p - origin;
    Vec2::new(d.dot(u_axis), d.dot(v_axis))
}

/// Canonical (min, max) edge tuple.
fn canon_edge(a: usize, b: usize) -> (usize, usize) {
    if a <= b { (a, b) } else { (b, a) }
}

/// Multi-pass polygon merge.
///
/// Walks the CDT output triangle list and recombines adjacent polygons
/// whose shared edge is "internal" (not a boundary ring edge, not a
/// user-supplied constraint edge) into larger convex polygons. Each
/// pass merges as many disjoint pairs as possible; subsequent passes
/// run until no more merges are accepted. The polygon size grows from
/// triangle to quad to pentagon to hexagon, etc., until either every
/// candidate would produce a concave polygon or every internal edge is
/// already merged.
///
/// Per pass:
///
/// 1. Build a map: internal edge -> the two polygons that share it.
/// 2. For each shared internal edge, compute the candidate merged ring.
///    Reject if the merge is concave (any reflex vertex relative to the
///    other corners) or degenerate (any vertex collinear within tolerance
///    along the resulting ring).
/// 3. Score each valid candidate by the resulting polygon's aspect
///    quality (favor square-ish shapes over elongated slivers).
/// 4. Greedily accept the best-scoring candidates in order, with each
///    candidate consuming both source polygons.
/// 5. Append unconsumed polygons unchanged, then repeat the pass.
///
/// Boundary edges (face ring) and constraint edges (user path) are
/// never merged across.
///
/// Returns one `Vec<usize>` per output polygon (length 3 for kept
/// triangles, length 4 for quads, etc.). Vert indices are the same
/// indices used by `points_2d` (`0..ring_len` are ring verts,
/// `ring_len..` are Steiner points).
fn merge_polygons(
    triangles: &[[usize; 3]],
    points_2d: &[(f64, f64)],
    ring_len: usize,
    constraint_edges: &[(usize, usize)],
) -> Vec<Vec<usize>> {
    use std::collections::HashSet;

    let constraint_set: HashSet<(usize, usize)> = constraint_edges
        .iter()
        .map(|&(a, b)| canon_edge(a, b))
        .collect();

    let mut polygons: Vec<Vec<usize>> = triangles.iter().map(|t| t.to_vec()).collect();

    // Safety cap: every pass strictly reduces polygon count, so the loop
    // must terminate after at most `polygons.len()` passes. The bound
    // doubles as a guard against an unexpected fixpoint failure.
    let pass_cap = polygons.len() + 1;
    for _pass in 0..pass_cap {
        let merged_any = merge_pass(&mut polygons, points_2d, ring_len, &constraint_set);
        if !merged_any {
            break;
        }
    }

    polygons
}

/// One merge pass over `polygons`. Returns `true` if any merges were
/// accepted (caller should re-run). Replaces `polygons` in place with
/// the post-pass result (mix of merged polygons + unchanged carryovers).
fn merge_pass(
    polygons: &mut Vec<Vec<usize>>,
    points_2d: &[(f64, f64)],
    ring_len: usize,
    constraint_set: &std::collections::HashSet<(usize, usize)>,
) -> bool {
    use std::collections::HashMap;

    let is_boundary = |a: usize, b: usize| -> bool {
        if a >= ring_len || b >= ring_len {
            return false;
        }
        let diff = a.abs_diff(b);
        diff == 1 || diff == ring_len - 1
    };

    // edge_to_polys: canonical undirected edge -> list of polygon indices
    // that contain that edge. Internal edges have exactly 2 entries.
    let mut edge_to_polys: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for (pi, poly) in polygons.iter().enumerate() {
        let n = poly.len();
        for k in 0..n {
            let a = poly[k];
            let b = poly[(k + 1) % n];
            edge_to_polys.entry(canon_edge(a, b)).or_default().push(pi);
        }
    }

    // pair_share_count: for each unordered polygon pair (a, b) with a < b,
    // count how many shared edges they have. Two polygons sharing more
    // than one edge can NOT be merged: walking the ring would visit a
    // shared vertex twice, producing a degenerate (non-simple)
    // polygon. The classic case is a "lens" arrangement around a hub
    // vertex (e.g., a fan that has already merged into two larger
    // polygons that meet at the hub via two shared edges).
    let mut pair_share_count: HashMap<(usize, usize), usize> = HashMap::new();
    for owners in edge_to_polys.values() {
        if owners.len() != 2 {
            continue;
        }
        let (a, b) = if owners[0] < owners[1] {
            (owners[0], owners[1])
        } else {
            (owners[1], owners[0])
        };
        *pair_share_count.entry((a, b)).or_default() += 1;
    }

    #[derive(Clone)]
    struct Candidate {
        poly_a: usize,
        poly_b: usize,
        merged: Vec<usize>,
        score: f64,
    }

    let mut candidates: Vec<Candidate> = Vec::new();
    for (&edge, owners) in edge_to_polys.iter() {
        if owners.len() != 2 {
            continue;
        }
        if is_boundary(edge.0, edge.1) {
            continue;
        }
        if constraint_set.contains(&edge) {
            continue;
        }
        let pa = owners[0];
        let pb = owners[1];
        // Skip if these two polygons share more than one edge: merging
        // would visit the second shared vertex twice and produce a
        // self-touching ring.
        let key = if pa < pb { (pa, pb) } else { (pb, pa) };
        if pair_share_count.get(&key).copied().unwrap_or(0) != 1 {
            continue;
        }
        let Some(merged) = merge_polys_across_edge(&polygons[pa], &polygons[pb], edge) else {
            continue;
        };
        if !is_polygon_convex(&merged, points_2d) {
            continue;
        }
        let score = polygon_score(&merged, points_2d);
        candidates.push(Candidate {
            poly_a: pa,
            poly_b: pb,
            merged,
            score,
        });
    }

    if candidates.is_empty() {
        return false;
    }

    // Best score first, deterministic tiebreak.
    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.poly_a.min(a.poly_b).cmp(&b.poly_a.min(b.poly_b)))
            .then(a.poly_a.max(a.poly_b).cmp(&b.poly_a.max(b.poly_b)))
    });

    let mut consumed: Vec<bool> = vec![false; polygons.len()];
    let mut merged_polys: Vec<Vec<usize>> = Vec::new();
    let mut accepted_any = false;
    for c in candidates {
        if consumed[c.poly_a] || consumed[c.poly_b] {
            continue;
        }
        consumed[c.poly_a] = true;
        consumed[c.poly_b] = true;
        merged_polys.push(c.merged);
        accepted_any = true;
    }

    if !accepted_any {
        return false;
    }

    // Carry over unconsumed polygons unchanged.
    let mut next: Vec<Vec<usize>> = Vec::with_capacity(merged_polys.len() + polygons.len());
    next.extend(merged_polys);
    for (pi, poly) in polygons.drain(..).enumerate() {
        if !consumed[pi] {
            next.push(poly);
        }
    }
    *polygons = next;
    true
}

/// Merge two CCW polygons across a shared edge. Both polygons must be
/// CCW; `edge = (p, q)` is the canonical shared edge. The result is a
/// CCW ring covering both polygons, with the shared edge removed.
///
/// Topology: in one polygon the edge appears as `p -> q`; in the other
/// it appears as `q -> p` (opposite winding because both faces are CCW
/// on opposite sides of the shared edge). The merged ring walks:
/// `p, [B's verts after p until q], q, [A's verts after q until p]`
/// where `A` is the polygon with the `p -> q` edge and `B` is the one
/// with the `q -> p` edge.
///
/// Returns `None` if neither polygon contains the edge in the expected
/// direction (topology mismatch).
fn merge_polys_across_edge(
    poly_a: &[usize],
    poly_b: &[usize],
    edge: (usize, usize),
) -> Option<Vec<usize>> {
    let (p, q) = edge;
    // Locate `p -> q` in one polygon and `q -> p` in the other; this
    // pairs the polygons on opposite sides of the shared edge.
    let na = poly_a.len();
    let nb = poly_b.len();
    let pq_in_a = (0..na).find(|&i| poly_a[i] == p && poly_a[(i + 1) % na] == q);
    let qp_in_a = (0..na).find(|&i| poly_a[i] == q && poly_a[(i + 1) % na] == p);
    let pq_in_b = (0..nb).find(|&i| poly_b[i] == p && poly_b[(i + 1) % nb] == q);
    let qp_in_b = (0..nb).find(|&i| poly_b[i] == q && poly_b[(i + 1) % nb] == p);

    // `forward` polygon has `p -> q`; `backward` has `q -> p`.
    let (fwd, fwd_i, back, back_j) = match (pq_in_a, qp_in_b, pq_in_b, qp_in_a) {
        (Some(i), Some(j), _, _) => (poly_a, i, poly_b, j),
        (_, _, Some(i), Some(j)) => (poly_b, i, poly_a, j),
        _ => return None,
    };
    let n_fwd = fwd.len();
    let n_back = back.len();

    // Walk `back` from after-p back around to q (skipping the shared
    // edge), then `fwd` from after-q back around to p.
    let mut merged: Vec<usize> = Vec::with_capacity(n_fwd + n_back - 2);
    merged.push(p);
    // back[back_j] == q, back[(back_j + 1) % n_back] == p. Start at
    // (back_j + 2) % n_back and walk until back_j (= q) exclusive.
    let mut k = (back_j + 2) % n_back;
    while k != back_j {
        merged.push(back[k]);
        k = (k + 1) % n_back;
    }
    merged.push(q);
    // fwd[fwd_i] == p, fwd[(fwd_i + 1) % n_fwd] == q. Start at
    // (fwd_i + 2) % n_fwd and walk until fwd_i (= p) exclusive.
    let mut k = (fwd_i + 2) % n_fwd;
    while k != fwd_i {
        merged.push(fwd[k]);
        k = (k + 1) % n_fwd;
    }
    Some(merged)
}

/// Returns `true` if the polygon's ring is strictly convex (interior
/// angle at every vertex is below 180 degrees by at least the implicit
/// tolerance).
///
/// Method: walk the ring, compute the signed cross product at each
/// vertex. All non-trivial crosses must share the same sign (no reflex
/// angle). Crosses with magnitude below a small absolute epsilon are
/// treated as "near straight" and allowed (a 179.x degree interior
/// angle is still visually convex). A polygon with any clearly negative
/// cross (when the rest are positive, or vice versa) is concave.
fn is_polygon_convex(poly: &[usize], points_2d: &[(f64, f64)]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    // Absolute tolerance for "near straight" angle. The cross magnitude
    // is roughly edge_len^2 * sin(angle); for ~0.5 degree off-straight
    // on unit-length edges, cross ~= 0.0087. We use 1e-9 to clearly
    // separate "numerically zero collinear" from "0.5 degree concave",
    // matching the EPSILON ~ 0.5 degree spec.
    let eps = 1e-9;
    let cross_at = |i: usize| -> f64 {
        let pa = points_2d[poly[(i + n - 1) % n]];
        let pb = points_2d[poly[i]];
        let pc = points_2d[poly[(i + 1) % n]];
        (pb.0 - pa.0) * (pc.1 - pb.1) - (pb.1 - pa.1) * (pc.0 - pb.0)
    };
    let mut has_pos = false;
    let mut has_neg = false;
    for i in 0..n {
        let c = cross_at(i);
        if c > eps {
            has_pos = true;
        } else if c < -eps {
            has_neg = true;
        }
        if has_pos && has_neg {
            return false;
        }
    }
    // At least one non-near-zero corner; if all corners are zero the
    // polygon is degenerate (zero area).
    has_pos || has_neg
}

/// Score a candidate polygon: ratio of shortest side to longest side
/// (in `[0, 1]`). 1.0 is regular; lower values are more elongated.
/// Picking the highest-scoring candidates first biases the greedy merge
/// toward "less stretched" polygons rather than long thin slivers.
fn polygon_score(poly: &[usize], points_2d: &[(f64, f64)]) -> f64 {
    let n = poly.len();
    if n < 3 {
        return 0.0;
    }
    let mut max_e = 0.0f64;
    let mut min_e = f64::INFINITY;
    for i in 0..n {
        let a = points_2d[poly[i]];
        let b = points_2d[poly[(i + 1) % n]];
        let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        if len > max_e {
            max_e = len;
        }
        if len < min_e {
            min_e = len;
        }
    }
    if max_e <= 0.0 {
        return 0.0;
    }
    min_e / max_e
}
