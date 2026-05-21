//! Auto-populate `Brush::topology` for legacy brushes whose `.jsn` files
//! pre-date the topology field. Detects empty-topology brushes, derives
//! topology from the legacy plane representation, and writes it back.
//!
//! Runs once per brush at insertion time via the change-detection filter.

use bevy_ecs::prelude::*;
use bevy_app::prelude::*;
use jackdaw_geometry::compute_brush_topology;
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
        let topology = compute_brush_topology(&brush.faces);
        if !topology.polygons.is_empty() {
            brush.topology = topology;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_app::App;
    use jackdaw_geometry::topology::BrushTopology;
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
