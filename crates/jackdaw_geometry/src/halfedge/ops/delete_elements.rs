//! Destructive delete: remove geometry and leave a hole. Unlike dissolve, no
//! incident faces are merged or healed; selected elements are destroyed and any
//! vert left with no incident face is dropped as loose. Open, non-convex, and
//! non-closed results are intended; the mesh is not re-closed or capped.
//!
//! Rebuilds via the topology round-trip (flatten -> drop polygons / loose verts
//! -> lift) so no manual half-edge surgery is needed.

use std::collections::HashSet;

use crate::halfedge::types::*;
use crate::topology::{BrushTopology, MeshEdge, MeshLoop, MeshPoly, MeshVert};

/// What a delete removed, plus the original polygon indices (in pre-delete
/// flatten order) that survived. Callers keep parallel per-face data (planes,
/// materials) in sync by re-indexing through `surviving_faces`.
pub struct DeleteResult {
    pub removed_faces: usize,
    pub removed_verts: usize,
    /// For each polygon in the rebuilt mesh, the index of the original polygon
    /// it came from (in pre-delete flatten order). Parallel to the new faces.
    pub surviving_faces: Vec<usize>,
}

/// Delete the given verts: drop every face using any of them, then drop those
/// verts and any other vert left loose. `verts` are `VertKey`s into `mesh`.
pub fn delete_verts(mesh: &mut HalfedgeMesh, verts: &[VertKey]) -> DeleteResult {
    let topology = mesh.flatten_to_topology();
    let key_to_idx = vert_key_to_topology_index(mesh);

    let mut removed_vert_idx: HashSet<u32> = HashSet::new();
    for vk in verts {
        if let Some(&idx) = key_to_idx.get(vk) {
            removed_vert_idx.insert(idx);
        }
    }

    // A face is removed if its ring touches any removed vert.
    let mut removed_polys: HashSet<usize> = HashSet::new();
    for (face_idx, poly) in topology.polygons.iter().enumerate() {
        let start = poly.loop_start as usize;
        let total = poly.loop_total as usize;
        if topology.loops[start..start + total]
            .iter()
            .any(|lp| removed_vert_idx.contains(&lp.vert))
        {
            removed_polys.insert(face_idx);
        }
    }

    rebuild_without(mesh, &topology, &removed_polys, &removed_vert_idx)
}

/// Delete the given edges: drop every face that uses a selected edge (its two
/// verts consecutive in the face ring), then drop verts left loose. Endpoint
/// verts survive if a remaining face still uses them. `edges` are `EdgeKey`s
/// into `mesh`.
pub fn delete_edges(mesh: &mut HalfedgeMesh, edges: &[EdgeKey]) -> DeleteResult {
    let topology = mesh.flatten_to_topology();
    let key_to_idx = vert_key_to_topology_index(mesh);

    // Collect the selected edges as undirected topology vert-index pairs.
    let mut selected_pairs: HashSet<(u32, u32)> = HashSet::new();
    for ek in edges {
        let Some(edge) = mesh.edges.get(*ek) else {
            continue;
        };
        let Some(&a) = key_to_idx.get(&edge.v[0]) else {
            continue;
        };
        let Some(&b) = key_to_idx.get(&edge.v[1]) else {
            continue;
        };
        selected_pairs.insert((a.min(b), a.max(b)));
    }

    // A face uses a selected edge when that edge's two verts are consecutive
    // in the face ring.
    let mut removed_polys: HashSet<usize> = HashSet::new();
    for (face_idx, poly) in topology.polygons.iter().enumerate() {
        let start = poly.loop_start as usize;
        let total = poly.loop_total as usize;
        let ring = &topology.loops[start..start + total];
        for i in 0..total {
            let a = ring[i].vert;
            let b = ring[(i + 1) % total].vert;
            if selected_pairs.contains(&(a.min(b), a.max(b))) {
                removed_polys.insert(face_idx);
                break;
            }
        }
    }

    rebuild_without(mesh, &topology, &removed_polys, &HashSet::new())
}

