//! Live LOD levels: a [`LodGroup`] whose levels name models keeps in the world
//! only the levels some camera stands near enough to draw. A live level is
//! spawned as bare parts flattened from its model -- a mesh, a material and a
//! transform each -- under the level's node, and is never saved.
//!
//! Bevy's [`VisibilityRange`] still decides what draws, per view, with its
//! dithered cross-fade; this only decides which levels exist. A level wanted
//! but not yet in is covered by the nearest level that is, its range stretched
//! over the gap, so nothing vanishes while the queue catches up.
//!
//! A group that draws nothing yet first gets its least detailed level, which is
//! small and quick to place. The levels the cameras want follow, nearest and
//! largest on screen first, within a time budget a frame. Their files load in
//! the same order: no level file the cameras want is asked for until every
//! group draws something, and the files no camera wants yet come last.
//!
//! A native Bevy mesh-LOD component, picking the level per view in the render
//! world, would make this unnecessary.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bevy::camera::visibility::{RenderLayers, VisibilityRange, VisibilitySystems};
use bevy::gltf::{GltfAssetLabel, GltfMaterialName};
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot};
use jackdaw_scene_types::model_parts::{FlatModel, ModelParts, source_path};
use jackdaw_scene_types::{GltfSource, LodGroup};

use crate::frame_work::FramePace;
use crate::lod::{LodSwitches, level_shows};

/// Places and retires live LOD levels as the cameras move.
pub struct LiveLevelsPlugin;

impl Plugin for LiveLevelsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LiveLevelSettings>()
            .init_resource::<LiveLevelProgress>()
            .init_resource::<LevelQueue>()
            .add_observer(drop_live_levels)
            .add_systems(
                PostUpdate,
                (track_groups, select_live_levels, place_live_levels)
                    .chain()
                    .after(TransformSystems::Propagate)
                    .before(VisibilitySystems::VisibilityPropagate)
                    .before(VisibilitySystems::CalculateBounds),
            );
    }
}

/// How live levels come in.
#[derive(Resource, Clone, Debug)]
pub struct LiveLevelSettings {
    /// What placing levels may cost a frame once every group draws something.
    pub budget: Duration,
    /// What placing levels may cost a frame while some group draws nothing.
    pub opening_budget: Duration,
    /// Whether a group that draws nothing first gets its least detailed level
    /// before the level the cameras want. A game that keeps a loading screen up
    /// until the levels near the player are in turns this off and waits for
    /// [`LiveLevelProgress::is_refined`].
    pub stand_ins: bool,
}

impl Default for LiveLevelSettings {
    fn default() -> Self {
        Self {
            budget: Duration::from_millis(2),
            opening_budget: Duration::from_millis(12),
            stand_ins: true,
        }
    }
}

/// How far live levels have got, for a loading screen or a progress bar.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LiveLevelProgress {
    /// Groups whose levels are kept live.
    pub groups: usize,
    /// Of those, the ones drawing something wherever a camera wants them: the
    /// level it wants, or one standing in for it.
    pub drawn: usize,
    /// Of those, the ones with every level the cameras want in.
    pub refined: usize,
    /// Level models the cameras want that are still loading.
    pub loading: usize,
}

impl LiveLevelProgress {
    /// Whether every group draws something.
    pub fn is_drawn(&self) -> bool {
        self.drawn >= self.groups
    }

    /// Whether every group has the levels the cameras want, with nothing left
    /// to load or place.
    pub fn is_refined(&self) -> bool {
        self.refined >= self.groups && self.loading == 0
    }
}

/// One part of a live LOD level, spawned from the level's model under its node.
/// Derived, never saved.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct LodPart;

#[derive(Clone, Debug, PartialEq)]
enum LevelSource {
    /// A model placed as parts while the level is live.
    Model { path: String, scene_index: usize },
    /// Meshes that are always in the world: authored under the level, or an
    /// instance of its model spawned before the group kept it live.
    Fixed,
    /// A level that never shows, or whose model failed to load.
    Absent,
}

