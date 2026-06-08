//! Live entity cache populated by PIE state-event streams.
//!
//! [`PieMirror`] holds one entry per entity reported by the running game,
//! keyed by entity bits (`u64`). Each entry carries the same fields as
//! [`RemoteEntity`]: a component map and an optional scene-node id.
//!
//! The cache is updated incrementally as [`StateEvent`]s arrive from
//! the game process and cleared when play stops.
//!
//! [`PieViewMode`] tracks whether the outliner and inspector panels show
//! the authored scene or the live mirror data. It resets to `Scene` when
//! play stops.

use std::collections::HashMap;

use bevy::prelude::*;
use jackdaw_pie_protocol::{RemoteEntity, StateEvent};

/// Which data source the outliner and inspector panels display.
///
/// `Scene` shows the authored scene as normal. `Live` shows data from the
/// running game via [`PieMirror`]. Resets to `Scene` when play stops.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PieViewMode {
    #[default]
    Scene,
    Live,
}

/// Which segment of the Scene/Live toggle a UI button represents.
///
/// Carried as a component on each segment button. The click observer reads
/// this to know which mode to activate, and the appearance system reads it
/// to decide active/inactive styling.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PieViewSegment {
    Scene,
    Live,
}

/// Marker on the header row container for both the outliner and inspector.
///
/// The live-accent system queries this to tint the header when
/// [`PieViewMode`] is `Live`.
#[derive(Component)]
pub struct PieViewHeader;

/// One entity held in the PIE mirror cache.
///
/// Stored inline rather than as `RemoteEntity` so the map owns its data
/// without nesting a redundant bits field inside each entry.
#[derive(Clone, Debug)]
pub struct PieMirrorEntry {
    pub components: HashMap<String, serde_json::Value>,
    pub scene_node_id: Option<u64>,
}

impl From<RemoteEntity> for PieMirrorEntry {
    fn from(r: RemoteEntity) -> Self {
        Self {
            components: r.components,
            scene_node_id: r.scene_node_id,
        }
    }
}

/// Resource holding the running game's entity state, keyed by entity bits.
///
/// Updated incrementally from [`StateEvent`]s produced by the game process.
/// Cleared when play stops (see [`PieMirror::clear`]).
#[derive(Resource, Default, Debug)]
pub struct PieMirror {
    pub entities: HashMap<u64, PieMirrorEntry>,
}

impl PieMirror {
    /// Apply one [`StateEvent`] to the cache.
    ///
    /// - `EntitySpawned` inserts or replaces the full entry (the game may
    ///   re-spawn the same entity id after a despawn without sending a
    ///   `EntityDespawned` first, so replace is always correct).
    /// - `ComponentChanged` updates one component. If the entity is not yet
    ///   present the event is dropped with a debug-level log rather than
    ///   creating a half-populated entry, because without the full component
    ///   set any consumer would see an incomplete snapshot.
    /// - `EntityDespawned` removes the entry; a no-op when absent.
    /// - `Status` / `Log` are ignored here; callers handle them separately.
    pub fn apply(&mut self, event: StateEvent) {
        match event {
            StateEvent::EntitySpawned { entity } => {
                let bits = entity.entity;
                self.entities.insert(bits, PieMirrorEntry::from(entity));
            }
            StateEvent::ComponentChanged {
                entity,
                type_path,
                value,
            } => {
                if let Some(entry) = self.entities.get_mut(&entity) {
                    entry.components.insert(type_path, value);
                } else {
                    debug!(
                        "PIE mirror: ComponentChanged for unknown entity {:x}, dropped",
                        entity
                    );
                }
            }
            StateEvent::EntityDespawned { entity } => {
                self.entities.remove(&entity);
            }
            StateEvent::Status { .. } | StateEvent::Log { .. } => {}
        }
    }

    /// Remove all cached entities. Called when play stops.
    pub fn clear(&mut self) {
        self.entities.clear();
    }
}

/// Type path of the `Name` component as it appears in the mirror's
/// component map. Game-side reflection serializes `Name` under this key.
pub const NAME_TYPE_PATH: &str = "bevy_ecs::name::Name";

/// Extract a display name from a mirror component map. Returns the
/// `Name` string when present, otherwise a fallback label built from the
/// entity bits so every row stays addressable even for unnamed entities.
pub fn mirror_entry_label(components: &HashMap<String, serde_json::Value>, bits: u64) -> String {
    components
        .get(NAME_TYPE_PATH)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("Entity {bits:X}"))
}

/// The game entity currently selected in the Live outliner, keyed by
/// entity bits. Distinct from [`crate::selection::Selection`], which
/// tracks the selected editor ECS entity; mirror entities are not editor
/// ECS entities. Cleared whenever [`PieMirror`] clears.
#[derive(Resource, Default, Debug)]
pub struct PieLiveSelection {
    pub selected: Option<u64>,
}

