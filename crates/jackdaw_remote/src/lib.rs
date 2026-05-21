mod methods;
pub mod scene_snapshot;
pub mod schema;

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_remote::{RemotePlugin, http::RemoteHttpPlugin};
use methods::jackdaw_app_info_handler;
use scene_snapshot::scene_snapshot_handler;

pub mod prelude {
    pub use crate::JackdawRemotePlugin;
}

/// Default BRP HTTP port for Jackdaw remote connections.
pub const DEFAULT_PORT: u16 = 15702;

/// Plugin for game-side BRP integration with the Jackdaw editor.
///
/// Game devs add this to their app to expose the game's type registry
/// and ECS state to the editor over HTTP via BRP.
///
/// # Example
/// ```rust,ignore
/// app.add_plugins(JackdawRemotePlugin::default());
/// ```
pub struct JackdawRemotePlugin {
    /// BRP HTTP port (default: 15702).
    pub port: u16,
    /// App name for identification in the editor.
    pub app_name: Option<String>,
}

impl Default for JackdawRemotePlugin {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            app_name: None,
        }
    }
}

impl JackdawRemotePlugin {
    /// Set the HTTP port for BRP.
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Set the app name displayed in the editor.
    pub fn with_app_name(mut self, name: impl Into<String>) -> Self {
        self.app_name = Some(name.into());
        self
    }
}

/// Resource storing app metadata exposed via the `jackdaw/app_info` BRP method.
#[derive(Resource, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct JackdawAppInfo {
    pub app_name: String,
    pub bevy_version: String,
}

impl Plugin for JackdawRemotePlugin {
    fn build(&self, app: &mut App) {
        let app_name = self
            .app_name
            .clone()
            .unwrap_or_else(|| "Bevy Game".to_string());

        app.insert_resource(JackdawAppInfo {
            app_name,
            bevy_version: "0.18".to_string(),
        });

        if !app.is_plugin_added::<RemotePlugin>() {
            app.add_plugins(
                RemotePlugin::default()
                    .with_method("jackdaw/app_info", jackdaw_app_info_handler)
                    .with_method("jackdaw/scene_snapshot", scene_snapshot_handler),
            );
            app.add_plugins(RemoteHttpPlugin::default().with_port(self.port));
        }

        app.add_systems(Startup, methods::generate_component_definitions);
    }

    fn finish(&self, app: &mut App) {
        // If RemotePlugin was already added by the game before us,
        // inject our custom methods via the RemoteMethods resource.
        // We check if our method is already registered by attempting to get it.
        use bevy_remote::RemoteMethods;

        let world = app.world_mut();

        // Check which methods need registering (release the borrow before mutating)
        let needs_app_info;
        let needs_scene_snapshot;
        if let Some(methods) = world.get_resource::<RemoteMethods>() {
            needs_app_info = methods.get("jackdaw/app_info").is_none();
            needs_scene_snapshot = methods.get("jackdaw/scene_snapshot").is_none();
        } else {
            return;
        }

        if needs_app_info {
            let system_id = world.register_system(jackdaw_app_info_handler);
            world.resource_mut::<RemoteMethods>().insert(
                "jackdaw/app_info",
                bevy_remote::RemoteMethodSystemId::Instant(system_id),
            );
        }
        if needs_scene_snapshot {
            let system_id = world.register_system(scene_snapshot_handler);
            world.resource_mut::<RemoteMethods>().insert(
                "jackdaw/scene_snapshot",
                bevy_remote::RemoteMethodSystemId::Instant(system_id),
            );
        }
    }
}