#[derive(Clone, Debug)]
struct LiveLevel {
    node: Entity,
    source: LevelSource,
    parts: Vec<Entity>,
    /// The frame the parts were placed on, while they are in.
    placed_at: Option<u32>,
}

impl LiveLevel {
    fn is_ready(&self) -> bool {
        self.source == LevelSource::Fixed || self.placed_at.is_some()
    }
}

/// Which of a [`LodGroup`]'s levels are live. Derived, never saved.
#[derive(Component, Debug, Default)]
pub struct LiveLevels {
    levels: Vec<LiveLevel>,
    wanted: u32,
}

impl LiveLevels {
    /// The levels some camera wants, one bit each, level 0 lowest.
    pub fn wanted(&self) -> u32 {
        self.wanted
    }

    /// The levels in the world and ready to draw, one bit each.
    pub fn ready(&self) -> u32 {
        bits(self.levels.iter().map(LiveLevel::is_ready))
    }

    /// Whether the group draws something wherever a camera wants it.
    pub fn is_drawn(&self) -> bool {
        self.wanted == 0 || self.ready() != 0
    }

    /// Whether every level the cameras want is in.
    pub fn is_refined(&self) -> bool {
        self.wanted & !self.ready() == 0
    }

    /// The node of each level, in order.
    pub fn nodes(&self) -> impl Iterator<Item = Entity> + '_ {
        self.levels.iter().map(|level| level.node)
    }

    fn model_levels(&self) -> impl Iterator<Item = (usize, &str)> + '_ {
        self.levels
            .iter()
            .enumerate()
            .filter_map(|(index, level)| match &level.source {
                LevelSource::Model { path, .. } => Some((index, path.as_str())),
                _ => None,
            })
    }

    /// The least detailed level placed from a model: what a group that draws
    /// nothing gets first.
    fn coarsest_model(&self) -> Option<(usize, &str)> {
        self.model_levels().last()
    }
}

fn bit(index: usize) -> u32 {
    1u32.checked_shl(index as u32).unwrap_or(0)
}

fn bits(set: impl Iterator<Item = bool>) -> u32 {
    set.enumerate().fold(
        0,
        |bits, (index, on)| if on { bits | bit(index) } else { bits },
    )
}

/// The level standing in for a wanted level that is not in: the nearest
/// ready level, a coarser one first.
fn stand_in(missing: usize, ready: u32, count: usize) -> Option<usize> {
    (missing + 1..count)
        .find(|level| ready & bit(*level) != 0)
        .or_else(|| (0..missing).rev().find(|level| ready & bit(*level) != 0))
}

/// The range each ready level draws over: its own, stretched over the range
/// of every wanted level it stands in for. `None` for a level not in.
pub fn drawn_ranges(
    switches: &[VisibilityRange],
    wanted: u32,
    ready: u32,
) -> Vec<Option<VisibilityRange>> {
    let count = switches.len();
    let mut drawn: Vec<Option<VisibilityRange>> = switches
        .iter()
        .enumerate()
        .map(|(level, range)| (ready & bit(level) != 0).then(|| range.clone()))
        .collect();
    for missing in (0..count).filter(|level| wanted & !ready & bit(*level) != 0) {
        let Some(cover) = stand_in(missing, ready, count) else {
            continue;
        };
        if let Some(range) = drawn[cover].as_mut() {
            let over = &switches[missing];
            if over.start_margin.start < range.start_margin.start {
                range.start_margin = over.start_margin.clone();
            }
            if over.end_margin.end > range.end_margin.end {
                range.end_margin = over.end_margin.clone();
            }
        }
    }
    drawn
}

/// The levels a group needs in: those wanted, and those standing in for a
/// wanted level that is not.
fn needed(wanted: u32, ready: u32, count: usize) -> u32 {
    let mut needed = wanted;
    for missing in (0..count).filter(|level| wanted & !ready & bit(*level) != 0) {
        if let Some(cover) = stand_in(missing, ready, count) {
            needed |= bit(cover);
        }
    }
    needed
}

