//! Editor extension surfacing Jackdaw's networking proxy components
//! (`jackdaw_multiplayer::Replication`, `NetworkRoom`) in the inspector. Pure
//! authoring: no networking runtime, no lightyear dependency.

use jackdaw_api::prelude::{ExtensionContext, ExtensionKind, Icon, JackdawExtension};

/// Reflect type paths of the networking markers paired with their outliner
/// icon. Registration order is priority. Tested against the real `TypePath`
/// so a wrong string fails loudly instead of silently never matching.
pub const NETWORKING_ICONS: &[(&str, Icon)] = &[
    ("jackdaw_multiplayer::SpawnPoint", Icon::MapPin),
    ("jackdaw_multiplayer::ZoneTransition", Icon::DoorOpen),
    ("jackdaw_multiplayer::NetworkRoom", Icon::Network),
    ("jackdaw_multiplayer::Replication", Icon::Wifi),
];

/// The user-toggleable "Multiplayer" extension. The proxy components are
/// inspector-authorable automatically once their types are registered (by
/// `jackdaw_multiplayer::JackdawMultiplayerTypesPlugin`, which the editor adds alongside this
/// extension), so `register` is a no-op.
#[derive(Default)]
pub struct MultiplayerExtension;

impl JackdawExtension for MultiplayerExtension {
    fn id(&self) -> String {
        "jackdaw.multiplayer".to_string()
    }
    fn label(&self) -> String {
        "Multiplayer".to_string()
    }
    fn description(&self) -> String {
        "Author backend-agnostic networking on entities (Replication, NetworkRoom). \
         A backend (default: lightyear) translates these to real networking at runtime."
            .to_string()
    }
    fn kind(&self) -> ExtensionKind {
        ExtensionKind::Builtin
    }
    fn register(&self, ctx: &mut ExtensionContext) {
        for (type_path, icon) in NETWORKING_ICONS {
            ctx.register_entity_icon(*type_path, *icon);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;
    use jackdaw_api::entity_icons::{EntityIconRegistry, registered_icon};
    use jackdaw_multiplayer::{NetworkRoom, Replication, SpawnPoint, ZoneTransition};

    #[test]
    fn networking_icon_paths_match_real_type_paths() {
        // Catches a typo in any NETWORKING_ICONS path: a wrong string resolves
        // to no registered type and the lookup returns None, failing here.
        let mut world = World::new();
        world.init_resource::<AppTypeRegistry>();
        {
            let registry = world.resource::<AppTypeRegistry>();
            let mut registry = registry.write();
            registry.register::<SpawnPoint>();
            registry.register::<ZoneTransition>();
            registry.register::<NetworkRoom>();
            registry.register::<Replication>();
        }

        let mut icons = EntityIconRegistry::default();
        for (type_path, icon) in NETWORKING_ICONS {
            icons.register(*type_path, *icon);
        }
        world.insert_resource(icons);

        let cases = [
            (world.spawn(SpawnPoint::default()).id(), Icon::MapPin),
            (world.spawn(ZoneTransition::default()).id(), Icon::DoorOpen),
            (world.spawn(NetworkRoom::default()).id(), Icon::Network),
            (world.spawn(Replication::default()).id(), Icon::Wifi),
        ];
        for (entity, expected) in cases {
            assert_eq!(
                registered_icon(&world, entity).map(Icon::unicode),
                Some(expected.unicode()),
            );
        }
    }

    #[test]
    fn extension_metadata_is_stable_and_builtin() {
        let ext = MultiplayerExtension;
        // The id is the stable key the catalog and saved-enabled-set use;
        // it must stay exactly this string. The "jackdaw." prefix marks it
        // a reserved built-in.
        assert_eq!(ext.id(), "jackdaw.multiplayer");
        assert_eq!(ext.label(), "Multiplayer");
        assert_eq!(ext.kind(), ExtensionKind::Builtin);
        assert!(!ext.description().is_empty());
    }
}
