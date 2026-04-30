//! `GameViewportCamera` and the camera mirror that drives it.
//!
//! While `PlayState::Playing`, the editor's authoring `MainViewportCamera`
//! goes inactive and `GameViewportCamera` (managed by the editor, not
//! the user) takes its place rendering to the same viewport image.
//! `mirror_user_camera` copies the user's `Camera3d` settings from
//! the `SubApp` world onto `GameViewportCamera` so the user's camera
//! configuration drives the view.

use bevy::camera::{Camera, Projection, RenderTarget};
use bevy::ecs::query::{With, Without};
use bevy::prelude::*;
use bevy::render::view::Hdr;

use crate::viewport::{MainViewportCamera, ViewportRenderImage};

/// Editor-owned camera that renders the user's game while
/// `PlayState::Playing`. Spawned once when entering the Editor state;
/// activated/deactivated by `swap_active_camera_on_play_*` on `PlayState`
/// transitions.
#[derive(Component)]
pub struct GameViewportCamera;

/// Spawns the `GameViewportCamera` once when entering the Editor state.
/// Initially inactive so the authoring `MainViewportCamera` keeps
/// rendering. `swap_active_camera_on_play_*` flips activation on Play/Stop.
///
/// Targets the SAME render-target image as `MainViewportCamera` so
/// the existing `ViewportNode` displays whichever camera is active.
pub fn setup_game_viewport_camera(
    mut commands: Commands,
    viewport_image: Res<ViewportRenderImage>,
) {
    commands.spawn((
        GameViewportCamera,
        crate::EditorEntity,
        Camera3d::default(),
        Camera {
            is_active: false,
            order: -1,
            ..default()
        },
        RenderTarget::Image(viewport_image.0.clone().into()),
        Transform::from_xyz(0.0, 4.0, 8.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

/// Toggles `is_active` on `MainViewportCamera` and `GameViewportCamera`
/// on `PlayState::Playing` enter. Pause does NOT touch camera state;
/// last frame stays visible while paused.
pub fn swap_active_camera_on_play_enter(
    mut main_q: Query<&mut Camera, (With<MainViewportCamera>, Without<GameViewportCamera>)>,
    mut game_q: Query<&mut Camera, (With<GameViewportCamera>, Without<MainViewportCamera>)>,
) {
    if let Ok(mut main) = main_q.single_mut() {
        main.is_active = false;
    }
    if let Ok(mut game) = game_q.single_mut() {
        game.is_active = true;
    }
}

pub fn swap_active_camera_on_stop_enter(
    mut main_q: Query<&mut Camera, (With<MainViewportCamera>, Without<GameViewportCamera>)>,
    mut game_q: Query<&mut Camera, (With<GameViewportCamera>, Without<MainViewportCamera>)>,
) {
    if let Ok(mut main) = main_q.single_mut() {
        main.is_active = true;
    }
    if let Ok(mut game) = game_q.single_mut() {
        game.is_active = false;
    }
}

/// Walks the `SubApp` world for the user's primary `Camera3d` and
/// copies its `Transform`, `Projection`, and `Camera` settings onto
/// the editor's `GameViewportCamera`.
///
/// Primary camera = first `Camera3d` whose `RenderTarget` is the
/// default (window) or absent (defaults to window). Cameras with
/// explicit image render-targets are the user's render-to-texture
/// cameras (e.g. minimap) and aren't the play view.
///
/// In Bevy 0.18 `RenderTarget` is a separate component and `Hdr` is a
/// marker component, so the camera config travels via several pieces.
///
/// Multi-camera support is out of scope for v1; first-found wins.
pub fn mirror_user_camera(sub_world: &mut World, editor_world: &mut World) {
    let primary = {
        let mut q = sub_world.query::<(
            Entity,
            &Camera,
            &Transform,
            &Projection,
            Option<&RenderTarget>,
        )>();
        q.iter(sub_world)
            .find(|(_, _, _, _, target)| {
                target.is_none_or(|t| matches!(t, RenderTarget::Window(_)))
            })
            .map(|(e, c, t, p, _)| (e, c.clone(), *t, p.clone()))
    };

    let Some((sub_entity, sub_camera, sub_transform, sub_projection)) = primary else {
        return;
    };
    let sub_has_hdr = sub_world.get::<Hdr>(sub_entity).is_some();

    let mut q_editor = editor_world
        .query_filtered::<(Entity, &mut Transform, &mut Projection), With<GameViewportCamera>>();
    let Some((entity, mut editor_transform, mut editor_projection)) =
        q_editor.iter_mut(editor_world).next()
    else {
        return;
    };
    *editor_transform = sub_transform;
    *editor_projection = sub_projection;
    if let Some(mut editor_camera) = editor_world.get_mut::<Camera>(entity) {
        editor_camera.clear_color = sub_camera.clear_color;
        editor_camera.msaa_writeback = sub_camera.msaa_writeback;
    }
    let editor_has_hdr = editor_world.get::<Hdr>(entity).is_some();
    if sub_has_hdr && !editor_has_hdr {
        editor_world.entity_mut(entity).insert(Hdr);
    } else if !sub_has_hdr && editor_has_hdr {
        editor_world.entity_mut(entity).remove::<Hdr>();
    }
}
