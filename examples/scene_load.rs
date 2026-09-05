//! Load a `.bsn` scene authored in the Jackdaw editor.
//!
//! 1. Add `jackdaw_runtime` to your `Cargo.toml`
//! 2. Add `JackdawPlugin` to your app
//! 3. Spawn a `JackdawSceneRoot` with an asset server load
//!
//! The scene includes the camera, lights, brushes, and any other
//! entities saved from the editor.

use bevy::prelude::*;
use jackdaw_runtime::prelude::*;

fn main() -> AppExit {
    App::new()
        // log errors instead of panicking
        .set_error_handler(bevy::ecs::error::error)
        .add_plugins((DefaultPlugins, JackdawPlugin))
        .add_systems(Startup, setup)
        .run()
}

fn setup(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.spawn(JackdawSceneRoot(
        asset_server.load("examples/scenes/scene.bsn"),
    ));
}
