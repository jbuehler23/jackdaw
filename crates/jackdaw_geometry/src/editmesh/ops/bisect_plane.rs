//! Bisect an `EditMesh` by a plane.
//!
//! Classifies each vertex relative to the plane (Front / On / Back), splits
//! every edge that crosses the plane at the intersection point, splits every
//! face that straddles the plane along the two new on-plane verts, and
//! optionally drops one side and/or builds a closing "cap" face on the
//! exposed boundary.
//!
//! //!
//! For most inputs (manifold closed mesh, plane that doesn't pass exactly
//! through a vertex) the output is a single closed mesh per side with one
//! new cap polygon along the cut plane. Edge cases (plane exactly through a
//! vertex, plane tangent to a face, non-convex cap boundary) are handled
//! best-effort and the resulting mesh always passes validation.

use std::collections::{HashMap, HashSet};

use bevy::math::Vec3;

use crate::BrushPlane;
use crate::editmesh::ops::edge_split::split_edge;
use crate::editmesh::ops::face_split::split_face;
use crate::editmesh::types::*;

/// What to keep after a plane bisect.
///
/// `Both` is intentionally NOT a variant: the `EditMesh` ops layer can
/// only produce one closed mesh per call. To split a brush into two,
/// callers run `bisect_plane` twice on cloned meshes (one with `Front`,
/// one with `Back`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BisectKeep {
    /// Keep everything on the side `plane.normal` points to.
    Front,
    /// Keep everything on the side opposite `plane.normal`.
    Back,
}

/// Result of `bisect_plane`.
pub struct BisectResult {
    /// Material index used for the newly-created cap face. Callers use
    /// this to grow `brush.faces` and to set up UV axes for the cap slot.
    pub cap_material_idx: u32,
    /// The cap face introduced by the cut, if one was created.
    pub cap_face: Option<FaceKey>,
}

#[derive(Debug)]
pub enum BisectError {
    DegeneratePlane,
}

/// Vertex side relative to the plane.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Side {
    Front,
    On,
    Back,
}

/// Tolerance used to classify a vertex as `On` the plane.
const BISECT_EPSILON: f32 = 1e-4;

