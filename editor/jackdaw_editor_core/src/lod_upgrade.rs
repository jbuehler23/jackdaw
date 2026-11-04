//! Upgrading LOD groups written before models carried their levels in their
//! import settings.
//!
//! A group whose children are a model and its lower levels, or a group that
//! names its model itself, becomes a plain placement of the model, which draws
//! the levels its import settings list. The upgrade runs on the parsed
//! documents before anything spawns, as one undo entry, and writes nothing
//! until the scene is saved.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use bevy::ecs::entity::Entity;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::reflect::TypeRegistry;
use jackdaw_bsn::{BsnPatch, BsnValue, SceneBsnAst};
use jackdaw_scene_types::model_import::{LevelShow, LodImportSource, ModelLod, ModelLodLevel};
use jackdaw_scene_types::{LodFade, LodOverride};

use crate::prefab::canonical_path::canonical_prefab_path;
use crate::prefab::resolver_bsn::{
    ISA_TYPE, PREFAB_ENTITY_ID_TYPE, read_isa_deleted, read_isa_source, read_prefab_entity_id,
};

const LOD_GROUP: &str = "jackdaw_scene_types::types::LodGroup";
const GLTF_SOURCE: &str = "jackdaw_scene_types::types::GltfSource";
const TRANSFORM: &str = "bevy_transform::components::transform::Transform";
const VISIBILITY: &str = "bevy_camera::visibility::Visibility";

/// One document the upgrade reads: the open scene, or a prefab it instances.
pub struct UpgradeDocument {
    /// The prefab file, or `None` for the open scene.
    pub prefab: Option<PathBuf>,
    pub ast: SceneBsnAst,
}

/// What an upgrade did.
#[derive(Default, Debug)]
pub struct UpgradeOutcome {
    /// Groups now drawn by their model's settings.
    pub converted: usize,
    /// Groups left as hand-built groups, by name, with why.
    pub kept: Vec<String>,
    /// Settings made for models that had none.
    pub created: BTreeMap<String, ModelLod>,
    /// Indices of the documents the upgrade changed.
    pub changed: BTreeSet<usize>,
}

/// A group the upgrade may convert.
struct Candidate {
    document: usize,
    node: Entity,
    model: String,
    shows: Vec<LevelShow>,
    heights: Vec<f32>,
    fade: f32,
    size: f32,
    /// The level children to drop; empty for a group naming its model itself.
    levels: Vec<Entity>,
    /// The model's `GltfSource` patch, copied onto a group that drops its
    /// level children.
    source: Option<BsnPatch>,
    group_id: Option<u32>,
    level_ids: Vec<u32>,
    blocked: Option<String>,
}

/// An instance listing that overrides a converted group's `LodGroup`.
struct OverrideListing {
    document: usize,
    node: Entity,
    candidate: usize,
}

