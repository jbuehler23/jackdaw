//! The editor's remote side: the BRP server that `jd mcp`, scripts and tests
//! drive the editor through, and the debugger that connects to a running game
//! over BRP to browse and inspect it. The call and batch machinery the server
//! answers with stays in the editor core, where saved recipes run it too.
//!
//! Add [`RemoteEditorPlugin`] after the editor's own plugins, and
//! [`server::JackdawEditorRemotePlugin`] to the editor process only.

mod brp;
mod connection;
pub mod debug;
pub mod entity_browser;
pub mod paced;
pub mod panel;
mod registry_fetch;
pub mod remote_inspector;
pub mod server;

use bevy::prelude::*;
use jackdaw_api::DefaultArea;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ExtensionAppExt as _;

pub use connection::{ConnectionManager, ConnectionState};
pub use jackdaw_editor_core::remote_mode::REMOTE_EDITOR_EXTENSION_ID;

/// The remote debugger's state, systems and windows. The BRP server is
/// [`server::JackdawEditorRemotePlugin`], added apart from this so a test app
/// never binds the port.
pub struct RemoteEditorPlugin;

impl Plugin for RemoteEditorPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((RemoteConnectionPlugin, debug::RemoteDebugPlugin))
            .add_observer(panel::fill_connection_indicator_slot);
        app.register_extension::<RemoteEditorExtension>();
    }
}

/// The remote debugger's dock windows.
#[derive(Default)]
pub struct RemoteEditorExtension;

impl JackdawExtension for RemoteEditorExtension {
    fn id(&self) -> String {
        REMOTE_EDITOR_EXTENSION_ID.to_string()
    }

    fn label(&self) -> String {
        "Remote Debugger".to_string()
    }

    fn description(&self) -> String {
        "Browse, inspect and profile a running game over BRP.".to_string()
    }

    fn kind(&self) -> ExtensionKind {
        ExtensionKind::Builtin
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_window(
            WindowDescriptor::new("jackdaw.remote.entities")
                .with_name("Remote Entities")
                .with_default_area(DefaultArea::Left)
                .with_priority(20)
                .with_build(|window| {
                    window.spawn(crate::entity_browser::remote_debug_workspace_content());
                }),
        );

        ctx.register_window(
            WindowDescriptor::new("jackdaw.debug.diagnostics")
                .with_name("Remote Diagnostics")
                .with_default_area(DefaultArea::Left)
                .with_priority(21)
                .with_build(|window| {
                    crate::debug::diagnostics::build_diagnostics_window(window);
                }),
        );

        ctx.register_window(
            WindowDescriptor::new("jackdaw.debug.queries")
                .with_name("Remote Queries")
                .with_default_area(DefaultArea::Center)
                .with_priority(22)
                .with_build(|window| {
                    window.spawn(crate::debug::queries::queries_panel_content());
                }),
        );

        ctx.register_window(
            WindowDescriptor::new("jackdaw.debug.archetypes")
                .with_name("Remote Archetypes")
                .with_default_area(DefaultArea::Center)
                .with_priority(23)
                .with_build(|window| {
                    window.spawn(crate::debug::archetypes::archetypes_panel_content());
                }),
        );

        ctx.register_window(
            WindowDescriptor::new("jackdaw.debug.schedules")
                .with_name("Remote Schedules")
                .with_default_area(DefaultArea::Center)
                .with_priority(24)
                .with_build(|window| {
                    window.spawn(crate::debug::schedules::schedules_panel_content());
                }),
        );

        ctx.register_window(
            WindowDescriptor::new("jackdaw.debug.graph")
                .with_name("Remote System Graph")
                .with_default_area(DefaultArea::Center)
                .with_priority(25)
                .with_build(|window| {
                    window.spawn(crate::debug::depgraph::depgraph_panel_content());
                }),
        );

        ctx.register_window(
            WindowDescriptor::new("jackdaw.debug.relationships")
                .with_name("Remote Relationships")
                .with_default_area(DefaultArea::Center)
                .with_priority(26)
                .with_build(|window| {
                    window.spawn(crate::debug::relationships::relationships_panel_content());
                }),
        );

        ctx.register_window(
            WindowDescriptor::new("jackdaw.remote.inspector")
                .with_name("Remote Inspector")
                .with_default_area(DefaultArea::RightSidebar)
                .with_priority(20)
                .with_build(|window| {
                    window.spawn(crate::remote_inspector::remote_inspector());
                }),
        );
    }
}

/// The connection to a running game, its streamed entities and the remote
/// inspector.
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
                OnEnter(jackdaw_editor_core::AppState::Editor),
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
                    .run_if(in_state(jackdaw_editor_core::AppState::Editor)),
            )
            .add_observer(panel::on_connection_indicator_click)
            .add_observer(entity_browser::on_remote_tree_node_expanded)
            .add_observer(entity_browser::on_remote_tree_row_clicked);
    }
}
