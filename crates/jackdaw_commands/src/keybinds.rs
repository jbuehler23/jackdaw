use std::collections::HashMap;
use std::fmt;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Every remappable editor action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EditorAction {
    // File
    Undo,
    Redo,
    Save,
    SaveAs,
    Open,
    NewScene,

    // Entity operations
    Delete,
    Duplicate,
    CopyComponents,
    PasteComponents,
    ToggleVisibility,
    UnhideAll,
    HideAll,
    ResetPosition,
    ResetRotation,
    ResetScale,

    // Rotations (Alt + Arrow keys by default)
    Rotate90Left,
    Rotate90Right,
    Rotate90Up,
    Rotate90Down,
    Roll90Left,
    Roll90Right,

    // Nudge (Arrow keys / PageUp/Down by default)
    NudgeLeft,
    NudgeRight,
    NudgeForward,
    NudgeBack,
    NudgeUp,
    NudgeDown,

    // Brush edit modes
    VertexMode,
    EdgeMode,
    FaceMode,
    ClipMode,
    ExitEditMode,

    // Brush element deletion
    DeleteBrushElement,

    // Axis constraints (during drags)
    ConstrainX,
    ConstrainY,
    ConstrainZ,

    // Clip mode
    ClipCycleMode,
    ClipApply,
    ClipClear,

    // Draw brush
    DrawCut,
    ToggleDrawMode,
    AppendToBrush,
    ClosePolygon,
    RemoveLastVertex,
    CancelDraw,

    // CSG / Join
    JoinBrushes,
    CsgSubtract,
    CsgIntersect,
    ExtendFaceToBrush,

    // Tools
    ToolSelect,
    ToolTranslate,
    ToolRotate,
    ToolScale,
    ToggleGizmoSpace,

    // Viewport
    FocusSelected,

    // Grid
    DecreaseGrid,
    IncreaseGrid,

    // View modes
    ToggleWireframe,

    // Camera movement
    CameraForward,
    CameraBackward,
    CameraLeft,
    CameraRight,
    CameraUp,
    CameraDown,

    // Camera bookmarks
    SaveBookmark1,
    SaveBookmark2,
    SaveBookmark3,
    SaveBookmark4,
    SaveBookmark5,
    SaveBookmark6,
    SaveBookmark7,
    SaveBookmark8,
    SaveBookmark9,
    LoadBookmark1,
    LoadBookmark2,
    LoadBookmark3,
    LoadBookmark4,
    LoadBookmark5,
    LoadBookmark6,
    LoadBookmark7,
    LoadBookmark8,
    LoadBookmark9,

    // Modal transform (currently disabled, keybinds ready for future use)
    ModalGrab,
    ModalRotate,
    ModalScale,
    ModalConstrainX,
    ModalConstrainY,
    ModalConstrainZ,
    ModalConfirm,
    ModalCancel,
}

impl fmt::Display for EditorAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::Save => "Save",
            Self::SaveAs => "Save As",
            Self::Open => "Open",
            Self::NewScene => "New Scene",
            Self::Delete => "Delete",
            Self::Duplicate => "Duplicate",
            Self::CopyComponents => "Copy Components",
            Self::PasteComponents => "Paste Components",
            Self::ToggleVisibility => "Hide Selected",
            Self::UnhideAll => "Unhide All",
            Self::HideAll => "Hide All",
            Self::ResetPosition => "Reset Position",
            Self::ResetRotation => "Reset Rotation",
            Self::ResetScale => "Reset Scale",
            Self::Rotate90Left => "Rotate 90\u{b0} Left",
            Self::Rotate90Right => "Rotate 90\u{b0} Right",
            Self::Rotate90Up => "Rotate 90\u{b0} Up",
            Self::Rotate90Down => "Rotate 90\u{b0} Down",
            Self::Roll90Left => "Roll 90\u{b0} Left",
            Self::Roll90Right => "Roll 90\u{b0} Right",
            Self::NudgeLeft => "Nudge Left",
            Self::NudgeRight => "Nudge Right",
            Self::NudgeForward => "Nudge Forward",
            Self::NudgeBack => "Nudge Back",
            Self::NudgeUp => "Nudge Up",
            Self::NudgeDown => "Nudge Down",
            Self::VertexMode => "Vertex Mode",
            Self::EdgeMode => "Edge Mode",
            Self::FaceMode => "Face Mode",
            Self::ClipMode => "Clip Mode",
            Self::ExitEditMode => "Exit Edit Mode",
            Self::DeleteBrushElement => "Delete Element",
            Self::ConstrainX => "Constrain X",
            Self::ConstrainY => "Constrain Y",
            Self::ConstrainZ => "Constrain Z",
            Self::ClipCycleMode => "Clip Cycle Mode",
            Self::ClipApply => "Clip Apply",
            Self::ClipClear => "Clip Clear",
            Self::DrawCut => "Draw Brush (Cut)",
            Self::ToggleDrawMode => "Toggle Draw Mode",
            Self::AppendToBrush => "Append to Brush",
            Self::ClosePolygon => "Close Polygon",
            Self::RemoveLastVertex => "Remove Last Vertex",
            Self::CancelDraw => "Cancel Draw",
            Self::JoinBrushes => "Join Brushes",
            Self::CsgSubtract => "CSG Subtract",
            Self::CsgIntersect => "CSG Intersect",
            Self::ExtendFaceToBrush => "Extend Face to Brush",
            Self::ToolSelect => "Tool Select",
            Self::ToolTranslate => "Tool Translate",
            Self::ToolRotate => "Tool Rotate",
            Self::ToolScale => "Tool Scale",
            Self::ToggleGizmoSpace => "Toggle Gizmo Space",
            Self::FocusSelected => "Focus Selected",
            Self::DecreaseGrid => "Decrease Grid",
            Self::IncreaseGrid => "Increase Grid",
            Self::ToggleWireframe => "Toggle Wireframe",
            Self::CameraForward => "Camera Forward",
            Self::CameraBackward => "Camera Backward",
            Self::CameraLeft => "Camera Left",
            Self::CameraRight => "Camera Right",
            Self::CameraUp => "Camera Up",
            Self::CameraDown => "Camera Down",
            Self::SaveBookmark1 => "Save Bookmark 1",
            Self::SaveBookmark2 => "Save Bookmark 2",
            Self::SaveBookmark3 => "Save Bookmark 3",
            Self::SaveBookmark4 => "Save Bookmark 4",
            Self::SaveBookmark5 => "Save Bookmark 5",
            Self::SaveBookmark6 => "Save Bookmark 6",
            Self::SaveBookmark7 => "Save Bookmark 7",
            Self::SaveBookmark8 => "Save Bookmark 8",
            Self::SaveBookmark9 => "Save Bookmark 9",
            Self::LoadBookmark1 => "Load Bookmark 1",
            Self::LoadBookmark2 => "Load Bookmark 2",
            Self::LoadBookmark3 => "Load Bookmark 3",
            Self::LoadBookmark4 => "Load Bookmark 4",
            Self::LoadBookmark5 => "Load Bookmark 5",
            Self::LoadBookmark6 => "Load Bookmark 6",
            Self::LoadBookmark7 => "Load Bookmark 7",
            Self::LoadBookmark8 => "Load Bookmark 8",
            Self::LoadBookmark9 => "Load Bookmark 9",
            Self::ModalGrab => "Modal Grab",
            Self::ModalRotate => "Modal Rotate",
            Self::ModalScale => "Modal Scale",
            Self::ModalConstrainX => "Modal Constrain X",
            Self::ModalConstrainY => "Modal Constrain Y",
            Self::ModalConstrainZ => "Modal Constrain Z",
            Self::ModalConfirm => "Modal Confirm",
            Self::ModalCancel => "Modal Cancel",
        };
        f.write_str(name)
    }
}

