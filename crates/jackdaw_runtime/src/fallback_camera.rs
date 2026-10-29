use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::window::WindowRef;
use jackdaw_scene_types::Scene2dRoot;

/// The `Camera2d` the runtime spawns at the world origin when a 2D scene
/// loads into a world with no camera, so its sprites show. It goes once the
/// game adds a camera of its own or the last 2D scene leaves, and the editor
/// never sees it.
#[derive(Component, Clone, Copy, Debug, Default)]
#[require(crate::RuntimeOnly)]
pub struct FallbackCamera2d;

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
        .query_filtered::<(), (
            With<Camera>,
            Or<(Without<crate::RuntimeOnly>, With<FallbackCamera2d>)>,
        )>()
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
    game_cameras: Query<(), Without<crate::RuntimeOnly>>,
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

/// The window and order an active camera draws at, when it draws into a
/// window: `None` for the primary window.
fn window_slot(world: &World, camera: Entity) -> Option<(Option<Entity>, isize)> {
    let state = world
        .get::<Camera>(camera)
        .filter(|state| state.is_active)?;
    let window = match world.get::<RenderTarget>(camera) {
        None | Some(RenderTarget::Window(WindowRef::Primary)) => None,
        Some(RenderTarget::Window(WindowRef::Entity(window))) => Some(*window),
        Some(_) => return None,
    };
    Some((window, state.order))
}

/// Leave at most one active camera per window and order once a scene spawns:
/// a camera already drawing keeps its slot, then the first authored one in
/// scene order takes it, and the other authored ones switch off.
pub(crate) fn one_camera_per_window_order(world: &mut World, spawned: &[Entity]) {
    let drawing: Vec<Entity> = world
        .query_filtered::<Entity, (With<Camera>, Without<crate::RuntimeOnly>)>()
        .iter(world)
        .filter(|camera| !spawned.contains(camera))
        .collect();
    let mut taken: Vec<_> = drawing
        .into_iter()
        .filter_map(|camera| window_slot(world, camera))
        .collect();
    for &camera in spawned {
        let Some(slot) = window_slot(world, camera) else {
            continue;
        };
        if taken.contains(&slot) {
            if let Some(mut state) = world.get_mut::<Camera>(camera) {
                state.is_active = false;
            }
        } else {
            taken.push(slot);
        }
    }
}
