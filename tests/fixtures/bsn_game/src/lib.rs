//! End-to-end proof game: a real project library that loads an authored
//! `.bsn` scene at runtime through `JackdawPlugin`.
//!
//! `GamePlugin` spawns a `JackdawSceneRoot` pointing at `assets/scene.bsn`
//! at startup; the SDK's `JackdawPlugin` (added by
//! `jackdaw_api::__run_project_game`) then parses that file and spawns its
//! entities. An update system watches for the authored marker entity
//! (`SceneNodeId(999)`) and, once it appears, prints a unique stderr line
//! carrying real counts read out of the live world.

use bevy::prelude::*;
use jackdaw_runtime::prelude::*;
use jackdaw_scene_types::SceneNodeId;

#[derive(Default)]
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_scene)
            .add_systems(Update, report_scene_loaded);
    }
}

/// Load the authored scene. `JackdawPlugin` spawns its entities as children
/// of this root once the async asset load completes.
fn spawn_scene(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.spawn(JackdawSceneRoot(asset_server.load("scene.bsn")));
    eprintln!("BSN_GAME_STARTED");
}

/// The authored entity to wait for. Defaults to the committed fixture scene's
/// marker; callers that author their own scene (the editor journey) pass the
/// id the editor minted for theirs.
fn target_node_id() -> u64 {
    std::env::var("JACKDAW_E2E_NODE_ID")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(999)
}

/// Once the authored marker entity has spawned from the `.bsn`, emit a single
/// unique line with counts taken from the live world. Waiting on a specific id
/// rather than "any entity" avoids reporting a half-spawned scene.
fn report_scene_loaded(
    mut done: Local<bool>,
    node_ids: Query<&SceneNodeId>,
    names: Query<&Name>,
    transforms: Query<Entity, With<Transform>>,
) {
    if *done {
        return;
    }
    let ids: Vec<u64> = node_ids.iter().map(|n| n.0).collect();
    let target = target_node_id();
    // Wait until the specific authored entity from the scene file exists.
    if !ids.contains(&target) {
        return;
    }
    let names: Vec<String> = names.iter().map(|n| n.as_str().to_owned()).collect();
    eprintln!(
        "BSN_SCENE_LOADED entities={} node_ids={} target={} has_target={} ids={:?} names={:?}",
        transforms.iter().count(),
        ids.len(),
        target,
        ids.contains(&target),
        ids,
        names,
    );
    *done = true;
}
