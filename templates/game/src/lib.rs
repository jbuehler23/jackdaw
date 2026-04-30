use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Component, Reflect, Serialize, Deserialize, Default, Debug, Clone)]
#[reflect(Component, Serialize, Deserialize)]
pub struct Player {
    pub speed: f32,
}

pub struct MyGamePlugin;

impl Plugin for MyGamePlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Player>();
        app.add_systems(Startup, spawn_player);
        app.add_systems(Update, move_player);
    }
}

fn spawn_player(mut commands: Commands) {
    commands.spawn((
        Player { speed: 5.0 },
        Camera3d::default(),
        Transform::from_xyz(0.0, 4.0, 8.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

fn move_player(_time: Res<Time>, _players: Query<&mut Transform, With<Player>>) {
    // Stub: users replace this with real gameplay.
}

jackdaw::export_game_plugin!(MyGamePlugin);
