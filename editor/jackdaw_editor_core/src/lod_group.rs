//! Making a placed model into a LOD group from the level files beside it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_scene_types::{GltfSource, LodGroup, LodLevel};

use crate::selection::Selection;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<EntityLodGroupOp>()
        .register_operator::<ViewForceLodOp>()
        .register_operator::<ViewCycleForcedLodOp>();
}

/// The most levels the forced-LOD cycle steps through before going back to
/// automatic.
const FORCED_LOD_LEVELS: usize = 4;

/// Draw every LOD group at one level whatever the distance, or let the
/// distance choose again. A view setting: nothing is saved.
#[operator(
    id = "view.force_lod",
    label = "Force LOD",
    description = "Draw every LOD group at one level whatever the distance; a negative level \
                   lets the distance choose again.",
    allows_undo = false,
    params(level(i64, default = -1, doc = "The level to draw, 0 the most detailed.")),
)]
pub(crate) fn view_force_lod(
    params: In<OperatorParameters>,
    mut forced: ResMut<jackdaw_runtime::ForcedLod>,
) -> OperatorResult {
    let level = params.as_int("level").unwrap_or(-1);
    let wanted = jackdaw_runtime::ForcedLod(usize::try_from(level).ok());
    if *forced != wanted {
        *forced = wanted;
    }
    OperatorResult::Finished
}

/// Step the forced LOD level: automatic, then each level from the most
/// detailed, then automatic again.
#[operator(
    id = "view.cycle_forced_lod",
    label = "Cycle Forced LOD",
    description = "Step through drawing every LOD group at one level, then back to automatic.",
    allows_undo = false
)]
pub(crate) fn view_cycle_forced_lod(
    _: In<OperatorParameters>,
    mut forced: ResMut<jackdaw_runtime::ForcedLod>,
) -> OperatorResult {
    forced.0 = match forced.0 {
        None => Some(0),
        Some(level) if level + 1 < FORCED_LOD_LEVELS => Some(level + 1),
        Some(_) => None,
    };
    OperatorResult::Finished
}

/// What placing live LOD levels may cost a frame while a scene opens. The load
/// overlay is up and nothing is being edited.
const OPENING_BUDGET: Duration = Duration::from_millis(150);

/// What placing live LOD levels may cost a frame while the scene is being
/// edited.
const EDITING_BUDGET: Duration = Duration::from_millis(8);

/// What the footer calls refining detail.
const REFINING_PHASE: &str = "refining detail";

pub(crate) fn plugin(app: &mut App) {
    app.insert_resource(jackdaw_runtime::LiveLevelSettings {
        budget: EDITING_BUDGET,
        opening_budget: OPENING_BUDGET,
        stand_ins: true,
    })
    .add_systems(
        Update,
        name_refinement.run_if(resource_changed::<jackdaw_runtime::LiveLevelProgress>),
    );
}

/// Name the refinement in the footer while levels the cameras want are still
/// coming in.
fn name_refinement(
    progress: Res<jackdaw_runtime::LiveLevelProgress>,
    mut phase: Option<ResMut<crate::status_bar::EditorPhase>>,
) {
    let Some(phase) = phase.as_mut() else {
        return;
    };
    if progress.groups == 0 || progress.is_refined() {
        phase.finish(REFINING_PHASE);
    } else {
        phase.begin(
            REFINING_PHASE,
            format!(
                "Refining detail {} / {}",
                compact_count(progress.refined),
                compact_count(progress.groups)
            ),
        );
    }
}

/// A count as the footer words it: thousands with one decimal.
fn compact_count(count: usize) -> String {
    if count < 1000 {
        count.to_string()
    } else {
        format!("{:.1}k", count as f32 / 1000.0)
    }
}

/// The level files beside a model: `<stem>_LOD1`, `<stem>_LOD2` and on, with
/// the model's own extension, up to the first one missing.
pub fn level_files(assets: &Path, model: &str) -> Vec<String> {
    let path = Path::new(model);
    let (Some(stem), Some(extension)) = (path.file_stem(), path.extension()) else {
        return Vec::new();
    };
    let folder = path.parent().unwrap_or(Path::new(""));
    let mut files = Vec::new();
    for level in 1.. {
        let file: PathBuf = folder.join(format!(
            "{}_LOD{level}.{}",
            stem.to_string_lossy(),
            extension.to_string_lossy()
        ));
        if !assets.join(&file).is_file() {
            break;
        }
        files.push(file.to_string_lossy().replace('\\', "/"));
    }
    files
}

