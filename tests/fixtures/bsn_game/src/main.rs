//! Entry point for the bsn_game e2e fixture.
//!
//! Play and the schema extractor run this binary directly. Windowless
//! PIE swaps DefaultPlugins when `JACKDAW_PIE_WINDOWLESS` is set.

use bevy::prelude::*;

fn main() -> AppExit {
    let default_plugins = jackdaw_runtime::maybe_windowless(DefaultPlugins);
    App::new()
        .add_plugins(default_plugins)
        .add_plugins(jackdaw_runtime::JackdawPlugin)
        .add_plugins(bsn_scene_game::GamePlugin)
        .run()
}
