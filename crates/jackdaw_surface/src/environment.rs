//! Dresses a scene's 3D cameras in its [`Environment`].

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::camera::{Exposure, Hdr};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::mesh::{MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{DistanceFog, FogFalloff, MaterialPipeline, MaterialPipelineKey};
use bevy::post_process::bloom::Bloom;
use bevy::post_process::effect_stack::Vignette;
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
    TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
};
use bevy::render::view::{ColorGrading, ColorGradingGlobal, ColorGradingSection};
use bevy::shader::ShaderRef;
use jackdaw_scene_types::{
    Ambient, EditorHidden, Environment, Fog, FogMode, NavmeshExclude, PostProcess, SceneWind, Sky,
    Tonemapper,
};

const SHADER_PATH: &str = "embedded://jackdaw_surface/shaders/sky.wgsl";

/// Texels along one face of the generated ambient cubemap.
const AMBIENT_FACE_SIZE: u32 = 16;

/// The ambient cubemap's fixed id, so a save never embeds the generated image.
const AMBIENT_MAP: Handle<Image> =
    bevy::asset::uuid_handle!("5a3e0f4c-7d2b-4e61-9a8f-2c1b6d0e7f39");

/// Applies the first [`Environment`] in the scene to every 3D camera that draws the scene.
pub struct EnvironmentPlugin;

impl Plugin for EnvironmentPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "shaders/sky.wgsl");
        app.add_plugins(MaterialPlugin::<SkyMaterial>::default());
        add_environment_systems(app);
    }
}

fn add_environment_systems(app: &mut App) {
    app.init_resource::<SceneWind>()
        .init_resource::<SceneEnvironment>()
        .add_systems(
            PostUpdate,
            (
                follow_the_scene_environment,
                dress_the_cameras,
                keep_the_sky,
            )
                .chain()
                .run_if(resource_exists::<Assets<Image>>)
                .run_if(resource_exists::<Assets<Mesh>>),
        );
}

/// The environment the scene holds this frame.
#[derive(Resource, Default)]
struct SceneEnvironment(Option<Environment>);

/// What a camera carried before the environment dressed it, put back when it no longer does.
#[derive(Component, Clone)]
pub struct UndressedCamera {
    tonemapping: Option<Tonemapping>,
    exposure: Option<Exposure>,
    grading: Option<ColorGrading>,
    bloom: Option<Bloom>,
    vignette: Option<Vignette>,
    fog: Option<DistanceFog>,
    ambient: Option<EnvironmentMapLight>,
    hdr: bool,
}

/// The entity that draws the sky.
#[derive(Component)]
pub struct SceneSky;

/// The unlit material that draws the sky behind everything.
#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
pub struct SkyMaterial {
    #[uniform(0)]
    pub sky: SkyUniform,
}

/// The sky's settings as the shader reads them.
#[derive(ShaderType, Clone, Copy, Debug, Default, PartialEq)]
pub struct SkyUniform {
    pub zenith: Vec4,
    pub horizon: Vec4,
    pub ground: Vec4,
    pub cloud_color: Vec4,
    pub cloud_drift: Vec2,
    pub horizon_softness: f32,
    pub brightness: f32,
    pub sun_cos: f32,
    pub sun_intensity: f32,
    pub cloud_coverage: f32,
    pub cloud_scale: f32,
}

impl SkyUniform {
    fn new(sky: &Sky, wind: &SceneWind) -> Self {
        let linear = |color: Color| Vec4::from_array(color.to_linear().to_f32_array());
        Self {
            zenith: linear(sky.zenith),
            horizon: linear(sky.horizon),
            ground: linear(sky.ground),
            cloud_color: linear(sky.cloud_color),
            cloud_drift: wind.0.heading() * wind.0.strength * sky.cloud_speed,
            horizon_softness: sky.horizon_softness,
            brightness: sky.brightness,
            sun_cos: (sky.sun_size.max(0.0) * 0.5).to_radians().cos(),
            sun_intensity: sky.sun_intensity,
            cloud_coverage: sky.cloud_coverage.clamp(0.0, 1.0),
            cloud_scale: sky.cloud_scale,
        }
    }
}

impl Material for SkyMaterial {
    fn vertex_shader() -> ShaderRef {
        SHADER_PATH.into()
    }