/// Share of a switch distance a camera may stray past before a level it is
/// leaving goes, and before one it is nearing comes in. The gap between them
/// keeps a camera wobbling at a switch from placing and retiring the same
/// level over and over.
const KEEP_SLACK: f32 = 0.2;
const ADD_SLACK: f32 = 0.1;

/// How far ahead of a moving camera levels are placed, in seconds of travel.
const LEAD: f32 = 0.25;

/// A camera moving faster than this, in metres a second, has jumped rather
/// than flown, and places nothing ahead of itself.
const JUMP_SPEED: f32 = 200.0;

/// Frames a level stays after the one that replaces it is placed, so the new
/// one has drawn before the old one goes.
const SETTLE_FRAMES: u32 = 2;

fn wants(range: &VisibilityRange, distance: f32, keep: bool, lead: f32) -> bool {
    if range.end_margin.end <= range.start_margin.start {
        return false;
    }
    let slack = if keep { KEEP_SLACK } else { ADD_SLACK };
    let near = range.start_margin.start * (1.0 - slack) - lead;
    let far = if range.end_margin.end >= f32::MAX {
        f32::MAX
    } else {
        range.end_margin.end * (1.0 + slack) + lead
    };
    distance >= near && distance <= far
}

struct Job {
    group: Entity,
    level: usize,
    /// Placed for a group that draws nothing, ahead of every other job.
    stand_in: bool,
    /// How much of the screen the group covers; larger goes first.
    share: f32,
}

#[derive(Resource)]
struct LevelQueue {
    /// Waiting levels, the next to place last.
    jobs: Vec<Job>,
    pace: FramePace,
    /// What placing the last batch cost.
    cost: Duration,
    /// Groups that may hold levels nothing needs any more, with the frame
    /// they were queued on.
    retiring: Vec<Entity>,
    frame: u32,
    /// The camera positions the levels were last chosen for.
    viewed: Vec<Vec3>,
    viewed_at: Option<Instant>,
    lead: f32,
    /// Something other than a camera changed what is wanted.
    dirty: bool,
    /// Every level model has been asked for, not only those wanted.
    requested_all: bool,
    /// Wanted level models still loading, as of the last selection.
    loading: usize,
    /// The progress is out of date.
    recount: bool,
}

impl Default for LevelQueue {
    fn default() -> Self {
        Self {
            jobs: Vec::new(),
            pace: FramePace::new(1, 4096),
            cost: Duration::ZERO,
            retiring: Vec::new(),
            frame: 0,
            viewed: Vec::new(),
            viewed_at: None,
            lead: 0.0,
            dirty: true,
            requested_all: false,
            loading: 0,
            recount: true,
        }
    }
}

type ChangedGroups<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static LodGroup,
        &'static Children,
        Option<&'static mut LiveLevels>,
    ),
    Or<(Changed<LodGroup>, Changed<Children>)>,
>;

