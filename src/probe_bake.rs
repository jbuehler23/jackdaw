//! Baking a [`ReflectionProbe`]: six offscreen cameras at the probe render the
//! scene, and their faces are written beside the scene as one Radiance HDR
//! image the probe then reflects.

use std::path::{Path, PathBuf};

use bevy::camera::visibility::RenderLayers;
use bevy::camera::{Exposure, Hdr, RenderTarget};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::prelude::*;
use bevy::render::render_resource::{TextureFormat, TextureUsages};
use bevy::render::view::Msaa;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use jackdaw_api::prelude::*;
use jackdaw_scene_types::ReflectionProbe;
use jackdaw_surface::environment::{LightBakeCamera, cube_face_direction};

/// Frames a face camera renders before it is read back, so pipelines, shadows
/// and the environment settle.
const SETTLE_FRAMES: u32 = 30;

/// Exposure that leaves the recorded light as the scene's own luminance: Bevy
/// divides by `1.2 * 2^ev100`, which is one here.
const UNIT_EXPOSURE_EV100: f32 = -0.263_034_4;

const PROBE_TYPE_PATH: &str = "jackdaw_scene_types::types::ReflectionProbe";

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<ProbeBakes>()
        .add_observer(hide_probe_volumes)
        .add_systems(Update, (read_back_probe_faces, finish_probe_bakes).chain());
}

/// Keep a probe's light probe out of the outliner and out of the saved scene:
/// it is rebuilt from the probe whenever the scene loads.
fn hide_probe_volumes(add: On<Add, jackdaw_surface::probe::ProbeVolume>, mut commands: Commands) {
    commands
        .entity(add.entity)
        .insert((crate::EditorHidden, crate::NonSerializable));
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<EnvironmentBakeProbeOp>()
        .register_menu_entry::<EnvironmentBakeProbeOp>(TopLevelMenu::Tools);
}

/// Whether the selection is a reflection probe to bake.
fn selected_probe(
    selection: Res<crate::selection::Selection>,
    probes: Query<(), With<ReflectionProbe>>,
) -> bool {
    selection
        .primary()
        .is_some_and(|entity| probes.contains(entity))
}

/// A probe being baked: where its image goes and the faces read back so far.
struct ProbeBake {
    probe: Entity,
    file: PathBuf,
    asset_path: String,
    face_size: u32,
    faces: [Option<Vec<[f32; 3]>>; 6],
}

#[derive(Resource, Default)]
struct ProbeBakes(Vec<ProbeBake>);

/// One of a bake's six cameras, waiting to be read back.
#[derive(Component)]
struct ProbeFaceCamera {
    probe: Entity,
    face: usize,
    mirrored: bool,
    frames_left: u32,
}

/// How a camera looks to record cube face `face`: its forward and up, and
/// whether its image comes out mirrored against the face's own column order.
pub(crate) fn face_view(face: u32) -> (Vec3, Vec3, bool) {
    let forward = cube_face_direction(face, 0.0, 0.0);
    let off_axis = |toward: Vec3| toward - forward * toward.dot(forward);
    let across = off_axis(cube_face_direction(face, 0.5, 0.0));
    let up = -off_axis(cube_face_direction(face, 0.0, 0.5)).normalize();
    let camera_right = forward.cross(up);
    (forward, up, camera_right.dot(across) < 0.0)
}

