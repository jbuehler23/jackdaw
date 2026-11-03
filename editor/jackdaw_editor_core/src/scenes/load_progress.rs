//! The progress an opening scene reports: reading its file, spawning its
//! objects, loading the models it names and placing them in the world.
//!
//! The first two happen inside the open itself. The models come in over the
//! frames after it, from the asset server and then the world-asset spawner, so
//! `track_scene_load` counts them a few times a second until every one has
//! landed.

use std::time::{Duration, Instant};

use bevy::asset::RecursiveDependencyLoadState;
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot, WorldInstance, WorldInstanceSpawner};
use jackdaw_scene_types::GltfSource;

use crate::progress::{
    begin_progress, fail_progress, finish_progress, progress_count, progress_stage,
};

/// The owner every scene load reports under.
pub const SCENE_LOAD: &str = "scene load";

pub const READING: &str = "Reading";
pub const SPAWNING: &str = "Spawning objects";
pub const LOADING_MODELS: &str = "Loading models";
pub const PLACING_MODELS: &str = "Placing models";

/// How long the model counts may stand still before the load is called over.
/// A model the spawner never places would otherwise hold the overlay up for
/// good.
const STALL_LIMIT: Duration = Duration::from_secs(60);

/// How often the model counts are taken. A scene can name tens of thousands
/// of models, and a bar needs no more than a few updates a second.
const COUNT_INTERVAL: Duration = Duration::from_millis(250);

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        track_scene_load.run_if(bevy::time::common_conditions::on_real_timer(COUNT_INTERVAL)),
    );
}

/// Start reporting the load of the scene at `path`.
pub(crate) fn begin_scene_load(world: &mut World, path: &std::path::Path, immediate: bool) {
    let name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    begin_progress(world, SCENE_LOAD, format!("Opening {name}"), immediate);
    progress_stage(world, SCENE_LOAD, READING, None);
}

/// The file is read and parsed; spawning comes next.
pub(crate) fn scene_read(world: &mut World) {
    progress_stage(world, SCENE_LOAD, SPAWNING, None);
}

/// The tab at `target` has been activated. A refused activation ends the load
/// with its reason; otherwise the models it names start coming in.
pub(crate) fn scene_spawned(world: &mut World, target: usize) {
    let refusal = world
        .resource::<crate::scenes::Scenes>()
        .tabs
        .get(target)
        .and_then(|tab| tab.refusal.clone());
    match refusal {
        Some(refusal) => fail_scene_load(world, refusal.reason().to_string()),
        None => progress_stage(world, SCENE_LOAD, LOADING_MODELS, None),
    }
}

pub(crate) fn fail_scene_load(world: &mut World, error: impl Into<String>) {
    fail_progress(world, SCENE_LOAD, error);
}

/// What the models of the open scene have got to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ModelCounts {
    /// Distinct model files the scene names.
    files: usize,
    /// Of those, the ones the asset server is done with, loaded or failed.
    files_done: usize,
    /// Entities naming a model.
    models: usize,
    /// Of those, the ones whose model is in the world or will never be.
    models_done: usize,
}

fn load_settled(state: Option<&RecursiveDependencyLoadState>) -> bool {
    !matches!(
        state,
        Some(RecursiveDependencyLoadState::Loading | RecursiveDependencyLoadState::NotLoaded)
    )
}

fn load_failed(state: Option<&RecursiveDependencyLoadState>) -> bool {
    matches!(state, Some(RecursiveDependencyLoadState::Failed(_)))
}

fn model_counts(world: &mut World) -> ModelCounts {
    let mut models = world.query_filtered::<(
        Entity,
        Option<&WorldAssetRoot>,
        Option<&WorldInstance>,
    ), With<GltfSource>>();
    let rows: Vec<(Option<AssetId<WorldAsset>>, Option<_>)> = models
        .iter(world)
        .filter(|(entity, root, _)| {
            root.is_some() || !jackdaw_runtime::is_lod_level(world, *entity)
        })
        .map(|(_, root, instance)| {
            (
                root.map(|root| root.0.id()),
                instance.map(|instance| **instance),
            )
        })
        .collect();
    let mut ids: HashSet<AssetId<WorldAsset>> = world
        .get_resource::<crate::entity_ops::PendingModelRoots>()
        .map(|pending| pending.handles().map(Handle::id).collect())
        .unwrap_or_default();
    ids.extend(rows.iter().filter_map(|(root, _)| *root));

    let (Some(asset_server), Some(spawner)) = (
        world.get_resource::<AssetServer>(),
        world.get_resource::<WorldInstanceSpawner>(),
    ) else {
        return ModelCounts::default();
    };
    let states: HashMap<AssetId<WorldAsset>, Option<RecursiveDependencyLoadState>> = ids
        .iter()
        .map(|id| (*id, asset_server.get_recursive_dependency_load_state(*id)))
        .collect();
    let files_done = states
        .values()
        .filter(|state| load_settled(state.as_ref()))
        .count();
    let models_done = rows
        .iter()
        .filter(|(root, instance)| {
            let failed =
                root.is_some_and(|id| load_failed(states.get(&id).and_then(Option::as_ref)));
            failed || instance.is_some_and(|instance| spawner.instance_is_ready(instance))
        })
        .count();
    let (level_files, level_files_done) = world
        .get_resource::<jackdaw_scene_types::model_parts::ModelParts>()
        .map_or((0, 0), |parts| {
            (parts.len(), parts.len() - parts.loading_count())
        });
    let groups = world
        .query_filtered::<(), Or<(
            With<jackdaw_scene_types::LodGroup>,
            With<jackdaw_scene_types::model_import::ModelLevels>,
        )>>()
        .iter(world)
        .count();
    let drawn = world
        .get_resource::<jackdaw_runtime::LiveLevelProgress>()
        .map_or(0, |progress| progress.drawn);
    ModelCounts {
        files: ids.len() + level_files,
        files_done: files_done + level_files_done,
        models: rows.len() + groups,
        models_done: models_done + drawn.min(groups),
    }
}

/// Count the open scene's models, moving the load from loading them
/// to placing them and ending it once all are in.
fn track_scene_load(world: &mut World, mut last: Local<Option<(ModelCounts, Instant)>>) {
    let stage = world
        .get_resource::<crate::progress::EditorProgress>()
        .and_then(|progress| progress.get(SCENE_LOAD))
        .map(|task| task.stage.clone());
    let Some(stage) = stage.filter(|stage| stage == LOADING_MODELS || stage == PLACING_MODELS)
    else {
        *last = None;
        return;
    };
    let counts = model_counts(world);
    let stalled = match *last {
        Some((seen, since)) if seen == counts => since.elapsed() >= STALL_LIMIT,
        _ => {
            *last = Some((counts, Instant::now()));
            false
        }
    };
    if stalled {
        warn!(
            "scene load: {} of {} models never came in; the load is over without them",
            counts.models - counts.models_done,
            counts.models
        );
        finish_progress(world, SCENE_LOAD);
        return;
    }

    if stage == LOADING_MODELS {
        if counts.files_done < counts.files {
            progress_count(world, SCENE_LOAD, counts.files_done, Some(counts.files));
            return;
        }
        progress_stage(world, SCENE_LOAD, PLACING_MODELS, Some(counts.models));
    }
    if counts.models_done < counts.models {
        progress_count(world, SCENE_LOAD, counts.models_done, Some(counts.models));
        return;
    }
    finish_progress(world, SCENE_LOAD);
}
