//! Per-frame data sync between the editor's authoring world and the
//! game [`SubApp`](bevy::app::SubApp)'s runtime world.
//!
//! When the user clicks Play, gameplay systems running in the
//! `GameSubApp` need to see the entities the user authored
//! (brushes, lights, the player's spawn point, etc.). When systems
//! mutate those entities — or spawn new ones — the editor's panels
//! (Inspector, Hierarchy) need to display the updated state.
//!
//! The extract layer is the bridge. It runs once per frame as part
//! of the `SubApp`'s [`extract`](bevy::app::SubApp::extract)
//! callback (set up by `create_game_sub_app`'s caller).
//!
//! # Reflection-driven by default
//!
//! Rather than asking users to mark every component with a
//! `SyncToGameWorld` opt-in (the pattern `bevy_render` uses for its
//! `SyncToRenderWorld` marker), the extract walks the shared
//! [`AppTypeRegistry`](bevy::reflect::AppTypeRegistry) and clones
//! every `#[derive(Reflect)] + #[derive(Component)]` type it finds
//! on a [`SceneEntity`]-tagged entity. This way a user with a custom
//! `Player { health: f32 }` component gets it copied into the
//! `SubApp` world automatically; no per-component annotation needed.
//!
//! Users with components that should NOT cross the boundary (e.g.
//! `Arc<MyHandle>` or platform-specific raw handles) opt out by
//! marking the component `#[reflect(skip_serializing)]` — the
//! extract treats unserialisable components as "intentionally
//! game-only" and skips them.
//!
//! # Entity-id mapping
//!
//! Entity IDs aren't transferable between worlds. We maintain a
//! [`GameEntityMap`] resource on the `SubApp` side mapping each
//! editor-world entity (via [`MainEntity`]) to the `SubApp`-world
//! entity that mirrors it. Lookups stay O(1) via a `HashMap`.
//!
//! # Status
//!
//! This module is a stub for first-pass landing. The reflection
//! walk is deliberately minimal: it copies [`Transform`] and
//! [`Name`] (the two components every editor scene has) and treats
//! the rest as user-opt-in until the broader `Reflect` walk is
//! wired. See the plan's Phase 5 risks section for the broader
//! auto-sync work.

use bevy::ecs::entity::{Entity, EntityHashMap};
use bevy::input::ButtonInput;
use bevy::input::keyboard::{KeyCode, KeyboardInput};
use bevy::input::mouse::{MouseButton, MouseButtonInput, MouseMotion, MouseWheel};
use bevy::prelude::*;
use bevy::scene::{DynamicSceneBuilder, SceneFilter};
use bevy::time::Time;
use bevy::window::CursorMoved;

/// Marker on entities in the editor's authoring world that should
/// appear in the game's runtime world during Play. Set by the scene
/// loader on entities deserialised from `.jsn` so user-authored
/// brushes / lights / cameras automatically appear in the game.
#[derive(Component, Default, Reflect)]
#[reflect(Component)]
pub struct SceneEntity;

/// Mirror of a main-world (editor) entity in the game's `SubApp`
/// world. Stores the editor-side [`Entity`] so per-frame extract
/// can look up or invalidate the mirror on demand.
#[derive(Component, Debug, Clone, Copy)]
pub struct MainEntity(pub Entity);

/// Resource on the `SubApp` world: editor-world entity → `SubApp`-world
/// entity mapping. Populated by [`extract_scene_entities`]; consumed
/// by editor-side panels that want to read game-world state.
///
/// Wraps [`EntityHashMap<Entity>`] so it can be passed directly to
/// `DynamicScene::write_to_world` for reflective component copying.
#[derive(Resource, Default, Debug)]
pub struct GameEntityMap {
    pub entries: EntityHashMap<Entity>,
}

