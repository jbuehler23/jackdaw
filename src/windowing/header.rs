//! Shared window header shell for the editor and project launcher.
//!
//! Has window controls, drag region, and jackdaw repo button.

use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window};
use jackdaw_feathers::tokens;

use crate::EditorEntity;

use super::repo_link;

use super::controls;

#[derive(Component)]
pub struct WindowHeaderRoot;

pub struct WindowHeaderPlugin;

impl Plugin for WindowHeaderPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_drag_region_press);
    }
}

/// Header with fixed caption/repo chrome and caller-owned content between them.
pub fn window_header(
    icon_font: Handle<Font>,
    jackdaw_icon: Handle<Image>,
    content: impl Bundle,
) -> impl Bundle {
    // order controls to the left on mac
    #[cfg(target_os = "macos")]
    let foreground_row = (
        EditorEntity,
        Pickable::IGNORE,
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
        children![
            chrome_controls_slot(icon_font.clone()),
            content_slot(content),
            chrome_repo_slot(jackdaw_icon),
        ],
    );

    #[cfg(not(target_os = "macos"))]
    let foreground_row = (
        EditorEntity,
        Pickable::IGNORE,
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
        children![
            chrome_repo_slot(jackdaw_icon.clone()),
            content_slot(content),
            chrome_controls_slot(icon_font),
        ],
    );

    return (
        WindowHeaderRoot,
        EditorEntity,
        Node {
            position_type: PositionType::Relative,
            width: percent(100),
            height: px(tokens::WINDOW_HEADER_HEIGHT),
            ..default()
        },
        BackgroundColor(tokens::WINDOW_BG),
        children![foreground_row],
    );
}

fn content_slot(content: impl Bundle) -> impl Bundle {
    (
        EditorEntity,
        Pickable::IGNORE,
        Node {
            flex_grow: 1.0,
            min_width: px(0.0),
            height: percent(100),
            overflow: Overflow::clip(),
            ..default()
        },
        children![content],
    )
}

fn chrome_repo_slot(jackdaw_icon: Handle<Image>) -> impl Bundle {
    #[cfg(target_os = "macos")]
    let padding = UiRect::right(px(tokens::SPACING_MD));
    #[cfg(not(target_os = "macos"))]
    let padding = UiRect::left(px(tokens::SPACING_MD));

    return (
        EditorEntity,
        Pickable::IGNORE,
        Node {
            flex_shrink: 0.0,
            padding,
            height: percent(100),
            align_items: AlignItems::Center,
            ..default()
        },
        children![repo_link::jackdaw_link_button(jackdaw_icon)],
    );
}

fn chrome_controls_slot(icon_font: Handle<Font>) -> impl Bundle {
    #[cfg(target_os = "macos")]
    let padding = UiRect::left(px(tokens::SPACING_MD));
    #[cfg(not(target_os = "macos"))]
    let padding = UiRect::right(px(tokens::SPACING_MD));

    return (
        EditorEntity,
        Pickable::IGNORE,
        Node {
            flex_shrink: 0.0,
            padding,
            height: percent(100),
            align_items: AlignItems::Center,
            ..default()
        },
        children![controls::window_controls(icon_font)],
    );
}

fn on_drag_region_press(
    press: On<Pointer<Press>>,
    headers: Query<Entity, With<WindowHeaderRoot>>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if press.button != PointerButton::Primary {
        return;
    }
    if headers.get(press.original_event_target()).is_err() {
        return;
    }
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    window.start_drag_move();
}
