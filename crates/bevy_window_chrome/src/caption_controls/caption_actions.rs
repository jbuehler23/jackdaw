//! Window minimize / maximize / close actions shared by caption button implementations.

use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window, WindowCloseRequested};

use super::CaptionButton;
use crate::window::toggle_primary_window_maximized;

pub(crate) fn register_pointer_handlers(app: &mut App) {
    app.add_observer(on_caption_button_press);
}

fn on_caption_button_press(
    press: On<Pointer<Press>>,
    buttons: Query<&CaptionButton>,
    mut windows: Query<(Entity, &mut Window), With<PrimaryWindow>>,
    mut close_events: MessageWriter<WindowCloseRequested>,
) {
    if press.button != PointerButton::Primary {
        return;
    }
    let Ok(kind) = buttons.get(press.event_target()) else {
        return;
    };
    match *kind {
        CaptionButton::Minimize => {
            let Ok((_, mut window)) = windows.single_mut() else {
                return;
            };
            window.set_minimized(true);
        }
        CaptionButton::Maximize => {
            toggle_primary_window_maximized(windows);
        }
        CaptionButton::Close => {
            let Ok((window_entity, _)) = windows.single_mut() else {
                return;
            };
            close_events.write(WindowCloseRequested {
                window: window_entity,
            });
        }
    }
}
