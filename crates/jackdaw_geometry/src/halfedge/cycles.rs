//! Disk and radial cycle maintenance for HalfedgeMesh.
//!
//! Disk cycle: doubly-linked ring of edges around a vertex.
//! Radial cycle: doubly-linked ring of loops around an edge (one per incident face).
//!
//! These cycles give half-edge meshes O(1) adjacency walks. All HalfedgeMesh
//! mutations that add or remove edges / loops MUST keep the cycles consistent.

use crate::halfedge::types::*;

/// 0 if `vert == edge.v[0]`, 1 if `vert == edge.v[1]`. Panics otherwise (invariant violation).
fn disk_side(edge: &HalfedgeEdge, vert: VertKey) -> usize {
    if edge.v[0] == vert {
        0
    } else if edge.v[1] == vert {
        1
    } else {
        panic!("disk_side: vert {:?} not in edge {:?}", vert, edge.v);
    }
}

/// Insert `e` into the disk cycle of both its vertices.
pub fn disk_insert_edge(mesh: &mut HalfedgeMesh, e: EdgeKey) {
    for side in 0..2 {
        let v_key = mesh.edges[e].v[side];
        match mesh.verts[v_key].edge {
            None => {
                mesh.verts[v_key].edge = Some(e);
                mesh.edges[e].disk_next[side] = e;
                mesh.edges[e].disk_prev[side] = e;
            }
            Some(first) => {
                let first_side = disk_side(&mesh.edges[first], v_key);
                let prev = mesh.edges[first].disk_prev[first_side];
                let prev_side = disk_side(&mesh.edges[prev], v_key);
                mesh.edges[e].disk_next[side] = first;
                mesh.edges[e].disk_prev[side] = prev;
                mesh.edges[first].disk_prev[first_side] = e;
                mesh.edges[prev].disk_next[prev_side] = e;
            }
        }
    }
}

/// Remove `e` from the disk cycle of both its vertices.
pub fn disk_remove_edge(mesh: &mut HalfedgeMesh, e: EdgeKey) {
    for side in 0..2 {
        let v_key = mesh.edges[e].v[side];
        let next = mesh.edges[e].disk_next[side];
        let prev = mesh.edges[e].disk_prev[side];
        if next == e {
            mesh.verts[v_key].edge = None;
        } else {
            let next_side = disk_side(&mesh.edges[next], v_key);
            let prev_side = disk_side(&mesh.edges[prev], v_key);
            mesh.edges[next].disk_prev[next_side] = prev;
            mesh.edges[prev].disk_next[prev_side] = next;
            if mesh.verts[v_key].edge == Some(e) {
                mesh.verts[v_key].edge = Some(next);
            }
        }
    }
}

/// Walk the disk cycle around `v`, yielding each incident edge once.
pub fn disk_walk(mesh: &HalfedgeMesh, v: VertKey) -> impl Iterator<Item = EdgeKey> + '_ {
    let first = mesh.verts[v].edge;
    DiskWalk {
        mesh,
        v,
        first,
        current: first,
        done: false,
    }
}

struct DiskWalk<'a> {
    mesh: &'a HalfedgeMesh,
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
        let side = disk_side(&self.mesh.edges[cur], self.v);
        let next = self.mesh.edges[cur].disk_next[side];
        self.current = Some(next);
        if Some(next) == self.first {
            self.done = true;
        }
        Some(cur)
    }
}

/// Insert `lp` into the radial cycle of its edge.
pub fn radial_insert_loop(mesh: &mut HalfedgeMesh, lp: LoopKey) {
    let e = mesh.loops[lp].edge;
    match mesh.edges[e].loop_first {
        None => {
            mesh.edges[e].loop_first = Some(lp);
            mesh.loops[lp].radial_next = lp;
            mesh.loops[lp].radial_prev = lp;
        }
        Some(first) => {
            let prev = mesh.loops[first].radial_prev;
            mesh.loops[lp].radial_next = first;
            mesh.loops[lp].radial_prev = prev;
            mesh.loops[first].radial_prev = lp;
            mesh.loops[prev].radial_next = lp;
        }
    }
}

/// Remove `lp` from the radial cycle of its edge.
pub fn radial_remove_loop(mesh: &mut HalfedgeMesh, lp: LoopKey) {
    let e = mesh.loops[lp].edge;
    let next = mesh.loops[lp].radial_next;
    let prev = mesh.loops[lp].radial_prev;
    if next == lp {
        mesh.edges[e].loop_first = None;
    } else {
        mesh.loops[next].radial_prev = prev;
        mesh.loops[prev].radial_next = next;
        if mesh.edges[e].loop_first == Some(lp) {
            mesh.edges[e].loop_first = Some(next);
        }
    }
}

/// Walk the radial cycle of `e`, yielding each incident loop once.
pub fn radial_walk(mesh: &HalfedgeMesh, e: EdgeKey) -> impl Iterator<Item = LoopKey> + '_ {
    let first = mesh.edges[e].loop_first;
    RadialWalk {
        mesh,
        first,
        current: first,
        done: false,
    }
}

struct RadialWalk<'a> {
    mesh: &'a HalfedgeMesh,
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
        let next = self.mesh.loops[cur].radial_next;
        self.current = Some(next);
        if Some(next) == self.first {
            self.done = true;
        }
        Some(cur)
    }
}
