use bevy::prelude::*;
use jackdaw_scene_types::Scene2dRoot;

/// The `Camera2d` the runtime spawns at the world origin when a 2D scene
/// loads into a world with no camera, so its sprites show. It goes once the
/// game adds a camera of its own or the last 2D scene leaves, and the editor
/// never sees it.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct FallbackCamera2d;

#[cfg(feature = "pie")]
type NotCaptureCamera = Without<crate::pie_frames::FrameCaptureCamera>;
#[cfg(not(feature = "pie"))]
type NotCaptureCamera = ();

/// Spawn the fallback camera when one of the `spawned` scene entities is a
/// [`Scene2dRoot`] and the world holds no camera, a fallback included.
pub(crate) fn spawn_for_2d_scene(world: &mut World, spawned: &[Entity]) {
    let is_2d = spawned
        .iter()
        .any(|&entity| world.get::<Scene2dRoot>(entity).is_some());
    if !is_2d {
        return;
    }
    let has_camera = world
        .query_filtered::<(), (With<Camera>, NotCaptureCamera)>()
        .iter(world)
        .next()
        .is_some();
    if !has_camera {
        world.spawn((
            Name::new("Camera2d"),
            Camera2d,
            Transform::IDENTITY,
            FallbackCamera2d,
        ));
    }
}

pub(crate) fn yield_to_game_camera(
    add: On<Add<Camera>>,
    game_cameras: Query<(), (NotCaptureCamera, Without<FallbackCamera2d>)>,
    fallbacks: Query<Entity, With<FallbackCamera2d>>,
    mut commands: Commands,
) -> Result {
    if game_cameras.contains(add.entity) {
        for fallback in &fallbacks {
            commands.entity(fallback).try_despawn();
        }
    }
    Ok(())
}

pub(crate) fn leave_with_last_2d_scene(
    remove: On<Remove<Scene2dRoot>>,
    scenes: Query<Entity, With<Scene2dRoot>>,
    fallbacks: Query<Entity, With<FallbackCamera2d>>,
    mut commands: Commands,
) -> Result {
    if scenes.iter().all(|scene| scene == remove.entity) {
        for fallback in &fallbacks {
            commands.entity(fallback).try_despawn();
        }
    }
    Ok(())
}
