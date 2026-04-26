pub mod alert;
pub mod button;
pub mod checkbox;
pub mod collapsible;
pub mod color_picker;
pub mod combobox;
pub mod context_menu;
pub mod cursor;
pub mod dialog;
pub mod file_browser;
pub mod icons;
pub mod inspector_field;
pub mod list_view;
pub mod menu_bar;
pub mod panel_header;
pub mod panel_section;
pub mod popover;
pub mod progress;
pub mod scroll;
pub mod separator;
pub mod split_panel;
pub mod status_bar;
pub mod text_edit;
pub mod toast;
pub mod tokens;
pub mod tooltip;
pub mod tree_view;
pub mod utils;
pub mod variant_edit;
pub mod vector_edit;

use bevy::app::Plugin;

pub struct EditorFeathersPlugin;

impl Plugin for EditorFeathersPlugin {
    fn build(&self, app: &mut bevy::app::App) {
        // text_edit::plugin adds TextInputPlugin which adds InputDispatchPlugin,
        // so we must not add InputDispatchPlugin ourselves.
        app.add_plugins((
            jackdaw_widgets::EditorWidgetsPlugins,
            split_panel::SplitPanelPlugin,
            icons::IconFontPlugin,
            cursor::plugin,
            button::plugin,
            checkbox::plugin,
            popover::plugin,
            combobox::plugin,
            dialog::plugin,
            text_edit::plugin,
            panel_section::plugin,
            inspector_field::plugin,
            variant_edit::plugin,
            scroll::plugin,
            toast::plugin,
        ));
        app.add_plugins((
            alert::plugin,
            color_picker::plugin,
            menu_bar::plugin,
            context_menu::plugin,
            panel_header::plugin,
            tooltip::TooltipPlugin,
        ));
    }
}
