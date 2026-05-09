use jackdaw_geometry::{
    AttributeStack, BrushTopology, EdgeFlag, MeshEdge, MeshLoop, MeshPoly, MeshVert,
};
use bevy::math::Vec3;

/// Build a unit cube topology manually.
///
/// 8 vertices at (+/-0.5, +/-0.5, +/-0.5).
/// 12 edges in canonical order (v[0] < v[1]).
/// 6 quad polygons: bottom (-Z), top (+Z), front (-Y), back (+Y), left (-X), right (+X).
/// 24 loops total (4 per face), CCW winding viewed from outside.
fn test_cube_topology() -> BrushTopology {
    // Vertex layout (index: x, y, z)
    // 0: (-0.5, -0.5, -0.5)
    // 1: ( 0.5, -0.5, -0.5)
    // 2: ( 0.5,  0.5, -0.5)
    // 3: (-0.5,  0.5, -0.5)
    // 4: (-0.5, -0.5,  0.5)
    // 5: ( 0.5, -0.5,  0.5)
    // 6: ( 0.5,  0.5,  0.5)
    // 7: (-0.5,  0.5,  0.5)
    let vertices = vec![
        MeshVert { position: Vec3::new(-0.5, -0.5, -0.5) }, // 0
        MeshVert { position: Vec3::new( 0.5, -0.5, -0.5) }, // 1
        MeshVert { position: Vec3::new( 0.5,  0.5, -0.5) }, // 2
        MeshVert { position: Vec3::new(-0.5,  0.5, -0.5) }, // 3
        MeshVert { position: Vec3::new(-0.5, -0.5,  0.5) }, // 4
        MeshVert { position: Vec3::new( 0.5, -0.5,  0.5) }, // 5
        MeshVert { position: Vec3::new( 0.5,  0.5,  0.5) }, // 6
        MeshVert { position: Vec3::new(-0.5,  0.5,  0.5) }, // 7
    ];

    // 12 edges in canonical order (lower index first).
    // Bottom face ring: 0-1, 1-2, 2-3, 0-3
    // Top face ring: 4-5, 5-6, 6-7, 4-7
    // Vertical: 0-4, 1-5, 2-6, 3-7
    let edges = vec![
        MeshEdge { v: [0, 1], flags: EdgeFlag::empty() }, //  0
        MeshEdge { v: [1, 2], flags: EdgeFlag::empty() }, //  1
        MeshEdge { v: [2, 3], flags: EdgeFlag::empty() }, //  2
        MeshEdge { v: [0, 3], flags: EdgeFlag::empty() }, //  3
        MeshEdge { v: [4, 5], flags: EdgeFlag::empty() }, //  4
        MeshEdge { v: [5, 6], flags: EdgeFlag::empty() }, //  5
        MeshEdge { v: [6, 7], flags: EdgeFlag::empty() }, //  6
        MeshEdge { v: [4, 7], flags: EdgeFlag::empty() }, //  7
        MeshEdge { v: [0, 4], flags: EdgeFlag::empty() }, //  8
        MeshEdge { v: [1, 5], flags: EdgeFlag::empty() }, //  9
        MeshEdge { v: [2, 6], flags: EdgeFlag::empty() }, // 10
        MeshEdge { v: [3, 7], flags: EdgeFlag::empty() }, // 11
    ];

    // 6 faces, 4 loops each = 24 loops total.
    // CCW winding when viewed from the outside (negative direction faces).
    //
    // Face 0: bottom (-Z), normal = (0, 0, -1). Viewed from -Z: CCW is 0,3,2,1
    // Face 1: top    (+Z), normal = (0, 0, +1). Viewed from +Z: CCW is 4,5,6,7
    // Face 2: front  (-Y), normal = (0,-1,  0). Viewed from -Y: CCW is 0,1,5,4
    // Face 3: back   (+Y), normal = (0,+1,  0). Viewed from +Y: CCW is 2,3,7,6 (no — 3,2,6,7)
    // Face 4: left   (-X), normal = (-1,0,  0). Viewed from -X: CCW is 0,4,7,3
    // Face 5: right  (+X), normal = (+1,0,  0). Viewed from +X: CCW is 1,2,6,5
    let polygons = vec![
        MeshPoly { loop_start: 0,  loop_total: 4 }, // bottom
        MeshPoly { loop_start: 4,  loop_total: 4 }, // top
        MeshPoly { loop_start: 8,  loop_total: 4 }, // front
        MeshPoly { loop_start: 12, loop_total: 4 }, // back
        MeshPoly { loop_start: 16, loop_total: 4 }, // left
        MeshPoly { loop_start: 20, loop_total: 4 }, // right
    ];

    // Face 0 bottom: verts 0,3,2,1 edges 3,2,1,0
    // Face 1 top:    verts 4,5,6,7 edges 4,5,6,7
    // Face 2 front:  verts 0,1,5,4 edges 0,9,4,8
    // Face 3 back:   verts 3,7,6,2 edges 11,6,10,2
    // Face 4 left:   verts 0,4,7,3 edges 8,7,11,3
    // Face 5 right:  verts 1,2,6,5 edges 1,10,5,9
    let loops = vec![
        // Face 0 bottom
        MeshLoop { vert: 0, edge: 3  },
        MeshLoop { vert: 3, edge: 2  },
        MeshLoop { vert: 2, edge: 1  },
        MeshLoop { vert: 1, edge: 0  },
        // Face 1 top
        MeshLoop { vert: 4, edge: 4  },
        MeshLoop { vert: 5, edge: 5  },
        MeshLoop { vert: 6, edge: 6  },
        MeshLoop { vert: 7, edge: 7  },
        // Face 2 front
        MeshLoop { vert: 0, edge: 0  },
        MeshLoop { vert: 1, edge: 9  },
        MeshLoop { vert: 5, edge: 4  },
        MeshLoop { vert: 4, edge: 8  },
        // Face 3 back
        MeshLoop { vert: 3, edge: 11 },
        MeshLoop { vert: 7, edge: 6  },
        MeshLoop { vert: 6, edge: 10 },
        MeshLoop { vert: 2, edge: 2  },
        // Face 4 left
        MeshLoop { vert: 0, edge: 8  },
        MeshLoop { vert: 4, edge: 7  },
        MeshLoop { vert: 7, edge: 11 },
        MeshLoop { vert: 3, edge: 3  },
        // Face 5 right
        MeshLoop { vert: 1, edge: 1  },
        MeshLoop { vert: 2, edge: 10 },
        MeshLoop { vert: 6, edge: 5  },
        MeshLoop { vert: 5, edge: 9  },
    ];

    BrushTopology {
        vertices,
        edges,
        polygons,
        loops,
        attributes: AttributeStack::default(),
    }
}

