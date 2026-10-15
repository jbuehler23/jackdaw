//! The editor with the Navmesh module built in, for working on the module
//! without installing it as a bundle.
//!
//! ```text
//! cargo run --manifest-path integrations/navmesh/Cargo.toml --example editor
//! ```

use bevy::app::AppExit;
use jackdaw_api_internal::lifecycle::ExtensionAppExt as _;
use jackdaw_navmesh_editor::NavmeshExtension;

fn main() -> AppExit {
    jackdaw::launch::run(|app| {
        app.register_extension::<NavmeshExtension>();
    })
}
