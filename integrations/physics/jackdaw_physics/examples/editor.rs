//! The editor with the Physics module built in, for working on the module
//! without installing it as a bundle.
//!
//! ```text
//! cargo run --manifest-path integrations/physics/Cargo.toml --example editor
//! ```

use bevy::app::AppExit;
use jackdaw_api_internal::lifecycle::ExtensionAppExt as _;
use jackdaw_physics::PhysicsExtension;

fn main() -> AppExit {
    jackdaw::launch::run(|app| {
        app.register_extension::<PhysicsExtension>();
    })
}
