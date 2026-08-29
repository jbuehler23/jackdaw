//! Manual visual verification for the grid depth yield
//! (`src/editor_grid_depth_patch.wgsl`'s `GRID_DEPTH_YIELD_WORLD`).
//!
//! Not part of the test suite: needs a real GPU and a display, and opens an
//! actual window (winit requires its event loop on the process's main thread,
//! which `cargo test`'s worker threads are not). Run by hand:
//!
//! ```text
//! cargo run --example grid_yield_verify
//! ```
//!
//! Spawns a flat opaque plane at y=0, coplanar with the grid's own plane, which
//! is the geometry a default-Generate terrain produces (valley floors at y=0,
//! offset 0). Five camera compositions:
//!
//! - "far": the editor's default viewport start transform
//!   (`Transform::from_xyz(0.0, 4.0, 8.0)`, see `src/viewport.rs`), looking
//!   down at the plane.
//! - "close": pulled in along the same sightline to half that distance, the
//!   distance range over which the crosshatch grows.
//! - "grazing": low and far, skimming almost parallel to the plane, where the
//!   ray-plane intersection's denominator shrinks toward zero.
//! - "km-far" and "km-near": the same grazing sightline over a kilometre of
//!   ground rather than two hundred metres, approached in two steps. See
//!   [`KM_PLANE_SIZE`] for what changes with the scale.
//!
//! Writes all five to `/tmp/grid_yield_{far,close,grazing,km_far,km_near}.png`.
//!
//! Nothing here asserts on pixels; the run produces evidence for a human to
//! look at.

use std::path::{Path, PathBuf};

use bevy::app::AppExit;
use bevy::asset::io::embedded::{_embedded_asset_path, EmbeddedAssetRegistry};
use bevy::color::palettes::css;
use bevy::dev_tools::infinite_grid::{InfiniteGrid, InfiniteGridPlugin, InfiniteGridSettings};
use bevy::pbr::{MeshMaterial3d, StandardMaterial};
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};

const FAR: Vec3 = Vec3::new(0.0, 4.0, 8.0);
// Half the far distance along the same sightline, so the pair differs only in
// distance and the patches can be compared as the camera moves closer.
const CLOSE: Vec3 = Vec3::new(0.0, 2.0, 4.0);
// Low, oblique and zoomed out, where the ray-plane intersection's denominator
// shrinks toward zero as the ray nears parallel to the plane. This skims along
// the plane's surface rather than looking down at it, so the plane needs extent
// under the sightline; see PLANE_SIZE.
const GRAZING: Vec3 = Vec3::new(0.0, 0.3, 90.0);
const GRAZING_TARGET: Vec3 = Vec3::new(0.0, 0.0, -90.0);
const PLANE_SIZE: f32 = 200.0;

/// A kilometre of coplanar ground, which is what a default terrain is:
/// `terrain.generate` lays four regions a side at the default cell size.
///
/// Two things change at this scale that are not visible at two hundred metres.
/// The depth the grid has to lose a tie against is a thousand times the near
/// plane out here, so the fixed centimetre
/// [`GRID_DEPTH_YIELD_WORLD`](../src/editor_grid_depth_patch.wgsl) yields by is
/// a far smaller share of it; and the grid's fadeout keeps it drawing only
/// within [`FADEOUT`] of the eye, so a band can appear only in the near ground
/// while the far ground supplies the depth range. The pair below approaches
/// along one sightline so that a band growing on approach shows as a difference
/// between two shots.
const KM_PLANE_SIZE: f32 = 1024.0;

/// How far apart the kilometre plane's vertices sit.
///
/// Not one quad: a two-triangle plane interpolates depth exactly across the
/// whole screen, the one case that cannot disagree with the grid's analytic
/// depth. Real ground is a lattice of small triangles each interpolating on its
/// own, which is what the editor's clipmap emits; its coarsest level over a
/// kilometre is eight-metre quads.
const KM_PLANE_STEP: f32 = 8.0;