impl EditorAction {
    /// Parse an action from its display name (reverse of `Display`).
    pub fn from_display_name(name: &str) -> Option<Self> {
        match name {
            "Undo" => Some(Self::Undo),
            "Redo" => Some(Self::Redo),
            "Save" => Some(Self::Save),
            "Save As" => Some(Self::SaveAs),
            "Open" => Some(Self::Open),
            "New Scene" => Some(Self::NewScene),
            "Delete" => Some(Self::Delete),
            "Duplicate" => Some(Self::Duplicate),
            "Copy Components" => Some(Self::CopyComponents),
            "Paste Components" => Some(Self::PasteComponents),
            "Hide Selected" | "Toggle Visibility" => Some(Self::ToggleVisibility),
            "Unhide All" => Some(Self::UnhideAll),
            "Hide All" => Some(Self::HideAll),
            "Reset Position" => Some(Self::ResetPosition),
            "Reset Rotation" => Some(Self::ResetRotation),
            "Reset Scale" => Some(Self::ResetScale),
            "Rotate 90\u{b0} Left" => Some(Self::Rotate90Left),
            "Rotate 90\u{b0} Right" => Some(Self::Rotate90Right),
            "Rotate 90\u{b0} Up" => Some(Self::Rotate90Up),
            "Rotate 90\u{b0} Down" => Some(Self::Rotate90Down),
            "Roll 90\u{b0} Left" => Some(Self::Roll90Left),
            "Roll 90\u{b0} Right" => Some(Self::Roll90Right),
            "Nudge Left" => Some(Self::NudgeLeft),
            "Nudge Right" => Some(Self::NudgeRight),
            "Nudge Forward" => Some(Self::NudgeForward),
            "Nudge Back" => Some(Self::NudgeBack),
            "Nudge Up" => Some(Self::NudgeUp),
            "Nudge Down" => Some(Self::NudgeDown),
            "Vertex Mode" => Some(Self::VertexMode),
            "Edge Mode" => Some(Self::EdgeMode),
            "Face Mode" => Some(Self::FaceMode),
            "Clip Mode" => Some(Self::ClipMode),
            "Exit Edit Mode" => Some(Self::ExitEditMode),
            "Delete Element" => Some(Self::DeleteBrushElement),
            "Constrain X" => Some(Self::ConstrainX),
            "Constrain Y" => Some(Self::ConstrainY),
            "Constrain Z" => Some(Self::ConstrainZ),
            "Clip Cycle Mode" => Some(Self::ClipCycleMode),
            "Clip Apply" => Some(Self::ClipApply),
            "Clip Clear" => Some(Self::ClipClear),
            "Draw Brush (Cut)" => Some(Self::DrawCut),
            "Toggle Draw Mode" => Some(Self::ToggleDrawMode),
            "Append to Brush" => Some(Self::AppendToBrush),
            "Close Polygon" => Some(Self::ClosePolygon),
            "Remove Last Vertex" => Some(Self::RemoveLastVertex),
            "Cancel Draw" => Some(Self::CancelDraw),
            "Join Brushes" => Some(Self::JoinBrushes),
            "CSG Subtract" => Some(Self::CsgSubtract),
            "CSG Intersect" => Some(Self::CsgIntersect),
            "Extend Face to Brush" => Some(Self::ExtendFaceToBrush),
            "Tool Select" => Some(Self::ToolSelect),
            "Tool Translate" => Some(Self::ToolTranslate),
            "Tool Rotate" => Some(Self::ToolRotate),
            "Tool Scale" => Some(Self::ToolScale),
            "Toggle Gizmo Space" => Some(Self::ToggleGizmoSpace),
            "Focus Selected" => Some(Self::FocusSelected),
            "Decrease Grid" => Some(Self::DecreaseGrid),
            "Increase Grid" => Some(Self::IncreaseGrid),
            "Toggle Wireframe" => Some(Self::ToggleWireframe),
            "Camera Forward" => Some(Self::CameraForward),
            "Camera Backward" => Some(Self::CameraBackward),
            "Camera Left" => Some(Self::CameraLeft),
            "Camera Right" => Some(Self::CameraRight),
            "Camera Up" => Some(Self::CameraUp),
            "Camera Down" => Some(Self::CameraDown),
            "Save Bookmark 1" => Some(Self::SaveBookmark1),
            "Save Bookmark 2" => Some(Self::SaveBookmark2),
            "Save Bookmark 3" => Some(Self::SaveBookmark3),
            "Save Bookmark 4" => Some(Self::SaveBookmark4),
            "Save Bookmark 5" => Some(Self::SaveBookmark5),
            "Save Bookmark 6" => Some(Self::SaveBookmark6),
            "Save Bookmark 7" => Some(Self::SaveBookmark7),
            "Save Bookmark 8" => Some(Self::SaveBookmark8),
            "Save Bookmark 9" => Some(Self::SaveBookmark9),
            "Load Bookmark 1" => Some(Self::LoadBookmark1),
            "Load Bookmark 2" => Some(Self::LoadBookmark2),
            "Load Bookmark 3" => Some(Self::LoadBookmark3),
            "Load Bookmark 4" => Some(Self::LoadBookmark4),
            "Load Bookmark 5" => Some(Self::LoadBookmark5),
            "Load Bookmark 6" => Some(Self::LoadBookmark6),
            "Load Bookmark 7" => Some(Self::LoadBookmark7),
            "Load Bookmark 8" => Some(Self::LoadBookmark8),
            "Load Bookmark 9" => Some(Self::LoadBookmark9),
            "Modal Grab" => Some(Self::ModalGrab),
            "Modal Rotate" => Some(Self::ModalRotate),
            "Modal Scale" => Some(Self::ModalScale),
            "Modal Constrain X" => Some(Self::ModalConstrainX),
            "Modal Constrain Y" => Some(Self::ModalConstrainY),
            "Modal Constrain Z" => Some(Self::ModalConstrainZ),
            "Modal Confirm" => Some(Self::ModalConfirm),
            "Modal Cancel" => Some(Self::ModalCancel),
            _ => None,
        }
    }

