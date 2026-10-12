// Adapted from bevy_monitors 0.3.0 (https://github.com/Freyja-moth/bevy_monitors).
// Copyright (c) 2025 Freyja-moth. Licensed under MIT or Apache-2.0; the MIT
// notice is in LICENSE-MIT beside this file.

use std::marker::PhantomData;

use bevy::ecs::{lifecycle::HookContext, world::DeferredWorld};
use bevy::prelude::*;

use super::{Monitor, MonitorSelf};

/// The [`Update`] set whose systems trigger [`Mutation`].
#[derive(SystemSet, Hash, PartialEq, Eq, Clone, Debug, Default)]
pub struct MutationSystems;

/// Marks that component `C` already has a system watching it for changes.
#[derive(Resource)]
struct DetectingChanges<C>(PhantomData<C>);

/// Component `C` changed on an entity a monitor watches. See [`NotifyChanged`].
#[derive(EntityEvent)]
pub struct Mutation<C: Component> {
    /// The monitor.
    pub entity: Entity,
    /// The entity whose `C` changed.
    pub mutated: Entity,
    _phantom: PhantomData<C>,
}

/// Makes this entity react to changes of component `C` with [`Mutation`].
///
/// Without [`Monitor`] or [`MonitorSelf`] it reacts to every entity. The first
/// instance adds one system for `C` to [`Update`] in [`MutationSystems`], which
/// stays for the life of the world.
#[derive(Component, Reflect, Hash, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[component(on_add = NotifyChanged::<C>::register_component_change_system)]
pub struct NotifyChanged<C: Component>(PhantomData<C>);

impl<C: Component> Default for NotifyChanged<C> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<C: Component> NotifyChanged<C> {
    fn register_component_change_system(mut world: DeferredWorld, _: HookContext) {
        if world.contains_resource::<DetectingChanges<C>>() {
            return;
        }
        world.commands().queue(|world: &mut World| {
            world.schedule_scope(Update, |_, schedule| {
                schedule.configure_sets(MutationSystems);
                schedule.add_systems(watch_for_change::<C>.in_set(MutationSystems));
            });
            world.insert_resource(DetectingChanges::<C>(PhantomData));
        });
    }
}

fn watch_for_change<C: Component>(
    mut commands: Commands,
    changed: Populated<Entity, Changed<C>>,
    local_monitors: Query<Entity, (With<NotifyChanged<C>>, With<MonitorSelf>)>,
    monitors: Query<(Entity, &Monitor), With<NotifyChanged<C>>>,
    global_monitors: Query<
        Entity,
        (
            With<NotifyChanged<C>>,
            Without<MonitorSelf>,
            Without<Monitor>,
        ),
    >,
) {
    let trigger = |commands: &mut Commands, entity, mutated| {
        commands.trigger(Mutation::<C> {
            entity,
            mutated,
            _phantom: PhantomData,
        });
    };
    for entity in local_monitors.iter_many(changed.iter()).flatten() {
        trigger(&mut commands, entity, entity);
    }
    for (entity, &Monitor(mutated)) in &monitors {
        if changed.contains(mutated) {
            trigger(&mut commands, entity, mutated);
        }
    }
    for monitor in &global_monitors {
        for mutated in &changed {
            trigger(&mut commands, monitor, mutated);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct TimesMoved(usize);

    #[test]
    fn each_change_is_reported_until_the_notifier_is_removed() {
        let mut world = World::new();
        world.add_schedule(Schedule::new(Update));
        world.init_resource::<TimesMoved>();
        let player = world
            .spawn((
                Transform::default(),
                MonitorSelf,
                NotifyChanged::<Transform>::default(),
            ))
            .observe(
                |_: On<Mutation<Transform>>, mut moved: ResMut<TimesMoved>| {
                    moved.0 += 1;
                },
            )
            .id();

        for amount in [10.0, 5.0, -2.0] {
            world.get_mut::<Transform>(player).unwrap().translation.x += amount;
            world.run_schedule(Update);
        }
        assert_eq!(world.resource::<TimesMoved>().0, 3);

        world
            .entity_mut(player)
            .remove::<NotifyChanged<Transform>>();
        world.get_mut::<Transform>(player).unwrap().translation.x -= 100.0;
        world.run_schedule(Update);
        assert_eq!(world.resource::<TimesMoved>().0, 3);
    }
}
