//! A model's levels of detail in its import settings: finding them when the
//! model is imported, holding edits in memory as undo entries, and writing the
//! model's `.meta` on Save or Apply, as Unity's model importer does.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_commands::CommandHistory;
use jackdaw_scene_types::LodFade;
use jackdaw_scene_types::model_import::{
    LevelShow, LodImportSource, ModelLod, ModelLodIndex, ModelLodLevel, ModelSettings, meta_path,
    model_meta, read_model_meta,
};

use crate::commands::EditorCommand;
use crate::lod_group::{default_screen_heights, level_files};

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ModelLodImportOp>()
        .register_operator::<ModelLodApplyOp>();
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<UnsavedModelSettings>();
}

/// Models whose import settings were edited and not yet written, by asset path.
#[derive(Resource, Default, Debug)]
pub struct UnsavedModelSettings(pub BTreeSet<String>);

/// The levels an import finds for the model at `model`, a path under `assets`:
/// nodes named with a `_LOD<n>` suffix inside it, or else `<name>_LOD1`,
/// `<name>_LOD2` ... files beside it. `None` when it has neither.
pub fn find_levels(assets: &Path, model: &str) -> Result<Option<ModelLod>, String> {
    let json = std::fs::read(assets.join(model))
        .ok()
        .and_then(|bytes| gltf_json(&bytes));
    if let Some(json) = &json
        && let Some(levels) = suffixed_node_levels(json)?
    {
        let size = json_bounds(json, levels.first().map(Vec::as_slice)).unwrap_or(0.0);
        let heights = default_screen_heights(levels.len());
        return Ok(Some(ModelLod {
            version: ModelLod::VERSION,
            source: LodImportSource::NodeSuffixes,
            size,
            fade: LodFade::Snap,
            levels: levels
                .into_iter()
                .zip(heights)
                .map(|(nodes, screen_height)| ModelLodLevel {
                    show: LevelShow::Nodes(nodes),
                    screen_height,
                })
                .collect(),
        }));
    }
    let files = level_files(assets, model);
    if files.is_empty() {
        return Ok(None);
    }
    let heights = default_screen_heights(files.len() + 1);
    let shows = std::iter::once(LevelShow::Model).chain(files.iter().map(|file| {
        LevelShow::File(
            Path::new(file)
                .file_name()
                .map_or_else(|| file.clone(), |name| name.to_string_lossy().into_owned()),
        )
    }));
    Ok(Some(ModelLod {
        version: ModelLod::VERSION,
        source: LodImportSource::SiblingFiles,
        size: json
            .as_ref()
            .and_then(|json| json_bounds(json, None))
            .unwrap_or(0.0),
        fade: LodFade::Snap,
        levels: shows
            .zip(heights)
            .map(|(show, screen_height)| ModelLodLevel {
                show,
                screen_height,
            })
            .collect(),
    }))
}

/// The largest side of the model at `file`, measured from the bounds its
/// file declares, or 0 when it cannot be read.
pub fn measured_size(file: &Path) -> f32 {
    std::fs::read(file)
        .ok()
        .and_then(|bytes| gltf_json(&bytes))
        .and_then(|json| json_bounds(&json, None))
        .unwrap_or(0.0)
}

/// `found` with the screen heights and fade of `current` kept, when both have
/// the same number of levels, as a reimport keeps them. The second value says
/// whether the heights were spread afresh because the count changed.
pub fn keep_user_settings(found: ModelLod, current: Option<&ModelLod>) -> (ModelLod, bool) {
    let Some(current) = current else {
        return (found, false);
    };
    if current.levels.len() != found.levels.len() {
        return (found, true);
    }
    let mut kept = found;
    for (level, held) in kept.levels.iter_mut().zip(&current.levels) {
        level.screen_height = held.screen_height;
    }
    kept.fade = current.fade;
    (kept, false)
}

/// The JSON of a `.gltf` file, or of the JSON chunk of a `.glb`.
fn gltf_json(bytes: &[u8]) -> Option<serde_json::Value> {
    if bytes.starts_with(b"glTF") {
        let length = u32::from_le_bytes(bytes.get(12..16)?.try_into().ok()?) as usize;
        let chunk = bytes.get(20..20 + length)?;
        return serde_json::from_slice(chunk).ok();
    }
    serde_json::from_slice(bytes).ok()
}

