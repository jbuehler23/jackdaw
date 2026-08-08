use bevy::prelude::*;

/// The probe target. Never referenced by any registration code.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
pub struct AutoRegisteredComponent {
    pub strength: f32,
    pub label: String,
}

/// The game's plugin. Plain Bevy code; schema extraction reads the
/// link-time reflect inventory rather than anything this plugin does.
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, announce);
    }
}

fn announce(mut commands: Commands) {
    commands.spawn(AutoRegisteredComponent {
        strength: 1.0,
        label: "schema".into(),
    });
}
