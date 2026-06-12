//! Registry mapping component type paths to the outliner icon shown for
//! entities carrying them.

use bevy::prelude::*;
use lucide_icons::Icon;

/// Maps a component type path to the icon a tree row shows when an entity
/// carries that component. Registration order is the priority: the first
/// registered component an entity has wins. Seeded by jackdaw for its own
/// types; extensions add more via `ExtensionContext::register_entity_icon`.
#[derive(Resource, Default)]
pub struct EntityIconRegistry {
    entries: Vec<(String, Icon)>,
}

impl EntityIconRegistry {
    /// Register the icon shown for entities carrying `type_path`. Later
    /// registrations have lower priority than earlier ones.
    pub fn register(&mut self, type_path: impl Into<String>, icon: Icon) {
        self.entries.push((type_path.into(), icon));
    }

    /// Iterate registered `(type_path, icon)` pairs in registration order.
    pub fn iter(&self) -> impl Iterator<Item = &(String, Icon)> {
        self.entries.iter()
    }
}

/// The first registered icon for any component the entity carries, in
/// registration order. `None` when nothing matches.
pub fn registered_icon(world: &World, entity: Entity) -> Option<Icon> {
    let registry = world.get_resource::<EntityIconRegistry>()?;
    let type_registry = world.get_resource::<AppTypeRegistry>()?.read();
    let entity_ref = world.get_entity(entity).ok()?;
    for (path, icon) in registry.iter() {
        if let Some(reg) = type_registry.get_with_type_path(path)
            && entity_ref.contains_type_id(reg.type_id())
        {
            return Some(*icon);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Component, Reflect)]
    #[reflect(Component)]
    struct Mark;

    #[derive(Component, Reflect)]
    #[reflect(Component)]
    struct Other;

    fn world_with_types() -> World {
        let mut world = World::new();
        world.init_resource::<AppTypeRegistry>();
        {
            let registry = world.resource::<AppTypeRegistry>();
            let mut registry = registry.write();
            registry.register::<Mark>();
            registry.register::<Other>();
        }
        world
    }

    #[test]
    fn registered_icon_returns_first_match() {
        let mut world = world_with_types();
        let mut registry = EntityIconRegistry::default();
        registry.register(Mark::type_path(), Icon::Box);
        world.insert_resource(registry);

        let marked = world.spawn(Mark).id();
        let plain = world.spawn_empty().id();
        assert_eq!(
            registered_icon(&world, marked).map(Icon::unicode),
            Some(Icon::Box.unicode())
        );
        assert!(registered_icon(&world, plain).is_none());
    }

    #[test]
    fn first_registered_match_wins() {
        let mut world = world_with_types();
        let mut registry = EntityIconRegistry::default();
        registry.register(Mark::type_path(), Icon::Box);
        registry.register(Other::type_path(), Icon::Video);
        world.insert_resource(registry);

        let both = world.spawn((Mark, Other)).id();
        assert_eq!(
            registered_icon(&world, both).map(Icon::unicode),
            Some(Icon::Box.unicode())
        );
    }
}
