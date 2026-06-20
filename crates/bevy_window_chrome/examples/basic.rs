//! Basic demo for custom window chrome.
//!
//! Run with: `cargo run -p bevy_window_chrome --example basic`

use bevy::picking::Pickable;
use bevy::prelude::*;
use bevy::window::WindowPlugin;
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
use bevy_window_chrome::CaptionFont;
use bevy_window_chrome::{
    WindowChromePlugin, WindowChromeTheme, primary_window_attributes, spawn_window_shell,
};

#[derive(Component, Copy, Clone)]
struct DemoRoot;

fn main() -> AppExit {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(primary_window_attributes()),
            ..default()
        }))
        .add_plugins(WindowChromePlugin::new(WindowChromeTheme::default()))
        .add_systems(Startup, setup)
        .run()
}

fn setup(
    mut commands: Commands,
    theme: Res<WindowChromeTheme>,
    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
    caption_font: Res<CaptionFont>,
) {
    let slots = spawn_window_shell(
        &mut commands,
        &theme,
        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
        caption_font,
        DemoRoot,
    );

    commands.entity(slots.title_bar).with_children(|title_bar| {
        title_bar.spawn((
            Text::new("bevy_window_chrome"),
            Pickable::IGNORE,
            Node {
                margin: UiRect::all(Val::Px(12.0)),
                ..default()
            },
        ));
    });

    commands.entity(slots.body).with_children(|body| {
        body.spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            children![(Text::new("Hello!"),)],
        ));
    });
}
