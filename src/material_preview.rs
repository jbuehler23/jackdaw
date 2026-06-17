use bevy::{
    asset::{embedded_asset, load_embedded_asset},
    camera::{RenderTarget, visibility::RenderLayers},
    ecs::error::BevyError,
    prelude::*,
    render::render_resource::TextureFormat,
};

use crate::default_style;

pub(super) struct MaterialPreviewPlugin;

impl Plugin for MaterialPreviewPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(
            app,
            "../assets/environment_maps/voortrekker_interior_1k_diffuse.ktx2"
        );
        embedded_asset!(
            app,
            "../assets/environment_maps/voortrekker_interior_1k_specular.ktx2"
        );
        app.add_systems(
            OnEnter(crate::AppState::Editor),
            setup_material_preview_scene,
        )
        .add_systems(
            Update,
            (
                update_preview_camera_transform,
                update_active_preview_material,
                update_preview_shape,
            )
                .run_if(in_state(crate::AppState::Editor)),
        );
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PreviewShape {
    #[default]
    Sphere,
    Cube,
    Plane,
}

impl PreviewShape {
    pub const ALL: [PreviewShape; 3] = [
        PreviewShape::Sphere,
        PreviewShape::Cube,
        PreviewShape::Plane,
    ];
}

#[derive(Resource)]
pub struct PreviewShapeMeshes {
    pub sphere: Handle<Mesh>,
    pub cube: Handle<Mesh>,
    pub plane: Handle<Mesh>,
}

impl PreviewShapeMeshes {
    pub fn get(&self, shape: PreviewShape) -> Handle<Mesh> {
        match shape {
            PreviewShape::Sphere => self.sphere.clone(),
            PreviewShape::Cube => self.cube.clone(),
            PreviewShape::Plane => self.plane.clone(),
        }
    }
}

#[derive(Component)]
pub struct PreviewSphere;

#[derive(Component)]
pub struct PreviewCamera;

#[derive(Resource)]
pub struct MaterialPreviewState {
    pub active_material: Option<Handle<StandardMaterial>>,
    pub orbit_yaw: f32,
    pub orbit_pitch: f32,
    pub zoom_distance: f32,
    pub preview_image: Handle<Image>,
    pub preview_shape: PreviewShape,
}

impl Default for MaterialPreviewState {
    fn default() -> Self {
        Self {
            active_material: None,
            orbit_yaw: 0.5,
            orbit_pitch: -0.3,
            zoom_distance: 3.0,
            preview_image: Handle::default(),
            preview_shape: PreviewShape::default(),
        }
    }
}

const PREVIEW_LAYER: usize = 1;

fn setup_material_preview_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut preview_state: ResMut<MaterialPreviewState>,
    assets: Res<AssetServer>,
) {
    let preview_layer = RenderLayers::layer(PREVIEW_LAYER);

    let sphere = meshes.add(
        Sphere::new(1.0)
            .mesh()
            .ico(5)
            .unwrap()
            .with_generated_tangents()
            .unwrap(),
    );
    let cube = meshes.add(
        Cuboid::new(1.6, 1.6, 1.6)
            .mesh()
            .build()
            .with_generated_tangents()
            .unwrap(),
    );
    let plane = meshes.add(
        Plane3d::default()
            .mesh()
            .size(2.0, 2.0)
            .build()
            .with_generated_tangents()
            .unwrap(),
    );
    commands.insert_resource(PreviewShapeMeshes {
        sphere: sphere.clone(),
        cube,
        plane,
    });
    let mat = materials.add(StandardMaterial::default());

    commands.spawn((
        PreviewSphere,
        crate::EditorEntity,
        Mesh3d(sphere),
        MeshMaterial3d(mat),
        Transform::default(),
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
        crate::EditorEntity,
        Camera3d::default(),
        Camera {
            order: -1,
            is_active: false,
            clear_color: ClearColorConfig::Custom(default_style::MATERIAL_PREVIEW_BG),
            ..default()
        },
        // It may seem like this bit of code is duplicated in viewport.rs, but that is incidental
        // Since the user may have any number of material preview and viewport windows open, we cannot have a global resource for the current env map light
        // Instead, it should be per view. Ideally we should however have a global resource telling us about the *available* env map light textures!
        EnvironmentMapLight {
            diffuse_map: load_embedded_asset!(
                &*assets,
                "../assets/environment_maps/voortrekker_interior_1k_diffuse.ktx2"
            ),
            specular_map: load_embedded_asset!(
                &*assets,
                "../assets/environment_maps/voortrekker_interior_1k_specular.ktx2"
            ),
            intensity: 2000.0,
            ..default()
        },
        RenderTarget::Image(preview_image_handle.into()),
        Transform::from_translation(Vec3::new(0.0, 0.0, 3.0)).looking_at(Vec3::ZERO, Vec3::Y),
        preview_layer,
    ));
}

fn update_preview_camera_transform(
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

fn update_active_preview_material(
    preview_state: Res<MaterialPreviewState>,
    mut sphere_q: Query<&mut MeshMaterial3d<StandardMaterial>, With<PreviewSphere>>,
    mut camera_q: Query<&mut Camera, With<PreviewCamera>>,
) {
    if !preview_state.is_changed() {
        return;
    }

    match &preview_state.active_material {
        Some(handle) if *handle != Handle::default() => {
            if let Ok(mut sphere_mat) = sphere_q.single_mut() {
                sphere_mat.0 = handle.clone();
            }
            if let Ok(mut cam) = camera_q.single_mut() {
                cam.is_active = true;
            }
        }
        _ => {
            if let Ok(mut cam) = camera_q.single_mut() {
                cam.is_active = false;
            }
        }
    }
}

fn update_preview_shape(
    preview_state: Res<MaterialPreviewState>,
    shapes: Option<Res<PreviewShapeMeshes>>,
    mut sphere_q: Query<&mut Mesh3d, With<PreviewSphere>>,
) -> Result<(), BevyError> {
    if !preview_state.is_changed() {
        return Ok(());
    }
    let Some(shapes) = shapes else {
        return Ok(());
    };
    if let Ok(mut mesh) = sphere_q.single_mut() {
        let wanted = shapes.get(preview_state.preview_shape);
        if mesh.0 != wanted {
            mesh.0 = wanted;
        }
    }
    Ok(())
}

#[cfg(test)]
mod preview_shape_tests {
    use super::PreviewShape;

    #[test]
    fn shapes_cycle_in_order() {
        assert_eq!(
            PreviewShape::ALL,
            [
                PreviewShape::Sphere,
                PreviewShape::Cube,
                PreviewShape::Plane
            ]
        );
        assert_eq!(PreviewShape::default(), PreviewShape::Sphere);
    }
}
