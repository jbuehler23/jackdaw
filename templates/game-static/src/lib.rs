//! `{{project-name}}` — a Jackdaw game (static linkage).
//!
//! Game logic lives in this file as a vanilla Bevy [`Plugin`]. The
//! same plugin runs in two contexts:
//!
//! - **Standalone** (`cargo play`): the plugin is added to `App`'s
//!   main schedules; the binary runs the game directly.
//! - **Editor** (`cargo editor`): the plugin is installed against the
//!   editor's `GameSubApp`. The user's `Update` / `Startup` /
//!   `FixedUpdate` schedules tick only when Play is engaged; the
//!   editor's authoring world stays untouched.
//!
//! Plugin code is byte-identical to a stand-alone Bevy game — no
//! jackdaw-specific imports, no schedule labels, no PlayState gating.

use bevy::prelude::*;

#[derive(Default)]
pub struct {{crate_name | upper_camel_case}}Plugin;

impl Plugin for {{crate_name | upper_camel_case}}Plugin {
    fn build(&self, app: &mut App) {
        // Spawned once per Play transition (in editor) or once at app
        // start (standalone). Use this for setting up player, camera,
        // initial level, etc.
        app.add_systems(Startup, spawn_player);

        // Per-frame gameplay tick.
        app.add_systems(Update, move_player);
    }
}

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
        // Edit this constant, save, and (with `--features hot-reload`)
        // see the change reflected without restarting the editor.
        transform.translation.x += 1.0 * time.delta_secs();
    }
}
