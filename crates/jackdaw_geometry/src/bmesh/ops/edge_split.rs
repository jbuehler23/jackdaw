//! Split an edge at parametric position `t`. Inserts a new vertex on the
//! edge, relabels the edge to (v[0], new_vert), creates a new edge
//! (new_vert, v[1]), and splits each incident loop into two. Disk + radial
//! cycles maintained.

use crate::bmesh::cycles::{disk_insert_edge, disk_remove_edge, radial_insert_loop, radial_remove_loop};
use crate::bmesh::types::*;

#[derive(Debug)]
pub enum SplitError {
    EdgeNotFound,
}

pub fn bm_edge_split(bmesh: &mut BMesh, edge: EdgeKey, t: f32) -> Result<VertKey, SplitError> {
    let Some(edge_data) = bmesh.edges.get(edge).cloned() else {
        return Err(SplitError::EdgeNotFound);
    };
    let v0 = edge_data.v[0];
    let v1 = edge_data.v[1];
    let pos0 = bmesh.verts[v0].co;
    let pos1 = bmesh.verts[v1].co;
    let new_pos = pos0.lerp(pos1, t);

    // 1. Add new vertex.
    let new_vert = bmesh.add_vert(new_pos);

    // 2. Relabel the original edge from (v0, v1) to (v0, new_vert).
    //    Remove from disk of both v0 and v1, change v[1], re-insert.
    //    After this: edge belongs to disk(v0) and disk(new_vert).
    disk_remove_edge(bmesh, edge);
    bmesh.edges[edge].v[1] = new_vert;
    disk_insert_edge(bmesh, edge);

    // 3. Create new edge (new_vert, v1) and insert into disk cycles.
    let new_edge = bmesh.edges.insert(BMEdge {
        v: [new_vert, v1],
        flag: edge_data.flag,
        loop_first: None,
        disk_next: [EdgeKey::default(); 2],
        disk_prev: [EdgeKey::default(); 2],
    });
    disk_insert_edge(bmesh, new_edge);

    // 4. Collect all loops currently on the original edge's radial cycle.
    //    We do this before any radial mutations.
    let incident_loops: Vec<LoopKey> = crate::bmesh::cycles::radial_walk(bmesh, edge).collect();

    // 5. For each incident loop, split it into two loops passing through new_vert.
    for lp_old in incident_loops {
        let face = bmesh.loops[lp_old].face;
        let next_lp = bmesh.loops[lp_old].next;
        let lp_old_vert = bmesh.loops[lp_old].vert;

        // lp_old walks: lp_old_vert -> next_lp.vert along `edge` (v0<->v1).
        // After the split:
        //   If lp_old_vert == v0: walk becomes v0 -> new_vert -> v1
        //     lp_old stays on `edge` (v0, new_vert), vert unchanged.
        //     lp_new: vert = new_vert, edge = new_edge (new_vert, v1).
        //   If lp_old_vert == v1: walk becomes v1 -> new_vert -> v0
        //     lp_old now walks v1 -> new_vert, which is new_edge reversed.
        //     lp_new: vert = new_vert, edge = `edge` (v0, new_vert) reversed.
        let lp_old_starts_at_v0 = lp_old_vert == v0;

        // Remove lp_old from the radial cycle before we create lp_new,
        // to keep the radial count clean while we mutate.
        radial_remove_loop(bmesh, lp_old);

        // Allocate lp_new and wire it into the face ring between lp_old and next_lp.
        let lp_new = bmesh.loops.insert(BMLoop {
            vert: new_vert,
            edge: EdgeKey::default(), // filled below
            face,
            next: next_lp,
            prev: lp_old,
            radial_next: LoopKey::default(),
            radial_prev: LoopKey::default(),
        });
        bmesh.loops[lp_old].next = lp_new;
        bmesh.loops[next_lp].prev = lp_new;

        // Assign edges and re-insert both into their radial cycles.
        if lp_old_starts_at_v0 {
            // lp_old: v0 -> new_vert  =>  edge (v0, new_vert)
            // lp_new: new_vert -> v1  =>  new_edge (new_vert, v1)
            bmesh.loops[lp_new].edge = new_edge;
            // lp_old.edge is already `edge`; no change needed.
            radial_insert_loop(bmesh, lp_old);
            radial_insert_loop(bmesh, lp_new);
        } else {
            // lp_old: v1 -> new_vert  =>  new_edge (new_vert, v1) traversed backward
            // lp_new: new_vert -> v0  =>  edge (v0, new_vert) traversed backward
            bmesh.loops[lp_old].edge = new_edge;
            bmesh.loops[lp_new].edge = edge;
            radial_insert_loop(bmesh, lp_old);
            radial_insert_loop(bmesh, lp_new);
        }

        bmesh.faces[face].loop_count += 1;
    }

    Ok(new_vert)
}