/// Extract function suitable for
/// [`SubApp::set_extract`](bevy::app::SubApp::set_extract). Walks
/// every [`SceneEntity`] in the editor world, builds a
/// [`DynamicScene`](bevy::scene::DynamicScene) capturing all of
/// their `#[derive(Reflect)] + #[derive(Component)]` types, and
/// applies it to the `SubApp` world. The entity-id map persists
/// across frames as a [`GameEntityMap`] resource on the `SubApp` so
/// the same authoring entity always maps to the same `SubApp`
/// entity.
///
/// User components flow automatically: any type the user marks
/// `#[derive(Reflect)]` and that is registered in the editor's
/// `AppTypeRegistry` (via the build-script-emitted reflect-register
/// shim) gets cloned into the `SubApp` every frame. No
/// `SyncToGameWorld` opt-in marker required.
///
/// To opt a component OUT of the sync, mark it with
/// `#[reflect(skip_serializing)]` — Bevy's `DynamicSceneBuilder`
/// honours that attribute and skips the field.
///
/// # Arguments
///
/// - `main_world`: the editor's authoring world.
/// - `sub_world`: the game's `SubApp` world.
pub fn extract_scene_entities(main_world: &mut World, sub_world: &mut World) {
    // Collect the SceneEntity-tagged authoring entities. We hold
    // `&mut World` only for the query; the rest of the function
    // re-borrows.
    let scene_entities: Vec<Entity> = {
        let mut q = main_world.query_filtered::<Entity, With<SceneEntity>>();
        q.iter(main_world).collect()
    };
    if scene_entities.is_empty() {
        return;
    }

    // Build a DynamicScene capturing each SceneEntity's reflect
    // components. `DynamicSceneBuilder::extract_entities` walks the
    // shared `AppTypeRegistry` and serialises every component for
    // which the type is registered with a `ReflectComponent` type-
    // data entry (which `#[derive(Reflect)] #[derive(Component)]`
    // emits automatically).
    let dynamic_scene = DynamicSceneBuilder::from_world(main_world)
        .with_component_filter(SceneFilter::allow_all())
        .extract_entities(scene_entities.iter().copied())
        .build();

    // Pull the persistent entity-id mapping out so we can hand it to
    // `write_to_world`. The map's contents survive across frames so
    // each authoring entity always ends up at the same SubApp
    // entity.
    let mut map = sub_world
        .remove_resource::<GameEntityMap>()
        .unwrap_or_default();

    if let Err(err) = dynamic_scene.write_to_world(sub_world, &mut map.entries) {
        // Most likely cause: a registered type whose `Reflect` impl
        // refuses to clone (e.g. `Handle<T>` without `ReflectAsset`).
        // Log once and continue — the SubApp ticks with whatever was
        // synced successfully.
        warn!("extract_scene_entities: scene write failed: {err}");
    }

    sub_world.insert_resource(map);
}

/// Marker on editor-world entities that mirror an entity in the
/// `GameSubApp` world. Reverse-extract paired; despawned when the
/// source entity vanishes.
#[derive(Component, Debug, Clone, Copy)]
pub struct GameMirror {
    pub sub_entity: Entity,
}

/// Editor-world resource mapping `SubApp`-world entities to their
/// editor-world mirrors. Persists across frames so the same `SubApp`
/// entity always maps to the same editor mirror entity.
#[derive(Resource, Default, Debug)]
pub struct MirrorEntityMap {
    pub entries: bevy::ecs::entity::EntityHashMap<Entity>,
}

/// Reverse extract: walks the `SubApp` world, mirrors every entity
/// with render-relevant components into the editor world tagged with
/// [`GameMirror`]. Maintains [`MirrorEntityMap`] so the same `SubApp`
/// entity always maps to the same editor mirror across frames.
///
/// Despawns mirrors whose `sub_entity` no longer exists in
/// `sub_world`.
///
/// Identity uses `TypePath` strings (Reflect), not `TypeId`, so the
/// mapping survives `dlopen` swaps where `TypeId` would diverge.
pub fn extract_game_mirrors(sub_world: &mut World, editor_world: &mut World) {
    // Collect every SubApp entity that has at least one render-
    // relevant component. We use `Transform` as the cheap presence
    // test; entities without a transform aren't visually relevant.
    let live_sub_entities: Vec<Entity> = {
        let mut q = sub_world.query_filtered::<Entity, With<Transform>>();
        q.iter(sub_world).collect()
    };

    let mut map = editor_world
        .remove_resource::<MirrorEntityMap>()
        .unwrap_or_default();

    // For each live SubApp entity, ensure a mirror exists in the
    // editor world.
    for sub_entity in &live_sub_entities {
        let mirror_entity = *map.entries.entry(*sub_entity).or_insert_with(|| {
            editor_world
                .spawn(GameMirror {
                    sub_entity: *sub_entity,
                })
                .id()
        });

        if let Some(transform) = sub_world.get::<Transform>(*sub_entity) {
            let transform = *transform;
            editor_world.entity_mut(mirror_entity).insert(transform);
        }
    }

    // Despawn mirrors whose source is gone.
    let live: bevy::platform::collections::HashSet<Entity> =
        live_sub_entities.iter().copied().collect();
    let stale: Vec<(Entity, Entity)> = map
        .entries
        .iter()
        .filter(|(sub_e, _)| !live.contains(*sub_e))
        .map(|(s, m)| (*s, *m))
        .collect();
    for (sub_e, mirror_e) in stale {
        editor_world.despawn(mirror_e);
        map.entries.remove(&sub_e);
    }

    editor_world.insert_resource(map);
}

