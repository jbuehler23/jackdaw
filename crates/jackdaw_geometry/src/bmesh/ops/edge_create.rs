//! Idempotent edge creation. Returns existing edge if one already connects
//! `va` and `vb`; otherwise creates one and installs in disk cycles.

use crate::bmesh::cycles::disk_insert_edge;
use crate::bmesh::types::*;

pub fn bm_edge_create(bmesh: &mut BMesh, va: VertKey, vb: VertKey) -> EdgeKey {
    // Check existence first.
    for (k, e) in bmesh.edges.iter() {
        if (e.v[0] == va && e.v[1] == vb) || (e.v[0] == vb && e.v[1] == va) {
            return k;
        }
    }
    // Create new.
    let v = if va < vb { [va, vb] } else { [vb, va] };
    let e = bmesh.edges.insert(BMEdge {
        v,
        flag: EdgeFlag::empty(),
        loop_first: None,
        disk_next: [EdgeKey::default(); 2],
        disk_prev: [EdgeKey::default(); 2],
    });
    disk_insert_edge(bmesh, e);
    e
}
