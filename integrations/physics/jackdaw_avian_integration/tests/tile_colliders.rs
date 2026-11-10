#![cfg(feature = "tilemap")]
//! A painted tilemap collides as it is drawn: the collision shapes of its
//! tiles become static 2D bodies, one per chunk, with a compound collider per
//! physics layer and kind, and sensors for triggers.

use std::path::{Path, PathBuf};
use std::time::Duration;

use avian2d::prelude::*;
use bevy::asset::AssetApp;
use bevy::ecs::reflect::AppTypeRegistry;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use jackdaw_avian_integration::{AvianColliderBridgePlugin, OneWayTile, TileChunkBody};
use jackdaw_runtime::{
    JackdawCatalogPath, JackdawPlugin, JackdawScene, JackdawSceneRoot, TilemapChunkCollision,
    TilemapCollision,
};
use jackdaw_scene_types::Tilemap;
use jackdaw_tilemap::{
    CellCoord, ChunkCoord, ChunkedLayer, CollisionGroup, LayerData, LayerKind, Placement, TileRef,
    TilemapData,
};
use jackdaw_tileset::{CollisionPreset, Tileset, TilesetCollision};

const TILE: f32 = 16.0;

const SOLID: u16 = 0;
const HALF: u16 = 1;
const SLOPE: u16 = 2;
const TRIGGER: u16 = 3;
const ONE_WAY: u16 = 4;

const TILESET: &str = "\
#tiles
jackdaw_tileset::Tileset {
    image: \"tiles.png\",
    tile_size: glam::UVec2 { x: 16, y: 16 },
    grid: glam::UVec2 { x: 5, y: 1 },
    sheets: [
        jackdaw_tileset::TilesetSheet {
            source: \"tiles.png\",
            columns: 5,
            rows: 1,
            first_layer: 0,
        },
    ],
}
";

const SCENE: &str = "\
#Map
jackdaw_scene_types::tilemap::Tilemap {
    data_path: \"zone.tilemap-0.jdtiles\",
    layers: [
        jackdaw_scene_types::tilemap::TilemapLayer { name: \"Ground\", tileset: \"tiles.tileset.bsn\", kind: jackdaw_scene_types::tilemap::TilemapLayerKind::Tile },
    ],
}
";

/// A run of solid ground, a bottom half tile, a rising slope and a mirrored
/// one, a gap, a trigger on physics layer 1, a one-way ledge, and one more
/// solid tile a chunk to the right.
fn painted() -> TilemapData {
    let mut cells: Vec<_> = (0..4).map(|x| (x, 0, TileRef::new(SOLID))).collect();
    cells.extend([
        (4, 0, TileRef::new(HALF)),
        (5, 0, TileRef::new(SLOPE)),
        (6, 0, TileRef::new(SLOPE).with_flip_x(true)),
        (8, 0, TileRef::new(TRIGGER)),
        (12, 3, TileRef::new(ONE_WAY)),
        (40, 0, TileRef::new(SOLID)),
    ]);
    map_of(cells)
}

/// Flat ground from x = 20 to 64, across the border of the first two
/// chunks, with a rising slope at x = 54 up onto a solid step.
fn seams() -> TilemapData {
    let mut cells: Vec<_> = (20..64).map(|x| (x, 0, TileRef::new(SOLID))).collect();
    cells.push((54, 1, TileRef::new(SLOPE)));
    cells.extend((55..60).map(|x| (x, 1, TileRef::new(SOLID))));
    map_of(cells)
}

fn map_of(cells: Vec<(i32, i32, TileRef)>) -> TilemapData {
    let mut tiles = ChunkedLayer::new();
    for (x, y, tile) in cells {
        tiles.set(CellCoord::new(x, y), tile);
    }
    TilemapData {
        layers: vec![LayerData {
            kind: LayerKind::Tile,
            placement: Placement::OnCells,
            tiles,
            authoring: None,
        }],
    }
}

fn project(map: &TilemapData) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let assets = tmp.path().join("assets");
    std::fs::create_dir_all(&assets).expect("assets");
    std::fs::write(assets.join("catalog.bsn"), "// no catalog entries\n").expect("catalog");
    std::fs::write(assets.join("tiles.tileset.bsn"), TILESET).expect("tileset");
    std::fs::write(
        assets.join("zone.tilemap-0.jdtiles"),
        jackdaw_tilemap::encode(map),
    )
    .expect("sidecar");
    tmp
}

