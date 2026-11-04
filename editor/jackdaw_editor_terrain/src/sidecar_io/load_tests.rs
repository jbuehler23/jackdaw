/// Tests for reading terrain sidecars back into the store.
///
/// A terrain reaches the world by two routes, and only an explicit open takes
/// disk as the truth: reloading on a tab switch would discard unsaved
/// sculpting.
mod terrain_sidecar_import_tests {
    use std::path::PathBuf;

    use bevy::prelude::*;
    use jackdaw_terrain::{RegionTerrainData, TerrainData, sidecar};

    use crate::TerrainDataStore;
    use crate::sidecar_io::{SidecarImport, import_terrain_sidecars};

    fn unique_tmp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jd_terrin_{}_{label}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn on_disk() -> TerrainData {
        TerrainData {
            resolution: 4,
            heights: (0..16).map(|i| i as f32).collect(),
            channels: vec![],
        }
    }

    fn document(data: &TerrainData) -> RegionTerrainData {
        RegionTerrainData::from_legacy_v1(data).expect("a power-of-two resolution migrates")
    }

    /// A world with one terrain naming `data_path`, and a pre-region-format
    /// sidecar for it written beside `zone.bsn` in a fresh temp dir.
    fn world_and_scene(label: &str, data_path: &str) -> (World, PathBuf) {
        let tmp = unique_tmp_dir(label);
        let bytes = sidecar::encode(&on_disk()).expect("encodes");
        std::fs::write(tmp.join(data_path), bytes).expect("write sidecar");

        let mut world = World::new();
        world.insert_resource(TerrainDataStore::default());
        world.spawn(jackdaw_scene_types::Terrain {
            resolution: 4,
            data_path: data_path.to_string(),
            ..default()
        });
        (world, tmp.join("zone.bsn"))
    }

    #[test]
    fn a_terrain_with_no_stored_data_is_hydrated_from_its_sidecar() {
        let (mut world, scene) = world_and_scene("fill", "zone.terrain-0.jdterrain");
        import_terrain_sidecars(
            &mut world,
            &scene.to_string_lossy(),
            SidecarImport::FillMissing,
        );
        assert_eq!(
            world
                .resource::<TerrainDataStore>()
                .heights("zone.terrain-0.jdterrain"),
            on_disk().heights.as_slice(),
        );
        let _ = std::fs::remove_dir_all(scene.parent().expect("temp dir"));
    }

    /// The regression this mode exists for: sculpt, switch tabs without
    /// saving, switch back. The store is the truth, not the older file.
    #[test]
    fn fill_missing_leaves_unsaved_edits_alone() {
        let (mut world, scene) = world_and_scene("unsaved", "zone.terrain-0.jdterrain");
        let unsaved = TerrainData {
            resolution: 4,
            heights: vec![99.0; 16],
            channels: vec![],
        };
        world
            .resource_mut::<TerrainDataStore>()
            .insert("zone.terrain-0.jdterrain".to_string(), document(&unsaved));

        import_terrain_sidecars(
            &mut world,
            &scene.to_string_lossy(),
            SidecarImport::FillMissing,
        );
        assert_eq!(
            world
                .resource::<TerrainDataStore>()
                .heights("zone.terrain-0.jdterrain"),
            vec![99.0; 16].as_slice(),
            "a tab swap must not re-read over unsaved sculpting",
        );
        let _ = std::fs::remove_dir_all(scene.parent().expect("temp dir"));
    }

    /// An explicit open, by contrast, is exactly a request for what is on
    /// disk.
    #[test]
    fn reload_overwrites_what_the_store_was_holding() {
        let (mut world, scene) = world_and_scene("reload", "zone.terrain-0.jdterrain");
        world.resource_mut::<TerrainDataStore>().insert(
            "zone.terrain-0.jdterrain".to_string(),
            document(&TerrainData {
                resolution: 4,
                heights: vec![99.0; 16],
                channels: vec![],
            }),
        );

        import_terrain_sidecars(&mut world, &scene.to_string_lossy(), SidecarImport::Reload);
        assert_eq!(
            world
                .resource::<TerrainDataStore>()
                .heights("zone.terrain-0.jdterrain"),
            on_disk().heights.as_slice(),
        );
        let _ = std::fs::remove_dir_all(scene.parent().expect("temp dir"));
    }

