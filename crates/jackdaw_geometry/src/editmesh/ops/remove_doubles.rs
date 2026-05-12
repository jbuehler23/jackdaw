//! Weld vertices within `distance` of each other. Implements the cleanup
//! that Blender's "Merge by Distance" performs. Removes degenerate edges
//! (both endpoints same) and degenerate faces (loop count < 3 after collapse).
//!
//! Uses the "rebuild via topology" approach: flatten -> remap -> clean -> lift.
//! O(n²) pairwise distance check + union-find clustering. Brush sizes are
//! small enough that this is fast enough for MVP.

use std::collections::HashMap;

use crate::editmesh::types::*;
use crate::topology::{BrushTopology, MeshEdge, MeshLoop, MeshPoly, MeshVert};

#[derive(Debug)]
pub enum MergeError {
    InvalidDistance,
}

pub struct MergeResult {
    pub merged_verts: usize,
    pub removed_edges: usize,
    pub removed_faces: usize,
}

pub fn remove_doubles(bmesh: &mut EditMesh, distance: f32) -> Result<MergeResult, MergeError> {
    if !distance.is_finite() || distance < 0.0 {
        return Err(MergeError::InvalidDistance);
    }
    if distance == 0.0 || bmesh.vert_count() < 2 {
        return Ok(MergeResult {
            merged_verts: 0,
            removed_edges: 0,
            removed_faces: 0,
        });
    }

    // Step 1: Collect vertices in a stable sorted order (by slotmap key FFI value).
    let mut keyed: Vec<(VertKey, bevy::math::Vec3)> =
        bmesh.verts.iter().map(|(k, v)| (k, v.co)).collect();
    keyed.sort_by_key(|(k, _)| {
        use slotmap::Key;
        k.data().as_ffi()
    });

    // Step 2: Union-find clustering over all vert pairs within distance.
    let n = keyed.len();
    let mut parent: Vec<usize> = (0..n).collect();

    let dist_sq = distance * distance;
    for i in 0..n {
        for j in (i + 1)..n {
            if (keyed[i].1 - keyed[j].1).length_squared() <= dist_sq {
                union_find_union(&mut parent, i, j);
            }
        }
    }

    // Step 3: Pick canonical vert per cluster (lowest index in stable-sorted order
    // is first encountered, since we iterate in sorted order).
    // cluster_canonical maps root -> canonical VertKey.
    let mut cluster_canonical: HashMap<usize, VertKey> = HashMap::new();
    for i in 0..n {
        let root = union_find_root(&mut parent, i);
        cluster_canonical.entry(root).or_insert(keyed[i].0);
    }

    // Build vert_remap: original_key -> canonical_key.
    let mut vert_remap: HashMap<VertKey, VertKey> = HashMap::new();
    for i in 0..n {
        let root = union_find_root(&mut parent, i);
        let canonical = cluster_canonical[&root];
        vert_remap.insert(keyed[i].0, canonical);
    }

    let merged_count = vert_remap.iter().filter(|(k, v)| **k != **v).count();
    if merged_count == 0 {
        return Ok(MergeResult {
            merged_verts: 0,
            removed_edges: 0,
            removed_faces: 0,
        });
    }

    // Step 4: Flatten current EditMesh to topology.
    let topology_before = bmesh.flatten_to_topology();

    // Build reverse lookup: topology vertex index -> VertKey.
    // flatten_to_topology iterates bmesh.verts in slotmap iter() order, which may
    // differ from our sorted `keyed` order, so we build the mapping directly.
    let mut old_idx_to_key: Vec<VertKey> = Vec::with_capacity(topology_before.vertices.len());
    for (k, _) in bmesh.verts.iter() {
        old_idx_to_key.push(k);
    }

    // Build the new vertex list: only canonical verts, preserving their positions.
    // We need canonical_key -> new index.
    let mut canonical_to_new_idx: HashMap<VertKey, u32> = HashMap::new();
    let mut new_vertices: Vec<MeshVert> = Vec::new();
    // Iterate topology verts in order; for each, if it maps to itself (canonical), add it.
    for (old_idx, _mv) in topology_before.vertices.iter().enumerate() {
        let original_key = old_idx_to_key[old_idx];
        let canonical = vert_remap[&original_key];
        if canonical == original_key && !canonical_to_new_idx.contains_key(&canonical) {
            let new_idx = new_vertices.len() as u32;
            canonical_to_new_idx.insert(canonical, new_idx);
            new_vertices.push(MeshVert {
                position: bmesh.verts[canonical].co,
            });
        }
    }
    // Any canonical key not yet inserted (could happen if slotmap iter order places
    // non-canonical verts before canonical in the topology flatten). Ensure all
    // canonicals are present.
    for i in 0..n {
        let root = union_find_root(&mut parent, i);
        let canonical = cluster_canonical[&root];
        if !canonical_to_new_idx.contains_key(&canonical) {
            let new_idx = new_vertices.len() as u32;
            canonical_to_new_idx.insert(canonical, new_idx);
            new_vertices.push(MeshVert {
                position: bmesh.verts[canonical].co,
            });
        }
    }

    // Helper: map old topology vert index to new topology vert index via canonical.
    let map_old_idx = |old_idx: u32| -> u32 {
        let original_key = old_idx_to_key[old_idx as usize];
        let canonical = vert_remap[&original_key];
        canonical_to_new_idx[&canonical]
    };

    // Step 5: Build new edges, deduplicating by canonical pair.
    let mut new_edges: Vec<MeshEdge> = Vec::new();
    let mut edge_lookup: HashMap<(u32, u32), u32> = HashMap::new();
    let mut old_edge_to_new_edge: HashMap<u32, u32> = HashMap::new();
    for (old_e_idx, edge) in topology_before.edges.iter().enumerate() {
        let v0_new = map_old_idx(edge.v[0]);
        let v1_new = map_old_idx(edge.v[1]);
        if v0_new == v1_new {
            // Degenerate edge; skip. old_edge_to_new_edge has no entry.
            continue;
        }
        let pair = if v0_new < v1_new {
            (v0_new, v1_new)
        } else {
            (v1_new, v0_new)
        };
        let new_e_idx = if let Some(&existing) = edge_lookup.get(&pair) {
            existing
        } else {
            let idx = new_edges.len() as u32;
            new_edges.push(MeshEdge {
                v: [pair.0, pair.1],
                flags: edge.flags,
            });
            edge_lookup.insert(pair, idx);
            idx
        };
        old_edge_to_new_edge.insert(old_e_idx as u32, new_e_idx);
    }

    // Step 6: Build new polygons and loops.
    // For each polygon, remap vertices; skip consecutive duplicates; skip degenerate
    // edges; drop faces with fewer than 3 distinct verts after remapping.
    let mut new_polygons: Vec<MeshPoly> = Vec::new();
    let mut new_loops: Vec<MeshLoop> = Vec::new();
    let mut removed_faces = 0usize;

    for poly in &topology_before.polygons {
        let start = poly.loop_start as usize;
        let total = poly.loop_total as usize;
        let face_loops = &topology_before.loops[start..start + total];

        // Build the remapped ring, skipping loops whose edge became degenerate.
        let mut ring: Vec<(u32, u32)> = Vec::new(); // (new_vert_idx, new_edge_idx)
        for lp in face_loops {
            let new_v = map_old_idx(lp.vert);
            let Some(&new_e) = old_edge_to_new_edge.get(&lp.edge) else {
                // This loop's edge was degenerate after remapping; skip it.
                continue;
            };
            // Skip if this vert is the same as the previous one (consecutive duplicate).
            if let Some(&(prev_v, _)) = ring.last() {
                if prev_v == new_v {
                    continue;
                }
            }
            ring.push((new_v, new_e));
        }

        // Close-check: if the last vert in the ring equals the first, remove the last.
        if ring.len() >= 2 && ring[0].0 == ring.last().unwrap().0 {
            ring.pop();
        }

        if ring.len() < 3 {
            removed_faces += 1;
            continue;
        }

        let loop_start = new_loops.len() as u32;
        for (v, e) in &ring {
            new_loops.push(MeshLoop { vert: *v, edge: *e });
        }
        new_polygons.push(MeshPoly {
            loop_start,
            loop_total: ring.len() as u32,
        });
    }

    let removed_edges = topology_before.edges.len() - new_edges.len();

    // Step 7: Rebuild EditMesh from cleaned topology.
    let new_topology = BrushTopology {
        vertices: new_vertices,
        edges: new_edges,
        polygons: new_polygons,
        loops: new_loops,
        attributes: Default::default(),
    };

    *bmesh = EditMesh::lift_from_topology(&new_topology);

    Ok(MergeResult {
        merged_verts: merged_count,
        removed_edges,
        removed_faces,
    })
}

// Union-find helpers (path-compressing, union-by-index).

fn union_find_root(parent: &mut [usize], i: usize) -> usize {
    if parent[i] != i {
        parent[i] = union_find_root(parent, parent[i]);
    }
    parent[i]
}

fn union_find_union(parent: &mut [usize], a: usize, b: usize) {
    let ra = union_find_root(parent, a);
    let rb = union_find_root(parent, b);
    if ra != rb {
        parent[ra] = rb;
    }
}
