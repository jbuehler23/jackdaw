// Adapted from bevy_monitors 0.3.0 (https://github.com/Freyja-moth/bevy_monitors).
// Copyright (c) 2025 Freyja-moth. Licensed under MIT or Apache-2.0; the MIT
// notice is in LICENSE-MIT beside this file.

//! Monitors: entities that react to components being added to or changed on
//! other entities, through [`NotifyAdded`] and [`NotifyChanged`].

mod addition;
mod mutation;

pub use addition::{Addition, NotifyAdded};
pub use mutation::{Mutation, MutationSet, NotifyChanged};

use bevy::prelude::*;

/// Contains all the monitors that are watching this entity.
#[derive(Component, Reflect, Hash, PartialEq, Eq, PartialOrd, Ord, Default, Debug)]
#[relationship_target(relationship = Monitor)]
pub struct MonitoredBy(Vec<Entity>);

/// The entity a monitor reacts to.
///
/// Being a [`Relationship`](bevy::ecs::relationship::Relationship) it cannot
/// point at its own entity; use [`MonitorSelf`] for that. With both, the
/// monitor reacts to itself and to the entity given here. A monitor with
/// neither reacts to every entity, which is costly for frequently changing
/// components.
#[derive(Component, Reflect, Hash, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[relationship(relationship_target = MonitoredBy)]
pub struct Monitor(pub Entity);

/// Makes a monitor react to its own entity. See [`Monitor`].
#[derive(Component, Reflect, Hash, PartialEq, Eq, PartialOrd, Ord, Default, Debug)]
pub struct MonitorSelf;

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Component)]
    struct Tester;

    #[derive(Resource, Default)]
    struct TesterAdded(usize);

    fn world_with_monitor(bundle: impl Bundle) -> (World, Entity) {
        let mut world = World::new();
        world.init_resource::<TesterAdded>();
        let monitor = world
            .spawn(bundle)
            .observe(|_: On<Addition<Tester>>, mut added: ResMut<TesterAdded>| {
                added.0 += 1;
            })
            .id();
        (world, monitor)
    }

    #[test]
    fn a_self_monitor_reacts_only_to_its_own_entity() {
        let (mut world, monitor) =
            world_with_monitor((MonitorSelf, NotifyAdded::<Tester>::default()));
        let other = world.spawn_empty().id();
        world.entity_mut(other).insert(Tester);
        assert_eq!(world.resource::<TesterAdded>().0, 0);
        world.entity_mut(monitor).insert(Tester);
        assert_eq!(world.resource::<TesterAdded>().0, 1);
    }

    #[test]
    fn a_related_monitor_reacts_only_to_its_target() {
        let mut world = World::new();
        let target = world.spawn_empty().id();
        world.init_resource::<TesterAdded>();
        let monitor = world
            .spawn((Monitor(target), NotifyAdded::<Tester>::default()))
            .observe(|_: On<Addition<Tester>>, mut added: ResMut<TesterAdded>| {
                added.0 += 1;
            })
            .id();
        world.entity_mut(monitor).insert(Tester);
        assert_eq!(world.resource::<TesterAdded>().0, 0);
        world.entity_mut(target).insert(Tester);
        assert_eq!(world.resource::<TesterAdded>().0, 1);
    }

    #[test]
    fn a_monitor_without_a_target_reacts_to_every_entity() {
        let (mut world, monitor) = world_with_monitor(NotifyAdded::<Tester>::default());
        let first = world.spawn_empty().id();
        let second = world.spawn_empty().id();
        world.entity_mut(monitor).insert(Tester);
        world.entity_mut(first).insert(Tester);
        world.entity_mut(second).insert(Tester);
        assert_eq!(world.resource::<TesterAdded>().0, 3);
    }
}
