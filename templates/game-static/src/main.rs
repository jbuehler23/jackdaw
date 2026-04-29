//! Standalone runner for `{{project-name}}`.
//!
//! `cargo play` (or `cargo run`) runs the game without the editor.
//! The plugin's `Update` / `Startup` / `FixedUpdate` schedules tick
//! exactly like a vanilla Bevy game.

use bevy::prelude::*;
use {{crate_name}}::{{crate_name | upper_camel_case}}Plugin;

fn main() -> AppExit {
    App::new()
        .set_error_handler(bevy::ecs::error::error)
        .add_plugins((DefaultPlugins, {{crate_name | upper_camel_case}}Plugin::default()))
        .run()
}
