//! Load a `.jsn` scene exported from the Jackdaw editor.
//!
//! 1. Add `jackdaw_runtime` to your `Cargo.toml`
//! 2. Add `JackdawPlugin` to your app
//! 3. Spawn a `JackdawSceneRoot` with an asset server load
//!
//! The scene includes lights, brushes, and any other entities
//! saved from the editor. You only need to provide a camera.

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
        asset_server.load("examples/scenes/scene.jsn"),
    ));

    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(5.0, 5.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}
