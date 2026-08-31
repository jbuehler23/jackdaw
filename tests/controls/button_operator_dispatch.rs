//! An editor button carrying a `ButtonOperatorCall` runs its operator
//! when it is activated.
//!
//! The whole chain is under test: `button()` builds a `FeathersButton`,
//! `bevy_ui_widgets` raises `Activate` on it, and the core extension's
//! `dispatch_activate_operator` observer turns that into an operator
//! call. It is the one dispatch path every toolbar, menu row and panel
//! button in the editor shares, so nothing else covers it end to end.

use crate::util;

use bevy::ui_widgets::Activate;
use jackdaw::view_modes::ViewModeSettings;
use jackdaw_feathers::button::{ButtonOperatorCall, ButtonProps, button};

/// The wireframe toggle: a parameterless operator whose whole effect is
/// one bool on a resource, so the assertion is about dispatch and not
/// about what the operator does.
const TOGGLE_WIREFRAME: &str = "view.toggle_wireframe";

#[test]
fn activating_an_operator_button_runs_its_operator() {
    let mut app = util::editor_test_app();

    let entity = app
        .world_mut()
        .spawn((
            button(ButtonProps::new("Toggle Wireframe")),
            ButtonOperatorCall::new(TOGGLE_WIREFRAME),
        ))
        .id();
    app.update();

    let before = app.world().resource::<ViewModeSettings>().wireframe;

    app.world_mut().trigger(Activate { entity });
    app.update();
    app.update();

    assert_eq!(
        app.world().resource::<ViewModeSettings>().wireframe,
        !before,
        "activating the button dispatched the operator it names",
    );
}

/// A button with no operator call is left alone, so a button that owns
/// its click through an observer of its own is not dispatched twice.
#[test]
fn a_button_without_an_operator_call_dispatches_nothing() {
    let mut app = util::editor_test_app();

    let entity = app
        .world_mut()
        .spawn(button(ButtonProps::new("Nothing")))
        .id();
    app.update();

    let before = app.world().resource::<ViewModeSettings>().wireframe;

    app.world_mut().trigger(Activate { entity });
    app.update();
    app.update();

    assert_eq!(
        app.world().resource::<ViewModeSettings>().wireframe,
        before,
        "a plain button dispatches nothing",
    );
}