fn nodes(json: &serde_json::Value) -> &[serde_json::Value] {
    json.get("nodes")
        .and_then(serde_json::Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn node_name(node: &serde_json::Value) -> Option<&str> {
    node.get("name").and_then(serde_json::Value::as_str)
}

fn child_indices(node: &serde_json::Value) -> Vec<usize> {
    node.get("children")
        .and_then(serde_json::Value::as_array)
        .map(|children| {
            children
                .iter()
                .filter_map(|child| child.as_u64().map(|child| child as usize))
                .collect()
        })
        .unwrap_or_default()
}

/// The nodes no other node holds as a child: the tops of the file's trees.
fn root_indices(json: &serde_json::Value) -> Vec<usize> {
    let all = nodes(json);
    let held: BTreeSet<usize> = all.iter().flat_map(child_indices).collect();
    (0..all.len())
        .filter(|index| !held.contains(index))
        .collect()
}

/// The level a node name's `_LOD<n>` suffix names, in any case.
fn level_suffix(name: &str) -> Option<usize> {
    let at = name.to_ascii_lowercase().rfind("_lod")?;
    let digits = &name[at + 4..];
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The nodes of each level, for a file whose nodes carry `_LOD<n>` suffixes:
/// a suffixed node with everything under it goes on its level, and a mesh
/// node under no suffixed one goes on every level. `None` when no node
/// carries a suffix.
fn suffixed_node_levels(json: &serde_json::Value) -> Result<Option<Vec<Vec<String>>>, String> {
    let all = nodes(json);
    let mut levels: Vec<Vec<String>> = Vec::new();
    let mut everywhere: Vec<String> = Vec::new();
    let mut pending = root_indices(json);
    while let Some(index) = pending.pop() {
        let Some(node) = all.get(index) else {
            continue;
        };
        let name = node_name(node);
        if let Some(level) = name.and_then(level_suffix) {
            if levels.len() <= level {
                levels.resize(level + 1, Vec::new());
            }
            levels[level].push(name.unwrap_or_default().to_string());
            continue;
        }
        if node.get("mesh").is_some()
            && let Some(name) = name
        {
            everywhere.push(name.to_string());
        }
        pending.extend(child_indices(node));
    }
    if levels.is_empty() {
        return Ok(None);
    }
    if let Some(missing) = levels.iter().position(Vec::is_empty) {
        return Err(format!("no node is named for LOD{missing}"));
    }
    let named: Vec<&str> = levels
        .iter()
        .flatten()
        .chain(&everywhere)
        .map(String::as_str)
        .collect();
    for name in &named {
        if all
            .iter()
            .filter_map(node_name)
            .filter(|other| other == name)
            .count()
            > 1
        {
            return Err(format!("more than one node is named {name}"));
        }
    }
    for level in &mut levels {
        level.extend(everywhere.iter().cloned());
        level.sort();
    }
    Ok(Some(levels))
}

fn node_transform(node: &serde_json::Value) -> Mat4 {
    let floats = |key: &str| -> Option<Vec<f32>> {
        node.get(key)?
            .as_array()?
            .iter()
            .map(|value| value.as_f64().map(|value| value as f32))
            .collect()
    };
    if let Some(matrix) = floats("matrix").filter(|matrix| matrix.len() == 16) {
        return Mat4::from_cols_slice(&matrix);
    }
    let translation = floats("translation")
        .filter(|value| value.len() == 3)
        .map_or(Vec3::ZERO, |value| Vec3::from_slice(&value));
    let rotation = floats("rotation")
        .filter(|value| value.len() == 4)
        .map_or(Quat::IDENTITY, |value| Quat::from_slice(&value));
    let scale = floats("scale")
        .filter(|value| value.len() == 3)
        .map_or(Vec3::ONE, |value| Vec3::from_slice(&value));
    Mat4::from_scale_rotation_translation(scale, rotation, translation)
}

/// The bounds a mesh's positions declare, from their accessors' `min` and
/// `max`, which glTF requires of every position accessor.
fn mesh_bounds(json: &serde_json::Value, mesh: usize) -> Option<(Vec3, Vec3)> {
    let accessors = json.get("accessors")?.as_array()?;
    let primitives = json
        .get("meshes")?
        .get(mesh)?
        .get("primitives")?
        .as_array()?;
    let corner = |accessor: &serde_json::Value, key: &str| -> Option<Vec3> {
        let values = accessor.get(key)?.as_array()?;
        Some(Vec3::new(
            values.first()?.as_f64()? as f32,
            values.get(1)?.as_f64()? as f32,
            values.get(2)?.as_f64()? as f32,
        ))
    };
    let mut bounds: Option<(Vec3, Vec3)> = None;
    for primitive in primitives {
        let Some(accessor) = primitive
            .get("attributes")
            .and_then(|attributes| attributes.get("POSITION"))
            .and_then(serde_json::Value::as_u64)
            .and_then(|index| accessors.get(index as usize))
        else {
            continue;
        };
        let (Some(low), Some(high)) = (corner(accessor, "min"), corner(accessor, "max")) else {
            continue;
        };
        bounds = Some(match bounds {
            Some((lo, hi)) => (lo.min(low), hi.max(high)),
            None => (low, high),
        });
    }
    bounds
}

/// The largest side of the bounds of the file's meshes, or of only those under
/// the nodes named in `only`, through the node transforms above them.
fn json_bounds(json: &serde_json::Value, only: Option<&[String]>) -> Option<f32> {
    let all = nodes(json);
    let mut low = Vec3::splat(f32::INFINITY);
    let mut high = Vec3::splat(f32::NEG_INFINITY);
    let mut pending: Vec<(usize, Mat4, bool)> = root_indices(json)
        .into_iter()
        .map(|index| (index, Mat4::IDENTITY, only.is_none()))
        .collect();
    while let Some((index, parent, above)) = pending.pop() {
        let Some(node) = all.get(index) else {
            continue;
        };
        let world = parent * node_transform(node);
        let kept = above
            || only.is_some_and(|only| {
                node_name(node).is_some_and(|name| only.iter().any(|kept| kept == name))
            });
        pending.extend(
            child_indices(node)
                .into_iter()
                .map(|child| (child, world, kept)),
        );
        if !kept {
            continue;
        }
        let Some((lo, hi)) = node
            .get("mesh")
            .and_then(serde_json::Value::as_u64)
            .and_then(|mesh| mesh_bounds(json, mesh as usize))
        else {
            continue;
        };
        for corner in 0..8 {
            let point = Vec3::new(
                if corner & 1 == 0 { lo.x } else { hi.x },
                if corner & 2 == 0 { lo.y } else { hi.y },
                if corner & 4 == 0 { lo.z } else { hi.z },
            );
            let placed = world.transform_point3(point);
            low = low.min(placed);
            high = high.max(placed);
        }
    }
    low.cmple(high).all().then(|| (high - low).max_element())
}

/// The settings held in the meta beside the model at `file`: its own when the
/// meta names jackdaw's model loader, the glTF settings of a meta written for
/// Bevy's glTF loader, or the settings every drawn model loads with.
fn settings_on_disk(file: &Path) -> (ModelSettings, bool) {
    let Ok(bytes) = std::fs::read(meta_path(file)) else {
        return (ModelSettings::drawn(None), false);
    };
    if let Some(settings) = read_model_meta(&bytes) {
        return (settings, true);
    }
    let gltf = bevy::asset::meta::AssetMeta::<bevy::gltf::GltfLoader, ()>::deserialize(&bytes)
        .ok()
        .and_then(|meta| match meta.asset {
            bevy::asset::meta::AssetAction::Load { settings, .. } => Some(settings),
            _ => None,
        });
    match gltf {
        Some(gltf) => (ModelSettings { gltf, lod: None }, true),
        None => (ModelSettings::drawn(None), true),
    }
}

/// The levels the meta beside the model at `file` holds.
pub fn levels_on_disk(file: &Path) -> Option<ModelLod> {
    settings_on_disk(file).0.lod
}

/// Write `lod` into the meta beside the model at `file`, keeping its other
/// settings, and report whether the file changed. A model with no levels and
/// no meta is left without one.
pub fn write_levels(file: &Path, lod: Option<&ModelLod>) -> std::io::Result<bool> {
    let (mut settings, had_meta) = settings_on_disk(file);
    if lod.is_none() && !had_meta {
        return Ok(false);
    }
    settings.lod = lod.cloned();
    let bytes = model_meta(settings);
    let meta = meta_path(file);
    if std::fs::read(&meta).is_ok_and(|held| held == bytes) {
        return Ok(false);
    }
    crate::scene_io::save::write_atomic(&meta, &bytes)?;
    Ok(true)
}

/// The model's asset path, from a path given as either an asset path or a
/// file under the project's assets.
fn asset_path_of(assets: &Path, given: &str) -> String {
    let path = Path::new(given);
    let relative = path.strip_prefix(assets).unwrap_or(path);
    relative.to_string_lossy().replace('\\', "/")
}

/// The levels the model at `path` has now: its in-memory settings once read,
/// or what its meta holds.
fn current_levels(world: &World, assets: &Path, path: &str) -> Option<ModelLod> {
    let index = world.resource::<ModelLodIndex>();
    if index.is_known(path) {
        return index.get(path).map(|lod| (**lod).clone());
    }
    levels_on_disk(&assets.join(path))
}

/// Replace a model's levels in memory, as one undo entry.
pub struct SetModelLod {
    pub path: String,
    pub before: Option<ModelLod>,
    pub after: Option<ModelLod>,
    /// Whether the model held unsaved settings before this edit.
    pub was_unsaved: bool,
}

impl SetModelLod {
    fn put(world: &mut World, path: &str, lod: Option<ModelLod>, unsaved: bool) {
        world.resource_mut::<ModelLodIndex>().set(path, lod);
        let mut held = world.resource_mut::<UnsavedModelSettings>();
        if unsaved {
            held.0.insert(path.to_string());
        } else {
            held.0.remove(path);
        }
        crate::inspector::file_card::refresh_file_card(world);
    }
}

impl EditorCommand for SetModelLod {
    fn execute(&mut self, world: &mut World) {
        Self::put(world, &self.path, self.after.clone(), true);
    }

    fn undo(&mut self, world: &mut World) {
        Self::put(world, &self.path, self.before.clone(), self.was_unsaved);
    }

    fn description(&self) -> &str {
        "Import LOD levels"
    }
}

/// Change a model's levels in memory as one undo entry, to be written on Save
/// or Apply.
pub fn edit_model_levels(world: &mut World, path: &str, after: Option<ModelLod>) {
    let Some(assets) = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(crate::project::ProjectRoot::assets_dir)
    else {
        return;
    };
    let before = current_levels(world, &assets, path);
    if before == after {
        return;
    }
    let mut command = SetModelLod {
        path: path.to_string(),
        before,
        after,
        was_unsaved: world.resource::<UnsavedModelSettings>().0.contains(path),
    };
    command.execute(world);
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(command));
}

/// Write the settings of every model edited since the last write, returning
/// the meta files that changed.
pub fn write_unsaved_model_settings(world: &mut World) -> std::io::Result<Vec<PathBuf>> {
    let paths: Vec<String> = world
        .get_resource::<UnsavedModelSettings>()
        .map(|unsaved| unsaved.0.iter().cloned().collect())
        .unwrap_or_default();
    let mut written = Vec::new();
    for path in paths {
        if let Some(file) = write_model_settings(world, &path)? {
            written.push(file);
        }
    }
    Ok(written)
}

/// Write one model's settings as they are in memory, returning its meta file
/// when it changed.
pub fn write_model_settings(world: &mut World, path: &str) -> std::io::Result<Option<PathBuf>> {
    let Some(assets) = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(crate::project::ProjectRoot::assets_dir)
    else {
        return Ok(None);
    };
    let lod = world
        .resource::<ModelLodIndex>()
        .get(path)
        .map(|lod| (**lod).clone());
    let file = assets.join(path);
    let changed = write_levels(&file, lod.as_ref())?;
    world.resource_mut::<UnsavedModelSettings>().0.remove(path);
    if changed {
        world.resource_mut::<ModelLodIndex>().reread(path);
    }
    Ok(changed.then(|| meta_path(&file)))
}

/// Find a model's levels of detail and put them in its import settings: nodes
/// named `_LOD0`, `_LOD1` ... inside it, or files named `<name>_LOD1`,
/// `<name>_LOD2` ... beside it. A reimport keeps the screen heights and fade
/// while the number of levels stays the same.
#[operator(
    id = "model.lod.import",
    label = "Import LOD Levels",
    description = "Find a model's levels of detail, from nodes named _LOD0, _LOD1 ... inside it \
                   or files named <name>_LOD1, _LOD2 ... beside it, and put them in its import \
                   settings. Written to the model's .meta on Save or Apply.",
    allows_undo = false,
    params(path(String, doc = "The model, as a path under the project's assets."))
)]
pub(crate) fn model_lod_import(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(path) = params.as_str("path").map(str::to_string) else {
        warn!("model.lod.import: no model given");
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| import_model_levels(world, &path));
    OperatorResult::Finished
}

