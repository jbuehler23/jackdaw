//! Engine-agnostic half-edge selection traversal.
//!
//! These functions grow, shrink, and walk selections on a
//! [`jackdaw_geometry::halfedge::HalfedgeMesh`]. Selection crosses the boundary
//! as plain index collections into a brush's caches: vertices and faces as
//! cache indices, edges as ordered pairs of vertex cache indices. The functions
//! map those indices to half-edge keys through the brush's key tables
//! (`vert_keys`, `face_keys`), do the pure traversal, and map the result back to
//! indices. There is no engine dependency, so the same traversal drives
//! selection in any host; the editor wraps each one in a thin operator.

use std::collections::HashMap;
use std::collections::HashSet;

use jackdaw_geometry::halfedge::cycles::{disk_walk, radial_walk};
use jackdaw_geometry::halfedge::select::loop_walk::loop_walk;
use jackdaw_geometry::halfedge::select::ring_walk::ring_walk;
use jackdaw_geometry::halfedge::{EdgeKey, FaceKey, HalfedgeMesh, VertKey};

/// Reverse map from a vertex key to its cache index, the inverse of the
/// `vert_keys` table. Each operator used to rebuild this inline; it is built
/// once here per call.
fn vert_key_to_index(vert_keys: &[VertKey]) -> HashMap<VertKey, usize> {
    let mut map = HashMap::with_capacity(vert_keys.len());
    for (i, &k) in vert_keys.iter().enumerate() {
        map.insert(k, i);
    }
    map
}

/// Find the cache index of the vertex at the other end of `edge` from `vk`.
fn other_vert_index(
    mesh: &HalfedgeMesh,
    vert_keys: &[VertKey],
    edge: EdgeKey,
    vk: VertKey,
) -> Option<usize> {
    let e = &mesh.edges[edge];
    let other = if e.v[0] == vk { e.v[1] } else { e.v[0] };
    vert_keys.iter().position(|&k| k == other)
}

/// Order a pair of indices so the smaller is first, matching the cache edge
/// convention.
fn ordered(a: usize, b: usize) -> (usize, usize) {
    if a < b { (a, b) } else { (b, a) }
}

/// Find the half-edge edge between two vertex keys, in either direction.
fn find_edge_between(mesh: &HalfedgeMesh, va: VertKey, vb: VertKey) -> Option<EdgeKey> {
    mesh.edges
        .iter()
        .find(|(_, e)| (e.v[0] == va && e.v[1] == vb) || (e.v[0] == vb && e.v[1] == va))
        .map(|(k, _)| k)
}

/// Grow a vertex selection by one ring: add every vertex sharing an edge with a
/// currently selected vertex. Returns the new selection as sorted cache indices.
pub fn grow_verts(mesh: &HalfedgeMesh, vert_keys: &[VertKey], current: &[usize]) -> Vec<usize> {
    let mut result: HashSet<usize> = current.iter().copied().collect();
    for &vi in current {
        let Some(&vk) = vert_keys.get(vi) else {
            continue;
        };
        for ek in disk_walk(mesh, vk).collect::<Vec<_>>() {
            if let Some(other) = other_vert_index(mesh, vert_keys, ek, vk) {
                result.insert(other);
            }
        }
    }
    let mut out: Vec<usize> = result.into_iter().collect();
    out.sort_unstable();
    out
}

/// Shrink a vertex selection to its interior: keep only vertices all of whose
/// edge-neighbors are also selected. Returns sorted cache indices.
pub fn shrink_verts(mesh: &HalfedgeMesh, vert_keys: &[VertKey], current: &[usize]) -> Vec<usize> {
    let selected: HashSet<usize> = current.iter().copied().collect();
    let mut out: Vec<usize> = selected
        .iter()
        .copied()
        .filter(|&vi| {
            let Some(&vk) = vert_keys.get(vi) else {
                return false;
            };
            for ek in disk_walk(mesh, vk).collect::<Vec<_>>() {
                if let Some(other) = other_vert_index(mesh, vert_keys, ek, vk)
                    && !selected.contains(&other)
                {
                    return false;
                }
            }
            true
        })
        .collect();
    out.sort_unstable();
    out
}

