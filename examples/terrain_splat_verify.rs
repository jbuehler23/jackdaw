//! Manual visual verification for the terrain splat material
//! (`jackdaw_terrain::render`), over geometry from the clipmap mesher.
//!
//! Not part of the test suite: needs a real GPU and a display, and opens an
//! actual window (winit requires its event loop on the process's main thread,
//! which `cargo test`'s worker threads are not). Run by hand:
//!
//! ```text
//! cargo run --example terrain_splat_verify
//! ```
//!
//! Writes its PNGs into a temp directory at startup and points the asset server
//! at them, so nothing binary lives in the repo. The texture set is built the
//! way the editor builds one: three slots, each naming the albedo and height a
//! material would supply.
//!
//! The terrain is 65x65 grid points over 32x32 world units, meshed by
//! `jackdaw_terrain::clipmap`. A terrain this small resolves to a single level
//! at full detail, so no geometry stands between the control map and what is on
//! screen; level boundaries are `examples/terrain_clipmap_verify.rs`'s subject.
//! The control map is painted in four bands:
//!
//! - **ramp** (far edge): grass to rock across grid lines 20..45. Rock's height
//!   map is coarse blobs and grass's is a fine weave, so a working height blend
//!   makes rock erupt through in patches rather than fading in evenly.
//! - **plateau**: a rectangle painted at one uniform mid blend. Its interior
//!   takes the shader's uniform-word fast path and the cells at its edge take
//!   the bilinear path, so any difference between the two draws a one-cell band
//!   around it. There must be no band.
//! - **twins**: two rectangles that must render identically. The left one names
//!   rock as both its base and its overlay at full blend, the right one names
//!   rock as a plain base.
//! - **sand** (near edge): a third texture, so the shot shows three distinct
//!   textures from three distinct control values.
//!
//! A lit box hangs over the grass, so the shots show terrain receiving a
//! shadow, which it does not do unless the fragment shader passes the mesh
//! flags through.
//!
//! Five cameras, written to `/tmp/terrain_splat_{far,close,plateau,
//! twins,shadow}.png`.
//!
//! Nothing here asserts on pixels; the run produces evidence for a human to
//! look at.

use std::path::{Path, PathBuf};

use bevy::app::AppExit;
use bevy::asset::AssetPlugin;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use jackdaw_terrain::render::{
    SplatArrayHandles, SplatBuildError, TerrainRenderPlugin, TerrainSplatMaterial,
    TextureSetImages, control_image, slope_image, splat_images,
};
use jackdaw_terrain::sidecar::AutoTerrainSettings;
use jackdaw_terrain::splat::ControlTexels;
use jackdaw_terrain::texture_set::{TextureSet, TextureSetEntry};
use jackdaw_terrain::{Control, Heightmap, SurfaceMeshData};

const RESOLUTION: u32 = 65;
const WORLD_SIZE: f32 = 32.0;
const TEXTURE_SIDE: u32 = 64;

/// Texture ids, matching the entry order in the set below.
const GRASS: u8 = 0;
const ROCK: u8 = 1;
const SAND: u8 = 2;

// Band boundaries in grid lines.
const RAMP_TO: u32 = 20;
const PLATEAU_TO: u32 = 38;
const TWINS_TO: u32 = 52;
const RAMP_START: u32 = 20;
const RAMP_END: u32 = 45;
const PLATEAU_X: std::ops::Range<u32> = 18..47;
const TWIN_LEFT_X: std::ops::Range<u32> = 10..30;
const TWIN_RIGHT_X: std::ops::Range<u32> = 35..55;

/// World position of a grid point, matching the mesher's own mapping.
fn world_of(g: u32) -> f32 {
    g as f32 * (WORLD_SIZE / (RESOLUTION - 1) as f32) - WORLD_SIZE / 2.0
}

fn main() {
    // Panics rather than logging: bevy's logger is not up yet, and a run that
    // cannot write its test assets has nothing to verify.
    let assets = write_test_assets().expect("terrain_splat_verify: cannot write test assets");
    App::new()
        .add_plugins(DefaultPlugins.set(AssetPlugin {
            file_path: assets.to_string_lossy().into_owned(),
            ..default()
        }))
        .add_plugins(TerrainRenderPlugin)
        .add_systems(Startup, setup)
        .add_systems(Update, (attach_material, drive).chain())
        .run();
}

