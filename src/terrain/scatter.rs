//! The `terrain.scatter` operator: PCG placement over paint channels.
//!
//! Generator proposes, painter disposes. A run stamps ordinary editable
//! entities -- the same `GltfSource` + `WorldAssetRoot` + `Transform`
//! shape a drag from the asset browser produces
//! ([`crate::entity_ops::spawn_gltf`]) -- parented under one named group
//! so the outliner shows a single collapsible node. Nothing about an
//! instance is special-cased afterwards: move it, scale it, delete it,
//! give it components.
//!
//! What makes a re-run safe is [`ScatterInstance`], a small reflected
//! marker recording the generator, the seed, and the exact transform the
//! generator produced. On re-run an instance whose live `Transform` still
//! equals its recorded one is *untouched* and gets replaced; one the user
//! moved, rotated or scaled no longer matches and is preserved. That is
//! the whole definition, and it is deliberately narrow: it needs no
//! bookkeeping beyond what is already saved in the scene, and it survives
//! a save/load round trip because both halves are reflected data.
//!
//! The random stream lives in [`jackdaw_terrain::scatter()`] and derives
//! from `(seed, cell index)` only. Nothing in this file adds randomness.

use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use bevy::world_serialization::WorldAssetRoot;
use jackdaw_api::prelude::*;
use jackdaw_feathers::{
    button::{self, ButtonProps, ButtonVariant},
    text_edit::{self, TextEditCommitEvent, TextEditProps},
    tokens,
};
use jackdaw_scene_types::GltfSource;
use jackdaw_terrain::{Heightmap, ScatterMask, ScatterParams};

use super::TerrainDataStore;
use super::inspector::{
    spawn_add_tile, spawn_hint, spawn_labeled_field, spawn_tile, spawn_tile_grid, spawn_tile_remove,
};
use super::ops::has_selected_terrain;

/// A run estimated to place more instances than this is refused rather
/// than spawning that many entities (and undo-stack history) in one
/// click. High enough for any legitimate scatter; low enough that a
/// stray density (a pasted value, a unit mismatch) cannot hang the
/// editor or balloon the undo stack.
const MAX_SCATTER_INSTANCES: u32 = 100_000;
use crate::commands::{CommandGroup, CommandHistory, DespawnEntity, EditorCommand, SpawnEntity};
use crate::selection::Selection;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<TerrainScatterState>()
        .init_resource::<TerrainScatterReport>()
        .register_type::<ScatterInstance>()
        .register_type::<ScatterGroup>()
        .add_observer(on_scatter_text_commit);
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TerrainScatterOp>()
        .register_operator::<TerrainScatterClearOp>()
        .register_operator::<TerrainScatterAssetAddOp>()
        .register_operator::<TerrainScatterAssetRemoveOp>()
        .register_operator::<TerrainScatterAssetToggleOp>()
        .register_operator::<TerrainScatterValueToggleOp>()
        .register_operator::<TerrainScatterToggleYawOp>()
        .register_operator::<TerrainScatterToggleAlignOp>();
}

// --- Provenance ---

/// Marks the group entity a scatter run stamps its instances under.
///
/// `key` is what a re-run matches on, so two scatter runs over the same
/// terrain (undergrowth and trees, say) stay independent stamps rather
/// than overwriting each other.
#[derive(Component, Reflect, Clone, Debug, Default)]
#[reflect(Component, Default)]
pub struct ScatterGroup {
    /// Operator id that produced this group.
    pub generator: String,
    /// Stamp identity. Matched on re-run.
    pub key: String,
}

/// Provenance carried by every generated instance.
///
/// `generated` is the transform this instance was born with. An instance
/// whose live `Transform` still equals it has not been touched by hand
/// and a re-run may replace it; anything else is the user's work and is
/// preserved. Exact equality is the right test here because nothing in
/// the editor perturbs a transform without the user asking -- there is no
/// physics settle, no re-import, no snapping pass over placed entities.
#[derive(Component, Reflect, Clone, Debug, Default)]
#[reflect(Component, Default)]
pub struct ScatterInstance {
    pub generator: String,
    pub key: String,
    pub seed: u64,
    pub generated: Transform,
}

// --- Panel state ---

/// One entry in the scatter palette.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScatterAsset {
    /// Project-relative or absolute path to a `.gltf` / `.glb`.
    pub path: String,
    /// Whether this entry takes part in the next run. Unreal's per-row
    /// checkbox and `Terrain3D`'s per-tile eye are the same control.
    pub active: bool,
}

impl ScatterAsset {
    /// File stem, used as the tile caption and the instance name.
    pub fn stem(&self) -> &str {
        std::path::Path::new(&self.path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("model")
    }
}

/// Everything the Scatter panel edits, and the defaults every parameter
/// of `terrain.scatter` falls back to when the caller omits it.
///
/// Keeping the defaults in a resource is what lets the same operator be
/// clicked with no arguments and scripted with all of them.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct TerrainScatterState {
    pub seed: u64,
    pub density: f32,
    pub min_spacing: f32,
    pub mask_channel: usize,
    /// Palette values that accept a cell. Empty means no mask.
    pub accept: Vec<u16>,
    /// Channel index scaling density per cell, or `None`.
    pub weight_channel: Option<usize>,
    pub scale_min: f32,
    pub scale_max: f32,
    pub random_yaw: bool,
    pub align_to_normal: bool,
    pub assets: Vec<ScatterAsset>,
    /// Path typed into the panel's asset field, waiting on `+`.
    pub asset_draft: String,
}