/// Upgrade every LOD group in `documents` that can be drawn from its model's
/// settings. `existing` answers the settings a model already has, and
/// `measure` the size of a model that gets new ones.
pub fn upgrade_lod_groups(
    documents: &mut [UpgradeDocument],
    existing: &dyn Fn(&str) -> Option<ModelLod>,
    measure: &dyn Fn(&str) -> f32,
    registry: &TypeRegistry,
) -> UpgradeOutcome {
    let mut candidates = Vec::new();
    for (index, document) in documents.iter().enumerate() {
        let nested = !document.ast.entities_with_component(ISA_TYPE).is_empty();
        for node in document.ast.entities_with_component(LOD_GROUP) {
            if let Some(mut candidate) = candidate_at(&document.ast, index, node) {
                if nested && document.prefab.is_some() {
                    candidate.blocked = Some("its prefab instances another prefab".into());
                }
                candidates.push(candidate);
            }
        }
    }
    let listings = check_instances(documents, &mut candidates);
    let settings = settle_settings(&mut candidates, existing, measure);

    let mut outcome = UpgradeOutcome::default();
    for candidate in &candidates {
        if let Some(reason) = &candidate.blocked {
            let name = documents[candidate.document]
                .ast
                .get_name(candidate.node)
                .unwrap_or("a group")
                .to_string();
            outcome.kept.push(format!("{name}: {reason}"));
        }
    }
    let mut retired: HashMap<PathBuf, BTreeSet<u32>> = HashMap::new();
    for candidate in candidates
        .iter()
        .filter(|candidate| candidate.blocked.is_none())
    {
        let Some(lod) = settings.get(&candidate.model) else {
            continue;
        };
        let ast = &mut documents[candidate.document].ast;
        convert(ast, candidate, lod, registry);
        outcome.converted += 1;
        outcome.changed.insert(candidate.document);
        if let Some(prefab) = &documents[candidate.document].prefab {
            retired
                .entry(canonical(prefab))
                .or_default()
                .extend(candidate.level_ids.iter().copied());
        }
    }
    for listing in &listings {
        let candidate = &candidates[listing.candidate];
        if candidate.blocked.is_some() {
            continue;
        }
        let Some(lod) = settings.get(&candidate.model) else {
            continue;
        };
        override_listing(
            &mut documents[listing.document].ast,
            listing.node,
            lod,
            registry,
        );
        outcome.changed.insert(listing.document);
    }
    for (index, document) in documents.iter_mut().enumerate() {
        if drop_retired_listings(&mut document.ast, &retired) {
            outcome.changed.insert(index);
        }
    }
    for candidate in candidates
        .iter()
        .filter(|candidate| candidate.blocked.is_none())
    {
        if existing(&candidate.model).is_none()
            && let Some(lod) = settings.get(&candidate.model)
        {
            outcome.created.insert(candidate.model.clone(), lod.clone());
        }
    }
    outcome
}

fn canonical(path: &Path) -> PathBuf {
    canonical_prefab_path(path).as_path().to_path_buf()
}

/// The group at `node` as the upgrade sees it, when it is one of the forms the
/// upgrade knows: level children that are a model each, or a model named on
/// the group itself.
fn candidate_at(ast: &SceneBsnAst, document: usize, node: Entity) -> Option<Candidate> {
    let (heights, size, fade) = read_lod_group(ast, node)?;
    if heights.is_empty() {
        return None;
    }
    let group_id = read_prefab_entity_id(ast, node);
    if let Some((model, scene_index)) = read_gltf_source(ast, node) {
        let mut candidate = Candidate {
            document,
            node,
            shows: implied_shows(&model, heights.len()),
            model,
            heights,
            fade,
            size,
            levels: Vec::new(),
            source: None,
            group_id,
            level_ids: Vec::new(),
            blocked: None,
        };
        if scene_index != 0 {
            candidate.blocked = Some("its model draws a scene other than the first".into());
        }
        return Some(candidate);
    }
    let children = ast.get_children_ast(node);
    let mut candidate = Candidate {
        document,
        node,
        model: String::new(),
        shows: Vec::new(),
        heights: heights.clone(),
        fade,
        size,
        levels: Vec::new(),
        source: None,
        group_id,
        level_ids: Vec::new(),
        blocked: None,
    };
    if children.len() < heights.len() {
        candidate.blocked = Some("it has fewer children than levels".into());
        return Some(candidate);
    }
    let mut paths = Vec::new();
    for level in children.iter().take(heights.len()) {
        match plain_level(ast, *level) {
            Some((path, 0)) => paths.push(path),
            Some(_) => {
                candidate.blocked = Some("a level draws a scene other than the first".into());
                return Some(candidate);
            }
            None => {
                candidate.blocked =
                    Some("a level carries more than its model and an identity transform".into());
                return Some(candidate);
            }
        }
        candidate.levels.push(*level);
        candidate
            .level_ids
            .extend(read_prefab_entity_id(ast, *level));
    }
    candidate.model = paths[0].clone();
    candidate.source = ast
        .find_patch_by_type_path(children[0], GLTF_SOURCE)
        .and_then(|patch| ast.get_patch(patch))
        .cloned();
    candidate.shows = paths
        .iter()
        .enumerate()
        .map(|(index, path)| match index {
            0 => LevelShow::Model,
            _ => LevelShow::File(relative_file(&candidate.model, path)),
        })
        .collect();
    Some(candidate)
}