/// Render the six faces of the probe `entity` sees from where it stands.
#[operator(
    id = "environment.bake_probe",
    label = "Bake Reflection Probe",
    description = "Render what a reflection probe sees from where it stands into the cubemap it \
                   reflects, written beside the scene.",
    allows_undo = false,
    is_available = selected_probe,
    params(entity(Entity, doc = "The probe to bake. Defaults to the selected entity."))
)]
pub(crate) fn environment_bake_probe(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let id = "environment.bake_probe";
    let probe = params
        .as_entity("entity")
        .or_else(|| world.resource::<crate::selection::Selection>().primary());
    let Some(probe) = probe.filter(|probe| world.get::<ReflectionProbe>(*probe).is_some()) else {
        warn_caller(
            world,
            format!("{id}: name an entity with a ReflectionProbe"),
        );
        return OperatorResult::Cancelled;
    };
    if world
        .resource::<ProbeBakes>()
        .0
        .iter()
        .any(|bake| bake.probe == probe)
    {
        warn_caller(world, format!("{id}: that probe is already baking"));
        return OperatorResult::Cancelled;
    }
    let Some(assets_dir) = crate::project::open_project_assets_dir() else {
        warn_caller(world, format!("{id}: no project is open"));
        return OperatorResult::Cancelled;
    };
    let Some(scene) = world
        .get_resource::<crate::scene_io::SceneFilePath>()
        .and_then(|scene| scene.path.clone())
    else {
        warn_caller(
            world,
            format!("{id}: save the scene first; the bake is written beside it"),
        );
        return OperatorResult::Cancelled;
    };
    let settings = world
        .get::<ReflectionProbe>(probe)
        .cloned()
        .unwrap_or_default();
    let name = world
        .get::<Name>(probe)
        .map(|name| name.as_str().to_string());
    let taken = baked_paths_except(world, probe);
    let file = bake_file(Path::new(&scene), &settings, name.as_deref(), probe, &taken);
    let asset_path = jackdaw_scene_types::to_asset_path(&file.to_string_lossy(), Some(&assets_dir));
    let Some(position) = world
        .get::<GlobalTransform>(probe)
        .map(GlobalTransform::translation)
    else {
        return OperatorResult::Cancelled;
    };

    let face_size = settings.face_size();
    for face in 0..6 {
        spawn_face_camera(world, probe, face, position, face_size);
    }
    world.resource_mut::<ProbeBakes>().0.push(ProbeBake {
        probe,
        file,
        asset_path,
        face_size,
        faces: Default::default(),
    });
    OperatorResult::Finished
}

/// The baked paths every other probe in the world holds.
fn baked_paths_except(world: &mut World, probe: Entity) -> Vec<String> {
    let mut probes = world.query::<(Entity, &ReflectionProbe)>();
    probes
        .iter(world)
        .filter(|(entity, other)| *entity != probe && !other.baked.is_empty())
        .map(|(_, other)| other.baked.clone())
        .collect()
}

/// Where a probe's bake is written: the path it already names, or a new one in
/// a folder named after the scene, after the probe's name, that no other probe
/// holds.
fn bake_file(
    scene: &Path,
    probe: &ReflectionProbe,
    name: Option<&str>,
    entity: Entity,
    taken: &[String],
) -> PathBuf {
    let folder = scene.with_extension("");
    if !probe.baked.is_empty() && !taken.contains(&probe.baked) {
        let assets = crate::project::open_project_assets_dir();
        let named = assets
            .map(|assets| assets.join(&probe.baked))
            .unwrap_or_else(|| PathBuf::from(&probe.baked));
        return named;
    }
    let stem: String = name
        .unwrap_or("probe")
        .chars()
        .map(|c| match c.is_ascii_alphanumeric() {
            true => c.to_ascii_lowercase(),
            false => '_',
        })
        .collect();
    let stem = if stem.trim_matches('_').is_empty() {
        format!("probe_{}", entity.index())
    } else {
        stem
    };
    (0..)
        .map(|n| match n {
            0 => folder.join(format!("{stem}.probe.hdr")),
            n => folder.join(format!("{stem}_{n}.probe.hdr")),
        })
        .find(|candidate| {
            let spelled = candidate.to_string_lossy();
            !taken.iter().any(|path| spelled.ends_with(path.as_str()))
        })
        .expect("the candidates are unbounded")
}

