//! Backend-agnostic networking proxy components for Jackdaw scenes.
//! Authored in the editor; a backend crate (default: `jackdaw_multiplayer_lightyear`)
//! translates them to real networking components at runtime. These are plain
//! `Reflect` components with NO component hooks, so they insert cleanly in the
//! editor (unlike lightyear's `Replicate`, whose `on_insert` hook panics
//! without the runtime plugins).

use bevy::prelude::*;
use bevy::reflect::std_traits::ReflectDefault;
use jackdaw_jsn::EditorCategory;
use serde::{Deserialize, Serialize};

/// Backend-agnostic replication intent for an entity. A designer adds this in
/// the inspector; the active networking backend (e.g. `jackdaw_multiplayer_lightyear`)
/// reads it at load and inserts the concrete replication component.
#[derive(Component, Reflect, Clone, Copy, PartialEq, Debug, Default)]
#[reflect(Component, Default, @EditorCategory::new("Multiplayer"))]
pub struct Replication {
    /// Which clients receive this entity.
    pub target: ReplTarget,
    /// Smoothly interpolate this entity on remote clients (for moving actors).
    pub interpolated: bool,
}

/// Author-time replication target. Peer-specific targets (a single client id)
/// are runtime-only (a concrete `PeerId` is never known at scene-author time),
/// so the authoring surface exposes only the scene-meaningful choices.
#[derive(Reflect, Clone, Copy, PartialEq, Debug, Default)]
pub enum ReplTarget {
    /// Replicate to every connected client (server-authoritative default).
    #[default]
    All,
    /// Registered but not actively replicated.
    None,
}

/// A stable zone/room id for interest management. The backend maps this to its
/// own room/relevance mechanism (lightyear: a `Room` entity + `RoomEvent`).
#[derive(Component, Reflect, Clone, Default, PartialEq, Debug)]
#[reflect(Component, Default, @EditorCategory::new("Multiplayer"))]
pub struct NetworkRoom {
    /// Stable room/zone identifier.
    pub id: ZoneId,
}

/// A human-readable zone identifier authored on a `SpawnPoint` (and matched by a
/// `ZoneTransition`). Serializes as a bare string so a `.jsn` reads
/// `"zone": "starter_zone"`. `reflect(Serialize, Deserialize)` makes Bevy reflect
/// delegate to serde; `serde(transparent)` drops the newtype wrapper. Without both,
/// reflect would emit the tuple-struct form `["starter_zone"]`.
#[derive(Reflect, Clone, Default, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[reflect(Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ZoneId(pub String);

impl ZoneId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ZoneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ZoneId {
    fn from(s: &str) -> Self {
        ZoneId(s.to_string())
    }
}

impl From<String> for ZoneId {
    fn from(s: String) -> Self {
        ZoneId(s)
    }
}

/// Where a connecting player materializes. Authored in the editor; the
/// multiplayer server reads spawn points from the loaded scene.
#[derive(Component, Reflect, Clone, Default, PartialEq, Debug)]
#[reflect(Component, Default, @EditorCategory::new("Multiplayer"))]
pub struct SpawnPoint {
    /// Zone this spawn belongs to (matches a `NetworkRoom` id). Empty = unset.
    #[reflect(default)]
    pub zone: ZoneId,
    /// Spawn tag. Empty = the zone's default spawn (initial connect). Named
    /// tags (e.g. `north_gate`) are destination targets a `ZoneTransition`
    /// names.
    pub tag: String,
}

/// A trigger volume that moves a player into another zone. Authored on a
/// box volume; the server tests player overlap against its bounds.
#[derive(Component, Reflect, Clone, Default, PartialEq, Debug)]
#[reflect(Component, Default, @EditorCategory::new("Multiplayer"))]
pub struct ZoneTransition {
    /// Zone id (a `NetworkRoom` id) the player is moved INTO.
    pub dest_zone: ZoneId,
    /// Tag of the `SpawnPoint` in `dest_zone` to place the player at.
    pub dest_spawn_tag: String,
    /// Half-extents of the trigger box (local space), tested against the
    /// player position relative to this entity's `GlobalTransform`.
    pub half_extents: Vec3,
}