/// The levels a group that names its model itself drew: the model, then the
/// `<name>_LOD<n>` files beside it.
fn implied_shows(model: &str, count: usize) -> Vec<LevelShow> {
    (0..count)
        .map(|level| match level {
            0 => LevelShow::Model,
            _ => LevelShow::File(relative_file(model, &implied_level_path(model, level))),
        })
        .collect()
}

fn implied_level_path(model: &str, level: usize) -> String {
    match model.rsplit_once('.') {
        Some((stem, extension)) if !extension.contains('/') => {
            format!("{stem}_LOD{level}.{extension}")
        }
        _ => format!("{model}_LOD{level}"),
    }
}

/// `path` relative to the folder of `model`, both asset paths.
fn relative_file(model: &str, path: &str) -> String {
    let folder = Path::new(model).parent().unwrap_or(Path::new(""));
    pathdiff::diff_paths(path, folder).map_or_else(
        || path.to_string(),
        |relative| relative.to_string_lossy().replace('\\', "/"),
    )
}

fn field<'a>(value: &'a BsnValue, name: &str) -> Option<&'a BsnValue> {
    match value {
        BsnValue::Struct(data) => data
            .fields
            .0
            .iter()
            .find(|field| field.name == name)
            .map(|field| &field.value),
        _ => None,
    }
}

fn number(value: Option<&BsnValue>) -> Option<f32> {
    match value? {
        BsnValue::Float(value) => Some(*value as f32),
        BsnValue::Int(value) => Some(*value as f32),
        _ => None,
    }
}

/// The screen heights, size and fade a `LodGroup` patch holds, with elided
/// fields at their defaults.
fn read_lod_group(ast: &SceneBsnAst, node: Entity) -> Option<(Vec<f32>, f32, f32)> {
    let whole = jackdaw_bsn::get_bsn_field(ast, node, LOD_GROUP, "")?;
    Some(lod_group_values(&whole))
}

fn lod_group_values(whole: &BsnValue) -> (Vec<f32>, f32, f32) {
    let heights = match field(whole, "levels") {
        Some(BsnValue::List(levels)) => levels
            .iter()
            .map(|level| number(field(level, "screen_height")).unwrap_or(0.0))
            .collect(),
        _ => Vec::new(),
    };
    let size = number(field(whole, "size")).unwrap_or(0.0);
    let fade = number(field(whole, "fade")).unwrap_or(0.0);
    (heights, size, fade)
}

fn read_gltf_source(ast: &SceneBsnAst, node: Entity) -> Option<(String, i128)> {
    let whole = jackdaw_bsn::get_bsn_field(ast, node, GLTF_SOURCE, "")?;
    let BsnValue::String(path) = field(&whole, "path")? else {
        return None;
    };
    let scene_index = match field(&whole, "scene_index") {
        Some(BsnValue::Int(index)) => *index,
        _ => 0,
    };
    Some((jackdaw_scene_types::to_asset_path(path, None), scene_index))
}

/// The model a level child draws, when it carries nothing but its model, its
/// ids, a name and an identity transform.
fn plain_level(ast: &SceneBsnAst, node: Entity) -> Option<(String, i128)> {
    if !ast.get_children_ast(node).is_empty() {
        return None;
    }
    let patches = ast.get_patches(node)?;
    for patch in &patches.0 {
        let patch = ast.get_patch(*patch)?;
        let plain = match patch {
            BsnPatch::Name(_) => true,
            BsnPatch::Children(children) => children.is_empty(),
            BsnPatch::Type(path) => {
                path == TRANSFORM || path.starts_with(VISIBILITY) && path.ends_with("Inherited")
            }
            BsnPatch::Struct(data) if data.type_path == TRANSFORM => {
                is_identity(&BsnValue::Struct(data.clone()))
            }
            other => matches!(
                jackdaw_bsn::patch_type_path(other),
                Some(
                    GLTF_SOURCE
                        | jackdaw_scene_types::SCENE_NODE_ID_TYPE_PATH
                        | PREFAB_ENTITY_ID_TYPE
                )
            ),
        };
        if !plain {
            return None;
        }
    }
    read_gltf_source(ast, node)
}