/// Grow an edge selection by one ring: add every edge sharing a vertex with a
/// currently selected edge. Edges are ordered pairs of vertex cache indices.
pub fn grow_edges(
    mesh: &HalfedgeMesh,
    vert_keys: &[VertKey],
    current: &[(usize, usize)],
) -> Vec<(usize, usize)> {
    let key_to_idx = vert_key_to_index(vert_keys);
    let mut result: HashSet<(usize, usize)> = current.iter().copied().collect();
    for &(a, b) in current {
        let Some(&va) = vert_keys.get(a) else {
            continue;
        };
        let Some(&vb) = vert_keys.get(b) else {
            continue;
        };
        for vk in [va, vb] {
            for ek in disk_walk(mesh, vk).collect::<Vec<_>>() {
                let edge = &mesh.edges[ek];
                let Some(&i0) = key_to_idx.get(&edge.v[0]) else {
                    continue;
                };
                let Some(&i1) = key_to_idx.get(&edge.v[1]) else {
                    continue;
                };
                result.insert(ordered(i0, i1));
            }
        }
    }
    result.into_iter().collect()
}

/// Shrink an edge selection to its interior: keep only edges for which every
/// edge sharing one of their vertices is also selected.
pub fn shrink_edges(
    mesh: &HalfedgeMesh,
    vert_keys: &[VertKey],
    current: &[(usize, usize)],
) -> Vec<(usize, usize)> {
    let key_to_idx = vert_key_to_index(vert_keys);
    let selected: HashSet<(usize, usize)> = current.iter().copied().collect();
    selected
        .iter()
        .copied()
        .filter(|&(a, b)| {
            let Some(&va) = vert_keys.get(a) else {
                return false;
            };
            let Some(&vb) = vert_keys.get(b) else {
                return false;
            };
            for vk in [va, vb] {
                for ek in disk_walk(mesh, vk).collect::<Vec<_>>() {
                    let edge = &mesh.edges[ek];
                    let Some(&i0) = key_to_idx.get(&edge.v[0]) else {
                        continue;
                    };
                    let Some(&i1) = key_to_idx.get(&edge.v[1]) else {
                        continue;
                    };
                    if !selected.contains(&ordered(i0, i1)) {
                        return false;
                    }
                }
            }
            true
        })
        .collect()
}

/// Grow a face selection by one ring: add every face sharing an edge with a
/// currently selected face. Returns sorted cache indices.
pub fn grow_faces(mesh: &HalfedgeMesh, face_keys: &[FaceKey], current: &[usize]) -> Vec<usize> {
    let mut result: HashSet<usize> = current.iter().copied().collect();
    for &fi in current {
        let Some(&fk) = face_keys.get(fi) else {
            continue;
        };
        let face = &mesh.faces[fk];
        let mut cur = face.loop_first;
        for _ in 0..face.loop_count {
            let edge = mesh.loops[cur].edge;
            for radial_lp in radial_walk(mesh, edge).collect::<Vec<_>>() {
                let neighbor = mesh.loops[radial_lp].face;
                if let Some(idx) = face_keys.iter().position(|&k| k == neighbor) {
                    result.insert(idx);
                }
            }
            cur = mesh.loops[cur].next;
        }
    }
    let mut out: Vec<usize> = result.into_iter().collect();
    out.sort_unstable();
    out
}

/// Shrink a face selection to its interior: keep only faces all of whose
/// edge-adjacent faces are also selected. Returns sorted cache indices.
pub fn shrink_faces(mesh: &HalfedgeMesh, face_keys: &[FaceKey], current: &[usize]) -> Vec<usize> {
    let selected: HashSet<usize> = current.iter().copied().collect();
    let mut out: Vec<usize> = selected
        .iter()
        .copied()
        .filter(|&fi| {
            let Some(&fk) = face_keys.get(fi) else {
                return false;
            };
            let face = &mesh.faces[fk];
            let mut cur = face.loop_first;
            for _ in 0..face.loop_count {
                let edge = mesh.loops[cur].edge;
                for radial_lp in radial_walk(mesh, edge).collect::<Vec<_>>() {
                    let neighbor = mesh.loops[radial_lp].face;
                    if let Some(idx) = face_keys.iter().position(|&k| k == neighbor)
                        && !selected.contains(&idx)
                    {
                        return false;
                    }
                }
                cur = mesh.loops[cur].next;
            }
            true
        })
        .collect();
    out.sort_unstable();
    out
}

