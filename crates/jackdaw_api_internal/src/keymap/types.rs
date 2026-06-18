//! Serde/preset data types and name<->code helpers.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// One input trigger: a keyboard key chord, a mouse button chord, or a
/// scroll tick. Key code names follow the Bevy `KeyCode` variant spelling,
/// e.g. `"Digit1"`, `"KeyK"`, `"Escape"`, `"F9"`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PresetInput {
    Key {
        /// Bevy `KeyCode` name, e.g. `"Digit1"`, `"KeyK"`, `"Escape"`.
        /// Stored as a string so preset files stay readable and stable
        /// across enum reordering.
        key: String,
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        ctrl: bool,
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        shift: bool,
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        alt: bool,
    },
    /// Mouse button, optionally combined with modifier keys.
    /// `button` is one of `"Left"`, `"Right"`, `"Middle"`, `"Back"`, `"Forward"`.
    MouseButton {
        button: String,
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        ctrl: bool,
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        shift: bool,
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        alt: bool,
    },
    /// One wheel tick; `up: false` is a downward tick.
    Scroll {
        up: bool,
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        ctrl: bool,
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        shift: bool,
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        alt: bool,
    },
}

impl PresetInput {
    pub fn key(name: &str) -> Self {
        Self::Key {
            key: name.to_string(),
            ctrl: false,
            shift: false,
            alt: false,
        }
    }

    /// Construct a mouse-button input. `button` must be one of
    /// `"Left"`, `"Right"`, `"Middle"`, `"Back"`, `"Forward"`.
    pub fn mouse(button: &str) -> Self {
        Self::MouseButton {
            button: button.to_string(),
            ctrl: false,
            shift: false,
            alt: false,
        }
    }

    /// Construct a scroll-wheel input.
    pub fn scroll(up: bool) -> Self {
        Self::Scroll {
            up,
            ctrl: false,
            shift: false,
            alt: false,
        }
    }

    /// Set the Ctrl modifier.
    pub fn ctrl(mut self) -> Self {
        match &mut self {
            Self::Key { ctrl, .. } | Self::MouseButton { ctrl, .. } | Self::Scroll { ctrl, .. } => {
                *ctrl = true;
            }
        }
        self
    }

    /// Set the Shift modifier.
    pub fn shift(mut self) -> Self {
        match &mut self {
            Self::Key { shift, .. }
            | Self::MouseButton { shift, .. }
            | Self::Scroll { shift, .. } => *shift = true,
        }
        self
    }

    /// Set the Alt modifier.
    pub fn alt(mut self) -> Self {
        match &mut self {
            Self::Key { alt, .. } | Self::MouseButton { alt, .. } | Self::Scroll { alt, .. } => {
                *alt = true;
            }
        }
        self
    }
}

/// Parse a mouse button from the preset name string. Returns `None` for
/// `"Other"` or any unrecognised name.
pub fn mouse_button_from_name(name: &str) -> Option<MouseButton> {
    match name {
        "Left" => Some(MouseButton::Left),
        "Right" => Some(MouseButton::Right),
        "Middle" => Some(MouseButton::Middle),
        "Back" => Some(MouseButton::Back),
        "Forward" => Some(MouseButton::Forward),
        _ => None,
    }
}

/// Display-stable name for a `MouseButton`. Returns `None` for
/// `MouseButton::Other(_)`, which the preset format does not support.
pub fn mouse_button_name(button: MouseButton) -> Option<String> {
    match button {
        MouseButton::Left => Some("Left".to_string()),
        MouseButton::Right => Some("Right".to_string()),
        MouseButton::Middle => Some("Middle".to_string()),
        MouseButton::Back => Some("Back".to_string()),
        MouseButton::Forward => Some("Forward".to_string()),
        MouseButton::Other(_) => None,
    }
}

/// When the binding fires.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PresetPhase {
    #[default]
    Press,
    Release,
    DoubleClick,
    Tap,
}

impl PresetPhase {
    /// Used by serde to omit the default phase from generated files.
    pub fn is_press(&self) -> bool {
        matches!(self, Self::Press)
    }
}