    /// Category name for grouping in the keybind settings UI.
    pub fn category(&self) -> &'static str {
        match self {
            Self::Undo | Self::Redo | Self::Save | Self::SaveAs | Self::Open | Self::NewScene => {
                "File"
            }
            Self::Delete
            | Self::Duplicate
            | Self::CopyComponents
            | Self::PasteComponents
            | Self::ToggleVisibility
            | Self::UnhideAll
            | Self::HideAll
            | Self::ResetPosition
            | Self::ResetRotation
            | Self::ResetScale => "Entity",
            Self::Rotate90Left
            | Self::Rotate90Right
            | Self::Rotate90Up
            | Self::Rotate90Down
            | Self::Roll90Left
            | Self::Roll90Right
            | Self::NudgeLeft
            | Self::NudgeRight
            | Self::NudgeForward
            | Self::NudgeBack
            | Self::NudgeUp
            | Self::NudgeDown => "Transform",
            Self::VertexMode
            | Self::EdgeMode
            | Self::FaceMode
            | Self::ClipMode
            | Self::ExitEditMode
            | Self::DeleteBrushElement
            | Self::ConstrainX
            | Self::ConstrainY
            | Self::ConstrainZ
            | Self::ClipCycleMode
            | Self::ClipApply
            | Self::ClipClear => "Brush Edit",
            Self::DrawCut
            | Self::ToggleDrawMode
            | Self::AppendToBrush
            | Self::ClosePolygon
            | Self::RemoveLastVertex
            | Self::CancelDraw => "Draw Brush",
            Self::JoinBrushes
            | Self::CsgSubtract
            | Self::CsgIntersect
            | Self::ExtendFaceToBrush => "CSG",
            Self::ToolSelect
            | Self::ToolTranslate
            | Self::ToolRotate
            | Self::ToolScale
            | Self::ToggleGizmoSpace => "Tools",
            Self::FocusSelected
            | Self::CameraForward
            | Self::CameraBackward
            | Self::CameraLeft
            | Self::CameraRight
            | Self::CameraUp
            | Self::CameraDown
            | Self::SaveBookmark1
            | Self::SaveBookmark2
            | Self::SaveBookmark3
            | Self::SaveBookmark4
            | Self::SaveBookmark5
            | Self::SaveBookmark6
            | Self::SaveBookmark7
            | Self::SaveBookmark8
            | Self::SaveBookmark9
            | Self::LoadBookmark1
            | Self::LoadBookmark2
            | Self::LoadBookmark3
            | Self::LoadBookmark4
            | Self::LoadBookmark5
            | Self::LoadBookmark6
            | Self::LoadBookmark7
            | Self::LoadBookmark8
            | Self::LoadBookmark9 => "Navigation",
            Self::DecreaseGrid | Self::IncreaseGrid | Self::ToggleWireframe => "View",
            Self::ModalGrab
            | Self::ModalRotate
            | Self::ModalScale
            | Self::ModalConstrainX
            | Self::ModalConstrainY
            | Self::ModalConstrainZ
            | Self::ModalConfirm
            | Self::ModalCancel => "Modal",
        }
    }

    /// All actions in display order, excluding disabled Modal* variants.
    pub fn all() -> &'static [EditorAction] {
        use EditorAction as A;
        &[
            // File
            A::Undo,
            A::Redo,
            A::Save,
            A::SaveAs,
            A::Open,
            A::NewScene,
            // Entity
            A::Delete,
            A::Duplicate,
            A::CopyComponents,
            A::PasteComponents,
            A::ToggleVisibility,
            A::UnhideAll,
            A::HideAll,
            A::ResetPosition,
            A::ResetRotation,
            A::ResetScale,
            // Transform
            A::Rotate90Left,
            A::Rotate90Right,
            A::Rotate90Up,
            A::Rotate90Down,
            A::Roll90Left,
            A::Roll90Right,
            A::NudgeLeft,
            A::NudgeRight,
            A::NudgeForward,
            A::NudgeBack,
            A::NudgeUp,
            A::NudgeDown,
            // Brush Edit
            A::VertexMode,
            A::EdgeMode,
            A::FaceMode,
            A::ClipMode,
            A::ExitEditMode,
            A::DeleteBrushElement,
            A::ConstrainX,
            A::ConstrainY,
            A::ConstrainZ,
            A::ClipCycleMode,
            A::ClipApply,
            A::ClipClear,
            // Draw Brush
            A::DrawCut,
            A::ToggleDrawMode,
            A::AppendToBrush,
            A::ClosePolygon,
            A::RemoveLastVertex,
            A::CancelDraw,
            // CSG
            A::JoinBrushes,
            A::CsgSubtract,
            A::CsgIntersect,
            A::ExtendFaceToBrush,
            // Tools
            A::ToolSelect,
            A::ToolTranslate,
            A::ToolRotate,
            A::ToolScale,
            A::ToggleGizmoSpace,
            // Navigation
            A::FocusSelected,
            A::CameraForward,
            A::CameraBackward,
            A::CameraLeft,
            A::CameraRight,
            A::CameraUp,
            A::CameraDown,
            A::SaveBookmark1,
            A::SaveBookmark2,
            A::SaveBookmark3,
            A::SaveBookmark4,
            A::SaveBookmark5,
            A::SaveBookmark6,
            A::SaveBookmark7,
            A::SaveBookmark8,
            A::SaveBookmark9,
            A::LoadBookmark1,
            A::LoadBookmark2,
            A::LoadBookmark3,
            A::LoadBookmark4,
            A::LoadBookmark5,
            A::LoadBookmark6,
            A::LoadBookmark7,
            A::LoadBookmark8,
            A::LoadBookmark9,
            // View
            A::DecreaseGrid,
            A::IncreaseGrid,
            A::ToggleWireframe,
        ]
    }
}

