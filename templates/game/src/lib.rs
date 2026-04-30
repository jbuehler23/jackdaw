use bevy::prelude::*;

#[derive(Component, Reflect, Default, Debug, Clone)]
#[reflect(Component, Default)]
pub struct Player {
    pub speed: f32,
}

#[derive(Default)]
pub struct MyGamePlugin;

impl Plugin for MyGamePlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Player>();
        app.add_systems(Startup, spawn_player);
        app.add_systems(Update, orbit_player_camera);
    }
}

fn spawn_player(mut commands: Commands) {
    commands.spawn((
        Name::new("Player"),
        Player { speed: 0.6 },
        Camera3d::default(),
        Transform::from_xyz(8.0, 4.0, 0.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

// Demo: orbit the player camera around the origin so PIE motion is
// visually obvious. Authored brushes / lights stay where the user
// placed them and remain visible.
fn orbit_player_camera(time: Res<Time>, mut players: Query<(&Player, &mut Transform)>) {
    let elapsed = time.elapsed_secs();
    for (player, mut transform) in players.iter_mut() {
        let radius = 8.0;
        let height = 4.0;
        let angle = elapsed * player.speed;
        transform.translation = Vec3::new(angle.cos() * radius, height, angle.sin() * radius);
        transform.look_at(Vec3::ZERO, Vec3::Y);
    }
}

jackdaw_runtime::export_game_plugin!(MyGamePlugin);
