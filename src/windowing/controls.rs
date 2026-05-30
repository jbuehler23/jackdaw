//! Minimize / maximize / close caption buttons.

use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window, WindowCloseRequested};
use jackdaw_feathers::{
    button::{ButtonClickEvent, ButtonSize, ButtonVariant, IconButtonProps, icon_button},
    icons::Icon,
};

use crate::EditorEntity;

use super::primary_window_is_maximized;

#[derive(Component)]
pub(crate) struct WindowControlsMinimize;

#[derive(Component)]
pub(crate) struct WindowControlsMaximize;

#[derive(Component)]
pub(crate) struct WindowControlsClose;

pub struct WindowControlsPlugin;

impl Plugin for WindowControlsPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_minimize_click)
            .add_observer(on_maximize_click)
            .add_observer(on_close_click);
    }
}

fn caption_button(
    icon: Icon,
    marker: impl Bundle,
    icon_font: Handle<Font>,
    variant: ButtonVariant,
) -> impl Bundle {
    (
        marker,
        EditorEntity,
        icon_button(
            IconButtonProps::new(icon)
                .variant(variant)
                .with_size(ButtonSize::Icon),
            &icon_font,
        ),
    )
}

/// Minimize / maximize / close cluster for the top chrome row.
pub fn window_controls(icon_font: Handle<Font>) -> impl Bundle {
    (
        EditorEntity,
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(2.0),
            flex_shrink: 0.0,
            ..default()
        },
        Pickable::IGNORE,
        children![
            caption_button(
                Icon::Minus,
                WindowControlsMinimize,
                icon_font.clone(),
                ButtonVariant::Ghost,
            ),
            caption_button(
                Icon::Maximize2,
                WindowControlsMaximize,
                icon_font.clone(),
                ButtonVariant::Ghost,
            ),
            caption_button(
                Icon::X,
                WindowControlsClose,
                icon_font,
                ButtonVariant::Close
            ),
        ],
    )
}

fn on_minimize_click(
    click: On<ButtonClickEvent>,
    buttons: Query<Entity, With<WindowControlsMinimize>>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if buttons.get(click.entity).is_err() {
        return;
    }
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    window.set_minimized(true);
}

fn on_maximize_click(
    click: On<ButtonClickEvent>,
    buttons: Query<Entity, With<WindowControlsMaximize>>,
    mut windows: Query<(Entity, &mut Window), With<PrimaryWindow>>,
) {
    if buttons.get(click.entity).is_err() {
        return;
    }
    let Ok((window_entity, mut window)) = windows.single_mut() else {
        return;
    };
    #[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
    let next_maximized = !primary_window_is_maximized(window_entity);
    #[cfg(any(target_arch = "wasm32", target_os = "ios", target_os = "android"))]
    let next_maximized = true;
    window.set_maximized(next_maximized);
}

fn on_close_click(
    click: On<ButtonClickEvent>,
    buttons: Query<Entity, With<WindowControlsClose>>,
    windows: Query<Entity, With<PrimaryWindow>>,
    mut close_events: MessageWriter<WindowCloseRequested>,
) {
    if buttons.get(click.entity).is_err() {
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };
    close_events.write(WindowCloseRequested { window });
}
