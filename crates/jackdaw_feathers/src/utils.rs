use bevy::prelude::*;

/// Attach `child` as a child of `parent` at `Commands` flush time,
/// cleanly despawning `child` if `parent` has gone away by then.
///
/// Use this instead of `commands.entity(parent).add_child(child)` in
/// any setup system / widget initializer where:
/// - `child` has just been spawned via `commands.spawn(...).id()`
///   and may outlive its intended parent, AND
/// - `parent` is a UI-tree entity that can be cascade-despawned by
///   an inspector / panel rebuild between the queue and the flush.
///
/// The raw `add_child` call internally queues a command that takes
/// `EntityWorldMut` of the parent; if that parent was despawned
/// between the queue and the flush the command fails with
/// `Entity despawned: ... is invalid`, and the just-spawned `child`
/// is left as an orphan with a `ChildOf(dead parent)` that Bevy
/// strips with an additional `WARN` (manifesting as stray floating
/// UI nodes like "Inherited" or "Component field" at the window
/// root). This helper closes that race: inside one world-exclusive
/// closure it checks the parent is alive, then attaches; and if
/// the parent isn't alive, it despawns the orphan instead.
pub fn attach_or_despawn(commands: &mut Commands, parent: Entity, child: Entity) {
    commands.queue(move |world: &mut World| {
        if world.get_entity(parent).is_ok() {
            if let Ok(mut ec) = world.get_entity_mut(parent) {
                ec.add_child(child);
            }
        } else if let Ok(ec) = world.get_entity_mut(child) {
            ec.despawn();
        }
    });
}

/// Variant of [`attach_or_despawn`] for attaching multiple children at
/// once. If the parent is dead, every child is despawned.
pub fn attach_children_or_despawn(commands: &mut Commands, parent: Entity, children: &[Entity]) {
    let children: Box<[Entity]> = children.into();
    commands.queue(move |world: &mut World| {
        if world.get_entity(parent).is_ok() {
            if let Ok(mut ec) = world.get_entity_mut(parent) {
                ec.add_children(&children);
            }
        } else {
            for child in &children {
                if let Ok(ec) = world.get_entity_mut(*child) {
                    ec.despawn();
                }
            }
        }
    });
}

/// Insert `bundle` into `entity` at `Commands` flush time if
/// `entity` is still alive, otherwise silently skip. Use for
/// component inserts that target a widget-internal entity whose
/// wrapper might have been torn down by [`attach_or_despawn`]'s
/// fallback despawn path before this command drains; the raw
/// `commands.entity(entity).insert(bundle)` would otherwise log
/// `Entity despawned: ... is invalid`.
pub fn insert_if_alive<B: Bundle>(commands: &mut Commands, entity: Entity, bundle: B) {
    commands.queue(move |world: &mut World| {
        if let Ok(mut ec) = world.get_entity_mut(entity) {
            ec.insert(bundle);
        }
    });
}

/// Remove `B` from `entity` at `Commands` flush time if `entity` is
/// still alive, otherwise silently skip.
///
/// The counterpart to [`insert_if_alive`], for the other half of a
/// toggle. A control that answers a value change by writing
/// `Checked` on a row keeps a captured `Entity`, and a panel or
/// inspector rebuild between the event and the flush despawns it;
/// the raw `commands.entity(entity).remove::<Checked>()` then logs
/// `Entity despawned: ... is invalid`.
pub fn remove_if_alive<B: Bundle>(commands: &mut Commands, entity: Entity) {
    commands.queue(move |world: &mut World| {
        if let Ok(mut ec) = world.get_entity_mut(entity) {
            ec.remove::<B>();
        }
    });
}

/// Drive a marker component on `entity` from a bool at flush time,
/// skipping an entity that has gone away. The shape every
/// `Checked` toggle wants.
pub fn set_marker_if_alive<B: Bundle + Default>(
    commands: &mut Commands,
    entity: Entity,
    present: bool,
) {
    if present {
        insert_if_alive(commands, entity, B::default());
    } else {
        remove_if_alive::<B>(commands, entity);
    }
}

