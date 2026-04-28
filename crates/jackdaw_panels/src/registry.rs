use std::{collections::HashMap, sync::Arc};

use bevy::prelude::*;

pub type DockWindowBuildFn = Arc<dyn Fn(&mut ChildSpawner) + Send + Sync + 'static>;

pub struct DockWindowDescriptor {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub default_area: String,
    pub priority: i32,
    pub build: DockWindowBuildFn,
}

#[derive(Resource, Default)]
pub struct WindowRegistry {
    windows: Vec<DockWindowDescriptor>,
    index: HashMap<String, usize>,
}

impl WindowRegistry {
    pub fn register(&mut self, descriptor: DockWindowDescriptor) {
        let idx = self.windows.len();
        self.index.insert(descriptor.id.clone(), idx);
        self.windows.push(descriptor);
    }

    /// Remove a window by id. Returns true if the window was found.
    /// Rebuilds the id -> index mapping after removal.
    pub fn unregister(&mut self, id: &str) -> bool {
        let Some(idx) = self.index.remove(id) else {
            return false;
        };
        self.windows.remove(idx);
        // Re-index remaining entries since positions shifted.
        self.index.clear();
        for (i, w) in self.windows.iter().enumerate() {
            self.index.insert(w.id.clone(), i);
        }
        true
    }

    pub fn get(&self, id: &str) -> Option<&DockWindowDescriptor> {
        self.index.get(id).map(|&i| &self.windows[i])
    }

    pub fn by_area(&self, area: &str) -> Vec<&DockWindowDescriptor> {
        let mut result: Vec<&DockWindowDescriptor> = self
            .windows
            .iter()
            .filter(|w| w.default_area == area)
            .collect();
        result.sort_by_key(|w| w.priority);
        result
    }

    pub fn iter(&self) -> impl Iterator<Item = &DockWindowDescriptor> {
        self.windows.iter()
    }
}