/// Low over the far end of the kilometre plane, looking down its length.
const KM_FAR: Vec3 = Vec3::new(0.0, 3.0, 480.0);
/// The same sightline, most of the way in.
const KM_NEAR: Vec3 = Vec3::new(0.0, 3.0, 120.0);
const KM_TARGET: Vec3 = Vec3::new(0.0, 0.0, -480.0);

/// How far from the eye the grid stops drawing, matching the editor's value
/// (`GridSettings::default`, `src/snapping.rs`). Past this the grid has faded
/// to nothing and cannot tie with anything, so a band stays a near-ground
/// phenomenon however large the terrain is.
const FADEOUT: f32 = 100.0;

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, InfiniteGridPlugin))
        .add_plugins(patch_grid_shader)
        .add_systems(Startup, setup)
        .add_systems(Update, drive)
        .run();
}

/// The patch `src/editor_grid_depth_patch.rs` applies in the editor, inlined
/// here rather than imported: that module's `plugin` fn is `pub(crate)` and an
/// example is a separate crate.
fn patch_grid_shader(app: &mut App) {
    let registry = app.world().resource::<EmbeddedAssetRegistry>();
    let asset_path = _embedded_asset_path(
        "bevy_dev_tools",
        Path::new("src"),
        Path::new("src/infinite_grid.rs"),
        Path::new("infinite_grid.wgsl"),
    );
    registry.insert_asset(
        PathBuf::from("src/editor_grid_depth_patch.wgsl"),
        &asset_path,
        include_bytes!("../src/editor_grid_depth_patch.wgsl").as_slice(),
    );
}

#[derive(Resource)]
struct Verify {
    frame: u32,
    camera: Entity,
    /// The two-hundred-metre plane and the kilometre one. Only one is visible
    /// at a time: they are coplanar, so both at once would z-fight each other.
    small_plane: Entity,
    km_plane: Entity,
}

