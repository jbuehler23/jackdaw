//! What a fly look drag does to the pointer.
//!
//! A look is a relative gesture, so the pointer is held for its duration
//! and handed back when the drag ends. Without that it walks out of the
//! viewport mid-turn and the drag finishes against whatever it lands on.

use crate::util;

use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow, WindowFocused};
use jackdaw::viewport::CameraFlyActive;

fn cursor(app: &mut App) -> CursorOptions {
    app.world_mut()
        .query_filtered::<&CursorOptions, With<Window>>()
        .iter(app.world())
        .next()
        .expect("the app has a window")
        .clone()
}

fn primary_window(app: &mut App) -> Entity {
    app.world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .iter(app.world())
        .next()
        .expect("the app has a primary window")
}

/// Start a look drag and settle, so the pointer is held.
fn start_flying(app: &mut App) {
    app.world_mut().resource_mut::<CameraFlyActive>().0 = true;
    app.update();
    assert_eq!(cursor(app).grab_mode, CursorGrabMode::Locked);
}

#[test]
fn the_pointer_is_held_for_the_drag_and_released_after_it() {
    let mut app = util::editor_test_app();
    assert_eq!(cursor(&mut app).grab_mode, CursorGrabMode::None);

    app.world_mut().resource_mut::<CameraFlyActive>().0 = true;
    app.update();
    let held = cursor(&mut app);
    assert_eq!(held.grab_mode, CursorGrabMode::Locked);
    assert!(!held.visible, "a held pointer is not drawn");

    app.world_mut().resource_mut::<CameraFlyActive>().0 = false;
    app.update();
    let released = cursor(&mut app);
    assert_eq!(released.grab_mode, CursorGrabMode::None);
    assert!(released.visible);
}

/// The button that ends a drag goes to whatever took focus, so nothing else
/// would ever hand the pointer back.
#[test]
fn losing_window_focus_hands_the_pointer_back() {
    let mut app = util::editor_test_app();
    start_flying(&mut app);

    let window = primary_window(&mut app);
    app.world_mut().write_message(WindowFocused {
        window,
        focused: false,
    });
    app.update();

    assert!(
        !app.world().resource::<CameraFlyActive>().0,
        "the drag ends"
    );
    let released = cursor(&mut app);
    assert_eq!(released.grab_mode, CursorGrabMode::None);
    assert!(released.visible);
}

/// A modal takes the release with it, and a dialog answered against a held
/// pointer is a dialog nobody can click.
#[test]
fn a_dialog_opening_mid_drag_hands_the_pointer_back() {
    let mut app = util::editor_test_app();
    start_flying(&mut app);

    app.world_mut()
        .commands()
        .trigger(jackdaw_feathers::dialog::OpenDialogEvent::new("Held", "OK"));
    app.world_mut().flush();
    app.update();
    app.update();

    assert!(
        !app.world().resource::<CameraFlyActive>().0,
        "the drag ends"
    );
    let released = cursor(&mut app);
    assert_eq!(released.grab_mode, CursorGrabMode::None);
    assert!(released.visible);
}