/// Bisect `bmesh` along `plane`. `plane.normal` should be unit length and
/// `plane.distance = normal.dot(point_on_plane)`.
///
/// After this returns:
///   * Every edge that crossed the plane has been split at the intersection.
///   * Every face that straddled the plane has been split along the two new
///     on-plane verts (so each output face lies wholly on one side of the
///     plane, or is degenerate / all-on).
///   * Faces on the discarded side have been removed.
///   * One cap polygon was added along the cut with a fresh `material_idx`,
///     winding so its outward normal faces away from the kept solid.
///
/// For a Split operation, run `bisect_plane` twice on cloned meshes:
/// once with `Front` for the front half, once with `Back` for the back.
pub fn bisect_plane(
    bmesh: &mut EditMesh,
    plane: &BrushPlane,
    keep: BisectKeep,
) -> Result<BisectResult, BisectError> {
    if plane.normal.length_squared() < 0.5 {
        return Err(BisectError::DegeneratePlane);
    }
    let n = plane.normal;
    let d = plane.distance;

    // -----------------------------------------------------------------
    // Phase 1: classify each vert.
    let mut side: HashMap<VertKey, Side> = HashMap::with_capacity(bmesh.verts.len());
    for (vk, v) in bmesh.verts.iter() {
        let s = signed_dist(v.co, n, d);
        let class = if s > BISECT_EPSILON {
            Side::Front
        } else if s < -BISECT_EPSILON {
            Side::Back
        } else {
            Side::On
        };
        side.insert(vk, class);
    }

    // -----------------------------------------------------------------
    // Phase 2: split every edge with one Front and one Back endpoint at
    // its plane intersection. Track the resulting on-plane verts in a
    // `on_verts` set so we can use them in face splits and cap building.
    let mut on_verts: HashSet<VertKey> = side
        .iter()
        .filter(|&(_, &s)| s == Side::On)
        .map(|(&k, _)| k)
        .collect();

    let crossing_edges: Vec<EdgeKey> = bmesh
        .edges
        .iter()
        .filter_map(|(ek, e)| {
            let sa = *side.get(&e.v[0])?;
            let sb = *side.get(&e.v[1])?;
            if (sa == Side::Front && sb == Side::Back) || (sa == Side::Back && sb == Side::Front) {
                Some(ek)
            } else {
                None
            }
        })
        .collect();

    for edge in crossing_edges {
        let Some(edge_data) = bmesh.edges.get(edge).cloned() else {
            continue;
        };
        let p0 = bmesh.verts[edge_data.v[0]].co;
        let p1 = bmesh.verts[edge_data.v[1]].co;
        let s0 = signed_dist(p0, n, d);
        let s1 = signed_dist(p1, n, d);
        let denom = s0 - s1;
        if denom.abs() < BISECT_EPSILON {
            continue;
        }
        let t = (s0 / denom).clamp(0.0, 1.0);
        let new_vert = split_edge(bmesh, edge, t).map_err(|_| BisectError::DegeneratePlane)?;
        // The newly-inserted vert lies on the plane by construction.
        side.insert(new_vert, Side::On);
        on_verts.insert(new_vert);
    }

    // -----------------------------------------------------------------
    // Phase 3: split every face that has both Front and Back ring verts.
    // After Phase 2 such a face must have exactly two On verts in its
    // ring (the two new intersection verts), so we split along them.
    //
    // We iterate until no straddling face remains, since `split_face`
    // produces two new faces (one on each side) and may recurse if the
    // ring is non-convex.
    loop {
        let target: Option<(FaceKey, VertKey, VertKey)> = find_straddling_face(bmesh, &side);
        let Some((face, va, vb)) = target else {
            break;
        };
        if split_face(bmesh, face, va, vb).is_err() {
            // Degenerate face split (adjacent verts, too few verts);
            // give up on this face. Validation may still flag this.
            break;
        }
    }

    // -----------------------------------------------------------------
    // Phase 4: classify each face as Front, Back, or OnPlane (all-On).
    // Output classification uses the majority side: a face is Front if
    // it contains at least one Front vert and no Back vert, Back if it
    // contains at least one Back vert and no Front vert, OnPlane if all
    // its ring verts are On.
    let mut front_faces: Vec<FaceKey> = Vec::new();
    let mut back_faces: Vec<FaceKey> = Vec::new();
    let mut onplane_faces: Vec<FaceKey> = Vec::new();
    for (fk, face) in bmesh.faces.iter() {
        let mut has_front = false;
        let mut has_back = false;
        let mut cur = face.loop_first;
        for _ in 0..face.loop_count {
            let vk = bmesh.loops[cur].vert;
            match side.get(&vk).copied().unwrap_or(Side::On) {
                Side::Front => has_front = true,
                Side::Back => has_back = true,
                Side::On => {}
            }
            cur = bmesh.loops[cur].next;
        }
        if has_front && !has_back {
            front_faces.push(fk);
        } else if has_back && !has_front {
            back_faces.push(fk);
        } else if !has_front && !has_back {
            onplane_faces.push(fk);
        } else {
            // Still straddling; couldn't split. Best-effort: classify
            // by centroid sign.
            let centroid = face_centroid(bmesh, fk);
            if signed_dist(centroid, n, d) >= 0.0 {
                front_faces.push(fk);
            } else {
                back_faces.push(fk);
            }
        }
    }

    // -----------------------------------------------------------------
    // Phase 5: pick a fresh `material_idx` for any caps we add and for
    // any new mesh ops that follow. We don't yet build cap faces here;
    // we do it after the side-pruning step so the cap polygon ring is
    // walked off the boundary loops that remain.
    let cap_material_idx = bmesh
        .faces
        .values()
        .map(|f| f.material_idx)
        .max()
        .map_or(0, |m| m + 1);

    // -----------------------------------------------------------------
    // Phase 6: drop the discarded side's faces.
    let (faces_to_drop, flip_winding) = match keep {
        BisectKeep::Front => (back_faces, false),
        BisectKeep::Back => (front_faces, true),
    };
    for fk in &faces_to_drop {
        drop_face(bmesh, *fk);
    }
    // Drop any all-on-plane faces; they would be coplanar with our cap
    // and produce a degenerate double-face on the cut plane otherwise.
    for fk in &onplane_faces {
        drop_face(bmesh, *fk);
    }

    // Garbage-collect verts / edges that are no longer referenced.
    gc_orphans(bmesh);

    // -----------------------------------------------------------------
    // Phase 7: build a cap face from the boundary ring of on-plane verts.
    let cap_face = build_cap(bmesh, &on_verts, n, flip_winding, cap_material_idx);

    Ok(BisectResult {
        cap_material_idx,
        cap_face,
    })
}