/// A flat lattice at y=0, `step` apart, `size` on a side.
///
/// Written out rather than taking `Plane3d`'s two triangles, for the reason
/// [`KM_PLANE_STEP`] gives.
fn lattice(size: f32, step: f32) -> Mesh {
    let side = (size / step).round() as u32;
    let half = size * 0.5;
    let vertices = side + 1;
    let mut positions = Vec::with_capacity((vertices * vertices) as usize);
    let mut normals = Vec::with_capacity((vertices * vertices) as usize);
    let mut uvs = Vec::with_capacity((vertices * vertices) as usize);
    for z in 0..vertices {
        for x in 0..vertices {
            positions.push([x as f32 * step - half, 0.0, z as f32 * step - half]);
            normals.push([0.0, 1.0, 0.0]);
            uvs.push([x as f32 / side as f32, z as f32 / side as f32]);
        }
    }
    let mut indices = Vec::with_capacity((side * side * 6) as usize);
    for z in 0..side {
        for x in 0..side {
            let tl = z * vertices + x;
            let tr = tl + 1;
            let bl = (z + 1) * vertices + x;
            let br = bl + 1;
            indices.extend_from_slice(&[tl, bl, tr, tr, bl, br]);
        }
    }
    let mut mesh = Mesh::new(
        bevy::mesh::PrimitiveTopology::TriangleList,
        bevy::asset::RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(bevy::mesh::Indices::U32(indices));
    mesh
}

/// Show one plane and hide the other.
fn show(commands: &mut Commands, shown: Entity, hidden: Entity) {
    commands.entity(shown).insert(Visibility::Inherited);
    commands.entity(hidden).insert(Visibility::Hidden);
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        InfiniteGrid,
        InfiniteGridSettings {
            fadeout_distance: FADEOUT,
            ..Default::default()
        },
        Transform::IDENTITY,
    ));
    let ground = mats.add(StandardMaterial::from(Color::from(css::SLATE_GRAY)));
    // Coplanar with the grid, like a default-Generate terrain's valley floors
    // at y=0 (offset 0). Sized to give the grazing shot surface to skim across.
    let small_plane = commands
        .spawn((
            Mesh3d(meshes.add(Plane3d::default().mesh().size(PLANE_SIZE, PLANE_SIZE))),
            MeshMaterial3d(ground.clone()),
            Transform::IDENTITY,
        ))
        .id();
    let km_plane = commands
        .spawn((
            Mesh3d(meshes.add(lattice(KM_PLANE_SIZE, KM_PLANE_STEP))),
            MeshMaterial3d(ground),
            Transform::IDENTITY,
            Visibility::Hidden,
        ))
        .id();
    commands.spawn((
        DirectionalLight::default(),
        Transform::from_xyz(3.0, 8.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    let camera = commands
        .spawn((
            Camera3d::default(),
            Transform::from_translation(FAR).looking_at(Vec3::ZERO, Vec3::Y),
        ))
        .id();
    commands.insert_resource(Verify {
        frame: 0,
        camera,
        small_plane,
        km_plane,
    });
}

fn drive(
    mut verify: ResMut<Verify>,
    mut transforms: Query<&mut Transform>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    verify.frame += 1;
    // Settle, capture "far"; move and settle, capture "close"; move and settle,
    // capture "grazing"; then exit. The 90-frame settle budget matches
    // screenshot.rs and boot_ops.rs, so a slow first-frame shader compile
    // cannot land inside a capture.
    match verify.frame {
        90 => {
            commands.spawn(Screenshot::primary_window()).observe(
                |capture: On<ScreenshotCaptured>| {
                    write_png(&capture.image, Path::new("/tmp/grid_yield_far.png"));
                },
            );
        }
        120 => {
            if let Ok(mut tf) = transforms.get_mut(verify.camera) {
                *tf = Transform::from_translation(CLOSE).looking_at(Vec3::ZERO, Vec3::Y);
            }
        }
        210 => {
            commands.spawn(Screenshot::primary_window()).observe(
                |capture: On<ScreenshotCaptured>| {
                    write_png(&capture.image, Path::new("/tmp/grid_yield_close.png"));
                },
            );
        }
        240 => {
            if let Ok(mut tf) = transforms.get_mut(verify.camera) {
                *tf = Transform::from_translation(GRAZING).looking_at(GRAZING_TARGET, Vec3::Y);
            }
        }
        330 => {
            commands.spawn(Screenshot::primary_window()).observe(
                |capture: On<ScreenshotCaptured>| {
                    write_png(&capture.image, Path::new("/tmp/grid_yield_grazing.png"));
                },
            );
        }
        // Swap onto the kilometre of ground and take the same sightline twice,
        // far then near, so a band that grows on approach shows as a difference
        // between the pair rather than having to be caught mid-move.
        360 => {
            show(&mut commands, verify.km_plane, verify.small_plane);
            if let Ok(mut tf) = transforms.get_mut(verify.camera) {
                *tf = Transform::from_translation(KM_FAR).looking_at(KM_TARGET, Vec3::Y);
            }
        }
        450 => {
            commands.spawn(Screenshot::primary_window()).observe(
                |capture: On<ScreenshotCaptured>| {
                    write_png(&capture.image, Path::new("/tmp/grid_yield_km_far.png"));
                },
            );
        }
        480 => {
            if let Ok(mut tf) = transforms.get_mut(verify.camera) {
                *tf = Transform::from_translation(KM_NEAR).looking_at(KM_TARGET, Vec3::Y);
            }
        }
        570 => {
            commands.spawn(Screenshot::primary_window()).observe(
                |capture: On<ScreenshotCaptured>| {
                    write_png(&capture.image, Path::new("/tmp/grid_yield_km_near.png"));
                },
            );
        }
        600 => {
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}

fn write_png(image: &Image, path: &Path) {
    match image.clone().try_into_dynamic() {
        Ok(dynamic) => match dynamic
            .to_rgb8()
            .save_with_format(path, ::image::ImageFormat::Png)
        {
            Ok(()) => info!("grid_yield_verify: wrote {}", path.display()),
            Err(err) => error!("grid_yield_verify: cannot write {}: {err}", path.display()),
        },
        Err(err) => error!("grid_yield_verify: cannot convert captured frame: {err}"),
    }
}