/// Screen heights for `count` levels, Unity's LOD Group defaults for three
/// levels spread over however many there are, ending in a cull at 1%.
pub fn default_screen_heights(count: usize) -> Vec<f32> {
    (0..count)
        .map(|level| {
            if level + 1 == count {
                0.01
            } else {
                0.6 * 0.5f32.powi(level as i32)
            }
        })
        .collect()
}

/// Give a placed model levels of detail, or make an entity's children the
/// levels of a LOD group.
///
/// A placed model draws the levels its import settings list; one without
/// them has them found, from the `_LOD` files beside it or the `_LOD` nodes
/// inside it, and its file card opens to show them. Any other entity becomes
/// a hand-built group, each child one level.
#[operator(
    id = "entity.lod_group",
    label = "Make LOD Group",
    description = "Give a placed model levels of detail in its import settings, found in the \
                   _LOD files beside it or the _LOD nodes inside it; make any other entity a \
                   LOD group whose children are its levels.",
    allows_undo = true,
    params(entity(Entity, doc = "The placed model or group. Defaults to the selection."))
)]
pub(crate) fn entity_lod_group(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    sources: Query<&GltfSource>,
    children: Query<&Children>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(target) = params
        .as_entity("entity")
        .or_else(|| selection.entities.last().copied())
    else {
        warn!("entity.lod_group: nothing selected");
        return OperatorResult::Cancelled;
    };
    if let Ok(source) = sources.get(target) {
        let model = jackdaw_scene_types::model_parts::source_path(source);
        commands.queue(move |world: &mut World| open_model_levels(world, &model));
        return OperatorResult::Finished;
    }
    let count = children.get(target).map_or(0, RelationshipTarget::len);
    if count == 0 {
        warn!("entity.lod_group: {target} is neither a placed model nor a parent of levels");
        return OperatorResult::Cancelled;
    }
    let group = LodGroup {
        levels: default_screen_heights(count)
            .into_iter()
            .map(|screen_height| LodLevel { screen_height })
            .collect(),
        size: 0.0,
        fade: 0.0,
    };
    commands.queue(move |world: &mut World| make_lod_group(world, target, group));
    OperatorResult::Finished
}

/// Find a model's levels when its settings have none, and show its file card.
fn open_model_levels(world: &mut World, model: &str) {
    let has_levels = world
        .resource::<jackdaw_scene_types::model_import::ModelLodIndex>()
        .get(model)
        .is_some();
    if !has_levels {
        crate::model_lod::import_model_levels(world, model);
    }
    let Some(assets) = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(crate::project::ProjectRoot::assets_dir)
    else {
        return;
    };
    crate::inspector::file_card::show_file(world, &assets.join(model));
}

fn make_lod_group(world: &mut World, model: Entity, group: LodGroup) {
    let Ok(mut entity) = world.get_entity_mut(model) else {
        return;
    };
    entity.insert(group.clone());
    crate::commands::sync_component_to_bsn_doc(world, model, &group);
    crate::selection::select_only(world, model);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_level_files_beside_a_model_are_found_in_order_until_one_is_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let folder = dir.path().join("models");
        std::fs::create_dir_all(&folder).expect("folder");
        for file in [
            "House.gltf",
            "House_LOD1.gltf",
            "House_LOD2.gltf",
            "House_LOD4.gltf",
        ] {
            std::fs::write(folder.join(file), "{}").expect("file");
        }

        assert_eq!(
            level_files(dir.path(), "models/House.gltf"),
            ["models/House_LOD1.gltf", "models/House_LOD2.gltf"]
        );
        assert!(level_files(dir.path(), "models/Shed.gltf").is_empty());
    }

    #[test]
    fn the_default_heights_halve_down_the_levels_and_cull_at_one_percent() {
        assert_eq!(default_screen_heights(1), [0.01]);
        assert_eq!(default_screen_heights(3), [0.6, 0.3, 0.01]);
    }
}
