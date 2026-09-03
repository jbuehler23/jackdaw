//! Decide whether a keybind/operator gate should refuse because the
//! user is typing into a UI text input.
//!
//! Why a wrapper exists: Bevy's [`bevy::input_focus::InputFocus`]
//! `set_initial_focus` system runs in `PostStartup` and assigns the
//! `PrimaryWindow` entity as the focused entity when nothing else has
//! claimed focus yet. A gate written as `input_focus.get().is_none()`
//! therefore reads "user is typing" whenever the editor is in its
//! post-load steady state, and refuses the keybind. In production the
//! viewport-click handler clears focus, masking the bug; in headless
//! tests (and on the very first key press after launch) the gate
//! refuses spuriously.
//!
//! [`KeybindFocus`] returns `is_typing()` only when the focused entity
//! holds an editable text buffer.
//!
//! The buffer rather than the editor's own field wrapper: a field built
//! straight out of Bevy's text input -- the canvas text entry, the Add
//! Entity search, an inspector row -- holds the keyboard exactly as the
//! wrapper does, and asking after the wrapper let every one of those type
//! a name into the field and a chord into the scene at the same time.
//!
//! Recording a chord in the keybind settings is the same class of thing:
//! the press names a key rather than commanding the editor with it, and
//! everything that reads the keyboard for a gesture has to stand down for
//! it exactly as it does for typing. So the two share one answer,
//! [`KeybindFocus::keyboard_is_spoken_for`], rather than each reader
//! remembering to ask twice. Readers in the crates below this one, which
//! cannot see this type, ask [`KeymapCapture::is_recording`] instead --
//! the same flag, one layer down.

use bevy::ecs::system::SystemParam;
use bevy::input_focus::InputFocus;
use bevy::prelude::*;
use bevy::text::EditableText;
use jackdaw_api::prelude::ActionSources;
use jackdaw_commands::KeymapCapture;

/// `SystemParam` that returns whether keybinds and operator dispatches
/// should be suppressed because the keyboard belongs to something other
/// than the editor's commands.
#[derive(SystemParam)]
pub struct KeybindFocus<'w, 's> {
    input_focus: Res<'w, InputFocus>,
    text_inputs: Query<'w, 's, (), With<EditableText>>,
    capture: Option<Res<'w, KeymapCapture>>,
}

impl KeybindFocus<'_, '_> {
    /// True when the focused entity holds an editable text buffer.
    pub fn is_typing(&self) -> bool {
        let Some(focused) = self.input_focus.get() else {
            return false;
        };
        self.text_inputs.contains(focused)
    }

    /// True while the keybind settings are recording a chord.
    pub fn is_recording(&self) -> bool {
        KeymapCapture::is_recording(self.capture.as_deref())
    }

    /// True when this press is not the editor's to act on: the user is
    /// typing it into a field, or naming it as a binding.
    ///
    /// What every gate predicate and every direct keyboard reader outside
    /// the operator path asks.
    pub fn keyboard_is_spoken_for(&self) -> bool {
        self.is_typing() || self.is_recording()
    }

    /// Whether any of `keys` is held *for this gesture*.
    ///
    /// What a modal drag asks instead of reading `ButtonInput` itself. A
    /// modal reads Ctrl, Shift and Alt as modifiers rather than as chords, so
    /// the keymap's own suppression does not reach it: with a text field
    /// focused or a rebind recording, a Shift typed into the field also
    /// changed what the drag under way was doing, and there was nothing on
    /// screen saying it had. The keyboard belongs to one thing at a time.
    pub fn any_pressed(
        &self,
        keyboard: &ButtonInput<KeyCode>,
        keys: impl IntoIterator<Item = KeyCode>,
    ) -> bool {
        !self.keyboard_is_spoken_for() && keyboard.any_pressed(keys)
    }

    /// True if the input focus or the recording flag changed since the
    /// system last ran.
    pub fn is_changed(&self) -> bool {
        self.input_focus.is_changed()
            || self.capture.as_ref().is_some_and(DetectChanges::is_changed)
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

    // Suppress action keybinds while the keyboard is spoken for, a numeric
    // transform entry is capturing the keyboard, or Live input capture is
    // forwarding to the game, so typed digits go to the entry and game
    // input does not fire edit-mode and tool keybinds.
    sources.keyboard = !focus.keyboard_is_spoken_for() && numeric.axis.is_none() && !capture.active;
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
