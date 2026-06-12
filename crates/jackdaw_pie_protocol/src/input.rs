//! Input events forwarded from the editor's Live viewport to the game.
//!
//! Carried inside [`ControlEvent::Input`](crate::event::ControlEvent) on the
//! reliable channel: ordered with everything else, and a release must never
//! overtake its press. The bevy input types serialize directly (the
//! workspace `bevy` enables its `serialize` feature), so nothing is lossy.

use bevy::input::keyboard::{Key, KeyCode};
use bevy::input::mouse::MouseButton;
use bevy::math::Vec2;
use serde::{Deserialize, Serialize};

/// One forwarded input event.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum PieInputEvent {
    /// A key press or release. `logical` carries layout-aware text
    /// (`Key::Character`), which UI text inputs read.
    Key {
        key: KeyCode,
        logical: Key,
        pressed: bool,
        repeat: bool,
    },
    /// A mouse button press or release.
    MouseButton { button: MouseButton, pressed: bool },
    /// Absolute cursor position in stream pixels (the streamed frame's
    /// pixel space).
    CursorMoved { position: Vec2 },
    /// Raw relative motion for mouse-look, unscaled.
    MouseMotion { delta: Vec2 },
    /// Scroll deltas `x`/`y`; `line_units` is true for line scrolling
    /// (deltas count lines), false for pixel scrolling (deltas count pixels).
    MouseWheel { x: f32, y: f32, line_units: bool },
    /// Capture engaged: the game window should consider itself focused.
    FocusGained,
    /// Capture released: the game clears held keys and loses focus.
    FocusLost,
}
