use bevy::{prelude::*, tasks::Task, tasks::futures_lite::future};
use jackdaw_remote::{JackdawAppInfo, schema::JsnRegistry};

use super::brp;

/// Connection state machine for the remote game link.
#[derive(Debug, Clone)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected { app_info: JackdawAppInfo },
    Error(String),
}

/// Resource managing the connection to a running Bevy game via BRP.
#[derive(Resource)]
pub struct ConnectionManager {
    pub state: ConnectionState,
    /// BRP endpoint URL (default: `http://127.0.0.1:15702`).
    pub endpoint: String,
    /// Cached type registry from the connected game.
    pub registry: Option<JsnRegistry>,
    /// Seconds since last successful heartbeat.
    heartbeat_timer: f32,
}

impl Default for ConnectionManager {
    fn default() -> Self {
        Self {
            state: ConnectionState::Disconnected,
            endpoint: format!("http://127.0.0.1:{}", jackdaw_remote::DEFAULT_PORT),
            registry: None,
            heartbeat_timer: 0.0,
        }
    }
}

impl ConnectionManager {
    pub fn is_connected(&self) -> bool {
        matches!(self.state, ConnectionState::Connected { .. })
    }
}

/// In-flight connection task (`app_info` request).
#[derive(Resource)]
pub struct ConnectionTask(pub Task<Result<serde_json::Value, anyhow::Error>>);

/// In-flight heartbeat task.
#[derive(Resource)]
pub struct HeartbeatTask(pub Task<Result<serde_json::Value, anyhow::Error>>);

/// Initiate a connection attempt to the given endpoint.
pub fn start_connect(commands: &mut Commands, endpoint: &str) {
    let task = brp::brp_request(endpoint, "jackdaw/app_info", None);
    commands.insert_resource(ConnectionTask(task));
}

/// Disconnect from the remote game.
pub fn disconnect(commands: &mut Commands, manager: &mut ConnectionManager) {
    manager.state = ConnectionState::Disconnected;
    manager.registry = None;
    manager.heartbeat_timer = 0.0;
    commands.remove_resource::<ConnectionTask>();
    commands.remove_resource::<HeartbeatTask>();
    commands.remove_resource::<super::registry_fetch::RegistryFetchTask>();
    commands.remove_resource::<super::entity_browser::RemoteSnapshotTask>();
}

/// Poll the in-flight connection task for completion.
pub fn poll_connection_tasks(
    mut commands: Commands,
    mut manager: ResMut<ConnectionManager>,
    task: Option<ResMut<ConnectionTask>>,
) {
    let Some(mut task) = task else { return };

    let Some(result) = future::block_on(future::poll_once(&mut task.0)) else {
        return;
    };
    commands.remove_resource::<ConnectionTask>();

    match result {
        Ok(value) => {
            match serde_json::from_value::<JackdawAppInfo>(value) {
                Ok(app_info) => {
                    info!("Connected to remote game: {}", app_info.app_name);
                    manager.state = ConnectionState::Connected {
                        app_info: app_info.clone(),
                    };
                    manager.heartbeat_timer = 0.0;

                    // Kick off registry fetch
                    super::registry_fetch::start_registry_fetch(&mut commands, &manager.endpoint);
                }
                Err(e) => {
                    manager.state = ConnectionState::Error(format!("Invalid app_info: {e}"));
                }
            }
        }
        Err(e) => {
            manager.state = ConnectionState::Error(e.to_string());
        }
    }
}

/// Heartbeat: periodically ping the game to detect disconnection.
const HEARTBEAT_INTERVAL: f32 = 3.0;

pub fn heartbeat_system(
    mut commands: Commands,
    mut manager: ResMut<ConnectionManager>,
    time: Res<Time>,
    heartbeat: Option<ResMut<HeartbeatTask>>,
) {
    if !manager.is_connected() {
        return;
    }

    // Poll in-flight heartbeat
    if let Some(mut hb) = heartbeat {
        if let Some(result) = future::block_on(future::poll_once(&mut hb.0)) {
            commands.remove_resource::<HeartbeatTask>();
            match result {
                Ok(value) => {
                    // Update app_info if it changed
                    if let Ok(app_info) = serde_json::from_value::<JackdawAppInfo>(value) {
                        manager.state = ConnectionState::Connected { app_info };
                    }
                    manager.heartbeat_timer = 0.0;
                }
                Err(e) => {
                    warn!("Remote heartbeat failed: {e}");
                    manager.state = ConnectionState::Error(format!("Connection lost: {e}"));
                    manager.registry = None;
                }
            }
        }
        return; // Don't send another while one is in flight
    }

    // Timer
    manager.heartbeat_timer += time.delta_secs();
    if manager.heartbeat_timer >= HEARTBEAT_INTERVAL {
        manager.heartbeat_timer = 0.0;
        let task = brp::brp_request(&manager.endpoint, "jackdaw/app_info", None);
        commands.insert_resource(HeartbeatTask(task));
    }
}