/// A single key binding: a key plus modifier flags and an optional mouse-button chord.
///
/// Modifier matching is exact: a keybind with `ctrl: true, shift: false`
/// matches only when Ctrl is held AND Shift is NOT held. Left/Right variants
/// of each modifier are unified internally. When `mouse` is `Some(btn)`, the
/// named button must be held. When `mouse` is `None`, any held mouse button
/// causes a non-match, preventing plain tool keys from firing mid-drag.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Keybind {
    pub key: KeyCode,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    #[serde(default)]
    pub mouse: Option<MouseButton>,
}

impl Keybind {
    /// Parse a human-readable keybind string like `"Ctrl+Shift+Z"` or `"RMB+W"`.
    pub fn parse(s: &str) -> Option<Self> {
        let mut ctrl = false;
        let mut shift = false;
        let mut alt = false;
        let mut mouse: Option<MouseButton> = None;

        let parts: Vec<&str> = s.split('+').collect();
        if parts.is_empty() {
            return None;
        }

        // All parts except the last are modifiers
        for &part in &parts[..parts.len() - 1] {
            match part.trim() {
                "Ctrl" => ctrl = true,
                "Shift" => shift = true,
                "Alt" => alt = true,
                "RMB" => mouse = Some(MouseButton::Right),
                "MMB" => mouse = Some(MouseButton::Middle),
                "LMB" => mouse = Some(MouseButton::Left),
                _ => return None,
            }
        }

        let key = key_from_display_name(parts.last()?.trim())?;
        Some(Self {
            key,
            ctrl,
            shift,
            alt,
            mouse,
        })
    }

    /// Human-readable string for this keybind, e.g. `"Ctrl+Z"` or `"RMB+W"`.
    pub fn display_name(&self) -> String {
        let mut out = String::new();
        if let Some(btn) = self.mouse {
            out.push_str(match btn {
                MouseButton::Right => "RMB+",
                MouseButton::Middle => "MMB+",
                MouseButton::Left => "LMB+",
                MouseButton::Back | MouseButton::Forward | MouseButton::Other(_) => "Mouse?+",
            });
        }
        if self.ctrl {
            out.push_str("Ctrl+");
        }
        if self.shift {
            out.push_str("Shift+");
        }
        if self.alt {
            out.push_str("Alt+");
        }
        out.push_str(key_display_name(self.key));
        out
    }

    /// Keybind with no modifiers.
    pub const fn key(key: KeyCode) -> Self {
        Self {
            key,
            ctrl: false,
            shift: false,
            alt: false,
            mouse: None,
        }
    }

    /// Keybind with Ctrl.
    pub const fn ctrl(key: KeyCode) -> Self {
        Self {
            key,
            ctrl: true,
            shift: false,
            alt: false,
            mouse: None,
        }
    }

    /// Keybind with Ctrl+Shift.
    pub const fn ctrl_shift(key: KeyCode) -> Self {
        Self {
            key,
            ctrl: true,
            shift: true,
            alt: false,
            mouse: None,
        }
    }

    /// Keybind with Alt.
    pub const fn alt(key: KeyCode) -> Self {
        Self {
            key,
            ctrl: false,
            shift: false,
            alt: true,
            mouse: None,
        }
    }