impl Default for TerrainScatterState {
    fn default() -> Self {
        Self {
            seed: 1,
            density: 0.5,
            min_spacing: 0.35,
            mask_channel: 0,
            accept: Vec::new(),
            weight_channel: None,
            scale_min: 0.8,
            scale_max: 1.25,
            random_yaw: true,
            align_to_normal: false,
            assets: Vec::new(),
            asset_draft: String::new(),
        }
    }
}

impl TerrainScatterState {
    fn active_assets(&self) -> Vec<String> {
        self.assets
            .iter()
            .filter(|asset| asset.active)
            .map(|asset| asset.path.clone())
            .collect()
    }
}

/// What the last run did. Shown in the panel and logged, so a headless
/// caller sees the same numbers a clicking user does.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct TerrainScatterReport {
    /// Instances the run spawned.
    pub placed: usize,
    /// Hand-edited instances the run left alone.
    pub kept: usize,
    /// Untouched instances the run replaced.
    pub replaced: usize,
    /// Human-readable summary, including the reason for a run that did
    /// nothing.
    pub message: String,
}

// --- Field bindings ---

/// Which Scatter panel field a `text_edit` writes into.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScatterField {
    AssetDraft,
    Seed,
    Density,
    Spacing,
    MaskChannel,
    WeightChannel,
    ScaleMin,
    ScaleMax,
}

// --- Operators ---

/// Scatter instances across a terrain from its paint channels.
///
/// `allows_undo = false` because the stamp pushes its own
/// [`CommandGroup`] history entry; letting the framework also capture a
/// scene diff would record the change twice.
#[operator(
    id = "terrain.scatter",
    label = "Scatter",
    description = "Place instances across a terrain, masked by a paint channel.",
    allows_undo = false,
    params(
        terrain(String, doc = "Name of the terrain entity. Defaults to the selection."),
        group(String, doc = "Stamp identity. Re-running the same group replaces it."),
        seed(
            i64,
            doc = "Random seed. The same seed always places the same instances."
        ),
        density(f64, doc = "Instances per square world unit."),
        spacing(f64, doc = "Minimum world-unit distance between instances."),
        channel(i64, doc = "Index of the mask channel."),
        accept(String, doc = "Comma-separated palette values that accept a cell."),
        weight_channel(i64, doc = "Channel index scaling density per cell. -1 for none."),
        assets(String, doc = "Comma-separated model paths to place."),
        scale_min(f64, doc = "Smallest uniform scale."),
        scale_max(f64, doc = "Largest uniform scale."),
        random_yaw(bool, doc = "Randomise rotation about Y."),
        align_to_normal(bool, doc = "Tilt instances onto the surface.")
    )
)]
pub(crate) fn terrain_scatter(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let params = params.0;
    commands.queue(move |world: &mut World| run_scatter(world, &params));
    OperatorResult::Finished
}

/// Delete a scatter stamp and everything in it, as one undo entry.
#[operator(
    id = "terrain.scatter.clear",
    label = "Clear Scatter",
    description = "Delete a scatter group and every instance under it.",
    allows_undo = false,
    params(group(String, doc = "Stamp identity to clear. Defaults to the selection's."))
)]
pub(crate) fn terrain_scatter_clear(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let key = params.as_str("group").map(str::to_string);
    commands.queue(move |world: &mut World| clear_scatter(world, key.as_deref()));
    OperatorResult::Finished
}

/// Add a model to the scatter palette.
#[operator(
    id = "terrain.scatter.asset.add",
    label = "Add Scatter Asset",
    description = "Add a model to the scatter palette.",
    allows_undo = false,
    params(path(String, doc = "Model path. Defaults to the panel's asset field."))
)]
pub(crate) fn terrain_scatter_asset_add(
    params: In<OperatorParameters>,
    mut state: ResMut<TerrainScatterState>,
) -> OperatorResult {
    let path = match params.as_str("path").filter(|p| !p.trim().is_empty()) {
        Some(path) => path.trim().to_string(),
        None => state.asset_draft.trim().to_string(),
    };
    if path.is_empty() || state.assets.iter().any(|asset| asset.path == path) {
        return OperatorResult::Cancelled;
    }
    state.assets.push(ScatterAsset { path, active: true });
    state.asset_draft.clear();
    OperatorResult::Finished
}

/// Remove a model from the scatter palette.
#[operator(
    id = "terrain.scatter.asset.remove",
    label = "Remove Scatter Asset",
    description = "Remove a model from the scatter palette.",
    allows_undo = false,
    params(index(i64, doc = "Palette index to remove."))
)]
pub(crate) fn terrain_scatter_asset_remove(
    params: In<OperatorParameters>,
    mut state: ResMut<TerrainScatterState>,
) -> OperatorResult {
    let index = usize::try_from(params.as_int("index")?).ok()?;
    if index >= state.assets.len() {
        return OperatorResult::Cancelled;
    }
    state.assets.remove(index);
    OperatorResult::Finished
}

