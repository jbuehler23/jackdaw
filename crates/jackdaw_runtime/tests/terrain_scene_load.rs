#![cfg(feature = "terrain")]
//! A scene's terrain drawn the way a built game draws it: the sidecar is found
//! beside the scene file, decoded, and turned into clipmap surfaces under the
//! terrain entity.
//!
//! Headless, so no texture decodes and the surfaces carry the untextured
//! material rather than the splat one.

use std::path::{Path, PathBuf};

use bevy::asset::AssetApp;
use bevy::prelude::*;
use jackdaw_runtime::{JackdawCatalogPath, JackdawPlugin, JackdawScene, JackdawSceneRoot};
use jackdaw_terrain::sidecar::{self, RegionTerrainData, TerrainMaterialSlot};

const RESOLUTION: u32 = 65;

/// A scene naming a cell size and no rectangle.
const SCENE: &str = "\
#Terrain
jackdaw_scene_types::types::Terrain {
    cell_size: 1.0,
    data_path: \"zone.terrain-0.jdterrain\",
}
";

/// The same scene in the older form, declaring a rectangle. It loads through
/// the migration and draws its ground at the spacing that rectangle implies.
const LEGACY_SCENE: &str = "\
#Terrain
jackdaw_scene_types::types::Terrain {
    resolution: 65,
    size: glam::Vec2 { x: 64.0, y: 64.0 },
    data_path: \"zone.terrain-0.jdterrain\",
}
";

/// An asset root laid out like a shipped game's: the scene at the top, its
/// sidecar beside it, the materials it names in `materials/`.
fn project(sidecar_bytes: Option<Vec<u8>>) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let assets = tmp.path().join("assets");
    std::fs::create_dir_all(assets.join("materials")).expect("materials dir");
    std::fs::write(assets.join("catalog.bsn"), "// no catalog entries\n").expect("catalog");
    std::fs::write(
        assets.join("materials/grass.material.bsn"),
        "#grass\nbevy_pbr::pbr_material::StandardMaterial {\n    \
         base_color_texture: \"textures/grass.png\",\n}\n",
    )
    .expect("material");
    if let Some(bytes) = sidecar_bytes {
        std::fs::write(assets.join("zone.terrain-0.jdterrain"), bytes).expect("sidecar");
    }
    tmp
}

/// A hill under one material slot, at the scene's resolution.
fn authored_sidecar() -> Vec<u8> {
    let mut data = RegionTerrainData {
        materials: vec![TerrainMaterialSlot::new("grass")],
        ..default()
    };
    let _ = data.regions.ensure_grid(RESOLUTION);
    let heights: Vec<f32> = (0..RESOLUTION * RESOLUTION)
        .map(|index| {
            let x = (index % RESOLUTION) as f32 / RESOLUTION as f32;
            let z = (index / RESOLUTION) as f32 / RESOLUTION as f32;
            (x * z * 8.0).sin() * 3.0
        })
        .collect();
    data.set_grid_heights(&heights);
    sidecar::save(&data).expect("encode")
}

fn game(assets_root: &Path) -> App {
    game_with(assets_root, SCENE)
}

fn game_with(assets_root: &Path, scene: &str) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(bevy::asset::AssetPlugin {
        file_path: assets_root.to_string_lossy().into_owned(),
        ..default()
    });
    // The mesh store a windowed game gets from its render plugins.
    app.init_asset::<Mesh>();
    // Both the catalog and the sidecar are resolved against this root.
    app.insert_resource(JackdawCatalogPath(assets_root.join("catalog.bsn")));
    app.add_plugins(JackdawPlugin);
    app.world_mut().spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 20.0, 40.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    let handle = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(scene.to_string(), PathBuf::new()));
    app.world_mut().spawn(JackdawSceneRoot(handle));
    app.update();
    app.update();
    app
}

/// Surfaces the terrain drew, with the vertex count of each.
fn surfaces(app: &mut App) -> Vec<usize> {
    let meshes: Vec<Handle<Mesh>> = app
        .world_mut()
        .query::<&Mesh3d>()
        .iter(app.world())
        .map(|mesh| mesh.0.clone())
        .collect();
    let assets = app.world().resource::<Assets<Mesh>>();
    meshes
        .iter()
        .filter_map(|handle| assets.get(handle))
        .map(Mesh::count_vertices)
        .collect()
}

#[test]
fn an_authored_terrain_draws_a_clipmap_surface() {
    let project = project(Some(authored_sidecar()));
    let mut app = game(&project.path().join("assets"));

    let drawn = surfaces(&mut app);

    assert!(
        !drawn.is_empty(),
        "the terrain's sidecar decoded, so it has ground to draw"
    );
    assert!(drawn.iter().all(|count| *count > 0));
}

#[test]
fn a_terrain_whose_sidecar_will_not_decode_draws_nothing() {
    let project = project(Some(b"not a terrain at all".to_vec()));
    let mut app = game(&project.path().join("assets"));

    assert!(surfaces(&mut app).is_empty());
}

/// A terrain's extent is the cells it holds, so with no sidecar it draws
/// nothing rather than a rectangle of flat ground it never declared.
#[test]
fn a_terrain_with_no_sidecar_on_disk_draws_no_ground() {
    let project = project(None);
    let mut app = game(&project.path().join("assets"));

    assert!(surfaces(&mut app).is_empty());
}

/// A scene declaring a rectangle draws the same ground as one naming a cell
/// size, because the sidecar states its own geometry and both scene texts
/// reconcile onto it.
#[test]
fn a_legacy_scene_draws_the_same_ground_as_a_current_one() {
    let project = project(Some(authored_sidecar()));
    let mut current = game_with(&project.path().join("assets"), SCENE);
    let mut legacy = game_with(&project.path().join("assets"), LEGACY_SCENE);

    let drawn = surfaces(&mut current);
    assert!(!drawn.is_empty(), "the current scene draws its ground");
    assert_eq!(drawn, surfaces(&mut legacy));
}