fn is_identity(transform: &BsnValue) -> bool {
    let near = |value: Option<&BsnValue>, key: &str, wanted: f32| {
        number(value.and_then(|value| field(value, key))).unwrap_or(wanted) - wanted
    };
    let close = |offset: f32| offset.abs() < 1e-6;
    let translation = field(transform, "translation");
    let rotation = field(transform, "rotation");
    let scale = field(transform, "scale");
    ["x", "y", "z"]
        .iter()
        .all(|axis| close(near(translation, axis, 0.0)) && close(near(scale, axis, 1.0)))
        && ["x", "y", "z"]
            .iter()
            .all(|axis| close(near(rotation, axis, 0.0)))
        && close(near(rotation, "w", 1.0))
}

/// Check every instance of a prefab whose groups would convert: a group whose
/// levels an instance deletes or changes stays as it is. Returns the
/// instance listings that override a group's `LodGroup`.
fn check_instances(
    documents: &[UpgradeDocument],
    candidates: &mut [Candidate],
) -> Vec<OverrideListing> {
    let mut by_prefab: HashMap<PathBuf, Vec<usize>> = HashMap::new();
    for (index, candidate) in candidates.iter().enumerate() {
        if let Some(prefab) = &documents[candidate.document].prefab {
            by_prefab.entry(canonical(prefab)).or_default().push(index);
        }
    }
    let mut listings = Vec::new();
    for (document_index, document) in documents.iter().enumerate() {
        let ast = &document.ast;
        for instance in ast.entities_with_component(ISA_TYPE) {
            let Some(source) = read_isa_source(ast, instance) else {
                continue;
            };
            let Some(owned) = by_prefab.get(&canonical(&source)) else {
                continue;
            };
            let deleted = read_isa_deleted(ast, instance);
            let listed: Vec<(Entity, u32)> = listing_nodes(ast, instance);
            for &index in owned {
                let candidate = &mut candidates[index];
                let touches =
                    |id: &u32| candidate.level_ids.contains(id) || candidate.group_id == Some(*id);
                if deleted.iter().any(touches) {
                    candidate.blocked = Some("an instance deletes the group or a level".into());
                    continue;
                }
                for (node, id) in &listed {
                    if candidate.level_ids.contains(id) && !is_bare_listing(ast, *node) {
                        candidate.blocked = Some("an instance changes one of its levels".into());
                    } else if candidate.group_id == Some(*id)
                        && ast.find_patch_by_type_path(*node, LOD_GROUP).is_some()
                    {
                        listings.push(OverrideListing {
                            document: document_index,
                            node: *node,
                            candidate: index,
                        });
                    }
                }
            }
        }
    }
    listings
}

/// The nodes under an instance root that list one of its prefab's ids, with
/// the id, not reaching into nested instances.
fn listing_nodes(ast: &SceneBsnAst, instance: Entity) -> Vec<(Entity, u32)> {
    let mut found = Vec::new();
    let mut pending = ast.get_children_ast(instance);
    while let Some(node) = pending.pop() {
        if ast.find_patch_by_type_path(node, ISA_TYPE).is_some() {
            continue;
        }
        if let Some(id) = read_prefab_entity_id(ast, node) {
            found.push((node, id));
        }
        pending.extend(ast.get_children_ast(node));
    }
    found
}

/// Whether a listing names its prefab node and nothing more.
fn is_bare_listing(ast: &SceneBsnAst, node: Entity) -> bool {
    ast.get_children_ast(node).is_empty()
        && ast.get_patches(node).is_some_and(|patches| {
            patches.0.iter().all(|patch| match ast.get_patch(*patch) {
                Some(BsnPatch::Children(children)) => children.is_empty(),
                Some(other) => jackdaw_bsn::patch_type_path(other) == Some(PREFAB_ENTITY_ID_TYPE),
                None => true,
            })
        })
}

