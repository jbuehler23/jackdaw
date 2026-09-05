pub mod bsn_methods;
pub mod diagnostics;
pub mod ecs_methods;
mod methods;
pub mod playback;
pub mod scene_snapshot;
pub mod schema;

use bevy::{
    prelude::*,
    remote::{RemotePlugin, http::RemoteHttpPlugin},
};
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
///
/// When the host app adds `RemoteHttpPlugin` itself, it must configure CORS
/// headers itself for the editor to reach BRP; see
/// `RemoteHttpPlugin::with_headers`.
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
            bevy_version: "0.19".to_string(),
        });

        app.init_resource::<playback::PlaybackStepState>();
        app.add_systems(
            First,
            playback::playback_step_system.before(bevy::time::TimeSystems),
        );

        if !app.is_plugin_added::<RemotePlugin>() {
            app.add_plugins(
                RemotePlugin::default()
                    .with_method_main("jackdaw/app_info", jackdaw_app_info_handler)
                    .with_method_main("jackdaw/scene_snapshot", scene_snapshot_handler)
                    .with_method_main(
                        "jackdaw/diagnostics",
                        diagnostics::jackdaw_diagnostics_handler,
                    )
                    .with_method_main("jackdaw/playback", playback::jackdaw_playback_handler)
                    .with_method_main("jackdaw/apply_bsn", bsn_methods::jackdaw_apply_bsn_handler)
                    .with_method_main(
                        "jackdaw/entity_bsn",
                        bsn_methods::jackdaw_entity_bsn_handler,
                    )
                    .with_method_main(
                        "jackdaw/archetypes",
                        ecs_methods::jackdaw_archetypes_handler,
                    )
                    .with_method_main("jackdaw/schedules", ecs_methods::jackdaw_schedules_handler),
            );
            let cors = bevy::remote::http::Headers::new()
                .insert("Access-Control-Allow-Origin", "*")
                .insert("Access-Control-Allow-Headers", "Content-Type")
                .insert("Access-Control-Allow-Methods", "POST, OPTIONS");
            app.add_plugins(
                RemoteHttpPlugin::default()
                    .with_port(self.port)
                    .with_headers(cors),
            );
        }

        if !app.is_plugin_added::<bevy::diagnostic::FrameTimeDiagnosticsPlugin>() {
            app.add_plugins(bevy::diagnostic::FrameTimeDiagnosticsPlugin::default());
        }

        app.add_systems(Startup, methods::generate_component_definitions);
    }

    fn finish(&self, app: &mut App) {
        // If RemotePlugin was already added by the game before us,
        // inject our custom methods via the RemoteMethods resource.
        use bevy::remote::RemoteMethods;

        let world = app.world_mut();
        if world.get_resource::<RemoteMethods>().is_none() {
            return;
        }

        register_if_missing(world, "jackdaw/app_info", |w| {
            let id = w.register_system(jackdaw_app_info_handler);
            bevy::remote::RemoteMethodSystemId::Instant(id)
        });
        register_if_missing(world, "jackdaw/scene_snapshot", |w| {
            let id = w.register_system(scene_snapshot_handler);
            bevy::remote::RemoteMethodSystemId::Instant(id)
        });
        register_if_missing(world, "jackdaw/diagnostics", |w| {
            let id = w.register_system(diagnostics::jackdaw_diagnostics_handler);
            bevy::remote::RemoteMethodSystemId::Instant(id)
        });
        register_if_missing(world, "jackdaw/playback", |w| {
            let id = w.register_system(playback::jackdaw_playback_handler);
            bevy::remote::RemoteMethodSystemId::Instant(id)
        });
        register_if_missing(world, "jackdaw/apply_bsn", |w| {
            let id = w.register_system(bsn_methods::jackdaw_apply_bsn_handler);
            bevy::remote::RemoteMethodSystemId::Instant(id)
        });
        register_if_missing(world, "jackdaw/entity_bsn", |w| {
            let id = w.register_system(bsn_methods::jackdaw_entity_bsn_handler);
            bevy::remote::RemoteMethodSystemId::Instant(id)
        });
        register_if_missing(world, "jackdaw/archetypes", |w| {
            let id = w.register_system(ecs_methods::jackdaw_archetypes_handler);
            bevy::remote::RemoteMethodSystemId::Instant(id)
        });
        register_if_missing(world, "jackdaw/schedules", |w| {
            let id = w.register_system(ecs_methods::jackdaw_schedules_handler);
            bevy::remote::RemoteMethodSystemId::Instant(id)
        });
    }
}

/// Register a BRP method into `RemoteMethods` unless the name is taken.
fn register_if_missing(
    world: &mut World,
    name: &str,
    register: impl FnOnce(&mut World) -> bevy::remote::RemoteMethodSystemId,
) {
    use bevy::remote::RemoteMethods;
    if world.resource::<RemoteMethods>().get(name).is_some() {
        return;
    }
    let id = register(world);
    world.resource_mut::<RemoteMethods>().insert(name, id);
}
