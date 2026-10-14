//! A game that loads a world authored in the editor: terrain, materials,
//! the navmesh baked beside the scene, and a ball that falls onto the
//! ground rather than through it.
//!
//! The game side is an asset folder with the files the editor wrote,
//! `JackdawPlugin` added after `DefaultPlugins`, and a `JackdawSceneRoot`
//! pointed at the scene. Nothing registers anything, nothing exports anything,
//! and no code here knows how a `.jdterrain` or a `.jdnav` is laid out.
//!
//! The world is authored first into a temp folder, so the example runs against
//! files rather than against a world held in memory.
//!
//! ```text
//! cargo run --example load_terrain_world              # a window
//! cargo run --example load_terrain_world -- --frames 8 # headless, exits
//! cargo run --example load_terrain_world -- <dir>     # author into <dir>
//! ```

#![expect(
    clippy::print_stdout,
    reason = "the example reports what it authored and what loaded back"
)]

use std::path::{Path, PathBuf};

use avian3d::prelude::*;
use bevy::prelude::*;
use jackdaw_runtime::prelude::*;
use jackdaw_terrain::navmesh::{self, NO_NEIGHBOR, NavPolygon, NavmeshArtifact};
use jackdaw_terrain::sidecar::{self, RegionTerrainData, TerrainMaterialSlot};

/// Grid the terrain is authored over. The regions round this up to their own
/// size, and the document's grid is what ends up drawn.
const AUTHORED: u32 = 65;

/// Cells across one edge of a coarse navmesh polygon.
const NAV_CELLS: u32 = 32;

/// How far above the ground the ball is let go, and how big it is.
const DROP: f32 = 60.0;
const BALL_RADIUS: f32 = 2.0;

/// What the editor writes for a terrain: a cell size, and the sidecar
/// name relative to the scene file.
const SCENE: &str = "\
#Terrain
jackdaw_scene_types::types::Terrain {
    cell_size: 1.0,
    data_path: \"scene.terrain-0.jdterrain\",
}
";

/// One material slot's material. A slot with textures needs image files
/// beside it; a bare colour is enough to show the catalog resolving.
const GRASS: &str = "\
#grass
bevy_pbr::pbr_material::StandardMaterial {
    base_color: bevy_color::color::Color::Srgba(bevy_color::srgba::Srgba { red: 0.35, green: 0.5, blue: 0.25, alpha: 1.0 }),
}
";

fn main() -> AppExit {
    let mut frames = None;
    let mut project = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--frames" => frames = args.next().and_then(|n| n.parse::<u32>().ok()),
            other => project = Some(PathBuf::from(other)),
        }
    }
    let project =
        project.unwrap_or_else(|| std::env::temp_dir().join("jackdaw_load_terrain_world"));
    let assets = project.join("assets");
    author_world(&assets);
    println!("authored {}", assets.display());

    let mut app = App::new();
    // The asset folder is the only thing a game configures, and it configures
    // it on Bevy: sidecars and the catalog are found through the same
    // `AssetPlugin` the scene is.
    let asset_plugin = AssetPlugin {
        file_path: assets.to_string_lossy().into_owned(),
        ..default()
    };
    match frames {
        // Headless: no window, no GPU, no renderer. The scene loads, the
        // terrain meshes, and the navmesh answers.
        Some(frames) => {
            app.add_plugins(MinimalPlugins)
                .add_plugins(bevy::transform::TransformPlugin)
                .add_plugins(asset_plugin)
                .init_asset::<Mesh>()
                .insert_resource(FramesLeft(frames))
                .add_systems(Update, count_down);
        }
        None => {
            app.add_plugins(DefaultPlugins.set(asset_plugin));
        }
    }

    // Avian simulates what the runtime builds. The terrain's collider comes off
    // the same heights it is drawn from, so the ball lands on the ground the
    // window shows.
    app.add_plugins(JackdawPlugin)
        .add_plugins(PhysicsPlugins::default())
        .add_systems(Startup, open_scene)
        .add_systems(Update, (orbit, report_navmesh, report_ball))
        .run()
}

/// Everything the editor would have written, written here instead: the scene,
/// its terrain sidecar, the material its slot names, and a bake under the
/// scene's name.
fn author_world(assets: &Path) {
    std::fs::create_dir_all(assets.join("materials")).expect("assets dir");
    let data = authored_terrain();
    std::fs::write(assets.join("scene.bsn"), SCENE).expect("scene");
    std::fs::write(
        assets.join("scene.terrain-0.jdterrain"),
        sidecar::save(&data).expect("encode the terrain"),
    )
    .expect("sidecar");
    std::fs::write(assets.join("materials/grass.material.bsn"), GRASS).expect("material");
    std::fs::write(
        assets.join("scene.jdnav"),
        navmesh::encode(&baked_navmesh(&data)),
    )
    .expect("navmesh");
}

/// Rolling ground under one material slot.
fn authored_terrain() -> RegionTerrainData {
    let mut data = RegionTerrainData {
        materials: vec![TerrainMaterialSlot::new("grass")],
        ..default()
    };
    let _ = data.regions.ensure_grid(AUTHORED);
    let resolution = data.grid_shape(Vec2::ZERO, 0).resolution;
    let heights: Vec<f32> = (0..resolution * resolution)
        .map(|index| {
            let x = (index % resolution) as f32 / 24.0;
            let z = (index / resolution) as f32 / 24.0;
            (x.sin() + z.cos()) * 3.0
        })
        .collect();
    data.set_grid_heights(&heights);
    data
}

