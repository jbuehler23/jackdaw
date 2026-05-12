use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::layout::LayoutState;
use crate::tree::DockTree;

pub struct WorkspaceDescriptor {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub accent_color: Color,
    /// Legacy field. No longer applied; kept so older callers still
    /// compile. The live layout lives in `tree`.
    pub layout: LayoutState,
    /// Per-workspace dock tree. An empty default is seeded on first
    /// activation by the editor's normal init flow.
    pub tree: DockTree,
}

#[derive(Resource, Default)]
pub struct WorkspaceRegistry {
    pub workspaces: Vec<WorkspaceDescriptor>,
    pub active: Option<String>,
}

impl WorkspaceRegistry {
    pub fn register(&mut self, descriptor: WorkspaceDescriptor) {
        if self.active.is_none() {
            self.active = Some(descriptor.id.clone());
        }
        self.workspaces.push(descriptor);
    }

    /// Remove a workspace by id. If it was the active workspace, falls
    /// back to the first remaining workspace (or `None` if none remain).
    /// Returns true if a workspace was removed.
    pub fn unregister(&mut self, id: &str) -> bool {
        let Some(idx) = self.workspaces.iter().position(|w| w.id == id) else {
            return false;
        };
        self.workspaces.remove(idx);
        if self.active.as_deref() == Some(id) {
            self.active = self.workspaces.first().map(|w| w.id.clone());
        }
        true
    }

    pub fn get(&self, id: &str) -> Option<&WorkspaceDescriptor> {
        self.workspaces.iter().find(|w| w.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut WorkspaceDescriptor> {
        self.workspaces.iter_mut().find(|w| w.id == id)
    }

    pub fn active_workspace(&self) -> Option<&WorkspaceDescriptor> {
        self.active.as_ref().and_then(|id| self.get(id))
    }

    pub fn set_active(&mut self, id: &str) {
        if self.workspaces.iter().any(|w| w.id == id) {
            self.active = Some(id.to_string());
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &WorkspaceDescriptor> {
        self.workspaces.iter()
    }
}

#[derive(Component)]
pub struct WorkspaceTabStrip;

#[derive(Component)]
pub struct WorkspaceTab {
    pub workspace_id: String,
}

#[derive(Event, Clone, Debug)]
pub struct WorkspaceChanged {
    pub old: Option<String>,
    pub new: String,
}

/// Serializable snapshot of every workspace in the registry, suitable
/// for round-tripping through `project.jsn`. Each workspace owns its
/// full `DockTree` (Blender's model: each workspace owns its layout
/// independently).
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct WorkspacesPersist {
    pub active: Option<String>,
    pub workspaces: Vec<WorkspacePersist>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WorkspacePersist {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub icon: Option<String>,
    pub accent_color: [f32; 4],
    #[serde(default)]
    pub tree: DockTree,
}

impl WorkspacesPersist {
    pub fn from_registry(registry: &WorkspaceRegistry) -> Self {
        Self {
            active: registry.active.clone(),
            workspaces: registry
                .workspaces
                .iter()
                .map(|w| {
                    let s = w.accent_color.to_srgba();
                    WorkspacePersist {
                        id: w.id.clone(),
                        name: w.name.clone(),
                        icon: w.icon.clone(),
                        accent_color: [s.red, s.green, s.blue, s.alpha],
                        tree: w.tree.clone(),
                    }
                })
                .collect(),
        }
    }

    /// Merge persisted workspaces into the live registry. Built-in
    /// workspaces registered at startup are preserved, with their
    /// `tree` (and accent / icon / name overrides) updated from the
    /// persist if a matching id is present. Workspaces only in the
    /// persist (e.g. user-created ones via the `+` tab) are appended.
    /// New built-in workspaces added in editor updates that the user
    /// has never seen show up alongside their persisted ones rather
    /// than being silently dropped.
    pub fn apply_to_registry(&self, registry: &mut WorkspaceRegistry) {
        // First, update existing built-in workspaces' tree / metadata
        // from the persist where ids match.
        for persist in &self.workspaces {
            if let Some(existing) = registry.get_mut(&persist.id) {
                existing.tree = persist.tree.clone();
                existing.name = persist.name.clone();
                existing.icon = persist.icon.clone();
                existing.accent_color = Color::srgba(
                    persist.accent_color[0],
                    persist.accent_color[1],
                    persist.accent_color[2],
                    persist.accent_color[3],
                );
            }
        }
        // Then append any workspaces that exist only in the persist
        // (user-created ones).
        for persist in &self.workspaces {
            if registry.get(&persist.id).is_none() {
                registry.workspaces.push(WorkspaceDescriptor {
                    id: persist.id.clone(),
                    name: persist.name.clone(),
                    icon: persist.icon.clone(),
                    accent_color: Color::srgba(
                        persist.accent_color[0],
                        persist.accent_color[1],
                        persist.accent_color[2],
                        persist.accent_color[3],
                    ),
                    layout: LayoutState::default(),
                    tree: persist.tree.clone(),
                });
            }
        }
        if let Some(active) = &self.active
            && registry.get(active).is_some()
        {
            registry.active = Some(active.clone());
        }
    }
}
