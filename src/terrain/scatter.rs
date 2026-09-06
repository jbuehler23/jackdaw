//! The `terrain.scatter` operator: PCG placement over paint channels.
//!
//! A run stamps ordinary editable entities under one named group. Each
//! carries a `ScatterInstance` recording the generator, the seed and the
//! transform produced; a re-run replaces instances whose live `Transform`
//! still equals the recorded one and preserves the ones the user moved.

use std::sync::{Arc, Mutex};

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::ui::Checked;
use bevy::ui_widgets::{SliderValue, ValueChange};
use bevy::world_serialization::WorldAssetRoot;
use jackdaw_api::prelude::*;
use jackdaw_feathers::{
    button::{self, ButtonProps, ButtonVariant},
    text_edit::{self, TextEditCommitEvent, TextEditProps},
    tokens,
};
use jackdaw_scene_types::GltfSource;
use jackdaw_terrain::{ScatterMask, ScatterParams};

use super::TerrainDataStore;
use super::ops::has_selected_terrain;
use super::scatter_data::{self, PendingPlacement};
use super::ui_fields::{
    FieldKind, spawn_add_tile, spawn_checkbox, spawn_hint, spawn_slider_row, spawn_tile,
    spawn_tile_grid, spawn_tile_remove,
};

/// A run estimated to place more instances than this is refused rather
/// than spawning that many entities, and that much undo history, in one
/// click.
const MAX_SCATTER_INSTANCES: u32 = 100_000;
use crate::commands::{CommandGroup, CommandHistory, DespawnEntity, EditorCommand, SpawnEntity};
use crate::selection::Selection;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<TerrainScatterState>()
        .init_resource::<TerrainScatterReport>()
        .add_systems(
            Update,
            sync_scatter_fields.run_if(in_state(crate::AppState::Editor)),
        )
        .add_observer(on_scatter_asset_draft_commit)
        .add_observer(on_scatter_value_change)
        .add_observer(on_scatter_checkbox_value_change);
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TerrainScatterOp>()
        .register_operator::<TerrainScatterClearOp>()
        .register_operator::<TerrainScatterAdoptOp>()
        .register_operator::<TerrainScatterPromoteOp>()
        .register_operator::<TerrainScatterGroupSelectOp>()
        .register_operator::<TerrainScatterAssetAddOp>()
        .register_operator::<TerrainScatterAssetRemoveOp>()
        .register_operator::<TerrainScatterAssetToggleOp>()
        .register_operator::<TerrainScatterValueToggleOp>()
        .register_operator::<TerrainScatterToggleYawOp>()
        .register_operator::<TerrainScatterToggleAlignOp>();
}

// --- Provenance ---

/// Provenance is scene data: a group the editor cannot read back out of a
/// reopened scene is one the next run duplicates.
pub use jackdaw_scene_types::{ScatterGroup, ScatterInstance};

// --- Panel state ---

/// One entry in the scatter palette.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScatterAsset {
    /// Project-relative or absolute path to a `.gltf` / `.glb`.
    pub path: String,
    /// Whether this entry takes part in the next run.
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

/// What the last run did, shown in the panel and logged.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct TerrainScatterReport {
    /// Instances the run spawned.
    pub placed: usize,
    /// Hand-edited instances the run left alone.
    pub kept: usize,
    /// Untouched instances the run replaced.
    pub replaced: usize,
    /// Human-readable summary, including the reason a run did nothing.
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
/// `allows_undo = false`: the stamp pushes its own `CommandGroup` entry, so
/// a framework scene diff would record the change twice.
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
    params(
        terrain(
            String,
            doc = "Name of the terrain the group sits under. Defaults to the selection."
        ),
        group(String, doc = "Stamp identity to clear. Defaults to the selection's.")
    )
)]
pub(crate) fn terrain_scatter_clear(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let key = params.as_str("group").map(str::to_string);
    let terrain = params.as_str("terrain").map(str::to_string);
    commands.queue(move |world: &mut World| {
        clear_scatter(world, terrain.as_deref(), key.as_deref());
    });
    OperatorResult::Finished
}

/// Take a hand-authored group of models into the scatter loop.
///
/// Reparents the group under the terrain, converts its instances to
/// terrain-local and stamps the provenance a run would have left, as one
/// undo entry. Only direct children carrying a `GltfSource` are taken.
#[operator(
    id = "terrain.scatter.adopt",
    label = "Adopt Scatter Group",
    description = "Put a hand-authored group under a terrain with scatter provenance.",
    allows_undo = false,
    params(
        entity(Entity, doc = "Group to adopt. Defaults to the selection."),
        terrain(
            String,
            doc = "Name of the terrain that adopts it. Defaults to the selection."
        ),
        key(
            String,
            doc = "Stamp identity to give it. Defaults to the group's name."
        )
    )
)]
pub(crate) fn terrain_scatter_adopt(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let entity = params.as_entity("entity")?;
    let terrain = params.as_str("terrain").map(str::to_string);
    let key = params.as_str("key").map(str::to_string);
    commands.queue(move |world: &mut World| {
        adopt_group(world, entity, terrain.as_deref(), key.as_deref());
    });
    OperatorResult::Finished
}

/// Turn one stored placement back into an editable entity, the inverse of
/// `terrain_scatter_adopt` for a single instance.
#[operator(
    id = "terrain.scatter.promote",
    label = "Promote Placement",
    description = "Turn one stored scatter placement into an editable entity.",
    allows_undo = false,
    params(
        terrain(
            String,
            doc = "Name of the terrain holding the placement. Defaults to the selection."
        ),
        key(String, doc = "Stamp identity the placement belongs to."),
        index(i64, doc = "Position of the placement within that group.")
    )
)]
pub(crate) fn terrain_scatter_promote(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let terrain = params.as_str("terrain").map(str::to_string);
    let key = params.as_str("key").map(str::to_string);
    let index = usize::try_from(params.as_int("index").unwrap_or(0)).ok()?;
    commands.queue(move |world: &mut World| {
        promote_placement(world, terrain.as_deref(), key.as_deref(), index);
    });
    OperatorResult::Finished
}

