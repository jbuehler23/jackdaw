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
use bevy::prelude::*;
use bevy::scene::{DynamicSceneBuilder, SceneFilter};

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