    fn fragment_shader() -> ShaderRef {
        SHADER_PATH.into()
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_write_enabled = Some(false);
        }
        Ok(())
    }
}

/// A cubemap whose every texel holds the ambient colour for its direction.
pub fn ambient_cubemap(ambient: &Ambient) -> Image {
    let size = Extent3d {
        width: AMBIENT_FACE_SIZE,
        height: AMBIENT_FACE_SIZE,
        depth_or_array_layers: 6,
    };
    let mut image = Image::new_fill(
        size,
        TextureDimension::D2,
        &[0; 8],
        TextureFormat::Rgba16Float,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube),
        ..default()
    });
    for face in 0..6 {
        for row in 0..AMBIENT_FACE_SIZE {
            for column in 0..AMBIENT_FACE_SIZE {
                let up = cubemap_direction(face, column, row).y;
                let color = ambient.color_facing(up);
                image
                    .set_color_at_3d(column, row, face, Color::LinearRgba(color))
                    .expect("the texel lies inside the cubemap");
            }
        }
    }
    image
}

/// The world direction through a texel of a cubemap face, in the +X, -X, +Y, -Y, +Z, -Z order.
pub fn cubemap_direction(face: u32, column: u32, row: u32) -> Vec3 {
    let at = |index: u32| (index as f32 + 0.5) / AMBIENT_FACE_SIZE as f32 * 2.0 - 1.0;
    let (u, v) = (at(column), at(row));
    let direction = match face {
        0 => Vec3::new(1.0, -v, -u),
        1 => Vec3::new(-1.0, -v, u),
        2 => Vec3::new(u, 1.0, v),
        3 => Vec3::new(u, -1.0, -v),
        4 => Vec3::new(u, -v, 1.0),
        _ => Vec3::new(-u, -v, -1.0),
    };
    direction.normalize()
}

fn follow_the_scene_environment(
    environments: Query<&Environment>,
    mut scene: ResMut<SceneEnvironment>,
) {
    let holding = environments.iter().next().cloned();
    if scene.0 != holding {
        scene.0 = holding;
    }
}

type CameraParts = (
    Entity,
    Option<&'static RenderLayers>,
    Option<&'static UndressedCamera>,
    Option<&'static Tonemapping>,
    Option<&'static Exposure>,
    Option<&'static ColorGrading>,
    Option<&'static Bloom>,
    Option<&'static Vignette>,
    Option<&'static DistanceFog>,
    Option<&'static EnvironmentMapLight>,
    Has<Hdr>,
);

fn dress_the_cameras(
    mut commands: Commands,
    scene: Res<SceneEnvironment>,
    mut images: ResMut<Assets<Image>>,
    mut built_ambient: Local<Option<Ambient>>,
    cameras: Query<CameraParts, With<Camera3d>>,
) {
    let dressing = scene.0.as_ref().filter(|env| env.dresses_cameras());
    let ambient = dressing
        .filter(|env| env.ambient.mode != jackdaw_scene_types::AmbientMode::Off)
        .map(|env| {
            if built_ambient.as_ref() != Some(&env.ambient) {
                images
                    .insert(&AMBIENT_MAP, ambient_cubemap(&env.ambient))
                    .expect("a fixed id is always valid");
                *built_ambient = Some(env.ambient.clone());
            }
            AMBIENT_MAP
        });

    for (
        entity,
        layers,
        undressed,
        tonemapping,
        exposure,
        grading,
        bloom,
        vignette,
        fog,
        light,
        hdr,
    ) in &cameras
    {
        if !layers.is_none_or(|layers| layers.intersects(&RenderLayers::default())) {
            continue;
        }
        let mut camera = commands.entity(entity);
        let Some(env) = dressing else {
            if let Some(undressed) = undressed {
                undressed.restore_all(&mut camera);
                camera.remove::<UndressedCamera>();
            }
            continue;
        };
        if undressed.is_some() && !scene.is_changed() {
            continue;
        }
        let undressed = undressed.cloned().unwrap_or_else(|| UndressedCamera {
            tonemapping: tonemapping.copied(),
            exposure: exposure.copied(),
            grading: grading.cloned(),
            bloom: bloom.cloned(),
            vignette: vignette.cloned(),
            fog: fog.cloned(),
            ambient: light.cloned(),
            hdr,
        });
        dress_fog(&mut camera, &env.fog, &undressed);
        match &ambient {
            Some(map) => {
                camera.insert(EnvironmentMapLight {
                    diffuse_map: map.clone(),
                    specular_map: map.clone(),
                    intensity: env.ambient.brightness,
                    ..default()
                });
            }
            None => restore(&mut camera, undressed.ambient.clone()),
        }
        dress_post(&mut camera, &env.post, &undressed);
        camera.insert(undressed);
    }
}

