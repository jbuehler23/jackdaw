// Adapted from bevy_monitors 0.3.0 (https://github.com/Freyja-moth/bevy_monitors).
// Copyright (c) 2025 Freyja-moth. Licensed under MIT or Apache-2.0; the MIT
// notice is in LICENSE-MIT beside this file.

use std::marker::PhantomData;

use bevy::ecs::{lifecycle::HookContext, world::DeferredWorld};
use bevy::prelude::*;

use super::{Monitor, MonitorSelf};

/// Marks that component `C` already has an observer watching for it being added.
#[derive(Resource)]
struct DetectingAdded<C: Component>(PhantomData<C>);

/// Component `C` was added to an entity a monitor watches. See [`NotifyAdded`].
#[derive(EntityEvent)]
pub struct Addition<C: Component> {
    /// The monitor.
    pub entity: Entity,
    /// The entity `C` was added to.
    pub added: Entity,
    _phantom: PhantomData<C>,
}

/// Makes this entity react to component `C` being added with [`Addition`].
///
/// Without [`Monitor`] or [`MonitorSelf`] it reacts to every entity. The first
/// instance spawns one observer for `C`, which stays for the life of the world.
#[derive(Component)]
#[component(on_add = NotifyAdded::<C>::register_component_add_observer)]
pub struct NotifyAdded<C: Component>(PhantomData<C>);

impl<C: Component> Default for NotifyAdded<C> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<C: Component> NotifyAdded<C> {
    fn register_component_add_observer(mut world: DeferredWorld, _: HookContext) {
        if world.contains_resource::<DetectingAdded<C>>() {
            return;
        }
        world.commands().add_observer(notify_on_add::<C>);
        world
            .commands()
            .insert_resource(DetectingAdded::<C>(PhantomData));
    }
}

fn notify_on_add<C: Component>(
    add: On<Add<C>>,
    mut commands: Commands,
    local_monitors: Query<Entity, (With<NotifyAdded<C>>, With<MonitorSelf>)>,
    monitors: Query<(Entity, &Monitor), With<NotifyAdded<C>>>,
    global_monitors: Query<Entity, (With<NotifyAdded<C>>, Without<Monitor>, Without<MonitorSelf>)>,
) {
    let trigger = |commands: &mut Commands, entity, added| {
        commands.trigger(Addition::<C> {
            entity,
            added,
            _phantom: PhantomData,
        });
    };
    if local_monitors.contains(add.entity) {
        trigger(&mut commands, add.entity, add.entity);
    }
    for (entity, &Monitor(added)) in &monitors {
        if added == add.entity {
            trigger(&mut commands, entity, added);
        }
    }
    for entity in &global_monitors {
        trigger(&mut commands, entity, add.entity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Component)]
    struct Poisoned;

    #[derive(Resource, Default)]
    struct TimesPoisoned(usize);

    #[test]
    fn each_insert_is_reported_until_the_notifier_is_removed() {
        let mut world = World::new();
        world.init_resource::<TimesPoisoned>();
        let player = world
            .spawn((MonitorSelf, NotifyAdded::<Poisoned>::default()))
            .observe(
                |_: On<Addition<Poisoned>>, mut count: ResMut<TimesPoisoned>| {
                    count.0 += 1;
                },
            )
            .id();

        world.entity_mut(player).insert(Poisoned);
        world.entity_mut(player).remove::<Poisoned>();
        world.entity_mut(player).insert(Poisoned);
        assert_eq!(world.resource::<TimesPoisoned>().0, 2);

        world
            .entity_mut(player)
            .remove::<NotifyAdded<Poisoned>>()
            .insert(Poisoned);
        assert_eq!(world.resource::<TimesPoisoned>().0, 2);
    }
}