/// Delete the given faces (polygons), then drop verts left loose. Verts and
/// edges still shared with a surviving face are kept. `faces` are `FaceKey`s
/// into `mesh`.
pub fn delete_faces(mesh: &mut HalfedgeMesh, faces: &[FaceKey]) -> DeleteResult {
    let topology = mesh.flatten_to_topology();

    // flatten_to_topology orders polygons by material_idx; map each FaceKey to
    // its polygon index the same way.
    let mut sorted_faces: Vec<(FaceKey, u32)> = mesh
        .faces
        .iter()
        .map(|(k, f)| (k, f.material_idx))
        .collect();
    sorted_faces.sort_by_key(|(_, mat)| *mat);
    let mut key_to_poly: std::collections::HashMap<FaceKey, usize> =
        std::collections::HashMap::with_capacity(sorted_faces.len());
    for (poly_idx, (fk, _)) in sorted_faces.iter().enumerate() {
        key_to_poly.insert(*fk, poly_idx);
    }

    let mut removed_polys: HashSet<usize> = HashSet::new();
    for fk in faces {
        if let Some(&poly_idx) = key_to_poly.get(fk) {
            removed_polys.insert(poly_idx);
        }
    }

    rebuild_without(mesh, &topology, &removed_polys, &HashSet::new())
}

/// Rebuild `mesh` from `topology`, dropping the polygons in `removed_polys`,
/// the verts in `removed_verts`, and any vert left with no incident surviving
/// face. Edges follow surviving polygon rings; loose verts are kept only when
/// not explicitly removed (so an isolated selection still vanishes).
fn rebuild_without(
    mesh: &mut HalfedgeMesh,
    topology: &BrushTopology,
    removed_polys: &HashSet<usize>,
    removed_verts: &HashSet<u32>,
) -> DeleteResult {
    // Verts used by a surviving polygon.
    let mut used_verts: HashSet<u32> = HashSet::new();
    for (face_idx, poly) in topology.polygons.iter().enumerate() {
        if removed_polys.contains(&face_idx) {
            continue;
        }
        let start = poly.loop_start as usize;
        let total = poly.loop_total as usize;
        for lp in &topology.loops[start..start + total] {
            used_verts.insert(lp.vert);
        }
    }

    // Keep a vert when a surviving face still uses it and it was not explicitly
    // removed. Verts not used by any surviving face are dropped as loose.
    let keep_vert = |idx: u32| used_verts.contains(&idx) && !removed_verts.contains(&idx);

    // Old vert index -> new vert index, building the new vert list in order.
    let mut old_to_new_vert: Vec<Option<u32>> = vec![None; topology.vertices.len()];
    let mut new_vertices: Vec<MeshVert> = Vec::new();
    let mut removed_vert_count = 0usize;
    for (old_idx, vert) in topology.vertices.iter().enumerate() {
        if keep_vert(old_idx as u32) {
            old_to_new_vert[old_idx] = Some(new_vertices.len() as u32);
            new_vertices.push(*vert);
        } else {
            removed_vert_count += 1;
        }
    }

    // Surviving polygons, rebuilding edges and loops from their rings.
    let mut new_edges: Vec<MeshEdge> = Vec::new();
    let mut edge_lookup: std::collections::HashMap<(u32, u32), u32> =
        std::collections::HashMap::new();
    let mut new_polygons: Vec<MeshPoly> = Vec::new();
    let mut new_loops: Vec<MeshLoop> = Vec::new();
    let mut surviving_faces: Vec<usize> = Vec::new();

    for (face_idx, poly) in topology.polygons.iter().enumerate() {
        if removed_polys.contains(&face_idx) {
            continue;
        }
        let start = poly.loop_start as usize;
        let total = poly.loop_total as usize;
        let ring = &topology.loops[start..start + total];

        let loop_start = new_loops.len() as u32;
        for i in 0..total {
            let v0_old = ring[i].vert;
            let v1_old = ring[(i + 1) % total].vert;
            let v0 = old_to_new_vert[v0_old as usize].expect("ring vert kept");
            let v1 = old_to_new_vert[v1_old as usize].expect("ring vert kept");
            let pair = if v0 <= v1 { (v0, v1) } else { (v1, v0) };
            let edge_idx = *edge_lookup.entry(pair).or_insert_with(|| {
                let idx = new_edges.len() as u32;
                new_edges.push(MeshEdge {
                    v: [pair.0, pair.1],
                    flags: crate::EdgeFlag::empty(),
                });
                idx
            });
            new_loops.push(MeshLoop {
                vert: v0,
                edge: edge_idx,
            });
        }
        new_polygons.push(MeshPoly {
            loop_start,
            loop_total: total as u32,
        });
        surviving_faces.push(face_idx);
    }

    let removed_faces = removed_polys.len();

    let new_topology = BrushTopology {
        vertices: new_vertices,
        edges: new_edges,
        polygons: new_polygons,
        loops: new_loops,
        attributes: Default::default(),
    };
    *mesh = HalfedgeMesh::lift_from_topology(&new_topology);

    DeleteResult {
        removed_faces,
        removed_verts: removed_vert_count,
        surviving_faces,
    }
}