/// The three slots this scene paints with, as the editor would resolve them
/// from three saved materials.
///
/// Grass tiles at 0.3 repeats per world unit, not a whole number of control
/// cells (a cell is 0.5 world units): a commensurate scale would line the grass
/// weave up with the control grid, leaving the close shot unable to separate
/// the texture's own pattern from the control cells.
fn verify_set() -> TextureSet {
    let slot = |material: &str, albedo: &str, height: &str, uv_scale: f32| TextureSetEntry {
        height: Some(height.to_string()),
        uv_scale,
        ..TextureSetEntry::new(material, albedo)
    };
    TextureSet {
        entries: vec![
            slot("grass", "grass.png", "grass_h.png", 0.3),
            slot("rock", "rock.png", "rock_h.png", 0.25),
            slot("sand", "sand.png", "sand_h.png", 0.5),
        ],
    }
}

/// Write three albedo/height texture pairs into a fresh temp directory.
///
/// The albedos are flat colours, so any structure on screen came from the
/// blend; the height maps carry all the structure.
fn write_test_assets() -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join("jackdaw_terrain_splat_verify");
    std::fs::create_dir_all(&dir)?;

    write_png(&dir.join("grass.png"), |_, _| [70, 120, 55, 255])?;
    write_png(&dir.join("rock.png"), |_, _| [150, 105, 85, 255])?;
    write_png(&dir.join("sand.png"), |_, _| [205, 190, 130, 255])?;

    // Grass is almost flat: a slight weave, low enough to lose every blend.
    write_png(&dir.join("grass_h.png"), |x, y| {
        let v = 96 + (((x / 4 + y / 4) % 2) as u8) * 12;
        [v, v, v, 255]
    })?;
    // Rock is coarse blobs, high in the middle of each one. Against grass's
    // flat height, a height blend makes these erupt through the transition
    // band first.
    write_png(&dir.join("rock_h.png"), |x, y| {
        let cx = (x % 32) as f32 - 16.0;
        let cy = (y % 32) as f32 - 16.0;
        let d = (cx * cx + cy * cy).sqrt() / 16.0;
        let v = (255.0 * (1.0 - d).clamp(0.0, 1.0)).round() as u8;
        [v, v, v, 255]
    })?;
    // Sand is fine ripples, so the third texture reads as its own material
    // rather than as flat colour.
    write_png(&dir.join("sand_h.png"), |x, _| {
        let v = 120 + ((x % 8) as u8) * 12;
        [v, v, v, 255]
    })?;

    Ok(dir)
}

fn write_png(path: &Path, texel: impl Fn(u32, u32) -> [u8; 4]) -> std::io::Result<()> {
    let mut buffer = ::image::RgbaImage::new(TEXTURE_SIDE, TEXTURE_SIDE);
    for (x, y, pixel) in buffer.enumerate_pixels_mut() {
        *pixel = ::image::Rgba(texel(x, y));
    }
    buffer
        .save_with_format(path, ::image::ImageFormat::Png)
        .map_err(std::io::Error::other)
}

/// The clipmap levels of one terrain, as meshes, for a viewer at `viewer`
/// in that terrain's grid coordinates.
fn clipmap_meshes(map: &Heightmap, viewer: Vec2) -> Vec<SurfaceMeshData> {
    jackdaw_terrain::clipmap_levels(map.resolution, viewer)
        .iter()
        .map(|level| jackdaw_terrain::build_clipmap_mesh_data(map, level, |_, _| true))
        .collect()
}

fn mesh_of(data: SurfaceMeshData) -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, data.positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, data.normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, data.uvs);
    mesh.insert_indices(Indices::U32(data.indices));
    mesh
}

