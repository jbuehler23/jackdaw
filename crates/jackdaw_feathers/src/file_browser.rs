use bevy::{feathers::theme::ThemedText, prelude::*};
use jackdaw_widgets::file_browser::FileBrowserItem;
use lucide_icons::Icon;

use crate::{icons::IconFont, tokens};

/// Spawn a file browser item (grid cell).
pub fn file_browser_item(item: &FileBrowserItem, icon_font: &IconFont) -> impl Bundle {
    let icon = if item.is_directory {
        Icon::Folder
    } else {
        file_icon(&item.file_name)
    };
    let icon_color = if item.is_directory {
        tokens::DIR_ICON_COLOR
    } else {
        tokens::FILE_ICON_COLOR
    };
    let file_name = item.file_name.clone();
    let font = icon_font.0.clone();

    (
        item.clone(),
        Node {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            padding: UiRect::all(Val::Px(6.0)),
            width: Val::Px(80.0),
            border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
            ..Default::default()
        },
        BackgroundColor(Color::NONE),
        children![
            // Icon
            (
                Text::new(String::from(icon.unicode())),
                TextFont {
                    font,
                    font_size: tokens::ICON_LG,
                    ..Default::default()
                },
                TextColor(icon_color),
            ),
            // File name
            (
                Text::new(truncate_name(&file_name, 12)),
                TextFont {
                    font_size: tokens::FONT_SM,
                    ..Default::default()
                },
                ThemedText,
            )
        ],
    )
}

/// Spawn a file browser item for list view mode.
pub fn file_browser_list_item(item: &FileBrowserItem, icon_font: &IconFont) -> impl Bundle {
    let icon = if item.is_directory {
        Icon::Folder
    } else {
        file_icon(&item.file_name)
    };
    let icon_color = if item.is_directory {
        tokens::DIR_ICON_COLOR
    } else {
        tokens::FILE_ICON_COLOR
    };
    let font = icon_font.0.clone();

    (
        item.clone(),
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            padding: UiRect::axes(Val::Px(tokens::SPACING_MD), Val::Px(tokens::SPACING_SM)),
            column_gap: Val::Px(tokens::SPACING_MD),
            width: Val::Percent(100.0),
            border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_SM)),
            ..Default::default()
        },
        BackgroundColor(Color::NONE),
        children![
            (
                Text::new(String::from(icon.unicode())),
                TextFont {
                    font,
                    font_size: tokens::FONT_MD,
                    ..Default::default()
                },
                TextColor(icon_color),
            ),
            (
                Text::new(item.file_name.clone()),
                TextFont {
                    font_size: tokens::FONT_MD,
                    ..Default::default()
                },
                ThemedText,
            )
        ],
    )
}

fn file_icon(name: &str) -> Icon {
    // Check for compound extensions first
    if name.ends_with(".template.json") {
        return Icon::LayoutTemplate;
    }
    let ext = name.rsplit('.').next().unwrap_or("");
    match ext {
        "gltf" | "glb" => Icon::Cuboid,
        "png" | "jpg" | "jpeg" | "bmp" | "tga" | "ktx2" | "webp" => Icon::Image,
        "json" | "ron" => Icon::FileBraces,
        "rs" => Icon::FileCode,
        "toml" => Icon::Settings,
        _ => Icon::File,
    }
}

fn truncate_name(name: &str, max_len: usize) -> String {
    if name.len() <= max_len {
        name.to_string()
    } else {
        format!("{}...", &name[..max_len - 3])
    }
}
