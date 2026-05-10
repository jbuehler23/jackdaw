//! Disk and radial cycle maintenance for EditMesh.
//!
//! Disk cycle: doubly-linked ring of edges around a vertex.
//! Radial cycle: doubly-linked ring of loops around an edge (one per incident face).
//!
//! These cycles give half-edge meshes O(1) adjacency walks. All EditMesh
//! mutations that add or remove edges / loops MUST keep the cycles consistent.

use crate::editmesh::types::*;

/// 0 if `vert == edge.v[0]`, 1 if `vert == edge.v[1]`. Panics otherwise (invariant violation).
fn disk_side(edge: &EditEdge, vert: VertKey) -> usize {
    if edge.v[0] == vert {
        0
    } else if edge.v[1] == vert {
        1
    } else {
        panic!("disk_side: vert {:?} not in edge {:?}", vert, edge.v);
    }
}

/// Insert `e` into the disk cycle of both its vertices.
pub fn disk_insert_edge(bmesh: &mut EditMesh, e: EdgeKey) {
    for side in 0..2 {
        let v_key = bmesh.edges[e].v[side];
        match bmesh.verts[v_key].edge {
            None => {
                bmesh.verts[v_key].edge = Some(e);
                bmesh.edges[e].disk_next[side] = e;
                bmesh.edges[e].disk_prev[side] = e;
            }
            Some(first) => {
                let first_side = disk_side(&bmesh.edges[first], v_key);
                let prev = bmesh.edges[first].disk_prev[first_side];
                let prev_side = disk_side(&bmesh.edges[prev], v_key);
                bmesh.edges[e].disk_next[side] = first;
                bmesh.edges[e].disk_prev[side] = prev;
                bmesh.edges[first].disk_prev[first_side] = e;
                bmesh.edges[prev].disk_next[prev_side] = e;
            }
        }
    }
}

/// Remove `e` from the disk cycle of both its vertices.
pub fn disk_remove_edge(bmesh: &mut EditMesh, e: EdgeKey) {
    for side in 0..2 {
        let v_key = bmesh.edges[e].v[side];
        let next = bmesh.edges[e].disk_next[side];
        let prev = bmesh.edges[e].disk_prev[side];
        if next == e {
            bmesh.verts[v_key].edge = None;
        } else {
            let next_side = disk_side(&bmesh.edges[next], v_key);
            let prev_side = disk_side(&bmesh.edges[prev], v_key);
            bmesh.edges[next].disk_prev[next_side] = prev;
            bmesh.edges[prev].disk_next[prev_side] = next;
            if bmesh.verts[v_key].edge == Some(e) {
                bmesh.verts[v_key].edge = Some(next);
            }
        }
    }
}

/// Walk the disk cycle around `v`, yielding each incident edge once.
pub fn disk_walk(bmesh: &EditMesh, v: VertKey) -> impl Iterator<Item = EdgeKey> + '_ {
    let first = bmesh.verts[v].edge;
    DiskWalk { bmesh, v, first, current: first, done: false }
}

struct DiskWalk<'a> {
    bmesh: &'a EditMesh,
    v: VertKey,
    first: Option<EdgeKey>,
    current: Option<EdgeKey>,
    done: bool,
}

impl<'a> Iterator for DiskWalk<'a> {
    type Item = EdgeKey;
    fn next(&mut self) -> Option<EdgeKey> {
        if self.done {
            return None;
        }
        let cur = self.current?;
        let side = disk_side(&self.bmesh.edges[cur], self.v);
        let next = self.bmesh.edges[cur].disk_next[side];
        self.current = Some(next);
        if Some(next) == self.first {
            self.done = true;
        }
        Some(cur)
    }
}

/// Insert `lp` into the radial cycle of its edge.
pub fn radial_insert_loop(bmesh: &mut EditMesh, lp: LoopKey) {
    let e = bmesh.loops[lp].edge;
    match bmesh.edges[e].loop_first {
        None => {
            bmesh.edges[e].loop_first = Some(lp);
            bmesh.loops[lp].radial_next = lp;
            bmesh.loops[lp].radial_prev = lp;
        }
        Some(first) => {
            let prev = bmesh.loops[first].radial_prev;
            bmesh.loops[lp].radial_next = first;
            bmesh.loops[lp].radial_prev = prev;
            bmesh.loops[first].radial_prev = lp;
            bmesh.loops[prev].radial_next = lp;
        }
    }
}

/// Remove `lp` from the radial cycle of its edge.
pub fn radial_remove_loop(bmesh: &mut EditMesh, lp: LoopKey) {
    let e = bmesh.loops[lp].edge;
    let next = bmesh.loops[lp].radial_next;
    let prev = bmesh.loops[lp].radial_prev;
    if next == lp {
        bmesh.edges[e].loop_first = None;
    } else {
        bmesh.loops[next].radial_prev = prev;
        bmesh.loops[prev].radial_next = next;
        if bmesh.edges[e].loop_first == Some(lp) {
            bmesh.edges[e].loop_first = Some(next);
        }
    }
}

/// Walk the radial cycle of `e`, yielding each incident loop once.
pub fn radial_walk(bmesh: &EditMesh, e: EdgeKey) -> impl Iterator<Item = LoopKey> + '_ {
    let first = bmesh.edges[e].loop_first;
    RadialWalk { bmesh, first, current: first, done: false }
}

struct RadialWalk<'a> {
    bmesh: &'a EditMesh,
    first: Option<LoopKey>,
    current: Option<LoopKey>,
    done: bool,
}

impl<'a> Iterator for RadialWalk<'a> {
    type Item = LoopKey;
    fn next(&mut self) -> Option<LoopKey> {
        if self.done {
            return None;
        }
        let cur = self.current?;
        let next = self.bmesh.loops[cur].radial_next;
        self.current = Some(next);
        if Some(next) == self.first {
            self.done = true;
        }
        Some(cur)
    }
}
