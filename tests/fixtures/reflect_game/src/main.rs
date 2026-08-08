//! Entry point for the reflect_game schema-extract fixture.
//!
//! `JackdawPlugin` answers `--jackdaw-extract-schema` before anything
//! else runs, which is how the editor learns this crate's types.

use bevy::prelude::*;

fn main() -> AppExit {
    App::new()
        .add_plugins(MinimalPlugins)
        .add_plugins(jackdaw_runtime::JackdawPlugin)
        .add_plugins(reflect_game::GamePlugin)
        .run()
}
