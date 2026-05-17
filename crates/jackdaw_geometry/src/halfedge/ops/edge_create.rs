//! Idempotent edge creation. Returns existing edge if one already connects
//! `va` and `vb`; otherwise creates one and installs in disk cycles.

use crate::halfedge::cycles::disk_insert_edge;
use crate::halfedge::types::*;

pub fn create_edge(mesh: &mut HalfedgeMesh, va: VertKey, vb: VertKey) -> EdgeKey {
    // Check existence first.
    for (k, e) in mesh.edges.iter() {
        if (e.v[0] == va && e.v[1] == vb) || (e.v[0] == vb && e.v[1] == va) {
            return k;
        }
    }
    // Create new.
    let v = if va < vb { [va, vb] } else { [vb, va] };
    let e = mesh.edges.insert(HalfedgeEdge {
        v,
        flag: EdgeFlag::empty(),
        loop_first: None,
        disk_next: [EdgeKey::default(); 2],
        disk_prev: [EdgeKey::default(); 2],
    });
    disk_insert_edge(mesh, e);
    e
}