/// Read which of each changed group's levels are models to keep live, keeping
/// the parts of a level that is still the same model. The group and its level
/// nodes are given a visibility, as a spawned model's root is, so the parts
/// under them inherit one.
fn track_groups(
    mut commands: Commands,
    mut groups: ChangedGroups,
    sources: Query<&GltfSource>,
    instanced: Query<(), With<WorldAssetRoot>>,
    mut models: ResMut<ModelParts>,
    mut queue: ResMut<LevelQueue>,
) {
    for (group, lod, children, live) in &mut groups {
        let levels: Vec<LiveLevel> = children
            .iter()
            .take(lod.levels.len())
            .enumerate()
            .map(|(index, node)| {
                let source = match sources.get(node) {
                    _ if !level_shows(lod, index) => LevelSource::Absent,
                    Ok(_) if instanced.contains(node) => LevelSource::Fixed,
                    Ok(source) => LevelSource::Model {
                        path: source_path(source),
                        scene_index: source.scene_index,
                    },
                    Err(_) => LevelSource::Fixed,
                };
                LiveLevel {
                    node,
                    source,
                    parts: Vec::new(),
                    placed_at: None,
                }
            })
            .collect();
        for node in std::iter::once(group).chain(levels.iter().map(|level| level.node)) {
            commands.entity(node).insert_if_new(Visibility::default());
        }
        let mut fresh = LiveLevels { levels, wanted: 0 };
        if let Some(mut live) = live {
            if live
                .levels
                .iter()
                .zip(&fresh.levels)
                .all(|(old, new)| old.node == new.node && old.source == new.source)
                && live.levels.len() == fresh.levels.len()
            {
                continue;
            }
            for (index, old) in live.levels.drain(..).enumerate() {
                match fresh.levels.get_mut(index) {
                    Some(new) if new.node == old.node && new.source == old.source => {
                        new.parts = old.parts;
                        new.placed_at = old.placed_at;
                    }
                    _ => despawn_parts(&mut commands, &old.parts),
                }
            }
            if let Some((_, path)) = fresh.coarsest_model() {
                models.request(path);
            }
            *live = fresh;
        } else {
            if let Some((_, path)) = fresh.coarsest_model() {
                models.request(path);
            }
            commands.entity(group).insert(fresh);
        }
        queue.dirty = true;
        queue.requested_all = false;
    }
}

fn despawn_parts(commands: &mut Commands, parts: &[Entity]) {
    for part in parts {
        commands.entity(*part).try_despawn();
    }
}

/// Take a group's live levels down with its [`LodGroup`].
fn drop_live_levels(
    removed: On<Remove, LodGroup>,
    mut commands: Commands,
    groups: Query<&LiveLevels>,
) {
    let group = removed.event_target();
    let Ok(live) = groups.get(group) else {
        return;
    };
    for level in &live.levels {
        despawn_parts(&mut commands, &level.parts);
    }
    commands
        .entity(group)
        .try_remove::<(LiveLevels, LodSwitches)>();
}

type Views<'w, 's> = Query<
    'w,
    's,
    (
        &'static Camera,
        &'static GlobalTransform,
        Option<&'static RenderLayers>,
    ),
    With<Camera3d>,
>;

type Groups<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static GlobalTransform,
        &'static LodSwitches,
        &'static mut LiveLevels,
        Option<&'static RenderLayers>,
    ),
>;