/// Registers the proxy components for reflection so the inspector + `.jsn`
/// (de)serializer handle them.
pub struct JackdawMultiplayerTypesPlugin;

impl Plugin for JackdawMultiplayerTypesPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Replication>()
            .register_type::<ReplTarget>()
            .register_type::<NetworkRoom>()
            .register_type::<SpawnPoint>()
            .register_type::<ZoneTransition>()
            .register_type::<ZoneId>();
        app.register_type_data::<Replication, ReflectDefault>()
            .register_type_data::<ReplTarget, ReflectDefault>()
            .register_type_data::<NetworkRoom, ReflectDefault>()
            .register_type_data::<SpawnPoint, ReflectDefault>()
            .register_type_data::<ZoneTransition, ReflectDefault>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::reflect::TypeRegistry;
    use bevy::reflect::serde::{TypedReflectDeserializer, TypedReflectSerializer};
    use bevy::reflect::std_traits::ReflectDefault;
    use serde::de::DeserializeSeed;
    use std::any::TypeId;

    #[test]
    fn proxies_insert_without_panic_and_register() {
        let mut app = App::new();
        app.add_plugins(JackdawMultiplayerTypesPlugin);

        let e = app.world_mut().spawn_empty().id();
        app.world_mut().entity_mut(e).insert(Replication {
            target: ReplTarget::All,
            interpolated: true,
        });
        app.world_mut().entity_mut(e).insert(NetworkRoom {
            id: ZoneId::from("7"),
        });
        app.update();

        assert!(app.world().entity(e).contains::<Replication>());
        assert!(app.world().entity(e).contains::<NetworkRoom>());

        let registry = app.world().resource::<AppTypeRegistry>().read();
        for tn in [
            std::any::type_name::<Replication>(),
            std::any::type_name::<NetworkRoom>(),
        ] {
            let reg = registry
                .get_with_type_path(tn)
                .unwrap_or_else(|| panic!("{tn} not registered"));
            assert!(
                reg.data::<bevy::ecs::reflect::ReflectComponent>().is_some(),
                "{tn} missing ReflectComponent"
            );
            assert!(
                reg.data::<ReflectDefault>().is_some(),
                "{tn} missing ReflectDefault"
            );
        }
    }

    #[test]
    fn spawn_and_transition_register_with_component_and_default() {
        let mut app = App::new();
        app.add_plugins(JackdawMultiplayerTypesPlugin);
        let registry = app.world().resource::<AppTypeRegistry>().read();
        for tn in [
            std::any::type_name::<SpawnPoint>(),
            std::any::type_name::<ZoneTransition>(),
        ] {
            let reg = registry
                .get_with_type_path(tn)
                .unwrap_or_else(|| panic!("{tn} not registered"));
            assert!(
                reg.data::<bevy::ecs::reflect::ReflectComponent>().is_some(),
                "{tn} missing ReflectComponent"
            );
            assert!(
                reg.data::<bevy::reflect::std_traits::ReflectDefault>()
                    .is_some(),
                "{tn} missing ReflectDefault"
            );
        }
    }

    #[test]
    fn zone_id_reflect_serializes_as_bare_string() {
        let mut registry = TypeRegistry::new();
        registry.register::<ZoneId>();
        let zone = ZoneId("starter_zone".to_string());
        let serializer = TypedReflectSerializer::new(zone.as_partial_reflect(), &registry);
        let json = serde_json::to_value(&serializer).unwrap();
        assert_eq!(json, serde_json::json!("starter_zone"));
    }

    #[test]
    fn zone_id_reflect_deserializes_from_bare_string() {
        let mut registry = TypeRegistry::new();
        registry.register::<ZoneId>();
        let reg = registry.get(TypeId::of::<ZoneId>()).unwrap();
        let de = TypedReflectDeserializer::new(reg, &registry);
        let json = serde_json::json!("starter_zone");
        let json_str = json.to_string();
        let mut deser = serde_json::Deserializer::from_str(&json_str);
        let val = de.deserialize(&mut deser).unwrap();
        let zone = ZoneId::from_reflect(val.as_ref()).unwrap();
        assert_eq!(zone, ZoneId("starter_zone".to_string()));
    }
}
