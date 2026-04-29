//! Operator bridge into the generic feathers tooltip pipeline.
//!
//! [`jackdaw_feathers::tooltip`] owns hover/render and reads only the
//! generic [`Tooltip`] component. This module's `Add,
//! ButtonOperatorCall` observer looks up the matching
//! [`OperatorEntity`] and inserts a `Tooltip` carrying its label,
//! description, signature, and the live keybind.
//!
//! The keybind is read from BEI at tooltip-attach time (and refreshed
//! whenever a `Bindings` component changes) so user remaps surface
//! immediately. The link from `ButtonOperatorCall.id` to the BEI
//! action entity is the [`OperatorAction`] marker that
//! `register_operator` auto-inserts; nothing in the editor's call
//! sites needs to opt in.
//!
//! Other tooltip sources follow the same shape (one source
//! component, one `Add` observer). See
//! `src/inspector/component_tooltip.rs` for the reflection-driven
//! counterpart.

use bevy::prelude::*;
use bevy_enhanced_input::prelude::{Binding, Bindings};
use jackdaw_api_internal::lifecycle::{OperatorAction, OperatorEntity};
use jackdaw_commands::keybinds::key_display_name;
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_feathers::tooltip::Tooltip;

pub struct OperatorTooltipPlugin;

impl Plugin for OperatorTooltipPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(auto_attach_button_tooltip)
            .add_systems(Update, refresh_keybind_on_bindings_change);
    }
}

/// Derive a [`Tooltip`] from the operator backing a freshly-added
/// [`ButtonOperatorCall`] and insert it on the same entity.
/// Silently skips when the operator id doesn't resolve (e.g.
/// extension not loaded yet); the button just renders without a
/// tooltip until the next layout pass.
fn auto_attach_button_tooltip(
    trigger: On<Add, ButtonOperatorCall>,
    calls: Query<&ButtonOperatorCall>,
    operators: Query<&OperatorEntity>,
    actions: Query<(&OperatorAction, &Bindings)>,
    binding_components: Query<&Binding>,
    mut commands: Commands,
) {
    let entity = trigger.event_target();
    let Ok(call) = calls.get(entity) else {
        return;
    };
    let Some(op) = operators.iter().find(|o| o.id() == call.id.as_ref()) else {
        return;
    };
    let keybind = display_keybind(call.id.as_ref(), &actions, &binding_components);
    commands.entity(entity).insert(
        Tooltip::title(op.label())
            .with_keybind(keybind)
            .with_description(op.description())
            .with_footer(call.to_string()),
    );
}

/// When BEI bindings change for any operator's action entity (user
/// remap, extension load/unload), refresh the cached keybind string
/// on every tooltip whose `ButtonOperatorCall` matches.
fn refresh_keybind_on_bindings_change(
    changed_actions: Query<&OperatorAction, Changed<Bindings>>,
    actions: Query<(&OperatorAction, &Bindings)>,
    binding_components: Query<&Binding>,
    mut tooltips: Query<(&ButtonOperatorCall, &mut Tooltip)>,
) {
    if changed_actions.is_empty() {
        return;
    }
    for (call, mut tip) in &mut tooltips {
        if !changed_actions
            .iter()
            .any(|action| action.0 == call.id.as_ref())
        {
            continue;
        }
        let new_keybind = display_keybind(call.id.as_ref(), &actions, &binding_components);
        if tip.keybind != new_keybind {
            tip.keybind = new_keybind;
        }
    }
}

/// Walk the action entity carrying `OperatorAction(operator_id)` and
/// stringify its keyboard / mouse-button bindings. Multiple bindings
/// join with `" / "` (e.g. `Delete / Backspace`). Returns an empty
/// string when no action entity carries that id, or the operator's
/// only bindings are mouse-motion / wheel which the tooltip skips.
pub fn display_keybind(
    operator_id: &str,
    actions: &Query<(&OperatorAction, &Bindings)>,
    binding_components: &Query<&Binding>,
) -> String {
    let mut entries: Vec<String> = Vec::new();
    for (action, bindings) in actions {
        if action.0 != operator_id {
            continue;
        }
        for binding_entity in bindings {
            let Ok(binding) = binding_components.get(binding_entity) else {
                continue;
            };
            if let Some(label) = format_binding(*binding) {
                entries.push(label);
            }
        }
    }
    entries.join(" / ")
}

/// Stringify a single [`Binding`]. Returns `None` for variants the
/// tooltip deliberately skips (mouse motion, mouse wheel, gamepad
/// axes; nothing useful to surface as a single key glyph).
fn format_binding(binding: Binding) -> Option<String> {
    match binding {
        Binding::Keyboard { key, mod_keys } => {
            let key_name = key_display_name(key);
            if mod_keys.is_empty() {
                Some(key_name.to_string())
            } else {
                Some(format!("{mod_keys} + {key_name}"))
            }
        }
        Binding::MouseButton { button, mod_keys } => {
            let button_name = match button {
                MouseButton::Left => "Mouse Left",
                MouseButton::Right => "Mouse Right",
                MouseButton::Middle => "Mouse Middle",
                MouseButton::Back => "Mouse Back",
                MouseButton::Forward => "Mouse Forward",
                MouseButton::Other(_) => return None,
            };
            if mod_keys.is_empty() {
                Some(button_name.to_string())
            } else {
                Some(format!("{mod_keys} + {button_name}"))
            }
        }
        // MouseMotion / MouseWheel / GamepadButton / GamepadAxis /
        // AnyKey / None: not usefully expressible as a static label
        // in the tooltip.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_enhanced_input::prelude::ModKeys;

    #[test]
    fn keyboard_binding_no_modifier() {
        let binding = Binding::Keyboard {
            key: KeyCode::KeyR,
            mod_keys: ModKeys::empty(),
        };
        assert_eq!(format_binding(binding).as_deref(), Some("R"));
    }

    #[test]
    fn keyboard_binding_with_modifier() {
        let binding = Binding::Keyboard {
            key: KeyCode::KeyW,
            mod_keys: ModKeys::CONTROL | ModKeys::SHIFT,
        };
        assert_eq!(format_binding(binding).as_deref(), Some("Ctrl + Shift + W"),);
    }

    #[test]
    fn mouse_button_no_modifier() {
        let binding = Binding::MouseButton {
            button: MouseButton::Back,
            mod_keys: ModKeys::empty(),
        };
        assert_eq!(format_binding(binding).as_deref(), Some("Mouse Back"));
    }

    #[test]
    fn mouse_button_with_modifier() {
        let binding = Binding::MouseButton {
            button: MouseButton::Left,
            mod_keys: ModKeys::ALT,
        };
        assert_eq!(format_binding(binding).as_deref(), Some("Alt + Mouse Left"));
    }

    #[test]
    fn unsupported_variants_return_none() {
        assert!(
            format_binding(Binding::MouseMotion {
                mod_keys: ModKeys::empty(),
            })
            .is_none(),
        );
        assert!(
            format_binding(Binding::MouseWheel {
                mod_keys: ModKeys::empty(),
            })
            .is_none(),
        );
        assert!(format_binding(Binding::AnyKey).is_none());
        assert!(format_binding(Binding::None).is_none());
    }
}
