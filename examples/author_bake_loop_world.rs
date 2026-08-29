//! Headless world-authoring driver.
//!
//! Authors a small test world using jackdaw's own kernels (terrain
//! sculpt/quantize/paint from `jackdaw_terrain`, the BSN writer from
//! `jackdaw_bsn`) and writes a loadable `scene.bsn` plus its terrain
//! sidecar into `<project-dir>/assets/`. This is the input to a terrain
//! export/import pipeline built in a parallel session.
//!
//! Run: `cargo run --example author_bake_loop_world -- <project-dir>`

#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "the example reports authored output and verification results"
)]

use std::collections::HashSet;
use std::path::PathBuf;

use bevy::app::App;
use bevy::ecs::hierarchy::{ChildOf, Children};
use bevy::prelude::*;

use jackdaw_bsn::BsnWriterConfig;
use jackdaw_scene_types::{
    GltfSource, Terrain, TerrainChannel, TerrainChannelElement, TerrainPaletteEntry,
    TerrainQuantization,
};
use jackdaw_terrain::{
    ChannelData, ChannelElement, Heightmap, SculptTool, TerrainData, apply_brush,
    apply_channel_brush, quantize_heights, sidecar,
};

const RESOLUTION: u32 = 129;
const SIZE: f32 = 128.0;
const MAX_HEIGHT: f32 = 50.0;
const HILL_CENTER: Vec2 = Vec2::new(64.0, 64.0);
const HILL_RADIUS: f32 = 28.0;
const FOREST_CENTER: Vec2 = Vec2::new(40.0, 88.0);
const FOREST_RADIUS: f32 = 22.0;

/// Palette values for the `biome` channel.
///
/// Deliberately NOT the consuming importer's own biome cell ids (meadow is 1
/// and forest is 2 over there). The importer resolves palette LABELS, so an
/// implementation that wrongly read raw pixel values would emit unknown ids
/// 7 and 3 and be refused loudly, instead of coincidentally producing the
/// right answer and hiding the bug.
const MEADOW_VALUE: u16 = 7;
const FOREST_VALUE: u16 = 3;