/// The settings each model's groups convert to: those it has, when its groups
/// draw the same levels, or new ones with the heights most of its groups use.
fn settle_settings(
    candidates: &mut [Candidate],
    existing: &dyn Fn(&str) -> Option<ModelLod>,
    measure: &dyn Fn(&str) -> f32,
) -> BTreeMap<String, ModelLod> {
    let mut by_model: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, candidate) in candidates.iter().enumerate() {
        if candidate.blocked.is_none() {
            by_model
                .entry(candidate.model.clone())
                .or_default()
                .push(index);
        }
    }
    let mut settled = BTreeMap::new();
    for (model, groups) in by_model {
        let lod = match existing(&model) {
            Some(lod) => lod,
            None => {
                let first = &candidates[groups[0]];
                let heights = majority(
                    groups
                        .iter()
                        .map(|index| candidates[*index].heights.clone()),
                );
                let fade = majority(groups.iter().map(|index| candidates[*index].fade));
                let size = groups
                    .iter()
                    .map(|index| candidates[*index].size)
                    .find(|size| *size > 0.0)
                    .unwrap_or_else(|| measure(&model));
                let implied = implied_shows(&model, first.shows.len()) == first.shows;
                ModelLod {
                    version: ModelLod::VERSION,
                    source: if implied {
                        LodImportSource::SiblingFiles
                    } else {
                        LodImportSource::Authored
                    },
                    size,
                    fade: fade_of(fade),
                    levels: first
                        .shows
                        .iter()
                        .zip(heights)
                        .map(|(show, screen_height)| ModelLodLevel {
                            show: show.clone(),
                            screen_height,
                        })
                        .collect(),
                }
            }
        };
        let shows: Vec<LevelShow> = lod.levels.iter().map(|level| level.show.clone()).collect();
        for index in &groups {
            if candidates[*index].shows != shows {
                candidates[*index].blocked =
                    Some("its levels differ from the ones its model's settings list".into());
            }
        }
        settled.insert(model, lod);
    }
    settled
}

/// The value most of `values` hold, the first seen winning a tie.
fn majority<T: PartialEq + Clone>(values: impl Iterator<Item = T>) -> T {
    let mut counted: Vec<(T, usize)> = Vec::new();
    for value in values {
        match counted.iter_mut().find(|(held, _)| *held == value) {
            Some((_, count)) => *count += 1,
            None => counted.push((value, 1)),
        }
    }
    let best = counted.iter().map(|(_, count)| *count).max().unwrap_or(0);
    counted
        .into_iter()
        .find(|(_, count)| *count == best)
        .map(|(value, _)| value)
        .expect("at least one value")
}

fn fade_of(width: f32) -> LodFade {
    if width > 0.0 {
        LodFade::CrossFade { width }
    } else {
        LodFade::Snap
    }
}

/// The override a group with these heights and fade needs over `lod`, if any.
fn override_for(heights: Option<&[f32]>, fade: Option<f32>, lod: &ModelLod) -> Option<LodOverride> {
    let held: Vec<f32> = lod.levels.iter().map(|level| level.screen_height).collect();
    let screen_heights = heights
        .filter(|heights| *heights != held.as_slice())
        .map(<[f32]>::to_vec);
    let fade = fade.map(fade_of).filter(|fade| *fade != lod.fade);
    (screen_heights.is_some() || fade.is_some()).then_some(LodOverride {
        screen_heights,
        fade,
    })
}

fn add_patch(ast: &mut SceneBsnAst, node: Entity, patch: BsnPatch) {
    let patch = ast.world.spawn(patch).id();
    if let Some(patches) = ast.get_patches_mut(node) {
        patches.0.push(patch);
    }
}

fn override_patch(value: &LodOverride, registry: &TypeRegistry) -> BsnPatch {
    jackdaw_bsn::component_to_bsn_patch(value, registry)
}

fn convert(ast: &mut SceneBsnAst, candidate: &Candidate, lod: &ModelLod, registry: &TypeRegistry) {
    ast.remove_component_patch(candidate.node, LOD_GROUP);
    if let Some(source) = &candidate.source {
        add_patch(ast, candidate.node, source.clone());
    }
    for level in &candidate.levels {
        ast.remove_child_from_ast(candidate.node, *level);
    }
    drop_empty_children(ast, candidate.node);
    if let Some(value) = override_for(Some(&candidate.heights), Some(candidate.fade), lod) {
        add_patch(ast, candidate.node, override_patch(&value, registry));
    }
}

