use crate::PROTOCOL_ID;
use crate::rooms::ZoneRooms;
use bevy::prelude::*;
use lightyear::prelude::server::{NetcodeConfig, NetcodeServer, ServerPlugins, ServerUdpIo, Start};
use lightyear::prelude::{
    AppChannelExt, ChannelMode, ChannelSettings, LocalAddr, NetworkDirection, ReliableSettings,
    RoomPlugin,
};
use std::net::SocketAddr;

/// Which zones this server process hosts. The MVP only uses `All`; the field is
/// the seam a future sharded deployment uses to host a subset of zones.
#[derive(Clone, Debug, Default)]
pub enum HostedZones {
    /// Host every zone in the loaded world (single-process / one-shard).
    #[default]
    All,
    /// Host only these zone ids (future: sharding).
    Only(Vec<u64>),
}

/// Turnkey multiplayer SERVER plugin. Headless. Owns transport + lifecycle +
/// auto-spawn + rooms + zone transitions + movement scheduling.
pub struct JackdawMultiplayerServerPlugin {
    /// Address the server binds (UDP).
    pub bind: SocketAddr,
    /// Network tick duration.
    pub tick: std::time::Duration,
    /// Which zones this process hosts (MVP: `All`).
    pub hosted_zones: HostedZones,
    /// Per-connection inbound-RPC cap (messages/second). `None` = unlimited.
    pub max_msgs_per_sec: Option<u32>,
    /// Whether to auto-spawn a player on connect (`SpawnPolicy::OnConnect`, the default)
    /// or leave spawning to the game (`SpawnPolicy::Manual`).
    pub spawn_policy: crate::lifecycle::SpawnPolicy,
}

impl Default for JackdawMultiplayerServerPlugin {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:0".parse().expect("valid default bind addr"),
            tick: std::time::Duration::from_millis(50),
            hosted_zones: HostedZones::All,
            max_msgs_per_sec: None,
            spawn_policy: crate::lifecycle::SpawnPolicy::OnConnect,
        }
    }
}

/// Runtime config for the server layer. Holds the zones this process hosts (read
/// by Phase 4 room/zone wiring) and the bind address (read by the startup system).
#[derive(Resource)]
pub(crate) struct ServerConfig {
    #[expect(
        dead_code,
        reason = "read by Phase 4 zone/room wiring to decide which zones this process hosts; captured now so the sharding seam exists"
    )]
    pub hosted_zones: HostedZones,
    pub bind: SocketAddr,
}

impl Plugin for JackdawMultiplayerServerPlugin {
    fn build(&self, app: &mut App) {
        // The server reads authored world positions (SpawnPoint / ZoneTransition)
        // via `GlobalTransform`, which only propagates from `Transform` under
        // `TransformPlugin`, absent from `MinimalPlugins` on a headless server. The
        // `is_plugin_added` guard makes this a no-op when the game already brought
        // it in (e.g. via `DefaultPlugins`).
        if !app.is_plugin_added::<bevy::transform::TransformPlugin>() {
            app.add_plugins(bevy::transform::TransformPlugin);
        }
        app.add_plugins(ServerPlugins {
            tick_duration: self.tick,
        });
        // One reliable, ordered channel every turnkey RPC rides (login/select must
        // never be dropped or reordered). Registered by both turnkey plugins; on a
        // dedicated server + separate client each app adds exactly one plugin, so
        // it is registered once per app.
        app.add_channel::<crate::rpc::RpcChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            send_frequency: std::time::Duration::default(),
            priority: 1.0,
        })
        .add_direction(NetworkDirection::Bidirectional);
        app.insert_resource(crate::rate_limit::RpcRateLimit(self.max_msgs_per_sec));
        app.init_resource::<crate::rate_limit::RpcInboundCounts>();
        app.insert_resource(crate::rate_limit::RpcWindow(Timer::from_seconds(
            1.0,
            TimerMode::Repeating,
        )));
        app.add_systems(Update, crate::rate_limit::reset_rpc_counts);
        // RoomPlugin is NOT pulled in by ServerPlugins; rooms need it explicitly.
        app.add_plugins(RoomPlugin);
        app.init_resource::<ZoneRooms>();
        app.insert_resource(ServerConfig {
            hosted_zones: self.hosted_zones.clone(),
            bind: self.bind,
        });
        app.insert_resource(self.spawn_policy);
        app.add_plugins(crate::lifecycle::ServerLifecyclePlugin);
        app.add_plugins(crate::transitions::ZoneTransitionPlugin);
        app.add_systems(Startup, spawn_server);
    }
}

/// Spawn + start the lightyear server entity (UDP transport + netcode) on startup.
fn spawn_server(mut commands: Commands, config: Res<ServerConfig>) {
    let server = commands
        .spawn((
            NetcodeServer::new(NetcodeConfig::default().with_protocol_id(PROTOCOL_ID)),
            LocalAddr(config.bind),
            ServerUdpIo::default(),
        ))
        .id();
    commands.trigger(Start { entity: server });
}
