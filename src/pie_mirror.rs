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
    /// Bumped only when the entity set changes (an entity is added or removed),
    /// never on a component value update. The Live outliner rebuilds on this so
    /// the constant stream of value deltas does not rebuild its rows every frame
    /// (which flickers and steals clicks).
    pub structure_generation: u64,
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
                if self
                    .entities
                    .insert(bits, PieMirrorEntry::from(entity))
                    .is_none()
                {
                    self.structure_generation += 1;
                }
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
                if self.entities.remove(&entity).is_some() {
                    self.structure_generation += 1;
                }
            }
            StateEvent::Status { .. }
            | StateEvent::Log { .. }
            | StateEvent::CursorState { .. }
            | StateEvent::PickResult { .. } => {}
        }
    }

    /// Remove all cached entities. Called when play stops.
    pub fn clear(&mut self) {
        if !self.entities.is_empty() {
            self.structure_generation += 1;
        }
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

/// Accumulated stream snapshot for one running instance. Tracks the last
/// known state of every entity reported by that instance so the editor can
/// re-project the buffer when focus switches.
#[derive(Default, Debug)]
pub struct InstanceBuffer {
    pub entities: HashMap<u64, PieMirrorEntry>,
}

impl InstanceBuffer {
    /// Accumulate one event into this instance's snapshot.
    ///
    /// Mirrors the spawn/change/despawn handling of [`PieMirror::apply`]
    /// without the `structure_generation` counter (nothing renders from the
    /// buffer directly). `Status` and `Log` events are ignored.
    pub fn apply(&mut self, event: &jackdaw_pie_protocol::StateEvent) {
        match event {
            jackdaw_pie_protocol::StateEvent::EntitySpawned { entity } => {
                self.entities
                    .insert(entity.entity, PieMirrorEntry::from(entity.clone()));
            }
            jackdaw_pie_protocol::StateEvent::ComponentChanged {
                entity,
                type_path,
                value,
            } => {
                if let Some(entry) = self.entities.get_mut(entity) {
                    entry.components.insert(type_path.clone(), value.clone());
                }
            }
            jackdaw_pie_protocol::StateEvent::EntityDespawned { entity } => {
                self.entities.remove(entity);
            }
            jackdaw_pie_protocol::StateEvent::Status { .. }
            | jackdaw_pie_protocol::StateEvent::Log { .. }
            | jackdaw_pie_protocol::StateEvent::CursorState { .. }
            | jackdaw_pie_protocol::StateEvent::PickResult { .. } => {}
        }
    }
}

/// All running instances' per-instance buffers and the currently focused
/// instance key. Only the focused instance's events are projected into the
/// preview world each frame.
#[derive(Resource, Default, Debug)]
pub struct PieInstances {
    pub buffers: HashMap<crate::pie::InstanceKey, InstanceBuffer>,
    /// The instance currently projected into the preview world. Set to the
    /// first instance seen when play starts, and cleared on stop.
    pub focused: Option<crate::pie::InstanceKey>,
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

    #[test]
    fn structure_generation_bumps_only_on_set_changes() {
        let mut mirror = PieMirror::default();
        assert_eq!(mirror.structure_generation, 0);

        // A new entity changes the set.
        mirror.apply(StateEvent::EntitySpawned {
            entity: make_entity(1),
        });
        assert_eq!(mirror.structure_generation, 1);

        // A component value update is the per-frame stream; it must not bump.
        mirror.apply(StateEvent::ComponentChanged {
            entity: 1,
            type_path: "bevy_transform::components::transform::Transform".to_string(),
            value: serde_json::json!({"translation": [9.0, 9.0, 9.0]}),
        });
        assert_eq!(mirror.structure_generation, 1);

        // Re-spawning the same id replaces the entry without changing the set.
        mirror.apply(StateEvent::EntitySpawned {
            entity: make_entity(1),
        });
        assert_eq!(mirror.structure_generation, 1);

        // A second distinct entity changes the set.
        mirror.apply(StateEvent::EntitySpawned {
            entity: make_entity(2),
        });
        assert_eq!(mirror.structure_generation, 2);

        // Despawning a present entity changes the set; an absent one does not.
        mirror.apply(StateEvent::EntityDespawned { entity: 2 });
        assert_eq!(mirror.structure_generation, 3);
        mirror.apply(StateEvent::EntityDespawned { entity: 2 });
        assert_eq!(mirror.structure_generation, 3);
    }
}

#[cfg(test)]
mod instance_buffer_tests {
    use super::*;
    use crate::pie::InstanceKey;

    fn server_key() -> InstanceKey {
        InstanceKey {
            config: "Server".into(),
            instance: 1,
        }
    }

    fn client_key() -> InstanceKey {
        InstanceKey {
            config: "Client".into(),
            instance: 1,
        }
    }

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
            scene_node_id: None,
        }
    }

    // Two instances applying EntitySpawned for the same entity bits hold
    // separate, independent buffers. No collision should occur.
    #[test]
    fn two_instances_same_bits_do_not_collide() {
        let mut instances = PieInstances::default();

        let server_buf = instances.buffers.entry(server_key()).or_default();
        server_buf.apply(&StateEvent::EntitySpawned {
            entity: make_entity(1),
        });

        let client_buf = instances.buffers.entry(client_key()).or_default();
        client_buf.apply(&StateEvent::EntitySpawned {
            entity: make_entity(1),
        });

        assert!(instances.buffers[&server_key()].entities.contains_key(&1));
        assert!(instances.buffers[&client_key()].entities.contains_key(&1));

        // Modifying one buffer does not affect the other.
        instances
            .buffers
            .entry(server_key())
            .or_default()
            .apply(&StateEvent::EntityDespawned { entity: 1 });

        assert!(
            !instances.buffers[&server_key()].entities.contains_key(&1),
            "server buffer should have removed entity 1"
        );
        assert!(
            instances.buffers[&client_key()].entities.contains_key(&1),
            "client buffer must not be affected by server despawn"
        );
    }

    // InstanceBuffer accumulates spawn -> component change -> despawn correctly.
    #[test]
    fn instance_buffer_apply_accumulation() {
        let mut buf = InstanceBuffer::default();

        buf.apply(&StateEvent::EntitySpawned {
            entity: make_entity(5),
        });
        assert!(buf.entities.contains_key(&5));

        buf.apply(&StateEvent::ComponentChanged {
            entity: 5,
            type_path: "bevy_transform::components::transform::Transform".to_string(),
            value: serde_json::json!({"translation": [1.0, 2.0, 3.0]}),
        });

        let val = &buf.entities[&5].components["bevy_transform::components::transform::Transform"];
        assert_eq!(val["translation"][0], 1.0);

        buf.apply(&StateEvent::EntityDespawned { entity: 5 });
        assert!(!buf.entities.contains_key(&5));
    }

    // ComponentChanged on an unknown entity is silently ignored (no entry created).
    #[test]
    fn component_changed_unknown_entity_is_ignored() {
        let mut buf = InstanceBuffer::default();
        buf.apply(&StateEvent::ComponentChanged {
            entity: 99,
            type_path: "some::Component".to_string(),
            value: serde_json::json!(null),
        });
        assert!(!buf.entities.contains_key(&99));
    }

    // Status and Log events are no-ops.
    #[test]
    fn status_and_log_are_ignored() {
        let mut buf = InstanceBuffer::default();
        buf.apply(&StateEvent::Status {
            mode: jackdaw_pie_protocol::event::PieMode::Play,
            ready: true,
        });
        buf.apply(&StateEvent::Log {
            level: "info".to_string(),
            message: "hello".to_string(),
        });
        assert!(buf.entities.is_empty());
    }

    // PieInstances starts with no focus and accumulates focus on first insert.
    #[test]
    fn pie_instances_starts_empty() {
        let instances = PieInstances::default();
        assert!(instances.focused.is_none());
        assert!(instances.buffers.is_empty());
    }
}
