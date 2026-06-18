//! Index-preserving half-space clip. Discards the geometry on one side of an
//! axis-aligned plane while keeping every input vertex in place, so a caller's
//! identity prefix survives the cut. Split vertices and the cut cap carry the
//! `NO_SOURCE` marker.

use std::collections::HashMap;

use glam::Vec3;

use crate::{EvaluatedBrush, NO_SOURCE, newell_normal};

/// Distance from the plane within which a vertex is treated as lying on it.
/// On-plane verts are shared rather than duplicated, so a face touching the
/// plane along an edge does not spawn coincident split verts.
const ON_PLANE_EPS: f32 = 1e-5;

/// Clip `vertices` / `face_polygons` to the half-space on one side of an
/// axis-aligned plane, preserving every input vertex index.
///
/// `keep_positive` keeps the `v[axis] >= plane` side; clear it to keep the
/// `v[axis] <= plane` side. Discarded-side verts stay in the output `vertices`
/// array (so input indices and the caller's source maps are untouched) but are
/// no longer referenced by any face. Edges that straddle the plane gain a split
/// vertex on the plane, shared between the two faces meeting at that edge, and
/// the cut segments chain into one cap face per closed loop. Split verts and
/// cap faces carry `NO_SOURCE`.
pub fn clip_to_halfspace(
    vertices: &[Vec3],
    face_polygons: &[Vec<usize>],
    vert_source: &[u32],
    face_source: &[u32],
    axis: usize,
    plane: f32,
    keep_positive: bool,
) -> EvaluatedBrush {
    let mut out_vertices = vertices.to_vec();
    let mut out_vert_source = vert_source.to_vec();
    let mut out_faces: Vec<Vec<usize>> = Vec::new();
    let mut out_face_source: Vec<u32> = Vec::new();

    // Signed distance from the plane, positive toward the kept side.
    let signed = |v: Vec3| -> f32 {
        let d = v[axis] - plane;
        if keep_positive { d } else { -d }
    };
    // A vert is kept when it sits on the kept side or on the plane (within eps).
    let kept = |v: Vec3| -> bool { signed(v) >= -ON_PLANE_EPS };

    // Split verts are cached per undirected input edge so both faces sharing a
    // cut edge reference the same new vertex, keeping the cap watertight.
    let mut split_cache: HashMap<(usize, usize), usize> = HashMap::new();
    // One cut segment per clipped face: its two on-plane split-vertex indices.
    let mut cut_segments: Vec<(usize, usize)> = Vec::new();

    for (fi, ring) in face_polygons.iter().enumerate() {
        if ring.len() < 3 {
            continue;
        }

        let mut clipped: Vec<usize> = Vec::new();
        // Split verts this face produced, in ring order, to form its segment.
        let mut face_splits: Vec<usize> = Vec::new();

        for i in 0..ring.len() {
            let cur = ring[i];
            let next = ring[(i + 1) % ring.len()];

            if kept(out_vertices[cur]) {
                clipped.push(cur);
            }

            // Emit a split vert only when the edge strictly crosses the plane.
            // A shared on-plane endpoint (one side at the plane, the other off)
            // is not a strict crossing, so no duplicate is made there.
            let cur_s = signed(out_vertices[cur]);
            let next_s = signed(out_vertices[next]);
            let strictly_crosses = (cur_s > ON_PLANE_EPS && next_s < -ON_PLANE_EPS)
                || (cur_s < -ON_PLANE_EPS && next_s > ON_PLANE_EPS);
            if strictly_crosses {
                let key = if cur < next { (cur, next) } else { (next, cur) };
                let split_idx = *split_cache.entry(key).or_insert_with(|| {
                    let a = out_vertices[cur];
                    let b = out_vertices[next];
                    // Linear interpolation along `axis` onto the plane.
                    let t = (plane - a[axis]) / (b[axis] - a[axis]);
                    let mut p = a + (b - a) * t;
                    // Pin the split coordinate exactly onto the plane.
                    p[axis] = plane;
                    let idx = out_vertices.len();
                    out_vertices.push(p);
                    out_vert_source.push(NO_SOURCE);
                    idx
                });
                clipped.push(split_idx);
                face_splits.push(split_idx);
            }
        }

        if clipped.len() >= 3 {
            out_faces.push(clipped);
            out_face_source.push(face_source[fi]);
        }
        // A clean clip of a convex ring crosses the plane exactly twice, giving
        // one cut segment. Record it for cap chaining.
        if face_splits.len() == 2 {
            cut_segments.push((face_splits[0], face_splits[1]));
        }
    }

    // Chain the cut segments into closed loops by shared endpoints and emit one
    // cap face per loop. A remnant whose segments do not close is skipped.
    for loop_ring in chain_segments_into_loops(&cut_segments) {
        if loop_ring.len() < 3 {
            continue;
        }
        let oriented = orient_cap_loop(&out_vertices, loop_ring, axis, keep_positive);
        out_faces.push(oriented);
        out_face_source.push(NO_SOURCE);
    }

    EvaluatedBrush {
        vertices: out_vertices,
        face_polygons: out_faces,
        face_source: out_face_source,
        vert_source: out_vert_source,
    }
}