    /// Keybind that requires RMB held alongside the key, with no keyboard modifiers.
    pub const fn rmb_key(key: KeyCode) -> Self {
        Self {
            key,
            ctrl: false,
            shift: false,
            alt: false,
            mouse: Some(MouseButton::Right),
        }
    }

    /// Check if the modifier keys and mouse button state match exactly (Left/Right unified).
    pub fn modifiers_match(
        &self,
        keyboard: &ButtonInput<KeyCode>,
        mouse: &ButtonInput<MouseButton>,
    ) -> bool {
        let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
        let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
        let alt = keyboard.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
        let mouse_ok = match self.mouse {
            Some(btn) => mouse.pressed(btn),
            None => {
                !mouse.pressed(MouseButton::Right)
                    && !mouse.pressed(MouseButton::Middle)
                    && !mouse.pressed(MouseButton::Left)
            }
        };
        self.ctrl == ctrl && self.shift == shift && self.alt == alt && mouse_ok
    }
}

impl fmt::Display for Keybind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display_name())
    }
}

/// Human-readable name for a `KeyCode`.
pub fn key_display_name(key: KeyCode) -> &'static str {
    match key {
        KeyCode::KeyA => "A",
        KeyCode::KeyB => "B",
        KeyCode::KeyC => "C",
        KeyCode::KeyD => "D",
        KeyCode::KeyE => "E",
        KeyCode::KeyF => "F",
        KeyCode::KeyG => "G",
        KeyCode::KeyH => "H",
        KeyCode::KeyI => "I",
        KeyCode::KeyJ => "J",
        KeyCode::KeyK => "K",
        KeyCode::KeyL => "L",
        KeyCode::KeyM => "M",
        KeyCode::KeyN => "N",
        KeyCode::KeyO => "O",
        KeyCode::KeyP => "P",
        KeyCode::KeyQ => "Q",
        KeyCode::KeyR => "R",
        KeyCode::KeyS => "S",
        KeyCode::KeyT => "T",
        KeyCode::KeyU => "U",
        KeyCode::KeyV => "V",
        KeyCode::KeyW => "W",
        KeyCode::KeyX => "X",
        KeyCode::KeyY => "Y",
        KeyCode::KeyZ => "Z",
        KeyCode::Digit0 => "0",
        KeyCode::Digit1 => "1",
        KeyCode::Digit2 => "2",
        KeyCode::Digit3 => "3",
        KeyCode::Digit4 => "4",
        KeyCode::Digit5 => "5",
        KeyCode::Digit6 => "6",
        KeyCode::Digit7 => "7",
        KeyCode::Digit8 => "8",
        KeyCode::Digit9 => "9",
        KeyCode::ArrowLeft => "Left",
        KeyCode::ArrowRight => "Right",
        KeyCode::ArrowUp => "Up",
        KeyCode::ArrowDown => "Down",
        KeyCode::Escape => "Esc",
        KeyCode::Enter => "Enter",
        KeyCode::Tab => "Tab",
        KeyCode::Space => "Space",
        KeyCode::Backspace => "Backspace",
        KeyCode::Delete => "Del",
        KeyCode::PageUp => "PgUp",
        KeyCode::PageDown => "PgDn",
        KeyCode::Home => "Home",
        KeyCode::End => "End",
        KeyCode::BracketLeft => "[",
        KeyCode::BracketRight => "]",
        KeyCode::Backquote => "`",
        KeyCode::Minus => "-",
        KeyCode::Equal => "=",
        KeyCode::Semicolon => ";",
        KeyCode::Quote => "'",
        KeyCode::Comma => ",",
        KeyCode::Period => ".",
        KeyCode::Slash => "/",
        KeyCode::Backslash => "\\",
        KeyCode::F1 => "F1",
        KeyCode::F2 => "F2",
        KeyCode::F3 => "F3",
        KeyCode::F4 => "F4",
        KeyCode::F5 => "F5",
        KeyCode::F6 => "F6",
        KeyCode::F7 => "F7",
        KeyCode::F8 => "F8",
        KeyCode::F9 => "F9",
        KeyCode::F10 => "F10",
        KeyCode::F11 => "F11",
        KeyCode::F12 => "F12",
        KeyCode::Insert => "Ins",
        _ => "???",
    }
}

