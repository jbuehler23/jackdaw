//! PIE outliner/inspector view-mode types and per-instance state buffers.
//!
//! [`PieViewMode`] tracks whether the outliner and inspector panels show the
//! authored scene or the running game's live data; it resets to `Scene` when
//! play stops. [`PieInstances`] holds one [`InstanceBuffer`] per running game so
//! a focus switch can re-project that instance's accumulated state into the
//! preview world (see `pie_projection`).

use std::collections::HashMap;

use bevy::prelude::*;
use jackdaw_pie_protocol::RemoteEntity;

/// Which data source the outliner and inspector panels display.
///
/// `Scene` shows the authored scene as normal. `Live` shows data streamed from
/// the running game. Resets to `Scene` when play stops.
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
    /// Mirrors the spawn/change/despawn handling of the live instance set
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
mod instance_buffer_tests {
    use super::*;
    use crate::pie::InstanceKey;
    use jackdaw_pie_protocol::StateEvent;

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