/// Include or exclude one palette entry from the next run.
#[operator(
    id = "terrain.scatter.asset.toggle",
    label = "Toggle Scatter Asset",
    description = "Include or exclude a palette entry from the next run.",
    allows_undo = false,
    params(index(i64, doc = "Palette index to toggle."))
)]
pub(crate) fn terrain_scatter_asset_toggle(
    params: In<OperatorParameters>,
    mut state: ResMut<TerrainScatterState>,
) -> OperatorResult {
    let index = usize::try_from(params.as_int("index")?).ok()?;
    let asset = state.assets.get_mut(index)?;
    asset.active = !asset.active;
    OperatorResult::Finished
}

/// Include or exclude one mask palette value.
///
/// Takes the palette *row* index, like `terrain.channel.value.select`,
/// and resolves the value off the terrain -- so the tile grid can pass
/// its own index and the stored accept set stays in palette values.
#[operator(
    id = "terrain.scatter.value.toggle",
    label = "Toggle Mask Value",
    description = "Include or exclude a palette value from the scatter mask.",
    allows_undo = false,
    is_available = has_selected_terrain,
    params(index(i64, doc = "Palette row index in the mask channel."))
)]
pub(crate) fn terrain_scatter_value_toggle(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    terrains: Query<&jackdaw_scene_types::Terrain>,
    mut state: ResMut<TerrainScatterState>,
) -> OperatorResult {
    let row = usize::try_from(params.as_int("index")?).ok()?;
    let terrain = terrains.get(selection.primary()?).ok()?;
    let value = terrain
        .channels
        .get(state.mask_channel)?
        .palette
        .get(row)?
        .value;
    match state.accept.iter().position(|v| *v == value) {
        Some(at) => {
            state.accept.remove(at);
        }
        None => {
            state.accept.push(value);
            state.accept.sort_unstable();
        }
    }
    OperatorResult::Finished
}

/// Randomise rotation about Y, or stop doing so.
#[operator(
    id = "terrain.scatter.toggle_yaw",
    label = "Random Yaw",
    description = "Randomise each instance's rotation about Y.",
    allows_undo = false
)]
pub(crate) fn terrain_scatter_toggle_yaw(
    _: In<OperatorParameters>,
    mut state: ResMut<TerrainScatterState>,
) -> OperatorResult {
    state.random_yaw = !state.random_yaw;
    OperatorResult::Finished
}

/// Tilt instances onto the surface, or stand them upright.
#[operator(
    id = "terrain.scatter.toggle_align",
    label = "Align to Normal",
    description = "Tilt each instance onto the surface it sits on.",
    allows_undo = false
)]
pub(crate) fn terrain_scatter_toggle_align(
    _: In<OperatorParameters>,
    mut state: ResMut<TerrainScatterState>,
) -> OperatorResult {
    state.align_to_normal = !state.align_to_normal;
    OperatorResult::Finished
}

// --- The stamp ---

/// Resolve the terrain a run targets: an explicit name first, the
/// selection otherwise.
///
/// The name lookup is what makes the operator drivable with no mouse and
/// no selection, which is the whole point of taking it as a parameter.
fn resolve_terrain(world: &mut World, name: Option<&str>) -> Option<Entity> {
    if let Some(name) = name.filter(|n| !n.is_empty()) {
        let mut query = world.query::<(Entity, Option<&Name>, &jackdaw_scene_types::Terrain)>();
        return query
            .iter(world)
            .find(|(_, have, _)| have.is_some_and(|have| have.as_str() == name))
            .map(|(entity, _, _)| entity);
    }
    if let Some(entity) = world.resource::<Selection>().primary()
        && world.get::<jackdaw_scene_types::Terrain>(entity).is_some()
    {
        return Some(entity);
    }
    // A scene with exactly one terrain needs no disambiguation, and a
    // headless caller has no way to make a selection. Two or more and the
    // caller has to say which.
    let mut query = world.query_filtered::<Entity, With<jackdaw_scene_types::Terrain>>();
    let mut found = query.iter(world);
    let only = found.next()?;
    found.next().is_none().then_some(only)
}

/// Names of every terrain in the scene, for a failure message that says
/// what the caller could have asked for instead.
fn terrain_names(world: &mut World) -> Vec<String> {
    let mut query = world.query::<(Option<&Name>, &jackdaw_scene_types::Terrain)>();
    query
        .iter(world)
        .map(|(name, _)| {
            name.map(|n| n.as_str().to_string())
                .unwrap_or_else(|| "<unnamed>".to_string())
        })
        .collect()
}

/// Default stamp identity: one group per terrain unless the caller names
/// another.
fn default_group_key(world: &World, terrain: Entity) -> String {
    let name = world
        .get::<Name>(terrain)
        .map(|n| n.as_str().to_string())
        .unwrap_or_else(|| "Terrain".to_string());
    format!("{name} Scatter")
}

fn set_report(world: &mut World, report: TerrainScatterReport) {
    info!("terrain.scatter: {}", report.message);
    *world.resource_mut::<TerrainScatterReport>() = report;
}