fn restore<T: Component>(camera: &mut EntityCommands, kept: Option<T>) {
    match kept {
        Some(value) => {
            camera.insert(value);
        }
        None => {
            camera.remove::<T>();
        }
    }
}

impl UndressedCamera {
    fn restore_all(&self, camera: &mut EntityCommands) {
        restore(camera, self.fog.clone());
        restore(camera, self.ambient.clone());
        self.restore_post(camera);
    }

    fn restore_post(&self, camera: &mut EntityCommands) {
        restore(camera, self.tonemapping);
        restore(camera, self.exposure);
        restore(camera, self.grading.clone());
        restore(camera, self.vignette.clone());
        self.restore_bloom(camera);
    }

    fn restore_bloom(&self, camera: &mut EntityCommands) {
        restore(camera, self.bloom.clone());
        if !self.hdr && self.bloom.is_none() {
            camera.remove::<Hdr>();
        }
    }
}

/// The fog component an environment's fog settings describe, if it has any.
pub fn distance_fog(fog: &Fog) -> Option<DistanceFog> {
    let falloff = match fog.mode {
        FogMode::Off => return None,
        FogMode::Linear => FogFalloff::Linear {
            start: fog.start,
            end: fog.end,
        },
        FogMode::Exponential => FogFalloff::Exponential {
            density: fog.density,
        },
        FogMode::ExponentialSquared => FogFalloff::ExponentialSquared {
            density: fog.density,
        },
    };
    Some(DistanceFog {
        color: fog.color,
        directional_light_color: fog.sun_color,
        directional_light_exponent: fog.sun_exponent,
        falloff,
    })
}

fn dress_fog(camera: &mut EntityCommands, fog: &Fog, undressed: &UndressedCamera) {
    match distance_fog(fog) {
        Some(fog) => {
            camera.insert(fog);
        }
        None => restore(camera, undressed.fog.clone()),
    }
}

/// Bevy's tone curve for an environment's choice.
pub fn tonemapping(tonemapper: Tonemapper) -> Tonemapping {
    match tonemapper {
        Tonemapper::None => Tonemapping::None,
        Tonemapper::Reinhard => Tonemapping::Reinhard,
        Tonemapper::ReinhardLuminance => Tonemapping::ReinhardLuminance,
        Tonemapper::AcesFitted => Tonemapping::AcesFitted,
        Tonemapper::AgX => Tonemapping::AgX,
        Tonemapper::SomewhatBoringDisplayTransform => Tonemapping::SomewhatBoringDisplayTransform,
        Tonemapper::TonyMcMapface => Tonemapping::TonyMcMapface,
        Tonemapper::BlenderFilmic => Tonemapping::BlenderFilmic,
        Tonemapper::KhronosPbrNeutral => Tonemapping::KhronosPbrNeutral,
    }
}

/// The colour grading an environment's post settings describe.
pub fn color_grading(post: &PostProcess) -> ColorGrading {
    let section = ColorGradingSection {
        saturation: post.saturation,
        contrast: post.contrast,
        ..default()
    };
    ColorGrading {
        global: ColorGradingGlobal {
            exposure: post.post_exposure,
            temperature: post.temperature,
            tint: post.tint,
            hue: post.hue_shift.to_radians(),
            ..default()
        },
        shadows: section,
        midtones: section,
        highlights: section,
    }
}