/// Convert a set of walked half-edge edges back to ordered cache pairs, skipping
/// any whose endpoints are not in the cache.
fn edges_to_cache_pairs(
    mesh: &HalfedgeMesh,
    key_to_idx: &HashMap<VertKey, usize>,
    walked: impl IntoIterator<Item = EdgeKey>,
) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for ek in walked {
        let edge = &mesh.edges[ek];
        let Some(&a) = key_to_idx.get(&edge.v[0]) else {
            continue;
        };
        let Some(&b) = key_to_idx.get(&edge.v[1]) else {
            continue;
        };
        let pair = ordered(a, b);
        if !out.contains(&pair) {
            out.push(pair);
        }
    }
    out
}

/// Expand an edge selection along parallel-edge loops: for each selected edge,
/// walk its loop ring through quad faces and union the results. Returns the new
/// selection as ordered cache pairs, or an empty vec if nothing walks (no
/// matching mesh edge, or every walk was empty).
pub fn loop_edges(
    mesh: &HalfedgeMesh,
    vert_keys: &[VertKey],
    current: &[(usize, usize)],
) -> Vec<(usize, usize)> {
    walk_edges(mesh, vert_keys, current, loop_walk)
}

/// Expand an edge selection along perpendicular-edge rings: for each selected
/// edge, walk its ring through quad faces and union the results. Returns the new
/// selection as ordered cache pairs, or an empty vec if nothing walks.
pub fn ring_edges(
    mesh: &HalfedgeMesh,
    vert_keys: &[VertKey],
    current: &[(usize, usize)],
) -> Vec<(usize, usize)> {
    walk_edges(mesh, vert_keys, current, ring_walk)
}