/// Select a scatter group by its stamp key, so the outliner and the
/// panel's buttons act on it without a click in the tree.
#[operator(
    id = "terrain.scatter.group.select",
    label = "Select Scatter Group",
    description = "Select a terrain's scatter group by its stamp identity.",
    allows_undo = false,
    params(
        terrain(
            String,
            doc = "Name of the terrain the group sits under. Defaults to the selection."
        ),
        key(String, doc = "Stamp identity to select.")
    )
)]
pub(crate) fn terrain_scatter_group_select(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let terrain = params.as_str("terrain").map(str::to_string);
    let key = params.as_str("key").map(str::to_string);
    commands.queue(move |world: &mut World| {
        let Some(terrain) = resolve_terrain(world, terrain.as_deref()) else {
            return;
        };
        let key = match key.filter(|k| !k.is_empty()) {
            Some(key) => key,
            None => default_group_key(world, terrain),
        };
        let Some(group) = find_group(world, terrain, &key) else {
            set_report(
                world,
                TerrainScatterReport {
                    message: format!("no scatter group named '{key}' under this terrain"),
                    ..default()
                },
            );
            return;
        };
        crate::selection::select_only(world, group);
    });
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
/// Takes the palette row index and resolves the value off the terrain, so
/// the stored accept set stays in palette values.
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
pub(crate) fn resolve_terrain(world: &mut World, name: Option<&str>) -> Option<Entity> {
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
    // headless caller cannot make a selection.
    let mut query = world.query_filtered::<Entity, With<jackdaw_scene_types::Terrain>>();
    let mut found = query.iter(world);
    let only = found.next()?;
    found.next().is_none().then_some(only)
}

/// Names of every terrain in the scene, for a failure message listing what
/// the caller could name.
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

/// Whether `global` is a rotation, a translation and one uniform scale --
/// the only shapes a child `Transform` can reproduce exactly.
fn is_rigid_with_uniform_scale(global: GlobalTransform) -> bool {
    let scale = global.compute_transform().scale;
    let uniform = (scale.x - scale.y).abs() <= 1e-4 * scale.x.abs().max(1.0)
        && (scale.x - scale.z).abs() <= 1e-4 * scale.x.abs().max(1.0);
    if !uniform {
        return false;
    }
    // `compute_transform` decomposes whatever it is given, so the round
    // trip is what catches a shear.
    let recomposed = global.compute_transform().compute_affine();
    recomposed.abs_diff_eq(global.affine(), 1e-3)
}

/// An entity's world pose composed from the authored `Transform`s along
/// its ancestor chain.
///
/// `GlobalTransform` is a frame behind for anything spawned or reparented
/// since the last propagation pass, so the locals are composed instead.
fn composed_global(world: &World, entity: Entity) -> bevy::math::Affine3A {
    let mut affine = bevy::math::Affine3A::IDENTITY;
    let mut at = Some(entity);
    // `ChildOf` cannot spell a cycle, but a malformed scene must not hang
    // a load, so the walk is bounded.
    for _ in 0..1024 {
        let Some(current) = at else { break };
        let local = world
            .get::<Transform>(current)
            .copied()
            .unwrap_or_default()
            .compute_affine();
        affine = local * affine;
        at = world.get::<ChildOf>(current).map(ChildOf::parent);
    }
    affine
}

/// Reparent scatter groups that were saved beside their terrain onto it.
///
/// Instances are stored in the terrain's local space, and a group the
/// lookup cannot see is one a re-scatter duplicates. Each child is
/// converted into the terrain's space, so nothing moves on screen. Only a
/// group with exactly one terrain among its siblings is moved.
pub(crate) fn migrate_legacy_scatter_groups(world: &mut World) {
    let mut group_query = world.query_filtered::<Entity, With<ScatterGroup>>();
    let groups: Vec<Entity> = group_query.iter(world).collect();
    if groups.is_empty() {
        return;
    }
    let mut terrain_query = world.query_filtered::<Entity, With<jackdaw_scene_types::Terrain>>();
    let terrains: Vec<Entity> = terrain_query.iter(world).collect();

    for group in groups {
        let parent = world.get::<ChildOf>(group).map(ChildOf::parent);
        if parent.is_some_and(|parent| terrains.contains(&parent)) {
            continue;
        }
        let mut siblings = terrains
            .iter()
            .copied()
            .filter(|terrain| world.get::<ChildOf>(*terrain).map(ChildOf::parent) == parent);
        let Some(terrain) = siblings.next() else {
            continue;
        };
        if siblings.next().is_some() {
            continue;
        }
        reparent_group_onto_terrain(world, group, terrain);
    }
}

/// Move `group` under `terrain` at identity, holding every child's world
/// pose. No history entry: a format migration is not an edit the user made.
fn reparent_group_onto_terrain(world: &mut World, group: Entity, terrain: Entity) {
    let to_terrain = composed_global(world, terrain).inverse() * composed_global(world, group);
    let children: Vec<Entity> = world
        .get::<Children>(group)
        .map(|children| children.iter().collect())
        .unwrap_or_default();

    let mut commands: Vec<Box<dyn EditorCommand>> = vec![
        Box::new(crate::commands::ReparentEntity {
            entity: group,
            old_parent: world.get::<ChildOf>(group).map(ChildOf::parent),
            new_parent: Some(terrain),
        }),
        Box::new(crate::commands::SetTransform {
            entity: group,
            old_transform: world.get::<Transform>(group).copied().unwrap_or_default(),
            new_transform: Transform::default(),
        }),
    ];
    for child in children {
        let old = world.get::<Transform>(child).copied().unwrap_or_default();
        let new = Transform::from_matrix(Mat4::from(to_terrain * old.compute_affine()));
        if new == old {
            continue;
        }
        commands.push(Box::new(crate::commands::SetTransform {
            entity: child,
            old_transform: old,
            new_transform: new,
        }));
        // The recorded pose is what a re-run compares a live transform
        // against, so it moves into the terrain's space alongside it.
        if let Some(mut instance) = world.get::<ScatterInstance>(child).cloned() {
            instance.generated = new;
            commands.push(Box::new(StampProvenance {
                entity: child,
                value: instance,
                previous: None,
            }));
        }
    }
    for command in &mut commands {
        command.execute(world);
    }
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
        // An empty accept set means no mask rather than accept nothing, so
        // an unpainted terrain still scatters.
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

    // A mask or a weight channel only reduces the instance count, so
    // density times the full world area bounds what the kernel would
    // produce and can be checked before running it.
    let ground = world
        .resource::<TerrainDataStore>()
        .grid_shape(&terrain)
        .size;
    let world_area = f64::from(ground.x) * f64::from(ground.y);
    // A terrain holding no regions covers no ground, and the cap above is
    // measured against area, so every density would pass it.
    if world_area <= 0.0 {
        set_report(
            world,
            TerrainScatterReport {
                message: "refused: no ground to scatter on; generate the terrain first".to_string(),
                ..default()
            },
        );
        return;
    }
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

    // Read rather than `entry_for`: scattering writes no heights, so it
    // does not retire the terrain's shared heightmap. It masks over the
    // whole terrain at once, so the planes are gathered into dense form.
    let channels = {
        let mut store = world.resource_mut::<TerrainDataStore>();
        let Some(data) = store.read_for(&terrain) else {
            return;
        };
        let document = data.document();
        let resolution = document.grid_resolution();
        document
            .channels
            .iter()
            .enumerate()
            .map(|(index, descriptor)| jackdaw_terrain::ChannelData {
                name: descriptor.name.clone(),
                element: descriptor.element,
                values: document.regions.read_grid_channel(index, resolution),
            })
            .collect::<Vec<_>>()
    };
    let heightmap = world.resource::<TerrainDataStore>().heightmap(&terrain);

    let placements = jackdaw_terrain::scatter(&heightmap.map, &channels, &scatter_params);

    let key = params
        .as_str("group")
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| default_group_key(world, terrain_entity));

    let pending: Vec<PendingPlacement> = placements
        .iter()
        .map(|placement| PendingPlacement {
            position: placement.position,
            yaw: placement.yaw,
            scale: placement.scale,
            asset: placement.asset_index,
        })
        .collect();

    // A group that is still entities is replaced as entities: a run that
    // wrote both forms would draw everything twice.
    if find_group(world, terrain_entity, &key).is_some() {
        stamp(
            world,
            StampRequest {
                key,
                seed,
                terrain: terrain_entity,
                assets,
                placements,
            },
        );
        return;
    }

    // A stored placement is a yaw and a uniform scale, so a tilt has
    // nowhere to live in it. Said out loud, because the option still
    // applies to the groups that are entities.
    if scatter_params.align_to_normal {
        jackdaw_api_internal::operator::warn_caller(
            world,
            "scatter: align to normal has no effect on a stored group -- a stored placement \
             stands upright; promote a placement to tilt it",
        );
    }
    let stored = match scatter_data::stamp(world, &terrain.data_path, &key, &assets, pending) {
        Ok(stored) => stored,
        Err(reason) => {
            set_report(
                world,
                TerrainScatterReport {
                    message: format!("refused: {}", reason.message()),
                    ..default()
                },
            );
            return;
        }
    };
    set_report(
        world,
        TerrainScatterReport {
            placed: stored,
            message: format!("placed {stored} in '{key}'"),
            ..default()
        },
    );
}