fn dress_post(camera: &mut EntityCommands, post: &PostProcess, undressed: &UndressedCamera) {
    if !post.enabled {
        undressed.restore_post(camera);
        return;
    }
    camera.insert((
        tonemapping(post.tonemapper),
        Exposure {
            ev100: post.exposure,
        },
        color_grading(post),
    ));
    if post.bloom_intensity > 0.0 {
        let mut bloom = Bloom::NATURAL;
        bloom.intensity = post.bloom_intensity;
        bloom.prefilter.threshold = post.bloom_threshold;
        camera.insert(bloom);
    } else {
        undressed.restore_bloom(camera);
    }
    if post.vignette_intensity > 0.0 {
        camera.insert(Vignette {
            intensity: post.vignette_intensity,
            smoothness: post.vignette_smoothness,
            ..default()
        });
    } else {
        restore(camera, undressed.vignette.clone());
    }
}

#[derive(Default)]
struct SkyState {
    mesh: Option<Handle<Mesh>>,
    material: Option<Handle<SkyMaterial>>,
    uniform: Option<SkyUniform>,
}

fn keep_the_sky(
    mut commands: Commands,
    scene: Res<SceneEnvironment>,
    wind: Res<SceneWind>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<SkyMaterial>>,
    mut state: Local<SkyState>,
    skies: Query<Entity, With<SceneSky>>,
) {
    let Some(sky) = scene
        .0
        .as_ref()
        .map(|env| &env.sky)
        .filter(|sky| sky.enabled)
    else {
        for entity in &skies {
            commands.entity(entity).despawn();
        }
        return;
    };

    let uniform = SkyUniform::new(sky, &wind);
    let material = match state.material.clone() {
        Some(handle) => {
            if state.uniform != Some(uniform)
                && let Some(mut material) = materials.get_mut(&handle)
            {
                material.sky = uniform;
            }
            handle
        }
        None => {
            let handle = materials.add(SkyMaterial { sky: uniform });
            state.material = Some(handle.clone());
            handle
        }
    };
    state.uniform = Some(uniform);

    if !skies.is_empty() {
        return;
    }
    let mesh = state
        .mesh
        .get_or_insert_with(|| meshes.add(screen_triangle()))
        .clone();
    commands.spawn((
        SceneSky,
        Mesh3d(mesh),
        MeshMaterial3d(material),
        Transform::IDENTITY,
        Visibility::default(),
        NoFrustumCulling,
        NotShadowCaster,
        NotShadowReceiver,
        NavmeshExclude,
        EditorHidden,
    ));
}

