//! End-to-end proof game: a real project library that loads an authored
//! `.bsn` scene at runtime through `JackdawPlugin`.
//!
//! `GamePlugin` spawns a `JackdawSceneRoot` pointing at `assets/scene.bsn`
//! at startup; `JackdawPlugin` (added from `main`) then parses that file
//! and spawns its entities. An update system watches for the authored
//! marker entity (`SceneNodeId(999)`) and, once it appears, prints a
//! unique stderr line carrying real counts read out of the live world.

use bevy::prelude::*;
use jackdaw_runtime::prelude::*;
use jackdaw_scene_types::{SceneNodeId, types::ScatterGroup};

/// A minimal editor extension type kept in this fixture for
/// `jackdaw_extension` API surface coverage beside the game plugin.
#[derive(Default)]
pub struct BundleFixtureExtension;

impl jackdaw_extension::JackdawExtension for BundleFixtureExtension {
    fn id(&self) -> String {
        "bundle_fixture".to_string()
    }

    fn register(&self, _ctx: &mut jackdaw_extension::ExtensionRegistrar<'_>) {}
}

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
    scatter_groups: Query<&ScatterGroup>,
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
    // Scatter provenance is scene data, so a runtime that cannot apply it
    // loads the scene with the groups stripped back to bare models. The
    // keys are reported so the test reads what arrived rather than what
    // failed to be complained about.
    let scatter: Vec<String> = scatter_groups
        .iter()
        .map(|group| group.key.clone())
        .collect();
    eprintln!(
        "BSN_SCENE_LOADED entities={} node_ids={} target={} has_target={} ids={:?} names={:?} \
         scatter_keys={:?}",
        transforms.iter().count(),
        ids.len(),
        target,
        ids.contains(&target),
        ids,
        names,
        scatter,
    );
    *done = true;
}