fn run_scatter(world: &mut World, params: &OperatorParameters) {
    let Some(terrain_entity) = resolve_terrain(world, params.as_str("terrain")) else {
        let available = terrain_names(world);
        set_report(
            world,
            TerrainScatterReport {
                message: format!(
                    "no terrain resolved (asked for {:?}); this scene has {available:?}",
                    params.as_str("terrain").unwrap_or("")
                ),
                ..default()
            },
        );
        return;
    };
    let Some(terrain) = world
        .get::<jackdaw_scene_types::Terrain>(terrain_entity)
        .cloned()
    else {
        return;
    };
    let state = world.resource::<TerrainScatterState>().clone();

    let assets: Vec<String> = match params.as_str("assets").filter(|s| !s.trim().is_empty()) {
        Some(list) => list
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        None => state.active_assets(),
    };
    if assets.is_empty() {
        set_report(
            world,
            TerrainScatterReport {
                message: "no assets in the scatter palette".to_string(),
                ..default()
            },
        );
        return;
    }

    let accept: Vec<u16> = match params.as_str("accept").filter(|s| !s.trim().is_empty()) {
        Some(list) => list
            .split(',')
            .filter_map(|v| v.trim().parse::<u16>().ok())
            .collect(),
        None => state.accept.clone(),
    };
    let channel = params
        .as_int("channel")
        .and_then(|v| usize::try_from(v).ok())
        .unwrap_or(state.mask_channel);
    let weight_channel = match params.as_int("weight_channel") {
        Some(value) => usize::try_from(value).ok(),
        None => state.weight_channel,
    };
    let seed = params.as_int("seed").map_or(state.seed, |v| v as u64);

    let scatter_params = ScatterParams {
        seed,
        density: params
            .as_float("density")
            .map_or(state.density, |v| v as f32),
        min_spacing: params
            .as_float("spacing")
            .map_or(state.min_spacing, |v| v as f32),
        // An empty accept set means "no mask" rather than "accept
        // nothing": a terrain nobody painted should still scatter.
        mask: (!accept.is_empty()).then_some(ScatterMask { channel, accept }),
        weight_channel,
        scale_min: params
            .as_float("scale_min")
            .map_or(state.scale_min, |v| v as f32),
        scale_max: params
            .as_float("scale_max")
            .map_or(state.scale_max, |v| v as f32),
        random_yaw: params.as_bool("random_yaw").unwrap_or(state.random_yaw),
        align_to_normal: params
            .as_bool("align_to_normal")
            .unwrap_or(state.align_to_normal),
        asset_count: assets.len(),
    };

    // A mask or a weight channel only ever reduce the instance count from
    // here, so density times the full world area is a safe upper bound
    // on what the kernel would produce -- cheap to check before running
    // it. Without a cap, one click at a stray density (a pasted value, a
    // unit mismatch) can queue an unbounded number of spawns onto the
    // undo stack.
    let world_area = f64::from(terrain.size.x) * f64::from(terrain.size.y);
    let estimated_instances = f64::from(scatter_params.density) * world_area;
    if estimated_instances > MAX_SCATTER_INSTANCES as f64 {
        set_report(
            world,
            TerrainScatterReport {
                message: format!(
                    "refused: density {} over {world_area:.0} square units would place \
                     roughly {estimated_instances:.0} instances, over the \
                     {MAX_SCATTER_INSTANCES} limit; lower density or spacing and try again",
                    scatter_params.density,
                ),
                ..default()
            },
        );
        return;
    }

    let (heightmap, channels) = {
        let mut store = world.resource_mut::<TerrainDataStore>();
        let Some(data) = store.entry_for(&terrain) else {
            return;
        };
        (
            Heightmap {
                resolution: terrain.resolution,
                size: terrain.size,
                max_height: terrain.max_height,
                heights: data.heights.clone(),
            },
            data.channels.clone(),
        )
    };

    let placements = jackdaw_terrain::scatter(&heightmap, &channels, &scatter_params);

    let key = params
        .as_str("group")
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| default_group_key(world, terrain_entity));

    let terrain_transform = world
        .get::<Transform>(terrain_entity)
        .copied()
        .unwrap_or_default();

    stamp(
        world,
        StampRequest {
            key,
            seed,
            terrain: terrain_entity,
            terrain_transform,
            assets,
            placements,
        },
    );
}

struct StampRequest {
    key: String,
    seed: u64,
    terrain: Entity,
    /// Placements come out of the kernel in the terrain's local space, so
    /// the terrain's own transform has to be applied before they become
    /// world-space instances.
    terrain_transform: Transform,
    assets: Vec<String>,
    placements: Vec<jackdaw_terrain::Placement>,
}

/// Existing group entity for `key`, if a previous run made one.
fn find_group(world: &mut World, key: &str) -> Option<Entity> {
    let mut query = world.query::<(Entity, &ScatterGroup)>();
    query
        .iter(world)
        .find(|(_, group)| group.key == key)
        .map(|(entity, _)| entity)
}

/// Split a group's existing instances into the ones a re-run may replace
/// and the count it must preserve.
///
/// The test is exact transform equality against the recorded `generated`
/// transform. See [`ScatterInstance`] for why that is the whole rule.
fn partition_existing(world: &mut World, group: Entity) -> (Vec<Entity>, usize) {
    let children: Vec<Entity> = world
        .get::<Children>(group)
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    let mut untouched = Vec::new();
    let mut kept = 0;
    for child in children {
        let Some(generated) = world.get::<ScatterInstance>(child).map(|i| i.generated) else {
            // Something the user parented under the group by hand. Not
            // ours to remove.
            kept += 1;
            continue;
        };
        let live = world.get::<Transform>(child).copied().unwrap_or_default();
        if live == generated {
            untouched.push(child);
        } else {
            kept += 1;
        }
    }
    (untouched, kept)
}

