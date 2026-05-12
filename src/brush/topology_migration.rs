//! Auto-populate `Brush::topology` for legacy brushes whose `.jsn` files
//! pre-date the topology field. Detects empty-topology brushes, derives
//! topology from the legacy plane representation, and writes it back.
//!
//! Runs once per brush at insertion time via the change-detection filter.

use std::collections::HashMap;

use bevy::prelude::*;
use jackdaw_geometry::{
    compute_brush_geometry_from_planes,
    topology::{BrushTopology, EdgeFlag, MeshEdge, MeshLoop, MeshPoly, MeshVert},
};
use jackdaw_jsn::Brush;

/// System: when a legacy brush appears (faces present, topology empty),
/// derive topology from planes and write it back. Runs whenever `Brush`
/// changes.
pub fn migrate_legacy_brush_topology(
    mut brushes: Query<&mut Brush, Changed<Brush>>,
) -> Result<(), BevyError> {
    for mut brush in &mut brushes {
        if !brush.topology.polygons.is_empty() {
            continue;
        }
        if brush.faces.is_empty() {
            continue;
        }
        let topology = derive_topology_from_planes(&brush);
        if !topology.polygons.is_empty() {
            brush.topology = topology;
        }
    }
    Ok(())
}

fn derive_topology_from_planes(brush: &Brush) -> BrushTopology {
    let (positions, face_polygons) = compute_brush_geometry_from_planes(&brush.faces);
    if positions.is_empty() || face_polygons.is_empty() {
        return BrushTopology::default();
    }

    // Vertices.
    let vertices: Vec<MeshVert> = positions
        .iter()
        .map(|&p| MeshVert { position: p })
        .collect();

    // Build canonical (v0 <= v1) edge set and an edge-index lookup.
    let mut edge_map: HashMap<(u32, u32), u32> = HashMap::new();
    let mut edges: Vec<MeshEdge> = Vec::new();
    let mut canonicalize = |a: u32, b: u32| {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        if let Some(&idx) = edge_map.get(&(lo, hi)) {
            idx
        } else {
            let idx = edges.len() as u32;
            edges.push(MeshEdge {
                v: [lo, hi],
                flags: EdgeFlag::empty(),
            });
            edge_map.insert((lo, hi), idx);
            idx
        }
    };

    // Polygons + loops: walk each face polygon in order; one MeshLoop per ring vertex.
    let mut polygons: Vec<MeshPoly> = Vec::with_capacity(face_polygons.len());
    let mut loops: Vec<MeshLoop> = Vec::new();
    for ring in &face_polygons {
        if ring.len() < 3 {
            // Degenerate face; skip but keep parallel-array invariant by emitting an empty poly.
            polygons.push(MeshPoly {
                loop_start: loops.len() as u32,
                loop_total: 0,
            });
            continue;
        }
        let loop_start = loops.len() as u32;
        for i in 0..ring.len() {
            let v_cur = ring[i] as u32;
            let v_next = ring[(i + 1) % ring.len()] as u32;
            let edge_idx = canonicalize(v_cur, v_next);
            loops.push(MeshLoop {
                vert: v_cur,
                edge: edge_idx,
            });
        }
        polygons.push(MeshPoly {
            loop_start,
            loop_total: ring.len() as u32,
        });
    }

    BrushTopology {
        vertices,
        edges,
        polygons,
        loops,
        attributes: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::app::App;
    use jackdaw_jsn::Brush;

    #[test]
    fn legacy_brush_with_empty_topology_gets_populated_by_system() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(Update, migrate_legacy_brush_topology);

        // Construct a "legacy" brush: faces with planes, but topology cleared.
        let mut brush = Brush::cuboid(0.5, 0.5, 0.5);
        brush.topology = BrushTopology::default();

        let entity = app.world_mut().spawn(brush).id();
        app.update();

        // The system should have populated topology.
        let brush = app.world().get::<Brush>(entity).unwrap();
        assert!(
            !brush.topology.polygons.is_empty(),
            "topology should be populated after migration"
        );
        assert_eq!(brush.topology.polygons.len(), 6);
        assert_eq!(brush.topology.vertices.len(), 8);
    }

    #[test]
    fn brush_with_existing_topology_is_not_overwritten() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(Update, migrate_legacy_brush_topology);

        // A fully-populated brush should be left alone.
        let brush = Brush::cuboid(1.0, 1.0, 1.0);
        assert!(!brush.topology.polygons.is_empty());
        let original_poly_count = brush.topology.polygons.len();

        let entity = app.world_mut().spawn(brush).id();
        app.update();

        let brush = app.world().get::<Brush>(entity).unwrap();
        assert_eq!(
            brush.topology.polygons.len(),
            original_poly_count,
            "existing topology should not be replaced"
        );
    }

    #[test]
    fn empty_brush_no_faces_no_topology_is_skipped() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(Update, migrate_legacy_brush_topology);

        let brush = Brush::default(); // no faces, no topology
        let entity = app.world_mut().spawn(brush).id();
        app.update();

        let brush = app.world().get::<Brush>(entity).unwrap();
        assert!(
            brush.topology.polygons.is_empty(),
            "empty brush should remain empty"
        );
    }
}