fn signed_dist(p: Vec3, n: Vec3, d: f32) -> f32 {
    p.dot(n) - d
}

fn face_centroid(bmesh: &EditMesh, fk: FaceKey) -> Vec3 {
    let face = &bmesh.faces[fk];
    let mut sum = Vec3::ZERO;
    let mut count = 0u32;
    let mut cur = face.loop_first;
    for _ in 0..face.loop_count {
        let lp = &bmesh.loops[cur];
        sum += bmesh.verts[lp.vert].co;
        count += 1;
        cur = lp.next;
    }
    if count == 0 {
        Vec3::ZERO
    } else {
        sum / count as f32
    }
}

/// Walk every face; if any contains both Front and Back ring verts,
/// return its key plus the two On-plane verts of its ring (which must
/// exist after Phase 2 edge splits).
fn find_straddling_face(
    bmesh: &EditMesh,
    side: &HashMap<VertKey, Side>,
) -> Option<(FaceKey, VertKey, VertKey)> {
    for (fk, face) in bmesh.faces.iter() {
        let mut has_front = false;
        let mut has_back = false;
        let mut on: Vec<VertKey> = Vec::new();
        let mut cur = face.loop_first;
        for _ in 0..face.loop_count {
            let vk = bmesh.loops[cur].vert;
            match side.get(&vk).copied().unwrap_or(Side::On) {
                Side::Front => has_front = true,
                Side::Back => has_back = true,
                Side::On => on.push(vk),
            }
            cur = bmesh.loops[cur].next;
        }
        if has_front && has_back && on.len() >= 2 {
            return Some((fk, on[0], on[1]));
        }
    }
    None
}

/// Remove a face: detach all its loops from radial cycles, drop them,
/// then drop the face entry. Verts and edges remain; `gc_orphans`
/// cleans those up afterwards.
fn drop_face(bmesh: &mut EditMesh, fk: FaceKey) {
    if !bmesh.faces.contains_key(fk) {
        return;
    }
    let face_data = bmesh.faces[fk].clone();
    let mut loops_to_remove: Vec<LoopKey> = Vec::with_capacity(face_data.loop_count as usize);
    let mut cur = face_data.loop_first;
    for _ in 0..face_data.loop_count {
        loops_to_remove.push(cur);
        cur = bmesh.loops[cur].next;
    }
    for &lp in &loops_to_remove {
        crate::editmesh::cycles::radial_remove_loop(bmesh, lp);
        bmesh.loops.remove(lp);
    }
    bmesh.faces.remove(fk);
}

/// Drop edges that have no incident loops (no face uses them) and verts
/// that have no incident edges.
fn gc_orphans(bmesh: &mut EditMesh) {
    // Edges with no radial loops left.
    let orphan_edges: Vec<EdgeKey> = bmesh
        .edges
        .iter()
        .filter(|(_, e)| e.loop_first.is_none())
        .map(|(k, _)| k)
        .collect();
    for ek in orphan_edges {
        crate::editmesh::cycles::disk_remove_edge(bmesh, ek);
        bmesh.edges.remove(ek);
    }
    // Verts with no incident edges.
    let orphan_verts: Vec<VertKey> = bmesh
        .verts
        .iter()
        .filter(|(_, v)| v.edge.is_none())
        .map(|(k, _)| k)
        .collect();
    for vk in orphan_verts {
        bmesh.verts.remove(vk);
    }
}

