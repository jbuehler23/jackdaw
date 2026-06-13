use jackdaw_geometry::halfedge::HalfedgeMesh;
use jackdaw_geometry::halfedge::ops::delete_elements::{delete_edges, delete_faces, delete_verts};
use jackdaw_jsn::Brush;

/// A cube corner is used by 3 faces; deleting it must drop those 3 faces and
/// the vert, leaving an open mesh that is not re-closed.
#[test]
fn delete_corner_vert_removes_three_faces_and_drops_vert() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let initial_verts = mesh.vert_count();

    let corner = mesh.verts.keys().next().unwrap();
    let result = delete_verts(&mut mesh, &[corner]);

    assert_eq!(
        mesh.face_count(),
        3,
        "3 faces remain after deleting a corner"
    );
    assert_eq!(result.removed_faces, 3);
    assert_eq!(
        mesh.vert_count(),
        initial_verts - 1,
        "the corner vert is gone"
    );
    assert_eq!(mesh.surviving_check(&result), 3);
    mesh.validate().expect("valid after vert delete");
}

/// Deleting all 4 verts of one face removes that face plus the 4 side faces
/// touching them, leaving only the opposite face.
#[test]
fn delete_face_ring_verts_removes_five_faces() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);

    // Face 4 of the cuboid is the +Z cap; its ring is verts 4,5,6,7.
    let mut keys = Vec::new();
    for &target in &[4u32, 5, 6, 7] {
        let (k, _) = mesh.verts.iter().nth(target as usize).unwrap();
        keys.push(k);
    }
    let result = delete_verts(&mut mesh, &keys);

    assert_eq!(mesh.face_count(), 1, "only the opposite face survives");
    assert_eq!(result.removed_faces, 5);
    assert_eq!(mesh.vert_count(), 4, "the 4 deleted verts are gone");
    mesh.validate().expect("valid after multi-vert delete");
}

/// Deleting one face removes exactly that polygon and keeps every still-used
/// vert.
#[test]
fn delete_one_face_removes_only_that_polygon() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let initial_verts = mesh.vert_count();

    let face = mesh.faces.keys().next().unwrap();
    let result = delete_faces(&mut mesh, &[face]);

    assert_eq!(mesh.face_count(), 5, "exactly one polygon removed");
    assert_eq!(result.removed_faces, 1);
    assert_eq!(result.removed_verts, 0, "no vert becomes loose");
    assert_eq!(mesh.vert_count(), initial_verts, "all verts still used");
    mesh.validate().expect("valid after face delete");
}

/// Deleting every face drops every vert as loose, leaving an empty mesh.
#[test]
fn delete_all_faces_leaves_empty_mesh() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let all: Vec<_> = mesh.faces.keys().collect();
    let result = delete_faces(&mut mesh, &all);

    assert_eq!(mesh.face_count(), 0);
    assert_eq!(
        mesh.vert_count(),
        0,
        "no surviving face means no kept verts"
    );
    assert_eq!(result.removed_faces, 6);
    assert_eq!(result.removed_verts, 8);
}

/// Deleting an edge removes the faces using it (each cube edge is shared by 2
/// faces).
#[test]
fn delete_edge_removes_two_adjacent_faces() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let initial_verts = mesh.vert_count();

    let edge = mesh.edges.keys().next().unwrap();
    let result = delete_edges(&mut mesh, &[edge]);

    assert_eq!(
        mesh.face_count(),
        4,
        "the 2 faces sharing the edge are gone"
    );
    assert_eq!(result.removed_faces, 2);
    // The edge's endpoints stay used by the 4 remaining faces, so no vert drops.
    assert_eq!(mesh.vert_count(), initial_verts, "endpoints survive");
    mesh.validate().expect("valid after edge delete");
}

/// Surviving faces are reported parallel to the rebuilt polygons, in ascending
/// original order, so callers can re-index per-face data.
trait SurvivingCheck {
    fn surviving_check(
        &self,
        result: &jackdaw_geometry::halfedge::ops::delete_elements::DeleteResult,
    ) -> usize;
}

impl SurvivingCheck for HalfedgeMesh {
    fn surviving_check(
        &self,
        result: &jackdaw_geometry::halfedge::ops::delete_elements::DeleteResult,
    ) -> usize {
        assert_eq!(
            result.surviving_faces.len(),
            self.face_count(),
            "surviving_faces parallel to polygons"
        );
        assert!(
            result.surviving_faces.windows(2).all(|w| w[0] < w[1]),
            "surviving_faces ascending"
        );
        result.surviving_faces.len()
    }
}