/// One triangle that covers the whole screen in clip space.
fn screen_triangle() -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![[-1.0, -1.0, 0.0], [3.0, -1.0, 0.0], [-1.0, 3.0, 0.0]],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use jackdaw_scene_types::AmbientMode;

    const SHADER_SOURCE: &str = include_str!("shaders/sky.wgsl");

    #[test]
    fn the_uniform_struct_matches_the_rust_field_order() {
        let start = SHADER_SOURCE
            .find("struct SkyUniform {")
            .expect("SkyUniform is declared");
        let body = &SHADER_SOURCE[start..];
        let body = &body[..body.find('}').expect("SkyUniform is closed")];
        let mut at = 0usize;
        for field in [
            "zenith: vec4<f32>",
            "horizon: vec4<f32>",
            "ground: vec4<f32>",
            "cloud_color: vec4<f32>",
            "cloud_drift: vec2<f32>",
            "horizon_softness: f32",
            "brightness: f32",
            "sun_cos: f32",
            "sun_intensity: f32",
            "cloud_coverage: f32",
            "cloud_scale: f32",
        ] {
            let found = body[at..]
                .find(field)
                .unwrap_or_else(|| panic!("`{field}` is missing or out of order"));
            at += found + field.len();
        }
        assert!(
            SHADER_SOURCE.contains(
                "@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> sky: SkyUniform;"
            )
        );
    }

    fn environment_app() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<Image>()
            .init_asset::<Mesh>()
            .init_asset::<SkyMaterial>();
        add_environment_systems(&mut app);
        app
    }

    fn spawn_camera(app: &mut App) -> Entity {
        app.world_mut()
            .spawn((Camera3d::default(), Tonemapping::BlenderFilmic))
            .id()
    }

    #[derive(Debug, PartialEq)]
    struct Dressing {
        tonemapping: Option<Tonemapping>,
        exposure: Option<f32>,
        fog: bool,
        ambient: bool,
        bloom: bool,
        vignette: bool,
        grading: bool,
        hdr: bool,
        undressed: bool,
    }

    fn dressing(app: &App, camera: Entity) -> Dressing {
        let camera = app.world().entity(camera);
        Dressing {
            tonemapping: camera.get::<Tonemapping>().copied(),
            exposure: camera.get::<Exposure>().map(|exposure| exposure.ev100),
            fog: camera.contains::<DistanceFog>(),
            ambient: camera.contains::<EnvironmentMapLight>(),
            bloom: camera.contains::<Bloom>(),
            vignette: camera.contains::<Vignette>(),
            grading: camera.contains::<ColorGrading>(),
            hdr: camera.contains::<Hdr>(),
            undressed: camera.contains::<UndressedCamera>(),
        }
    }

    fn hold(app: &mut App, environment: Environment) -> Entity {
        let root = app.world_mut().spawn(environment).id();
        app.update();
        root
    }

    #[test]
    fn an_unedited_environment_changes_nothing_on_the_cameras() {
        let mut app = environment_app();
        let camera = spawn_camera(&mut app);
        app.update();
        let before = dressing(&app, camera);

        hold(&mut app, Environment::default());

        assert_eq!(dressing(&app, camera), before);
        let skies = app
            .world_mut()
            .query_filtered::<(), With<SceneSky>>()
            .iter(app.world())
            .count();
        assert_eq!(skies, 0, "and draws no sky");
    }

    #[test]
    fn fog_mode_and_colour_reach_the_cameras_distance_fog() {
        let mut app = environment_app();
        let camera = spawn_camera(&mut app);
        let color = Color::srgb(0.16, 0.46, 0.59);
        hold(
            &mut app,
            Environment {
                fog: Fog {
                    mode: FogMode::ExponentialSquared,
                    color,
                    density: 0.003,
                    ..Fog::default()
                },
                ..Environment::default()
            },
        );

        let fog = app
            .world()
            .get::<DistanceFog>(camera)
            .expect("the camera is fogged");
        assert_eq!(fog.color, color);
        assert!(matches!(
            fog.falloff,
            FogFalloff::ExponentialSquared { density } if density == 0.003
        ));
    }

    #[test]
    fn post_settings_reach_the_camera_components() {
        let mut app = environment_app();
        let camera = spawn_camera(&mut app);
        hold(
            &mut app,
            Environment {
                post: PostProcess {
                    enabled: true,
                    tonemapper: Tonemapper::AcesFitted,
                    exposure: 11.0,
                    post_exposure: 0.7,
                    contrast: 1.2,
                    saturation: 1.1,
                    hue_shift: 5.0,
                    bloom_intensity: 0.2,
                    bloom_threshold: 0.9,
                    vignette_intensity: 0.25,
                    vignette_smoothness: 2.0,
                    ..PostProcess::default()
                },
                ..Environment::default()
            },
        );

        let world = app.world();
        assert_eq!(
            world.get::<Tonemapping>(camera),
            Some(&Tonemapping::AcesFitted)
        );
        assert_eq!(world.get::<Exposure>(camera).map(|e| e.ev100), Some(11.0));
        let grading = world.get::<ColorGrading>(camera).expect("graded");
        assert_eq!(grading.global.exposure, 0.7);
        assert!((grading.global.hue - 5.0_f32.to_radians()).abs() < 1e-6);
        assert_eq!(grading.midtones.contrast, 1.2);
        assert_eq!(grading.midtones.saturation, 1.1);
        let bloom = world.get::<Bloom>(camera).expect("blooming");
        assert_eq!((bloom.intensity, bloom.prefilter.threshold), (0.2, 0.9));
        let vignette = world.get::<Vignette>(camera).expect("vignetted");
        assert_eq!((vignette.intensity, vignette.smoothness), (0.25, 2.0));
    }

    #[test]
    fn removing_the_environment_gives_the_camera_back_what_it_had() {
        let mut app = environment_app();
        let camera = spawn_camera(&mut app);
        app.update();
        let before = dressing(&app, camera);

        let root = hold(
            &mut app,
            Environment {
                fog: Fog {
                    mode: FogMode::Linear,
                    ..Fog::default()
                },
                ambient: Ambient {
                    mode: AmbientMode::Trilight,
                    ..Ambient::default()
                },
                post: PostProcess {
                    enabled: true,
                    bloom_intensity: 0.3,
                    vignette_intensity: 0.5,
                    ..PostProcess::default()
                },
                ..Environment::default()
            },
        );
        assert_ne!(dressing(&app, camera), before);

        app.world_mut().entity_mut(root).remove::<Environment>();
        app.update();

        assert_eq!(dressing(&app, camera), before);
    }

    #[test]
    fn the_ambient_map_has_a_fixed_id_a_save_leaves_out() {
        let mut app = environment_app();
        let camera = spawn_camera(&mut app);
        hold(
            &mut app,
            Environment {
                ambient: Ambient {
                    mode: AmbientMode::Flat,
                    ..Ambient::default()
                },
                ..Environment::default()
            },
        );
        let light = app
            .world()
            .get::<EnvironmentMapLight>(camera)
            .expect("the camera takes the ambient light");
        assert!(matches!(light.diffuse_map, Handle::Uuid(..)));
        assert!(
            app.world()
                .resource::<Assets<Image>>()
                .contains(&light.diffuse_map)
        );
    }

    #[test]
    fn a_camera_that_does_not_draw_the_scene_is_left_alone() {
        let mut app = environment_app();
        let camera = app
            .world_mut()
            .spawn((Camera3d::default(), RenderLayers::layer(7)))
            .id();
        hold(
            &mut app,
            Environment {
                fog: Fog {
                    mode: FogMode::Exponential,
                    ..Fog::default()
                },
                ..Environment::default()
            },
        );
        assert!(!app.world().entity(camera).contains::<DistanceFog>());
    }

    #[test]
    fn an_enabled_sky_is_drawn_by_one_entity_and_goes_with_it() {
        let mut app = environment_app();
        let root = hold(
            &mut app,
            Environment {
                sky: Sky {
                    enabled: true,
                    ..Sky::default()
                },
                ..Environment::default()
            },
        );
        app.update();
        let count = |app: &mut App| {
            app.world_mut()
                .query_filtered::<(), With<SceneSky>>()
                .iter(app.world())
                .count()
        };
        assert_eq!(count(&mut app), 1);

        app.world_mut().entity_mut(root).despawn();
        app.update();
        assert_eq!(count(&mut app), 0);
    }

    #[test]
    fn trilight_ambient_lights_an_upward_face_with_the_sky_and_a_downward_face_with_the_ground() {
        let ambient = Ambient {
            mode: AmbientMode::Trilight,
            sky: Color::linear_rgb(0.62, 0.64, 0.66),
            equator: Color::linear_rgb(0.11, 0.12, 0.13),
            ground: Color::linear_rgb(0.05, 0.04, 0.03),
            ..Ambient::default()
        };
        let map = ambient_cubemap(&ambient);
        let texel = |face: u32, column: u32, row: u32| {
            map.get_color_at_3d(column, row, face)
                .expect("inside the cubemap")
                .to_linear()
        };
        let near = |a: LinearRgba, b: LinearRgba| {
            (a.red - b.red).abs() < 0.01
                && (a.green - b.green).abs() < 0.01
                && (a.blue - b.blue).abs() < 0.01
        };
        let middle = AMBIENT_FACE_SIZE / 2;

        assert!(near(texel(2, middle, middle), ambient.sky.to_linear()));
        assert!(near(texel(3, middle, middle), ambient.ground.to_linear()));
        for face in 0..6 {
            for (column, row) in [(0, 0), (middle, 3), (AMBIENT_FACE_SIZE - 1, middle)] {
                let up = cubemap_direction(face, column, row).y;
                assert!(
                    near(texel(face, column, row), ambient.color_facing(up)),
                    "face {face} texel ({column}, {row}) mirrors the gradient",
                );
            }
        }
    }

    #[test]
    fn the_cubemap_faces_look_the_way_their_names_say() {
        let middle = AMBIENT_FACE_SIZE / 2;
        for (face, axis) in [
            (0, Vec3::X),
            (1, Vec3::NEG_X),
            (2, Vec3::Y),
            (3, Vec3::NEG_Y),
            (4, Vec3::Z),
            (5, Vec3::NEG_Z),
        ] {
            assert!(cubemap_direction(face, middle, middle).dot(axis) > 0.99);
        }
        assert!(
            cubemap_direction(0, middle, 0).y > 0.0,
            "the top row of a side face looks up",
        );
    }
}