/// Reverse of `key_display_name`: parse a human-readable key name back to `KeyCode`.
pub fn key_from_display_name(name: &str) -> Option<KeyCode> {
    match name {
        "A" => Some(KeyCode::KeyA),
        "B" => Some(KeyCode::KeyB),
        "C" => Some(KeyCode::KeyC),
        "D" => Some(KeyCode::KeyD),
        "E" => Some(KeyCode::KeyE),
        "F" => Some(KeyCode::KeyF),
        "G" => Some(KeyCode::KeyG),
        "H" => Some(KeyCode::KeyH),
        "I" => Some(KeyCode::KeyI),
        "J" => Some(KeyCode::KeyJ),
        "K" => Some(KeyCode::KeyK),
        "L" => Some(KeyCode::KeyL),
        "M" => Some(KeyCode::KeyM),
        "N" => Some(KeyCode::KeyN),
        "O" => Some(KeyCode::KeyO),
        "P" => Some(KeyCode::KeyP),
        "Q" => Some(KeyCode::KeyQ),
        "R" => Some(KeyCode::KeyR),
        "S" => Some(KeyCode::KeyS),
        "T" => Some(KeyCode::KeyT),
        "U" => Some(KeyCode::KeyU),
        "V" => Some(KeyCode::KeyV),
        "W" => Some(KeyCode::KeyW),
        "X" => Some(KeyCode::KeyX),
        "Y" => Some(KeyCode::KeyY),
        "Z" => Some(KeyCode::KeyZ),
        "0" => Some(KeyCode::Digit0),
        "1" => Some(KeyCode::Digit1),
        "2" => Some(KeyCode::Digit2),
        "3" => Some(KeyCode::Digit3),
        "4" => Some(KeyCode::Digit4),
        "5" => Some(KeyCode::Digit5),
        "6" => Some(KeyCode::Digit6),
        "7" => Some(KeyCode::Digit7),
        "8" => Some(KeyCode::Digit8),
        "9" => Some(KeyCode::Digit9),
        "Left" => Some(KeyCode::ArrowLeft),
        "Right" => Some(KeyCode::ArrowRight),
        "Up" => Some(KeyCode::ArrowUp),
        "Down" => Some(KeyCode::ArrowDown),
        "Esc" => Some(KeyCode::Escape),
        "Enter" => Some(KeyCode::Enter),
        "Tab" => Some(KeyCode::Tab),
        "Space" => Some(KeyCode::Space),
        "Backspace" => Some(KeyCode::Backspace),
        "Del" => Some(KeyCode::Delete),
        "PgUp" => Some(KeyCode::PageUp),
        "PgDn" => Some(KeyCode::PageDown),
        "Home" => Some(KeyCode::Home),
        "End" => Some(KeyCode::End),
        "[" => Some(KeyCode::BracketLeft),
        "]" => Some(KeyCode::BracketRight),
        "`" => Some(KeyCode::Backquote),
        "-" => Some(KeyCode::Minus),
        "=" => Some(KeyCode::Equal),
        ";" => Some(KeyCode::Semicolon),
        "'" => Some(KeyCode::Quote),
        "," => Some(KeyCode::Comma),
        "." => Some(KeyCode::Period),
        "/" => Some(KeyCode::Slash),
        "\\" => Some(KeyCode::Backslash),
        "F1" => Some(KeyCode::F1),
        "F2" => Some(KeyCode::F2),
        "F3" => Some(KeyCode::F3),
        "F4" => Some(KeyCode::F4),
        "F5" => Some(KeyCode::F5),
        "F6" => Some(KeyCode::F6),
        "F7" => Some(KeyCode::F7),
        "F8" => Some(KeyCode::F8),
        "F9" => Some(KeyCode::F9),
        "F10" => Some(KeyCode::F10),
        "F11" => Some(KeyCode::F11),
        "F12" => Some(KeyCode::F12),
        "Ins" => Some(KeyCode::Insert),
        _ => None,
    }
}

/// Resource holding the full set of keybindings for editor actions.
///
/// Each action maps to one or more [`Keybind`]s (e.g. Delete can be bound to
/// both `Delete` and `Backspace`). Query methods return `true` if **any** of
/// the action's bindings match.
#[derive(Resource, Clone, Serialize, Deserialize)]
pub struct KeybindRegistry {
    pub bindings: HashMap<EditorAction, Vec<Keybind>>,
    /// When `true`, all query methods short-circuit to `false`.
    /// Used during keybind recording to prevent actions from firing.
    #[serde(skip)]
    pub recording: bool,
}

impl KeybindRegistry {
    /// Check if the action's full keybind (key + exact modifiers) was just pressed.
    pub fn just_pressed(
        &self,
        action: EditorAction,
        keyboard: &ButtonInput<KeyCode>,
        mouse: &ButtonInput<MouseButton>,
    ) -> bool {
        if self.recording {
            return false;
        }
        self.bindings.get(&action).is_some_and(|binds| {
            binds
                .iter()
                .any(|b| keyboard.just_pressed(b.key) && b.modifiers_match(keyboard, mouse))
        })
    }

    /// Check if the action's full keybind (key + exact modifiers) is held.
    pub fn pressed(
        &self,
        action: EditorAction,
        keyboard: &ButtonInput<KeyCode>,
        mouse: &ButtonInput<MouseButton>,
    ) -> bool {
        if self.recording {
            return false;
        }
        self.bindings.get(&action).is_some_and(|binds| {
            binds
                .iter()
                .any(|b| keyboard.pressed(b.key) && b.modifiers_match(keyboard, mouse))
        })
    }

    /// Check if the action's full keybind (key + exact modifiers) was just released.
    pub fn just_released(
        &self,
        action: EditorAction,
        keyboard: &ButtonInput<KeyCode>,
        mouse: &ButtonInput<MouseButton>,
    ) -> bool {
        if self.recording {
            return false;
        }
        self.bindings.get(&action).is_some_and(|binds| {
            binds
                .iter()
                .any(|b| keyboard.just_released(b.key) && b.modifiers_match(keyboard, mouse))
        })
    }

    /// Check only if the action's key was just pressed (ignoring modifiers).
    ///
    /// Use when the system handles modifier logic itself (e.g. arrow nudge
    /// where Ctrl+Arrow has additional duplicate behaviour).
    pub fn key_just_pressed(&self, action: EditorAction, keyboard: &ButtonInput<KeyCode>) -> bool {
        if self.recording {
            return false;
        }
        self.bindings
            .get(&action)
            .is_some_and(|binds| binds.iter().any(|b| keyboard.just_pressed(b.key)))
    }

    /// Check only if the action's key is held (ignoring modifiers).
    ///
    /// Use for continuous input where modifier logic is handled by the system
    /// itself. For chorded continuous input (key + mouse button), use
    /// [`Self::key_chord_pressed`].
    pub fn key_pressed(&self, action: EditorAction, keyboard: &ButtonInput<KeyCode>) -> bool {
        if self.recording {
            return false;
        }
        self.bindings
            .get(&action)
            .is_some_and(|binds| binds.iter().any(|b| keyboard.pressed(b.key)))
    }

