#![cfg(all(feature = "terrain", feature = "physics"))]
//! The ground a game stands on is the ground it draws.
//!
//! With both the terrain and physics features, every loaded terrain gets a
//! heightfield collider built from the heights the mesher draws from. These
//! tests check that the two agree: a ray dropped on a world point meets the
//! collider at the height stored for that point, wherever the grid is
//! anchored.
//!
//! The ground is authored as a tilted plane, at a different gradient along
//! each axis. Between grid points a triangulated surface follows the plane
//! exactly, so every sample has one answer and no interpolation error, while a
//! collider at the wrong spacing, with its axes swapped, or anchored off the
//! grid reads a different height at every sample.

use std::path::Path;

use avian3d::prelude::*;
use bevy::asset::AssetApp;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use jackdaw_runtime::{JackdawPlugin, JackdawScene, JackdawSceneRoot};
use jackdaw_terrain::sidecar::{self, GridGeometry, RegionTerrainData, TerrainMaterialSlot};

/// Cells per edge of the authored grid, and the region size that holds it
/// exactly, so the document's grid is this and not a rounded-up one.
const RESOLUTION: u32 = 64;
const CELL_SIZE: f32 = 2.0;

/// A sidecar name and no rectangle. Where the cells sit is the sidecar's.
const SCENE: &str = "\
#Terrain
jackdaw_scene_types::types::Terrain {
    data_path: \"zone.terrain-0.jdterrain\",
}
";

/// The authored ground, at a fractional grid coordinate. Gentle enough that a
/// body settles on it rather than sliding off.
fn height_at(x: f32, z: f32) -> f32 {
    2.0 + x * 0.3 - z * 0.16
}

/// A terrain whose cells sit at `anchor`, `CELL_SIZE` metres apart.
fn authored_terrain(anchor: Vec2) -> RegionTerrainData {
    let mut data = RegionTerrainData {
        materials: vec![TerrainMaterialSlot::new("grass")],
        regions: jackdaw_terrain::TerrainRegions::new(
            jackdaw_terrain::RegionSize::new(RESOLUTION).expect("a power of two"),
        ),
        grid: Some(GridGeometry {
            cell_size: CELL_SIZE,
            anchor,
        }),
        ..default()
    };
    let _ = data.regions.ensure_grid(RESOLUTION);
    let heights: Vec<f32> = (0..RESOLUTION * RESOLUTION)
        .map(|index| height_at((index % RESOLUTION) as f32, (index / RESOLUTION) as f32))
        .collect();
    data.set_grid_heights(&heights);
    data
}

/// An asset folder laid out like a shipped game's.
fn project(data: &RegionTerrainData) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let assets = tmp.path().join("assets");
    std::fs::create_dir_all(&assets).expect("assets dir");
    std::fs::write(assets.join("scene.bsn"), SCENE).expect("scene");
    std::fs::write(
        assets.join("zone.terrain-0.jdterrain"),
        sidecar::save(data).expect("encode"),
    )
    .expect("sidecar");
    tmp
}

/// Everything a game does: an asset folder, the plugin, a scene root, and
/// avian's own plugins to simulate what the plugin builds.
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
    app.add_plugins(PhysicsPlugins::default());
    // Avian registers its diagnostics resources from `Plugin::finish`, and
    // the systems reading them require those resources. `run` would call
    // these; a driven app calls them itself.
    app.finish();
    app.cleanup();
    app.world_mut().spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 60.0, 60.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    let handle: Handle<JackdawScene> = app.world().resource::<AssetServer>().load("scene.bsn");
    app.world_mut().spawn(JackdawSceneRoot(handle));

    // The scene arrives on an asset-server task, so the spawn loop is pumped
    // until it does.
    for _ in 0..200 {
        app.update();
        if app
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
    // A few more frames for the sidecar to decode and the collider to build.
    for _ in 0..4 {
        app.update();
    }
    app
}

/// The terrain's ground collider and where it sits in the world.
///
/// A child of the terrain, because a heightfield is centred on its own origin
/// while a terrain's cells start at its grid anchor. The child's transform
/// carries the difference.
fn ground(app: &mut App) -> Option<(Collider, GlobalTransform)> {
    let world = app.world_mut();
    world
        .query_filtered::<(&Collider, &GlobalTransform), With<ChildOf>>()
        .iter(world)
        .next()
        .map(|(collider, transform)| (collider.clone(), *transform))
}

/// Where a ray dropped straight down on `(x, z)` meets the ground, or `None`
/// if it meets nothing.
fn ground_height_at(app: &mut App, x: f32, z: f32) -> Option<f32> {
    let (collider, transform) = ground(app)?;
    let from = Vec3::new(x, 500.0, z);
    collider
        .cast_ray(
            transform.translation(),
            transform.rotation(),
            from,
            Vec3::NEG_Y,
            1000.0,
            true,
        )
        .map(|(distance, _)| from.y - distance)
}

/// Grid coordinates the ground is sampled at: near the first cell, off both
/// axes of symmetry, and in the last cell.
///
/// None of them is a grid vertex. A ray through a vertex meets a triangulated
/// surface on a shared corner, which parry answers inconsistently.
const SAMPLES: [(f32, f32); 3] = [
    (0.5, 0.5),
    (7.25, 41.75),
    (RESOLUTION as f32 - 1.5, RESOLUTION as f32 - 1.5),
];

