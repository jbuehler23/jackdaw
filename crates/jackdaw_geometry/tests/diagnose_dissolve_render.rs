use bevy::math::Vec3;
use jackdaw_geometry::editmesh::{
    EditMesh,
    ops::{dissolve_verts::dissolve_verts, subdivide::subdivide},
};
use jackdaw_jsn::Brush;

fn fan_triangulate(n: usize) -> Vec<[u32; 3]> {
    if n < 3 {
        return Vec::new();
    }
    (1..n - 1)
        .map(|i| [0u32, i as u32, (i + 1) as u32])
        .collect()
}

#[test]
fn diagnose_post_fix_render_state() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut bmesh = EditMesh::lift_from_topology(&brush.topology);

    // STEP 1: subdivide one edge.
    let some_edge = bmesh.edges.keys().next().unwrap();
    let result = subdivide(&mut bmesh, &[some_edge]).expect("subdivide");
    let midpoint = result.new_verts[0];

    // STEP 2: dissolve the midpoint.
    let _ = dissolve_verts(&mut bmesh, &[midpoint]).expect("dissolve");
    bmesh.validate().expect("valid");

    println!("\n=== POST-DISSOLVE EditMesh STATE ===");
    println!(
        "verts: {}, edges: {}, loops: {}, faces: {}",
        bmesh.vert_count(),
        bmesh.edge_count(),
        bmesh.loop_count(),
        bmesh.face_count()
    );
    let mut mat_idxs: Vec<u32> = bmesh.faces.values().map(|f| f.material_idx).collect();
    mat_idxs.sort();
    println!("material_idxes (sorted): {:?}", mat_idxs);
    let unique: std::collections::HashSet<u32> = mat_idxs.iter().copied().collect();
    println!(
        "UNIQUE material_idxes (count): {} (should match face count)",
        unique.len()
    );
    assert_eq!(
        unique.len(),
        bmesh.face_count(),
        "material_idx should be unique now per the fix"
    );

    println!("\nFaces:");
    for (k, f) in bmesh.faces.iter() {
        // Walk the face's ring to gather positions.
        let mut ring_pos: Vec<Vec3> = Vec::new();
        let mut cur = f.loop_first;
        for _ in 0..f.loop_count {
            ring_pos.push(bmesh.verts[bmesh.loops[cur].vert].co);
            cur = bmesh.loops[cur].next;
        }
        let _ = k; // suppress unused warning
        println!(
            "  face material_idx={}, loop_count={}, normal_cache={}",
            f.material_idx, f.loop_count, f.normal_cache
        );
        for (i, p) in ring_pos.iter().enumerate() {
            println!("    ring[{}]: {:?}", i, p);
        }
        // Check if this face's ring is convex (crucial for fan triangulation).
        if ring_pos.len() >= 4 {
            let convex = is_convex_ring(&ring_pos, f.normal_cache);
            println!("    CONVEX: {}", convex);
        }
    }

    // STEP 3: simulate the render path.
    let topology = bmesh.flatten_to_topology();
    let positions: Vec<Vec3> = topology.vertices.iter().map(|v| v.position).collect();
    println!("\n=== RENDER SIMULATION ===");
    for (i, poly) in topology.polygons.iter().enumerate() {
        let normal = topology.face_normal_with(&positions, i);
        let ring_indices: Vec<u32> = topology.face_ring(i).collect();
        let ring_pos: Vec<Vec3> = ring_indices
            .iter()
            .map(|&vi| positions[vi as usize])
            .collect();
        let tris = fan_triangulate(ring_indices.len());
        let _ = poly; // suppress unused warning
        println!(
            "polygon[{}]: normal={}, ring_len={}, tris={}",
            i,
            normal,
            ring_indices.len(),
            tris.len()
        );
        let convex = is_convex_ring(&ring_pos, normal);
        println!("  CONVEX: {}", convex);
        if !convex {
            println!("  !! NON-CONVEX FACE - fan triangulation may produce invalid tris");
        }

        // Compute area of each fan tri to detect zero-area / degenerate.
        for (ti, tri) in tris.iter().enumerate() {
            let a = ring_pos[tri[0] as usize];
            let b = ring_pos[tri[1] as usize];
            let c = ring_pos[tri[2] as usize];
            let cross = (b - a).cross(c - a);
            let area = cross.length() * 0.5;
            let aligned = cross.normalize_or_zero().dot(normal);
            println!("  tri[{}]: area={}, normal-aligned={}", ti, area, aligned);
        }
    }
}

fn is_convex_ring(ring: &[Vec3], normal: Vec3) -> bool {
    let n = ring.len();
    if n < 4 {
        return true;
    }
    for i in 0..n {
        let a = ring[i];
        let b = ring[(i + 1) % n];
        let c = ring[(i + 2) % n];
        let cross = (b - a).cross(c - b);
        if cross.dot(normal) < -0.001 {
            return false;
        }
    }
    true
}
