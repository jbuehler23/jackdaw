use bevy::{
    camera::{RenderTarget, visibility::RenderLayers},
    prelude::*,
    render::render_resource::TextureFormat,
};

use crate::material_definition::MaterialDefinitionCache;

#[derive(Component)]
pub struct PreviewSphere;

#[derive(Component)]
pub struct PreviewCamera;

#[derive(Resource)]
pub struct MaterialPreviewState {
    pub active_material: Option<String>,
    pub orbit_yaw: f32,
    pub orbit_pitch: f32,
    pub zoom_distance: f32,
    pub preview_image: Handle<Image>,
}

impl Default for MaterialPreviewState {
    fn default() -> Self {
        Self {
            active_material: None,
            orbit_yaw: 0.5,
            orbit_pitch: -0.3,
            zoom_distance: 3.0,
            preview_image: Handle::default(),
        }
    }
}

const PREVIEW_LAYER: usize = 1;

pub fn setup_material_preview_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut preview_state: ResMut<MaterialPreviewState>,
) {
    let preview_layer = RenderLayers::layer(PREVIEW_LAYER);

    let sphere = meshes.add(Sphere::new(1.0).mesh().ico(5).unwrap());
    let mat = materials.add(StandardMaterial::default());

    commands.spawn((
        PreviewSphere,
        Mesh3d(sphere),
        MeshMaterial3d(mat),
        Transform::default(),
        Visibility::Inherited,
        preview_layer.clone(),
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 5000.0,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.7, 0.5, 0.0)),
        Visibility::Inherited,
        preview_layer.clone(),
    ));

    let preview_image = Image::new_target_texture(
        256,
        256,
        TextureFormat::Rgba8Unorm,
        Some(TextureFormat::Rgba8UnormSrgb),
    );
    let preview_image_handle = images.add(preview_image);
    preview_state.preview_image = preview_image_handle.clone();

    commands.spawn((
        PreviewCamera,
        Camera3d::default(),
        Camera {
            order: -1,
            is_active: false,
            clear_color: ClearColorConfig::Custom(Color::srgba(0.15, 0.15, 0.15, 1.0)),
            ..default()
        },
        RenderTarget::Image(preview_image_handle.into()),
        Transform::from_translation(Vec3::new(0.0, 0.0, 3.0)).looking_at(Vec3::ZERO, Vec3::Y),
        preview_layer,
    ));
}

pub fn update_preview_camera_transform(
    preview_state: Res<MaterialPreviewState>,
    mut camera_q: Query<&mut Transform, With<PreviewCamera>>,
) {
    if !preview_state.is_changed() || preview_state.active_material.is_none() {
        return;
    }

    let yaw = preview_state.orbit_yaw;
    let pitch = preview_state.orbit_pitch.clamp(-1.4, 1.4);
    let dist = preview_state.zoom_distance;

    let x = dist * pitch.cos() * yaw.sin();
    let y = dist * pitch.sin();
    let z = dist * pitch.cos() * yaw.cos();

    if let Ok(mut transform) = camera_q.single_mut() {
        *transform =
            Transform::from_translation(Vec3::new(x, y, z)).looking_at(Vec3::ZERO, Vec3::Y);
    }
}

pub fn update_active_preview_material(
    preview_state: Res<MaterialPreviewState>,
    mat_cache: Res<MaterialDefinitionCache>,
    mut sphere_q: Query<&mut MeshMaterial3d<StandardMaterial>, With<PreviewSphere>>,
    mut camera_q: Query<&mut Camera, With<PreviewCamera>>,
) {
    if !preview_state.is_changed() {
        return;
    }

    match &preview_state.active_material {
        Some(name) => {
            if let Some(entry) = mat_cache.entries.get(name) {
                if let Ok(mut sphere_mat) = sphere_q.single_mut() {
                    sphere_mat.0 = entry.material.clone();
                }
            }
            if let Ok(mut cam) = camera_q.single_mut() {
                cam.is_active = true;
            }
        }
        None => {
            if let Ok(mut cam) = camera_q.single_mut() {
                cam.is_active = false;
            }
        }
    }
}