/// Choose the levels each group needs from where the cameras stand, and queue
/// the ones not in yet.
fn select_live_levels(
    views: Views,
    mut groups: Groups,
    moved: Query<
        (),
        (
            With<LiveLevels>,
            Or<(Changed<LodSwitches>, Changed<GlobalTransform>)>,
        ),
    >,
    mut models: ResMut<ModelParts>,
    settings: Res<LiveLevelSettings>,
    progress: Res<LiveLevelProgress>,
    mut queue: ResMut<LevelQueue>,
) {
    let default_layers = RenderLayers::default();
    let cameras: Vec<(Vec3, RenderLayers)> = views
        .iter()
        .filter(|(camera, ..)| camera.is_active)
        .map(|(_, transform, layers)| {
            (transform.translation(), layers.cloned().unwrap_or_default())
        })
        .collect();
    let positions: Vec<Vec3> = cameras.iter().map(|(at, _)| *at).collect();
    let camera_moved = positions.len() != queue.viewed.len()
        || positions
            .iter()
            .zip(&queue.viewed)
            .any(|(now, was)| now.distance_squared(*was) > 1e-4);
    let stopped = !camera_moved && queue.lead > 0.0;
    if !camera_moved && !stopped && !queue.dirty && moved.is_empty() && models.settled().is_empty()
    {
        return;
    }
    let now = Instant::now();
    if camera_moved && positions.len() == queue.viewed.len() {
        let travelled = positions
            .iter()
            .zip(&queue.viewed)
            .map(|(now, was)| now.distance(*was))
            .fold(0.0, f32::max);
        let seconds = queue
            .viewed_at
            .map_or(0.0, |at| now.duration_since(at).as_secs_f32());
        let speed = if seconds > 0.0 {
            travelled / seconds
        } else {
            f32::INFINITY
        };
        queue.lead = if speed < JUMP_SPEED {
            speed * LEAD
        } else {
            0.0
        };
    } else {
        queue.lead = 0.0;
    }
    queue.viewed = positions;
    queue.viewed_at = Some(now);
    queue.dirty = false;
    let lead = queue.lead;

    let mut jobs = Vec::new();
    let mut wanted_models: Vec<(f32, String)> = Vec::new();
    let mut stand_in_models: Vec<(f32, String)> = Vec::new();
    let mut loading = 0;
    for (group, transform, switches, mut live, layers) in &mut groups {
        let at = transform.translation();
        let layers = layers.unwrap_or(&default_layers);
        let distances = || {
            cameras
                .iter()
                .filter(|(_, seen)| seen.intersects(layers))
                .map(|(camera, _)| camera.distance(at))
        };
        let ready = live.ready();
        let wanted = bits(live.levels.iter().enumerate().map(|(index, level)| {
            level.source != LevelSource::Absent
                && switches.ranges.get(index).is_some_and(|range| {
                    distances()
                        .any(|distance| wants(range, distance, ready & bit(index) != 0, lead))
                })
        }));
        if live.wanted != wanted {
            live.wanted = wanted;
        }
        let nearest = distances().fold(f32::INFINITY, f32::min);
        let share = switches.size / nearest.max(1e-3);
        let missing = wanted & !ready;
        if ready & !needed(wanted, ready, live.levels.len()) != 0 {
            queue.retiring.push(group);
        }
        if missing == 0 {
            continue;
        }
        if ready == 0 && settings.stand_ins {
            if let Some((level, path)) = live.coarsest_model() {
                match models.get(path) {
                    Some(_) => jobs.push(Job {
                        group,
                        level,
                        stand_in: true,
                        share,
                    }),
                    None if models.is_loading(path) => loading += 1,
                    None if !models.failed(path) => {
                        stand_in_models.push((share, path.to_string()));
                    }
                    None => {}
                }
            }
        } else {
            for (level, path) in live.model_levels() {
                if missing & bit(level) == 0 {
                    continue;
                }
                match models.get(path) {
                    Some(_) => jobs.push(Job {
                        group,
                        level,
                        stand_in: false,
                        share,
                    }),
                    None if models.is_loading(path) => loading += 1,
                    None if !models.failed(path) => {
                        wanted_models.push((share, path.to_string()));
                    }
                    None => {}
                }
            }
        }
    }

    stand_in_models.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (_, path) in &stand_in_models {
        models.request(path);
    }
    let usable = !settings.stand_ins || progress.is_drawn();
    if usable {
        wanted_models.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (_, path) in &wanted_models {
            models.request(path);
        }
    }
    if wanted_models.is_empty()
        && stand_in_models.is_empty()
        && loading == 0
        && jobs.is_empty()
        && !queue.requested_all
    {
        for (.., live, _) in groups.iter() {
            for (_, path) in live.model_levels() {
                models.request(path);
            }
        }
        queue.requested_all = true;
    }
    queue.loading = loading + wanted_models.len() + stand_in_models.len();
    jobs.sort_by(|a, b| {
        a.stand_in
            .cmp(&b.stand_in)
            .then(a.share.total_cmp(&b.share))
    });
    queue.jobs = jobs;
    queue.recount = true;
}

