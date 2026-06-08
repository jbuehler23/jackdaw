//! Window minimize / maximize / close actions shared by caption button implementations.

use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window, WindowCloseRequested};

use crate::window::toggle_primary_window_maximized;

use super::{WindowControlsClose, WindowControlsMaximize, WindowControlsMinimize};

pub(crate) fn register_pointer_handlers(app: &mut App) {
    app.add_observer(on_minimize_press)
        .add_observer(on_maximize_press)
        .add_observer(on_close_press);
}

fn on_minimize_press(
    press: On<Pointer<Press>>,
    buttons: Query<Entity, With<WindowControlsMinimize>>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if press.button != PointerButton::Primary {
        return;
    }
    if buttons.get(press.event_target()).is_err() {
        return;
    }
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    window.set_minimized(true);
}

fn on_maximize_press(
    press: On<Pointer<Press>>,
    buttons: Query<Entity, With<WindowControlsMaximize>>,
    windows: Query<(Entity, &mut Window), With<PrimaryWindow>>,
) {
    if press.button != PointerButton::Primary {
        return;
    }
    if buttons.get(press.event_target()).is_err() {
        return;
    }
    toggle_primary_window_maximized(windows);
}

fn on_close_press(
    press: On<Pointer<Press>>,
    buttons: Query<Entity, With<WindowControlsClose>>,
    windows: Query<Entity, With<PrimaryWindow>>,
    mut close_events: MessageWriter<WindowCloseRequested>,
) {
    if press.button != PointerButton::Primary {
        return;
    }
    if buttons.get(press.event_target()).is_err() {
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };
    close_events.write(WindowCloseRequested { window });
}
