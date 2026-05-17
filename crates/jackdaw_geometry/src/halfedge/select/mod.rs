//! Selection utilities for `HalfedgeMesh`. Selection state lives on element flags
//! (`VertFlag::SELECT`, `EdgeFlag::SELECT`, `FaceFlag::SELECT`).
//!
//! `SelectionDelta` records changes; `apply_*_delta` functions apply a delta
//! and return its inverse, suitable for stashing on an undo stack.

pub mod linked_walk;
pub mod loop_walk;
pub mod ring_walk;

use crate::halfedge::types::*;

#[derive(Clone, Debug, Default)]
pub struct SelectionDelta<K> {
    pub add: Vec<K>,
    pub remove: Vec<K>,
}

pub fn apply_vert_delta(
    mesh: &mut HalfedgeMesh,
    delta: &SelectionDelta<VertKey>,
) -> SelectionDelta<VertKey> {
    let mut inverse = SelectionDelta::<VertKey>::default();
    for &k in &delta.add {
        if let Some(v) = mesh.verts.get_mut(k)
            && !v.flag.contains(VertFlag::SELECT)
        {
            v.flag.insert(VertFlag::SELECT);
            inverse.remove.push(k);
        }
    }
    for &k in &delta.remove {
        if let Some(v) = mesh.verts.get_mut(k)
            && v.flag.contains(VertFlag::SELECT)
        {
            v.flag.remove(VertFlag::SELECT);
            inverse.add.push(k);
        }
    }
    inverse
}

pub fn apply_edge_delta(
    mesh: &mut HalfedgeMesh,
    delta: &SelectionDelta<EdgeKey>,
) -> SelectionDelta<EdgeKey> {
    let mut inverse = SelectionDelta::<EdgeKey>::default();
    for &k in &delta.add {
        if let Some(e) = mesh.edges.get_mut(k)
            && !e.flag.contains(EdgeFlag::SELECT)
        {
            e.flag.insert(EdgeFlag::SELECT);
            inverse.remove.push(k);
        }
    }
    for &k in &delta.remove {
        if let Some(e) = mesh.edges.get_mut(k)
            && e.flag.contains(EdgeFlag::SELECT)
        {
            e.flag.remove(EdgeFlag::SELECT);
            inverse.add.push(k);
        }
    }
    inverse
}

pub fn apply_face_delta(
    mesh: &mut HalfedgeMesh,
    delta: &SelectionDelta<FaceKey>,
) -> SelectionDelta<FaceKey> {
    let mut inverse = SelectionDelta::<FaceKey>::default();
    for &k in &delta.add {
        if let Some(f) = mesh.faces.get_mut(k)
            && !f.flag.contains(FaceFlag::SELECT)
        {
            f.flag.insert(FaceFlag::SELECT);
            inverse.remove.push(k);
        }
    }
    for &k in &delta.remove {
        if let Some(f) = mesh.faces.get_mut(k)
            && f.flag.contains(FaceFlag::SELECT)
        {
            f.flag.remove(FaceFlag::SELECT);
            inverse.add.push(k);
        }
    }
    inverse
}

/// Promote vertex selection to edge selection: an edge becomes selected iff both endpoints are.
pub fn flush_vert_to_edge(mesh: &mut HalfedgeMesh) {
    let to_select: Vec<EdgeKey> = mesh
        .edges
        .iter()
        .filter(|(_, e)| {
            mesh.verts[e.v[0]].flag.contains(VertFlag::SELECT)
                && mesh.verts[e.v[1]].flag.contains(VertFlag::SELECT)
        })
        .map(|(k, _)| k)
        .collect();
    for k in to_select {
        mesh.edges[k].flag.insert(EdgeFlag::SELECT);
    }
}

/// Promote edge selection to face selection: a face becomes selected iff all its boundary edges are.
pub fn flush_edge_to_face(mesh: &mut HalfedgeMesh) {
    let to_select: Vec<FaceKey> = mesh
        .faces
        .iter()
        .filter(|(_, face)| {
            let mut cur = face.loop_first;
            for _ in 0..face.loop_count {
                let edge = mesh.loops[cur].edge;
                if !mesh.edges[edge].flag.contains(EdgeFlag::SELECT) {
                    return false;
                }
                cur = mesh.loops[cur].next;
            }
            true
        })
        .map(|(k, _)| k)
        .collect();
    for k in to_select {
        mesh.faces[k].flag.insert(FaceFlag::SELECT);
    }
}