/// Place the next levels in the queue, as many as the budget allows, and retire
/// the levels nothing needs any more.
fn place_live_levels(world: &mut World) {
    world.resource_scope(|world, mut queue: Mut<LevelQueue>| {
        queue.frame = queue.frame.wrapping_add(1);
        let frame = queue.frame;
        let opening = !world.resource::<LiveLevelProgress>().is_drawn();
        let settings = world.resource::<LiveLevelSettings>();
        let budget = if opening {
            settings.opening_budget
        } else {
            settings.budget
        };
        let mut changed = false;
        if !queue.jobs.is_empty() {
            let waiting = queue.jobs.len();
            let cost = queue.cost;
            let count = queue.pace.take(cost, budget, waiting);
            let started = Instant::now();
            for _ in 0..count {
                let Some(job) = queue.jobs.pop() else {
                    break;
                };
                if place(world, &job, frame) {
                    queue.retiring.push(job.group);
                    changed = true;
                }
            }
            queue.cost = started.elapsed();
            if queue.jobs.is_empty() {
                queue.dirty = true;
            }
        }
        if !queue.retiring.is_empty() {
            let retiring = std::mem::take(&mut queue.retiring);
            for group in retiring {
                match retire(world, group, frame) {
                    Retired::Done => changed = true,
                    Retired::Settling => queue.retiring.push(group),
                    Retired::Nothing => {}
                }
            }
            queue.retiring.sort_unstable();
            queue.retiring.dedup();
        }
        if changed || std::mem::take(&mut queue.recount) {
            recount(world, queue.loading);
        }
    });
}

/// Place one queued level as parts, unless something has changed what it is
/// wanted for since it was queued.
fn place(world: &mut World, job: &Job, frame: u32) -> bool {
    let Some(live) = world.get::<LiveLevels>(job.group) else {
        return false;
    };
    let Some(level) = live.levels.get(job.level) else {
        return false;
    };
    let LevelSource::Model { path, scene_index } = &level.source else {
        return false;
    };
    let ready = live.ready();
    let still_wanted = if job.stand_in {
        ready == 0 && live.wanted != 0
    } else {
        live.wanted & bit(job.level) != 0
    };
    if level.is_ready() || !still_wanted {
        return false;
    }
    let node = level.node;
    let scene_index = *scene_index;
    let path = path.clone();
    let Some(model) = world.resource::<ModelParts>().get(&path).cloned() else {
        return false;
    };
    if model.needs_instance {
        return place_instance(world, job, node, path, scene_index, frame);
    }
    let Some(range) = world.get::<LodSwitches>(job.group).and_then(|switches| {
        drawn_ranges(&switches.ranges, live.wanted, ready | bit(job.level))
            .get(job.level)
            .cloned()
            .flatten()
    }) else {
        return false;
    };
    let parts = spawn_parts(world, node, &model, range);
    for part in &parts {
        crate::dress_part(world, *part);
    }
    if let Some(mut live) = world.get_mut::<LiveLevels>(job.group)
        && let Some(level) = live.levels.get_mut(job.level)
    {
        level.parts = parts;
        level.placed_at = Some(frame);
    }
    true
}

/// Spawn a level's parts under `node`, already placed in the world: levels
/// are placed after transforms propagate, so a part has to arrive with its
/// global transform to draw on the frame it lands.
fn spawn_parts(
    world: &mut World,
    node: Entity,
    model: &Arc<FlatModel>,
    range: VisibilityRange,
) -> Vec<Entity> {
    let placed = world
        .get::<GlobalTransform>(node)
        .copied()
        .unwrap_or_default();
    model
        .parts
        .iter()
        .map(|part| {
            let mut spawned = world.spawn((
                LodPart,
                Mesh3d(part.mesh.clone()),
                MeshMaterial3d(part.material.clone()),
                part.local,
                placed.mul_transform(part.local),
                range.clone(),
                ChildOf(node),
            ));
            if let Some(name) = &part.material_name {
                spawned.insert(GltfMaterialName(name.clone()));
            }
            spawned.id()
        })
        .collect()
}