/// Chain undirected segments into closed vertex loops. Each segment connects to
/// another that shares one of its endpoints. Segments that cannot be walked into
/// a closed ring are dropped (their loop is left out rather than half-built).
fn chain_segments_into_loops(segments: &[(usize, usize)]) -> Vec<Vec<usize>> {
    // Adjacency: each vertex -> the segment endpoints it links to.
    let mut adjacency: HashMap<usize, Vec<usize>> = HashMap::new();
    for &(a, b) in segments {
        if a == b {
            continue;
        }
        adjacency.entry(a).or_default().push(b);
        adjacency.entry(b).or_default().push(a);
    }

    let mut loops: Vec<Vec<usize>> = Vec::new();
    let mut visited_edge: std::collections::HashSet<(usize, usize)> =
        std::collections::HashSet::new();
    let edge_key = |a: usize, b: usize| if a < b { (a, b) } else { (b, a) };

    for &(start, _) in segments {
        // Skip starts whose every incident segment is already consumed.
        let already = adjacency
            .get(&start)
            .map(|ns| {
                ns.iter()
                    .all(|&n| visited_edge.contains(&edge_key(start, n)))
            })
            .unwrap_or(true);
        if already {
            continue;
        }

        let mut ring = vec![start];
        let mut prev = start;
        let mut cur = start;
        let mut closed = false;

        // Walk forward edge by edge until the ring closes back on `start` or a
        // dead end is hit.
        while let Some(neighbours) = adjacency.get(&cur) {
            // Pick the next unvisited neighbour, preferring one that is not the
            // vertex we just came from so we keep walking forward.
            let next = neighbours
                .iter()
                .copied()
                .find(|&n| n != prev && !visited_edge.contains(&edge_key(cur, n)))
                .or_else(|| {
                    neighbours
                        .iter()
                        .copied()
                        .find(|&n| !visited_edge.contains(&edge_key(cur, n)))
                });
            let Some(next) = next else {
                break;
            };
            visited_edge.insert(edge_key(cur, next));
            if next == start {
                closed = true;
                break;
            }
            ring.push(next);
            prev = cur;
            cur = next;
        }

        if closed && ring.len() >= 3 {
            loops.push(ring);
        }
        // An open chain is a degenerate remnant: leave it out, no cap.
    }

    loops
}

