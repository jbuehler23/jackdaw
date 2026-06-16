//! Interactive showcase for the editor color picker widget.
//!
//! Renders an inline picker (HSV rectangle, hue and alpha sliders, preview
//! swatch, and numeric input fields). Useful for verifying that the picker
//! shaders compile and render, independent of the full editor.

use bevy::feathers::FeathersPlugins;
use bevy::input_focus::InputDispatchPlugin;
use bevy::prelude::*;
use bevy_enhanced_input::prelude::EnhancedInputPlugin;
use jackdaw_feathers::EditorFeathersPlugin;
use jackdaw_feathers::color_picker::{ColorPickerProps, color_picker};

fn spawn(mut commands: Commands) {
    commands.spawn(Camera2d);

    commands
        .spawn(Node {
            width: percent(100.0),
            height: percent(100.0),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            ..default()
        })
        .with_child((
            Node {
                width: px(280.0),
                padding: UiRect::all(px(16.0)),
                ..default()
            },
            children![color_picker(
                ColorPickerProps::new()
                    .with_color([0.2, 0.6, 0.9, 1.0])
                    .inline()
            )],
        ));
}

fn main() -> AppExit {
    App::new()
        .set_error_handler(bevy::ecs::error::error)
        .add_plugins((
            DefaultPlugins,
            EnhancedInputPlugin,
            FeathersPlugins.build().disable::<InputDispatchPlugin>(),
            EditorFeathersPlugin,
        ))
        .add_systems(Startup, spawn)
        .insert_resource(ClearColor(jackdaw_feathers::tokens::WINDOW_BG))
        .run()
}