fn stamp(world: &mut World, request: StampRequest) {
    let StampRequest {
        key,
        seed,
        terrain,
        terrain_transform,
        assets,
        placements,
    } = request;

    let existing_group = find_group(world, &key);
    let (stale, kept) = match existing_group {
        Some(group) => partition_existing(world, group),
        None => (Vec::new(), 0),
    };

    let mut cmds: Vec<Box<dyn EditorCommand>> = Vec::new();

    // The group entity's id is not known until its spawn command runs,
    // and redo re-runs it, so the instance commands read it out of a
    // shared slot rather than capturing an id that would go stale.
    let slot: Arc<Mutex<Option<Entity>>> = Arc::new(Mutex::new(existing_group));
    if existing_group.is_none() {
        let group_parent = world.get::<ChildOf>(terrain).map(ChildOf::parent);
        let group_parent = group_parent.filter(|p| world.get_entity(*p).is_ok());
        let slot = slot.clone();
        let key = key.clone();
        cmds.push(Box::new(SpawnEntity {
            spawned: None,
            spawn_fn: Box::new(move |world: &mut World| {
                let entity = spawn_group(world, &key, group_parent);
                *slot.lock().expect("scatter group slot") = Some(entity);
                entity
            }),
            label: "Scatter".to_string(),
        }));
    }

    for entity in &stale {
        cmds.push(Box::new(DespawnEntity::from_world(world, *entity)));
    }

    let placed = placements.len();
    for (index, placement) in placements.into_iter().enumerate() {
        let path = assets[placement.asset_index.min(assets.len() - 1)].clone();
        let name = format!(
            "{}-{index}",
            std::path::Path::new(&path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("scatter")
        );
        let transform = instance_transform(&placement, &terrain_transform);
        let provenance = ScatterInstance {
            generator: TerrainScatterOp::ID.to_string(),
            key: key.clone(),
            seed,
            generated: transform,
        };
        let slot = slot.clone();
        cmds.push(Box::new(SpawnEntity {
            spawned: None,
            spawn_fn: Box::new(move |world: &mut World| {
                let Some(parent) = *slot.lock().expect("scatter group slot") else {
                    return Entity::PLACEHOLDER;
                };
                spawn_instance(world, parent, &path, &name, transform, provenance.clone())
            }),
            label: "Scatter instance".to_string(),
        }));
    }

    let replaced = stale.len();
    let mut group: Box<dyn EditorCommand> = Box::new(CommandGroup {
        commands: cmds,
        label: format!("Scatter {key}"),
    });
    group.execute(world);
    world.resource_mut::<CommandHistory>().push_executed(group);

    set_report(
        world,
        TerrainScatterReport {
            placed,
            kept,
            replaced,
            message: format!(
                "placed {placed} in '{key}', replaced {replaced} generated, kept {kept} edited"
            ),
        },
    );
}

/// World transform for one placement, with the terrain's own transform
/// applied.
fn instance_transform(
    placement: &jackdaw_terrain::Placement,
    terrain_transform: &Transform,
) -> Transform {
    let upright = Quat::from_rotation_arc(Vec3::Y, placement.normal);
    let rotation = upright * Quat::from_rotation_y(placement.yaw);
    Transform {
        translation: terrain_transform.transform_point(placement.position),
        rotation: terrain_transform.rotation * rotation,
        scale: terrain_transform.scale * Vec3::splat(placement.scale),
    }
}

fn spawn_group(world: &mut World, key: &str, parent: Option<Entity>) -> Entity {
    let mut entity = world.spawn((
        Name::new(key.to_string()),
        ScatterGroup {
            generator: TerrainScatterOp::ID.to_string(),
            key: key.to_string(),
        },
        Transform::default(),
        Visibility::default(),
    ));
    if let Some(parent) = parent {
        entity.insert(ChildOf(parent));
    }
    let entity = entity.id();
    crate::scene_io::register_entity_in_ast(world, entity);
    entity
}

/// Spawn one instance with exactly the component shape a browser drop
/// produces, plus its provenance marker.
fn spawn_instance(
    world: &mut World,
    parent: Entity,
    path: &str,
    name: &str,
    transform: Transform,
    provenance: ScatterInstance,
) -> Entity {
    let asset_path = crate::entity_ops::to_asset_path(path);
    let scene = world
        .resource::<AssetServer>()
        .load(GltfAssetLabel::Scene(0).from_asset(asset_path));
    let entity = world
        .spawn((
            Name::new(name.to_string()),
            GltfSource {
                path: path.to_string(),
                scene_index: 0,
            },
            WorldAssetRoot(scene),
            transform,
            provenance,
            ChildOf(parent),
        ))
        .id();
    crate::scene_io::register_entity_in_ast(world, entity);
    entity
}

fn clear_scatter(world: &mut World, key: Option<&str>) {
    let key = match key.filter(|k| !k.is_empty()) {
        Some(key) => key.to_string(),
        None => {
            let Some(terrain) = resolve_terrain(world, None) else {
                return;
            };
            default_group_key(world, terrain)
        }
    };
    let Some(group) = find_group(world, &key) else {
        set_report(
            world,
            TerrainScatterReport {
                message: format!("no scatter group named '{key}'"),
                ..default()
            },
        );
        return;
    };
    let children: Vec<Entity> = world
        .get::<Children>(group)
        .map(|c| c.iter().collect())
        .unwrap_or_default();

    let mut cmds: Vec<Box<dyn EditorCommand>> = Vec::new();
    for child in &children {
        cmds.push(Box::new(DespawnEntity::from_world(world, *child)));
    }
    cmds.push(Box::new(DespawnEntity::from_world(world, group)));
    let removed = cmds.len();

    let mut command: Box<dyn EditorCommand> = Box::new(CommandGroup {
        commands: cmds,
        label: format!("Clear Scatter {key}"),
    });
    command.execute(world);
    world
        .resource_mut::<CommandHistory>()
        .push_executed(command);

    set_report(
        world,
        TerrainScatterReport {
            message: format!("cleared '{key}': {removed} entities removed"),
            ..default()
        },
    );
}

// --- Panel ---

/// What the Scatter panel's appearance depends on.
///
/// The inspector compares this rather than change-detecting the resource,
/// because the numeric fields are typed into and rebuilding the section
/// on every committed keystroke would take the focus away mid-edit.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct ScatterSignature {
    assets: Vec<(String, bool)>,
    accept: Vec<u16>,
    mask_channel: usize,
    random_yaw: bool,
    align_to_normal: bool,
    message: String,
}