/// Shared driver for the loop and ring edge walks: map each selected cache edge
/// to its mesh edge, run `walk` from each, union the keys, and map back.
fn walk_edges(
    mesh: &HalfedgeMesh,
    vert_keys: &[VertKey],
    current: &[(usize, usize)],
    walk: impl Fn(&HalfedgeMesh, EdgeKey) -> Vec<EdgeKey>,
) -> Vec<(usize, usize)> {
    let mut mesh_edges: Vec<EdgeKey> = Vec::with_capacity(current.len());
    for &(a, b) in current {
        let Some(&va) = vert_keys.get(a) else {
            continue;
        };
        let Some(&vb) = vert_keys.get(b) else {
            continue;
        };
        if let Some(ek) = find_edge_between(mesh, va, vb) {
            mesh_edges.push(ek);
        }
    }
    if mesh_edges.is_empty() {
        return Vec::new();
    }

    let mut walked: HashSet<EdgeKey> = HashSet::new();
    for ek in mesh_edges {
        for k in walk(mesh, ek) {
            walked.insert(k);
        }
    }
    if walked.is_empty() {
        return Vec::new();
    }

    let key_to_idx = vert_key_to_index(vert_keys);
    edges_to_cache_pairs(mesh, &key_to_idx, walked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use jackdaw_geometry::halfedge::HalfedgeBinding;
    use jackdaw_geometry::{compute_brush_topology, cuboid_faces};

    /// A unit cuboid lifted exactly the way the editor does: topology from
    /// `cuboid_faces`, then a `HalfedgeBinding` carrying the mesh plus the
    /// `vert_keys` / `face_keys` index tables. 8 verts, 12 edges, 6 quad faces.
    fn cuboid() -> HalfedgeBinding {
        let topo = compute_brush_topology(&cuboid_faces(Vec3::splat(1.0)));
        HalfedgeBinding::lift_from_topology(&topo)
    }

    /// The three edges incident to vertex cache index `vi`, as ordered cache
    /// pairs. Used to build edge fixtures without hard-coding the lift order.
    fn incident_edges(b: &HalfedgeBinding, vi: usize) -> Vec<(usize, usize)> {
        let key_to_idx = vert_key_to_index(&b.vert_keys);
        let vk = b.vert_keys[vi];
        let mut out = Vec::new();
        for ek in disk_walk(&b.mesh, vk).collect::<Vec<_>>() {
            let e = &b.mesh.edges[ek];
            let a = key_to_idx[&e.v[0]];
            let c = key_to_idx[&e.v[1]];
            out.push(ordered(a, c));
        }
        out
    }

    #[test]
    fn cuboid_fixture_has_expected_counts() {
        let b = cuboid();
        assert_eq!(b.vert_keys.len(), 8);
        assert_eq!(b.face_keys.len(), 6);
        assert_eq!(b.mesh.edge_count(), 12);
    }

    #[test]
    fn grow_single_vertex_adds_its_three_neighbors() {
        let b = cuboid();
        // Every cuboid corner has exactly three edge-neighbors.
        let grown = grow_verts(&b.mesh, &b.vert_keys, &[0]);
        assert_eq!(grown.len(), 4, "self plus three neighbors: {grown:?}");
        assert!(grown.contains(&0));
        // The three added vertices are exactly the disk neighbors of vertex 0.
        let neighbors: Vec<usize> = incident_edges(&b, 0)
            .into_iter()
            .map(|(a, c)| if a == 0 { c } else { a })
            .collect();
        for n in neighbors {
            assert!(grown.contains(&n), "missing neighbor {n} in {grown:?}");
        }
    }

    #[test]
    fn shrink_reverses_a_grown_vertex_selection() {
        let b = cuboid();
        // Grow from a single vertex, then shrink: only the original interior
        // vertex (all of whose neighbors are now selected) survives.
        let grown = grow_verts(&b.mesh, &b.vert_keys, &[0]);
        let shrunk = shrink_verts(&b.mesh, &b.vert_keys, &grown);
        assert_eq!(shrunk, vec![0], "shrink returns to the seed: {shrunk:?}");
    }

    #[test]
    fn shrink_all_vertices_keeps_all() {
        let b = cuboid();
        let all: Vec<usize> = (0..b.vert_keys.len()).collect();
        let shrunk = shrink_verts(&b.mesh, &b.vert_keys, &all);
        assert_eq!(shrunk, all, "a fully selected mesh has no boundary");
    }

    #[test]
    fn grow_then_shrink_faces_returns_seed() {
        let b = cuboid();
        // Growing one face on a cuboid adds the four side faces (every other
        // face except the opposite one shares an edge). Shrinking returns to it.
        let grown = grow_faces(&b.mesh, &b.face_keys, &[0]);
        assert_eq!(grown.len(), 5, "seed plus four edge-adjacent: {grown:?}");
        let shrunk = shrink_faces(&b.mesh, &b.face_keys, &grown);
        assert_eq!(shrunk, vec![0]);
    }

    #[test]
    fn grow_edges_then_shrink_returns_seed() {
        let b = cuboid();
        let seed = incident_edges(&b, 0);
        assert_eq!(seed.len(), 3);
        let grown = grow_edges(&b.mesh, &b.vert_keys, &seed);
        // Growing the three edges at a corner adds the edges at the three
        // adjacent corners, so the set strictly grows.
        assert!(grown.len() > seed.len(), "grew: {grown:?}");
        let shrunk = shrink_edges(&b.mesh, &b.vert_keys, &grown);
        // Shrink keeps only edges all of whose vertex-incident edges are
        // selected. The three seed edges meet at the fully-surrounded corner 0.
        let seed_set: HashSet<(usize, usize)> = seed.iter().copied().collect();
        let shrunk_set: HashSet<(usize, usize)> = shrunk.iter().copied().collect();
        assert!(
            seed_set.is_subset(&shrunk_set),
            "seed survives shrink: seed {seed:?} shrunk {shrunk:?}"
        );
    }

    #[test]
    fn loop_from_one_edge_walks_a_four_edge_ring() {
        let b = cuboid();
        // Any single cuboid edge belongs to a loop of exactly four parallel
        // edges that wraps the cube.
        let seed = vec![incident_edges(&b, 0)[0]];
        let looped = loop_edges(&b.mesh, &b.vert_keys, &seed);
        assert_eq!(looped.len(), 4, "edge loop on a cuboid: {looped:?}");
        // The seed edge is part of its own loop.
        assert!(looped.contains(&seed[0]));
    }

    #[test]
    fn ring_from_one_edge_walks_the_perpendicular_set() {
        let b = cuboid();
        let seed_edge = incident_edges(&b, 0)[0];
        let ringed = ring_edges(&b.mesh, &b.vert_keys, &[seed_edge]);
        // The ring of perpendicular edges on a cuboid also has four members and
        // is disjoint from the parallel loop except for the seed.
        assert_eq!(ringed.len(), 4, "edge ring on a cuboid: {ringed:?}");
        let looped: HashSet<(usize, usize)> = loop_edges(&b.mesh, &b.vert_keys, &[seed_edge])
            .into_iter()
            .collect();
        let ring_set: HashSet<(usize, usize)> = ringed.iter().copied().collect();
        // The only edge shared between the loop and ring is the seed itself.
        let shared: Vec<_> = ring_set.intersection(&looped).collect();
        assert_eq!(shared, vec![&seed_edge], "loop and ring meet only at seed");
    }
}
