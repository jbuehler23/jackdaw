//! Bevy-driven caption buttons for jackdaw's client-side chrome (Linux).
//!
//! The minimize/maximize/close visuals use jackdaw's feathers buttons; window actions are wired
//! through the markers re-exported by [`bevy_window_chrome`].

use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window, WindowCloseRequested};
use bevy_window_chrome::{
    WindowControlsClose, WindowControlsMaximize, WindowControlsMinimize,
    primary_window_is_maximized,
};
use jackdaw_feathers::{
    button::{ButtonClickEvent, ButtonSize, ButtonVariant, IconButtonProps, icon_button},
    icons::Icon,
};

use crate::EditorEntity;

pub fn register(app: &mut App) {
    app.add_observer(on_minimize_click)
        .add_observer(on_maximize_click)
        .add_observer(on_close_click);
}

fn caption_button(
    icon: Icon,
    marker: impl Bundle,
    icon_font: Handle<Font>,
    variant: ButtonVariant,
) -> impl Bundle {
    return (
        marker,
        EditorEntity,
        icon_button(
            IconButtonProps::new(icon)
                .variant(variant)
                .with_size(ButtonSize::Icon),
            &icon_font,
        ),
    );
}

/// Minimize / maximize / close cluster for the top chrome row.
pub fn window_controls_interactive(icon_font: Handle<Font>) -> impl Bundle {
    return (
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
    );
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
    let next_maximized = !primary_window_is_maximized(window_entity);
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