fn spawn_face_camera(world: &mut World, probe: Entity, face: u32, position: Vec3, size: u32) {
    let (forward, up, mirrored) = face_view(face);
    let mut image = Image::new_target_texture(size, size, TextureFormat::Rgba16Float, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = world.resource_mut::<Assets<Image>>().add(image);
    world.spawn((
        crate::EditorEntity,
        Camera3d::default(),
        Camera {
            order: -3,
            ..default()
        },
        Hdr,
        Tonemapping::None,
        Exposure {
            ev100: UNIT_EXPOSURE_EV100,
        },
        Msaa::Off,
        LightBakeCamera,
        Projection::Perspective(PerspectiveProjection {
            fov: std::f32::consts::FRAC_PI_2,
            aspect_ratio: 1.0,
            near: 0.05,
            far: 50_000.0,
            ..default()
        }),
        RenderTarget::Image(target.into()),
        Transform::from_translation(position).looking_to(forward, up),
        RenderLayers::layer(0),
        crate::camera_capture::KeepsGizmosOut,
        ProbeFaceCamera {
            probe,
            face: face as usize,
            mirrored,
            frames_left: SETTLE_FRAMES,
        },
    ));
}

fn read_back_probe_faces(
    mut commands: Commands,
    mut cameras: Query<(Entity, &mut ProbeFaceCamera, &RenderTarget)>,
) {
    for (camera, mut face, target) in &mut cameras {
        if face.frames_left > 0 {
            face.frames_left -= 1;
            continue;
        }
        let Some(image) = target.as_image().cloned() else {
            commands.entity(camera).despawn();
            continue;
        };
        let (probe, index, mirrored) = (face.probe, face.face, face.mirrored);
        commands.entity(camera).remove::<ProbeFaceCamera>();
        commands.spawn(Screenshot::image(image)).observe(
            move |captured: On<ScreenshotCaptured>,
                  mut commands: Commands,
                  mut bakes: ResMut<ProbeBakes>| {
                if let Some(bake) = bakes.0.iter_mut().find(|bake| bake.probe == probe) {
                    bake.faces[index] = face_texels(&captured.image, mirrored);
                }
                if let Ok(mut spent) = commands.get_entity(camera) {
                    spent.despawn();
                }
            },
        );
    }
}

/// A read-back face as linear RGB rows, mirrored left to right when the
/// camera's columns run against the face's.
fn face_texels(image: &Image, mirrored: bool) -> Option<Vec<[f32; 3]>> {
    let width = image.width() as usize;
    let height = image.height() as usize;
    let data = image.data.as_ref()?;
    if image.texture_descriptor.format != TextureFormat::Rgba16Float
        || data.len() < width * height * 8
    {
        return None;
    }
    let texel = |x: usize, y: usize| {
        let at = (y * width + x) * 8;
        let channel = |offset: usize| {
            half::f16::from_le_bytes([data[at + offset], data[at + offset + 1]]).to_f32()
        };
        [channel(0), channel(2), channel(4)]
    };
    let mut out = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let column = if mirrored { width - 1 - x } else { x };
            out.push(texel(column, y));
        }
    }
    Some(out)
}

/// Write the six faces top to bottom as one Radiance HDR image.
fn write_stacked_faces(path: &Path, face_size: u32, faces: &[Vec<[f32; 3]>]) -> Result<(), String> {
    let pixels: Vec<::image::Rgb<f32>> = faces
        .iter()
        .flatten()
        .map(|&[r, g, b]| ::image::Rgb([r.max(0.0), g.max(0.0), b.max(0.0)]))
        .collect();
    let size = face_size as usize;
    if pixels.len() != size * size * 6 {
        return Err("the faces do not make a full cube".to_string());
    }
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|err| err.to_string())?;
    }
    let file = std::fs::File::create(path).map_err(|err| err.to_string())?;
    ::image::codecs::hdr::HdrEncoder::new(std::io::BufWriter::new(file))
        .encode(&pixels, size, size * 6)
        .map_err(|err| err.to_string())
}

