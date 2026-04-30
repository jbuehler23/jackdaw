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
    pub fn select_multiple(&mut self, commands: &mut Commands, entities: &[Entity]) {
        self.clear(commands);
        for &entity in entities {
            self.entities.push(entity);
            if let Ok(mut ec) = commands.get_entity(entity) {
                ec.insert(Selected);
            }
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

/// Clean up the Selection resource when a Selected component is removed
/// (e.g., entity despawned).
fn on_selected_removed(trigger: On<Remove, Selected>, mut selection: ResMut<Selection>) {
    let entity = trigger.event_target();
    selection.entities.retain(|&e| e != entity);
}