/// Place a level whose model moves as a spawned instance of the model, under
/// one part that comes and goes as the level does.
fn place_instance(
    world: &mut World,
    job: &Job,
    node: Entity,
    path: String,
    scene_index: usize,
    frame: u32,
) -> bool {
    let Some(server) = world.get_resource::<AssetServer>() else {
        return false;
    };
    let scene: Handle<WorldAsset> = server
        .load_builder()
        .with_settings(jackdaw_scene_types::render_assets::model_settings)
        .load(GltfAssetLabel::Scene(scene_index).from_asset(path));
    let placed = world
        .get::<GlobalTransform>(node)
        .copied()
        .unwrap_or_default();
    let part = world
        .spawn((
            LodPart,
            Transform::IDENTITY,
            placed,
            Visibility::default(),
            WorldAssetRoot(scene),
            ChildOf(node),
        ))
        .id();
    if let Some(mut live) = world.get_mut::<LiveLevels>(job.group)
        && let Some(level) = live.levels.get_mut(job.level)
    {
        level.parts = vec![part];
        level.placed_at = Some(frame);
    }
    true
}

enum Retired {
    Done,
    Settling,
    Nothing,
}

/// Take down the levels of `group` that nothing needs, once the levels that
/// replace them have had time to draw.
fn retire(world: &mut World, group: Entity, frame: u32) -> Retired {
    let Some(live) = world.get::<LiveLevels>(group) else {
        return Retired::Nothing;
    };
    let ready = live.ready();
    let needed = needed(live.wanted, ready, live.levels.len());
    let surplus: Vec<usize> = live
        .levels
        .iter()
        .enumerate()
        .filter(|(index, level)| level.placed_at.is_some() && needed & bit(*index) == 0)
        .map(|(index, _)| index)
        .collect();
    if surplus.is_empty() {
        return Retired::Nothing;
    }
    let settled = live.levels.iter().enumerate().all(|(index, level)| {
        needed & bit(index) == 0
            || level
                .placed_at
                .is_none_or(|at| frame.wrapping_sub(at) >= SETTLE_FRAMES)
    });
    if !settled {
        return Retired::Settling;
    }
    let mut parts = Vec::new();
    if let Some(mut live) = world.get_mut::<LiveLevels>(group) {
        for index in surplus {
            let level = &mut live.levels[index];
            parts.append(&mut level.parts);
            level.placed_at = None;
        }
    }
    for part in parts {
        if let Ok(part) = world.get_entity_mut(part) {
            part.despawn();
        }
    }
    Retired::Done
}

fn recount(world: &mut World, loading: usize) {
    let mut groups = world.query::<(&LiveLevels, Has<LodSwitches>)>();
    let viewless = world
        .get_resource::<crate::lod::LodView>()
        .is_none_or(|view| view.half_fov_tan <= 0.0);
    let models = world.resource::<ModelParts>();
    let mut counts = LiveLevelProgress {
        loading,
        ..default()
    };
    for (live, measured) in groups.iter(world) {
        counts.groups += 1;
        let settled =
            viewless || measured || live.model_levels().all(|(_, path)| models.failed(path));
        counts.drawn += usize::from(settled && live.is_drawn());
        counts.refined += usize::from(settled && live.is_refined());
    }
    let mut progress = world.resource_mut::<LiveLevelProgress>();
    if *progress != counts {
        *progress = counts;
    }
}

/// Whether `child`, one of a [`LodGroup`]'s `children`, is one of its levels,
/// whose model the group keeps live rather than spawning as an instance.
pub fn is_level_of(group: &LodGroup, children: &Children, child: Entity) -> bool {
    children
        .iter()
        .take(group.levels.len())
        .any(|level| level == child)
}

/// Whether `entity` is a level of a [`LodGroup`].
pub fn is_lod_level(world: &World, entity: Entity) -> bool {
    let Some(group) = world.get::<ChildOf>(entity).map(ChildOf::parent) else {
        return false;
    };
    match (world.get::<LodGroup>(group), world.get::<Children>(group)) {
        (Some(lod), Some(children)) => is_level_of(lod, children, entity),
        _ => false,
    }
}