pub(super) fn signature(
    state: &TerrainScatterState,
    report: &TerrainScatterReport,
) -> ScatterSignature {
    ScatterSignature {
        assets: state
            .assets
            .iter()
            .map(|asset| (asset.path.clone(), asset.active))
            .collect(),
        accept: state.accept.clone(),
        mask_channel: state.mask_channel,
        random_yaw: state.random_yaw,
        align_to_normal: state.align_to_normal,
        message: report.message.clone(),
    }
}

/// The Scatter section of the terrain inspector.
///
/// Laid out in the order the reference tools use, so the workflow reads
/// top to bottom the way it does in Unreal's Foliage panel and Unity's
/// Paint Trees: *what* is being placed (the asset palette), *where* it
/// may go (the mask), then *how much* and *how varied*.
pub(super) fn spawn_scatter_ui(
    commands: &mut Commands,
    parent: Entity,
    terrain: Option<&jackdaw_scene_types::Terrain>,
    state: &TerrainScatterState,
    report: &TerrainScatterReport,
) {
    // --- Palette ---
    spawn_hint(
        commands,
        parent,
        "Models to place. Click a tile to include or exclude it.",
    );
    let grid = spawn_tile_grid(commands, parent);
    for (index, asset) in state.assets.iter().enumerate() {
        spawn_tile(
            commands,
            grid,
            if asset.active {
                tokens::ACCENT_BLUE
            } else {
                Color::srgb(0.28, 0.28, 0.30)
            },
            asset.stem(),
            asset.active,
            TerrainScatterAssetToggleOp::ID,
            Some(index),
        );
        spawn_tile_remove(commands, grid, TerrainScatterAssetRemoveOp::ID, index);
    }
    spawn_add_tile(commands, grid, TerrainScatterAssetAddOp::ID);
    spawn_path_field(commands, parent, &state.asset_draft);

    // --- Mask ---
    // Reusing the channel's own palette swatches means the values here
    // read identically to the ones in the Paint Channels section: the
    // user picks the same tiles they painted with.
    let empty: &[jackdaw_scene_types::TerrainChannel] = &[];
    let channels = terrain.map(|t| t.channels.as_slice()).unwrap_or(empty);
    if let Some(channel) = channels.get(state.mask_channel) {
        spawn_hint(
            commands,
            parent,
            &format!("Place only on '{}' values:", channel.name),
        );
        let values = spawn_tile_grid(commands, parent);
        for (row, entry) in channel.palette.iter().enumerate() {
            spawn_tile(
                commands,
                values,
                entry.color,
                &entry.label,
                state.accept.contains(&entry.value),
                TerrainScatterValueToggleOp::ID,
                Some(row),
            );
        }
        if state.accept.is_empty() {
            spawn_hint(
                commands,
                parent,
                "No value picked, so the whole terrain is eligible.",
            );
        }
    } else {
        spawn_hint(
            commands,
            parent,
            "This terrain has no paint channels, so nothing masks the scatter.",
        );
    }

    // --- Numerics ---
    spawn_labeled_field(
        commands,
        parent,
        "Seed",
        "Same seed always places the same instances",
        state.seed as f64,
        ScatterField::Seed,
    );
    spawn_labeled_field(
        commands,
        parent,
        "Density",
        "Instances per square world unit",
        state.density as f64,
        ScatterField::Density,
    );
    spawn_labeled_field(
        commands,
        parent,
        "Min Spacing",
        "Closest two instances may sit, in world units",
        state.min_spacing as f64,
        ScatterField::Spacing,
    );
    spawn_labeled_field(
        commands,
        parent,
        "Mask Channel",
        "Which paint channel gates placement",
        state.mask_channel as f64,
        ScatterField::MaskChannel,
    );
    spawn_labeled_field(
        commands,
        parent,
        "Weight Channel",
        "Channel scaling density per cell. -1 for none",
        state.weight_channel.map_or(-1.0, |c| c as f64),
        ScatterField::WeightChannel,
    );
    spawn_labeled_field(
        commands,
        parent,
        "Scale Min",
        "Smallest uniform scale",
        state.scale_min as f64,
        ScatterField::ScaleMin,
    );
    spawn_labeled_field(
        commands,
        parent,
        "Scale Max",
        "Largest uniform scale",
        state.scale_max as f64,
        ScatterField::ScaleMax,
    );

    // --- Toggles ---
    // Toggle buttons rather than checkboxes, matching the Quantization
    // section directly above: the label carries the state, so there is no
    // separate control to read.
    spawn_flag_toggle(
        commands,
        parent,
        "Random Yaw",
        state.random_yaw,
        TerrainScatterToggleYawOp::ID,
    );
    spawn_flag_toggle(
        commands,
        parent,
        "Align to Normal",
        state.align_to_normal,
        TerrainScatterToggleAlignOp::ID,
    );

    // --- Run ---
    commands.spawn((
        button::button(
            ButtonProps::new("Scatter")
                .with_variant(ButtonVariant::Primary)
                .call_operator(TerrainScatterOp::ID),
        ),
        ChildOf(parent),
    ));
    commands.spawn((
        button::button(ButtonProps::new("Clear Scatter").call_operator(TerrainScatterClearOp::ID)),
        ChildOf(parent),
    ));

    // Instance-count feedback, the way Unreal shows a live count on the
    // foliage type and ProtonScatter shows Instance Count: without it a
    // user cannot tell a run that placed nothing from one that never ran.
    if !report.message.is_empty() {
        spawn_hint(commands, parent, &report.message);
    }
}