/// Orient a cap loop so its Newell normal points toward the discarded side
/// (opposite the kept half), keeping the solid outward-facing.
fn orient_cap_loop(
    vertices: &[Vec3],
    ring: Vec<usize>,
    axis: usize,
    keep_positive: bool,
) -> Vec<usize> {
    let positions: Vec<Vec3> = ring.iter().map(|&vi| vertices[vi]).collect();
    let normal = newell_normal(&positions);
    // Discarded side is opposite the kept half along `axis`.
    let want_positive_axis = !keep_positive;
    let normal_on_axis = normal[axis];
    let points_to_discard = if want_positive_axis {
        normal_on_axis > 0.0
    } else {
        normal_on_axis < 0.0
    };
    if points_to_discard || normal == Vec3::ZERO {
        ring
    } else {
        let mut reversed = ring;
        reversed.reverse();
        reversed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Axis-aligned unit cube spanning [-1, 1] on every axis, 8 verts, 6 quad
    /// faces wound CCW when viewed from outside. Identity source maps.
    fn cube() -> (Vec<Vec3>, Vec<Vec<usize>>, Vec<u32>, Vec<u32>) {
        let verts = vec![
            Vec3::new(-1.0, -1.0, -1.0), // 0
            Vec3::new(1.0, -1.0, -1.0),  // 1
            Vec3::new(1.0, 1.0, -1.0),   // 2
            Vec3::new(-1.0, 1.0, -1.0),  // 3
            Vec3::new(-1.0, -1.0, 1.0),  // 4
            Vec3::new(1.0, -1.0, 1.0),   // 5
            Vec3::new(1.0, 1.0, 1.0),    // 6
            Vec3::new(-1.0, 1.0, 1.0),   // 7
        ];
        let faces = vec![
            vec![0, 3, 2, 1], // -Z
            vec![4, 5, 6, 7], // +Z
            vec![0, 1, 5, 4], // -Y
            vec![3, 7, 6, 2], // +Y
            vec![0, 4, 7, 3], // -X
            vec![1, 2, 6, 5], // +X
        ];
        let vsrc: Vec<u32> = (0..verts.len() as u32).collect();
        let fsrc: Vec<u32> = (0..faces.len() as u32).collect();
        (verts, faces, vsrc, fsrc)
    }

    #[test]
    fn clip_keep_positive_x_caps_and_preserves_indices() {
        let (verts, faces, vsrc, fsrc) = cube();
        let out = clip_to_halfspace(&verts, &faces, &vsrc, &fsrc, 0, 0.0, true);

        // No input vert is deleted: 8 authored + 4 split verts on x=0.
        assert_eq!(out.vertices.len(), 12, "8 authored + 4 split on the plane");
        // The authored prefix is untouched in position and source.
        assert_eq!(&out.vertices[..8], &verts[..]);
        assert_eq!(&out.vert_source[..8], &vsrc[..]);
        // Exactly the 4 split verts carry NO_SOURCE.
        let no_src_verts = out.vert_source.iter().filter(|&&s| s == NO_SOURCE).count();
        assert_eq!(no_src_verts, 4, "four split verts, all NO_SOURCE");
        // The four split verts sit exactly on x = 0.
        for v in &out.vertices[8..] {
            assert!((v.x - 0.0).abs() < 1e-6, "split vert on the plane: {v:?}");
        }

        // Exactly one cap face, marked NO_SOURCE.
        let cap_count = out.face_source.iter().filter(|&&s| s == NO_SOURCE).count();
        assert_eq!(cap_count, 1, "one cut cap");
        let cap_idx = out
            .face_source
            .iter()
            .position(|&s| s == NO_SOURCE)
            .expect("a cap face");
        assert_eq!(out.face_polygons[cap_idx].len(), 4, "cap is a quad");
        // The cap uses only split verts (all on the plane).
        for &vi in &out.face_polygons[cap_idx] {
            assert!(vi >= 8, "cap references only split verts");
        }

        // No kept face references a vertex on the discarded (x < 0) side.
        for (fi, ring) in out.face_polygons.iter().enumerate() {
            if out.face_source[fi] == NO_SOURCE {
                continue;
            }
            for &vi in ring {
                assert!(
                    out.vertices[vi].x >= -1e-5,
                    "kept face {fi} touches discarded vert {vi}: {:?}",
                    out.vertices[vi]
                );
            }
        }

        // Cap winding faces the discarded (-X) side.
        let cap_positions: Vec<Vec3> = out.face_polygons[cap_idx]
            .iter()
            .map(|&vi| out.vertices[vi])
            .collect();
        let n = newell_normal(&cap_positions);
        assert!(n.x < 0.0, "cap normal points to discarded -X side: {n:?}");
    }

    #[test]
    fn clip_keep_negative_x_keeps_the_other_half() {
        let (verts, faces, vsrc, fsrc) = cube();
        let out = clip_to_halfspace(&verts, &faces, &vsrc, &fsrc, 0, 0.0, false);

        assert_eq!(out.vertices.len(), 12, "8 authored + 4 split on the plane");
        assert_eq!(&out.vertices[..8], &verts[..]);
        let cap_count = out.face_source.iter().filter(|&&s| s == NO_SOURCE).count();
        assert_eq!(cap_count, 1, "one cut cap");

        // Kept faces stay on the x <= 0 side.
        for (fi, ring) in out.face_polygons.iter().enumerate() {
            if out.face_source[fi] == NO_SOURCE {
                continue;
            }
            for &vi in ring {
                assert!(
                    out.vertices[vi].x <= 1e-5,
                    "kept face {fi} touches discarded vert {vi}: {:?}",
                    out.vertices[vi]
                );
            }
        }

        // Cap winding faces the discarded (+X) side now.
        let cap_idx = out
            .face_source
            .iter()
            .position(|&s| s == NO_SOURCE)
            .expect("a cap face");
        let cap_positions: Vec<Vec3> = out.face_polygons[cap_idx]
            .iter()
            .map(|&vi| out.vertices[vi])
            .collect();
        let n = newell_normal(&cap_positions);
        assert!(n.x > 0.0, "cap normal points to discarded +X side: {n:?}");
    }

    #[test]
    fn fully_kept_geometry_passes_through_without_a_cap() {
        let (verts, faces, vsrc, fsrc) = cube();
        // Plane below the cube: every vert is kept, nothing crosses.
        let out = clip_to_halfspace(&verts, &faces, &vsrc, &fsrc, 0, -5.0, true);
        assert_eq!(out.vertices.len(), verts.len(), "no split verts added");
        assert_eq!(out.face_polygons.len(), faces.len(), "all faces kept");
        assert!(
            out.face_source.iter().all(|&s| s != NO_SOURCE),
            "no cap when nothing is cut"
        );
        assert_eq!(out.face_polygons, faces, "kept faces unchanged");
    }

    #[test]
    fn fully_discarded_geometry_drops_every_face() {
        let (verts, faces, vsrc, fsrc) = cube();
        // Plane above the cube, keep_positive: every vert is discarded.
        let out = clip_to_halfspace(&verts, &faces, &vsrc, &fsrc, 0, 5.0, true);
        assert!(out.face_polygons.is_empty(), "all faces discarded");
        // Verts are preserved in place even when unreferenced.
        assert_eq!(out.vertices.len(), verts.len());
    }
}
