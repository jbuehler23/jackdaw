//! A synthetic click reaches an editor control the way a real one does: the whole
//! chain from `input.pointer` to the operator the `FeathersButton` names, with
//! nothing triggered by hand along the way.

use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::{
    prelude::*,
    ui::UiGlobalTransform,
    window::{PrimaryWindow, WindowResolution},
};
use jackdaw::{test_input::SyntheticInput, view_modes::ViewModeSettings};
use jackdaw_feathers::button::{ButtonOperatorCall, ButtonProps, button};

/// The wireframe toggle, as in `button_operator_dispatch`: a
/// parameterless operator whose whole effect is one bool, so the
/// assertion is about the click and not about what the operator does.
const TOGGLE_WIREFRAME: &str = "view.toggle_wireframe";

fn settle(app: &mut App) {
    for _ in 0..8 {
        app.update();
    }
}

/// Advance until the queued gesture has been delivered.
fn play(app: &mut App) {
    for _ in 0..200 {
        app.update();
        if app.world().resource::<SyntheticInput>().is_idle() {
            break;
        }
    }
    settle(app);
}

/// Where `entity` is drawn, in the window logical pixels
/// `input.pointer` takes.
fn centre_of(app: &App, entity: Entity) -> Vec2 {
    let transform = app
        .world()
        .get::<UiGlobalTransform>(entity)
        .expect("the button is placed");
    let computed = app
        .world()
        .get::<ComputedNode>(entity)
        .expect("the button is laid out");
    transform.translation * computed.inverse_scale_factor() * app.world().resource::<UiScale>().0
}

#[test]
fn a_synthetic_click_activates_the_button_under_it() {
    let mut app = util::editor_test_app();
    {
        let mut windows = app
            .world_mut()
            .query_filtered::<&mut Window, With<PrimaryWindow>>();
        let mut window = windows
            .single_mut(app.world_mut())
            .expect("headless apps still have a primary window");
        window.resolution = WindowResolution::new(1600, 1000);
    }
    let entity = app
        .world_mut()
        .spawn((
            button(ButtonProps::new("Toggle Wireframe")),
            ButtonOperatorCall::new(TOGGLE_WIREFRAME),
        ))
        .id();
    settle(&mut app);

    let before = app.world().resource::<ViewModeSettings>().wireframe;
    let at = centre_of(&app, entity);

    jackdaw::boot_ops::run_op_clause(
        app.world_mut(),
        &format!("input.pointer x={} y={} action=click", at.x, at.y),
    )
    .expect("the clause dispatches")
    .assert_finished();
    play(&mut app);

    assert_eq!(
        app.world().resource::<ViewModeSettings>().wireframe,
        !before,
        "the click reached the button and it dispatched its operator",
    );
}

/// A click that lands nowhere near the button leaves it alone, so the
/// test above is measuring the position and not merely the dispatch.
#[test]
fn a_click_beside_the_button_activates_nothing() {
    let mut app = util::editor_test_app();
    {
        let mut windows = app
            .world_mut()
            .query_filtered::<&mut Window, With<PrimaryWindow>>();
        let mut window = windows
            .single_mut(app.world_mut())
            .expect("headless apps still have a primary window");
        window.resolution = WindowResolution::new(1600, 1000);
    }
    app.world_mut().spawn((
        button(ButtonProps::new("Toggle Wireframe")),
        ButtonOperatorCall::new(TOGGLE_WIREFRAME),
    ));
    settle(&mut app);

    let before = app.world().resource::<ViewModeSettings>().wireframe;
    jackdaw::boot_ops::run_op_clause(app.world_mut(), "input.pointer x=1500 y=950 action=click")
        .expect("the clause dispatches")
        .assert_finished();
    play(&mut app);

    assert_eq!(
        app.world().resource::<ViewModeSettings>().wireframe,
        before,
        "a click on nothing dispatches nothing",
    );
}