/// Gives the tileset's tiles their shapes, as painting them in the editor
/// does.
fn give_shapes(app: &mut App) {
    let shapes = [
        (SOLID, CollisionPreset::Solid, 0, false, false),
        (HALF, CollisionPreset::BottomHalf, 0, false, false),
        (SLOPE, CollisionPreset::SlopeUp, 0, false, false),
        (TRIGGER, CollisionPreset::Solid, 1, true, false),
        (ONE_WAY, CollisionPreset::Solid, 0, false, true),
    ];
    let mut assets = app.world_mut().resource_mut::<Assets<Tileset>>();
    let (_, tileset) = assets.iter_mut().next().expect("the tileset loaded");
    for (tile, preset, layer, trigger, one_way) in shapes {
        let shape = TilesetCollision {
            layer,
            trigger,
            one_way,
            preset,
            polygons: preset.polygons().expect("a preset shape"),
        };
        tileset
            .set_collision(0, u32::from(tile), shape)
            .expect("a tile of the sheet");
    }
}

/// A headless game with avian's 2D physics, stepping 1/60 s per update,
/// with the painted map loaded and its tiles given their shapes.
fn game(assets: &Path) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(bevy::asset::AssetPlugin {
        file_path: assets.to_string_lossy().into_owned(),
        ..default()
    });
    app.init_asset::<Mesh>();
    app.insert_resource(JackdawCatalogPath(assets.join("catalog.bsn")));
    app.add_plugins((JackdawPlugin, AvianColliderBridgePlugin));
    app.add_plugins(PhysicsPlugins::default());
    app.insert_resource(Gravity(Vec2::NEG_Y * 20.0 * TILE));
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(
        1.0 / 60.0,
    )));
    app.finish();
    app.cleanup();

    let handle = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(SCENE.to_string(), PathBuf::new()));
    app.world_mut().spawn(JackdawSceneRoot(handle));
    for _ in 0..200 {
        app.update();
        if !app.world().resource::<Assets<Tileset>>().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    give_shapes(&mut app);
    for _ in 0..4 {
        app.update();
    }
    app
}

fn bodies(app: &mut App) -> Vec<(Entity, TileChunkBody, Transform)> {
    let world = app.world_mut();
    let mut found: Vec<_> = (world.query::<(Entity, &TileChunkBody, &RigidBody, &Transform)>())
        .iter(world)
        .map(|(entity, body, rigid, transform)| {
            assert_eq!(*rigid, RigidBody::Static);
            (entity, *body, *transform)
        })
        .collect();
    found.sort_by_key(|(_, body, _)| body.chunk.0.x);
    found
}

/// The colliders under `body`: how many shapes each holds, its memberships,
/// whether it is a sensor and whether it is one-way.
fn colliders_of(app: &mut App, body: Entity) -> Vec<(usize, u32, bool, bool)> {
    let world = app.world_mut();
    let mut found: Vec<_> = (world.query::<(
        &Collider,
        &CollisionLayers,
        Has<Sensor>,
        Has<OneWayTile>,
        &ChildOf,
    )>())
    .iter(world)
    .filter(|(.., parent)| parent.parent() == body)
    .map(|(collider, layers, sensor, one_way, _)| {
        let shapes = collider
            .shape()
            .as_compound()
            .expect("a compound collider")
            .shapes()
            .len();
        (shapes, layers.memberships.0, sensor, one_way)
    })
    .collect();
    found.sort_by_key(|&(_, member, sensor, one_way)| (member, sensor, one_way));
    found
}

/// The top of the solid ground under `x`, or `None` over a gap.
fn ground_at(app: &mut App, x: f32) -> Option<f32> {
    let from = Vec2::new(x, 10.0 * TILE);
    let world = app.world_mut();
    (world.query_filtered::<(&Collider, &GlobalTransform), (With<ChildOf>, Without<Sensor>)>())
        .iter(world)
        .filter_map(|(collider, at)| {
            collider.cast_ray(
                at.translation().truncate(),
                Rot2::IDENTITY,
                from,
                Vec2::NEG_Y,
                100.0 * TILE,
                true,
            )
        })
        .map(|(distance, _)| from.y - distance)
        .reduce(f32::max)
}

