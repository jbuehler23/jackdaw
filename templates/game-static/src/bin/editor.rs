//! Editor runner for `{{project-name}}`.
//!
//! `cargo editor` opens Jackdaw with this game's plugin installed
//! against the `GameSubApp`. The plugin's schedules tick only when
//! `PlayState::Playing`; outside Play, the editor's authoring world
//! is the active world and gameplay is paused.

use bevy::prelude::*;
use jackdaw::prelude::EditorPlugins;
use {{crate_name}}::{{crate_name | upper_camel_case}}Plugin;

fn main() -> AppExit {
    App::new()
        .set_error_handler(bevy::ecs::error::error)
        .add_plugins(EditorPlugins::default().with_game::<{{crate_name | upper_camel_case}}Plugin>())
        .run()
}
