//! Render-to-texture preview of a scene-authored camera.
//!
//! Scene-document cameras never render in the editor (see
//! `scene_io::deactivate_document_cameras`), so selecting one gives no
//! feedback about what it frames. This module keeps an editor-owned
//! mirror camera that copies the selected camera's world transform and
//! projection each frame and renders the scene into a small texture the
//! inspector shows inside the Camera3d component card. The document
//! entity itself is never touched.

use bevy::camera::RenderTarget;
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;

use crate::selection::Selection;

/// Preview texture size. 16:9 to match the common game aspect; small
/// because the inspector shows it at card width.
const PREVIEW_WIDTH: u32 = 320;
const PREVIEW_HEIGHT: u32 = 180;

pub struct CameraPreviewPlugin;

impl Plugin for CameraPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraPreviewState>()
            .add_systems(OnEnter(crate::AppState::Editor), setup_camera_preview)
            .add_systems(
                Update,
                mirror_selected_camera.run_if(in_state(crate::AppState::Editor)),
            )
            .add_observer(on_preview_strip_added);
    }
}

#[derive(Resource, Default)]
pub(crate) struct CameraPreviewState {
    /// The render-to-texture image the inspector card displays.
    pub image: Handle<Image>,
}

/// The inspector-card node displaying the preview texture.
#[derive(Component)]
pub(crate) struct CameraPreviewStrip;

/// Spawns the preview strip at the top of a Camera3d component card.
/// The image handle lives in [`CameraPreviewState`], which a spawn-time
/// observer reads; callers only need `Commands`.
pub(crate) fn spawn_camera_preview_strip(commands: &mut Commands, parent: Entity) {
    commands.spawn((
        CameraPreviewStrip,
        Node {
            width: Val::Percent(100.0),
            aspect_ratio: Some(PREVIEW_WIDTH as f32 / PREVIEW_HEIGHT as f32),
            ..default()
        },
        ChildOf(parent),
    ));
}

fn on_preview_strip_added(
    trigger: On<Add, CameraPreviewStrip>,
    state: Res<CameraPreviewState>,
    mut commands: Commands,
) {
    commands
        .entity(trigger.event_target())
        .insert(ImageNode::new(state.image.clone()));
}

/// Marker for the editor-owned mirror camera.
#[derive(Component)]
struct CameraPreviewCamera;

fn setup_camera_preview(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut state: ResMut<CameraPreviewState>,
) {
    let image = images.add(Image::new_target_texture(
        PREVIEW_WIDTH,
        PREVIEW_HEIGHT,
        TextureFormat::Rgba8Unorm,
        Some(TextureFormat::Rgba8UnormSrgb),
    ));
    state.image = image.clone();

    commands.spawn((
        CameraPreviewCamera,
        crate::EditorEntity,
        Camera3d::default(),
        Camera {
            // Below the material preview and viewport cameras; renders
            // only while a camera entity is selected.
            order: -3,
            is_active: false,
            ..default()
        },
        RenderTarget::Image(image.into()),
        // Scene content only: the editor's grid, gizmos, and handles
        // live on per-viewport layers this camera does not carry.
        RenderLayers::layer(0),
        Transform::default(),
    ));
}

/// Follows the primary selection: while it is a scene camera, the mirror
/// copies its world transform and projection and renders; otherwise the
/// mirror goes inactive and costs nothing.
fn mirror_selected_camera(
    selection: Res<Selection>,
    sources: Query<
        (&GlobalTransform, Option<&Projection>),
        (
            With<Camera3d>,
            Without<crate::EditorEntity>,
            Without<CameraPreviewCamera>,
        ),
    >,
    mut mirror: Query<(&mut Camera, &mut Transform, &mut Projection), With<CameraPreviewCamera>>,
) {
    let Ok((mut camera, mut transform, mut projection)) = mirror.single_mut() else {
        return;
    };

    let source = selection.primary().and_then(|e| sources.get(e).ok());
    let Some((source_transform, source_projection)) = source else {
        if camera.is_active {
            camera.is_active = false;
        }
        return;
    };

    if !camera.is_active {
        camera.is_active = true;
    }
    *transform = source_transform.compute_transform();
    *projection = source_projection.cloned().unwrap_or_default();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world_with_mirror() -> (World, Entity) {
        let mut world = World::new();
        let mirror = world
            .spawn((
                CameraPreviewCamera,
                Camera {
                    is_active: false,
                    ..default()
                },
                Transform::default(),
                Projection::default(),
            ))
            .id();
        world.insert_resource(Selection::default());
        (world, mirror)
    }

    #[test]
    fn mirror_follows_a_selected_scene_camera_and_sleeps_otherwise() {
        let (mut world, mirror) = world_with_mirror();
        let at = Transform::from_xyz(1.0, 2.0, 3.0);
        let scene_camera = world
            .spawn((Camera3d::default(), GlobalTransform::from(at)))
            .id();

        world.resource_mut::<Selection>().entities = vec![scene_camera];
        world
            .run_system_cached(mirror_selected_camera)
            .expect("run mirror system");
        assert!(world.get::<Camera>(mirror).unwrap().is_active);
        assert_eq!(
            world.get::<Transform>(mirror).unwrap().translation,
            at.translation
        );

        world.resource_mut::<Selection>().entities.clear();
        world
            .run_system_cached(mirror_selected_camera)
            .expect("run mirror system");
        assert!(!world.get::<Camera>(mirror).unwrap().is_active);
    }

    #[test]
    fn editor_cameras_do_not_feed_the_mirror() {
        let (mut world, mirror) = world_with_mirror();
        let editor_camera = world
            .spawn((
                Camera3d::default(),
                GlobalTransform::default(),
                crate::EditorEntity,
            ))
            .id();
        world.resource_mut::<Selection>().entities = vec![editor_camera];
        world
            .run_system_cached(mirror_selected_camera)
            .expect("run mirror system");
        assert!(!world.get::<Camera>(mirror).unwrap().is_active);
    }
}
