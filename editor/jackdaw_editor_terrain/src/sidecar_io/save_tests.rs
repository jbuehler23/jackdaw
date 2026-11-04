/// Tests for the terrain sidecar sibling writer.
///
/// These drive the synchronous save boundary directly: success means the
/// bytes are already durable enough to read back, and failure is returned.
mod terrain_sidecar_tests {
    use std::path::PathBuf;

    use bevy::prelude::*;
    use jackdaw_terrain::{RegionTerrainData, TerrainData, sidecar};

    use jackdaw_editor_core::scene_io::emit_bsn_scene_with_inline_assets;

    use crate::TerrainDataStore;
    use crate::sidecar_io::export_terrain_sidecars;

    fn unique_tmp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jd_terr_{}_{label}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// A sculpted 4x4 terrain, distinctive enough that a zeroed or
    /// truncated round-trip fails loudly.
    fn sculpted() -> TerrainData {
        TerrainData {
            resolution: 4,
            heights: (0..16).map(|i| i as f32 * 0.5).collect(),
            channels: vec![],
        }
    }

    /// A store holding ground for `terrain`, in regions sized to keep the fixture small.
    /// Nothing allocates implicitly, so a terrain a test writes to is laid down first.
    fn store_holding(terrain: &jackdaw_scene_types::Terrain) -> TerrainDataStore {
        let mut regions = jackdaw_terrain::TerrainRegions::new(
            jackdaw_terrain::RegionSize::new(terrain.resolution.next_power_of_two())
                .expect("a power of two"),
        );
        regions
            .ensure_grid(terrain.resolution)
            .expect("inside the region cap");
        let mut store = TerrainDataStore::default();
        store.insert(
            terrain.data_path.clone(),
            jackdaw_terrain::RegionTerrainData {
                regions,
                ..Default::default()
            },
        );
        store
    }

    fn document(data: &TerrainData) -> RegionTerrainData {
        let mut document =
            RegionTerrainData::from_legacy_v1(data).expect("a power-of-two resolution migrates");
        // The load path settles a document onto the geometry its cells are drawn at before
        // a save, so a stand-in for a document the editor holds states its own.
        document.grid = Some(jackdaw_terrain::sidecar::GridGeometry::DEFAULT);
        document
    }

    fn world_with_terrain(data_path: &str, data: TerrainData) -> World {
        let mut world = World::new();
        let mut store = TerrainDataStore::default();
        store.insert(data_path.to_string(), document(&data));
        world.insert_resource(store);
        world.spawn(jackdaw_scene_types::Terrain {
            resolution: 4,
            data_path: data_path.to_string(),
            ..default()
        });
        world
    }