/// Turn an instance's override of a group's `LodGroup` into the `LodOverride`
/// it now means.
fn override_listing(ast: &mut SceneBsnAst, node: Entity, lod: &ModelLod, registry: &TypeRegistry) {
    let Some(whole) = jackdaw_bsn::get_bsn_field(ast, node, LOD_GROUP, "") else {
        return;
    };
    let heights = match field(&whole, "levels") {
        Some(BsnValue::List(_)) => Some(lod_group_values(&whole).0),
        _ => None,
    };
    let fade = number(field(&whole, "fade"));
    ast.remove_component_patch(node, LOD_GROUP);
    if let Some(value) = override_for(heights.as_deref(), fade, lod) {
        add_patch(ast, node, override_patch(&value, registry));
    }
}

/// Drop the listings instances keep of the level nodes the upgrade retired
/// from their prefabs, reporting whether any went.
fn drop_retired_listings(ast: &mut SceneBsnAst, retired: &HashMap<PathBuf, BTreeSet<u32>>) -> bool {
    let mut dropped = false;
    for instance in ast.entities_with_component(ISA_TYPE) {
        let Some(ids) =
            read_isa_source(ast, instance).and_then(|source| retired.get(&canonical(&source)))
        else {
            continue;
        };
        for (node, id) in listing_nodes(ast, instance) {
            if !ids.contains(&id) || !is_bare_listing(ast, node) {
                continue;
            }
            let mut at = node;
            while let Some(parent) = ast.find_ast_parent_of(at) {
                ast.remove_child_from_ast(parent, at);
                drop_empty_children(ast, parent);
                dropped = true;
                if parent == instance || !is_bare_listing(ast, parent) {
                    break;
                }
                at = parent;
            }
        }
    }
    dropped
}

fn drop_empty_children(ast: &mut SceneBsnAst, node: Entity) {
    let Some(patches) = ast.get_patches(node).map(|patches| patches.0.clone()) else {
        return;
    };
    let empty: Vec<Entity> = patches
        .into_iter()
        .filter(|patch| matches!(ast.get_patch(*patch), Some(BsnPatch::Children(children)) if children.is_empty()))
        .collect();
    if empty.is_empty() {
        return;
    }
    if let Some(held) = ast.get_patches_mut(node) {
        held.0.retain(|patch| !empty.contains(patch));
    }
    for patch in empty {
        ast.world.despawn(patch);
    }
}

/// Prefab files an upgrade changed in memory, written with the next save.
#[derive(Resource, Default, Debug)]
pub struct UpgradedPrefabs(pub BTreeSet<PathBuf>);

/// One state of an upgrade's documents: the scene's text, each changed
/// prefab's text, and each model's settings.
#[derive(Clone)]
struct UpgradeState {
    scene: String,
    prefabs: Vec<(PathBuf, String)>,
    settings: Vec<(String, Option<ModelLod>)>,
}

/// The upgrade done when a scene opened, as one undo entry: undo puts the
/// scene and its prefabs back in their old form and drops the settings it
/// made; redo upgrades them again.
pub struct UpgradeLodGroups {
    before: UpgradeState,
    after: UpgradeState,
}

impl UpgradeLodGroups {
    fn put(world: &mut World, state: &UpgradeState, upgraded: bool) {
        for (path, text) in &state.prefabs {
            if let Ok(ast) = jackdaw_bsn::parse_bsn_text(text)
                && let Some(mut cache) = world.get_resource_mut::<crate::prefab::PrefabAstCache>()
            {
                cache.insert(path, ast);
            }
            let mut held = world.resource_mut::<UpgradedPrefabs>();
            if upgraded {
                held.0.insert(path.clone());
            } else {
                held.0.remove(path);
            }
        }
        for (model, lod) in &state.settings {
            world
                .resource_mut::<jackdaw_scene_types::model_import::ModelLodIndex>()
                .set(model, lod.clone());
            let mut unsaved = world.resource_mut::<crate::model_lod::UnsavedModelSettings>();
            if upgraded {
                unsaved.0.insert(model.clone());
            } else {
                unsaved.0.remove(model);
            }
        }
        crate::undo_snapshot::reload_document_text(world, &state.scene);
    }
}

