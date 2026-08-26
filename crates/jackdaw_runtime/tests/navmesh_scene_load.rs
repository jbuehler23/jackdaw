#![cfg(feature = "terrain")]
//! One app holding the whole game-side contract: an asset folder, a scene
//! path, and [`JackdawPlugin`]. What comes back is the authored ground drawn
//! at the extent the sidecar states, plus the navmesh baked beside the scene.
//!
//! Headless, so no texture decodes and the surfaces carry the untextured
//! material rather than the splat one.

use std::path::Path;

use bevy::asset::AssetApp;
use bevy::prelude::*;
use jackdaw_runtime::{JackdawNavmesh, JackdawPlugin, JackdawScene, JackdawSceneRoot};
use jackdaw_terrain::navmesh::{self, NO_NEIGHBOR, NavPolygon, NavmeshArtifact};
use jackdaw_terrain::sidecar::{self, RegionTerrainData, TerrainMaterialSlot};

/// The grid the author asked for. The regions round it up to their own size,
/// and the document's grid, not this one, is what gets drawn.
const AUTHORED: u32 = 65;
const CELL_SIZE: f32 = 1.0;

/// What the editor writes for a terrain: a cell size, and a sidecar name
/// relative to the scene file.
const SCENE: &str = "\
#Terrain
jackdaw_scene_types::types::Terrain {
    cell_size: 1.0,
    data_path: \"scene.terrain-0.jdterrain\",
}
";

/// A hill under one material slot, sculpted across the whole grid the regions
/// hold.
fn authored_terrain() -> RegionTerrainData {
    let mut data = RegionTerrainData {
        materials: vec![TerrainMaterialSlot::new("grass")],
        ..default()
    };
    let _ = data.regions.ensure_grid(AUTHORED);
    let resolution = data.grid_shape(Vec2::ZERO, 0).resolution;
    let heights: Vec<f32> = (0..resolution * resolution)
        .map(|index| {
            let x = (index % resolution) as f32 / resolution as f32;
            let z = (index / resolution) as f32 / resolution as f32;
            (x * z * 8.0).sin() * 3.0
        })
        .collect();
    data.set_grid_heights(&heights);
    data
}

/// A bake over that ground: the detail surface is the terrain's own triangles,
/// and one coarse polygon spans the footprint. A real bake has far more
/// polygons; the runtime reads whatever number is on disk.
fn baked_navmesh(data: &RegionTerrainData) -> NavmeshArtifact {
    let shape = data.grid_shape(Vec2::ZERO, 0);
    let surface = navmesh::surface_mesh(&data.regions, Vec2::splat(CELL_SIZE), shape.origin);
    let corners = [
        Vec3::new(shape.origin.x, 0.0, shape.origin.y),
        Vec3::new(shape.origin.x + shape.size.x, 0.0, shape.origin.y),
        Vec3::new(
            shape.origin.x + shape.size.x,
            0.0,
            shape.origin.y + shape.size.y,
        ),
        Vec3::new(shape.origin.x, 0.0, shape.origin.y + shape.size.y),
    ];
    NavmeshArtifact {
        params: jackdaw_terrain::navmesh::BakeParams::default(),
        source_hash: 0,
        terrain: "scene.terrain-0.jdterrain".to_string(),
        vertices: corners.to_vec(),
        polygons: vec![NavPolygon {
            area: 63,
            corners: vec![0, 1, 2, 3],
            neighbors: vec![NO_NEIGHBOR; 4],
        }],
        surface_vertices: surface.vertices,
        surface_triangles: surface.triangles,
    }
}

/// An asset folder laid out like a shipped game's: the scene at the top, its
/// sidecar and its bake beside it under the scene's own name.
fn project() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let assets = tmp.path().join("assets");
    std::fs::create_dir_all(&assets).expect("assets dir");
    let data = authored_terrain();
    std::fs::write(assets.join("scene.bsn"), SCENE).expect("scene");
    std::fs::write(
        assets.join("scene.terrain-0.jdterrain"),
        sidecar::save(&data).expect("encode"),
    )
    .expect("sidecar");
    std::fs::write(
        assets.join("scene.jdnav"),
        navmesh::encode(&baked_navmesh(&data)),
    )
    .expect("navmesh");
    tmp
}

/// Everything a game does: point Bevy at its assets, add the plugin, spawn a
/// scene root over an asset-server handle.
fn game(assets: &Path) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(bevy::asset::AssetPlugin {
        file_path: assets.to_string_lossy().into_owned(),
        ..default()
    });
    // The mesh store a windowed game gets from its render plugins.
    app.init_asset::<Mesh>();
    app.add_plugins(JackdawPlugin);
    app.world_mut().spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 20.0, 40.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    let handle: Handle<JackdawScene> = app.world().resource::<AssetServer>().load("scene.bsn");
    app.world_mut().spawn(JackdawSceneRoot(handle));

    // The scene arrives on an asset-server task, so the spawn loop is pumped
    // until it does.
    for _ in 0..200 {
        app.update();
        if app
            .world_mut()
            .query::<&JackdawSceneRoot>()
            .iter(app.world())
            .count()
            > 0
            && app
                .world_mut()
                .query::<&jackdaw_scene_types::Terrain>()
                .iter(app.world())
                .next()
                .is_some()
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // A few more frames for the sidecar to decode and the clipmap to mesh.
    for _ in 0..4 {
        app.update();
    }
    app
}

