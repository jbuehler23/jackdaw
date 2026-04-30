//! Decide whether a keybind/operator gate should refuse because the
//! user is typing into a UI text input.
//!
//! Why a wrapper exists: Bevy's [`bevy::input_focus::InputFocus`]
//! `set_initial_focus` system runs in `PostStartup` and assigns the
//! `PrimaryWindow` entity as the focused entity when nothing else has
//! claimed focus yet. A gate written as `input_focus.0.is_none()`
//! therefore reads "user is typing" whenever the editor is in its
//! post-load steady state, and refuses the keybind. In production the
//! viewport-click handler clears focus, masking the bug; in headless
//! tests (and on the very first key press after launch) the gate
//! refuses spuriously.
//!
//! [`KeybindFocus`] returns `is_typing()` only when the focused entity
//! has a [`TextInputNode`].

use bevy::ecs::system::SystemParam;
use bevy::input_focus::InputFocus;
use bevy::prelude::*;
use jackdaw_api_internal::KeybindsBlocked;
use jackdaw_feathers::text_edit::TextInputNode;

/// `SystemParam` that returns whether keybinds and operator dispatches
/// should be suppressed because the user is editing a text input.
#[derive(SystemParam)]
pub struct KeybindFocus<'w, 's> {
    input_focus: Res<'w, InputFocus>,
    text_inputs: Query<'w, 's, (), With<TextInputNode>>,
}

impl KeybindFocus<'_, '_> {
    /// True when the focused entity carries a `TextInputNode`.
    /// Used by gate predicates to refuse keyboard-driven operators
    /// while the user is editing a text field.
    pub fn is_typing(&self) -> bool {
        let Some(focused) = self.input_focus.0 else {
            return false;
        };
        self.text_inputs.contains(focused)
    }
}

/// Plugin that mirrors [`KeybindFocus::is_typing`] into the
/// [`KeybindsBlocked`] resource so the BEI dispatch observer in
/// `jackdaw_api_internal` can suppress operator firing while the
/// user is editing a text field. The dispatch observer can't
/// import `KeybindFocus` directly (no editor-crate dependency from
/// `jackdaw_api_internal`), so the editor side mirrors the bool
/// each frame.
pub struct KeybindFocusPlugin;

impl Plugin for KeybindFocusPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<KeybindsBlocked>()
            .add_systems(PreUpdate, sync_keybinds_blocked);
    }
}

fn sync_keybinds_blocked(focus: KeybindFocus, mut blocked: ResMut<KeybindsBlocked>) {
    let typing = focus.is_typing();
    if blocked.0 != typing {
        blocked.0 = typing;
    }
}
