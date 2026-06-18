use glam::Vec3;
use jackdaw_geometry::halfedge::{HalfedgeBinding, apply_topology_edit};
use jackdaw_geometry::{BrushFaceData, BrushTopology, compute_brush_topology, cuboid_faces};

fn cube_topology() -> BrushTopology {
    compute_brush_topology(&cuboid_faces(Vec3::splat(1.0)))
}

#[test]
fn no_op_edit_leaves_topology_and_face_count_stable() {
    let topo = cube_topology();
    let face_count = topo.polygons.len();
    let mut faces = vec![BrushFaceData::default(); face_count];
    let mut binding = HalfedgeBinding::lift_from_topology(&topo);
    let mut topology = topo;

    apply_topology_edit(&mut faces, &mut topology, &mut binding, |_mesh| {});

    assert_eq!(faces.len(), face_count, "no-op preserves face count");
    assert_eq!(topology.polygons.len(), face_count);
    for f in &faces {
        assert!(f.plane.normal.is_finite() && (f.plane.normal.length() - 1.0).abs() < 1e-4);
    }
}

#[test]
fn edit_that_removes_a_face_keeps_face_data_and_keys_in_sync() {
    let topo = cube_topology();
    let before = topo.polygons.len();
    let mut faces = vec![BrushFaceData::default(); before];
    let mut binding = HalfedgeBinding::lift_from_topology(&topo);
    let mut topology = topo;

    let ek = binding.mesh.edges.keys().next().expect("an edge exists");
    apply_topology_edit(&mut faces, &mut topology, &mut binding, |mesh| {
        let _ = jackdaw_geometry::halfedge::ops::dissolve_edges::dissolve_edges(mesh, &[ek]);
    });

    assert_eq!(
        faces.len(),
        topology.polygons.len(),
        "face data tracks polygon count"
    );
    assert_eq!(
        binding.face_keys.len(),
        topology.polygons.len(),
        "keys re-lifted"
    );
}