    /// Check that the action's key is held and its mouse modifier is held (if any),
    /// while ignoring ctrl/shift/alt state. Use for continuous-input chords where
    /// keyboard modifiers must compose freely (e.g. camera fly + Shift run).
    pub fn key_chord_pressed(
        &self,
        action: EditorAction,
        keyboard: &ButtonInput<KeyCode>,
        mouse: &ButtonInput<MouseButton>,
    ) -> bool {
        if self.recording {
            return false;
        }
        self.bindings.get(&action).is_some_and(|binds| {
            binds
                .iter()
                .any(|b| keyboard.pressed(b.key) && b.mouse.is_none_or(|btn| mouse.pressed(btn)))
        })
    }

    /// Check only if the action's modifiers are currently held (not the key).
    pub fn modifiers_held(
        &self,
        action: EditorAction,
        keyboard: &ButtonInput<KeyCode>,
        mouse: &ButtonInput<MouseButton>,
    ) -> bool {
        if self.recording {
            return false;
        }
        self.bindings
            .get(&action)
            .is_some_and(|binds| binds.iter().any(|b| b.modifiers_match(keyboard, mouse)))
    }
}

impl Default for KeybindRegistry {
    fn default() -> Self {
        use EditorAction as A;
        use KeyCode as K;

        let mut bindings = HashMap::new();

        let b = |key: KeyCode, ctrl: bool, shift: bool, alt: bool| -> Vec<Keybind> {
            vec![Keybind {
                key,
                ctrl,
                shift,
                alt,
                mouse: None,
            }]
        };
        let k = |key: KeyCode| -> Vec<Keybind> { vec![Keybind::key(key)] };
        let rmb = |key: KeyCode| -> Vec<Keybind> { vec![Keybind::rmb_key(key)] };

        // -- File ------------------------------------------------------
        bindings.insert(A::Undo, b(K::KeyZ, true, false, false));
        bindings.insert(A::Redo, b(K::KeyZ, true, true, false));
        bindings.insert(A::Save, b(K::KeyS, true, false, false));
        bindings.insert(A::SaveAs, b(K::KeyS, true, true, false));
        bindings.insert(A::Open, b(K::KeyO, true, false, false));
        bindings.insert(A::NewScene, b(K::KeyN, true, true, false));

        // -- Entity operations -----------------------------------------
        bindings.insert(A::Delete, k(K::Delete));
        bindings.insert(A::Duplicate, b(K::KeyD, true, false, false));
        bindings.insert(A::CopyComponents, b(K::KeyC, true, false, false));
        bindings.insert(A::PasteComponents, b(K::KeyV, true, false, false));
        bindings.insert(A::ToggleVisibility, k(K::KeyH));
        bindings.insert(A::UnhideAll, b(K::KeyH, true, false, false));
        bindings.insert(A::HideAll, b(K::KeyH, false, false, true));
        bindings.insert(A::ResetPosition, b(K::KeyG, false, false, true));
        bindings.insert(A::ResetRotation, b(K::KeyR, false, false, true));
        bindings.insert(A::ResetScale, b(K::KeyS, false, false, true));

        // -- Rotations (Alt + Arrow / PageUp/Down) --------------------
        bindings.insert(A::Rotate90Left, b(K::ArrowLeft, false, false, true));
        bindings.insert(A::Rotate90Right, b(K::ArrowRight, false, false, true));
        bindings.insert(A::Rotate90Up, b(K::ArrowUp, false, false, true));
        bindings.insert(A::Rotate90Down, b(K::ArrowDown, false, false, true));
        bindings.insert(A::Roll90Left, b(K::PageUp, false, false, true));
        bindings.insert(A::Roll90Right, b(K::PageDown, false, false, true));

        // -- Nudge (Arrow keys / PageUp/Down) -------------------------
        bindings.insert(A::NudgeLeft, k(K::ArrowLeft));
        bindings.insert(A::NudgeRight, k(K::ArrowRight));
        bindings.insert(A::NudgeForward, k(K::ArrowUp));
        bindings.insert(A::NudgeBack, k(K::ArrowDown));
        bindings.insert(A::NudgeUp, k(K::PageUp));
        bindings.insert(A::NudgeDown, k(K::PageDown));

        // -- Brush edit modes ------------------------------------------
        bindings.insert(A::VertexMode, k(K::Digit1));
        bindings.insert(A::EdgeMode, k(K::Digit2));
        bindings.insert(A::FaceMode, k(K::Digit3));
        bindings.insert(A::ClipMode, k(K::Digit4));
        bindings.insert(A::ExitEditMode, k(K::Escape));

        // -- Brush element deletion (Delete + Backspace) ---------------
        bindings.insert(
            A::DeleteBrushElement,
            vec![Keybind::key(K::Delete), Keybind::key(K::Backspace)],
        );

        // -- Axis constraints ------------------------------------------
        bindings.insert(A::ConstrainX, k(K::KeyX));
        bindings.insert(A::ConstrainY, k(K::KeyY));
        bindings.insert(A::ConstrainZ, k(K::KeyZ));

        // -- Clip mode -------------------------------------------------
        bindings.insert(A::ClipCycleMode, k(K::Tab));
        bindings.insert(A::ClipApply, k(K::Enter));
        bindings.insert(A::ClipClear, k(K::Escape));

        // -- Draw brush ------------------------------------------------
        bindings.insert(A::DrawCut, k(K::KeyC));
        bindings.insert(A::ToggleDrawMode, k(K::Tab));
        bindings.insert(A::AppendToBrush, b(K::KeyB, false, false, true));
        bindings.insert(A::ClosePolygon, k(K::Enter));
        bindings.insert(A::RemoveLastVertex, k(K::Backspace));
        bindings.insert(A::CancelDraw, k(K::Escape));

        // -- CSG / Join ------------------------------------------------
        bindings.insert(A::JoinBrushes, k(K::KeyJ));
        bindings.insert(A::CsgSubtract, b(K::KeyK, true, false, false));
        bindings.insert(A::CsgIntersect, b(K::KeyK, true, true, false));
        bindings.insert(A::ExtendFaceToBrush, b(K::KeyE, true, false, false));

        // -- Tools -----------------------------------------------------
        bindings.insert(A::ToolSelect, k(K::KeyQ));
        bindings.insert(A::ToolTranslate, k(K::KeyW));
        bindings.insert(A::ToolRotate, k(K::KeyE));
        bindings.insert(A::ToolScale, k(K::KeyR));
        bindings.insert(A::ToggleGizmoSpace, k(K::KeyX));

        // -- Viewport --------------------------------------------------
        bindings.insert(A::FocusSelected, k(K::KeyF));

        // -- Grid ------------------------------------------------------
        bindings.insert(A::DecreaseGrid, k(K::BracketLeft));
        bindings.insert(A::IncreaseGrid, k(K::BracketRight));

        // -- View modes ------------------------------------------------
        bindings.insert(A::ToggleWireframe, b(K::KeyW, true, true, false));

        // -- Camera movement -------------------------------------------
        bindings.insert(A::CameraForward, rmb(K::KeyW));
        bindings.insert(A::CameraBackward, rmb(K::KeyS));
        bindings.insert(A::CameraLeft, rmb(K::KeyA));
        bindings.insert(A::CameraRight, rmb(K::KeyD));
        bindings.insert(A::CameraDown, rmb(K::KeyQ));
        bindings.insert(A::CameraUp, rmb(K::KeyE));

        // -- Camera bookmarks (Ctrl+N to save, N to load) -------------
        bindings.insert(A::SaveBookmark1, b(K::Digit1, true, false, false));
        bindings.insert(A::SaveBookmark2, b(K::Digit2, true, false, false));
        bindings.insert(A::SaveBookmark3, b(K::Digit3, true, false, false));
        bindings.insert(A::SaveBookmark4, b(K::Digit4, true, false, false));
        bindings.insert(A::SaveBookmark5, b(K::Digit5, true, false, false));
        bindings.insert(A::SaveBookmark6, b(K::Digit6, true, false, false));
        bindings.insert(A::SaveBookmark7, b(K::Digit7, true, false, false));
        bindings.insert(A::SaveBookmark8, b(K::Digit8, true, false, false));
        bindings.insert(A::SaveBookmark9, b(K::Digit9, true, false, false));
        bindings.insert(A::LoadBookmark1, k(K::Digit1));
        bindings.insert(A::LoadBookmark2, k(K::Digit2));
        bindings.insert(A::LoadBookmark3, k(K::Digit3));
        bindings.insert(A::LoadBookmark4, k(K::Digit4));
        bindings.insert(A::LoadBookmark5, k(K::Digit5));
        bindings.insert(A::LoadBookmark6, k(K::Digit6));
        bindings.insert(A::LoadBookmark7, k(K::Digit7));
        bindings.insert(A::LoadBookmark8, k(K::Digit8));
        bindings.insert(A::LoadBookmark9, k(K::Digit9));

        // -- Modal transform (disabled, keybinds ready) ----------------
        bindings.insert(A::ModalGrab, k(K::KeyG));
        bindings.insert(A::ModalRotate, k(K::KeyR));
        bindings.insert(A::ModalScale, k(K::KeyS));
        bindings.insert(A::ModalConstrainX, k(K::KeyX));
        bindings.insert(A::ModalConstrainY, k(K::KeyY));
        bindings.insert(A::ModalConstrainZ, k(K::KeyZ));
        bindings.insert(A::ModalConfirm, k(K::Enter));
        bindings.insert(A::ModalCancel, k(K::Escape));

        Self {
            bindings,
            recording: false,
        }
    }
}

