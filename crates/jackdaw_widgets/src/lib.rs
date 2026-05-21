pub mod collapsible;
pub mod context_menu;
pub mod file_browser;
pub mod list_view;
pub mod menu_bar;
pub mod split_panel;
pub mod tree_view;

use bevy_app::{PluginGroup, PluginGroupBuilder};

pub struct EditorWidgetsPlugins;

impl PluginGroup for EditorWidgetsPlugins {
    fn build(self) -> bevy_app::PluginGroupBuilder {
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