#[test]
fn cube_counts() {
    let topo = test_cube_topology();
    assert_eq!(topo.vertices.len(), 8, "expected 8 vertices");
    assert_eq!(topo.edges.len(), 12, "expected 12 edges");
    assert_eq!(topo.polygons.len(), 6, "expected 6 polygons");
    assert_eq!(topo.loops.len(), 24, "expected 24 loops");
}

#[test]
fn edge_flag_default_is_empty() {
    assert!(EdgeFlag::default().is_empty(), "default EdgeFlag must be empty");
}

#[test]
fn face_ring_bottom() {
    let topo = test_cube_topology();
    let verts: Vec<u32> = topo.face_ring(0).collect();
    assert_eq!(verts.len(), 4);
    // Bottom face: verts 0,3,2,1
    assert_eq!(verts, vec![0, 3, 2, 1]);
}

#[test]
fn face_ring_top() {
    let topo = test_cube_topology();
    let verts: Vec<u32> = topo.face_ring(1).collect();
    assert_eq!(verts.len(), 4);
    // Top face: verts 4,5,6,7
    assert_eq!(verts, vec![4, 5, 6, 7]);
}

#[test]
fn face_ring_all_four_verts() {
    let topo = test_cube_topology();
    for face_idx in 0..6 {
        let verts: Vec<u32> = topo.face_ring(face_idx).collect();
        assert_eq!(
            verts.len(),
            4,
            "face {face_idx} should have 4 vertices in its ring"
        );
    }
}

#[test]
fn edge_id_lookup() {
    let topo = test_cube_topology();
    // Edge 0 is v[0]=0, v[1]=1
    assert_eq!(topo.edge_id(0, 1), Some(0));
    // Reverse order should give same result
    assert_eq!(topo.edge_id(1, 0), Some(0));
    // Edge 11 is v[0]=3, v[1]=7
    assert_eq!(topo.edge_id(3, 7), Some(11));
    // Non-existent edge
    assert_eq!(topo.edge_id(0, 6), None);
}

#[test]
fn cube_face_normals_are_axis_aligned() {
    let t = test_cube_topology();
    let positions: Vec<Vec3> = t.vertices.iter().map(|v| v.position).collect();
    // Bottom face (-Z): normal should be -Z.
    let n0 = t.face_normal_with(&positions, 0);
    assert!(n0.distance(Vec3::NEG_Z) < 1e-4, "face 0 normal {n0}");
    // Top face (+Z): normal should be +Z.
    let n1 = t.face_normal_with(&positions, 1);
    assert!(n1.distance(Vec3::Z) < 1e-4, "face 1 normal {n1}");
}

#[test]
fn cube_face_centroid_for_top_face_is_origin_xy() {
    let t = test_cube_topology();
    let positions: Vec<Vec3> = t.vertices.iter().map(|v| v.position).collect();
    let c = t.face_centroid_with(&positions, 1);
    assert!((c.x.abs() + c.y.abs()) < 1e-4);
    assert!((c.z - 0.5).abs() < 1e-4);
}
