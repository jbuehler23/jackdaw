//! Registry mapping component type paths to the outliner icon shown for
//! entities carrying them.

use std::any::TypeId;
use std::sync::OnceLock;

use bevy::prelude::*;
use lucide_icons::Icon;

/// Decides an icon from an entity's component values, for kinds that a
/// component's presence alone does not separate: a UI container is a
/// `Node` either way, and only its `flex_direction` says whether it is a
/// row or a column.
pub type IconPredicate = fn(EntityRef) -> Option<Icon>;

/// When a rule is asked, relative to every other rule.
///
/// Registration order decides priority inside a tier, and the tier decides it
/// between them, so a rule matching a whole shape cannot answer ahead of an
/// extension's own kinds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IconTier {
    /// Says what an entity is. Asked first.
    Kind,
    /// Says what an entity is when no `Kind` rule did.
    LastResort,
}

/// A component rule, with the type id it resolves to once looked up.
struct ComponentRule {
    type_path: String,
    icon: Icon,
    /// Filled on the first lookup that finds it, so a type registered after
    /// the rule is still found later.
    type_id: OnceLock<TypeId>,
}

impl ComponentRule {
    fn type_id(&self, registry: &bevy::reflect::TypeRegistry) -> Option<TypeId> {
        if let Some(id) = self.type_id.get() {
            return Some(*id);
        }
        let id = registry.get_with_type_path(&self.type_path)?.type_id();
        let _ = self.type_id.set(id);
        Some(id)
    }
}

/// One rule for deciding a row's icon.
enum IconRule {
    /// An entity carrying this component type path shows this icon.
    Component(ComponentRule),
    /// A rule that reads component values.
    Predicate(IconPredicate),
}

/// The ordered rules deciding the icon a tree row shows, in two tiers.
///
/// Inside a tier the first rule that matches wins, so specific kinds are
/// registered before general ones, and a rule answering for a whole shape goes
/// in [`IconTier::LastResort`]. Extensions add rules through
/// `ExtensionContext::register_entity_icon`.
#[derive(Resource, Default)]
pub struct EntityIconRegistry {
    entries: Vec<IconRule>,
    last_resort: Vec<IconRule>,
}

impl EntityIconRegistry {
    /// Register the icon shown for entities carrying `type_path`. Later
    /// registrations have lower priority than earlier ones.
    pub fn register(&mut self, type_path: impl Into<String>, icon: Icon) {
        self.entries.push(IconRule::Component(ComponentRule {
            type_path: type_path.into(),
            icon,
            type_id: OnceLock::new(),
        }));
    }

    /// Registers a rule that reads an entity's component values, at lower
    /// priority than the rules already registered.
    pub fn register_predicate(&mut self, predicate: IconPredicate) {
        self.entries.push(IconRule::Predicate(predicate));
    }

    /// Registers a rule asked only when no [`IconTier::Kind`] rule matched.
    pub fn register_last_resort_predicate(&mut self, predicate: IconPredicate) {
        self.last_resort.push(IconRule::Predicate(predicate));
    }

    /// Iterates the registered `(type_path, icon)` pairs in the order they are
    /// asked in, skipping the value predicates, which have no type path.
    pub fn iter(&self) -> impl Iterator<Item = (&String, Icon)> {
        self.entries
            .iter()
            .chain(self.last_resort.iter())
            .filter_map(|rule| match rule {
                IconRule::Component(rule) => Some((&rule.type_path, rule.icon)),
                IconRule::Predicate(_) => None,
            })
    }
}

/// The first registered icon that matches the entity, `Kind` rules before
/// `LastResort` ones and registration order within each. `None` when
/// nothing matches.
pub fn registered_icon(world: &World, entity: Entity) -> Option<Icon> {
    let registry = world.get_resource::<EntityIconRegistry>()?;
    let type_registry = world.get_resource::<AppTypeRegistry>()?.read();
    let entity_ref = world.get_entity(entity).ok()?;
    for rule in registry.entries.iter().chain(registry.last_resort.iter()) {
        match rule {
            IconRule::Component(rule) => {
                if let Some(id) = rule.type_id(&type_registry)
                    && entity_ref.contains_type_id(id)
                {
                    return Some(rule.icon);
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

    /// A rule answering for a whole shape stands behind every rule naming a
    /// kind, including ones registered after it.
    #[test]
    fn a_last_resort_rule_stands_behind_a_kind_registered_after_it() {
        let mut world = world_with_types();
        let mut registry = EntityIconRegistry::default();
        registry.register_last_resort_predicate(|entity| {
            entity.contains::<Other>().then_some(Icon::Video)
        });
        registry.register(Mark::type_path(), Icon::Box);
        world.insert_resource(registry);

        let both = world.spawn((Mark, Other)).id();
        assert_eq!(
            registered_icon(&world, both).map(Icon::unicode),
            Some(Icon::Box.unicode()),
            "the kind rule answers even though the fallback was registered first",
        );

        let only_shape = world.spawn(Other).id();
        assert_eq!(
            registered_icon(&world, only_shape).map(Icon::unicode),
            Some(Icon::Video.unicode()),
            "and the fallback still answers when no kind rule did",
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
