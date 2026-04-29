//! `{{project-name}}` — a Jackdaw game (dylib linkage).
//!
//! Game logic lives in this file as a vanilla Bevy [`Plugin`]. The
//! crate produces a `cdylib` artefact that the editor's loader
//! dlopens at startup; the [`export_game_plugin!`] macro emits the
//! `jackdaw_game_entry_v1` symbol the loader looks up.
//!
//! Plugin code is byte-identical to a stand-alone Bevy game — no
//! jackdaw-specific imports beyond the FFI export line.

use bevy::prelude::*;

#[derive(Default)]
pub struct {{crate_name | upper_camel_case}}Plugin;

impl Plugin for {{crate_name | upper_camel_case}}Plugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_player);
        app.add_systems(Update, move_player);
    }
}

// Emits the FFI factory the editor's dylib loader looks up. Required
// for the `cdylib` install path; not used by static linkage.
jackdaw::export_game_plugin!({{crate_name | upper_camel_case}}Plugin);

#[derive(Component)]
struct Player;

fn spawn_player(mut commands: Commands) {
    commands.spawn((
        Name::new("Player"),
        Player,
        Transform::default(),
        Visibility::default(),
    ));
}

#[cfg_attr(feature = "hot-reload", bevy_simple_subsecond_system::hot)]
fn move_player(time: Res<Time>, mut players: Query<&mut Transform, With<Player>>) {
    for mut transform in &mut players {
        transform.translation.x += 1.0 * time.delta_secs();
    }
}