/// The painted control map: four bands, described on the module above.
fn painted_control() -> Vec<Control> {
    let base = |id: u8| Control::default().with_base_id(id);
    let mut control = vec![Control::default(); (RESOLUTION * RESOLUTION) as usize];
    for gz in 0..RESOLUTION {
        for gx in 0..RESOLUTION {
            let word = if gz < RAMP_TO {
                if gx < RAMP_START {
                    base(GRASS)
                } else if gx < RAMP_END {
                    let t = (gx - RAMP_START) as f32 / (RAMP_END - RAMP_START - 1) as f32;
                    base(GRASS)
                        .with_overlay_id(ROCK)
                        .with_blend((t * 255.0).round() as u8)
                } else {
                    base(ROCK)
                }
            } else if gz < PLATEAU_TO {
                // One uniform blend over a rectangle: the interior takes
                // the fast path, the boundary cells take the bilinear one.
                if PLATEAU_X.contains(&gx) {
                    base(GRASS).with_overlay_id(ROCK).with_blend(128)
                } else {
                    base(GRASS)
                }
            } else if gz < TWINS_TO {
                if TWIN_LEFT_X.contains(&gx) {
                    // The degenerate word: one id as both layers at full blend,
                    // which must draw as plain rock.
                    base(ROCK).with_overlay_id(ROCK).with_blend(255)
                } else if TWIN_RIGHT_X.contains(&gx) {
                    base(ROCK)
                } else {
                    base(GRASS)
                }
            } else {
                base(SAND)
            };
            control[(gz * RESOLUTION + gx) as usize] = word;
        }
    }
    control
}

#[derive(Resource)]
struct Verify {
    frame: u32,
    camera: Entity,
    set: TextureSet,
    images: TextureSetImages,
    surfaces: Vec<Entity>,
    attached: bool,
    /// Frames spent waiting for textures. Kept apart from `frame`, which times
    /// the shots and must not start counting until everything is on screen.
    waited: u32,
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut standard: ResMut<Assets<StandardMaterial>>,
    assets: Res<AssetServer>,
) {
    // Flat ground, so there is no shape for a blend artefact to hide behind.
    let heightmap = Heightmap::new(RESOLUTION, Vec2::splat(WORLD_SIZE), 10.0);
    let surfaces: Vec<Entity> = clipmap_meshes(&heightmap, Vec2::splat(RESOLUTION as f32 / 2.0))
        .into_iter()
        .map(|data| {
            commands
                .spawn((
                    Mesh3d(meshes.add(mesh_of(data))),
                    Transform::default(),
                    Visibility::default(),
                ))
                .id()
        })
        .collect();

    // Something to cast a shadow onto the terrain. Terrain only receives
    // one if the fragment shader forwards the mesh flags.
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(2.0, 2.0, 2.0))),
        MeshMaterial3d(standard.add(StandardMaterial::from(Color::srgb(0.8, 0.8, 0.85)))),
        Transform::from_xyz(-11.0, 3.0, -11.0),
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 12_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(6.0, 12.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    let camera = commands
        .spawn((
            Camera3d::default(),
            Transform::from_translation(Vec3::new(0.0, 30.0, 30.0)).looking_at(Vec3::ZERO, Vec3::Y),
        ))
        .id();

    // Albedo is colour and decodes as sRGB; height is data and must not, the
    // way the editor loads a material's depth map.
    let set = verify_set();
    let mut images = TextureSetImages::default();
    for entry in &set.entries {
        images.albedo.push(
            entry
                .albedo
                .as_ref()
                .map(|p| assets.load::<Image>(p.clone())),
        );
        images.normal.push(None);
        images.height.push(entry.height.as_ref().map(|path| {
            assets
                .load_builder()
                .with_settings(|settings: &mut bevy::image::ImageLoaderSettings| {
                    settings.is_srgb = false;
                })
                .load(path.clone())
        }));
    }

    commands.insert_resource(Verify {
        frame: 0,
        camera,
        set,
        images,
        surfaces,
        attached: false,
        waited: 0,
    });
}

/// Build the arrays and the material once every texture has decoded, then
/// put it on every level.
fn attach_material(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut verify: ResMut<Verify>,
    mut materials: ResMut<Assets<TerrainSplatMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    if verify.attached {
        return;
    }
    let set = verify.set.clone();
    let handles = verify.images.clone();
    let built = match splat_images(&set, &handles, &images) {
        Ok(built) => built,
        Err(SplatBuildError::NotReady) => {
            // Name the texture holding things up rather than spinning
            // silently: a colour-space slip on a height map stalls here.
            verify.waited += 1;
            if verify.waited == 300 {
                for handle in handles.handles() {
                    error!(
                        "terrain_splat_verify: {:?} is {:?}",
                        assets.get_path(handle.id()),
                        assets.get_load_state(handle.id())
                    );
                }
            }
            return;
        }
        Err(other) => {
            error!("terrain_splat_verify: unusable texture set: {other}");
            verify.attached = true;
            return;
        }
    };

    let texels = ControlTexels::from_control(&painted_control(), RESOLUTION);
    let control = images.add(control_image(&texels));
    let slope = images.add(slope_image(&Heightmap::new(
        RESOLUTION,
        Vec2::splat(WORLD_SIZE),
        10.0,
    )));
    let arrays = SplatArrayHandles {
        albedo: images.add(built.albedo),
        normal: images.add(built.normal),
        height: images.add(built.height),
    };
    let material = materials.add(TerrainSplatMaterial::new(
        &set,
        arrays,
        control,
        slope,
        Vec2::splat(WORLD_SIZE),
        RESOLUTION,
        AutoTerrainSettings::default(),
    ));
    for entity in &verify.surfaces {
        commands
            .entity(*entity)
            .insert(MeshMaterial3d(material.clone()));
    }
    verify.attached = true;
    info!("terrain_splat_verify: splat material attached");
}