pub fn is_descendant_of(entity: Entity, ancestor: Entity, parents: &Query<&ChildOf>) -> bool {
    let mut current = entity;
    for _ in 0..50 {
        if current == ancestor {
            return true;
        }
        if let Ok(child_of) = parents.get(current) {
            current = child_of.parent();
        } else {
            return false;
        }
    }
    false
}

pub fn find_ancestor<'a, C: Component>(
    entity: Entity,
    query: &'a Query<&C>,
    parents: &Query<&ChildOf>,
) -> Option<(Entity, &'a C)> {
    let mut current = entity;
    for _ in 0..50 {
        if let Ok(component) = query.get(current) {
            return Some((current, component));
        }
        if let Ok(child_of) = parents.get(current) {
            current = child_of.parent();
        } else {
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::world::CommandQueue;

    #[derive(Component, Default)]
    struct Mark;

    /// Queue against `entity`, then despawn it before the flush, the
    /// way a panel rebuild does mid-frame.
    fn flush_after_despawn(
        world: &mut World,
        entity: Entity,
        queue: impl FnOnce(&mut Commands, Entity),
    ) {
        let mut queued = CommandQueue::default();
        let mut commands = Commands::new(&mut queued, world);
        queue(&mut commands, entity);
        world.despawn(entity);
        queued.apply(world);
    }

    #[test]
    fn marker_writes_skip_a_despawned_entity() {
        let mut world = World::new();

        for present in [true, false] {
            let entity = world.spawn_empty().id();
            flush_after_despawn(&mut world, entity, |commands, entity| {
                set_marker_if_alive::<Mark>(commands, entity, present);
            });
            // The write is dropped rather than erroring on a dead id.
            assert!(world.get_entity(entity).is_err());
        }
    }

    #[test]
    fn marker_writes_still_land_on_a_live_entity() {
        let mut world = World::new();
        let entity = world.spawn_empty().id();

        let mut queued = CommandQueue::default();
        let mut commands = Commands::new(&mut queued, &world);
        set_marker_if_alive::<Mark>(&mut commands, entity, true);
        queued.apply(&mut world);
        assert!(world.get::<Mark>(entity).is_some());

        let mut queued = CommandQueue::default();
        let mut commands = Commands::new(&mut queued, &world);
        set_marker_if_alive::<Mark>(&mut commands, entity, false);
        queued.apply(&mut world);
        assert!(world.get::<Mark>(entity).is_none());
    }

    #[test]
    fn remove_if_alive_skips_a_despawned_entity() {
        let mut world = World::new();
        let entity = world.spawn(Mark).id();
        flush_after_despawn(&mut world, entity, |commands, entity| {
            remove_if_alive::<Mark>(commands, entity);
        });
        assert!(world.get_entity(entity).is_err());
    }

    /// The `ChildOf(dead parent)` case: the child must not be left
    /// orphaned with a relationship Bevy then strips with a warning.
    #[test]
    fn attach_despawns_the_orphan_when_the_parent_died() {
        let mut world = World::new();
        let parent = world.spawn_empty().id();
        let child = world.spawn_empty().id();

        let mut queued = CommandQueue::default();
        let mut commands = Commands::new(&mut queued, &world);
        attach_or_despawn(&mut commands, parent, child);
        world.despawn(parent);
        queued.apply(&mut world);

        assert!(world.get_entity(child).is_err());
    }

    #[test]
    fn attach_still_parents_when_the_parent_lives() {
        let mut world = World::new();
        let parent = world.spawn_empty().id();
        let child = world.spawn_empty().id();

        let mut queued = CommandQueue::default();
        let mut commands = Commands::new(&mut queued, &world);
        attach_or_despawn(&mut commands, parent, child);
        queued.apply(&mut world);

        assert_eq!(
            world.get::<ChildOf>(child).map(ChildOf::parent),
            Some(parent)
        );
    }
}
