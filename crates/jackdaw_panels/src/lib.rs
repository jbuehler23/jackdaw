pub mod add_window_popup;
pub mod area;
pub mod drag;
pub mod layout;
pub mod reconcile;
pub mod registry;
pub mod sidebar;
pub mod split;
pub mod tabs;
pub mod tree;
pub mod workspace;
pub mod workspace_tabs;

pub use area::{
    ActiveDockWindow, DockArea, DockAreaStyle, DockTab, DockTabBar, DockTabContent, DockWindow,
    IconFontHandle,
};
pub use layout::{AreaState, LayoutState};
pub use registry::{DockWindowBuildFn, DockWindowDescriptor, WindowRegistry};
pub use sidebar::{DockSidebarContainer, DockSidebarIcon};
pub use split::{Panel, PanelGroup, PanelHandle, panel, panel_group, panel_handle};
pub use workspace::{
    WorkspaceChanged, WorkspaceDescriptor, WorkspacePersist, WorkspaceRegistry, WorkspaceTab,
    WorkspaceTabStrip, WorkspacesPersist,
};

use bevy_app::prelude::*;

pub struct DockPlugin;

impl Plugin for DockPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            split::SplitPanelPlugin,
            tabs::DockTabPlugin,
            drag::DockDragPlugin,
            add_window_popup::AddWindowPopupPlugin,
            reconcile::ReconcilePlugin,
        ))
        .init_resource::<WindowRegistry>()
        .init_resource::<WorkspaceRegistry>()
        .init_resource::<workspace_tabs::WorkspaceClickTracker>()
        .init_resource::<workspace_tabs::WorkspaceListSnapshot>()
        .add_systems(
            Update,
            (
                sidebar::handle_sidebar_icon_clicks,
                workspace_tabs::populate_workspace_tabs,
                workspace_tabs::handle_workspace_tab_clicks,
                workspace_tabs::handle_add_workspace_clicks,
                workspace_tabs::show_workspace_close_on_hover,
                workspace_tabs::auto_focus_workspace_rename,
                workspace_tabs::update_workspace_tab_visuals,
            ),
        )
        .add_observer(sidebar::on_sidebar_icon_right_click)
        .add_observer(workspace_tabs::on_workspace_changed_swap_tree)
        .add_observer(workspace_tabs::on_workspace_close_click)
        .add_observer(workspace_tabs::detect_workspace_double_click)
        .add_observer(workspace_tabs::handle_workspace_rename_commit);
    }
}
