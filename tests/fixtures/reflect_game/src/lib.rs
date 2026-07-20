//! Does `reflect_auto_register` fire across `dlopen` with the shared
//! SDK? This crate derives Reflect on a component and registers NOTHING
//! anywhere: no export macro, no registration call, no build.rs. If the
//! editor side finds `AutoRegisteredComponent` in its registry after
//! dlopen, the zero-code contract works.

use bevy::prelude::*;

/// The probe target. Never referenced by any registration code.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
pub struct AutoRegisteredComponent {
    pub strength: f32,
    pub label: String,
}

/// The game's plugin, found by the runner shim under the `GamePlugin`
/// name convention. Plain Bevy code, no jackdaw imports. The stderr
/// markers let the runner test assert that this code actually ran in
/// the child process.
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, announce).add_systems(Update, tick);
    }
}

fn announce(mut commands: Commands) {
    commands.spawn(AutoRegisteredComponent {
        strength: 1.0,
        label: "runner".into(),
    });
    eprintln!("JACKDAW_TEST_GAME_STARTED");
}

fn tick(mut ticks: Local<u32>, query: Query<&AutoRegisteredComponent>) {
    *ticks += 1;
    if *ticks == 10 {
        eprintln!("JACKDAW_TEST_GAME_TICKED count={}", query.iter().count());
    }
}
