//! Selection utilities for BMesh. Selection state lives on element flags
//! (`VertFlag::SELECT`, `EdgeFlag::SELECT`, `FaceFlag::SELECT`).
//!
//! `SelectionDelta` records changes; `apply_*_delta` functions apply a delta
//! and return its inverse, suitable for stashing on an undo stack.

use crate::bmesh::types::*;

#[derive(Clone, Debug, Default)]
pub struct SelectionDelta<K> {
    pub add: Vec<K>,
    pub remove: Vec<K>,
}

pub fn apply_vert_delta(bmesh: &mut BMesh, delta: &SelectionDelta<VertKey>) -> SelectionDelta<VertKey> {
    let mut inverse = SelectionDelta::<VertKey>::default();
    for &k in &delta.add {
        if let Some(v) = bmesh.verts.get_mut(k) {
            if !v.flag.contains(VertFlag::SELECT) {
                v.flag.insert(VertFlag::SELECT);
                inverse.remove.push(k);
            }
        }
    }
    for &k in &delta.remove {
        if let Some(v) = bmesh.verts.get_mut(k) {
            if v.flag.contains(VertFlag::SELECT) {
                v.flag.remove(VertFlag::SELECT);
                inverse.add.push(k);
            }
        }
    }
    inverse
}

pub fn apply_edge_delta(bmesh: &mut BMesh, delta: &SelectionDelta<EdgeKey>) -> SelectionDelta<EdgeKey> {
    let mut inverse = SelectionDelta::<EdgeKey>::default();
    for &k in &delta.add {
        if let Some(e) = bmesh.edges.get_mut(k) {
            if !e.flag.contains(EdgeFlag::SELECT) {
                e.flag.insert(EdgeFlag::SELECT);
                inverse.remove.push(k);
            }
        }
    }
    for &k in &delta.remove {
        if let Some(e) = bmesh.edges.get_mut(k) {
            if e.flag.contains(EdgeFlag::SELECT) {
                e.flag.remove(EdgeFlag::SELECT);
                inverse.add.push(k);
            }
        }
    }
    inverse
}

pub fn apply_face_delta(bmesh: &mut BMesh, delta: &SelectionDelta<FaceKey>) -> SelectionDelta<FaceKey> {
    let mut inverse = SelectionDelta::<FaceKey>::default();
    for &k in &delta.add {
        if let Some(f) = bmesh.faces.get_mut(k) {
            if !f.flag.contains(FaceFlag::SELECT) {
                f.flag.insert(FaceFlag::SELECT);
                inverse.remove.push(k);
            }
        }
    }
    for &k in &delta.remove {
        if let Some(f) = bmesh.faces.get_mut(k) {
            if f.flag.contains(FaceFlag::SELECT) {
                f.flag.remove(FaceFlag::SELECT);
                inverse.add.push(k);
            }
        }
    }
    inverse
}

/// Promote vertex selection to edge selection: an edge becomes selected iff both endpoints are.
pub fn flush_vert_to_edge(bmesh: &mut BMesh) {
    let to_select: Vec<EdgeKey> = bmesh.edges.iter()
        .filter(|(_, e)| {
            bmesh.verts[e.v[0]].flag.contains(VertFlag::SELECT) &&
            bmesh.verts[e.v[1]].flag.contains(VertFlag::SELECT)
        })
        .map(|(k, _)| k)
        .collect();
    for k in to_select {
        bmesh.edges[k].flag.insert(EdgeFlag::SELECT);
    }
}

/// Promote edge selection to face selection: a face becomes selected iff all its boundary edges are.
pub fn flush_edge_to_face(bmesh: &mut BMesh) {
    let to_select: Vec<FaceKey> = bmesh.faces.iter()
        .filter(|(_, face)| {
            let mut cur = face.loop_first;
            for _ in 0..face.loop_count {
                let edge = bmesh.loops[cur].edge;
                if !bmesh.edges[edge].flag.contains(EdgeFlag::SELECT) {
                    return false;
                }
                cur = bmesh.loops[cur].next;
            }
            true
        })
        .map(|(k, _)| k)
        .collect();
    for k in to_select {
        bmesh.faces[k].flag.insert(FaceFlag::SELECT);
    }
}