struct StampRequest {
    key: String,
    seed: u64,
    terrain: Entity,
    assets: Vec<String>,
    placements: Vec<jackdaw_terrain::Placement>,
}

/// Existing group entity for `key` under `terrain`, if a previous run made
/// one.
///
/// Scoped to the terrain: two terrains in one scene default their stamp to
/// the same key, and a global match would cross them.
fn find_group(world: &mut World, terrain: Entity, key: &str) -> Option<Entity> {
    let mut query = world.query::<(Entity, &ScatterGroup, &ChildOf)>();
    query
        .iter(world)
        .find(|(_, group, child_of)| group.key == key && child_of.parent() == terrain)
        .map(|(entity, _, _)| entity)
}

/// The group for `key` under `terrain`, with its existing instances split
/// into the ones a re-run may replace and the count it must preserve.
///
/// The test is exact transform equality against the recorded `generated`
/// transform; a child parented by hand carries no provenance and is kept.
/// hand carries no provenance and is counted as kept, never removed.
fn partition_existing(
    world: &mut World,
    terrain: Entity,
    key: &str,
) -> (Option<Entity>, Vec<Entity>, usize) {
    let Some(group) = find_group(world, terrain, key) else {
        return (None, Vec::new(), 0);
    };
    let children: Vec<Entity> = world
        .get::<Children>(group)
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    let mut untouched = Vec::new();
    let mut kept = 0;
    for child in children {
        let Some(generated) = world.get::<ScatterInstance>(child).map(|i| i.generated) else {
            // Parented under the group by hand, so it is not removed.
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
    (Some(group), untouched, kept)
}

fn stamp(world: &mut World, request: StampRequest) {
    let StampRequest {
        key,
        seed,
        terrain,
        assets,
        placements,
    } = request;

    let (existing_group, stale, kept) = partition_existing(world, terrain, &key);

    let mut cmds: Vec<Box<dyn EditorCommand>> = Vec::new();

    // The group entity's id is unknown until its spawn command runs, and
    // redo re-runs it, so the instance commands read it from a shared slot.
    let slot: Arc<Mutex<Option<Entity>>> = Arc::new(Mutex::new(existing_group));
    if existing_group.is_none() {
        let slot = slot.clone();
        let key = key.clone();
        cmds.push(Box::new(SpawnEntity {
            spawned: None,
            spawn_fn: Box::new(move |world: &mut World| {
                let entity = spawn_group(world, &key, terrain);
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
        let transform = instance_transform(&placement);
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

/// Take one stored placement out of the document and spawn a model in its
/// place, as one undo entry.
fn promote_placement(
    world: &mut World,
    terrain_name: Option<&str>,
    key: Option<&str>,
    index: usize,
) {
    let Some(terrain) = resolve_terrain(world, terrain_name) else {
        set_report(
            world,
            TerrainScatterReport {
                message: "promote: no terrain resolved".to_string(),
                ..default()
            },
        );
        return;
    };
    let data_path = world
        .get::<jackdaw_scene_types::Terrain>(terrain)
        .map(|t| t.data_path.clone())
        .unwrap_or_default();
    let key = match key.filter(|k| !k.is_empty()) {
        Some(key) => key.to_string(),
        None => default_group_key(world, terrain),
    };
    let Some(promoted) = scatter_data::nth_in_group(
        world.resource::<TerrainDataStore>(),
        &data_path,
        &key,
        index,
    ) else {
        set_report(
            world,
            TerrainScatterReport {
                message: format!("promote: '{key}' has no placement {index}"),
                ..default()
            },
        );
        return;
    };

    let name = format!(
        "{}-{index}",
        std::path::Path::new(&promoted.asset)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("scatter")
    );
    let asset = promoted.asset.clone();
    let transform = promoted.transform;
    let mut commands: Vec<Box<dyn EditorCommand>> = vec![Box::new(SpawnEntity {
        spawned: None,
        spawn_fn: Box::new(move |world: &mut World| {
            spawn_instance(
                world,
                terrain,
                &asset,
                &name,
                transform,
                ScatterInstance {
                    generator: TerrainScatterOp::ID.to_string(),
                    key: String::new(),
                    seed: 0,
                    generated: transform,
                },
            )
        }),
        label: "Promote Placement".to_string(),
    })];
    commands.push(Box::new(RemovePlacement {
        data_path: data_path.clone(),
        region: promoted.region,
        index: promoted.index,
        removed: None,
    }));

    let mut command: Box<dyn EditorCommand> = Box::new(CommandGroup {
        commands,
        label: format!("Promote {key} {index}"),
    });
    command.execute(world);
    world
        .resource_mut::<CommandHistory>()
        .push_executed(command);

    set_report(
        world,
        TerrainScatterReport {
            message: format!("promoted placement {index} of '{key}' to an entity"),
            ..default()
        },
    );
}

/// Take one placement out of a terrain's document, and put it back on undo.
struct RemovePlacement {
    data_path: String,
    region: jackdaw_terrain::RegionCoord,
    index: usize,
    /// What was taken out, so undo can put it back where it was.
    removed: Option<jackdaw_terrain::ScatterPlacement>,
}

impl EditorCommand for RemovePlacement {
    fn execute(&mut self, world: &mut World) {
        let mut store = world.resource_mut::<TerrainDataStore>();
        let Some(data) = store.document_mut(&self.data_path) else {
            return;
        };
        self.removed = data.remove_placement(self.region, self.index);
        store.mark_scatter_dirty(&self.data_path, Some(self.region));
    }

    fn undo(&mut self, world: &mut World) {
        let Some(placement) = self.removed.take() else {
            return;
        };
        let mut store = world.resource_mut::<TerrainDataStore>();
        let Some(data) = store.document_mut(&self.data_path) else {
            return;
        };
        if let Some(region) = data.regions.region_mut(self.region) {
            let at = self.index.min(region.placements().len());
            region.placements_mut().insert(at, placement);
        }
        store.mark_scatter_dirty(&self.data_path, Some(self.region));
    }

    fn description(&self) -> &str {
        "Promote Placement"
    }
}

/// Transform for one placement, in the terrain's local space.
///
/// The group sits on the terrain at identity, so a placement is stored as
/// the kernel produced it and moves with the terrain.
fn instance_transform(placement: &jackdaw_terrain::Placement) -> Transform {
    let upright = Quat::from_rotation_arc(Vec3::Y, placement.normal);
    Transform {
        translation: placement.position,
        rotation: upright * Quat::from_rotation_y(placement.yaw),
        scale: Vec3::splat(placement.scale),
    }
}

fn spawn_group(world: &mut World, key: &str, terrain: Entity) -> Entity {
    let entity = world
        .spawn((
            Name::new(key.to_string()),
            ScatterGroup {
                generator: TerrainScatterOp::ID.to_string(),
                key: key.to_string(),
            },
            Transform::default(),
            Visibility::default(),
            ChildOf(terrain),
        ))
        .id();
    crate::scene_io::register_entity_in_ast(world, entity);
    entity
}

/// Spawn one instance with the component shape a browser drop produces,
/// plus its provenance marker.
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

fn clear_scatter(world: &mut World, terrain_name: Option<&str>, key: Option<&str>) {
    let Some(terrain) = resolve_terrain(world, terrain_name) else {
        // A script naming a group has no selection to fall back on, and
        // finishing silently would read as "the group is gone".
        let available = terrain_names(world);
        let message = format!(
            "clear: no terrain resolved (asked for {:?}); this scene has {available:?}",
            terrain_name.unwrap_or("")
        );
        jackdaw_api_internal::operator::warn_caller(world, message.clone());
        set_report(
            world,
            TerrainScatterReport {
                message,
                ..default()
            },
        );
        return;
    };
    let key = match key.filter(|k| !k.is_empty()) {
        Some(key) => key.to_string(),
        None => default_group_key(world, terrain),
    };
    let Some(group) = find_group(world, terrain, &key) else {
        let data_path = world
            .get::<jackdaw_scene_types::Terrain>(terrain)
            .map(|t| t.data_path.clone())
            .unwrap_or_default();
        let removed = scatter_data::clear(world, &data_path, &key).unwrap_or(0);
        set_report(
            world,
            TerrainScatterReport {
                message: if removed > 0 {
                    format!("cleared '{key}': {removed} placements removed")
                } else {
                    format!("no scatter group named '{key}' under this terrain")
                },
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

// --- Adoption ---

/// Insert one reflected component, and take it away again on undo.
///
/// `AddComponent` adds a component at its default, which for provenance
/// would be an empty key; this command carries the value.
struct StampProvenance<T: Component + Clone + Reflect + TypePath> {
    entity: Entity,
    value: T,
    /// What the entity carried before, restored on undo.
    ///
    /// Adopting a group whose children already carry provenance under
    /// another key overwrites it, and undo must not leave them looking
    /// hand-placed.
    previous: Option<T>,
}

impl<T: Component<Mutability = bevy::ecs::component::Mutable> + Clone + Reflect + TypePath>
    EditorCommand for StampProvenance<T>
{
    fn execute(&mut self, world: &mut World) {
        let Ok(mut entity) = world.get_entity_mut(self.entity) else {
            return;
        };
        self.previous = entity.get::<T>().cloned();
        entity.insert(self.value.clone());
        crate::commands::sync_component_to_ast(world, self.entity, T::type_path(), &self.value);
    }

    fn undo(&mut self, world: &mut World) {
        if let Some(previous) = self.previous.clone() {
            if let Ok(mut entity) = world.get_entity_mut(self.entity) {
                entity.insert(previous.clone());
            }
            crate::commands::sync_component_to_ast(world, self.entity, T::type_path(), &previous);
            return;
        }
        if let Ok(mut entity) = world.get_entity_mut(self.entity) {
            entity.remove::<T>();
        }
        let Some(mut ast) = world.get_resource_mut::<jackdaw_bsn::SceneBsnAst>() else {
            return;
        };
        if let Some(node) = ast.ast_for(self.entity) {
            ast.remove_component_patch(node, T::type_path());
        }
    }

    fn description(&self) -> &str {
        "Stamp scatter provenance"
    }
}

/// Take a hand-authored group into the terrain's stored scatter.
///
/// Every model child becomes a placement at the pose it already stood in
/// and the group goes; one undo entry restores both halves.
fn adopt_group(world: &mut World, entity: Entity, terrain_name: Option<&str>, key: Option<&str>) {
    let Some(terrain) = resolve_terrain(world, terrain_name) else {
        let available = terrain_names(world);
        set_report(
            world,
            TerrainScatterReport {
                message: format!(
                    "adopt: no terrain resolved (asked for {:?}); this scene has {available:?}",
                    terrain_name.unwrap_or("")
                ),
                ..default()
            },
        );
        return;
    };
    if world.get_entity(entity).is_err() || entity == terrain {
        set_report(
            world,
            TerrainScatterReport {
                message: "adopt: pick a group beside the terrain, not the terrain itself"
                    .to_string(),
                ..default()
            },
        );
        return;
    }
    // A group a previous build stamped as entities keeps that key, so
    // adopting it moves those instances under the name the panel shows.
    let existing_key = world.get::<ScatterGroup>(entity).map(|g| g.key.clone());
    let key = match key.filter(|k| !k.is_empty()) {
        Some(key) => key.to_string(),
        None => existing_key
            .clone()
            .or_else(|| {
                world
                    .get::<Name>(entity)
                    .map(|name| name.as_str().to_string())
            })
            .unwrap_or_else(|| default_group_key(world, terrain)),
    };
    if find_group(world, terrain, &key).is_some_and(|group| group != entity) {
        set_report(
            world,
            TerrainScatterReport {
                message: format!(
                    "adopt: a scatter group named '{key}' is already under this terrain"
                ),
                ..default()
            },
        );
        return;
    }

    // World poses are read before anything moves; the placements below
    // reproduce them against a terrain the group is no longer under.
    let terrain_global = GlobalTransform::from(composed_global(world, terrain));
    // A local cannot spell a shear or a non-uniform scale composed with a
    // rotation, so under such a terrain the decomposition below is the
    // nearest pose, not the same one, and the caller is told.
    if !is_rigid_with_uniform_scale(terrain_global) {
        jackdaw_api_internal::operator::warn_caller(
            world,
            "adopt: this terrain is sheared or scaled unevenly, so the adopted poses are \
             the nearest a transform can spell and the group may shift slightly",
        );
    }
    let terrain_inverse = terrain_global.affine().inverse();
    let children: Vec<Entity> = world
        .get::<Children>(entity)
        .map(|children| children.iter().collect())
        .unwrap_or_default();
    let models: Vec<(String, Transform)> = children
        .into_iter()
        .filter_map(|child| {
            let asset = world.get::<GltfSource>(child)?.path.clone();
            let global = composed_global(world, child);
            let local = Transform::from_matrix(Mat4::from(terrain_inverse * global));
            Some((asset, local))
        })
        .collect();
    if models.is_empty() {
        set_report(
            world,
            TerrainScatterReport {
                message: "adopt: this group holds no models".to_string(),
                ..default()
            },
        );
        return;
    }

    let data_path = world
        .get::<jackdaw_scene_types::Terrain>(terrain)
        .map(|t| t.data_path.clone())
        .unwrap_or_default();
    let adopted = match scatter_data::adopt(world, &data_path, &key, entity, models) {
        Ok(adopted) => adopted,
        Err(reason) => {
            set_report(
                world,
                TerrainScatterReport {
                    message: format!("adopt: {}", reason.message()),
                    ..default()
                },
            );
            return;
        }
    };

    set_report(
        world,
        TerrainScatterReport {
            placed: adopted,
            message: format!("adopted '{key}': {adopted} placements now stored on the terrain"),
            ..default()
        },
    );
}

// --- Panel ---

/// What the Scatter panel's appearance depends on.
///
/// Compared rather than change-detected on the resource: rebuilding the
/// section on every committed keystroke would take focus away mid-edit.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct ScatterSignature {
    assets: Vec<(String, bool)>,
    accept: Vec<u16>,
    mask_channel: usize,
    random_yaw: bool,
    align_to_normal: bool,
    message: String,
    groups: Vec<ScatterGroupRow>,
    adopt_candidate: Option<(Entity, String)>,
}

/// One row of the Groups section: a scatter group living under the
/// selected terrain.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct ScatterGroupRow {
    /// Stamp identity, which is what the buttons pass back as `group=`.
    pub key: String,
    /// The entity's own name, or the key itself for a group stored as data.
    pub name: String,
    /// Children carrying provenance, which is what a re-run replaces.
    pub instances: usize,
    /// Whether this group lives in the terrain's document rather than as
    /// entities.
    pub stored: bool,
}

/// Everything the Scatter tab reads, as one system parameter so the panel
/// system stays under bevy's argument limit.
#[derive(SystemParam)]
pub(super) struct ScatterTabRefs<'w, 's> {
    pub state: Res<'w, TerrainScatterState>,
    pub report: Res<'w, TerrainScatterReport>,
    store: Res<'w, TerrainDataStore>,
    terrains: Query<'w, 's, &'static jackdaw_scene_types::Terrain>,
    groups: Query<'w, 's, &'static ScatterGroup>,
    names: Query<'w, 's, &'static Name>,
    children: Query<'w, 's, &'static Children>,
    instances: Query<'w, 's, (), With<ScatterInstance>>,
    models: Query<'w, 's, (), With<GltfSource>>,
}

impl ScatterTabRefs<'_, '_> {
    /// The terrain's own name, which the scatter operators address it by.
    fn terrain_name(&self, terrain: Entity) -> Option<String> {
        self.names.get(terrain).ok().map(|n| n.as_str().to_string())
    }

    /// The scatter groups under `terrain`, sorted by key so the section
    /// does not reorder itself between frames.
    fn rows(&self, terrain: Entity) -> Vec<ScatterGroupRow> {
        let mut rows: Vec<ScatterGroupRow> = self
            .children
            .get(terrain)
            .into_iter()
            .flat_map(RelationshipTarget::iter)
            .filter_map(|child| {
                let group = self.groups.get(child).ok()?;
                Some(ScatterGroupRow {
                    key: group.key.clone(),
                    name: self
                        .names
                        .get(child)
                        .map_or_else(|_| group.key.clone(), |name| name.as_str().to_string()),
                    instances: self
                        .children
                        .get(child)
                        .into_iter()
                        .flat_map(RelationshipTarget::iter)
                        .filter(|entity| self.instances.contains(*entity))
                        .count(),
                    stored: false,
                })
            })
            .collect();
        // Stored groups sit beside the entity ones. A key can only be one
        // or the other: adopting a group replaces its entities with data.
        if let Ok(component) = self.terrains.get(terrain) {
            for (key, count) in scatter_data::group_counts(&self.store, &component.data_path) {
                if rows.iter().any(|row| row.key == key) {
                    continue;
                }
                rows.push(ScatterGroupRow {
                    name: key.clone(),
                    key,
                    instances: count,
                    stored: true,
                });
            }
        }
        rows.sort_by(|a, b| a.key.cmp(&b.key));
        rows
    }

    /// The selected node, when it is one `terrain.scatter.adopt` would
    /// take: not the terrain, not already a group, and holding models.
    fn adopt_candidate(&self, selection: &Selection, terrain: Entity) -> Option<(Entity, String)> {
        let entity = selection.primary()?;
        if entity == terrain {
            return None;
        }
        let children = self.children.get(entity).ok()?;
        children
            .iter()
            .any(|child| self.models.contains(child))
            .then(|| {
                let name = self
                    .names
                    .get(entity)
                    .map_or_else(|_| "the selection".to_string(), |n| n.as_str().to_string());
                (entity, name)
            })
    }
}

pub(super) fn signature(refs: &ScatterTabRefs, view: &ScatterGroupsView) -> ScatterSignature {
    let state = &refs.state;
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
        message: refs.report.message.clone(),
        groups: view.groups.clone(),
        adopt_candidate: view.adopt_candidate.clone(),
    }
}

/// What the panel hands the Groups section, read once per rebuild so the
/// buttons carry the same targets the signature was taken from.
#[derive(Clone)]
pub(super) struct ScatterGroupsView {
    pub groups: Vec<ScatterGroupRow>,
    pub adopt_candidate: Option<(Entity, String)>,
    pub terrain_name: Option<String>,
}

pub(super) fn groups_view(
    refs: &ScatterTabRefs,
    selection: &Selection,
    terrain: Entity,
) -> ScatterGroupsView {
    ScatterGroupsView {
        groups: refs.rows(terrain),
        adopt_candidate: refs.adopt_candidate(selection, terrain),
        terrain_name: refs.terrain_name(terrain),
    }
}

/// The Scatter section of the terrain inspector.
pub(super) fn spawn_scatter_ui(
    commands: &mut Commands,
    parent: Entity,
    terrain: Option<&jackdaw_scene_types::Terrain>,
    refs: &ScatterTabRefs,
    view: &ScatterGroupsView,
) {
    let state = &refs.state;
    let report = &refs.report;
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
    spawn_slider_row(
        commands,
        parent,
        "Seed",
        "Same seed always places the same instances",
        state.seed as f32,
        0.0..100_000.0,
        FieldKind::Count,
        ScatterField::Seed,
    );
    spawn_slider_row(
        commands,
        parent,
        "Density",
        "Instances per square world unit",
        state.density,
        0.0..10.0,
        FieldKind::Continuous,
        ScatterField::Density,
    );
    spawn_slider_row(
        commands,
        parent,
        "Min Spacing",
        "Closest two instances may sit, in world units",
        state.min_spacing,
        0.0..10.0,
        FieldKind::Continuous,
        ScatterField::Spacing,
    );
    spawn_slider_row(
        commands,
        parent,
        "Mask Channel",
        "Which paint channel gates placement",
        state.mask_channel as f32,
        0.0..16.0,
        FieldKind::Count,
        ScatterField::MaskChannel,
    );
    spawn_slider_row(
        commands,
        parent,
        "Weight Channel",
        "Channel scaling density per cell. -1 for none",
        state.weight_channel.map_or(-1.0, |c| c as f32),
        -1.0..16.0,
        FieldKind::Count,
        ScatterField::WeightChannel,
    );
    spawn_slider_row(
        commands,
        parent,
        "Scale Min",
        "Smallest uniform scale",
        state.scale_min,
        0.0..10.0,
        FieldKind::Continuous,
        ScatterField::ScaleMin,
    );
    spawn_slider_row(
        commands,
        parent,
        "Scale Max",
        "Largest uniform scale",
        state.scale_max,
        0.0..10.0,
        FieldKind::Continuous,
        ScatterField::ScaleMax,
    );

    // --- Toggles ---
    spawn_checkbox(
        commands,
        parent,
        "Random Yaw",
        state.random_yaw,
        RandomYawCheckbox,
    );
    spawn_checkbox(
        commands,
        parent,
        "Align to Normal",
        state.align_to_normal,
        AlignToNormalCheckbox,
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

    // The report line is what separates a run that placed nothing from one
    // that never ran.
    if !report.message.is_empty() {
        spawn_hint(commands, parent, &report.message);
    }

    spawn_groups_section(commands, parent, view);
}

/// The stamps already living under this terrain, with the actions
/// available on each.
fn spawn_groups_section(commands: &mut Commands, parent: Entity, view: &ScatterGroupsView) {
    spawn_hint(commands, parent, "Groups under this terrain");
    if view.groups.is_empty() {
        spawn_hint(
            commands,
            parent,
            "None yet. A run makes one, or adopt a group you placed by hand.",
        );
    }
    for row in &view.groups {
        let heading = commands
            .spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(tokens::SPACING_XS),
                    width: Val::Percent(100.0),
                    ..Default::default()
                },
                ChildOf(parent),
            ))
            .id();
        let label = commands
            .spawn((
                Text::new(if row.stored {
                    format!("{} ({} placements)", row.name, row.instances)
                } else {
                    format!("{} ({} instances)", row.name, row.instances)
                }),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
                ChildOf(heading),
            ))
            .id();
        if !row.stored {
            let key = row.key.clone();
            let terrain_name = view.terrain_name.clone();
            commands
                .entity(label)
                .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
                    let mut call = commands
                        .operator(TerrainScatterGroupSelectOp::ID)
                        .param("key", key.clone());
                    if let Some(name) = terrain_name.clone() {
                        call = call.param("terrain", name);
                    }
                    call.settings(selection_dispatch_settings()).call();
                });
        }

        for (caption, op_id) in [
            ("Re-scatter", TerrainScatterOp::ID),
            ("Clear", TerrainScatterClearOp::ID),
        ] {
            let key = row.key.clone();
            let terrain_name = view.terrain_name.clone();
            let button = commands
                .spawn((button::button(ButtonProps::new(caption)), ChildOf(heading)))
                .id();
            commands.entity(button).observe(
                move |_: On<Pointer<Click>>, mut commands: Commands| {
                    let mut call = commands.operator(op_id).param("group", key.clone());
                    if let Some(name) = terrain_name.clone() {
                        call = call.param("terrain", name);
                    }
                    call.settings(group_dispatch_settings()).call();
                },
            );
        }
    }

    // Disabled rather than absent, so the button says what the selection
    // would have to be instead of vanishing when it is wrong.
    let adopt = commands
        .spawn((
            button::button(ButtonProps::new("Adopt selected group").with_variant(
                if view.adopt_candidate.is_some() {
                    ButtonVariant::Default
                } else {
                    ButtonVariant::Disabled
                },
            )),
            ChildOf(parent),
        ))
        .id();
    let hint = match &view.adopt_candidate {
        Some((entity, name)) => {
            let entity = *entity;
            let terrain_name = view.terrain_name.clone();
            commands
                .entity(adopt)
                .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
                    let mut call = commands
                        .operator(TerrainScatterAdoptOp::ID)
                        .param("entity", entity);
                    if let Some(name) = terrain_name.clone() {
                        call = call.param("terrain", name);
                    }
                    call.settings(group_dispatch_settings()).call();
                });
            format!("Adopts '{name}' into this terrain's scatter.")
        }
        None => "Select a group of models beside the terrain to adopt it.".to_string(),
    };
    spawn_hint(commands, parent, &hint);
}

/// Dispatch settings for the Groups section's buttons: each is a document
/// edit the user expects one undo to take back.
fn group_dispatch_settings() -> CallOperatorSettings {
    CallOperatorSettings {
        creates_history_entry: true,
        execution_context: ExecutionContext::Invoke,
    }
}

/// Dispatch settings for clicking a group's name: a selection change, not
/// a document edit, so it takes no undo entry.
fn selection_dispatch_settings() -> CallOperatorSettings {
    CallOperatorSettings {
        creates_history_entry: false,
        execution_context: ExecutionContext::Invoke,
    }
}

/// The asset-path field. A separate helper from the numeric one beside it,
/// since a path is text.
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

/// Tags the Scatter section's "Random Yaw" checkbox.
#[derive(Component)]
struct RandomYawCheckbox;

/// Tags the Scatter section's "Align to Normal" checkbox.
#[derive(Component)]
struct AlignToNormalCheckbox;

/// Commit handler for the Scatter section's asset-path text field.
fn on_scatter_asset_draft_commit(
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
        if let Ok(&field) = bindings.get(parent)
            && field == ScatterField::AssetDraft
        {
            state.asset_draft = event.text.trim().to_string();
            return;
        }
        current = parent;
    }
}

/// Commit handler for the Scatter section's scrub-drag numeric fields.
fn on_scatter_value_change(
    event: On<ValueChange<f32>>,
    bindings: Query<&ScatterField>,
    mut state: ResMut<TerrainScatterState>,
) {
    let Ok(&field) = bindings.get(event.event_target()) else {
        return;
    };
    apply_scatter_numeric_field(&mut state, field, event.value);
}

fn apply_scatter_numeric_field(state: &mut TerrainScatterState, field: ScatterField, value: f32) {
    match field {
        ScatterField::AssetDraft => {}
        ScatterField::Seed => state.seed = value.max(0.0) as u64,
        ScatterField::Density => state.density = value.max(0.0),
        ScatterField::Spacing => state.min_spacing = value.max(0.0),
        ScatterField::MaskChannel => state.mask_channel = value.max(0.0) as usize,
        // -1 spells no weight channel, so the field says "off" without a
        // separate toggle beside it.
        ScatterField::WeightChannel => {
            state.weight_channel = (value >= 0.0).then_some(value as usize);
        }
        ScatterField::ScaleMin => state.scale_min = value.max(0.0),
        ScatterField::ScaleMax => state.scale_max = value.max(0.0),
    }
}

/// Re-insert `SliderValue` on every scrub-drag numeric field whenever
/// `TerrainScatterState` changes, so the fill and digits track a drag live.
fn sync_scatter_fields(
    state: Res<TerrainScatterState>,
    fields: Query<(Entity, &ScatterField)>,
    mut commands: Commands,
) {
    if !state.is_changed() {
        return;
    }
    for (entity, field) in &fields {
        let value = match field {
            ScatterField::AssetDraft => continue,
            ScatterField::Seed => state.seed as f32,
            ScatterField::Density => state.density,
            ScatterField::Spacing => state.min_spacing,
            ScatterField::MaskChannel => state.mask_channel as f32,
            ScatterField::WeightChannel => state.weight_channel.map_or(-1.0, |c| c as f32),
            ScatterField::ScaleMin => state.scale_min,
            ScatterField::ScaleMax => state.scale_max,
        };
        commands.entity(entity).insert(SliderValue(value));
    }
}

/// Commit handler for the Scatter section's two toggles.
/// `FeathersCheckbox` does not self-manage `Checked`, so this reflects the
/// new value onto the source entity before dispatching.
fn on_scatter_checkbox_value_change(
    event: On<ValueChange<bool>>,
    yaw: Query<(), With<RandomYawCheckbox>>,
    align: Query<(), With<AlignToNormalCheckbox>>,
    mut commands: Commands,
) {
    let target = event.event_target();
    let op_id = if yaw.contains(target) {
        TerrainScatterToggleYawOp::ID
    } else if align.contains(target) {
        TerrainScatterToggleAlignOp::ID
    } else {
        return;
    };

    jackdaw_feathers::utils::set_marker_if_alive::<Checked>(&mut commands, target, event.value);

    commands
        .operator(op_id)
        .settings(CallOperatorSettings {
            creates_history_entry: true,
            execution_context: ExecutionContext::Invoke,
        })
        .call();
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

    /// `-1` in the weight-channel field turns the weight off, so the panel
    /// needs no separate toggle beside it.
    #[test]
    fn a_negative_weight_channel_turns_the_weight_off() {
        let mut state = TerrainScatterState::default();
        apply_scatter_numeric_field(&mut state, ScatterField::WeightChannel, 0.0);
        assert_eq!(state.weight_channel, Some(0));
        apply_scatter_numeric_field(&mut state, ScatterField::WeightChannel, -1.0);
        assert_eq!(state.weight_channel, None);
    }

    #[test]
    fn numeric_fields_clamp_rather_than_going_negative() {
        let mut state = TerrainScatterState::default();
        apply_scatter_numeric_field(&mut state, ScatterField::Density, -4.0);
        assert_eq!(state.density, 0.0);
        apply_scatter_numeric_field(&mut state, ScatterField::Spacing, -1.0);
        assert_eq!(state.min_spacing, 0.0);
        apply_scatter_numeric_field(&mut state, ScatterField::Seed, 42.0);
        assert_eq!(state.seed, 42);
    }

    /// An instance whose transform matches its generated one is
    /// replaceable; anything else is preserved.
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

    /// An instance is stored in the terrain's space, not the world's: the
    /// group hangs off the terrain at identity, so the placement the
    /// kernel produced is the transform that is saved.
    #[test]
    fn an_instance_is_stored_in_terrain_local_space() {
        let placement = jackdaw_terrain::Placement {
            position: Vec3::new(1.0, 0.5, -2.0),
            yaw: 0.0,
            scale: 2.0,
            normal: Vec3::Y,
            asset_index: 0,
            cell: 0,
        };
        let placed = instance_transform(&placement);
        assert_eq!(placed.translation, Vec3::new(1.0, 0.5, -2.0));
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
        let tilted = instance_transform(&placement);
        assert!((tilted.rotation * Vec3::Y).angle_between(Vec3::Y) > 0.1);

        let upright = instance_transform(&jackdaw_terrain::Placement {
            normal: Vec3::Y,
            ..placement
        });
        assert!((upright.rotation * Vec3::Y).angle_between(Vec3::Y) < 1e-4);
    }

    /// A stray density over a large terrain is refused up front, with a
    /// report, and never reaches the kernel.
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
            jackdaw_terrain::RegionTerrainData::from_legacy_v1(&jackdaw_terrain::TerrainData {
                resolution: 4,
                heights: vec![0.0; 16],
                channels: vec![],
            })
            .expect("a power-of-two resolution migrates"),
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

    /// An ungenerated terrain holds no regions and so nothing to scatter
    /// on. The run says so and stops: the density cap is measured against
    /// the ground, and zero ground lets any density through.
    #[test]
    fn a_terrain_with_no_ground_is_refused_before_running_the_kernel() {
        let mut world = World::new();
        world.init_resource::<Selection>();
        world.init_resource::<TerrainScatterState>();
        world.init_resource::<TerrainScatterReport>();

        let data_path = "zone.terrain-0.jdterrain";
        let mut store = TerrainDataStore::default();
        store.insert(data_path, jackdaw_terrain::RegionTerrainData::default());
        world.insert_resource(store);

        world.spawn(jackdaw_scene_types::Terrain {
            data_path: data_path.to_string(),
            ..default()
        });

        use jackdaw_scene_types::PropertyValue;
        let mut params = OperatorParameters::default();
        params
            .0
            .insert("density".to_string(), PropertyValue::Float(0.1));
        params.0.insert(
            "assets".to_string(),
            PropertyValue::String("kit/Tree.gltf".into()),
        );

        run_scatter(&mut world, &params);

        let report = world.resource::<TerrainScatterReport>();
        assert_eq!(report.placed, 0, "a refused run must place nothing");
        assert!(
            report.message.contains("refused") && report.message.contains("no ground"),
            "report must explain the refusal: {}",
            report.message
        );
        assert!(
            world.query::<&ScatterGroup>().iter(&world).next().is_none(),
            "a refused run must not spawn a group",
        );
    }

    /// An intermediate drag tick updates both the state and the widget's
    /// `SliderValue`, not only the final tick.
    #[test]
    fn intermediate_drag_tick_updates_state_and_resyncs_the_widget_live() {
        let mut app = App::new();
        app.init_resource::<TerrainScatterState>();
        app.add_systems(Update, sync_scatter_fields);
        app.add_observer(on_scatter_value_change);

        let entity = app
            .world_mut()
            .spawn((ScatterField::Density, SliderValue(0.5)))
            .id();

        app.world_mut().trigger(ValueChange::<f32> {
            source: entity,
            value: 3.0,
            is_final: false,
        });
        app.update();

        assert!(
            (app.world().resource::<TerrainScatterState>().density - 3.0).abs() < 1e-5,
            "state must update on every drag tick, not just the final one",
        );
        let synced = app.world().get::<SliderValue>(entity).unwrap();
        assert!(
            (synced.0 - 3.0).abs() < 1e-5,
            "the field's own SliderValue must be resynced the same pass",
        );
    }

    /// `AssetDraft` lives on a `text_edit` rather than a slider, so the
    /// resync skips it instead of inserting a `SliderValue`.
    #[test]
    fn asset_draft_field_is_not_given_a_slider_value() {
        let mut app = App::new();
        app.init_resource::<TerrainScatterState>();
        app.add_systems(Update, sync_scatter_fields);

        let entity = app.world_mut().spawn(ScatterField::AssetDraft).id();
        app.world_mut()
            .resource_mut::<TerrainScatterState>()
            .density = 9.0;
        app.update();

        assert!(app.world().get::<SliderValue>(entity).is_none());
    }

    /// A world with the resources a run touches, plus the asset server
    /// `spawn_instance` loads through and the transform propagation the
    /// adoption reads world poses from.
    fn scatter_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::asset::AssetPlugin::default(),
            bevy::transform::TransformPlugin,
        ));
        app.init_asset::<bevy::world_serialization::WorldAsset>();
        app.init_resource::<Selection>()
            .init_resource::<TerrainScatterState>()
            .init_resource::<TerrainScatterReport>()
            .init_resource::<CommandHistory>()
            .init_resource::<jackdaw_bsn::SceneBsnAst>()
            .init_resource::<TerrainDataStore>();
        app
    }

    /// One flat terrain with ground under it, placed away from the origin
    /// so world space and terrain space cannot be mistaken for each other.
    fn spawn_ground(app: &mut App, name: &str, at: Vec3) -> Entity {
        let data_path = format!("{name}.terrain-0.jdterrain");
        app.world_mut().resource_mut::<TerrainDataStore>().insert(
            &data_path,
            jackdaw_terrain::RegionTerrainData::from_legacy_v1(&jackdaw_terrain::TerrainData {
                resolution: 16,
                heights: vec![0.0; 16 * 16],
                channels: vec![],
            })
            .expect("a power-of-two resolution migrates"),
        );
        app.world_mut()
            .spawn((
                Name::new(name.to_string()),
                jackdaw_scene_types::Terrain {
                    resolution: 16,
                    size: Vec2::splat(16.0),
                    data_path,
                    ..default()
                },
                Transform::from_translation(at),
                Visibility::default(),
            ))
            .id()
    }

    fn scatter_params(terrain: &str) -> OperatorParameters {
        use jackdaw_scene_types::PropertyValue;
        let mut params = OperatorParameters::default();
        params.0.insert(
            "terrain".to_string(),
            PropertyValue::String(terrain.to_string().into()),
        );
        params
            .0
            .insert("density".to_string(), PropertyValue::Float(0.2));
        params.0.insert(
            "assets".to_string(),
            PropertyValue::String("kit/Tree.gltf".into()),
        );
        params
    }

    fn groups_of(world: &mut World) -> Vec<(Entity, ScatterGroup)> {
        let mut query = world.query::<(Entity, &ScatterGroup)>();
        query
            .iter(world)
            .map(|(entity, group)| (entity, group.clone()))
            .collect()
    }

    /// A run stores placements on the terrain rather than spawning
    /// entities, in terrain space, so moving the terrain takes its scatter
    /// along and nothing reaches the outliner.
    #[test]
    fn a_run_stores_its_placements_on_the_terrain_in_terrain_space() {
        let mut app = scatter_app();
        spawn_ground(&mut app, "Hill", Vec3::new(1000.0, 0.0, 500.0));

        run_scatter(app.world_mut(), &scatter_params("Hill"));

        assert!(
            groups_of(app.world_mut()).is_empty(),
            "a run spawns no group entity"
        );
        assert_eq!(
            app.world_mut()
                .query::<&ScatterInstance>()
                .iter(app.world())
                .count(),
            0,
            "a run spawns no instance entities"
        );

        let store = app.world().resource::<TerrainDataStore>();
        let data = store.get("Hill.terrain-0.jdterrain").expect("a document");
        assert!(
            data.placement_count() > 0,
            "the run placed nothing to check"
        );
        assert_eq!(
            data.scatter.assets.first().map(|e| e.asset.as_str()),
            Some("kit/Tree.gltf")
        );
        for (coord, _, placement) in data.placements() {
            let local = data.placement_position(coord, placement);
            assert!(
                local.x.abs() < 100.0 && local.z.abs() < 100.0,
                "placements are stored terrain-local, so the terrain's own \
                 translation must not be baked into them: {local:?}"
            );
        }
    }

    /// Two terrains default their stamp to the same key, so a global match
    /// would have a run over one re-stamping the other's group.
    #[test]
    fn find_group_matches_within_one_terrain() {
        let mut app = scatter_app();
        let first = spawn_ground(&mut app, "North", Vec3::ZERO);
        let second = spawn_ground(&mut app, "South", Vec3::new(64.0, 0.0, 0.0));

        let key = "Scatter";
        let one = spawn_group(app.world_mut(), key, first);
        let two = spawn_group(app.world_mut(), key, second);

        assert_eq!(find_group(app.world_mut(), first, key), Some(one));
        assert_eq!(find_group(app.world_mut(), second, key), Some(two));
    }

    /// Adoption moves a group out of the scene and into the terrain's
    /// document: the entities go, and a placement stands where each model
    /// stood.
    #[test]
    fn adopting_a_hand_authored_group_stores_its_models_as_placements() {
        let mut app = scatter_app();
        spawn_ground(&mut app, "Hill", Vec3::new(1000.0, 0.0, 500.0));

        let group = app
            .world_mut()
            .spawn((
                Name::new("Scatter_Trees"),
                Transform::from_xyz(4.0, 0.0, 0.0),
                Visibility::default(),
            ))
            .id();
        let model = app
            .world_mut()
            .spawn((
                Name::new("Tree"),
                GltfSource {
                    path: "kit/Tree.gltf".to_string(),
                    scene_index: 0,
                },
                Transform::from_xyz(2.0, 1.0, -3.0),
                Visibility::default(),
                ChildOf(group),
            ))
            .id();
        app.update();
        let before = app
            .world()
            .get::<GlobalTransform>(model)
            .copied()
            .expect("transform propagation ran");

        adopt_group(app.world_mut(), group, Some("Hill"), None);
        app.update();

        assert!(app.world().get_entity(group).is_err(), "the group is gone");
        assert!(app.world().get_entity(model).is_err(), "the model is gone");

        let store = app.world().resource::<TerrainDataStore>();
        let data = store.get("Hill.terrain-0.jdterrain").expect("a document");
        assert_eq!(data.placement_count(), 1);
        assert_eq!(
            super::scatter_data::group_counts(store, "Hill.terrain-0.jdterrain"),
            vec![("Scatter_Trees".to_string(), 1)]
        );
        let (coord, _, placement) = data.placements().next().expect("one placement");
        assert_eq!(
            data.scatter
                .asset(placement.asset)
                .map(|e| e.asset.as_str()),
            Some("kit/Tree.gltf")
        );
        assert!(
            data.placement_position(coord, placement)
                .abs_diff_eq(before.translation() - Vec3::new(1000.0, 0.0, 500.0), 1e-3),
            "the placement stands where the model stood, in the terrain's space"
        );
    }

    /// Undo puts the group back and takes the placements away again: the
    /// two halves are one entry.
    #[test]
    fn undoing_an_adoption_restores_the_group_and_empties_the_document() {
        let mut app = scatter_app();
        spawn_ground(&mut app, "Hill", Vec3::ZERO);
        let group = app
            .world_mut()
            .spawn((
                Name::new("Scatter_Trees"),
                Transform::default(),
                Visibility::default(),
            ))
            .id();
        app.world_mut().spawn((
            Name::new("Tree"),
            GltfSource {
                path: "kit/Tree.gltf".to_string(),
                scene_index: 0,
            },
            Transform::from_xyz(2.0, 0.0, 3.0),
            Visibility::default(),
            ChildOf(group),
        ));
        app.update();

        adopt_group(app.world_mut(), group, Some("Hill"), None);
        app.update();

        let mut history = app.world_mut().remove_resource::<CommandHistory>().unwrap();
        history.undo(app.world_mut());
        app.world_mut().insert_resource(history);
        app.update();

        assert_eq!(
            app.world()
                .resource::<TerrainDataStore>()
                .get("Hill.terrain-0.jdterrain")
                .expect("a document")
                .placement_count(),
            0
        );
        let restored = app
            .world_mut()
            .query::<(&Name, &GltfSource)>()
            .iter(app.world())
            .count();
        assert_eq!(restored, 1, "the model entity is back");
    }

    /// A stored placement becomes an ordinary model entity, and leaves the
    /// document behind it.
    #[test]
    fn promoting_a_placement_spawns_an_entity_and_takes_it_out_of_the_document() {
        let mut app = scatter_app();
        spawn_ground(&mut app, "Hill", Vec3::ZERO);
        let placements: Vec<_> = (0..3)
            .map(|at| super::scatter_data::PendingPlacement {
                position: Vec3::new(at as f32, 0.0, 2.0),
                yaw: 0.0,
                scale: 1.0,
                asset: 0,
            })
            .collect();
        super::scatter_data::stamp(
            app.world_mut(),
            "Hill.terrain-0.jdterrain",
            "woods",
            &["kit/Tree.gltf".to_string()],
            placements,
        )
        .expect("a document to store into");

        promote_placement(app.world_mut(), Some("Hill"), Some("woods"), 1);
        app.update();

        assert_eq!(
            app.world()
                .resource::<TerrainDataStore>()
                .get("Hill.terrain-0.jdterrain")
                .expect("a document")
                .placement_count(),
            2
        );
        let mut query = app.world_mut().query::<(&GltfSource, &Transform)>();
        let (source, transform) = query.iter(app.world()).next().expect("one model entity");
        assert_eq!(source.path, "kit/Tree.gltf");
        assert_eq!(transform.translation, Vec3::new(1.0, 0.0, 2.0));
    }

    /// Clearing a stored group empties it without touching another.
    #[test]
    fn clearing_a_stored_group_leaves_the_others_standing() {
        let mut app = scatter_app();
        spawn_ground(&mut app, "Hill", Vec3::ZERO);
        let assets = ["kit/Tree.gltf".to_string()];
        for key in ["woods", "meadow"] {
            super::scatter_data::stamp(
                app.world_mut(),
                "Hill.terrain-0.jdterrain",
                key,
                &assets,
                vec![super::scatter_data::PendingPlacement {
                    position: Vec3::new(1.0, 0.0, 1.0),
                    yaw: 0.0,
                    scale: 1.0,
                    asset: 0,
                }],
            )
            .expect("a document to store into");
        }

        clear_scatter(app.world_mut(), Some("Hill"), Some("woods"));

        assert_eq!(
            super::scatter_data::group_counts(
                app.world().resource::<TerrainDataStore>(),
                "Hill.terrain-0.jdterrain"
            ),
            vec![("meadow".to_string(), 1)]
        );
    }
}
