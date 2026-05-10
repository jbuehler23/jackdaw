use bevy::math::Vec3;
use jackdaw_geometry::editmesh::{
    EditMesh, EditEdge, EdgeFlag, EdgeKey,
    cycles::{disk_insert_edge, disk_walk},
};

#[test]
fn single_vert_with_one_incident_edge_disk_walk_returns_just_that_edge() {
    let mut m = EditMesh::default();
    let v0 = m.add_vert(Vec3::ZERO);
    let v1 = m.add_vert(Vec3::X);
    let e = m.edges.insert(EditEdge {
        v: [v0, v1],
        flag: EdgeFlag::empty(),
        loop_first: None,
        disk_next: [EdgeKey::default(); 2],
        disk_prev: [EdgeKey::default(); 2],
    });
    disk_insert_edge(&mut m, e);
    let edges_at_v0: Vec<_> = disk_walk(&m, v0).collect();
    assert_eq!(edges_at_v0, vec![e]);
}

#[test]
fn vert_with_three_incident_edges_disk_walk_returns_all_three() {
    let mut m = EditMesh::default();
    let center = m.add_vert(Vec3::ZERO);
    let a = m.add_vert(Vec3::X);
    let b = m.add_vert(Vec3::Y);
    let c = m.add_vert(Vec3::Z);
    let add = |bm: &mut EditMesh, x, y| {
        let e = bm.edges.insert(EditEdge {
            v: [x, y],
            flag: EdgeFlag::empty(),
            loop_first: None,
            disk_next: [EdgeKey::default(); 2],
            disk_prev: [EdgeKey::default(); 2],
        });
        disk_insert_edge(bm, e);
        e
    };
    let e1 = add(&mut m, center, a);
    let e2 = add(&mut m, center, b);
    let e3 = add(&mut m, center, c);
    let edges_at_center: std::collections::HashSet<_> = disk_walk(&m, center).collect();
    assert_eq!(edges_at_center, [e1, e2, e3].iter().copied().collect());
}