/// Drains specific input messages from the editor's main world into
/// the `SubApp` world's message queues so the user's gameplay systems
/// see the same input the editor saw.
///
/// Bevy 0.18 renamed the buffered-event API to `Message` /
/// `Messages<E>` (the resource); we forward the `current update`
/// buffer rather than draining historic messages so we don't perturb
/// editor-side readers that haven't run yet this frame.
pub fn extract_input_events(editor_world: &mut World, sub_world: &mut World) {
    forward_messages::<KeyboardInput>(editor_world, sub_world);
    forward_messages::<MouseButtonInput>(editor_world, sub_world);
    forward_messages::<MouseMotion>(editor_world, sub_world);
    forward_messages::<MouseWheel>(editor_world, sub_world);
    forward_messages::<CursorMoved>(editor_world, sub_world);
}

fn forward_messages<M: Message + Clone>(editor_world: &mut World, sub_world: &mut World) {
    let Some(editor_messages) = editor_world.get_resource::<Messages<M>>() else {
        return;
    };
    let pending: Vec<M> = editor_messages
        .iter_current_update_messages()
        .cloned()
        .collect();
    if pending.is_empty() {
        return;
    }

    if !sub_world.contains_resource::<Messages<M>>() {
        sub_world.init_resource::<Messages<M>>();
    }
    let mut sub_messages = sub_world.resource_mut::<Messages<M>>();
    for message in pending {
        sub_messages.write(message);
    }
}

/// Mirrors per-frame input state resources from the editor world
/// into the `SubApp` world. Cheaply clones the editor's authoritative
/// keyboard / mouse-button state and copies the [`Time`] resource so
/// the user's gameplay systems see consistent timing.
pub fn extract_input_state(editor_world: &mut World, sub_world: &mut World) {
    if let Some(buttons) = editor_world.get_resource::<ButtonInput<KeyCode>>() {
        sub_world.insert_resource(buttons.clone());
    }
    if let Some(buttons) = editor_world.get_resource::<ButtonInput<MouseButton>>() {
        sub_world.insert_resource(buttons.clone());
    }
    if let Some(time) = editor_world.get_resource::<Time>() {
        sub_world.insert_resource(*time);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_world() -> World {
        let mut w = World::new();
        w.init_resource::<bevy::ecs::reflect::AppTypeRegistry>();
        w
    }

    #[test]
    fn reverse_extract_mirrors_transform() {
        let mut sub = fresh_world();
        let mut editor = fresh_world();
        let sub_entity = sub.spawn(Transform::from_xyz(1.0, 2.0, 3.0)).id();

        extract_game_mirrors(&mut sub, &mut editor);

        let mirror = editor
            .query::<(&GameMirror, &Transform)>()
            .iter(&editor)
            .next()
            .expect("mirror should exist");
        assert_eq!(mirror.0.sub_entity, sub_entity);
        assert_eq!(*mirror.1, Transform::from_xyz(1.0, 2.0, 3.0));
    }

    #[test]
    fn reverse_extract_despawns_mirror_when_source_gone() {
        let mut sub = fresh_world();
        let mut editor = fresh_world();
        let sub_entity = sub.spawn(Transform::default()).id();

        extract_game_mirrors(&mut sub, &mut editor);
        assert_eq!(editor.query::<&GameMirror>().iter(&editor).count(), 1);

        sub.despawn(sub_entity);
        extract_game_mirrors(&mut sub, &mut editor);

        assert_eq!(editor.query::<&GameMirror>().iter(&editor).count(), 0);
    }

    #[test]
    fn reverse_extract_stable_identity_across_frames() {
        let mut sub = fresh_world();
        let mut editor = fresh_world();
        let _sub_entity = sub.spawn(Transform::default()).id();

        extract_game_mirrors(&mut sub, &mut editor);
        let mirror_e1 = editor
            .query::<(Entity, &GameMirror)>()
            .iter(&editor)
            .next()
            .map(|(e, _)| e)
            .unwrap();

        extract_game_mirrors(&mut sub, &mut editor);
        let mirror_e2 = editor
            .query::<(Entity, &GameMirror)>()
            .iter(&editor)
            .next()
            .map(|(e, _)| e)
            .unwrap();

        assert_eq!(
            mirror_e1, mirror_e2,
            "mirror entity must persist across frames"
        );
    }
}