pub(crate) fn import_model_levels(world: &mut World, given: &str) {
    let Some(assets) = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(crate::project::ProjectRoot::assets_dir)
    else {
        return;
    };
    let path = asset_path_of(&assets, given);
    let found = match find_levels(&assets, &path) {
        Ok(Some(found)) => found,
        Ok(None) => {
            crate::status_bar::notify_error(
                world,
                format!("{path} has no _LOD nodes and no _LOD files beside it"),
            );
            return;
        }
        Err(reason) => {
            crate::status_bar::notify_error(world, format!("{path}: {reason}"));
            return;
        }
    };
    let current = current_levels(world, &assets, &path);
    let (lod, respread) = keep_user_settings(found, current.as_ref());
    if respread {
        crate::status_bar::notify_warn(
            world,
            format!(
                "{path} now has {} levels; their screen heights were spread again",
                lod.levels.len()
            ),
        );
    }
    edit_model_levels(world, &path, Some(lod));
}

/// Write a model's import settings to its `.meta` now, without saving the
/// scene.
#[operator(
    id = "model.lod.apply",
    label = "Apply Import Settings",
    description = "Write a model's edited import settings to its .meta now.",
    allows_undo = false,
    params(path(String, doc = "The model, as a path under the project's assets."))
)]
pub(crate) fn model_lod_apply(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(given) = params.as_str("path").map(str::to_string) else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        let Some(assets) = world
            .get_resource::<crate::project::ProjectRoot>()
            .map(crate::project::ProjectRoot::assets_dir)
        else {
            return;
        };
        let path = asset_path_of(&assets, &given);
        if let Err(err) = write_model_settings(world, &path) {
            crate::status_bar::notify_error(world, format!("{path} not written: {err}"));
        }
        crate::inspector::file_card::refresh_file_card(world);
    });
    OperatorResult::Finished
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_level_suffix_reads_in_any_case() {
        assert_eq!(level_suffix("Tree_LOD0"), Some(0));
        assert_eq!(level_suffix("tree_lod12"), Some(12));
        assert_eq!(level_suffix("Tree_LOD"), None);
        assert_eq!(level_suffix("Tree_LODx"), None);
        assert_eq!(level_suffix("Tree"), None);
    }

    #[test]
    fn suffixed_nodes_make_levels_and_unsuffixed_meshes_go_on_every_level() {
        let json = serde_json::json!({
            "nodes": [
                {"name": "Tree", "children": [1, 2, 3]},
                {"name": "Tree_LOD0", "mesh": 0},
                {"name": "Tree_LOD1", "mesh": 0},
                {"name": "Stump", "mesh": 0},
            ]
        });
        let levels = suffixed_node_levels(&json).unwrap().unwrap();
        assert_eq!(
            levels,
            [vec!["Stump", "Tree_LOD0"], vec!["Stump", "Tree_LOD1"]]
        );
    }

    #[test]
    fn node_levels_with_repeated_names_are_refused() {
        let json = serde_json::json!({
            "nodes": [
                {"name": "Tree_LOD0", "mesh": 0},
                {"name": "Tree_LOD1", "mesh": 0},
                {"name": "Tree_LOD1", "mesh": 0},
            ]
        });
        assert!(suffixed_node_levels(&json).is_err());
    }

    #[test]
    fn a_reimport_keeps_heights_and_fade_while_the_level_count_holds() {
        let held = ModelLod {
            version: ModelLod::VERSION,
            source: LodImportSource::SiblingFiles,
            size: 3.0,
            fade: LodFade::CrossFade { width: 0.2 },
            levels: vec![
                ModelLodLevel {
                    show: LevelShow::Model,
                    screen_height: 0.4,
                },
                ModelLodLevel {
                    show: LevelShow::File("a_LOD1.gltf".into()),
                    screen_height: 0.05,
                },
            ],
        };
        let mut found = held.clone();
        found.fade = LodFade::Snap;
        for (level, height) in found.levels.iter_mut().zip([0.6, 0.01]) {
            level.screen_height = height;
        }

        let (kept, respread) = keep_user_settings(found.clone(), Some(&held));
        assert!(!respread);
        assert_eq!(kept.levels[0].screen_height, 0.4);
        assert_eq!(kept.fade, held.fade);

        found.levels.pop();
        let (_, respread) = keep_user_settings(found, Some(&held));
        assert!(respread);
    }
}