impl PieLiveSelection {
    /// Forget the current selection. Called alongside [`PieMirror::clear`].
    pub fn clear(&mut self) {
        self.selected = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entity(bits: u64) -> RemoteEntity {
        RemoteEntity {
            entity: bits,
            components: {
                let mut m = HashMap::new();
                m.insert(
                    "bevy_transform::components::transform::Transform".to_string(),
                    serde_json::json!({"translation": [0.0, 0.0, 0.0]}),
                );
                m
            },
            scene_node_id: Some(42),
        }
    }

    #[test]
    fn entity_spawned_inserts_entry() {
        let mut mirror = PieMirror::default();
        mirror.apply(StateEvent::EntitySpawned {
            entity: make_entity(1),
        });
        assert!(mirror.entities.contains_key(&1));
        assert_eq!(mirror.entities[&1].scene_node_id, Some(42));
        assert!(
            mirror.entities[&1]
                .components
                .contains_key("bevy_transform::components::transform::Transform")
        );
    }

    #[test]
    fn entity_spawned_replaces_existing_entry() {
        let mut mirror = PieMirror::default();
        mirror.apply(StateEvent::EntitySpawned {
            entity: make_entity(1),
        });

        let mut replacement = make_entity(1);
        replacement.scene_node_id = None;
        replacement.components.clear();
        replacement
            .components
            .insert("new::Type".to_string(), serde_json::json!({}));

        mirror.apply(StateEvent::EntitySpawned {
            entity: replacement,
        });

        assert_eq!(mirror.entities[&1].scene_node_id, None);
        assert!(mirror.entities[&1].components.contains_key("new::Type"));
    }

    #[test]
    fn component_changed_updates_component() {
        let mut mirror = PieMirror::default();
        mirror.apply(StateEvent::EntitySpawned {
            entity: make_entity(1),
        });
        mirror.apply(StateEvent::ComponentChanged {
            entity: 1,
            type_path: "bevy_transform::components::transform::Transform".to_string(),
            value: serde_json::json!({"translation": [1.0, 2.0, 3.0]}),
        });

        let val =
            &mirror.entities[&1].components["bevy_transform::components::transform::Transform"];
        assert_eq!(val["translation"][0], 1.0);
        assert_eq!(val["translation"][1], 2.0);
        assert_eq!(val["translation"][2], 3.0);
    }

    #[test]
    fn component_changed_on_unknown_entity_is_ignored() {
        let mut mirror = PieMirror::default();
        mirror.apply(StateEvent::ComponentChanged {
            entity: 99,
            type_path: "some::Component".to_string(),
            value: serde_json::json!(null),
        });
        assert!(!mirror.entities.contains_key(&99));
    }

    #[test]
    fn entity_despawned_removes_entry() {
        let mut mirror = PieMirror::default();
        mirror.apply(StateEvent::EntitySpawned {
            entity: make_entity(1),
        });
        assert!(mirror.entities.contains_key(&1));

        mirror.apply(StateEvent::EntityDespawned { entity: 1 });
        assert!(!mirror.entities.contains_key(&1));
    }

    #[test]
    fn entity_despawned_on_absent_entity_is_noop() {
        let mut mirror = PieMirror::default();
        mirror.apply(StateEvent::EntityDespawned { entity: 77 });
        assert!(mirror.entities.is_empty());
    }

    #[test]
    fn status_and_log_are_ignored() {
        let mut mirror = PieMirror::default();
        mirror.apply(StateEvent::Status {
            mode: jackdaw_pie_protocol::event::PieMode::Play,
            ready: true,
        });
        mirror.apply(StateEvent::Log {
            level: "info".to_string(),
            message: "hello".to_string(),
        });
        assert!(mirror.entities.is_empty());
    }

    #[test]
    fn mirror_entry_label_reads_name_component() {
        let mut components = HashMap::new();
        components.insert(NAME_TYPE_PATH.to_string(), serde_json::json!("Player"));
        assert_eq!(mirror_entry_label(&components, 7), "Player");
    }

    #[test]
    fn mirror_entry_label_falls_back_to_bits_when_unnamed() {
        let components = HashMap::new();
        assert_eq!(mirror_entry_label(&components, 0xAB), "Entity AB");
    }

    #[test]
    fn mirror_entry_label_falls_back_when_name_is_not_a_string() {
        let mut components = HashMap::new();
        components.insert(NAME_TYPE_PATH.to_string(), serde_json::json!(42));
        assert_eq!(mirror_entry_label(&components, 0x10), "Entity 10");
    }

    #[test]
    fn live_selection_clear_resets() {
        let mut sel = PieLiveSelection { selected: Some(5) };
        sel.clear();
        assert_eq!(sel.selected, None);
    }

    #[test]
    fn clear_removes_all_entries() {
        let mut mirror = PieMirror::default();
        mirror.apply(StateEvent::EntitySpawned {
            entity: make_entity(1),
        });
        mirror.apply(StateEvent::EntitySpawned {
            entity: make_entity(2),
        });
        assert_eq!(mirror.entities.len(), 2);

        mirror.clear();
        assert!(mirror.entities.is_empty());
    }
}