/// The asset-path field. Its own helper rather than the numeric one next
/// to it, because a path is text.
fn spawn_path_field(commands: &mut Commands, parent: Entity, value: &str) {
    let row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(tokens::SPACING_XS),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();
    commands.spawn((
        Text::new("Asset Path"),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(row),
    ));
    commands.spawn((
        Text::new("Model to add with +, relative to the project's assets/"),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(row),
    ));
    commands.spawn((
        text_edit::text_edit(TextEditProps::default().with_default_value(value.to_string())),
        ScatterField::AssetDraft,
        ChildOf(row),
    ));
}

fn spawn_flag_toggle(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    on: bool,
    op_id: &'static str,
) {
    commands.spawn((
        button::button(
            ButtonProps::new(format!("{label}: {}", if on { "On" } else { "Off" }))
                .with_variant(if on {
                    ButtonVariant::Primary
                } else {
                    ButtonVariant::Default
                })
                .call_operator(op_id),
        ),
        ChildOf(parent),
    ));
}

/// Commit handler for the Scatter section's text fields.
///
/// A separate observer from the sculpt/generation one so the Scatter
/// panel owns its own bindings; both fire on every commit and each
/// ignores fields it does not recognise.
fn on_scatter_text_commit(
    event: On<TextEditCommitEvent>,
    bindings: Query<&ScatterField>,
    child_of_query: Query<&ChildOf>,
    mut state: ResMut<TerrainScatterState>,
) {
    let mut current = event.entity;
    for _ in 0..4 {
        let Ok(child_of) = child_of_query.get(current) else {
            return;
        };
        let parent = child_of.parent();
        if let Ok(&field) = bindings.get(parent) {
            apply_scatter_field(&mut state, field, &event.text);
            return;
        }
        current = parent;
    }
}

