use serde::{Deserialize, Serialize};

use crate::snapshot::RemoteEntity;

/// Which mode the editor launched the game in.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PieMode {
    #[default]
    Play,
    EditorPreview,
}

/// Delivery channel for a message. Reliable-ordered for control and
/// discrete changes; unreliable for high-frequency state.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum PieChannel {
    Reliable,
    Unreliable,
}

/// Game-to-editor messages.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum StateEvent {
    EntitySpawned {
        entity: RemoteEntity,
    },
    ComponentChanged {
        entity: u64,
        type_path: String,
        value: serde_json::Value,
    },
    EntityDespawned {
        entity: u64,
    },
    Status {
        mode: PieMode,
        ready: bool,
    },
    Log {
        level: String,
        message: String,
    },
}

/// Editor-to-game messages.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum ControlEvent {
    Pause,
    Resume,
    Stop,
    /// Replace an existing component on `entity` with a new value deserialized
    /// from `value`. The component must already be present; if not, the game
    /// side logs a warning and skips.
    SetComponent {
        entity: u64,
        type_path: String,
        value: serde_json::Value,
    },
    /// Insert a component onto `entity`. If the component is already present it
    /// is replaced.
    AddComponent {
        entity: u64,
        type_path: String,
        value: serde_json::Value,
    },
    /// Remove a component from `entity`. If the component is absent the event
    /// is silently ignored.
    RemoveComponent {
        entity: u64,
        type_path: String,
    },
}

/// Either direction, for transports that carry a single type.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum PieEvent {
    State(StateEvent),
    Control(ControlEvent),
}

/// Serialize a protocol message to bytes. Uses JSON so component payloads
/// (`serde_json::Value`) round-trip cleanly; swap the codec here to avoid
/// touching call sites.
pub fn to_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(value)
}

/// Deserialize a protocol message from bytes.
pub fn from_bytes<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, serde_json::Error> {
    serde_json::from_slice(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_event_round_trips_through_bytes() {
        let ev = ControlEvent::Pause;
        let bytes = to_bytes(&ev).unwrap();
        let back: ControlEvent = from_bytes(&bytes).unwrap();
        assert_eq!(back, ControlEvent::Pause);
    }

    #[test]
    fn state_event_status_round_trips() {
        let ev = StateEvent::Status {
            mode: PieMode::Play,
            ready: true,
        };
        let bytes = to_bytes(&ev).unwrap();
        let back: StateEvent = from_bytes(&bytes).unwrap();
        assert_eq!(back, ev);
    }
}
