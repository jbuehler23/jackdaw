use bevy_ecs::prelude::*;
use bevy_feathers::theme::ThemedText;
use bevy_text::prelude::*;
use bevy_ui::prelude::*;

use crate::tokens;

/// Marker for the status bar root node.
#[derive(Component)]
pub struct StatusBar;

/// Marker for the left status text (selection info).
#[derive(Component)]
pub struct StatusBarLeft;

/// Marker for the center status text (scene statistics).
#[derive(Component)]
pub struct StatusBarCenter;

/// Marker for the right status text (gizmo mode, scene path).
#[derive(Component)]
pub struct StatusBarRight;

/// Build the styled status bar bundle (22px bar at bottom).
pub fn status_bar() -> impl Bundle {
    (
        StatusBar,
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            width: Val::Percent(100.0),
            height: Val::Px(tokens::STATUS_BAR_HEIGHT),
            padding: UiRect::horizontal(Val::Px(tokens::SPACING_MD)),
            flex_shrink: 0.0,
            ..Default::default()
        },
        BackgroundColor(tokens::WINDOW_BG),
        children![
            (
                StatusBarLeft,
                Text::new("Ready"),
                TextFont {
                    font_size: tokens::FONT_SM,
                    ..Default::default()
                },
                ThemedText,
            ),
            (
                StatusBarCenter,
                Text::new(""),
                TextFont {
                    font_size: tokens::FONT_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
            ),
            (
                StatusBarRight,
                Text::new(""),
                TextFont {
                    font_size: tokens::FONT_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
            )
        ],
    )
}