impl crate::commands::EditorCommand for UpgradeLodGroups {
    fn execute(&mut self, world: &mut World) {
        Self::put(world, &self.after, true);
    }

    fn undo(&mut self, world: &mut World) {
        Self::put(world, &self.before, false);
    }

    fn description(&self) -> &str {
        "Upgrade LOD groups"
    }

    fn heap_bytes(&self) -> usize {
        let state = |state: &UpgradeState| {
            state.scene.capacity()
                + state
                    .prefabs
                    .iter()
                    .map(|(_, text)| text.capacity())
                    .sum::<usize>()
        };
        state(&self.before) + state(&self.after)
    }
}

/// An upgrade made while a scene opened, waiting to become an undo entry once
/// the scene is in.
pub struct PendingUpgrade {
    entry: UpgradeLodGroups,
    converted: usize,
    kept: Vec<String>,
}

/// The prefabs `ast` instances, and the ones those instance, as the cache
/// holds them.
fn instanced_prefabs(ast: &SceneBsnAst, cache: &crate::prefab::PrefabAstCache) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    let mut pending: Vec<PathBuf> = ast
        .entities_with_component(ISA_TYPE)
        .into_iter()
        .filter_map(|node| read_isa_source(ast, node))
        .collect();
    while let Some(path) = pending.pop() {
        if found.iter().any(|seen| canonical(seen) == canonical(&path)) {
            continue;
        }
        let Some(prefab) = cache.get(&path) else {
            continue;
        };
        pending.extend(
            prefab
                .entities_with_component(ISA_TYPE)
                .into_iter()
                .filter_map(|node| read_isa_source(prefab, node)),
        );
        found.push(path);
    }
    found
}

/// Upgrade the LOD groups of a scene being opened and of the prefabs it
/// instances, in memory and before anything spawns.
pub fn upgrade_on_open(
    world: &mut World,
    authored: &mut SceneBsnAst,
    assets: &Path,
) -> Option<PendingUpgrade> {
    let prefabs: Vec<(PathBuf, SceneBsnAst)> = world
        .get_resource::<crate::prefab::PrefabAstCache>()
        .map(|cache| {
            instanced_prefabs(authored, cache)
                .into_iter()
                .filter_map(|path| {
                    let ast = cache.get(&path)?;
                    Some((path, crate::prefab::resolver_bsn::clone_scene(ast)))
                })
                .collect()
        })
        .unwrap_or_default();
    let holds_groups = |ast: &SceneBsnAst| !ast.entities_with_component(LOD_GROUP).is_empty();
    if !holds_groups(authored) && !prefabs.iter().any(|(_, ast)| holds_groups(ast)) {
        return None;
    }

    let before_scene = jackdaw_bsn::emit_scene(authored);
    let before_prefabs: Vec<String> = prefabs
        .iter()
        .map(|(_, ast)| jackdaw_bsn::emit_scene(ast))
        .collect();
    let mut documents = vec![UpgradeDocument {
        prefab: None,
        ast: std::mem::take(authored),
    }];
    documents.extend(prefabs.into_iter().map(|(path, ast)| UpgradeDocument {
        prefab: Some(path),
        ast,
    }));

    let outcome = {
        let index = world.resource::<jackdaw_scene_types::model_import::ModelLodIndex>();
        let existing = |model: &str| {
            if index.is_known(model) {
                index.get(model).map(|lod| (**lod).clone())
            } else {
                crate::model_lod::levels_on_disk(&assets.join(model))
            }
        };
        let measure = |model: &str| crate::model_lod::measured_size(&assets.join(model));
        let registry = world.resource::<AppTypeRegistry>().read();
        upgrade_lod_groups(&mut documents, &existing, &measure, &registry)
    };

    let mut documents = documents.into_iter();
    *authored = documents
        .next()
        .map(|document| document.ast)
        .unwrap_or_default();
    if outcome.converted == 0 {
        if !outcome.kept.is_empty() {
            info!("LOD groups left as they are: {}", outcome.kept.join("; "));
        }
        return None;
    }
    let mut before = UpgradeState {
        scene: before_scene,
        prefabs: Vec::new(),
        settings: Vec::new(),
    };
    let mut after = UpgradeState {
        scene: jackdaw_bsn::emit_scene(authored),
        prefabs: Vec::new(),
        settings: Vec::new(),
    };
    for (offset, (document, old)) in documents.zip(before_prefabs).enumerate() {
        if !outcome.changed.contains(&(offset + 1)) {
            continue;
        }
        let Some(path) = document.prefab else {
            continue;
        };
        before.prefabs.push((path.clone(), old));
        after
            .prefabs
            .push((path, jackdaw_bsn::emit_scene(&document.ast)));
    }
    for (model, lod) in &outcome.created {
        before.settings.push((model.clone(), None));
        after.settings.push((model.clone(), Some(lod.clone())));
    }

    let entry = UpgradeLodGroups { before, after };
    for (path, text) in &entry.after.prefabs {
        if let Ok(ast) = jackdaw_bsn::parse_bsn_text(text)
            && let Some(mut cache) = world.get_resource_mut::<crate::prefab::PrefabAstCache>()
        {
            cache.insert(path, ast);
        }
    }
    Some(PendingUpgrade {
        entry,
        converted: outcome.converted,
        kept: outcome.kept,
    })
}