fn finish_probe_bakes(world: &mut World) {
    let finished: Vec<ProbeBake> = {
        let mut bakes = world.resource_mut::<ProbeBakes>();
        let (done, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut bakes.0)
            .into_iter()
            .partition(|bake| bake.faces.iter().all(Option::is_some));
        bakes.0 = waiting;
        done
    };
    for bake in finished {
        let faces: Vec<Vec<[f32; 3]>> = bake.faces.into_iter().flatten().collect();
        if let Err(err) = write_stacked_faces(&bake.file, bake.face_size, &faces) {
            error!(
                "reflection probe bake: cannot write {}: {err}",
                bake.file.display()
            );
            continue;
        }
        info!("reflection probe bake: wrote {}", bake.file.display());
        let current = world
            .get::<ReflectionProbe>(bake.probe)
            .map(|probe| probe.baked.clone());
        match current {
            Some(path) if path == bake.asset_path => {
                world
                    .resource::<AssetServer>()
                    .reload(bake.asset_path.clone());
            }
            Some(_) => {
                crate::selection::select_for_edit(world, bake.probe);
                crate::commands::field_edit_commit(
                    world,
                    PROBE_TYPE_PATH,
                    "baked",
                    &serde_json::Value::String(bake.asset_path.clone()),
                    "Bake reflection probes",
                );
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_face_camera_looks_down_its_axis_with_a_right_angled_up() {
        let mut axes = Vec::new();
        for face in 0..6 {
            let (forward, up, _) = face_view(face);
            assert!(forward.dot(up).abs() < 1e-5, "face {face}");
            axes.push(forward.round());
        }
        assert_eq!(
            axes,
            vec![
                Vec3::X,
                Vec3::NEG_X,
                Vec3::Y,
                Vec3::NEG_Y,
                Vec3::Z,
                Vec3::NEG_Z
            ]
        );
    }

    #[test]
    fn a_mirrored_face_is_read_back_right_to_left() {
        let mut image = Image::new_fill(
            bevy::render::render_resource::Extent3d {
                width: 2,
                height: 1,
                depth_or_array_layers: 1,
            },
            bevy::render::render_resource::TextureDimension::D2,
            &[0; 8],
            TextureFormat::Rgba16Float,
            bevy::asset::RenderAssetUsages::MAIN_WORLD,
        );
        let data = image.data.as_mut().unwrap();
        data[..2].copy_from_slice(&half::f16::from_f32(1.0).to_le_bytes());
        data[8..10].copy_from_slice(&half::f16::from_f32(2.0).to_le_bytes());

        assert_eq!(face_texels(&image, false).unwrap()[0][0], 1.0);
        assert_eq!(face_texels(&image, true).unwrap()[0][0], 2.0);
    }

    #[test]
    fn the_written_faces_load_back_as_a_cubemap_in_order() {
        let folder = tempfile::tempdir().expect("tempdir");
        let path = folder.path().join("lake.probe.hdr");
        let size = 4u32;
        let faces: Vec<Vec<[f32; 3]>> = (0..6)
            .map(|face| vec![[face as f32 + 0.5, 1.0, 2.0]; (size * size) as usize])
            .collect();

        write_stacked_faces(&path, size, &faces).expect("written");

        let decoded = ::image::open(&path).expect("reads back").to_rgba32f();
        let stacked = Image::from_dynamic(
            ::image::DynamicImage::ImageRgba32F(decoded),
            false,
            bevy::asset::RenderAssetUsages::MAIN_WORLD,
        );
        let cubemap =
            jackdaw_surface::probe::cubemap_from_stacked_faces(&stacked).expect("a cubemap");
        let data = cubemap.data.as_ref().unwrap();
        for face in 0..6u32 {
            let at = (face * size * size * 8) as usize;
            let red = half::f16::from_le_bytes([data[at], data[at + 1]]).to_f32();
            assert!(
                (red - (face as f32 + 0.5)).abs() < 0.05,
                "face {face}: {red}"
            );
        }
    }

    #[test]
    fn a_new_bake_lands_beside_the_scene_under_a_name_no_other_probe_holds() {
        let scene = Path::new("/project/assets/scenes/valley.bsn");
        let probe = ReflectionProbe::default();
        let entity = Entity::PLACEHOLDER;
        let first = bake_file(scene, &probe, Some("Lake Probe"), entity, &[]);
        assert_eq!(
            first,
            Path::new("/project/assets/scenes/valley/lake_probe.probe.hdr")
        );
        let second = bake_file(
            scene,
            &probe,
            Some("Lake Probe"),
            entity,
            &["scenes/valley/lake_probe.probe.hdr".to_string()],
        );
        assert_eq!(
            second,
            Path::new("/project/assets/scenes/valley/lake_probe_1.probe.hdr")
        );
    }
}
