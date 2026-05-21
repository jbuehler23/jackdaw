use bevy_app::prelude::*;
use bevy_camera::Camera2d;
use bevy_ecs::prelude::*;
use bevy_internal::DefaultPlugins;
use bevy_ui::prelude::*;
use jackdaw_feathers::{EditorFeathersPlugin, split_panel};

fn main() -> AppExit {
    App::new()
        .add_plugins((DefaultPlugins, EditorFeathersPlugin))
        .add_systems(Startup, spawn_panels)
        .run()
}

fn spawn_panels(mut commands: Commands) {
    commands.spawn(Camera2d);
    commands.spawn((
        Node {
            width: percent(100),
            height: percent(100),
            flex_direction: FlexDirection::Row,
            ..Default::default()
        },
        split_panel::panel_group(
            0.2,
            (
                Spawn((split_panel::panel(1), Text::new("Panel 1"))),
                Spawn(split_panel::panel_handle()),
                Spawn((split_panel::panel(2.5), Text::new("Panel 2"))),
                Spawn(split_panel::panel_handle()),
                Spawn((split_panel::panel(1), Text::new("Panel 3"))),
            ),
        ),
    ));
}
