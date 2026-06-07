//! Window title bar shell: caption controls, drag region, and an empty content slot.

use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window};

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
use crate::caption_controls::window_controls;
use crate::{WindowChromeEntity, WindowChromeTheme};

#[derive(Component)]
pub struct WindowTitleBarRoot;

#[derive(Component)]
pub struct WindowTitleBarContentSlot;

#[derive(Component)]
pub struct WindowTitleBarDragRegion;

/// Window title bar chrome with an empty [`WindowTitleBarContentSlot`]. Returns the slot entity.
pub fn spawn_window_title_bar(
    parent: &mut ChildSpawnerCommands,
    theme: &WindowChromeTheme,
    caption_font: Handle<Font>,
) -> Entity {
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "freebsd")))]
    let _ = caption_font;
    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
    let caption_controls = window_controls(theme, caption_font);
    #[cfg(target_os = "macos")]
    let macos_traffic_light_inset = theme.macos_traffic_light_inset;

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    let title_bar_border_radius = BorderRadius::top(px(theme.linux_corner_radius));
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    let title_bar_border_radius = BorderRadius::ZERO;

    let title_bar_root = (
        WindowTitleBarRoot,
        WindowChromeEntity,
        BackgroundColor(theme.window_background),
        Node {
            position_type: PositionType::Relative,
            width: percent(100),
            height: px(theme.title_bar_height),
            border_radius: title_bar_border_radius,
            overflow: Overflow::clip(),
            ..default()
        },
    );
    let mut title_bar_slot = None::<Entity>;
    parent.spawn(title_bar_root).with_children(|title_bar| {
        title_bar_slot = Some(spawn_foreground_row(
            title_bar,
            #[cfg(target_os = "macos")]
            macos_traffic_light_inset,
            #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
            caption_controls,
        ));
    });
    return title_bar_slot.expect("window title bar content slot spawned");
}

fn spawn_foreground_row(
    parent: &mut ChildSpawnerCommands,
    #[cfg(target_os = "macos")] macos_traffic_light_inset: f32,
    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
    caption_controls: impl Bundle,
) -> Entity {
    let mut title_bar_slot = None::<Entity>;
    parent
        .spawn((
            WindowChromeEntity,
            Pickable::IGNORE,
            Node {
                position_type: PositionType::Absolute,
                top: px(0.0),
                left: px(0.0),
                width: percent(100),
                height: percent(100),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Stretch,
                ..default()
            },
        ))
        .with_children(|row| {
            row.spawn(title_bar_drag_backplate());
            title_bar_slot = Some(
                row.spawn(title_bar_content_slot(
                    #[cfg(target_os = "macos")]
                    macos_traffic_light_inset,
                ))
                .id(),
            );
            #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
            row.spawn(caption_controls_slot(caption_controls));
        });
    return title_bar_slot.expect("window title bar content slot spawned");
}

fn title_bar_drag_backplate() -> impl Bundle {
    return (
        WindowTitleBarDragRegion,
        WindowChromeEntity,
        Node {
            position_type: PositionType::Absolute,
            top: px(0.0),
            left: px(0.0),
            right: px(0.0),
            height: percent(100),
            ..default()
        },
    );
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
fn caption_controls_slot(caption_controls: impl Bundle) -> impl Bundle {
    return (
        WindowChromeEntity,
        Pickable::IGNORE,
        Node {
            flex_shrink: 0.0,
            height: percent(100),
            align_items: AlignItems::Stretch,
            ..default()
        },
        children![caption_controls],
    );
}

fn title_bar_content_slot(
    #[cfg(target_os = "macos")] macos_traffic_light_inset: f32,
) -> impl Bundle {
    #[cfg(target_os = "macos")]
    let padding = UiRect {
        left: px(macos_traffic_light_inset),
        ..default()
    };
    #[cfg(not(target_os = "macos"))]
    let padding = UiRect::ZERO;

    return (
        WindowTitleBarContentSlot,
        WindowChromeEntity,
        Pickable::IGNORE,
        Node {
            flex_grow: 1.0,
            min_width: px(0.0),
            height: percent(100),
            overflow: Overflow::clip(),
            padding,
            ..default()
        },
    );
}

pub(crate) fn on_drag_region_press(
    press: On<Pointer<Press>>,
    drag_regions: Query<Entity, With<WindowTitleBarDragRegion>>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if press.button != PointerButton::Primary {
        return;
    }
    if drag_regions.get(press.event_target()).is_err() {
        return;
    }
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    window.start_drag_move();
}
