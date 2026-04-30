mod brp;
mod connection;
pub mod entity_browser;
pub mod panel;
mod registry_fetch;
pub mod remote_inspector;

use bevy::prelude::*;

pub use connection::{ConnectionManager, ConnectionState};

pub struct RemoteConnectionPlugin;

impl Plugin for RemoteConnectionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ConnectionManager>()
            .init_resource::<entity_browser::RemoteSceneCache>()
            .init_resource::<entity_browser::RemoteProxyIndex>()
            .init_resource::<entity_browser::RemoteTreeRowIndex>()
            .init_resource::<entity_browser::RemoteSelection>()
            .init_resource::<entity_browser::RemoteSnapshotPollTimer>()
            .add_systems(
                OnEnter(crate::AppState::Editor),
                entity_browser::setup_remote_name_watcher,
            )
            .add_systems(
                Update,
                (
                    connection::poll_connection_tasks,
                    connection::heartbeat_system,
                    registry_fetch::poll_registry_task,
                    panel::update_connection_status_indicator,
                    entity_browser::snapshot_poll_timer,
                    entity_browser::poll_snapshot_task,
                    entity_browser::cleanup_remote_proxies,
                    remote_inspector::populate_remote_proxy,
                    remote_inspector::build_remote_inspector_displays,
                )
                    .chain()
                    .run_if(in_state(crate::AppState::Editor)),
            )
            .add_observer(panel::on_connection_indicator_click)
            .add_observer(entity_browser::on_remote_tree_node_expanded)
            .add_observer(entity_browser::on_remote_tree_row_clicked);
    }
}