fn main() {
    let project_dir: PathBuf = std::env::args()
        .nth(1)
        .expect("usage: author_bake_loop_world <project-dir>")
        .into();
    let assets_dir = project_dir.join("assets");
    std::fs::create_dir_all(&assets_dir).expect("create assets dir");

    // ---- terrain heights: sculpt a hill, then quantize ----
    let heights = sculpt_heights();
    let min_h = heights.iter().copied().fold(f32::INFINITY, f32::min);
    let max_h = heights.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    println!("terrain height range: min={min_h:.3} max={max_h:.3}");

    // ---- biome channel: meadow everywhere, forest patch off-centre ----
    let biome_values = build_biome_channel();
    let meadow_count = biome_values.iter().filter(|&&v| v == MEADOW_VALUE).count();
    let forest_count = biome_values.iter().filter(|&&v| v == FOREST_VALUE).count();
    println!("biome cells: meadow={meadow_count} forest={forest_count}");

    let terrain_data = TerrainData {
        resolution: RESOLUTION,
        heights: heights.clone(),
        channels: vec![ChannelData {
            name: "biome".to_string(),
            element: ChannelElement::U16,
            values: biome_values.clone(),
        }],
    };
    let sidecar_bytes = sidecar::encode(&terrain_data).expect("terrain data fits in memory");
    let sidecar_path = assets_dir.join("scene.terrain-0.jdterrain");
    std::fs::write(&sidecar_path, &sidecar_bytes).expect("write terrain sidecar");

    // ---- scene: build entities in a headless World, emit via jackdaw's own BSN writer ----
    let (scene_text, expected_transforms) = build_scene(&heights);
    let scene_path = assets_dir.join("scene.bsn");
    std::fs::write(&scene_path, &scene_text).expect("write scene.bsn");

    println!(
        "wrote {} ({} bytes)",
        scene_path.display(),
        scene_text.len()
    );
    println!(
        "wrote {} ({} bytes)",
        sidecar_path.display(),
        sidecar_bytes.len()
    );

    // ================= self-verification =================
    let mut all_ok = true;

    let mut verify_app = App::new();
    register_types(&mut verify_app);
    let reloaded_text = std::fs::read_to_string(&scene_path).expect("read scene.bsn back");
    let load_result = jackdaw_bsn::load_bsn_scene(verify_app.world_mut(), &reloaded_text);
    all_ok &= check("scene.bsn parses via load_bsn_scene", load_result.is_ok());
    if let Err(err) = &load_result {
        println!("  load error: {err}");
    }

    if load_result.is_ok() {
        let world = verify_app.world_mut();

        let mut terrain_query = world.query::<&Terrain>();
        let terrain_count = terrain_query.iter(world).count();
        all_ok &= check(
            "exactly one entity has a Terrain component",
            terrain_count == 1,
        );

        let mut terrain_query = world.query::<&Terrain>();
        if let Some(t) = terrain_query.iter(world).next() {
            all_ok &= check("terrain.resolution == 129", t.resolution == RESOLUTION);
            all_ok &= check(
                "terrain.size == (128.0, 128.0)",
                (t.size - Vec2::new(SIZE, SIZE)).length() < 1e-6,
            );
            all_ok &= check(
                "terrain.max_height == 50.0",
                (t.max_height - MAX_HEIGHT).abs() < 1e-6,
            );
            all_ok &= check(
                "terrain.data_path == scene.terrain-0.jdterrain",
                t.data_path == "scene.terrain-0.jdterrain",
            );
            all_ok &= check(
                "terrain has exactly one channel named biome",
                t.channels.len() == 1 && t.channels[0].name == "biome",
            );
            if let Some(ch) = t.channels.first() {
                all_ok &= check(
                    "biome channel element is U16",
                    ch.element == TerrainChannelElement::U16,
                );
                all_ok &= check(
                    "biome palette has exactly two entries",
                    ch.palette.len() == 2,
                );
                let meadow = ch.palette.iter().find(|p| p.value == MEADOW_VALUE);
                let forest = ch.palette.iter().find(|p| p.value == FOREST_VALUE);
                all_ok &= check(
                    "palette value 7 is labeled meadow",
                    meadow.is_some_and(|p| p.label == "meadow"),
                );
                all_ok &= check(
                    "palette value 3 is labeled forest",
                    forest.is_some_and(|p| p.label == "forest"),
                );
            }
            all_ok &= check(
                "terrain.quantization.enabled == true",
                t.quantization.enabled,
            );
            all_ok &= check(
                "terrain.quantization.cell_size == 1.0",
                (t.quantization.cell_size - 1.0).abs() < 1e-6,
            );
            all_ok &= check(
                "terrain.quantization.height_step == 0.25",
                (t.quantization.height_step - 0.25).abs() < 1e-6,
            );
            all_ok &= check(
                "size.x / (resolution - 1) == cell_size exactly",
                t.size.x / ((t.resolution - 1) as f32) == t.quantization.cell_size,
            );
        }

        let mut transform_query = world.query::<&Transform>();
        let transform_count = transform_query.iter(world).count();
        all_ok &= check(
            &format!(
                "transform count == {expected_transforms} (camera, sun, terrain, 4 props, 1 spawn, root)"
            ),
            transform_count == expected_transforms,
        );
    } else {
        all_ok = false;
    }

    // sidecar round trip
    match std::fs::read(&sidecar_path)
        .map_err(|e| e.to_string())
        .and_then(|bytes| sidecar::decode(&bytes).map_err(|e| e.to_string()))
    {
        Ok(decoded) => {
            all_ok &= check("sidecar decode succeeds", true);
            all_ok &= check(
                "decoded heights are byte-identical to what was encoded",
                decoded.heights == terrain_data.heights,
            );
            let decoded_biome = decoded.channels.first().map(|c| c.values.clone());
            all_ok &= check(
                "decoded biome channel is byte-identical to what was encoded",
                decoded_biome.as_deref() == Some(terrain_data.channels[0].values.as_slice()),
            );
        }
        Err(err) => {
            all_ok = false;
            println!("[FAIL] sidecar decode: {err}");
        }
    }

    let all_quantized = heights.iter().all(|h| {
        let steps = h / 0.25;
        (steps - steps.round()).abs() < 1e-4
    });
    all_ok &= check(
        "every quantized height is an exact multiple of 0.25",
        all_quantized,
    );

    let unique_biome_values: HashSet<u16> = biome_values.iter().copied().collect();
    all_ok &= check(
        "biome channel contains both 1 and 2 and no other values",
        unique_biome_values.len() == 2
            && unique_biome_values.contains(&MEADOW_VALUE)
            && unique_biome_values.contains(&FOREST_VALUE),
    );

    println!("min height = {min_h:.3}, max height = {max_h:.3}");
    println!("meadow cells = {meadow_count}, forest cells = {forest_count}");

    if !all_ok {
        eprintln!("VERIFICATION FAILED");
        std::process::exit(1);
    }
    println!("ALL CHECKS PASSED");
}