/// Map each `VertKey` to its topology vertex index. `flatten_to_topology`
/// indexes verts in `mesh.verts.iter()` order, so mirror that here.
fn vert_key_to_topology_index(mesh: &HalfedgeMesh) -> std::collections::HashMap<VertKey, u32> {
    let mut map = std::collections::HashMap::with_capacity(mesh.verts.len());
    for (idx, (k, _)) in mesh.verts.iter().enumerate() {
        map.insert(k, idx as u32);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_topology_from_face_polygons;
    use glam::Vec3;

    /// A unit cube: 8 corner verts, 6 quad faces. Each corner is shared by
    /// three faces and each edge by two, so the delete counts are fixed.
    fn unit_cube_mesh() -> HalfedgeMesh {
        let positions = vec![
            Vec3::new(-1.0, -1.0, -1.0),
            Vec3::new(1.0, -1.0, -1.0),
            Vec3::new(1.0, 1.0, -1.0),
            Vec3::new(-1.0, 1.0, -1.0),
            Vec3::new(-1.0, -1.0, 1.0),
            Vec3::new(1.0, -1.0, 1.0),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(-1.0, 1.0, 1.0),
        ];
        let faces = vec![
            vec![0, 3, 2, 1],
            vec![4, 5, 6, 7],
            vec![0, 1, 5, 4],
            vec![3, 7, 6, 2],
            vec![0, 4, 7, 3],
            vec![1, 2, 6, 5],
        ];
        let topology = build_topology_from_face_polygons(positions, faces);
        HalfedgeMesh::lift_from_topology(&topology)
    }

    #[test]
    fn delete_vert_destroys_its_three_faces_and_leaves_a_hole() {
        let mut mesh = unit_cube_mesh();
        assert_eq!(mesh.faces.len(), 6);
        let vk = mesh.verts.keys().next().unwrap();
        let result = delete_verts(&mut mesh, &[vk]);
        // A corner is shared by three faces; deleting it destroys exactly
        // those three (an open result), rather than re-closing into a solid.
        assert_eq!(result.removed_faces, 3);
        assert_eq!(mesh.faces.len(), 3);
        // The corner is gone; the other seven stay used by the survivors.
        assert_eq!(result.removed_verts, 1);
        assert_eq!(mesh.verts.len(), 7);
    }

    #[test]
    fn delete_face_removes_only_that_polygon() {
        let mut mesh = unit_cube_mesh();
        let fk = mesh.faces.keys().next().unwrap();
        let result = delete_faces(&mut mesh, &[fk]);
        assert_eq!(result.removed_faces, 1);
        assert_eq!(mesh.faces.len(), 5);
        // Every cube vert is shared by three faces, so none becomes loose.
        assert_eq!(mesh.verts.len(), 8);
    }

    #[test]
    fn delete_edge_removes_its_two_faces() {
        let mut mesh = unit_cube_mesh();
        let ek = mesh.edges.keys().next().unwrap();
        let result = delete_edges(&mut mesh, &[ek]);
        // An edge borders exactly two faces; both are destroyed.
        assert_eq!(result.removed_faces, 2);
        assert_eq!(mesh.faces.len(), 4);
        // Endpoints remain used by their third face, so no vert is dropped.
        assert_eq!(mesh.verts.len(), 8);
    }
}