/// Which action set an entry binds into. `Operators` resolves through
/// the `OperatorAction` id tag; `Modal` and `Navigation` resolve through
/// the `BuiltinActions` registry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PresetContext {
    #[default]
    Operators,
    Modal,
    Navigation,
}

impl PresetContext {
    /// Used by serde to omit the default context from generated files.
    pub fn is_operators(&self) -> bool {
        matches!(self, Self::Operators)
    }
}

/// One preset entry: an input chord bound to an operator id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetBinding {
    pub operator: String,
    pub input: PresetInput,
    #[serde(default, skip_serializing_if = "PresetPhase::is_press")]
    pub phase: PresetPhase,
    #[serde(default, skip_serializing_if = "PresetContext::is_operators")]
    pub context: PresetContext,
}

/// A complete keymap preset document.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeymapPreset {
    pub name: String,
    pub bindings: Vec<PresetBinding>,
}

/// Defaults recorded by `ExtensionContext::bind_operator` during
/// extension registration. The classic preset is generated from this,
/// so it can never drift from what extensions declare.
#[derive(Resource, Default)]
pub struct DefaultKeymap {
    pub bindings: Vec<PresetBinding>,
}

/// Registry mapping builtin action names (e.g. `"modal.confirm"`) to the
/// action entities that the keymap applier binds into.
///
/// Populated by `input_contexts::spawn_contexts` at startup, before
/// `apply_active_keymap` runs. The Modal and Navigation arms in
/// `apply_keymap_preset` resolve entries here just like Operators entries
/// resolve through `OperatorAction`. Unknown names land in
/// `skipped_unknown_operator` (same slot; the semantics are identical: a
/// preset entry naming something that does not exist).
#[derive(Resource, Default)]
pub struct BuiltinActions {
    pub(super) map: std::collections::HashMap<String, Vec<Entity>>,
}

impl BuiltinActions {
    /// Register `name` as owning `entity`. May be called multiple times with
    /// the same name to accumulate multiple entities (analogous to multiple
    /// action entities per operator).
    pub fn register(&mut self, name: impl Into<String>, entity: Entity) {
        self.map.entry(name.into()).or_default().push(entity);
    }

    /// Look up the entities registered under `name`.
    pub fn get(&self, name: &str) -> Option<&[Entity]> {
        self.map.get(name).map(Vec::as_slice)
    }
}

impl DefaultKeymap {
    /// Snapshot the recorded defaults as the "classic" preset.
    pub fn to_classic_preset(&self) -> KeymapPreset {
        KeymapPreset {
            name: "classic".into(),
            bindings: self.bindings.clone(),
        }
    }
}

/// Which preset is active. Persisted as plain JSON in the user config
/// directory next to the keybinds file. Only "classic" exists today;
/// the file format is the contract future presets and the settings UI
/// build on.
#[derive(Resource, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveKeymapPreset {
    pub name: String,
}

impl Default for ActiveKeymapPreset {
    fn default() -> Self {
        Self {
            name: "classic".into(),
        }
    }
}

/// Parse the `KeyCode` named by a preset entry. Returns `None` for
/// unknown names so a typo in a preset file degrades to an unbound
/// operator plus a warning instead of a panic.
pub fn key_code_from_name(name: &str) -> Option<KeyCode> {
    serde_json::from_value(serde_json::Value::String(name.to_string())).ok()
}

