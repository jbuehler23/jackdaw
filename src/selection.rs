use bevy::prelude::*;

pub struct SelectionPlugin;

impl Plugin for SelectionPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Selection::default())
            .add_observer(on_selected_removed);
    }
}

/// Marker component placed on selected entities. Multiple entities can have this.
#[derive(Component)]
pub struct Selected;

/// Make `entity` the whole selection, from an exclusive-world caller.
pub fn select_only(world: &mut World, entity: Entity) {
    let mut state: bevy::ecs::system::SystemState<(Commands, ResMut<Selection>)> =
        bevy::ecs::system::SystemState::new(world);
    let Ok((mut commands, mut selection)) = state.get_mut(world) else {
        return;
    };
    selection.select_single(&mut commands, entity);
    state.apply(world);
}

/// Put `entities` back as the selection, in the order given.
pub fn select_many(world: &mut World, entities: &[Entity]) {
    let mut state: bevy::ecs::system::SystemState<(Commands, ResMut<Selection>)> =
        bevy::ecs::system::SystemState::new(world);
    let Ok((mut commands, mut selection)) = state.get_mut(world) else {
        return;
    };
    selection.select_multiple(&mut commands, entities);
    state.apply(world);
}

/// Bring `entity` into the selection, because the inspector's write paths act
/// on the selection rather than on an entity handed to them. A target outside
/// the selection replaces it; one already inside leaves the selection alone.
pub fn select_for_edit(world: &mut World, entity: Entity) {
    let already_selected = world
        .get_resource::<Selection>()
        .is_some_and(|selection| selection.is_selected(entity));
    if !already_selected {
        select_only(world, entity);
    }
}

/// Resource tracking the full selection state.
#[derive(Resource, Default)]
pub struct Selection {
    /// Ordered list of selected entities. The last entity is the primary selection.
    pub entities: Vec<Entity>,
}

impl Selection {
    /// Select a single entity, clearing all others.
    pub fn select_single(&mut self, commands: &mut Commands, entity: Entity) {
        // Remove Selected from all currently selected entities
        for &e in &self.entities {
            if e != entity
                && let Ok(mut ec) = commands.get_entity(e)
            {
                ec.remove::<Selected>();
            }
        }
        self.entities.clear();
        self.entities.push(entity);
        if let Ok(mut ec) = commands.get_entity(entity) {
            ec.insert(Selected);
        }
    }

    /// Toggle selection of an entity (Ctrl+Click behavior).
    pub fn toggle(&mut self, commands: &mut Commands, entity: Entity) {
        if let Some(pos) = self.entities.iter().position(|&e| e == entity) {
            self.entities.remove(pos);
            if let Ok(mut ec) = commands.get_entity(entity) {
                ec.remove::<Selected>();
            }
        } else {
            self.entities.push(entity);
            if let Ok(mut ec) = commands.get_entity(entity) {
                ec.insert(Selected);
            }
        }
    }

    /// Extend selection to include an entity (without removing others).
    pub fn extend(&mut self, commands: &mut Commands, entity: Entity) {
        if !self.entities.contains(&entity) {
            self.entities.push(entity);
            if let Ok(mut ec) = commands.get_entity(entity) {
                ec.insert(Selected);
            }
        }
    }

    /// Clear all selection.
    pub fn clear(&mut self, commands: &mut Commands) {
        for &e in &self.entities {
            if let Ok(mut ec) = commands.get_entity(e) {
                ec.remove::<Selected>();
            }
        }
        self.entities.clear();
    }

    /// Select multiple entities at once (for box select).
    ///
    /// An entity that stays selected keeps its [`Selected`] marker rather than
    /// losing it and being given it back: the removal fires the prune observer,
    /// which would take it out of this very list. An entity no longer in the
    /// world is dropped rather than listed.
    pub fn select_multiple(&mut self, commands: &mut Commands, entities: &[Entity]) {
        for &previous in &self.entities {
            if !entities.contains(&previous)
                && let Ok(mut ec) = commands.get_entity(previous)
            {
                ec.remove::<Selected>();
            }
        }
        self.entities.clear();
        for &entity in entities {
            let Ok(mut ec) = commands.get_entity(entity) else {
                continue;
            };
            ec.insert(Selected);
            self.entities.push(entity);
        }
    }

    /// Get the primary (last) selected entity.
    pub fn primary(&self) -> Option<Entity> {
        self.entities.last().copied()
    }

    /// Check if an entity is selected.
    pub fn is_selected(&self, entity: Entity) -> bool {
        self.entities.contains(&entity)
    }
}

/// Clear the selection directly against the world, removing the `Selected`
/// marker from each entity through guarded access so a despawned entry is
/// skipped rather than panicking. Used by paths that mutate selection inside
/// a `&mut World` closure, such as the PIE Scene/Live view toggle, which must
/// drop the selection before the previewed entities are despawned and
/// replaced.
pub fn clear_selection_in_world(world: &mut World) {
    let entities: Vec<Entity> = std::mem::take(&mut world.resource_mut::<Selection>().entities);
    for entity in entities {
        if let Ok(mut em) = world.get_entity_mut(entity) {
            em.remove::<Selected>();
        }
    }
}

/// Clean up the Selection resource when a Selected component is removed
/// (e.g., entity despawned).
fn on_selected_removed(trigger: On<Remove, Selected>, mut selection: ResMut<Selection>) {
    let entity = trigger.event_target();
    selection.entities.retain(|&e| e != entity);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_selection_in_world_empties_and_removes_markers() {
        let mut world = World::new();
        world.add_observer(on_selected_removed);
        world.insert_resource(Selection::default());

        let a = world.spawn(Selected).id();
        let b = world.spawn(Selected).id();
        world.resource_mut::<Selection>().entities = vec![a, b];

        clear_selection_in_world(&mut world);

        assert!(world.resource::<Selection>().entities.is_empty());
        assert!(world.get::<Selected>(a).is_none());
        assert!(world.get::<Selected>(b).is_none());
    }

    #[test]
    fn select_multiple_drops_an_entity_that_is_no_longer_there() {
        let mut world = World::new();
        world.insert_resource(Selection::default());
        let live = world.spawn_empty().id();
        let dead = world.spawn_empty().id();
        world.despawn(dead);

        let mut state: bevy::ecs::system::SystemState<Commands> =
            bevy::ecs::system::SystemState::new(&mut world);
        {
            let Ok(mut commands) = state.get_mut(&mut world) else {
                panic!("a fresh world hands out commands");
            };
            let mut selection = Selection::default();
            selection.select_multiple(&mut commands, &[live, dead]);
            assert_eq!(
                selection.entities,
                vec![live],
                "the despawned entity is left out rather than listed as selected",
            );
            assert_eq!(
                selection.primary(),
                Some(live),
                "so the primary is something that is still there",
            );
        }
        state.apply(&mut world);
        assert!(world.get::<Selected>(live).is_some());
    }

    #[test]
    fn clear_selection_in_world_skips_already_despawned_entity() {
        let mut world = World::new();
        world.insert_resource(Selection::default());

        let live = world.spawn(Selected).id();
        let dead = world.spawn(Selected).id();
        world.resource_mut::<Selection>().entities = vec![live, dead];

        // Mirror the toggle race: the previewed entity is despawned before
        // the selection prune runs. Guarded access must not panic on it.
        world.despawn(dead);

        clear_selection_in_world(&mut world);

        assert!(world.resource::<Selection>().entities.is_empty());
        assert!(world.get::<Selected>(live).is_none());
    }
}
