use serde::{Deserialize, Serialize};

use crate::snapshot::RemoteEntity;

/// Delivery channel for a message.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum PieChannel {
    Reliable,
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
    /// The game's cursor options changed (mouse-look grabs); the editor
    /// mirrors this onto its own cursor while input capture is engaged.
    CursorState {
        grabbed: bool,
        visible: bool,
    },
    /// A scene root needs to spawn the scene at `path`, relative to the asset
    /// folder. The editor replies with [`ControlEvent::SceneDocument`] carrying
    /// the same `request`.
    RequestScene {
        request: u64,
        path: String,
    },
}

/// What the game spawns a requested scene from.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum SceneAnswer {
    /// The editor's current document. `dir` is the folder it counts as being
    /// in, relative to the asset folder, and `stem` the name it goes by.
    Document {
        text: String,
        dir: String,
        stem: String,
    },
    /// Load this file, relative to the asset folder.
    UseFile { path: String },
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
    /// Box the given entity in the game's own render (gizmo bounds), or
    /// clear the highlight with `None`. Sent when the editor selection
    /// changes while an instance is focused.
    Highlight {
        entity: Option<u64>,
    },
    /// The authored components of one scene node changed in the editor's
    /// document. `set` holds the full current value of each changed
    /// component, keyed by type path; `removed` names the components the node
    /// no longer has. The game applies both to every entity spawned from the
    /// node, matched by its `SceneNodeId`.
    NodeComponents {
        node: u64,
        set: Vec<(String, serde_json::Value)>,
        removed: Vec<String>,
    },
    /// The reply to a [`StateEvent::RequestScene`].
    SceneDocument {
        request: u64,
        answer: SceneAnswer,
    },
    /// Named asset definitions, as the roots a served document embeds them with.
    /// Sent ahead of an edit naming an asset this game has not been given.
    SceneAssets {
        text: String,
    },
    /// Asset files the game may not hold as the editor does, as `(path, text)`,
    /// each text holding the file's one asset root. Sent ahead of anything naming
    /// the path, and when the asset changes.
    AssetFiles {
        files: Vec<(String, String)>,
    },
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
    fn state_event_round_trips() {
        let ev = StateEvent::CursorState {
            grabbed: true,
            visible: false,
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
    fn node_components_round_trip() {
        let ev = ControlEvent::NodeComponents {
            node: 1 << 40,
            set: vec![
                (
                    "bevy_transform::components::transform::Transform".to_string(),
                    serde_json::json!({
                        "translation": [1.0, 2.0, 3.0],
                        "rotation": [0.0, 0.0, 0.0, 1.0],
                        "scale": [1.0, 1.0, 1.0]
                    }),
                ),
                (
                    "bevy_ecs::name::Name".to_string(),
                    serde_json::json!("Rock"),
                ),
            ],
            removed: vec!["game::Health".to_string()],
        };
        let bytes = to_bytes(&ev).unwrap();
        assert_eq!(from_bytes::<ControlEvent>(&bytes).unwrap(), ev);
    }

    #[test]
    fn a_scene_request_and_its_answers_round_trip() {
        let ev = StateEvent::RequestScene {
            request: 3,
            path: "levels/boss#2.bsn".to_string(),
        };
        let bytes = to_bytes(&ev).unwrap();
        assert_eq!(from_bytes::<StateEvent>(&bytes).unwrap(), ev);

        for answer in [
            SceneAnswer::Document {
                text: "#Zone\n".to_string(),
                dir: "levels".to_string(),
                stem: "zone".to_string(),
            },
            SceneAnswer::UseFile {
                path: "scene.bsn".to_string(),
            },
        ] {
            let ev = ControlEvent::SceneDocument { request: 3, answer };
            let bytes = to_bytes(&ev).unwrap();
            assert_eq!(from_bytes::<ControlEvent>(&bytes).unwrap(), ev);
        }
    }

    #[test]
    fn scene_assets_round_trip() {
        let ev = ControlEvent::SceneAssets {
            text: "#Stone\ngame::Swatch\n".to_string(),
        };
        let bytes = to_bytes(&ev).unwrap();
        assert_eq!(from_bytes::<ControlEvent>(&bytes).unwrap(), ev);
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