fn check(label: &str, ok: bool) -> bool {
    println!("[{}] {}", if ok { "PASS" } else { "FAIL" }, label);
    ok
}

/// Sculpt a hill with `apply_brush`, then quantize to a 0.25 m step so the
/// export is lossless.
fn sculpt_heights() -> Vec<f32> {
    let mut hm = Heightmap::new(RESOLUTION, Vec2::new(SIZE, SIZE), MAX_HEIGHT);

    for _ in 0..3 {
        apply_brush(
            &mut hm,
            SculptTool::Raise,
            HILL_CENTER,
            HILL_RADIUS,
            5.0,
            1.5,
            1.0,
            None,
        );
    }
    for _ in 0..2 {
        apply_brush(
            &mut hm,
            SculptTool::Smooth,
            HILL_CENTER,
            HILL_RADIUS,
            1.0,
            1.0,
            1.0,
            None,
        );
    }

    let mut heights = hm.heights;
    quantize_heights(&mut heights, 0.25);
    heights
}

/// One `biome` channel: filled with [`MEADOW_VALUE`] everywhere, then a
/// forest patch of [`FOREST_VALUE`] painted off-centre. Filling first is
/// required: value `0` has no palette entry and the downstream importer
/// refuses unmapped values.
fn build_biome_channel() -> Vec<u16> {
    let mut values = vec![MEADOW_VALUE; (RESOLUTION as usize) * (RESOLUTION as usize)];
    apply_channel_brush(
        &mut values,
        RESOLUTION,
        ChannelElement::U16,
        FOREST_CENTER,
        FOREST_RADIUS,
        1.0,
        0.0,
        FOREST_VALUE,
    );
    values
}

/// Height at fractional grid coordinates, nearest-cell (no interpolation
/// needed here; this is only used to place props at a plausible elevation).
fn height_at(heights: &[f32], grid: Vec2) -> f32 {
    let gx = grid.x.round().clamp(0.0, (RESOLUTION - 1) as f32) as u32;
    let gz = grid.y.round().clamp(0.0, (RESOLUTION - 1) as f32) as u32;
    heights[(gz * RESOLUTION + gx) as usize]
}

/// Grid-space coordinates (as used by `apply_brush`/`apply_channel_brush`)
/// to terrain-local world XZ. The terrain is centred at its own origin, so
/// this is the inverse of `Heightmap::world_to_grid` with a 1.0 m cell.
fn grid_to_local(grid: Vec2) -> Vec2 {
    grid - Vec2::splat(SIZE / 2.0)
}

/// Registers every reflected type this scene's components need. Bevy's
/// `reflect_auto_register` feature (on for this workspace) already derives
/// most of these automatically, but registering explicitly here removes any
/// dependency on that being active for a bare `App::new()`.
fn register_types(app: &mut App) {
    app.register_type::<Name>()
        .register_type::<Transform>()
        .register_type::<ChildOf>()
        .register_type::<Children>()
        .register_type::<Visibility>()
        .register_type::<Camera3d>()
        .register_type::<DirectionalLight>()
        .register_type::<GltfSource>()
        .register_type::<Terrain>()
        .register_type::<TerrainChannel>()
        .register_type::<TerrainChannelElement>()
        .register_type::<TerrainPaletteEntry>()
        .register_type::<TerrainQuantization>();
}