fn drop_box(app: &mut App, x: f32) -> Entity {
    app.world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::rectangle(TILE / 2.0, TILE / 2.0),
            LockedAxes::ROTATION_LOCKED,
            CollidingEntities::default(),
            Transform::from_xyz(x, 3.0 * TILE, 0.0),
        ))
        .id()
}

#[test]
fn a_painted_map_builds_a_static_body_per_chunk_with_sensors_for_triggers() {
    let project = project(&painted());
    let mut app = game(&project.path().join("assets"));

    let found = bodies(&mut app);
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].2.translation, Vec3::ZERO);
    assert_eq!(found[1].2.translation, Vec3::new(32.0 * TILE, 0.0, 0.0));
    assert_eq!(
        colliders_of(&mut app, found[0].0),
        vec![
            (4, 1, false, false),
            (1, 1, false, true),
            (1, 1 << 1, true, false)
        ],
        "a merged run, a half tile and two slopes, a one-way ledge, and a trigger on layer 1"
    );
    assert_eq!(
        colliders_of(&mut app, found[1].0),
        vec![(1, 1, false, false)]
    );
}

#[test]
fn the_ground_is_where_the_tiles_draw_it() {
    let project = project(&painted());
    let mut app = game(&project.path().join("assets"));

    let samples = [
        (0.5, Some(1.0)),
        (3.9, Some(1.0)),
        (4.5, Some(0.5)),
        (5.25, Some(0.25)),
        (6.25, Some(0.75)),
        (7.5, None),
        (8.5, None),
        (12.5, Some(4.0)),
        (40.5, Some(1.0)),
    ];
    for (x, top) in samples {
        let hit = ground_at(&mut app, x * TILE);
        let expected = top.map(|top| top * TILE);
        assert!(
            match (hit, expected) {
                (Some(hit), Some(expected)) => (hit - expected).abs() < 1e-3,
                (hit, expected) => hit == expected,
            },
            "the ground at x = {x} tiles is {hit:?}, not {expected:?}"
        );
    }
}

#[test]
fn a_body_lands_on_solid_ground_and_falls_through_a_trigger() {
    let project = project(&painted());
    let mut app = game(&project.path().join("assets"));
    let trigger = {
        let world = app.world_mut();
        (world.query_filtered::<Entity, With<Sensor>>())
            .iter(world)
            .next()
            .expect("the trigger's sensor")
    };
    let lander = drop_box(&mut app, 1.5 * TILE);
    let faller = drop_box(&mut app, 8.5 * TILE);

    let mut overlapped = false;
    for _ in 0..120 {
        app.update();
        overlapped |= (app.world().get::<CollidingEntities>(faller))
            .is_some_and(|colliding| colliding.contains(&trigger));
    }

    let y = |app: &App, body| {
        app.world()
            .get::<Transform>(body)
            .expect("the body")
            .translation
            .y
    };
    let resting = y(&app, lander);
    assert!(
        (resting - 1.25 * TILE).abs() < 0.5,
        "the box rests on the ground at {}, not at {resting}",
        1.25 * TILE
    );
    assert!(y(&app, faller) < 0.0, "the trigger did not hold the box up");
    assert!(overlapped, "the trigger reported the box passing through");
}

/// Pushes a box resting on the ground at `from`, in tiles, rightward at
/// four tiles a second for `seconds`, and returns where it ends up, in
/// tiles.
fn slide(app: &mut App, from: Vec2, seconds: f32) -> Vec2 {
    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::rectangle(TILE / 2.0, TILE / 2.0),
            LockedAxes::ROTATION_LOCKED,
            Friction::ZERO.with_combine_rule(CoefficientCombine::Min),
            Transform::from_xyz(from.x * TILE, from.y * TILE + TILE / 4.0 + 0.5, 0.0),
        ))
        .id();
    for _ in 0..10 {
        app.update();
    }
    for _ in 0..(seconds * 60.0) as usize {
        if let Some(mut velocity) = app.world_mut().get_mut::<LinearVelocity>(body) {
            velocity.x = 4.0 * TILE;
        }
        app.update();
    }
    let at = app
        .world()
        .get::<Transform>(body)
        .expect("the box")
        .translation;
    at.truncate() / TILE
}