/// Display-stable name for a `KeyCode`. Inverse of `key_code_from_name`
/// for all named keys; the `Unidentified` platform variant falls back to
/// its debug form, which does not parse back and degrades to warn-and-skip.
pub fn key_code_name(key: KeyCode) -> String {
    match serde_json::to_value(key) {
        Ok(serde_json::Value::String(s)) => s,
        _ => format!("{key:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_round_trips_through_json() {
        let preset = KeymapPreset {
            name: "classic".into(),
            bindings: vec![
                PresetBinding {
                    operator: "edit_mode.vertex".into(),
                    input: PresetInput::key("Digit1"),
                    phase: PresetPhase::Press,
                    context: PresetContext::Operators,
                },
                PresetBinding {
                    operator: "history.undo".into(),
                    input: PresetInput::key("KeyZ").ctrl(),
                    phase: PresetPhase::Press,
                    context: PresetContext::Operators,
                },
                PresetBinding {
                    operator: "view.orbit".into(),
                    input: PresetInput::mouse("Middle"),
                    phase: PresetPhase::Press,
                    context: PresetContext::Operators,
                },
                PresetBinding {
                    operator: "view.zoom".into(),
                    input: PresetInput::scroll(true).ctrl(),
                    phase: PresetPhase::Press,
                    context: PresetContext::Operators,
                },
                PresetBinding {
                    operator: "select.deselect".into(),
                    input: PresetInput::key("Escape"),
                    phase: PresetPhase::Release,
                    context: PresetContext::Operators,
                },
                PresetBinding {
                    operator: "modal.confirm".into(),
                    input: PresetInput::key("Enter"),
                    phase: PresetPhase::Press,
                    context: PresetContext::Modal,
                },
            ],
        };
        let json = serde_json::to_string_pretty(&preset).expect("serialize");
        let back: KeymapPreset = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(preset, back);
    }

    #[test]
    fn key_code_names_round_trip() {
        for key in [KeyCode::Digit1, KeyCode::KeyK, KeyCode::Escape, KeyCode::F9] {
            let name = key_code_name(key);
            assert_eq!(
                key_code_from_name(&name),
                Some(key),
                "round trip failed for {name}"
            );
        }
    }

    #[test]
    fn unknown_key_name_is_none_not_panic() {
        assert_eq!(key_code_from_name("NotAKey"), None);
    }

    #[test]
    fn default_keymap_snapshot_is_classic() {
        let mut defaults = DefaultKeymap::default();
        defaults.bindings.push(PresetBinding {
            operator: "tool.select".into(),
            input: PresetInput::key("KeyQ"),
            phase: PresetPhase::Press,
            context: PresetContext::Operators,
        });
        let preset = defaults.to_classic_preset();
        assert_eq!(preset.name, "classic");
        assert_eq!(preset.bindings, defaults.bindings);
    }

    #[test]
    fn serialized_shape_is_the_documented_contract() {
        let binding = PresetBinding {
            operator: "history.undo".into(),
            input: PresetInput::key("KeyZ").ctrl(),
            phase: PresetPhase::Press,
            context: PresetContext::Operators,
        };
        let json = serde_json::to_string(&binding).expect("serialize");
        assert_eq!(
            json,
            r#"{"operator":"history.undo","input":{"type":"Key","key":"KeyZ","ctrl":true}}"#
        );
    }

    #[test]
    fn mouse_button_golden_shape() {
        let binding = PresetBinding {
            operator: "x".into(),
            input: PresetInput::mouse("Right"),
            phase: PresetPhase::Press,
            context: PresetContext::Operators,
        };
        let json = serde_json::to_string(&binding).expect("serialize");
        assert_eq!(
            json,
            r#"{"operator":"x","input":{"type":"MouseButton","button":"Right"}}"#
        );
    }

    #[test]
    fn minimal_handwritten_json_parses_with_defaults() {
        let json = r#"{"operator":"tool.select","input":{"type":"Key","key":"KeyQ"}}"#;
        let binding: PresetBinding = serde_json::from_str(json).expect("minimal JSON must parse");
        assert_eq!(binding.phase, PresetPhase::Press);
        assert_eq!(binding.context, PresetContext::Operators);
        assert_eq!(
            binding.input,
            PresetInput::key("KeyQ"),
            "omitted modifiers must default to false"
        );
    }

    #[test]
    fn mouse_button_name_round_trips() {
        for (name, button) in [
            ("Left", MouseButton::Left),
            ("Right", MouseButton::Right),
            ("Middle", MouseButton::Middle),
            ("Back", MouseButton::Back),
            ("Forward", MouseButton::Forward),
        ] {
            assert_eq!(
                mouse_button_from_name(name),
                Some(button),
                "from_name failed for {name}"
            );
            assert_eq!(
                mouse_button_name(button).as_deref(),
                Some(name),
                "to_name failed for {name}"
            );
        }
        assert_eq!(
            mouse_button_from_name("Other"),
            None,
            "Other must return None"
        );
        assert_eq!(
            mouse_button_name(MouseButton::Other(42)),
            None,
            "Other(_) must return None"
        );
    }
}