/// Every sample reads the height that was authored at it.
fn assert_ground_follows_the_grid(app: &mut App, anchor: Vec2) {
    for (x, z) in SAMPLES {
        let world = anchor + Vec2::new(x, z) * CELL_SIZE;
        let hit = ground_height_at(app, world.x, world.y)
            .unwrap_or_else(|| panic!("ground under grid point ({x}, {z}) at {world}"));
        let authored = height_at(x, z);
        assert!(
            (hit - authored).abs() < 1e-3,
            "grid point ({x}, {z}) is at {world} and was authored at {authored}, but the ground \
             there is {hit}"
        );
    }
}

#[test]
fn a_dropped_ray_meets_the_ground_the_sidecar_stores() {
    let project = project(&authored_terrain(Vec2::ZERO));
    let mut app = game(&project.path().join("assets"));

    assert!(ground(&mut app).is_some(), "the terrain got a collider");
    assert_ground_follows_the_grid(&mut app, Vec2::ZERO);
}

/// A terrain migrated from a declared rectangle anchors its cells at
/// `-size/2` rather than at the entity, and parry's heightfield is centred on
/// its own origin, so the collider must carry that offset or sit half a
/// terrain away from the drawn ground.
#[test]
fn a_grid_anchored_away_from_its_entity_collides_where_it_draws() {
    let anchor = Vec2::splat((RESOLUTION - 1) as f32 * CELL_SIZE / -2.0);
    let project = project(&authored_terrain(anchor));
    let mut app = game(&project.path().join("assets"));

    assert_ground_follows_the_grid(&mut app, anchor);
    // Nothing stands where an unanchored grid would put ground.
    assert_eq!(
        ground_height_at(&mut app, 0.5 * CELL_SIZE, 0.5 * CELL_SIZE),
        None,
        "the ground is where the cells were anchored, and nowhere else"
    );
}

/// An unsculpted terrain holds no regions, so it has no cells and no
/// collider.
#[test]
fn a_terrain_with_no_cells_has_no_ground_to_collide_with() {
    let data = RegionTerrainData {
        materials: vec![TerrainMaterialSlot::new("grass")],
        ..default()
    };
    assert_eq!(data.grid_resolution(), 0, "the fixture holds no regions");
    let project = project(&data);
    let mut app = game(&project.path().join("assets"));

    let world = app.world_mut();
    assert!(
        world
            .query::<&jackdaw_scene_types::Terrain>()
            .iter(world)
            .next()
            .is_some(),
        "the terrain still loaded"
    );
    assert!(
        ground(&mut app).is_none(),
        "a terrain with no cells has nothing to stand on"
    );
}

/// A body dropped over the ground stops on it. The ray tests pin where the
/// shape is; this one pins that avian generates contacts against it.
#[test]
fn a_falling_body_comes_to_rest_on_the_authored_ground() {
    const RADIUS: f32 = 0.5;
    let project = project(&authored_terrain(Vec2::ZERO));
    let mut app = game(&project.path().join("assets"));

    // A driven app advances by however long its systems take, so a fixed
    // step per update is what turns the frame count below into a duration.
    app.insert_resource(TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f32(1.0 / 60.0),
    ));

    let (x, z) = (19.5, 31.5);
    let over = Vec2::new(x, z) * CELL_SIZE;
    let ball = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::sphere(RADIUS),
            // A sphere rolls down any gradient; held upright it slides
            // instead, and this ground is well short of the angle friction
            // gives way at.
            LockedAxes::ROTATION_LOCKED,
            Transform::from_xyz(over.x, height_at(x, z) + 20.0, over.y),
        ))
        .id();

    for _ in 0..600 {
        app.update();
    }

    let resting = app
        .world()
        .get::<Transform>(ball)
        .expect("the ball is still there")
        .translation;
    let under = height_at(resting.x / CELL_SIZE, resting.z / CELL_SIZE);
    assert!(
        (resting.y - (under + RADIUS)).abs() < 0.1,
        "the ball rests on the ground at {}, not at {}",
        under + RADIUS,
        resting.y
    );
}

/// Hot reload respawns the terrain, and the collider is rebuilt from the
/// document inserted onto the new entity.
#[test]
fn a_reloaded_scene_brings_its_ground_back() {
    let project = project(&authored_terrain(Vec2::ZERO));
    let assets = project.path().join("assets");
    let mut app = game(&assets);
    assert!(ground(&mut app).is_some(), "ground to begin with");

    // Sculpted elsewhere and saved, so the file on disk disagrees with the
    // loaded collider.
    let moved = Vec2::splat(100.0);
    std::fs::write(
        assets.join("zone.terrain-0.jdterrain"),
        sidecar::save(&authored_terrain(moved)).expect("encode"),
    )
    .expect("sidecar");

    let world = app.world_mut();
    let handle = world
        .query::<&JackdawSceneRoot>()
        .iter(world)
        .next()
        .expect("a scene root")
        .0
        .clone();
    world.write_message(bevy::asset::AssetEvent::Modified { id: handle.id() });
    for _ in 0..8 {
        app.update();
    }

    assert_ground_follows_the_grid(&mut app, moved);
}