#[test]
#[ignore = "a sliding box stops at the edge where two chunk bodies meet"]
fn a_box_slides_across_a_chunk_border_without_snagging() {
    let project = project(&seams());
    let mut app = game(&project.path().join("assets"));

    let end = slide(&mut app, Vec2::new(26.5, 1.0), 2.0);
    assert!(end.x > 33.0, "the box stopped at x = {} tiles", end.x);
    assert!(
        (end.y - 1.25).abs() < 0.05,
        "the box left the floor, at y = {} tiles",
        end.y
    );
}

#[test]
#[ignore = "a sliding box stops at the foot of a slope tile on flat ground"]
fn a_box_climbs_a_slope_onto_solid_ground_without_snagging() {
    let project = project(&seams());
    let mut app = game(&project.path().join("assets"));

    let end = slide(&mut app, Vec2::new(51.5, 1.0), 1.5);
    assert!(end.x > 56.0, "the box stopped at x = {} tiles", end.x);
    assert!(
        (end.y - 2.25).abs() < 0.05,
        "the box is not on the step, at y = {} tiles",
        end.y
    );
}

fn group(layer: u32, one_way: bool, boxes: &[[Vec2; 2]]) -> CollisionGroup {
    CollisionGroup {
        layer,
        trigger: false,
        one_way,
        boxes: boxes.to_vec(),
        polygons: Vec::new(),
    }
}

/// Two chunks of ground, the left one with a one-way ledge.
fn built() -> TilemapCollision {
    let ground = TilemapChunkCollision {
        origin: Vec2::ZERO,
        tile_size: Vec2::splat(TILE),
        groups: vec![
            group(0, false, &[[Vec2::ZERO, Vec2::new(4.0, 1.0)]]),
            group(0, true, &[[Vec2::new(0.0, 3.0), Vec2::new(2.0, 4.0)]]),
        ],
        revision: 1,
    };
    let right = TilemapChunkCollision {
        origin: Vec2::new(32.0 * TILE, 0.0),
        groups: vec![group(0, false, &[[Vec2::ZERO, Vec2::ONE]])],
        revision: 2,
        ..ground.clone()
    };
    let mut collision = TilemapCollision::default();
    collision.chunks.insert((0, ChunkCoord::new(0, 0)), ground);
    collision.chunks.insert((0, ChunkCoord::new(1, 0)), right);
    collision
}

fn bare_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.init_resource::<AppTypeRegistry>();
    app.add_plugins(AvianColliderBridgePlugin);
    app
}

#[test]
fn an_edit_rebuilds_only_the_chunk_it_touched() {
    let mut app = bare_app();
    let map = app.world_mut().spawn((Tilemap::default(), built())).id();
    app.update();
    let before = bodies(&mut app);

    {
        let mut collision = app
            .world_mut()
            .get_mut::<TilemapCollision>(map)
            .expect("the map's collision");
        let ground = collision
            .chunks
            .get_mut(&(0, ChunkCoord::new(0, 0)))
            .expect("the ground chunk");
        ground.groups.truncate(1);
        ground.revision = 3;
    }
    app.update();

    let after = bodies(&mut app);
    assert_eq!(after.len(), 2);
    assert_ne!(after[0].0, before[0].0, "the edited chunk is built again");
    assert_eq!(after[1].0, before[1].0, "the other chunk is kept");
    assert_eq!(
        colliders_of(&mut app, after[0].0),
        vec![(1, 1, false, false)]
    );

    app.world_mut()
        .get_mut::<TilemapCollision>(map)
        .expect("the map's collision")
        .chunks
        .remove(&(0, ChunkCoord::new(1, 0)));
    app.update();
    assert_eq!(bodies(&mut app).len(), 1, "an emptied chunk loses its body");
}

#[test]
fn a_map_that_stops_colliding_loses_its_bodies() {
    let mut app = bare_app();
    let map = app.world_mut().spawn((Tilemap::default(), built())).id();
    app.update();
    assert_eq!(bodies(&mut app).len(), 2);

    app.world_mut().entity_mut(map).remove::<TilemapCollision>();
    app.update();
    assert!(bodies(&mut app).is_empty(), "removing the collision");

    app.world_mut().entity_mut(map).insert(built());
    app.update();
    assert_eq!(bodies(&mut app).len(), 2);
    app.world_mut().entity_mut(map).remove::<Tilemap>();
    app.update();
    assert!(bodies(&mut app).is_empty(), "removing the tilemap");
    assert!(app.world().get::<TilemapCollision>(map).is_none());
}
