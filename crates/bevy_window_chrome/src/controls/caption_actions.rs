//! Window minimize / maximize / close actions shared by caption button implementations.

use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window, WindowCloseRequested};

use crate::primary_window_is_maximized;

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
    mut windows: Query<(Entity, &mut Window), With<PrimaryWindow>>,
) {
    if press.button != PointerButton::Primary {
        return;
    }
    if buttons.get(press.event_target()).is_err() {
        return;
    }
    let Ok((window_entity, mut window)) = windows.single_mut() else {
        return;
    };
    let next_maximized = !primary_window_is_maximized(window_entity);
    window.set_maximized(next_maximized);
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