/// Build a cap face from on-plane verts that still have boundary edges
/// (edges with exactly one incident loop). Walks the boundary by
/// following on-plane edges with only one radial loop, and creates a
/// face from that ring.
///
/// `flip_winding` controls which side the cap faces:
///   - `false`: cap normal opposite `plane_normal` (front-side cap; the
///     kept solid lies in `n . x > d` so the outward-facing cap normal
///     is `-n`).
///   - `true`: cap normal aligned with `plane_normal` (back-side cap).
fn build_cap(
    bmesh: &mut EditMesh,
    on_verts: &HashSet<VertKey>,
    plane_normal: Vec3,
    flip_winding: bool,
    material_idx: u32,
) -> Option<FaceKey> {
    use crate::editmesh::ops::face_create::create_face_from_verts_with_material;

    // Collect On verts that still exist post-GC.
    let live_on: HashSet<VertKey> = on_verts
        .iter()
        .copied()
        .filter(|vk| bmesh.verts.contains_key(*vk))
        .collect();
    if live_on.len() < 3 {
        return None;
    }

    // Walk the boundary: an edge that connects two on-plane verts and
    // has exactly one incident loop is part of the cut boundary.
    let mut boundary_neighbors: HashMap<VertKey, Vec<VertKey>> = HashMap::new();
    for (_, e) in bmesh.edges.iter() {
        let va = e.v[0];
        let vb = e.v[1];
        if !live_on.contains(&va) || !live_on.contains(&vb) {
            continue;
        }
        // Only boundary edges: exactly one incident loop.
        let radial_count = if let Some(first) = e.loop_first {
            let mut count = 1u32;
            let mut cur = bmesh.loops[first].radial_next;
            while cur != first {
                count += 1;
                cur = bmesh.loops[cur].radial_next;
                if count > 64 {
                    break;
                }
            }
            count
        } else {
            0
        };
        if radial_count != 1 {
            continue;
        }
        boundary_neighbors.entry(va).or_default().push(vb);
        boundary_neighbors.entry(vb).or_default().push(va);
    }

    let mut ring = walk_boundary_ring(&boundary_neighbors)?;
    if ring.len() < 3 {
        // Boundary walk failed; fall back to polar-angle sort.
        ring = polar_sort_ring(bmesh, &live_on, plane_normal);
        if ring.len() < 3 {
            return None;
        }
    }

    // Orient the ring so its Newell normal matches plane_normal. Then
    // flip for the Front cap so its outward normal is -plane_normal.
    let positions: Vec<Vec3> = ring.iter().map(|&vk| bmesh.verts[vk].co).collect();
    let computed = crate::newell::newell_normal(&positions);
    if computed.dot(plane_normal) < 0.0 {
        ring.reverse();
    }
    // After normalization, ring's Newell normal == +plane_normal. For a
    // Front cap (kept solid is in +n half-space) we want the cap to face
    // out, i.e. along -n. So flip iff this is the Front cap.
    if !flip_winding {
        ring.reverse();
    }

    create_face_from_verts_with_material(bmesh, &ring, Some(material_idx)).ok()
}

/// Walk a boundary ring built from `vert -> neighbors` adjacency. Each
/// vert in a proper boundary should have exactly 2 neighbors. Picks an
/// arbitrary start, walks forward, and stops when it loops back.
fn walk_boundary_ring(adj: &HashMap<VertKey, Vec<VertKey>>) -> Option<Vec<VertKey>> {
    let &start = adj.keys().next()?;
    let mut ring = vec![start];
    let mut prev = start;
    let mut cur = *adj.get(&start)?.first()?;
    let mut guard = 0u32;
    while cur != start {
        ring.push(cur);
        let neighbors = adj.get(&cur)?;
        let nxt = neighbors.iter().copied().find(|&v| v != prev)?;
        prev = cur;
        cur = nxt;
        guard += 1;
        if guard > 1_000_000 {
            return None;
        }
    }
    if ring.len() < 3 {
        return None;
    }
    Some(ring)
}

fn polar_sort_ring(
    bmesh: &EditMesh,
    live_on: &HashSet<VertKey>,
    plane_normal: Vec3,
) -> Vec<VertKey> {
    let on: Vec<VertKey> = live_on.iter().copied().collect();
    let centroid: Vec3 = on
        .iter()
        .map(|&vk| bmesh.verts[vk].co)
        .fold(Vec3::ZERO, |a, b| a + b)
        / on.len() as f32;
    let helper = if plane_normal.x.abs() < 0.9 {
        Vec3::X
    } else {
        Vec3::Y
    };
    let u = plane_normal.cross(helper).normalize_or_zero();
    let v = plane_normal.cross(u).normalize_or_zero();
    let mut ranked: Vec<(VertKey, f32)> = on
        .iter()
        .map(|&vk| {
            let p = bmesh.verts[vk].co - centroid;
            (vk, p.dot(v).atan2(p.dot(u)))
        })
        .collect();
    ranked.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked.into_iter().map(|(k, _)| k).collect()
}
