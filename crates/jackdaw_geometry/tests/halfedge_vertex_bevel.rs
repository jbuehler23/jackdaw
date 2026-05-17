use bevy::math::Vec3;
use jackdaw_geometry::halfedge::{
    HalfedgeMesh,
    ops::vertex_bevel::{VertexBevelError, vertex_bevel},
};
use jackdaw_jsn::Brush;

#[test]
fn vertex_bevel_cube_corner_creates_triangle_face() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let initial_verts = mesh.vert_count();
    let initial_edges = mesh.edge_count();
    let initial_faces = mesh.face_count();

    // Pick any vertex; on a cube every vertex is a degree-3 corner.
    let vert = mesh
        .verts
        .keys()
        .next()
        .expect("cube has at least one vert");
    let result = vertex_bevel(&mut mesh, vert, 0.2).expect("bevel one cube corner");

    // 1 new bevel face; 3 new offset verts (degree 3).
    assert_eq!(result.new_verts.len(), 3, "degree-3 corner: 3 offset verts");

    // Cube: 8 -> 10 verts (-1 old corner, +3 offsets).
    assert_eq!(
        mesh.vert_count(),
        10,
        "cube vertex bevel: 8 -> 10 verts (got {})",
        mesh.vert_count()
    );
    let _ = initial_verts;

    // Cube: 12 -> 15 edges (-3 old incident edges, +3 boundary edges; the
    // rebuilt face's "shared" edges are the boundary edges).
    assert_eq!(
        mesh.edge_count(),
        15,
        "cube vertex bevel: 12 -> 15 edges (got {})",
        mesh.edge_count()
    );
    let _ = initial_edges;

    // Cube: 6 -> 7 faces (+1 bevel face).
    assert_eq!(
        mesh.face_count(),
        7,
        "cube vertex bevel: 6 -> 7 faces (got {})",
        mesh.face_count()
    );
    assert_eq!(mesh.face_count(), initial_faces + 1);

    // Bevel face is a triangle for a cube corner (degree 3).
    let bevel_face = &mesh.faces[result.new_face];
    assert_eq!(
        bevel_face.loop_count, 3,
        "cube-corner bevel face is a triangle"
    );

    mesh.validate()
        .expect("HalfedgeMesh invariants hold after vertex bevel");
}

#[test]
fn vertex_bevel_zero_width_errors() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let vert = mesh.verts.keys().next().unwrap();
    let err = vertex_bevel(&mut mesh, vert, 0.0).expect_err("zero width is rejected");
    assert!(matches!(err, VertexBevelError::WidthTooSmall));
}

#[test]
fn vertex_bevel_normal_faces_outward() {
    let brush = Brush::cuboid(2.0, 2.0, 2.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let vert = mesh
        .verts
        .keys()
        .next()
        .expect("cube has at least one vert");

    // Capture each adjacent face normal BEFORE the bevel so we can compute
    // the original outward direction at this corner.
    use jackdaw_geometry::halfedge::cycles::{disk_walk, radial_walk};
    use std::collections::HashSet;
    let mut adj_faces: HashSet<jackdaw_geometry::halfedge::FaceKey> = HashSet::new();
    for e in disk_walk(&mesh, vert).collect::<Vec<_>>() {
        for lp in radial_walk(&mesh, e).collect::<Vec<_>>() {
            adj_faces.insert(mesh.loops[lp].face);
        }
    }
    let mut outward = Vec3::ZERO;
    for &fk in &adj_faces {
        outward += mesh.faces[fk].normal_cache;
    }
    let outward = outward.normalize_or_zero();
    assert!(
        outward.length_squared() > 0.5,
        "outward dir should be well-defined for a cube corner"
    );

    let result = vertex_bevel(&mut mesh, vert, 0.3).expect("bevel one cube corner");
    let normal = mesh.faces[result.new_face].normal_cache;
    let alignment = normal.dot(outward);
    assert!(
        alignment > 0.0,
        "bevel face normal should align with outward direction: dot = {alignment}"
    );
}

#[test]
fn vertex_bevel_triangle_is_planar() {
    // For a cube corner where the 3 incident edges are mutually perpendicular
    // and equal length, the 3 offset verts form an equilateral triangle.
    // Verify planarity (scalar triple product < 1e-4) and equal edge lengths.
    let brush = Brush::cuboid(2.0, 2.0, 2.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let vert = mesh
        .verts
        .keys()
        .next()
        .expect("cube has at least one vert");
    let result = vertex_bevel(&mut mesh, vert, 0.25).expect("bevel one cube corner");

    assert_eq!(result.new_verts.len(), 3, "cube corner -> 3 offset verts");
    let a = mesh.verts[result.new_verts[0]].co;
    let b = mesh.verts[result.new_verts[1]].co;
    let c = mesh.verts[result.new_verts[2]].co;

    let e_ab = b - a;
    let e_bc = c - b;
    let e_ca = a - c;

    // Equilateral check: all three edges equal length to within 1e-4.
    let len_ab = e_ab.length();
    let len_bc = e_bc.length();
    let len_ca = e_ca.length();
    assert!(
        (len_ab - len_bc).abs() < 1e-4,
        "|a-b| ({len_ab}) should equal |b-c| ({len_bc})"
    );
    assert!(
        (len_bc - len_ca).abs() < 1e-4,
        "|b-c| ({len_bc}) should equal |c-a| ({len_ca})"
    );

    // Planarity: 3 points are always planar; check the bevel face is a
    // triangle (loop_count == 3) and scalar triple of edge vectors is zero.
    // For a triangle, the scalar triple of e_ab, e_bc, e_ca is structurally
    // zero (they sum to zero), but verify by computing.
    let cross = e_ab.cross(e_bc);
    let triple = cross.dot(e_ca).abs();
    assert!(
        triple < 1e-4,
        "triangle scalar triple should be ~0: {triple}"
    );
}

#[test]
fn vertex_bevel_preserves_adjacent_face_count() {
    let brush = Brush::cuboid(2.0, 2.0, 2.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let initial_faces = mesh.face_count();

    let vert = mesh
        .verts
        .keys()
        .next()
        .expect("cube has at least one vert");

    // Snapshot the material_idx of every adjacent face before the bevel.
    use jackdaw_geometry::halfedge::cycles::{disk_walk, radial_walk};
    use std::collections::HashSet;
    let mut adj_face_mats: Vec<u32> = Vec::new();
    let mut seen: HashSet<jackdaw_geometry::halfedge::FaceKey> = HashSet::new();
    for e in disk_walk(&mesh, vert).collect::<Vec<_>>() {
        for lp in radial_walk(&mesh, e).collect::<Vec<_>>() {
            let fk = mesh.loops[lp].face;
            if seen.insert(fk) {
                adj_face_mats.push(mesh.faces[fk].material_idx);
            }
        }
    }
    assert_eq!(adj_face_mats.len(), 3, "cube corner has 3 adjacent faces");

    let _result = vertex_bevel(&mut mesh, vert, 0.2).expect("bevel one cube corner");

    // After the op:
    //  - face count is original + 1 (the bevel face).
    //  - every original material_idx still exists on some face (rebuilt).
    assert_eq!(
        mesh.face_count(),
        initial_faces + 1,
        "face count grows by exactly 1"
    );
    let post_mats: HashSet<u32> = mesh.faces.values().map(|f| f.material_idx).collect();
    for m in &adj_face_mats {
        assert!(
            post_mats.contains(m),
            "rebuilt adjacent face should preserve material_idx {m}"
        );
    }
}