/// Upgrade the LOD groups of the scene at `path` as it opens into a tab, with
/// the prefabs it instances read into the cache first.
pub fn upgrade_scene_being_opened(
    world: &mut World,
    doc: &mut SceneBsnAst,
    path: &Path,
) -> Option<PendingUpgrade> {
    let folder = path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let prefab_root = crate::prefab::save_load::source_root(world, &folder);
    let assets = world
        .get_resource::<crate::project::ProjectRoot>()
        .map_or_else(
            || prefab_root.clone(),
            crate::project::ProjectRoot::assets_dir,
        );
    let epoch = crate::scene_io::prefab_cache_epoch(world);
    if let Some(mut cache) = world.get_resource_mut::<crate::prefab::PrefabAstCache>() {
        crate::prefab::save_load::populate_cache_for_scene_bsn(
            doc,
            &mut cache,
            &prefab_root,
            &folder,
        );
    }
    let upgrade = upgrade_on_open(world, doc, &assets);
    crate::scene_io::forget_prefab_cache_bump(world, epoch);
    upgrade
}

/// Record an upgrade made while the scene opened as the open scene's first
/// undo entry, and say what it did.
pub fn finish_upgrade(world: &mut World, upgrade: PendingUpgrade) {
    for (path, _) in &upgrade.entry.after.prefabs {
        world
            .resource_mut::<UpgradedPrefabs>()
            .0
            .insert(path.clone());
    }
    for (model, lod) in &upgrade.entry.after.settings {
        world
            .resource_mut::<jackdaw_scene_types::model_import::ModelLodIndex>()
            .set(model, lod.clone());
        world
            .resource_mut::<crate::model_lod::UnsavedModelSettings>()
            .0
            .insert(model.clone());
    }
    world
        .resource_mut::<crate::commands::CommandHistory>()
        .push_executed(Box::new(upgrade.entry));
    if !upgrade.kept.is_empty() {
        info!(
            "LOD groups left as hand-built groups: {}",
            upgrade.kept.join("; ")
        );
    }
    crate::status_bar::notify_info(
        world,
        format!(
            "Upgraded {} LOD groups. Save to keep, Undo to restore.",
            upgrade.converted
        ),
    );
}

/// Write the prefabs an upgrade changed, returning their files.
pub fn write_upgraded_prefabs(world: &mut World) -> std::io::Result<Vec<PathBuf>> {
    let paths: Vec<PathBuf> = world
        .get_resource_mut::<UpgradedPrefabs>()
        .map(|mut upgraded| std::mem::take(&mut upgraded.0).into_iter().collect())
        .unwrap_or_default();
    for path in &paths {
        crate::prefab::operators::save_prefab_to_disk(world, path)?;
    }
    Ok(paths)
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<UpgradedPrefabs>();
}
