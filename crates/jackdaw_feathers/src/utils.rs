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
/// `Entity despawned: … is invalid`, and the just-spawned `child`
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
/// `Entity despawned: … is invalid`.
pub fn insert_if_alive<B: Bundle>(commands: &mut Commands, entity: Entity, bundle: B) {
    commands.queue(move |world: &mut World| {
        if let Ok(mut ec) = world.get_entity_mut(entity) {
            ec.insert(bundle);
        }
    });
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
