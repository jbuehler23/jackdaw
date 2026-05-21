use bevy_color::prelude::*;
use bevy_ecs::prelude::*;
use bevy_picking::prelude::*;
use bevy_text::prelude::*;
use bevy_ui::prelude::*;
use bevy_ui_widgets::observe;
use bevy_utils::prelude::*;

use jackdaw_widgets::list_view::{ListItem, ListItemContent, ListView};

use crate::tokens;

/// Styled list view container (vertical column with left indent)
pub fn list_view() -> impl Bundle {
    (
        ListView,
        Node {
            flex_direction: FlexDirection::Column,
            padding: UiRect::left(px(tokens::SPACING_LG)),
            ..default()
        },
    )
}

/// Styled list item row: \[index\] label + content area + hover effects
pub fn list_item(index: usize) -> impl Bundle {
    (
        ListItem { index },
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(tokens::SPACING_SM),
            padding: UiRect::axes(px(tokens::SPACING_XS), px(1.0)),
            width: percent(100),
            ..default()
        },
        BackgroundColor(Color::NONE),
        children![
            // Index label
            (
                Text::new(format!("[{index}]")),
                TextFont {
                    font_size: tokens::FONT_SM,
                    ..default()
                },
                TextColor(tokens::TEXT_SECONDARY),
                Node {
                    min_width: px(28.0),
                    flex_shrink: 0.0,
                    ..default()
                },
            ),
            // Content placeholder
            (
                ListItemContent,
                Node {
                    flex_grow: 1.0,
                    ..default()
                },
            )
        ],
        // Hover effects
        observe(
            |hover: On<Pointer<Over>>, mut q: Query<&mut BackgroundColor, With<ListItem>>| {
                if let Ok(mut bg) = q.get_mut(hover.event_target()) {
                    bg.0 = tokens::HOVER_BG;
                }
            },
        ),
        observe(
            |out: On<Pointer<Out>>, mut q: Query<&mut BackgroundColor, With<ListItem>>| {
                if let Ok(mut bg) = q.get_mut(out.event_target()) {
                    bg.0 = Color::NONE;
                }
            },
        ),
    )
}