fn apply_scatter_field(state: &mut TerrainScatterState, field: ScatterField, text: &str) {
    if field == ScatterField::AssetDraft {
        state.asset_draft = text.trim().to_string();
        return;
    }
    let number: f64 = text.trim().parse().unwrap_or(0.0);
    match field {
        ScatterField::AssetDraft => unreachable!("handled above"),
        ScatterField::Seed => state.seed = number.max(0.0) as u64,
        ScatterField::Density => state.density = (number as f32).max(0.0),
        ScatterField::Spacing => state.min_spacing = (number as f32).max(0.0),
        ScatterField::MaskChannel => state.mask_channel = number.max(0.0) as usize,
        // -1 is the "no weight channel" spelling, so the field can say
        // "off" without a separate toggle beside it.
        ScatterField::WeightChannel => {
            state.weight_channel = (number >= 0.0).then_some(number as usize);
        }
        ScatterField::ScaleMin => state.scale_min = (number as f32).max(0.0),
        ScatterField::ScaleMax => state.scale_max = (number as f32).max(0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_active_palette_entries_take_part_in_a_run() {
        let state = TerrainScatterState {
            assets: vec![
                ScatterAsset {
                    path: "kit/Tree.gltf".to_string(),
                    active: true,
                },
                ScatterAsset {
                    path: "kit/Bush.gltf".to_string(),
                    active: false,
                },
            ],
            ..default()
        };
        assert_eq!(state.active_assets(), vec!["kit/Tree.gltf".to_string()]);
    }

    #[test]
    fn a_palette_entry_captions_with_its_file_stem() {
        let asset = ScatterAsset {
            path: "kit/nature/CommonTree_1.gltf".to_string(),
            active: true,
        };
        assert_eq!(asset.stem(), "CommonTree_1");
    }

    /// `-1` in the weight-channel field means "no weight channel", so the
    /// panel does not need a separate toggle beside it.
    #[test]
    fn a_negative_weight_channel_turns_the_weight_off() {
        let mut state = TerrainScatterState::default();
        apply_scatter_field(&mut state, ScatterField::WeightChannel, "0");
        assert_eq!(state.weight_channel, Some(0));
        apply_scatter_field(&mut state, ScatterField::WeightChannel, "-1");
        assert_eq!(state.weight_channel, None);
    }

    #[test]
    fn numeric_fields_clamp_rather_than_going_negative() {
        let mut state = TerrainScatterState::default();
        apply_scatter_field(&mut state, ScatterField::Density, "-4");
        assert_eq!(state.density, 0.0);
        apply_scatter_field(&mut state, ScatterField::Spacing, "-1");
        assert_eq!(state.min_spacing, 0.0);
        apply_scatter_field(&mut state, ScatterField::Seed, "42");
        assert_eq!(state.seed, 42);
    }

    #[test]
    fn the_asset_field_takes_text_rather_than_a_number() {
        let mut state = TerrainScatterState::default();
        apply_scatter_field(&mut state, ScatterField::AssetDraft, " kit/Tree.gltf ");
        assert_eq!(state.asset_draft, "kit/Tree.gltf");
    }

    /// An instance whose transform still matches what the generator made
    /// is replaceable; anything else is the user's work.
    #[test]
    fn untouched_means_the_transform_still_equals_the_generated_one() {
        let generated = Transform::from_xyz(1.0, 2.0, 3.0);
        let instance = ScatterInstance {
            generated,
            ..default()
        };
        assert_eq!(generated, instance.generated);
        assert_ne!(Transform::from_xyz(1.0, 2.5, 3.0), instance.generated);
    }

    /// The terrain's own transform has to reach the instances, or a
    /// scatter over a moved terrain lands under the world origin.
    #[test]
    fn the_terrain_transform_carries_into_the_instance_transform() {
        let placement = jackdaw_terrain::Placement {
            position: Vec3::new(1.0, 0.5, -2.0),
            yaw: 0.0,
            scale: 2.0,
            normal: Vec3::Y,
            asset_index: 0,
            cell: 0,
        };
        let placed = instance_transform(&placement, &Transform::from_xyz(10.0, 0.0, 5.0));
        assert_eq!(placed.translation, Vec3::new(11.0, 0.5, 3.0));
        assert_eq!(placed.scale, Vec3::splat(2.0));
    }

    /// Align-to-normal tilts the instance; upright leaves it alone.
    #[test]
    fn a_tilted_normal_tilts_the_instance() {
        let placement = jackdaw_terrain::Placement {
            position: Vec3::ZERO,
            yaw: 0.0,
            scale: 1.0,
            normal: Vec3::new(0.5, 1.0, 0.0).normalize(),
            asset_index: 0,
            cell: 0,
        };
        let tilted = instance_transform(&placement, &Transform::default());
        assert!((tilted.rotation * Vec3::Y).angle_between(Vec3::Y) > 0.1);

        let upright = instance_transform(
            &jackdaw_terrain::Placement {
                normal: Vec3::Y,
                ..placement
            },
            &Transform::default(),
        );
        assert!((upright.rotation * Vec3::Y).angle_between(Vec3::Y) < 1e-4);
    }

    /// I7 pinning test, the exact scenario from the review finding: a
    /// stray density over a large terrain would previously queue an
    /// unbounded number of spawns onto the undo stack. The run must be
    /// refused up front, with a report, and never reach the kernel.
    #[test]
    fn a_pathological_density_is_refused_before_running_the_kernel() {
        let mut world = World::new();
        world.init_resource::<Selection>();
        world.init_resource::<TerrainScatterState>();
        world.init_resource::<TerrainScatterReport>();

        let data_path = "zone.terrain-0.jdterrain";
        let mut store = TerrainDataStore::default();
        store.insert(
            data_path,
            jackdaw_terrain::TerrainData {
                resolution: 4,
                heights: vec![0.0; 16],
                channels: vec![],
            },
        );
        world.insert_resource(store);

        world.spawn(jackdaw_scene_types::Terrain {
            resolution: 4,
            size: Vec2::splat(1000.0),
            data_path: data_path.to_string(),
            ..default()
        });

        use jackdaw_scene_types::PropertyValue;

        let mut params = OperatorParameters::default();
        params
            .0
            .insert("density".to_string(), PropertyValue::Float(1000.0));
        params.0.insert(
            "assets".to_string(),
            PropertyValue::String("kit/Tree.gltf".into()),
        );

        run_scatter(&mut world, &params);

        let report = world.resource::<TerrainScatterReport>();
        assert_eq!(report.placed, 0, "a refused run must place nothing");
        assert!(
            report.message.contains("refused") && report.message.contains("100000"),
            "report must explain the refusal: {}",
            report.message
        );
        assert!(
            world.query::<&ScatterGroup>().iter(&world).next().is_none(),
            "a refused run must not spawn a group",
        );
    }
}