/// Builds the scene in a headless `World` and emits it to `.bsn` text via
/// `jackdaw_bsn::serialize_to_bsn_with_config` -- the same writer the editor
/// itself uses -- rather than hand-writing the text. Returns the emitted
/// text and the number of entities that carry a `Transform` (for the
/// self-verification step).
fn build_scene(heights: &[f32]) -> (String, usize) {
    let mut app = App::new();
    register_types(&mut app);
    let world = app.world_mut();

    let root = world
        .spawn((Name::new("Root"), Transform::default(), Visibility::Visible))
        .id();

    world.spawn((
        Name::new("Main Camera"),
        Camera3d::default(),
        Transform::from_xyz(0.0, 60.0, 90.0).looking_at(Vec3::new(0.0, 8.0, 0.0), Vec3::Y),
        Visibility::Visible,
        ChildOf(root),
    ));

    world.spawn((
        Name::new("Sun"),
        DirectionalLight::default(),
        Transform::default().looking_to(Vec3::new(-0.4, -1.0, -0.3), Vec3::Y),
        Visibility::Visible,
        ChildOf(root),
    ));

    world.spawn((
        Name::new("Ground"),
        Transform::default(),
        Visibility::Visible,
        Terrain {
            resolution: RESOLUTION,
            size: Vec2::new(SIZE, SIZE),
            max_height: MAX_HEIGHT,
            channels: vec![TerrainChannel {
                name: "biome".to_string(),
                element: TerrainChannelElement::U16,
                palette: vec![
                    TerrainPaletteEntry {
                        value: MEADOW_VALUE,
                        label: "meadow".to_string(),
                        color: Color::srgb(0.36, 0.56, 0.28),
                    },
                    TerrainPaletteEntry {
                        value: FOREST_VALUE,
                        label: "forest".to_string(),
                        color: Color::srgb(0.14, 0.32, 0.16),
                    },
                ],
            }],
            data_path: "scene.terrain-0.jdterrain".to_string(),
            heights: Vec::new(),
            // resolution 129 => 128 cells; cell_size 1.0 * 128 cells == size
            // 128.0, so this agrees exactly with `size` above, and
            // height_step matches the 0.25 m quantize_heights step the
            // sidecar heights were snapped to.
            quantization: TerrainQuantization {
                enabled: true,
                cell_size: 1.0,
                height_step: 0.25,
            },
            ..default()
        },
        ChildOf(root),
    ));

    let props: [(&str, &str, Vec2); 4] = [
        (
            "CommonTree_1",
            "models/quaternius/nature/CommonTree_1.gltf",
            Vec2::new(78.0, 58.0),
        ),
        (
            "CommonTree_3",
            "models/quaternius/nature/CommonTree_3.gltf",
            Vec2::new(46.0, 74.0),
        ),
        (
            "Bush_Common",
            "models/quaternius/nature/Bush_Common.gltf",
            Vec2::new(40.0, 82.0),
        ),
        (
            "Fern_1",
            "models/quaternius/nature/Fern_1.gltf",
            Vec2::new(70.0, 78.0),
        ),
    ];
    for (name, path, grid_pos) in props {
        let local = grid_to_local(grid_pos);
        let y = height_at(heights, grid_pos);
        world.spawn((
            Name::new(name),
            Transform::from_xyz(local.x, y, local.y),
            GltfSource {
                path: path.to_string(),
                scene_index: 0,
            },
            ChildOf(root),
        ));
    }

    // Spawn-point convention: the entity NAME encodes
    // "spawn:<species>:<count>:<respawn_seconds>". No game-specific
    // SpawnPoint component type is registered in a headless jackdaw type
    // registry, so the consuming importer parses this string instead.
    let spawn_grid = Vec2::new(90.0, 64.0);
    let spawn_local = grid_to_local(spawn_grid);
    let spawn_y = height_at(heights, spawn_grid);
    world.spawn((
        Name::new("spawn:moor_deer:3:120"),
        Transform::from_xyz(spawn_local.x, spawn_y, spawn_local.y),
        ChildOf(root),
    ));

    let config = BsnWriterConfig::default()
        // The editor's default config skips the whole `bevy_camera::visibility::`
        // module (ViewVisibility/InheritedVisibility/etc, which are runtime-only),
        // but the authored `Visibility` enum itself belongs in the scene.
        .always_save("bevy_camera::visibility::Visibility")
        // Camera3d requires (Camera, Projection); Camera itself further requires
        // Frustum/CameraMainTextureUsages/VisibleEntities/Transform/Visibility/
        // RenderTarget. Frustum and VisibleEntities are already covered by the
        // default config's `bevy_camera::primitives::`/`visibility::` skips; these
        // two prefixes cover the rest of that require chain so a headless spawn
        // (with no render plugins narrowing what actually gets attached) still
        // emits a clean bare `Camera3d`, matching what the real editor emits.
        .skip_prefix("bevy_camera::camera::")
        .skip_prefix("bevy_camera::projection::")
        // DirectionalLight requires Cascades/CascadesFrusta/CascadeShadowConfig/
        // CascadesVisibleEntities/.../VisibilityClass. CascadesFrusta and
        // CascadesVisibleEntities/VisibilityClass are already covered by the
        // default config's `bevy_camera::` skips; this covers the cascade
        // shadow config types that live in `bevy_light::cascade::`.
        .skip_prefix("bevy_light::cascade::");

    let text = jackdaw_bsn::serialize_to_bsn_with_config(app.world(), &config);
    // root + camera + sun + terrain + 4 props + spawn point
    (text, 1 + 1 + 1 + 1 + 4 + 1)
}
