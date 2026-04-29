//! Graph-local selection state.
//!
//! Kept separate from the editor's scene-level `Selection` resource because a
//! node graph lives inside a panel and must not hijack the viewport
//! selection. The API mirrors `src/selection.rs` (`select_single`, `toggle`,
//! `extend`, `clear`) so interaction code reads the same.

use bevy::prelude::*;

use crate::graph::GraphNodeSelected;

/// Ordered list of currently selected graph-node entities.
///
/// The last entry is the "primary" selection; used for focus-follows-primary
/// UI like the context inspector.
#[derive(Resource, Default, Debug)]
pub struct GraphSelection {
    pub entities: Vec<Entity>,
}

impl GraphSelection {
    /// Replace the selection with a single entity.
    pub fn select_single(&mut self, commands: &mut Commands, entity: Entity) {
        for &existing in &self.entities {
            if existing != entity
                && let Ok(mut ec) = commands.get_entity(existing)
            {
                ec.remove::<GraphNodeSelected>();
            }
        }
        self.entities.clear();
        self.entities.push(entity);
        if let Ok(mut ec) = commands.get_entity(entity) {
            ec.insert(GraphNodeSelected);
        }
    }

    /// Toggle membership of `entity` in the selection.
    pub fn toggle(&mut self, commands: &mut Commands, entity: Entity) {
        if let Some(pos) = self.entities.iter().position(|&e| e == entity) {
            self.entities.remove(pos);
            if let Ok(mut ec) = commands.get_entity(entity) {
                ec.remove::<GraphNodeSelected>();
            }
        } else {
            self.entities.push(entity);
            if let Ok(mut ec) = commands.get_entity(entity) {
                ec.insert(GraphNodeSelected);
            }
        }
    }

    /// Add `entity` to the selection without removing existing members.
    pub fn extend(&mut self, commands: &mut Commands, entity: Entity) {
        if !self.entities.contains(&entity) {
            self.entities.push(entity);
            if let Ok(mut ec) = commands.get_entity(entity) {
                ec.insert(GraphNodeSelected);
            }
        }
    }

    /// Remove every entity from the selection.
    pub fn clear(&mut self, commands: &mut Commands) {
        for &entity in &self.entities {
            if let Ok(mut ec) = commands.get_entity(entity) {
                ec.remove::<GraphNodeSelected>();
            }
        }
        self.entities.clear();
    }

    /// The entity last added to the selection, or `None` if empty.
    pub fn primary(&self) -> Option<Entity> {
        self.entities.last().copied()
    }
}
