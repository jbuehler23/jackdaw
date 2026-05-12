//! Bridge two edge loops with a quad strip. Each loop must be a closed cycle
//! with the same vertex count. Pairs verts in walk order (no rotation
//! optimization in MVP); generates N quad faces.

use std::collections::{HashMap, HashSet};

use crate::editmesh::ops::face_create::create_face_from_verts;
use crate::editmesh::types::*;

#[derive(Debug)]
pub enum BridgeError {
    NotAClosedLoop,
    UnequalVertexCounts,
    EmptyInput,
    FaceCreate,
}

pub struct BridgeResult {
    pub new_faces: Vec<FaceKey>,
    pub new_edges: Vec<EdgeKey>,
}

pub fn bridge_edge_loops(
    bmesh: &mut EditMesh,
    edges_a: &[EdgeKey],
    edges_b: &[EdgeKey],
) -> Result<BridgeResult, BridgeError> {
    if edges_a.is_empty() || edges_b.is_empty() {
        return Err(BridgeError::EmptyInput);
    }
    let ring_a = walk_edges_to_ring(bmesh, edges_a)?;
    let ring_b = walk_edges_to_ring(bmesh, edges_b)?;
    if ring_a.len() != ring_b.len() {
        return Err(BridgeError::UnequalVertexCounts);
    }
    let n = ring_a.len();
    let edges_before: HashSet<EdgeKey> = bmesh.edges.keys().collect();
    let mut new_faces: Vec<FaceKey> = Vec::with_capacity(n);
    for i in 0..n {
        let i_next = (i + 1) % n;
        let quad = [ring_a[i], ring_a[i_next], ring_b[i_next], ring_b[i]];
        let face = create_face_from_verts(bmesh, &quad).map_err(|_| BridgeError::FaceCreate)?;
        new_faces.push(face);
    }
    let new_edges: Vec<EdgeKey> = bmesh
        .edges
        .keys()
        .filter(|k| !edges_before.contains(k))
        .collect();
    Ok(BridgeResult {
        new_faces,
        new_edges,
    })
}

/// Given a list of edges, walk them to produce an ordered ring of vertex keys.
/// Errors if the edges don't form a single closed loop.
fn walk_edges_to_ring(bmesh: &EditMesh, edges: &[EdgeKey]) -> Result<Vec<VertKey>, BridgeError> {
    let edge_set: HashSet<EdgeKey> = edges.iter().copied().collect();
    if edge_set.len() != edges.len() {
        return Err(BridgeError::NotAClosedLoop); // dups
    }
    // Build a vert -> [edges] adjacency for this edge subset.
    let mut adj: HashMap<VertKey, Vec<EdgeKey>> = HashMap::new();
    for &e in edges {
        let edge = &bmesh.edges[e];
        adj.entry(edge.v[0]).or_default().push(e);
        adj.entry(edge.v[1]).or_default().push(e);
    }
    // For a closed loop, each vert appears in exactly 2 edges.
    if adj.values().any(|v| v.len() != 2) {
        return Err(BridgeError::NotAClosedLoop);
    }
    // Walk: start at any vert, follow edges until we return to start.
    let start_vert: VertKey = *adj.keys().next().ok_or(BridgeError::EmptyInput)?;
    let mut ring: Vec<VertKey> = Vec::with_capacity(edges.len());
    let mut visited_edges: HashSet<EdgeKey> = HashSet::new();
    let mut cur_vert = start_vert;
    loop {
        ring.push(cur_vert);
        let next_edge = *adj[&cur_vert]
            .iter()
            .find(|e| !visited_edges.contains(e))
            .ok_or(BridgeError::NotAClosedLoop)?;
        visited_edges.insert(next_edge);
        let edge = &bmesh.edges[next_edge];
        let other = if edge.v[0] == cur_vert {
            edge.v[1]
        } else {
            edge.v[0]
        };
        if other == start_vert {
            break;
        }
        cur_vert = other;
    }
    if ring.len() != edges.len() {
        return Err(BridgeError::NotAClosedLoop);
    }
    Ok(ring)
}
