//! Loads extension builds made outside the editor.
//!
//! `jd build` builds an extension project the same way the editor does and
//! records the library it produced under `.jackdaw/`. While the project is
//! open, a change to that recorded library is handed to
//! [`crate::extension_build`] to load, so a build started from a terminal
//! replaces the running extension like one started from inside the editor.

use std::time::Duration;

use bevy::prelude::*;

use crate::project::ProjectRoot;

/// Watches the open extension project's recorded build while in
/// `AppState::Editor`.
pub struct HotReloadPlugin;

impl Plugin for HotReloadPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HotReloadEnabled>()
            .init_resource::<RecordedBuildWatch>()
            .add_systems(
                Update,
                watch_recorded_build.run_if(in_state(crate::AppState::Editor)),
            );
    }
}

/// File-menu toggle. Off keeps the currently loaded library; builds made
/// outside the editor are ignored until it is flipped back on.
#[derive(Resource)]
pub struct HotReloadEnabled(pub bool);

impl Default for HotReloadEnabled {
    fn default() -> Self {
        Self(true)
    }
}

/// Throttle for the build-record poll, so it stats the library about
/// twice a second rather than every frame.
#[derive(Resource)]
struct RecordedBuildWatch(Timer);

impl Default for RecordedBuildWatch {
    fn default() -> Self {
        Self(Timer::new(Duration::from_millis(500), TimerMode::Repeating))
    }
}

fn watch_recorded_build(world: &mut World) {
    if !world.resource::<HotReloadEnabled>().0 {
        return;
    }
    let delta = world.resource::<Time>().delta();
    if !world
        .resource_mut::<RecordedBuildWatch>()
        .0
        .tick(delta)
        .just_finished()
    {
        return;
    }
    let Some(root) = world
        .get_resource::<ProjectRoot>()
        .map(|project| project.root.clone())
    else {
        return;
    };
    if !crate::extension_build::is_extension_project(world, &root, false) {
        return;
    }
    if crate::extension_build::load_changed_recorded_build(world, &root) {
        info!("Extension rebuilt outside the editor; loading it");
    }
}
