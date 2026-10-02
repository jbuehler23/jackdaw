//! Making a placed model into a LOD group from the level files beside it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use bevy::prelude::*;
use bevy::reflect::TypePath;
use jackdaw_api::prelude::*;
use jackdaw_scene_types::{GltfSource, LodGroup, LodLevel};

use crate::selection::Selection;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<EntityLodGroupOp>();
    ctx.register_operator::<SceneLodImplyLevelsOp>();
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

/// Make a placed model a LOD group, with the level files beside its glTF as
/// the group's further levels.
#[operator(
    id = "entity.lod_group",
    label = "Make LOD Group",
    description = "Make a placed model a LOD group, with the <name>_LOD1, _LOD2 ... files \
                   beside it as the further levels.",
    allows_undo = true,
    params(
        entity(Entity, doc = "The placed model. Defaults to the selection."),
        screen_heights(
            String,
            doc = "Comma-separated share of the screen's height each level needs, most \
                   detailed first; below the last one the model is not drawn. Defaults \
                   to Unity's."
        ),
        size(
            f64,
            default = 0.0,
            doc = "The model's size along its largest side; 0 measures it."
        ),
        fade(
            f64,
            default = 0.0,
            doc = "Share of each switch distance that cross-fades; 0 snaps."
        ),
    )
)]
pub(crate) fn entity_lod_group(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    project: Option<Res<crate::project::ProjectRoot>>,
    sources: Query<&GltfSource>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(model) = params
        .as_entity("entity")
        .or_else(|| selection.entities.last().copied())
    else {
        warn!("entity.lod_group: no model selected");
        return OperatorResult::Cancelled;
    };
    let Ok(source) = sources.get(model) else {
        warn!("entity.lod_group: {model} is not a placed model");
        return OperatorResult::Cancelled;
    };
    let files = project
        .map(|project| level_files(&project.assets_dir(), &source.path))
        .unwrap_or_default();
    let heights = match params.as_str("screen_heights") {
        Some(listed) => match listed
            .split(',')
            .map(|height| height.trim().parse::<f32>())
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(heights) => heights,
            Err(_) => {
                warn!("entity.lod_group: `{listed}` is not a list of numbers");
                return OperatorResult::Cancelled;
            }
        },
        None => default_screen_heights(files.len() + 1),
    };
    let group = LodGroup {
        levels: heights
            .into_iter()
            .map(|screen_height| LodLevel { screen_height })
            .collect(),
        size: params.as_float("size").unwrap_or(0.0) as f32,
        fade: params.as_float("fade").unwrap_or(0.0) as f32,
    };
    commands.queue(move |world: &mut World| make_lod_group(world, model, group));
    OperatorResult::Finished
}

fn make_lod_group(world: &mut World, model: Entity, group: LodGroup) {
    let Ok(mut entity) = world.get_entity_mut(model) else {
        return;
    };
    entity.insert(group.clone());
    crate::commands::sync_component_to_bsn_doc(world, model, &group);
    crate::selection::select_only(world, model);
}

/// Turn LOD groups whose levels are a model and its `<name>_LOD1`, `<name>_LOD2`
/// ... files into groups that name the model themselves, without level
/// children. A group whose levels carry anything more is left as it is.
#[operator(
    id = "scene.lod.imply_levels",
    label = "Imply LOD Levels",
    description = "Turn LOD groups whose levels are a model and its <name>_LOD1, _LOD2 ... \
                   files into groups that name the model themselves, without level children.",
    allows_undo = true
)]
pub(crate) fn scene_lod_imply_levels(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(imply_levels);
    OperatorResult::Finished
}

fn imply_levels(world: &mut World) {
    let mut groups = world.query_filtered::<(Entity, &LodGroup), Without<GltfSource>>();
    let groups: Vec<(Entity, usize)> = groups
        .iter(world)
        .map(|(entity, group)| (entity, group.levels.len()))
        .collect();
    let convertible: Vec<(Entity, GltfSource, Vec<Entity>)> = groups
        .into_iter()
        .filter_map(|(group, count)| {
            implied_form(world, group, count).map(|(source, levels)| (group, source, levels))
        })
        .collect();
    for (group, source, levels) in convertible {
        for level in levels {
            crate::commands::despawn_scene_entity(world, level);
        }
        world.entity_mut(group).insert(source.clone());
        crate::commands::sync_component_to_bsn_doc(world, group, &source);
    }
}

/// The model a group would name itself and the level children it would drop,
/// when every level child is that model or its `_LOD<n>` file and carries
/// nothing else.
fn implied_form(world: &World, group: Entity, count: usize) -> Option<(GltfSource, Vec<Entity>)> {
    let ast = world.resource::<jackdaw_bsn::SceneBsnAst>();
    let node = ast.ast_for(group)?;
    let children = ast.get_children_ast(node);
    if count == 0 || children.len() != count {
        return None;
    }
    let levels: Vec<Entity> = world.get::<Children>(group)?.iter().take(count).collect();
    let first = world.get::<GltfSource>(*levels.first()?)?.clone();
    let allowed = [
        Name::type_path(),
        Transform::type_path(),
        GltfSource::type_path(),
        jackdaw_scene_types::SCENE_NODE_ID_TYPE_PATH,
    ];
    for (index, level) in levels.iter().enumerate() {
        let level_node = ast.ast_for(*level)?;
        let source = world.get::<GltfSource>(*level)?;
        let plain = ast
            .component_type_paths(level_node)
            .iter()
            .all(|path| allowed.contains(&path.as_str()));
        if !plain
            || !ast.get_children_ast(level_node).is_empty()
            || world.get::<Transform>(*level) != Some(&Transform::IDENTITY)
            || source.scene_index != first.scene_index
            || source.path != LodGroup::implied_level_path(&first.path, index)
        {
            return None;
        }
    }
    Some((first, levels))
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
