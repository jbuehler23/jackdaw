use crate::PROTOCOL_ID;
use bevy::prelude::*;
use lightyear::prelude::client::{ClientPlugins, InputDelayConfig, NetcodeClient, NetcodeConfig};
use lightyear::prelude::{
    AppChannelExt, Authentication, ChannelMode, ChannelSettings, Client, Connect,
    InputTimelineConfig, Link, LocalAddr, NetworkDirection, PeerAddr, ReliableSettings,
    ReplicationReceiver, UdpIo,
};
use std::net::SocketAddr;

/// Minimum client-side input delay (in ticks). Inputs are buffered for
/// `current_tick + input_delay` so they reach the server BEFORE it simulates that
/// tick. With prediction OFF the client timeline only runs `rtt/2 + jitter` ahead
/// of the server (lightyear's `InputTimeline::sync_objective`); on a low-latency
/// link that margin rounds to ~0 ticks, so a client-tick-T input lands in the
/// server's buffer at T while the server is already simulating T+1, and the input
/// is never applied. A small minimum input delay closes that gap (and damps jitter
/// on a real network).
const MIN_INPUT_DELAY_TICKS: u16 = 3;

/// Turnkey multiplayer CLIENT plugin. Owns connect + input-marker placement +
/// interpolation (automatic).
///
/// `client_id` must be DISTINCT per client (the netcode handshake keys on it).
/// It is a required field (no `Default`) so callers are forced to choose unique
/// ids; a silent default of 0 would let two clients collide.
pub struct JackdawMultiplayerClientPlugin {
    /// Server address to connect to.
    pub server: SocketAddr,
    /// Unique netcode client id (distinct per connecting client).
    pub client_id: u64,
    /// Network tick duration. MUST match the server's `tick`; a mismatch
    /// silently diverges lightyear's sync/replication timelines. Default 50ms
    /// (matches `JackdawMultiplayerServerPlugin`'s default).
    pub tick: std::time::Duration,
}

/// Runtime config for the client layer, read by the startup connect system.
#[derive(Resource)]
struct ClientConfig {
    server: SocketAddr,
    client_id: u64,
}

impl Plugin for JackdawMultiplayerClientPlugin {
    fn build(&self, app: &mut App) {
        // The client reads/renders replicated world positions via `GlobalTransform`,
        // which only propagates from `Transform` under `TransformPlugin`, absent from
        // `MinimalPlugins` on a headless client. The `is_plugin_added` guard makes
        // this a no-op when the game already brought it in (e.g. via `DefaultPlugins`).
        if !app.is_plugin_added::<bevy::transform::TransformPlugin>() {
            app.add_plugins(bevy::transform::TransformPlugin);
        }
        app.add_plugins(ClientPlugins {
            tick_duration: self.tick,
        });
        // Mirror of the server's RPC channel; both ends must register it.
        app.add_channel::<crate::rpc::RpcChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            send_frequency: std::time::Duration::default(),
            priority: 1.0,
        })
        .add_direction(NetworkDirection::Bidirectional);
        app.insert_resource(ClientConfig {
            server: self.server,
            client_id: self.client_id,
        });
        app.add_systems(Startup, connect_client);
        // The gate owns rig activation: only the locally-controlled actor's
        // rig is active. Without claiming ownership, the rig crate's lone-rig
        // convenience re-activates any single non-player rig (e.g. one
        // authored in a zone) every frame while the gate revokes it, and the
        // camera flickers on and off.
        app.insert_resource(jackdaw_camera_rig::CameraRigActivation::Gated);
        app.add_systems(Update, crate::camera_gate::sync_active_camera);
    }
}

/// Spawn + connect the lightyear client entity (UDP transport + netcode) on startup.
fn connect_client(mut commands: Commands, config: Res<ClientConfig>) {
    let client_addr: SocketAddr = "127.0.0.1:0".parse().expect("valid client bind addr");
    let netcode = NetcodeClient::new(
        Authentication::Manual {
            server_addr: config.server,
            client_id: config.client_id,
            // Must match the server's default key ([0; 32]); see PROTOCOL_ID agreement.
            private_key: [0u8; 32],
            protocol_id: PROTOCOL_ID,
        },
        NetcodeConfig::default(),
    )
    .expect("netcode client config is valid");

    let client = commands
        .spawn((
            Client::default(),
            LocalAddr(client_addr),
            PeerAddr(config.server),
            Link::new(None),
            netcode,
            UdpIo::default(),
            // The client entity must hold a `ReplicationReceiver` to receive
            // replicated state (mirrors the server attaching a `ReplicationSender`
            // per `LinkOf`). `ClientPlugins` does NOT add this automatically.
            ReplicationReceiver::default(),
            // Override the default `InputTimelineConfig` (which carries
            // `InputDelayConfig::no_input_delay()`) with a fixed minimum input delay
            // so client inputs reach the server in time to be applied (see
            // `MIN_INPUT_DELAY_TICKS`). `InputTimelineConfig` is a required component
            // of `Client`, so this explicit value replaces the default.
            // `fixed_input_delay` pins min = max = the given ticks.
            InputTimelineConfig::default()
                .with_input_delay(InputDelayConfig::fixed_input_delay(MIN_INPUT_DELAY_TICKS)),
        ))
        .id();
    commands.trigger(Connect { entity: client });
}