    /// A scene whose sidecar was never copied alongside it opens flat with
    /// a warning rather than failing.
    #[test]
    fn a_missing_sidecar_loads_flat_rather_than_erroring() {
        let tmp = unique_tmp_dir("missing");
        let mut world = World::new();
        world.insert_resource(TerrainDataStore::default());
        world.spawn(jackdaw_scene_types::Terrain {
            resolution: 4,
            data_path: "gone.jdterrain".to_string(),
            ..default()
        });

        import_terrain_sidecars(
            &mut world,
            &tmp.join("zone.bsn").to_string_lossy(),
            SidecarImport::Reload,
        );
        assert!(
            world
                .resource::<TerrainDataStore>()
                .heights("gone.jdterrain")
                .is_empty(),
            "a missing sidecar leaves the terrain flat",
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

/// Settling a loaded terrain onto the geometry its cells are drawn at: the
/// sidecar's own geometry where it states one, and the declared rectangle where
/// it does not.
mod grid_settling_tests {
    use bevy::prelude::*;
    use jackdaw_terrain::{
        RegionTerrainData,
        region::{RegionCoord, RegionSize, TerrainRegions},
    };

    use crate::TerrainDataStore;
    use crate::sidecar_io::{SidecarImport, import_terrain_sidecars, settle_terrain_grids};

    /// Regions covering `span` regions per axis from the origin, at 256 cells per region.
    fn regions_spanning(span: i32) -> TerrainRegions {
        let mut regions = TerrainRegions::new(RegionSize::DEFAULT);
        for rz in 0..span {
            for rx in 0..span {
                regions.ensure_region(RegionCoord::new(rx, rz));
            }
        }
        regions
    }

    /// A document whose regions cover `span` regions per axis.
    fn document(span: i32) -> RegionTerrainData {
        RegionTerrainData {
            regions: regions_spanning(span),
            ..default()
        }
    }

    const DATA_PATH: &str = "scene.terrain-1.jdterrain";

    /// A terrain whose sidecar states no geometry, so the declared rectangle places its
    /// cells.
    fn world_with_legacy_terrain() -> World {
        world_with(256, 4, DATA_PATH)
    }

    fn world_with(resolution: u32, span: i32, data_path: &str) -> World {
        let mut world = World::new();
        let mut store = TerrainDataStore::default();
        // No stored geometry, so the declared rectangle places these cells.
        store.insert(data_path.to_string(), legacy_document(span));
        world.insert_resource(store);
        world.spawn(jackdaw_scene_types::Terrain {
            resolution,
            size: Vec2::splat(100.0),
            data_path: data_path.to_string(),
            ..default()
        });
        world
    }

    /// Where a terrain declaring a `size` by `resolution` rectangle draws the vertex at grid
    /// `(x, z)`, in entity-local space.
    ///
    /// The rectangle is centred on the entity and its `resolution` counts vertices, so the
    /// first sits at `-size/2` and the last on the far edge. The migration reproduces this
    /// mapping; it is spelled out here independently of the code under test.
    fn declared_rect_vertex(size: Vec2, resolution: u32, x: u32, z: u32) -> Vec2 {
        let spacing = size / (resolution.max(2) - 1) as f32;
        -size / 2.0 + Vec2::new(x as f32, z as f32) * spacing
    }

    /// A document as a sidecar without geometry hands it over: cells, and nothing saying
    /// where they sit.
    fn legacy_document(span: i32) -> RegionTerrainData {
        RegionTerrainData {
            grid: None,
            ..document(span)
        }
    }

    /// Where the settled geometry puts the vertex at grid `(x, z)`.
    fn settled_vertex(world: &mut World, x: u32, z: u32) -> Vec2 {
        let grid = world
            .resource::<TerrainDataStore>()
            .grid(DATA_PATH)
            .expect("the load settles a geometry onto every terrain");
        grid.anchor + Vec2::new(x as f32, z as f32) * grid.cell_size
    }

    /// The migration re-describes rather than moves: whatever rectangle a scene declared,
    /// every stored cell comes out of the load at the world position it had.
    ///
    /// Both forms are covered: a scene that elided the pair and refilled it from the
    /// component's defaults, and one that wrote it out explicitly.
    #[test]
    fn a_declared_rects_ground_stays_where_it_was_through_the_migration() {
        for (size, resolution) in [
            // Elided: refilled from the component defaults.
            (Vec2::splat(100.0), 256u32),
            // Explicit, the shape the shape panel offers.
            (Vec2::splat(1024.0), 1024),
            // A 2^k+1 grid, which lands on a whole spacing.
            (Vec2::splat(128.0), 129),
        ] {
            let mut world = World::new();
            let mut store = TerrainDataStore::default();
            store.insert(DATA_PATH.to_string(), legacy_document(4));
            world.insert_resource(store);
            world.spawn(jackdaw_scene_types::Terrain {
                resolution,
                size,
                data_path: DATA_PATH.to_string(),
                ..default()
            });

            settle_terrain_grids(&mut world);

            for (x, z) in [
                (0u32, 0u32),
                (1, 0),
                (0, 1),
                (resolution - 1, resolution - 1),
            ] {
                assert_eq!(
                    settled_vertex(&mut world, x, z),
                    declared_rect_vertex(size, resolution, x, z),
                    "vertex ({x}, {z}) of a {size:?} by {resolution} terrain moved",
                );
            }
        }
    }

    /// A cell is square, so a rectangle asking for two spacings cannot be re-described
    /// exactly. X wins and Z is respaced, which moves ground, so the settling warns.
    #[test]
    fn a_non_square_rect_settles_on_its_x_spacing_and_says_so() {
        let mut world = World::new();
        let mut store = TerrainDataStore::default();
        store.insert(DATA_PATH.to_string(), legacy_document(4));
        world.insert_resource(store);
        world.spawn(jackdaw_scene_types::Terrain {
            resolution: 1024,
            size: Vec2::new(2000.0, 500.0),
            data_path: DATA_PATH.to_string(),
            ..default()
        });

        settle_terrain_grids(&mut world);

        let mut query = world.query::<&jackdaw_scene_types::Terrain>();
        assert_eq!(
            query.single(&world).expect("one terrain").cell_size,
            2000.0 / 1023.0,
            "the X axis spacing is the one a scalar cell size keeps",
        );
        assert_eq!(
            jackdaw_terrain::sidecar::declared_rect_respacing(Vec2::new(2000.0, 500.0), 1024),
            Some((2000.0 / 1023.0, 500.0 / 1023.0)),
            "both spacings are available to name in the warning",
        );
    }

    /// The inlets are read once and emptied, so a saved scene carries the derived cell size
    /// and no rectangle for a later load to re-derive from.
    #[test]
    fn settling_a_terrain_fills_its_cell_size_and_empties_the_inlets() {
        let mut world = world_with(256, 4, DATA_PATH);
        settle_terrain_grids(&mut world);

        let defaults = jackdaw_scene_types::Terrain::default();
        let mut query = world.query::<&jackdaw_scene_types::Terrain>();
        let terrain = query.single(&world).expect("one terrain");
        assert_eq!(terrain.cell_size, 100.0 / 255.0);
        assert_eq!(terrain.size, defaults.size);
        assert_eq!(terrain.resolution, defaults.resolution);
    }

    /// A sidecar that states its own geometry wins over a scene text that declares a
    /// rectangle, the state a save interrupted between its two files leaves behind.
    #[test]
    fn a_sidecar_that_states_its_geometry_outranks_stale_scene_text() {
        use jackdaw_terrain::sidecar::GridGeometry;

        let stated = GridGeometry {
            cell_size: 2.5,
            anchor: Vec2::new(7.0, -3.0),
        };
        let mut world = world_with(256, 4, DATA_PATH);
        world
            .resource_mut::<TerrainDataStore>()
            .set_grid(DATA_PATH, stated);

        settle_terrain_grids(&mut world);

        assert_eq!(
            world.resource::<TerrainDataStore>().grid(DATA_PATH),
            Some(stated)
        );
        let mut query = world.query::<&jackdaw_scene_types::Terrain>();
        assert_eq!(query.single(&world).expect("one terrain").cell_size, 2.5);
    }

    /// A scene opened a second time, with the store warm, reads no sidecar again.
    #[test]
    fn reopening_a_warm_store_reads_no_sidecar_again() {
        use jackdaw_terrain::{RegionTerrainData, sidecar};

        let tmp = std::env::temp_dir().join(format!("jd_fb_warm_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let data_path = "zone.terrain-0.jdterrain";
        std::fs::write(
            tmp.join(data_path),
            sidecar::save(&RegionTerrainData {
                regions: regions_spanning(4),
                ..default()
            })
            .expect("encodes"),
        )
        .expect("write sidecar");

        let mut world = World::new();
        world.insert_resource(TerrainDataStore::default());
        world.spawn(jackdaw_scene_types::Terrain {
            resolution: 256,
            size: Vec2::splat(100.0),
            data_path: data_path.to_string(),
            ..default()
        });
        let scene = tmp.join("zone.bsn").to_string_lossy().to_string();

        let first = import_terrain_sidecars(&mut world, &scene, SidecarImport::FillMissing);
        assert_eq!(first.len(), 1, "the first load reads the sidecar in");

        let second = import_terrain_sidecars(&mut world, &scene, SidecarImport::FillMissing);
        assert!(
            second.is_empty(),
            "the store already holds it, so nothing is read again: {second:?}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A tab switch re-runs the import to fill anything missing. With every sidecar already
    /// in the store it imports nothing, costing no file read and moving no stored data.
    #[test]
    fn an_import_that_finds_everything_already_loaded_reads_nothing() {
        let mut world = world_with_legacy_terrain();
        let scene = std::path::Path::new("/nonexistent/zone.bsn");

        // A store holding every sidecar has been through a load, so its terrains are
        // settled onto their geometry.
        settle_terrain_grids(&mut world);

        // Nothing is missing, so FillMissing has nothing to read.
        let before = world.resource::<TerrainDataStore>().get(DATA_PATH).cloned();
        import_terrain_sidecars(
            &mut world,
            &scene.to_string_lossy(),
            SidecarImport::FillMissing,
        );

        assert_eq!(
            world.resource::<TerrainDataStore>().get(DATA_PATH).cloned(),
            before,
            "a no-op import leaves the store alone"
        );
    }
}