#[cfg(test)]
mod mouse_modifier_tests {
    use super::*;
    use bevy::input::ButtonInput;
    use bevy::prelude::{KeyCode, MouseButton};

    fn empty_keyboard() -> ButtonInput<KeyCode> {
        ButtonInput::default()
    }
    fn mouse_with(btns: &[MouseButton]) -> ButtonInput<MouseButton> {
        let mut input = ButtonInput::<MouseButton>::default();
        for &b in btns {
            input.press(b);
        }
        input
    }

    #[test]
    fn plain_keybind_has_no_mouse_modifier() {
        let kb = Keybind::key(KeyCode::KeyW);
        assert_eq!(kb.mouse, None);
    }

    #[test]
    fn rmb_key_sets_right_mouse_modifier() {
        let kb = Keybind::rmb_key(KeyCode::KeyW);
        assert_eq!(kb.mouse, Some(MouseButton::Right));
        assert_eq!(kb.key, KeyCode::KeyW);
        assert!(!kb.ctrl && !kb.shift && !kb.alt);
    }

    #[test]
    fn modifiers_match_requires_mouse_state_to_match() {
        let plain = Keybind::key(KeyCode::KeyW);
        let chord = Keybind::rmb_key(KeyCode::KeyW);
        let kb = empty_keyboard();
        let no_mouse = mouse_with(&[]);
        let rmb = mouse_with(&[MouseButton::Right]);

        assert!(plain.modifiers_match(&kb, &no_mouse));
        assert!(!plain.modifiers_match(&kb, &rmb));
        assert!(!chord.modifiers_match(&kb, &no_mouse));
        assert!(chord.modifiers_match(&kb, &rmb));
    }

    #[test]
    fn parse_round_trips_rmb_prefix() {
        let kb = Keybind::parse("RMB+W").expect("parse RMB+W");
        assert_eq!(kb, Keybind::rmb_key(KeyCode::KeyW));
        assert_eq!(kb.display_name(), "RMB+W");
    }
}
