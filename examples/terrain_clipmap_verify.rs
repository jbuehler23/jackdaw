//! Manual visual verification for the clipmap mesher and for texture
//! detiling.
//!
//! Not part of the test suite, for the reasons
//! `examples/terrain_splat_verify.rs` gives: a real GPU, a display, and
//! winit's event loop on the process's main thread. Run by hand:
//!
//! ```text
//! cargo run --example terrain_clipmap_verify
//! ```
//!
//! Three subjects in one world, each on its own render layer so a shot frames
//! its subject alone:
//!
//! - **hills** at the origin: 257 grid points of rolling ground, drawn by
//!   `jackdaw_terrain::clipmap` the way the editor draws it, with levels around
//!   the camera and each level's outer ring snapped to the coarser grid. Shot
//!   from four heights, so the ring boundaries pass under the camera and any
//!   crack between two levels shows as a lit gap along a square.
//! - **detiling**: a flat plate under the splat material, painted with the same
//!   texture on both halves. The left half's slot is detiled 0 and the right
//!   half's 0.7. One shot holds both, so the grid of repeats on the left and
//!   its absence on the right are the same picture.
//! - **129**: 129 grid points per edge, which no power-of-two region holds on
//!   its own. The heights are scattered into a `RegionTerrainData` and gathered
//!   back out before anything meshes them, so the surface on screen is the one
//!   the document reconstructs across four 128-cell regions, seam row and seam
//!   column included.
//!
//! Writes its PNGs into a temp directory at startup and points the asset server
//! at them, so nothing binary lives in the repo.
//!
//! Nothing here asserts on pixels; the run produces evidence for a human to
//! look at.

#![expect(
    clippy::print_stderr,
    reason = "the example reports where it wrote its evidence"
)]

use std::path::{Path, PathBuf};

use bevy::app::AppExit;
use bevy::asset::AssetPlugin;
use bevy::camera::visibility::RenderLayers;
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
use jackdaw_terrain::{
    Control, Heightmap, RegionSize, RegionTerrainData, SurfaceMeshData, TerrainRegions,
};

/// Grid points per edge of the hills terrain. Large enough to need
/// several clipmap levels, so the shots have ring boundaries to show.
const HILLS_RESOLUTION: u32 = 257;
/// World units across the hills terrain: one unit per cell.
const HILLS_SIZE: f32 = 256.0;

/// Grid points per edge of the 129 subject: 2^7 + 1.
const ODD_RESOLUTION: u32 = 129;
const ODD_SIZE: f32 = 128.0;

/// Grid points per edge of the detiling plate, and its extent. Wide and
/// flat: repetition is what it is there to show.
const PLATE_RESOLUTION: u32 = 65;
const PLATE_SIZE: f32 = 128.0;

/// Texture ids on the plate: the same material twice, detiled and not.
const PLAIN: u8 = 0;
const DETILED: u8 = 1;

const TEXTURE_SIDE: u32 = 64;

/// Where each subject sits, so one camera can frame any of them.
const HILLS_AT: Vec3 = Vec3::ZERO;
const PLATE_AT: Vec3 = Vec3::new(0.0, 0.0, -400.0);
const ODD_AT: Vec3 = Vec3::new(400.0, 0.0, 0.0);

/// A layer per subject. Each shot puts the camera on exactly one of them, so a
/// grazing angle across the hills cannot pick up the plate or the 129 behind
/// them and leave a stray shape that reads as a crack.
const HILLS_LAYER: usize = 1;
const PLATE_LAYER: usize = 2;
const ODD_LAYER: usize = 3;

fn main() {
    let assets = match write_test_assets() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("terrain_clipmap_verify: cannot write test assets: {err}");
            return;
        }
    };
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

/// One material, twice: id 0 undetiled, id 1 detiled.
///
/// Same albedo, same tiling, so the only difference on screen between the
/// plate's two halves is the detile strength.
fn verify_set() -> TextureSet {
    let slot = |detile: f32| TextureSetEntry {
        height: Some("stone_h.png".to_string()),
        uv_scale: 0.25,
        detile,
        ..TextureSetEntry::new(
            if detile > 0.0 {
                "stone_detiled"
            } else {
                "stone"
            },
            "stone.png",
        )
    };
    TextureSet {
        entries: vec![slot(0.0), slot(0.7)],
    }
}

