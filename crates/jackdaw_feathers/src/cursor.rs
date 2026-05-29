use bevy::feathers::cursor::CursorIconPlugin;
use bevy::prelude::*;

pub use bevy::feathers::cursor::{EntityCursor, OverrideCursor};

pub fn plugin(app: &mut App) {
    if !app.is_plugin_added::<CursorIconPlugin>() {
        app.add_plugins(CursorIconPlugin);
    }
}
