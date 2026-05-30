//! Shared window header shell for the editor and project launcher.
//!
//! Has window controls, drag region, and jackdaw repo button.

use bevy::prelude::*;
use jackdaw_feathers::tokens;

use crate::{
    EditorEntity, repo_link,
    window_chrome::{self, WindowChromeDragRegion},
};

#[derive(Component)]
pub struct WindowHeaderRoot;

#[derive(Clone, Copy)]
pub struct WindowHeaderStyle {
    pub background: Color,
    pub border_bottom: bool,
}

impl Default for WindowHeaderStyle {
    fn default() -> Self {
        Self {
            background: tokens::WINDOW_BG,
            border_bottom: false,
        }
    }
}

impl WindowHeaderStyle {
    pub fn launcher() -> Self {
        Self {
            background: tokens::PANEL_HEADER_BG,
            border_bottom: true,
        }
    }
}

#[allow(dead_code)]
enum ChromeSide {
    Start,
    End,
}

/// Header with fixed caption/repo chrome and caller-owned content between them.
pub fn window_header(
    icon_font: Handle<Font>,
    jackdaw_icon: Handle<Image>,
    style: WindowHeaderStyle,
    content: impl Bundle,
) -> impl Bundle {
    let border = if style.border_bottom {
        UiRect::bottom(px(1.0))
    } else {
        UiRect::ZERO
    };

    return (
        WindowHeaderRoot,
        EditorEntity,
        Node {
            position_type: PositionType::Relative,
            width: percent(100),
            height: px(tokens::WINDOW_HEADER_HEIGHT),
            flex_shrink: 0.0,
            border,
            ..default()
        },
        BackgroundColor(style.background),
        BorderColor::all(tokens::BORDER_SUBTLE),
        Pickable::IGNORE,
        children![
            header_drag_layer(),
            header_foreground_row(
                chrome_start_slot(icon_font.clone(), jackdaw_icon.clone()),
                content_slot(content),
                chrome_end_slot(icon_font, jackdaw_icon),
            ),
        ],
    );
}

#[cfg(target_os = "macos")]
fn chrome_start_slot(icon_font: Handle<Font>, jackdaw_icon: Handle<Image>) -> impl Bundle {
    let _ = jackdaw_icon;
    return chrome_controls_slot(icon_font);
}

#[cfg(not(target_os = "macos"))]
fn chrome_start_slot(icon_font: Handle<Font>, jackdaw_icon: Handle<Image>) -> impl Bundle {
    let _ = icon_font;
    return chrome_repo_slot(jackdaw_icon, ChromeSide::Start);
}

#[cfg(target_os = "macos")]
fn chrome_end_slot(icon_font: Handle<Font>, jackdaw_icon: Handle<Image>) -> impl Bundle {
    let _ = icon_font;
    return chrome_repo_slot(jackdaw_icon, ChromeSide::End);
}

#[cfg(not(target_os = "macos"))]
fn chrome_end_slot(icon_font: Handle<Font>, jackdaw_icon: Handle<Image>) -> impl Bundle {
    let _ = jackdaw_icon;
    return chrome_controls_slot(icon_font);
}

fn header_drag_layer() -> impl Bundle {
    (
        WindowChromeDragRegion,
        EditorEntity,
        Node {
            position_type: PositionType::Absolute,
            top: px(0.0),
            left: px(0.0),
            width: percent(100),
            height: percent(100),
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.001)),
    )
}

fn header_foreground_row(
    start: impl Bundle,
    content: impl Bundle,
    end: impl Bundle,
) -> impl Bundle {
    (
        EditorEntity,
        Pickable::IGNORE,
        GlobalZIndex(1),
        Node {
            position_type: PositionType::Absolute,
            top: px(0.0),
            left: px(0.0),
            width: percent(100),
            height: percent(100),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            ..default()
        },
        children![start, content, end],
    )
}

fn content_slot(content: impl Bundle) -> impl Bundle {
    (
        EditorEntity,
        Pickable::IGNORE,
        Node {
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_width: px(0.0),
            height: percent(100),
            overflow: Overflow::clip(),
            ..default()
        },
        children![content],
    )
}

fn chrome_repo_slot(jackdaw_icon: Handle<Image>, side: ChromeSide) -> impl Bundle {
    let padding = match side {
        ChromeSide::Start => UiRect::left(px(tokens::SPACING_MD)),
        ChromeSide::End => UiRect::right(px(tokens::SPACING_MD)),
    };
    return (
        EditorEntity,
        Pickable::IGNORE,
        Node {
            flex_shrink: 0.0,
            flex_grow: 0.0,
            padding,
            height: percent(100),
            align_items: AlignItems::Center,
            ..default()
        },
        children![repo_link::jackdaw_link_button(jackdaw_icon)],
    );
}

fn chrome_controls_slot(icon_font: Handle<Font>) -> impl Bundle {
    let padding = if window_chrome::controls_on_left() {
        UiRect::left(px(tokens::SPACING_MD))
    } else {
        UiRect::right(px(tokens::SPACING_MD))
    };
    return (
        EditorEntity,
        Pickable::IGNORE,
        Node {
            flex_shrink: 0.0,
            flex_grow: 0.0,
            padding,
            height: percent(100),
            align_items: AlignItems::Center,
            ..default()
        },
        children![window_chrome::window_controls(icon_font)],
    );
}
