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
    /// Raw frame-view pixel messages (see `crate::frame`); never JSON.
    Frames,
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
    /// The game's cursor options changed (mouse-look grabs); the editor
    /// mirrors this onto its own cursor while input capture is engaged.
    CursorState {
        grabbed: bool,
        visible: bool,
    },
    /// Answer to [`ControlEvent::Pick`]: the topmost streamable entity under
    /// the forwarded cursor, or `None` when nothing inspectable is hit.
    PickResult {
        entity: Option<u64>,
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
    /// Begin (or resize) streaming rendered frames at the given pixel size.
    /// The game clamps to its own limits; no frames flow until this arrives.
    StartFrameStream {
        width: u32,
        height: u32,
    },
    /// Stop streaming rendered frames and release the capture resources.
    StopFrameStream,
    /// Forwarded editor input while the Live viewport has input capture.
    Input(crate::input::PieInputEvent),
    /// Ask what is under the forwarded cursor. The game answers with
    /// [`StateEvent::PickResult`] using its own picking backend, so picking
    /// works in any game state and needs no editor-side camera alignment.
    Pick,
    /// Box the given entity in the game's own render (gizmo bounds), or
    /// clear the highlight with `None`. Sent when the editor selection
    /// changes while an instance is focused.
    Highlight {
        entity: Option<u64>,
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

    #[test]
    fn input_event_round_trips() {
        use crate::input::PieInputEvent;
        use bevy::input::keyboard::{Key, KeyCode};
        let ev = ControlEvent::Input(PieInputEvent::Key {
            key: KeyCode::KeyW,
            logical: Key::Character("w".into()),
            pressed: true,
            repeat: false,
        });
        let bytes = to_bytes(&ev).unwrap();
        assert_eq!(from_bytes::<ControlEvent>(&bytes).unwrap(), ev);

        let ev = ControlEvent::Input(PieInputEvent::CursorMoved {
            position: bevy::math::Vec2::new(640.5, 360.0),
        });
        let bytes = to_bytes(&ev).unwrap();
        assert_eq!(from_bytes::<ControlEvent>(&bytes).unwrap(), ev);
    }

    #[test]
    fn cursor_state_round_trips() {
        let ev = StateEvent::CursorState {
            grabbed: true,
            visible: false,
        };
        let bytes = to_bytes(&ev).unwrap();
        assert_eq!(from_bytes::<StateEvent>(&bytes).unwrap(), ev);
    }

    #[test]
    fn pick_events_round_trip() {
        let ev = ControlEvent::Pick;
        let bytes = to_bytes(&ev).unwrap();
        assert_eq!(from_bytes::<ControlEvent>(&bytes).unwrap(), ev);

        let ev = StateEvent::PickResult { entity: Some(42) };
        let bytes = to_bytes(&ev).unwrap();
        assert_eq!(from_bytes::<StateEvent>(&bytes).unwrap(), ev);

        let ev = StateEvent::PickResult { entity: None };
        let bytes = to_bytes(&ev).unwrap();
        assert_eq!(from_bytes::<StateEvent>(&bytes).unwrap(), ev);
    }

    #[test]
    fn highlight_events_round_trip() {
        let ev = ControlEvent::Highlight { entity: Some(7) };
        let bytes = to_bytes(&ev).unwrap();
        assert_eq!(from_bytes::<ControlEvent>(&bytes).unwrap(), ev);

        let ev = ControlEvent::Highlight { entity: None };
        let bytes = to_bytes(&ev).unwrap();
        assert_eq!(from_bytes::<ControlEvent>(&bytes).unwrap(), ev);
    }
}