    fn read_decode(path: &std::path::Path) -> Option<RegionTerrainData> {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| sidecar::load(&bytes).ok())
    }

    #[test]
    fn writes_a_sidecar_that_decodes_back_to_the_sculpted_heights() {
        let tmp = unique_tmp_dir("roundtrip");
        let scene_path = tmp.join("zone.bsn");
        let sidecar_path = tmp.join("zone.terrain-0.jdterrain");

        let mut world = world_with_terrain("zone.terrain-0.jdterrain", sculpted());
        export_terrain_sidecars(&mut world, &scene_path.to_string_lossy())
            .expect("sidecar write succeeds");

        let decoded = read_decode(&sidecar_path)
            .unwrap_or_else(|| panic!("sidecar not written: {}", sidecar_path.display()));
        assert_eq!(decoded, document(&sculpted()));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Saving twice with no edits in between must produce the same bytes,
    /// so a sidecar can be committed and diffed like any other artifact.
    #[test]
    fn saving_twice_produces_identical_bytes() {
        let tmp = unique_tmp_dir("stable");
        let scene_path = tmp.join("zone.bsn");
        let sidecar_path = tmp.join("zone.terrain-0.jdterrain");

        let mut world = world_with_terrain("zone.terrain-0.jdterrain", sculpted());
        export_terrain_sidecars(&mut world, &scene_path.to_string_lossy())
            .expect("first write succeeds");
        let first = std::fs::read(&sidecar_path).expect("read first write");

        export_terrain_sidecars(&mut world, &scene_path.to_string_lossy())
            .expect("second write succeeds");
        let second = std::fs::read(&sidecar_path).expect("read second write");

        assert_eq!(first, second);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_sidecar_write_failure_is_returned_to_the_save_caller() {
        let tmp = unique_tmp_dir("write-failure");
        let blocked_parent = tmp.join("not-a-directory");
        std::fs::write(&blocked_parent, b"file blocks directory creation")
            .expect("create blocking file");
        let scene_path = blocked_parent.join("zone.bsn");
        let sidecar_path = blocked_parent.join("zone.terrain-0.jdterrain");

        let mut world = world_with_terrain("zone.terrain-0.jdterrain", sculpted());
        let error = export_terrain_sidecars(&mut world, &scene_path.to_string_lossy())
            .expect_err("the write failure must reach the save boundary");

        assert!(
            error
                .to_string()
                .contains(&sidecar_path.display().to_string()),
            "the error names the failed sidecar: {error}",
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A scene with no terrain must not litter the project with an empty
    /// sidecar.
    #[test]
    fn no_sidecar_when_the_scene_has_no_terrain() {
        let tmp = unique_tmp_dir("bare");
        let scene_path = tmp.join("bare.bsn");

        let mut world = World::new();
        world.insert_resource(TerrainDataStore::default());
        export_terrain_sidecars(&mut world, &scene_path.to_string_lossy())
            .expect("terrain-less scene is a no-op");
        let stray = std::fs::read_dir(&tmp)
            .expect("temp dir readable")
            .filter_map(Result::ok)
            .any(|e| {
                e.path()
                    .extension()
                    .is_some_and(|ext| ext == sidecar::EXTENSION)
            });
        assert!(!stray, "no sidecar may be written for a terrain-less scene");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Heights live in the sidecar, not the scene text, so a terrain adds only
    /// a handful of lines to a `.bsn`.
    #[test]
    fn a_512_terrain_contributes_almost_nothing_to_the_scene_text() {
        use bevy::ecs::reflect::AppTypeRegistry;
        use jackdaw_bsn::SceneBsnAst;

        let mut world = World::new();
        world.init_resource::<AppTypeRegistry>();
        {
            let registry = world.resource::<AppTypeRegistry>().clone();
            let mut writer = registry.write();
            writer.register::<Name>();
            writer.register::<jackdaw_scene_types::Terrain>();
            writer.register::<jackdaw_scene_types::SceneNodeId>();
        }
        world.init_resource::<SceneBsnAst>();

        let mut store = TerrainDataStore::default();
        store.insert(
            "zone.terrain-0.jdterrain".to_string(),
            document(&TerrainData {
                resolution: 512,
                heights: vec![0.75; 512 * 512],
                channels: vec![],
            }),
        );
        world.insert_resource(store);

        let entity = world
            .spawn((
                Name::new("ground"),
                jackdaw_scene_types::Terrain {
                    resolution: 512,
                    data_path: "zone.terrain-0.jdterrain".to_string(),
                    ..default()
                },
            ))
            .id();
        jackdaw_editor_core::scene_io::register_entity_in_ast(&mut world, entity);

        let text = emit_bsn_scene_with_inline_assets(&mut world, std::path::Path::new("."));
        assert!(
            text.contains("zone.terrain-0.jdterrain"),
            "the scene must name the sidecar it depends on:\n{text}"
        );
        assert!(
            text.len() < 2048,
            "a 512 terrain must not bloat the scene text; got {} bytes:\n{text}",
            text.len()
        );
    }

    /// Store entries whose terrain is not in this scene belong to another
    /// tab (or to an undone delete) and must not be written beside it.
    #[test]
    fn only_terrains_present_in_the_scene_are_written() {
        let tmp = unique_tmp_dir("scoped");
        let scene_path = tmp.join("zone.bsn");

        let mut world = world_with_terrain("zone.terrain-0.jdterrain", sculpted());
        world.resource_mut::<TerrainDataStore>().insert(
            "other-scene.terrain-0.jdterrain".to_string(),
            document(&sculpted()),
        );
        export_terrain_sidecars(&mut world, &scene_path.to_string_lossy())
            .expect("this scene's sidecar writes");

        assert!(
            read_decode(&tmp.join("zone.terrain-0.jdterrain")).is_some(),
            "this scene's terrain is written"
        );
        assert!(
            !tmp.join("other-scene.terrain-0.jdterrain").exists(),
            "another scene's terrain must not be written here"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Painting and saving over a sidecar from a newer build must not write a
    /// zeroed document over it.
    #[test]
    fn an_unreadable_sidecar_is_never_overwritten_by_a_save() {
        let tmp = unique_tmp_dir("unreadable");
        let scene_path = tmp.join("zone.bsn");
        let sidecar_path = tmp.join("zone.terrain-0.jdterrain");

        let mut original = sidecar::save(&document(&sculpted())).expect("encodes");
        original[8..10].copy_from_slice(&(sidecar::VERSION_11 + 1).to_le_bytes());
        std::fs::write(&sidecar_path, &original).expect("write sidecar");

        let mut world = World::new();
        world.insert_resource(TerrainDataStore::default());
        world.spawn(jackdaw_scene_types::Terrain {
            resolution: 4,
            data_path: "zone.terrain-0.jdterrain".to_string(),
            ..default()
        });

        crate::sidecar_io::import_terrain_sidecars(
            &mut world,
            &scene_path.to_string_lossy(),
            jackdaw_editor_core::scene_io::SidecarImport::Reload,
        );
        assert!(
            world
                .resource::<TerrainDataStore>()
                .is_load_failed("zone.terrain-0.jdterrain"),
            "a decode failure must mark the entry load-failed",
        );

        // The stroke: an edit attempt must be refused, not minted as
        // zeroed data.
        let brushed = jackdaw_scene_types::Terrain {
            resolution: 4,
            data_path: "zone.terrain-0.jdterrain".to_string(),
            ..default()
        };
        assert!(
            world
                .resource_mut::<TerrainDataStore>()
                .entry_for(&brushed)
                .is_none(),
            "edits to a load-failed terrain must be refused",
        );

        // Ctrl+S: the save must succeed and must not touch the file.
        export_terrain_sidecars(&mut world, &scene_path.to_string_lossy())
            .expect("save must not hard-error on a load-failed entry");
        assert_eq!(
            std::fs::read(&sidecar_path).expect("sidecar still on disk"),
            original,
            "the unreadable original must survive the save byte-for-byte",
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The texture-set reference and every cell's control word round-trip
    /// through a save unchanged.
    #[test]
    fn authored_paint_and_materials_survive_a_save_and_reload() {
        use jackdaw_terrain::Control;

        let tmp = unique_tmp_dir("paint-roundtrip");
        let scene_path = tmp.join("zone.bsn");
        let data_path = "zone.terrain-0.jdterrain";
        let terrain = jackdaw_scene_types::Terrain {
            resolution: 4,
            data_path: data_path.to_string(),
            ..default()
        };
        let painted = Control::default()
            .with_base_id(3)
            .with_overlay_id(7)
            .with_blend(200);

        let mut world = World::new();
        world.insert_resource(store_holding(&terrain));
        world.spawn(terrain.clone());
        let authored = {
            let mut store = world.resource_mut::<TerrainDataStore>();
            store
                .entry_for(&terrain)
                .expect("keyed")
                .set_heights(&sculpted().heights);
            store.control_mut(&terrain).expect("keyed")[5] = painted;
            store
                .set_materials(
                    data_path,
                    vec![
                        jackdaw_terrain::sidecar::TerrainMaterialSlot::new("grass"),
                        jackdaw_terrain::sidecar::TerrainMaterialSlot {
                            material: "rock_05".to_string(),
                            uv_scale: 0.25,
                            detile: 0.5,
                            occlusion: String::new(),
                            roughness: String::new(),
                            perceptual_roughness: None,
                            reflectance: None,
                        },
                    ],
                )
                .expect("plain material names are accepted");
            store.get(data_path).expect("authored").clone()
        };

        export_terrain_sidecars(&mut world, &scene_path.to_string_lossy()).expect("save succeeds");

        // A fresh store, as a reopened editor has.
        let mut reopened = World::new();
        reopened.insert_resource(TerrainDataStore::default());
        reopened.spawn(terrain.clone());
        crate::sidecar_io::import_terrain_sidecars(
            &mut reopened,
            &scene_path.to_string_lossy(),
            jackdaw_editor_core::scene_io::SidecarImport::Reload,
        );

        let store = reopened.resource::<TerrainDataStore>();
        assert_eq!(
            store.get(data_path),
            Some(&authored),
            "the reloaded document must equal what was authored",
        );
        assert_eq!(store.control(data_path)[5], painted);
        assert_eq!(store.materials(data_path).len(), 2);
        assert_eq!(store.materials(data_path)[1].material, "rock_05");
        assert_eq!(store.materials(data_path)[1].uv_scale, 0.25);
        assert_eq!(store.materials(data_path)[1].detile, 0.5);
        assert_eq!(store.heights(data_path), sculpted().heights.as_slice());
        assert!(
            store.take_control_dirty(data_path).is_some(),
            "loaded paint must be marked for upload, or it never reaches the material",
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 129 vertices per edge is 2^7 + 1, so no single power-of-two region holds it. It has
    /// to open, be editable, and round-trip every height, seam row included.
    #[test]
    fn a_non_power_of_two_sidecar_opens_and_embeds_every_height() {
        let tmp = unique_tmp_dir("non-pow2");
        let scene_path = tmp.join("zone.bsn");
        let sidecar_path = tmp.join("zone.terrain-0.jdterrain");

        let heights: Vec<f32> = (0..129 * 129).map(|i| i as f32 * 0.5).collect();
        let original = sidecar::encode(&TerrainData {
            resolution: 129,
            heights: heights.clone(),
            channels: vec![],
        })
        .expect("v1 encodes");
        std::fs::write(&sidecar_path, &original).expect("write sidecar");

        let odd = jackdaw_scene_types::Terrain {
            resolution: 129,
            data_path: "zone.terrain-0.jdterrain".to_string(),
            ..default()
        };
        let mut world = World::new();
        world.insert_resource(TerrainDataStore::default());
        world.spawn(odd.clone());

        crate::sidecar_io::import_terrain_sidecars(
            &mut world,
            &scene_path.to_string_lossy(),
            jackdaw_editor_core::scene_io::SidecarImport::Reload,
        );
        let store = world.resource::<TerrainDataStore>();
        assert!(
            !store.is_load_failed("zone.terrain-0.jdterrain"),
            "a 2^k + 1 vertex grid is storable and must not be quarantined",
        );
        // A 129-vertex grid lands in the 2x2 block of 128-cell regions that holds it, so the
        // terrain is 256 cells across with the authored 129 embedded at its corner. Each
        // authored height stays on the cell it described; the rest is ground the regions
        // brought with them.
        let document = store.get("zone.terrain-0.jdterrain").expect("loaded");
        assert_eq!(document.grid_resolution(), 256);
        for (at, want) in heights.iter().enumerate() {
            let (x, z) = ((at % 129) as i32, (at / 129) as i32);
            assert_eq!(document.regions.height_at(x, z), *want);
        }
        assert!(
            world
                .resource_mut::<TerrainDataStore>()
                .entry_for(&odd)
                .is_some(),
            "edits to it must be accepted",
        );

        export_terrain_sidecars(&mut world, &scene_path.to_string_lossy()).expect("save succeeds");
        let reloaded = read_decode(&sidecar_path).expect("sidecar rewritten");
        for (at, want) in heights.iter().enumerate() {
            let (x, z) = ((at % 129) as i32, (at / 129) as i32);
            assert_eq!(
                reloaded.regions.height_at(x, z),
                *want,
                "the rewrite must keep every height, seam row included",
            );
        }
        assert_eq!(reloaded.regions.region_size().get(), 128);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A pre-region sidecar opens, migrates, and is rewritten in the current
    /// format with no user action.
    #[test]
    fn a_pre_region_sidecar_migrates_on_load_and_saves_in_the_current_format() {
        let tmp = unique_tmp_dir("migrate");
        let scene_path = tmp.join("zone.bsn");
        let sidecar_path = tmp.join("zone.terrain-0.jdterrain");
        std::fs::write(
            &sidecar_path,
            sidecar::encode(&sculpted()).expect("v1 encodes"),
        )
        .expect("write sidecar");

        let mut world = World::new();
        world.insert_resource(TerrainDataStore::default());
        world.spawn(jackdaw_scene_types::Terrain {
            resolution: 4,
            data_path: "zone.terrain-0.jdterrain".to_string(),
            ..default()
        });
        crate::sidecar_io::import_terrain_sidecars(
            &mut world,
            &scene_path.to_string_lossy(),
            jackdaw_editor_core::scene_io::SidecarImport::Reload,
        );
        assert_eq!(
            world
                .resource::<TerrainDataStore>()
                .heights("zone.terrain-0.jdterrain"),
            sculpted().heights.as_slice(),
        );

        export_terrain_sidecars(&mut world, &scene_path.to_string_lossy()).expect("save succeeds");
        let rewritten = std::fs::read(&sidecar_path).expect("read back");
        assert_eq!(
            u16::from_le_bytes([rewritten[8], rewritten[9]]),
            sidecar::VERSION_11,
        );
        // The load settled this terrain onto the geometry its declared rectangle drew with
        // (four vertices across the default 100 metres, cornered at -size/2) and the rewrite
        // records it, so reading the file back needs no rectangle.
        let mut migrated = document(&sculpted());
        migrated.grid = Some(sidecar::GridGeometry {
            cell_size: 100.0 / 3.0,
            anchor: Vec2::splat(-50.0),
        });
        assert_eq!(
            read_decode(&sidecar_path),
            Some(migrated),
            "the migrated document must survive the rewrite",
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A terrain that was edited and then flattened persists as a region on
    /// disk, rather than an empty document that reopens as no terrain.
    #[test]
    fn a_flat_but_authored_terrain_round_trips_as_a_present_region() {
        let tmp = unique_tmp_dir("flat-authored");
        let scene_path = tmp.join("zone.bsn");
        let data_path = "zone.terrain-0.jdterrain";
        let terrain = jackdaw_scene_types::Terrain {
            resolution: 4,
            data_path: data_path.to_string(),
            ..default()
        };

        let mut world = World::new();
        world.insert_resource(store_holding(&terrain));
        world.spawn(terrain.clone());
        {
            let mut store = world.resource_mut::<TerrainDataStore>();
            let mut entry = store.entry_for(&terrain).expect("keyed");
            entry.heights_mut()[0] = 5.0;
            entry.set_heights(&[0.0; 16]);
        }
        export_terrain_sidecars(&mut world, &scene_path.to_string_lossy()).expect("save succeeds");

        let reloaded = read_decode(&tmp.join(data_path)).expect("sidecar written");
        assert_eq!(reloaded.regions.region_count(), 1);
        assert!(reloaded.contiguous_grid().is_some());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// C1 pinning test for the twin bug: a scene whose sidecar was never
    /// copied alongside it (missing, not corrupt) loads flat and must
    /// stay saveable indefinitely, not hard-error on every save attempt.
    #[test]
    fn a_never_loaded_missing_sidecar_stays_saveable() {
        let tmp = unique_tmp_dir("never-loaded");
        let scene_path = tmp.join("zone.bsn");
        let sidecar_path = tmp.join("zone.terrain-0.jdterrain");

        let mut world = World::new();
        world.insert_resource(TerrainDataStore::default());
        world.spawn(jackdaw_scene_types::Terrain {
            resolution: 4,
            data_path: "zone.terrain-0.jdterrain".to_string(),
            ..default()
        });

        crate::sidecar_io::import_terrain_sidecars(
            &mut world,
            &scene_path.to_string_lossy(),
            jackdaw_editor_core::scene_io::SidecarImport::Reload,
        );
        assert!(
            !world
                .resource::<TerrainDataStore>()
                .contains("zone.terrain-0.jdterrain")
        );
        assert!(
            !world
                .resource::<TerrainDataStore>()
                .is_load_failed("zone.terrain-0.jdterrain")
        );

        export_terrain_sidecars(&mut world, &scene_path.to_string_lossy())
            .expect("a scene with a never-loaded terrain must remain saveable");
        assert!(
            !sidecar_path.exists(),
            "nothing should be written for data that never existed"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A tab that was just saved is not stale, so activating it again does
    /// not re-import the terrain.
    #[test]
    fn saving_a_sidecar_leaves_the_store_current_with_the_file() {
        let mut world = World::new();
        let tmp = tempfile::tempdir().expect("temp directory");
        let scene_path = tmp.path().join("zone.bsn");

        let data_path = "zone.terrain-0.jdterrain";
        let mut store = crate::TerrainDataStore::default();
        store.insert(
            data_path.to_string(),
            jackdaw_terrain::RegionTerrainData::from_legacy_v1(&jackdaw_terrain::TerrainData {
                resolution: 2,
                heights: vec![0.0; 4],
                channels: vec![],
            })
            .expect("a power-of-two resolution migrates"),
        );
        world.insert_resource(store);
        world.spawn(jackdaw_scene_types::Terrain {
            resolution: 2,
            data_path: data_path.to_string(),
            ..default()
        });

        let scene_path_str = scene_path.to_string_lossy().into_owned();
        export_terrain_sidecars(&mut world, &scene_path_str).expect("the sidecar writes");

        let written = tmp.path().join(data_path);
        assert!(written.exists(), "the sidecar landed");
        assert!(
            !world
                .resource::<crate::TerrainDataStore>()
                .sidecar_is_stale(data_path, &written),
            "the file the save just wrote must not read as an outside edit"
        );
    }
}
