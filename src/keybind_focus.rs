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
use jackdaw_api::prelude::ActionSources;
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

    /// True if the input focus changed since the system last ran.
    pub fn is_changed(&self) -> bool {
        self.input_focus.is_changed()
    }
}

pub(crate) fn disable_keyboard_input_when_typing(
    focus: KeybindFocus,
    numeric: Res<crate::numeric_transform::NumericTransformState>,
    capture: Res<crate::live_input::LiveInputCapture>,
    mut sources: ResMut<ActionSources>,
) {
    if !focus.is_changed() && !numeric.is_changed() && !capture.is_changed() {
        return;
    }

    // Suppress action keybinds while a text field is focused, a numeric
    // transform entry is capturing the keyboard, or Live input capture is
    // forwarding to the game, so typed digits go to the entry and game
    // input does not fire edit-mode and tool keybinds.
    sources.keyboard = !focus.is_typing() && numeric.axis.is_none() && !capture.active;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_active_suppresses_action_keybinds() {
        use bevy::input_focus::InputFocus;
        use jackdaw_api::prelude::ActionSources;
        let mut app = bevy::app::App::new();
        app.init_resource::<InputFocus>();
        app.init_resource::<crate::numeric_transform::NumericTransformState>();
        app.init_resource::<ActionSources>();
        app.init_resource::<crate::live_input::LiveInputCapture>();
        app.world_mut()
            .resource_mut::<crate::live_input::LiveInputCapture>()
            .active = true;
        app.world_mut()
            .run_system_cached(disable_keyboard_input_when_typing)
            .unwrap();
        assert!(
            !app.world().resource::<ActionSources>().keyboard,
            "capture active suppresses action keybinds"
        );
        app.world_mut()
            .resource_mut::<crate::live_input::LiveInputCapture>()
            .active = false;
        app.world_mut()
            .run_system_cached(disable_keyboard_input_when_typing)
            .unwrap();
        assert!(app.world().resource::<ActionSources>().keyboard);
    }
}
