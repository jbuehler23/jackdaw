//! Shared window header shell for the editor and project launcher.
//!
//! Has window controls, drag region, and jackdaw repo button.

use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window};
use jackdaw_feathers::tokens;

use crate::{EditorEntity, repo_link, window_chrome};

#[derive(Component)]
pub struct WindowHeaderRoot;

#[derive(Component)]
pub struct WindowHeaderDragRegion;

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
    #[cfg(target_os = "macos")]
    let foreground_row = (
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
            flex_shrink: 0.0,
            ..default()
        },
        BackgroundColor(tokens::WINDOW_BG),
        BorderColor::all(tokens::BORDER_SUBTLE),
        Pickable::IGNORE,
        children![header_drag_layer(), foreground_row],
    );
}

fn header_drag_layer() -> impl Bundle {
    (
        WindowHeaderDragRegion,
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
    #[cfg(target_os = "macos")]
    let padding = UiRect::left(px(tokens::SPACING_MD));
    #[cfg(not(target_os = "macos"))]
    let padding = UiRect::right(px(tokens::SPACING_MD));

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

fn on_drag_region_press(
    press: On<Pointer<Press>>,
    regions: Query<Entity, With<WindowHeaderDragRegion>>,
    parents: Query<&ChildOf>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if press.button != PointerButton::Primary {
        return;
    }
    let Some(_region) = find_ancestor_with(press.event_target(), &regions, &parents) else {
        return;
    };
    for mut window in windows.iter_mut() {
        window.start_drag_move();
    }
}

fn find_ancestor_with(
    mut entity: Entity,
    query: &Query<Entity, With<WindowHeaderDragRegion>>,
    parents: &Query<&ChildOf>,
) -> Option<Entity> {
    loop {
        if query.contains(entity) {
            return Some(entity);
        }
        let Ok(parent) = parents.get(entity) else {
            return None;
        };
        entity = parent.parent();
    }
}