/// An albedo with a mark in it that repetition cannot hide: a bright
/// corner blot and a diagonal, so a grid of repeats reads as a grid.
fn write_test_assets() -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join("jackdaw_terrain_clipmap_verify");
    std::fs::create_dir_all(&dir)?;

    write_png(&dir.join("stone.png"), |x, y| {
        let (fx, fy) = (x as f32, y as f32);
        let blot = ((fx - 12.0).powi(2) + (fy - 12.0).powi(2)).sqrt() < 7.0;
        let diagonal = (fx - fy).abs() < 3.0;
        match (blot, diagonal) {
            (true, _) => [210, 200, 175, 255],
            (_, true) => [80, 74, 68, 255],
            _ => [128, 122, 112, 255],
        }
    })?;
    write_png(&dir.join("stone_h.png"), |x, y| {
        let v = 90 + (((x / 8 + y / 8) % 2) as u8) * 40;
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

/// Rolling ground with features at several scales, so a level that
/// stepped over the fine ones shows it.
fn hills(resolution: u32, size: f32) -> Heightmap {
    let mut map = Heightmap::new(resolution, Vec2::splat(size), 40.0);
    for z in 0..resolution {
        for x in 0..resolution {
            let (fx, fz) = (x as f32, z as f32);
            let h = (fx * 0.031).sin() * 9.0
                + (fz * 0.027).cos() * 7.0
                + (fx * 0.11).sin() * (fz * 0.13).cos() * 2.5
                + (fx * 0.41).sin() * 0.6;
            map.set_height(x, z, h);
        }
    }
    map
}

/// The 129 subject's heights, scattered into a terrain document and gathered
/// back out of it.
///
/// 129 is not a power of two, so the document stores it as four regions of 128
/// with the last row and column of the grid living in the neighbours. Meshing
/// what reads back puts the region gather on screen rather than a grid that
/// never met region storage.
fn odd_through_regions() -> Heightmap {
    let mut map = hills(ODD_RESOLUTION, ODD_SIZE);
    let mut document = RegionTerrainData {
        regions: TerrainRegions::new(RegionSize::for_resolution(ODD_RESOLUTION)),
        ..default()
    };
    document
        .regions
        .ensure_grid(ODD_RESOLUTION)
        .expect("inside the region cap");
    document.set_grid_heights(&map.heights);
    map.heights = document.regions.sampled_grid_heights(ODD_RESOLUTION);
    map
}

/// The clipmap levels of one terrain, as meshes, for a viewer at
/// `viewer` in that terrain's grid coordinates.
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

/// Left half plain, right half detiled, split down the middle.
fn plate_control() -> Vec<Control> {
    let mut control = vec![Control::default(); (PLATE_RESOLUTION * PLATE_RESOLUTION) as usize];
    for gz in 0..PLATE_RESOLUTION {
        for gx in 0..PLATE_RESOLUTION {
            let id = if gx < PLATE_RESOLUTION / 2 {
                PLAIN
            } else {
                DETILED
            };
            control[(gz * PLATE_RESOLUTION + gx) as usize] = Control::default().with_base_id(id);
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
    plate: Vec<Entity>,
    attached: bool,
    waited: u32,
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut standard: ResMut<Assets<StandardMaterial>>,
    assets: Res<AssetServer>,
) {
    let ground = standard.add(StandardMaterial {
        base_color: Color::srgb(0.55, 0.55, 0.52),
        perceptual_roughness: 0.95,
        ..default()
    });

    // The hills are built for where the first shot stands, and the later shots
    // fly around that surface, so the ring boundaries stay put across shots.
    let hills_map = hills(HILLS_RESOLUTION, HILLS_SIZE);
    for data in clipmap_meshes(&hills_map, Vec2::splat(128.0)) {
        commands.spawn((
            Mesh3d(meshes.add(mesh_of(data))),
            MeshMaterial3d(ground.clone()),
            Transform::from_translation(HILLS_AT),
            RenderLayers::layer(HILLS_LAYER),
        ));
    }

    // The 129 subject, meshed the same way.
    let odd_map = odd_through_regions();
    for data in clipmap_meshes(&odd_map, Vec2::splat(64.0)) {
        commands.spawn((
            Mesh3d(meshes.add(mesh_of(data))),
            MeshMaterial3d(ground.clone()),
            Transform::from_translation(ODD_AT),
            RenderLayers::layer(ODD_LAYER),
        ));
    }

    // The detiling plate: flat, so nothing but the texture is on screen.
    let plate_map = Heightmap::new(PLATE_RESOLUTION, Vec2::splat(PLATE_SIZE), 1.0);
    let plate: Vec<Entity> = clipmap_meshes(&plate_map, Vec2::splat(32.0))
        .into_iter()
        .map(|data| {
            commands
                .spawn((
                    Mesh3d(meshes.add(mesh_of(data))),
                    Transform::from_translation(PLATE_AT),
                    RenderLayers::layer(PLATE_LAYER),
                ))
                .id()
        })
        .collect();

    commands.spawn((
        DirectionalLight {
            illuminance: 12_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(60.0, 120.0, 40.0).looking_at(Vec3::ZERO, Vec3::Y),
        // One light across all three subjects, so the shots differ only in
        // what the camera is allowed to see.
        RenderLayers::from_layers(&[HILLS_LAYER, PLATE_LAYER, ODD_LAYER]),
    ));

    let camera = commands
        .spawn((
            Camera3d::default(),
            Transform::from_xyz(0.0, 60.0, 120.0).looking_at(Vec3::ZERO, Vec3::Y),
            RenderLayers::layer(HILLS_LAYER),
        ))
        .id();

    let set = verify_set();
    let load = |path: &Option<String>, srgb: bool| {
        path.as_ref().map(|path| {
            assets
                .load_builder()
                .with_settings(move |settings: &mut bevy::image::ImageLoaderSettings| {
                    settings.is_srgb = srgb;
                })
                .load(path.clone())
        })
    };
    let images = TextureSetImages {
        albedo: set.entries.iter().map(|e| load(&e.albedo, true)).collect(),
        normal: set.entries.iter().map(|e| load(&e.normal, false)).collect(),
        height: set.entries.iter().map(|e| load(&e.height, false)).collect(),
    };

    commands.insert_resource(Verify {
        frame: 0,
        camera,
        set,
        images,
        plate,
        attached: false,
        waited: 0,
    });
}

/// Build the splat material once its textures have decoded and put it on
/// the plate.
fn attach_material(
    mut commands: Commands,
    mut verify: ResMut<Verify>,
    mut materials: ResMut<Assets<TerrainSplatMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    if verify.attached {
        return;
    }
    let built = match splat_images(&verify.set, &verify.images, &images) {
        Ok(built) => built,
        Err(SplatBuildError::NotReady) => {
            verify.waited += 1;
            if verify.waited == 300 {
                eprintln!("terrain_clipmap_verify: textures still not decoded after 300 frames");
            }
            return;
        }
        Err(other) => {
            eprintln!("terrain_clipmap_verify: {other}");
            verify.attached = true;
            return;
        }
    };

    let control = images.add(control_image(&ControlTexels::from_control(
        &plate_control(),
        PLATE_RESOLUTION,
    )));
    let slope = images.add(slope_image(&Heightmap::new(
        PLATE_RESOLUTION,
        Vec2::splat(PLATE_SIZE),
        1.0,
    )));
    let arrays = SplatArrayHandles {
        albedo: images.add(built.albedo),
        normal: images.add(built.normal),
        height: images.add(built.height),
    };
    let material = materials.add(TerrainSplatMaterial::new(
        &verify.set,
        arrays,
        control,
        slope,
        Vec2::splat(PLATE_SIZE),
        PLATE_RESOLUTION,
        AutoTerrainSettings::default(),
    ));
    for entity in &verify.plate {
        commands
            .entity(*entity)
            .insert(MeshMaterial3d(material.clone()));
    }
    verify.attached = true;
}

/// Where a shot stands, what it looks at, and the one subject it is
/// allowed to see.
struct Shot {
    path: &'static str,
    from: Vec3,
    at: Vec3,
    layer: usize,
}

/// Where each shot stands and what it shows.
fn shots() -> Vec<Shot> {
    let shot = |path, from, at, layer| Shot {
        path,
        from,
        at,
        layer,
    };
    vec![
        // Ring transitions from four heights over the same ground. The level
        // boundaries move outward as the camera climbs, so a crack that opens
        // at only one distance still lands in a shot.
        shot(
            "/tmp/terrain_clipmap_low.png",
            Vec3::new(0.0, 22.0, 190.0),
            Vec3::new(0.0, 0.0, -20.0),
            HILLS_LAYER,
        ),
        shot(
            "/tmp/terrain_clipmap_mid.png",
            Vec3::new(0.0, 90.0, 240.0),
            HILLS_AT,
            HILLS_LAYER,
        ),
        shot(
            "/tmp/terrain_clipmap_high.png",
            Vec3::new(0.0, 250.0, 330.0),
            HILLS_AT,
            HILLS_LAYER,
        ),
        shot(
            "/tmp/terrain_clipmap_grazing.png",
            Vec3::new(-215.0, 14.0, 30.0),
            Vec3::new(80.0, 0.0, -10.0),
            HILLS_LAYER,
        ),
        // Detiling: both halves of the plate in one frame, then close in
        // on the join so the two treatments meet in the middle of it.
        shot(
            "/tmp/terrain_detile_pair.png",
            PLATE_AT + Vec3::new(0.0, 95.0, 95.0),
            PLATE_AT,
            PLATE_LAYER,
        ),
        shot(
            "/tmp/terrain_detile_close.png",
            PLATE_AT + Vec3::new(0.0, 24.0, 34.0),
            PLATE_AT,
            PLATE_LAYER,
        ),
        // The 129 subject rendering at all.
        shot(
            "/tmp/terrain_clipmap_129.png",
            ODD_AT + Vec3::new(0.0, 120.0, 190.0),
            ODD_AT,
            ODD_LAYER,
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
    let shots = shots();
    const SETTLE: u32 = 90;
    const GAP: u32 = 30;
    for (index, shot) in shots.iter().enumerate() {
        let move_at = SETTLE + index as u32 * GAP * 2;
        if verify.frame == move_at {
            if let Ok(mut transform) = transforms.get_mut(verify.camera) {
                *transform = Transform::from_translation(shot.from).looking_at(shot.at, Vec3::Y);
            }
            commands
                .entity(verify.camera)
                .insert(RenderLayers::layer(shot.layer));
        }
        if verify.frame == move_at + GAP {
            shoot(&mut commands, shot.path);
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
            Ok(()) => info!("terrain_clipmap_verify: wrote {}", path.display()),
            Err(err) => error!(
                "terrain_clipmap_verify: cannot write {}: {err}",
                path.display()
            ),
        },
        Err(err) => error!("terrain_clipmap_verify: cannot convert captured frame: {err}"),
    }
}
