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
/// Registration order decides priority inside a tier, and the tier decides
/// it between them. Without the second tier a rule that matches everything
/// of a shape - every `Node` is a container of some kind - would answer for
/// each entity of that shape before any rule registered after it, so an
/// extension loaded later could never name one of its own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IconTier {
    /// Says what an entity is. Asked first.
    Kind,
    /// Says what an entity is when no `Kind` rule did.
    LastResort,
}

/// A component rule, with the type id it resolves to once it has been
/// looked up.
struct ComponentRule {
    type_path: String,
    icon: Icon,
    /// Resolved on the first lookup that finds it. Filled only on a hit,
    /// so a type registered after the rule is still found later.
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
/// Inside a tier the first rule that matches wins, so the specific kinds
/// are registered before the general ones. A rule that answers for a whole
/// shape rather than a kind goes in [`IconTier::LastResort`], which is
/// asked only once every `Kind` rule has declined. Seeded by jackdaw for
/// its own types; extensions add more via
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

    /// Register a rule that reads an entity's component values. Later
    /// registrations have lower priority than earlier ones.
    pub fn register_predicate(&mut self, predicate: IconPredicate) {
        self.entries.push(IconRule::Predicate(predicate));
    }

    /// Register a rule asked only when no [`IconTier::Kind`] rule matched.
    pub fn register_last_resort_predicate(&mut self, predicate: IconPredicate) {
        self.last_resort.push(IconRule::Predicate(predicate));
    }

    /// Iterate the registered `(type_path, icon)` pairs in the order they
    /// are asked in, skipping the value predicates, which have no type
    /// path.
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

    /// A rule that answers for a whole shape has to stand behind every
    /// rule that names a kind, including the ones registered after it, or
    /// no extension loaded later could name one of its own types.
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
