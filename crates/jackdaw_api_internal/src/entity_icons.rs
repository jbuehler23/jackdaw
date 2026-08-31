//! Registry mapping component type paths to the outliner icon shown for
//! entities carrying them.

use bevy::prelude::*;
use lucide_icons::Icon;

/// Decides an icon from an entity's component values, for kinds that a
/// component's presence alone does not separate: a UI container is a
/// `Node` either way, and only its `flex_direction` says whether it is a
/// row or a column.
pub type IconPredicate = fn(EntityRef) -> Option<Icon>;

/// One rule for deciding a row's icon.
enum IconRule {
    /// An entity carrying this component type path shows this icon.
    Component(String, Icon),
    /// A rule that reads component values.
    Predicate(IconPredicate),
}

/// The ordered rules deciding the icon a tree row shows. Order is the
/// priority and the whole of it: the first rule that matches wins, so
/// the specific kinds are registered before the general ones, and a
/// value predicate for a container goes after every component that would
/// make the same entity something more particular. Seeded by jackdaw for
/// its own types; extensions add more via
/// `ExtensionContext::register_entity_icon`.
#[derive(Resource, Default)]
pub struct EntityIconRegistry {
    entries: Vec<IconRule>,
}

impl EntityIconRegistry {
    /// Register the icon shown for entities carrying `type_path`. Later
    /// registrations have lower priority than earlier ones.
    pub fn register(&mut self, type_path: impl Into<String>, icon: Icon) {
        self.entries
            .push(IconRule::Component(type_path.into(), icon));
    }

    /// Register a rule that reads an entity's component values. Later
    /// registrations have lower priority than earlier ones.
    pub fn register_predicate(&mut self, predicate: IconPredicate) {
        self.entries.push(IconRule::Predicate(predicate));
    }

    /// Iterate the registered `(type_path, icon)` pairs in registration
    /// order, skipping the value predicates, which have no type path.
    pub fn iter(&self) -> impl Iterator<Item = (&String, Icon)> {
        self.entries.iter().filter_map(|rule| match rule {
            IconRule::Component(path, icon) => Some((path, *icon)),
            IconRule::Predicate(_) => None,
        })
    }
}

/// The first registered icon that matches the entity, in registration
/// order. `None` when nothing matches.
pub fn registered_icon(world: &World, entity: Entity) -> Option<Icon> {
    let registry = world.get_resource::<EntityIconRegistry>()?;
    let type_registry = world.get_resource::<AppTypeRegistry>()?.read();
    let entity_ref = world.get_entity(entity).ok()?;
    for rule in &registry.entries {
        match rule {
            IconRule::Component(path, icon) => {
                if let Some(reg) = type_registry.get_with_type_path(path)
                    && entity_ref.contains_type_id(reg.type_id())
                {
                    return Some(*icon);
                }
            }
            IconRule::Predicate(predicate) => {
                if let Some(icon) = predicate(entity_ref) {
                    return Some(icon);
                }
            }
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
    fn a_predicate_decides_when_no_component_rule_has() {
        let mut world = world_with_types();
        let mut registry = EntityIconRegistry::default();
        registry.register(Mark::type_path(), Icon::Box);
        registry.register_predicate(|entity| entity.contains::<Other>().then_some(Icon::Video));
        world.insert_resource(registry);

        let by_predicate = world.spawn(Other).id();
        assert_eq!(
            registered_icon(&world, by_predicate).map(Icon::unicode),
            Some(Icon::Video.unicode())
        );

        // The earlier component rule still wins over a later predicate.
        let both = world.spawn((Mark, Other)).id();
        assert_eq!(
            registered_icon(&world, both).map(Icon::unicode),
            Some(Icon::Box.unicode())
        );
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