/// A bake over that ground: a grid of walkable polygons that know their
/// neighbors, over the terrain's triangles as the detail surface. The editor's
/// baker produces this from the terrain and the scene's geometry; building the
/// same artifact here keeps the example free of the editor.
fn baked_navmesh(data: &RegionTerrainData) -> NavmeshArtifact {
    let shape = data.grid_shape(Vec2::ZERO, 0);
    let cell = shape.size.x / (shape.resolution - 1) as f32;
    let surface = navmesh::surface_mesh(&data.regions, Vec2::splat(cell), shape.origin);

    let across = (shape.resolution - 1).div_ceil(NAV_CELLS);
    let corner = |x: u32, z: u32| {
        let step = NAV_CELLS as f32 * cell;
        Vec3::new(
            shape.origin.x + x as f32 * step,
            0.0,
            shape.origin.y + z as f32 * step,
        )
    };
    let vertices: Vec<Vec3> = (0..=across)
        .flat_map(|z| (0..=across).map(move |x| corner(x, z)))
        .collect();
    let index = |x: u32, z: u32| z * (across + 1) + x;
    let neighbor = |x: i64, z: i64| {
        if x < 0 || z < 0 || x >= i64::from(across) || z >= i64::from(across) {
            NO_NEIGHBOR
        } else {
            (z * i64::from(across) + x) as u32
        }
    };
    let polygons = (0..across)
        .flat_map(|z| (0..across).map(move |x| (x, z)))
        .map(|(x, z)| NavPolygon {
            area: 63,
            corners: vec![
                index(x, z),
                index(x + 1, z),
                index(x + 1, z + 1),
                index(x, z + 1),
            ],
            neighbors: vec![
                neighbor(i64::from(x), i64::from(z) - 1),
                neighbor(i64::from(x) + 1, i64::from(z)),
                neighbor(i64::from(x), i64::from(z) + 1),
                neighbor(i64::from(x) - 1, i64::from(z)),
            ],
        })
        .collect();

    NavmeshArtifact {
        params: navmesh::BakeParams::default(),
        source_hash: 0,
        terrain: "scene.terrain-0.jdterrain".to_string(),
        vertices,
        polygons,
        surface_vertices: surface.vertices,
        surface_triangles: surface.triangles,
    }
}

/// A scene root over an asset-server handle and a camera to look at it with.
/// The terrain lays its finest detail around whichever camera is marked
/// `TerrainViewer`.
fn open_scene(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn(JackdawSceneRoot(assets.load("scene.bsn")));
    commands.spawn((
        Camera3d::default(),
        TerrainViewer,
        Transform::from_xyz(128.0, 90.0, 260.0).looking_at(Vec3::new(128.0, 0.0, 128.0), Vec3::Y),
    ));
    commands.spawn((
        Ball,
        RigidBody::Dynamic,
        Collider::sphere(BALL_RADIUS),
        // A sphere rolls down any gradient and nothing here damps rolling;
        // held upright it slides, and friction settles it.
        LockedAxes::ROTATION_LOCKED,
        Mesh3d(meshes.add(Sphere::new(BALL_RADIUS))),
        MeshMaterial3d(materials.add(Color::srgb(0.9, 0.4, 0.2))),
        Transform::from_xyz(128.0, DROP, 128.0),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 10_000.0,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, -0.6, -0.9, 0.0)),
    ));
}

/// Circle the camera around the middle of the terrain.
fn orbit(time: Res<Time>, mut cameras: Query<&mut Transform, With<Camera3d>>) {
    let middle = Vec3::new(128.0, 0.0, 128.0);
    let angle = time.elapsed_secs() * 0.15;
    for mut transform in &mut cameras {
        transform.translation = middle + Vec3::new(angle.sin() * 200.0, 90.0, angle.cos() * 200.0);
        transform.look_at(middle, Vec3::Y);
    }
}

/// Say what came off disk, once, when the scene has finished loading.
///
/// The navmesh arrives as a component on the scene root, so a game with two
/// scenes open queries the one it means.
fn report_navmesh(
    navmeshes: Query<&JackdawNavmesh, Added<JackdawNavmesh>>,
    terrain: Query<&Mesh3d>,
) {
    for navmesh in &navmeshes {
        println!("navmesh: {} polygons", navmesh.polygons.len());
        println!("terrain: {} surfaces drawn", terrain.iter().count());

        // The two queries a game validates a move with: whether the
        // destination is on the mesh, and how high the ground is there.
        for point in [Vec2::new(128.0, 128.0), Vec2::new(-40.0, 128.0)] {
            match navmesh.contains_point(point) {
                true => {
                    let height = navmesh.height_at(point).unwrap_or_default();
                    println!("  {point} is walkable, ground at y = {height:.2}");
                }
                false => println!("  {point} is off the navmesh"),
            }
        }
    }
}

/// The ball dropped over the terrain.
#[derive(Component)]
struct Ball;

/// Say where the ball settled, once it has. A ball that falls through the
/// ground never settles and never reports.
fn report_ball(balls: Query<(&Transform, &LinearVelocity), With<Ball>>, mut said: Local<bool>) {
    for (transform, velocity) in &balls {
        let fallen = transform.translation.y < DROP - 1.0;
        if !*said && fallen && velocity.length() < 0.05 {
            *said = true;
            println!(
                "ball at rest on the ground, y = {:.2}",
                transform.translation.y
            );
        }
    }
}

/// Frames left before a headless run exits.
#[derive(Resource)]
struct FramesLeft(u32);

fn count_down(mut left: ResMut<FramesLeft>, mut exit: MessageWriter<AppExit>) {
    left.0 = left.0.saturating_sub(1);
    if left.0 == 0 {
        exit.write(AppExit::Success);
    }
}