/// Where each shot stands and what it looks at.
fn shots() -> Vec<(&'static str, Vec3, Vec3)> {
    // Band centres in world Z, derived from the grid lines the control map
    // uses rather than restated, so moving a band moves its camera.
    let ramp_z = world_of(RAMP_TO / 2);
    let plateau_z = world_of((RAMP_TO + PLATEAU_TO) / 2);
    let twins_z = world_of((PLATEAU_TO + TWINS_TO) / 2);
    // The plateau's left edge, where the fast path meets the bilinear one.
    let plateau_edge_x = world_of(PLATEAU_X.start);

    vec![
        (
            "/tmp/terrain_splat_far.png",
            Vec3::new(0.0, 30.0, 30.0),
            Vec3::ZERO,
        ),
        (
            "/tmp/terrain_splat_close.png",
            Vec3::new(0.0, 3.0, ramp_z + 5.0),
            Vec3::new(0.0, 0.0, ramp_z),
        ),
        (
            "/tmp/terrain_splat_plateau.png",
            Vec3::new(plateau_edge_x, 6.0, plateau_z + 3.0),
            Vec3::new(plateau_edge_x, 0.0, plateau_z),
        ),
        (
            "/tmp/terrain_splat_twins.png",
            Vec3::new(0.0, 11.0, twins_z + 3.0),
            Vec3::new(0.0, 0.0, twins_z),
        ),
        (
            "/tmp/terrain_splat_shadow.png",
            Vec3::new(-4.0, 9.0, -4.0),
            Vec3::new(-12.0, 0.0, -12.0),
        ),
    ]
}

fn drive(
    mut verify: ResMut<Verify>,
    mut transforms: Query<&mut Transform>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    if !verify.attached {
        return;
    }
    verify.frame += 1;
    // Move, settle, shoot, repeat. The first settle matches the editor's
    // 90-frame screenshot budget, so a slow first-frame shader compile cannot
    // land inside a capture; later moves settle for 30 frames.
    let shots = shots();
    const SETTLE: u32 = 90;
    const GAP: u32 = 30;
    for (index, (path, from, at)) in shots.iter().enumerate() {
        let move_at = SETTLE + index as u32 * GAP * 2;
        if verify.frame == move_at
            && let Ok(mut tf) = transforms.get_mut(verify.camera)
        {
            *tf = Transform::from_translation(*from).looking_at(*at, Vec3::Y);
        }
        if verify.frame == move_at + GAP {
            shoot(&mut commands, path);
        }
    }
    if verify.frame > SETTLE + shots.len() as u32 * GAP * 2 {
        exit.write(AppExit::Success);
    }
}

fn shoot(commands: &mut Commands, path: &'static str) {
    commands
        .spawn(Screenshot::primary_window())
        .observe(move |capture: On<ScreenshotCaptured>| {
            write_capture(&capture.image, Path::new(path));
        });
}

fn write_capture(image: &Image, path: &Path) {
    match image.clone().try_into_dynamic() {
        Ok(dynamic) => match dynamic
            .to_rgb8()
            .save_with_format(path, ::image::ImageFormat::Png)
        {
            Ok(()) => info!("terrain_splat_verify: wrote {}", path.display()),
            Err(err) => error!(
                "terrain_splat_verify: cannot write {}: {err}",
                path.display()
            ),
        },
        Err(err) => error!("terrain_splat_verify: cannot convert captured frame: {err}"),
    }
}
