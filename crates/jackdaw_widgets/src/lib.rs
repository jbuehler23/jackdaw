pub mod collapsible;
pub mod context_menu;
pub mod file_browser;
pub mod list_view;
pub mod menu_bar;
pub mod radial_menu;
pub mod split_panel;
pub mod tree_view;

pub use radial_menu::{
    RadialIconFont, RadialMenu, RadialMenuItem, RadialMenuOpen, RadialMenuPlugin, RadialMenuSelect,
    RadialMenuState, RadialWedge, cancel_radial_menu, confirm_radial_menu, highlighted_index,
    open_radial_menu, wedge_angle,
};

use bevy::app::{PluginGroup, PluginGroupBuilder};

pub struct EditorWidgetsPlugins;

impl PluginGroup for EditorWidgetsPlugins {
    fn build(self) -> bevy::app::PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add(split_panel::SplitPanelPlugin)
            .add(tree_view::TreeViewPlugin)
            .add(list_view::ListViewPlugin)
            .add(context_menu::ContextMenuPlugin)
            .add(file_browser::FileBrowserPlugin)
            .add(menu_bar::MenuBarPlugin)
            .add(collapsible::CollapsiblePlugin)
    }
}