/// Every vertex the terrain drew, in world space.
fn drawn_vertices(app: &mut App) -> Vec<Vec3> {
    let drawn: Vec<(Handle<Mesh>, GlobalTransform)> = app
        .world_mut()
        .query::<(&Mesh3d, &GlobalTransform)>()
        .iter(app.world())
        .map(|(mesh, transform)| (mesh.0.clone(), *transform))
        .collect();
    let meshes = app.world().resource::<Assets<Mesh>>();
    let mut points = Vec::new();
    for (handle, transform) in &drawn {
        let Some(positions) = meshes
            .get(handle)
            .and_then(|mesh| mesh.attribute(Mesh::ATTRIBUTE_POSITION))
            .and_then(|values| values.as_float3())
        else {
            continue;
        };
        points.extend(
            positions
                .iter()
                .map(|position| transform.transform_point(Vec3::from(*position))),
        );
    }
    points
}

/// The ground lands on the grid the sidecar states, at its cell size and
/// anchor, and carries the heights sculpted onto it. A terrain drawn at
/// another spacing or anchor would put a different height under the same
/// world point.
#[test]
fn a_scene_folder_and_the_plugin_are_the_whole_of_what_a_game_provides() {
    let project = project();
    let mut app = game(&project.path().join("assets"));

    let drawn = drawn_vertices(&mut app);
    assert!(!drawn.is_empty(), "the authored terrain drew its ground");

    let data = authored_terrain();
    let shape = data.grid_shape(Vec2::ZERO, 0);
    assert_eq!(
        shape.size,
        Vec2::splat((shape.resolution - 1) as f32 * CELL_SIZE)
    );

    // A grid vertex a quarter of the way across, which the finest clipmap
    // ring samples one-for-one.
    let cell = UVec2::splat(shape.resolution / 4);
    let expected = shape.origin + cell.as_vec2() * CELL_SIZE;
    let sculpted = data.grid_heights()[(cell.y * shape.resolution + cell.x) as usize];

    let nearest = drawn
        .iter()
        .min_by(|a, b| {
            let distance = |v: &Vec3| (Vec2::new(v.x, v.z) - expected).length_squared();
            distance(a).total_cmp(&distance(b))
        })
        .expect("some vertex is nearest");
    assert!(
        (Vec2::new(nearest.x, nearest.z) - expected).length() < 1e-3,
        "a vertex sits on the grid point at {expected}, not {nearest}"
    );
    assert!(
        (nearest.y - sculpted).abs() < 1e-3,
        "the sculpted height {sculpted} is what is drawn, not {}",
        nearest.y
    );
}

#[test]
fn the_bake_beside_the_scene_loads_with_it() {
    let project = project();
    let mut app = game(&project.path().join("assets"));

    let world = app.world_mut();
    let mut roots = world.query::<(&JackdawSceneRoot, &JackdawNavmesh)>();
    let (_, nav) = roots
        .iter(world)
        .next()
        .expect("the navmesh beside the scene loaded onto its root");

    assert_eq!(nav.polygons.len(), 1);
    // The middle of the footprint is walkable, at the height the terrain was
    // sculpted to.
    let shape = authored_terrain().grid_shape(Vec2::ZERO, 0);
    let middle = shape.origin + shape.size / 2.0;
    assert!(nav.contains_point(middle));
    let height = nav.height_at(middle).expect("ground under the middle");
    assert!(height.abs() <= 3.0, "{height}");
    assert!(!nav.contains_point(shape.origin - Vec2::splat(10.0)));
}

#[test]
fn a_scene_with_no_bake_beside_it_loads_without_one() {
    let project = project();
    std::fs::remove_file(project.path().join("assets/scene.jdnav")).expect("remove the bake");
    let mut app = game(&project.path().join("assets"));

    assert!(
        !drawn_vertices(&mut app).is_empty(),
        "the ground still draws"
    );
    let world = app.world_mut();
    assert_eq!(world.query::<&JackdawNavmesh>().iter(world).count(), 0);
}

/// A bake that does not decode is reported and left on disk.
#[test]
fn a_bake_that_will_not_decode_is_not_half_loaded() {
    let project = project();
    std::fs::write(project.path().join("assets/scene.jdnav"), b"not a bake").expect("clobber");
    let mut app = game(&project.path().join("assets"));

    let world = app.world_mut();
    assert_eq!(world.query::<&JackdawNavmesh>().iter(world).count(), 0);
}

/// A reloaded scene spawns onto the same root, so a load that finds no bake
/// must drop the navmesh the root was carrying.
#[test]
fn a_reload_that_finds_no_bake_leaves_none_behind() {
    let project = project();
    let assets = project.path().join("assets");
    let mut app = game(&assets);
    assert_eq!(navmeshes(&mut app), 1, "the bake loaded to begin with");

    std::fs::remove_file(assets.join("scene.jdnav")).expect("remove the bake");
    reload_scene(&mut app);
    assert_eq!(navmeshes(&mut app), 0, "the deleted bake went with it");

    // A broken bake is no more loaded than a missing one: both leave the
    // root bare.
    std::fs::write(assets.join("scene.jdnav"), b"not a bake").expect("clobber");
    reload_scene(&mut app);
    assert_eq!(navmeshes(&mut app), 0);
}

fn navmeshes(app: &mut App) -> usize {
    let world = app.world_mut();
    world.query::<&JackdawNavmesh>().iter(world).count()
}

/// Announce the scene asset as modified, the form a file watcher's reload
/// arrives in: the spawned members are despawned and the surviving root spawns
/// from the asset again.
fn reload_scene(app: &mut App) {
    let world = app.world_mut();
    let handle = world
        .query::<&JackdawSceneRoot>()
        .iter(world)
        .next()
        .expect("a scene root")
        .0
        .clone();
    world.write_message(bevy::asset::AssetEvent::Modified { id: handle.id() });
    for _ in 0..4 {
        app.update();
    }
}
