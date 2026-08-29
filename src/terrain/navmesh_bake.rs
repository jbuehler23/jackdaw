//! Baking a navigation mesh from the selected terrain, and drawing the result
//! over the ground.
//!
//! The bake is an explicit action, never live: a terrain is sculpted in long
//! strokes and a bake costs seconds. The artifact records a hash of the world
//! it was baked from, and the options bar reports when the two disagree.
//!
//! The input is the terrain's own full-resolution geometry, taken from stored
//! heights rather than from the clipmap meshes in the viewport, which are built
//! around the camera at whatever level of detail the view called for and would
//! bake the camera's position into the navmesh. Scene meshes join it as more of
//! the same geometry rather than as obstacles: a flat roof at a walkable slope
//! comes back walkable, and a wall comes back as the gap around it.
//!
//! Meshing the ground is the expensive half of assembling a bake's input, a
//! million cells at editor scale, and happens inside the async bake task rather
//! than in the operator. The scene's own meshes are converted on the main
//! thread, since they live behind an asset collection a task cannot hold; that
//! half is proportional to the props in the scene rather than to the terrain.
//! The clock starts at the click, where the status changes, so the time
//! reported is the time waited.
//!
//! The overlay is view state and leaves no history entry. The agent a bake is
//! for is scene data on the terrain and does.

use std::path::PathBuf;
use std::time::Instant;

use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::math::Affine3A;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
use bevy_rerecast::TriMeshFromBevyMesh as _;
use bevy_rerecast::rerecast::{
    Aabb3d, AreaType, ConfigBuilder, DetailNavmesh, HeightfieldBuilder, PolygonNavmesh, TriMesh,
};
use jackdaw_api::prelude::*;
use jackdaw_terrain::navmesh::{self, BakeParams, NavPolygon, NavmeshArtifact};

use super::{TerrainDataStore, TerrainSurface};
use crate::EditorEntity;
use crate::default_style;
use crate::scene_io::SceneFilePath;
use crate::selection::Selection;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<TerrainNavmeshState>()
        .add_systems(
            Update,
            (
                poll_running_bake.run_if(resource_exists::<RunningBake>),
                refresh_staleness,
                sync_navmesh_overlay,
            )
                .chain()
                .run_if(in_state(crate::AppState::Editor)),
        )
        .add_systems(OnExit(crate::AppState::Editor), drop_running_bake);
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TerrainNavmeshBakeOp>()
        .register_operator::<TerrainNavmeshToggleOverlayOp>();
}

// --- State ---

/// The last bake's result and whether the overlay draws.
///
/// One scene's, not the editor's: a bake belongs to the ground it was taken
/// from, so this rides the tab the way [`TerrainDataStore`] does.
///
/// What a bake is *for* is not here. The agent it carries is authored per
/// terrain and saved with the scene as
/// [`jackdaw_scene_types::TerrainNavmesh`], so the next person to open the
/// scene can see which character the navmesh beside it was baked for.
#[derive(Resource)]
pub struct TerrainNavmeshState {
    pub show_overlay: bool,
    pub status: BakeStatus,
    pub baked: Option<BakedNavmesh>,
    /// Whether the world moved out from under [`Self::baked`], and in which
    /// way.
    pub staleness: Staleness,
}

/// Why a bake does not describe the world, if it does not.
///
/// The two ways a bake goes out of date are repaired in different places: an
/// agent scrubbed to a new size is a number on the terrain, and moved ground is
/// out in the scene. They are reported separately so the reader knows which.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Staleness {
    /// The bake still describes the world.
    #[default]
    Fresh,
    /// The terrain is asking for a different agent than it was baked for.
    /// Nothing in the scene has to have moved for this.
    AgentChanged,
    /// The ground, the terrain's shape, or the geometry over it has moved since
    /// the bake.
    SceneMoved,
}

impl Staleness {
    pub fn is_stale(self) -> bool {
        !matches!(self, Self::Fresh)
    }
}

impl Default for TerrainNavmeshState {
    fn default() -> Self {
        Self {
            show_overlay: true,
            status: BakeStatus::Idle,
            baked: None,
            staleness: Staleness::Fresh,
        }
    }
}

/// What the baker is asked for, from what the terrain authored.
///
/// The voxel is not authored: it is derived from the terrain's own cell so a
/// coarse terrain is not voxelized as if it were fine, and it is coarsened
/// again inside the bake to fit the grid budget.
pub(crate) fn bake_params(terrain: &jackdaw_scene_types::Terrain) -> BakeParams {
    BakeParams {
        agent_radius: terrain.navmesh.agent_radius,
        agent_height: terrain.navmesh.agent_height,
        max_slope_degrees: terrain.navmesh.max_slope_degrees,
        ..BakeParams::default()
    }
    .with_terrain_cell(terrain_cell(terrain))
}

/// Edits what a terrain's navmesh is baked for and pushes it to the document.
///
/// A change here is scene data, so it undoes and it saves.
pub(crate) fn commit_navmesh(
    world: &mut World,
    entity: Entity,
    edit: impl FnOnce(&mut jackdaw_scene_types::TerrainNavmesh),
) {
    let Some(mut terrain) = world.get_mut::<jackdaw_scene_types::Terrain>(entity) else {
        return;
    };
    let before = terrain.navmesh.clone();
    edit(&mut terrain.navmesh);
    if terrain.navmesh == before {
        return;
    }
    let terrain = terrain.clone();
    crate::commands::sync_component_to_ast(
        world,
        entity,
        "jackdaw_scene_types::types::Terrain",
        &terrain,
    );
}

/// One tab's navmesh: what a bake produced, and any bake still producing one.
/// Held on the tab while it is inactive; the active tab's halves live in the
/// world as resources.
#[derive(Default)]
pub struct TabNavmesh {
    pub state: TerrainNavmeshState,
    running: Option<RunningBake>,
}

/// Moves the active tab's navmesh out of the world and onto its tab.
pub(crate) fn take_from_world(world: &mut World) -> Option<TabNavmesh> {
    // The drawn overlay belongs to the bake going onto the tab, not to the tab
    // coming in. `sync_navmesh_overlay` builds the incoming one's on the next
    // frame from the state installed with it.
    despawn_overlay(world);
    let state = std::mem::take(&mut *world.get_resource_mut::<TerrainNavmeshState>()?);
    let running = world.remove_resource::<RunningBake>();
    Some(TabNavmesh { state, running })
}

/// Installs a tab's navmesh as the live one. The counterpart of
/// [`take_from_world`], called once its tab is the active one.
pub(crate) fn install_in_world(world: &mut World, tab: TabNavmesh) {
    if !world.contains_resource::<TerrainNavmeshState>() {
        return;
    }
    *world.resource_mut::<TerrainNavmeshState>() = tab.state;
    match tab.running {
        Some(running) => world.insert_resource(running),
        None => {
            world.remove_resource::<RunningBake>();
        }
    }
}

/// Forgets the active scene's navmesh, in the world and in flight.
///
/// A tab holds one scene at a time but not always the same one: File > Open,
/// File > New, and Save & New Scene all replace what is in a tab without
/// switching away from it. Without this the replaced scene's bake stays
/// resident, drawn over the new ground, counted in the bar, and blocking the
/// new scene's own artifact from being read, while reporting itself fresh
/// because the terrain it was measured against is absent to disagree.
///
/// [`take_from_world`] instead moves a bake somewhere it will be wanted again.
pub(crate) fn forget_scene_navmesh(world: &mut World) {
    despawn_overlay(world);
    if let Some(mut state) = world.get_resource_mut::<TerrainNavmeshState>() {
        // Whether the overlay is drawn is the author's preference, not the
        // scene's, so opening a scene does not turn it back on.
        let show_overlay = state.show_overlay;
        *state = TerrainNavmeshState {
            show_overlay,
            ..TerrainNavmeshState::default()
        };
    }
    world.remove_resource::<RunningBake>();
}

/// The last bake's result, kept so the overlay has something to draw and the
/// bar has something to report.
pub struct BakedNavmesh {
    pub artifact: NavmeshArtifact,
    /// Where the artifact landed. `None` when the scene had no path to put one
    /// beside, the one case a bake succeeds without producing a file.
    pub path: Option<PathBuf>,
    /// The scene file this was baked under, or `None` if that scene had never
    /// been saved. An export to any other scene is refused: an artifact names
    /// the ground it came from, not whatever file is being written.
    pub scene: Option<PathBuf>,
    /// How long the bake took, or `None` when this was read off disk rather
    /// than produced here.
    pub seconds: Option<f32>,
}

impl BakedNavmesh {
    /// `Terrain::data_path` of the ground this was baked from.
    ///
    /// Staleness is measured against *this* terrain, not against whatever is
    /// selected: a scene with two terrains would otherwise report a bake of one
    /// as stale the moment the other was clicked on.
    pub fn data_path(&self) -> &str {
        &self.artifact.terrain
    }
}

#[derive(Default, Clone, Debug, PartialEq)]
pub enum BakeStatus {
    #[default]
    Idle,
    Baking,
    Done,
    Failed(String),
}

/// The running bake. Its presence makes a second Bake click a no-op rather than
/// a second bake of the same ground.
#[derive(Resource)]
struct RunningBake {
    handle: Task<Result<NavmeshArtifact, String>>,
    started: Instant,
    /// The scene this bake was started under, and where the finished artifact
    /// should be written. Both are resolved before the bake starts, so the poll
    /// does not reach back into the scene and a bake that outlives a tab switch
    /// lands on its own tab rather than on whichever one is in front.
    scene: Option<PathBuf>,
    destination: Option<PathBuf>,
}

/// Leaving the editor drops any bake in flight.
///
/// The poll only runs in `Editor`, so a bake left running across the exit would
/// keep the resource alive with nothing to retire it, and every later Bake
/// click would be cancelled as a duplicate.
fn drop_running_bake(mut commands: Commands, mut state: ResMut<TerrainNavmeshState>) {
    commands.remove_resource::<RunningBake>();
    if state.status == BakeStatus::Baking {
        state.status = BakeStatus::Idle;
    }
}

// --- Operators ---

/// Bakes a navigation mesh from the selected terrain plus the scene's static
/// meshes.
///
/// `allows_undo = false`: the result is a file beside the scene and a viewport
/// overlay, neither of which is scene state an undo could restore.
#[operator(
    id = "terrain.navmesh.bake",
    label = "Bake Navmesh",
    description = "Bake a navigation mesh from this terrain and the scene's static meshes.",
    is_available = super::ops::has_selected_terrain,
    allows_undo = false
)]
pub(crate) fn terrain_navmesh_bake(
    _: In<OperatorParameters>,
    mut commands: Commands,
    selection: Res<Selection>,
    terrains: Query<(&jackdaw_scene_types::Terrain, &GlobalTransform)>,
    geometry: SceneGeometry,
    store: Res<TerrainDataStore>,
    scene_path: Res<SceneFilePath>,
    running: Option<Res<RunningBake>>,
    mut state: ResMut<TerrainNavmeshState>,
) -> OperatorResult {
    if running.is_some() {
        return OperatorResult::Cancelled;
    }
    let entity = selection.primary()?;
    let (terrain, transform) = terrains.get(entity).ok()?;

    let Some(document) = store.get(&terrain.data_path) else {
        state.status = BakeStatus::Failed("this terrain has no height data yet".to_string());
        return OperatorResult::Cancelled;
    };
    // Refused rather than baked around: a scene mesh whose asset has not
    // arrived would leave a building-shaped hole of walkable ground in a bake
    // that reported success.
    if geometry.still_loading() {
        state.status =
            BakeStatus::Failed("the scene's meshes are still loading -- bake again".to_string());
        return OperatorResult::Cancelled;
    }
    if document.regions.region_count() == 0 {
        state.status = BakeStatus::Failed("this terrain has no regions to bake".to_string());
        return OperatorResult::Cancelled;
    }
    // The options bar cannot author a negative agent, but a scene file can:
    // the agent is reflected component data and BSN text goes in unclamped.
    // Caught here because a bake at these numbers succeeds, the voxel
    // arithmetic saturating rather than failing, and produces an artifact no
    // build will read back.
    let params = bake_params(terrain);
    if !params.is_plausible() {
        state.status = BakeStatus::Failed(
            "this terrain's agent is not a size anything can walk at -- check its \
             radius, height, and slope"
                .to_string(),
        );
        return OperatorResult::Cancelled;
    }

    // Extent and placement come from the cells the terrain holds, so the bake
    // covers whatever ground has been allocated.
    let shape = store.grid_shape(terrain);
    let input = BakeInput {
        regions: document.regions.clone(),
        cell: terrain_cell(terrain),
        origin: shape.origin,
        placement: transform.affine(),
        data_path: terrain.data_path.clone(),
        geometry: geometry.placed(),
        geometry_hashes: geometry_hashes(geometry.placements()),
        params,
    };

    let scene = scene_path.path.as_ref().map(PathBuf::from);
    let destination = scene
        .as_ref()
        .map(|path| path.with_extension(jackdaw_terrain::navmesh::EXTENSION));

    let handle = AsyncComputeTaskPool::get().spawn(async move { bake(input) });
    commands.insert_resource(RunningBake {
        handle,
        started: Instant::now(),
        scene,
        destination,
    });
    state.status = BakeStatus::Baking;
    OperatorResult::Finished
}

/// Shows or hides the baked navmesh over the ground. View state, no history
/// entry.
#[operator(
    id = "terrain.navmesh.toggle_overlay",
    label = "Show Navmesh",
    description = "Draw the baked navigation mesh over the terrain.",
    allows_undo = false
)]
pub(crate) fn terrain_navmesh_toggle_overlay(
    _: In<OperatorParameters>,
    mut state: ResMut<TerrainNavmeshState>,
) -> OperatorResult {
    state.show_overlay = !state.show_overlay;
    OperatorResult::Finished
}

// --- Bake ---

/// The scene meshes a bake rasterizes alongside the terrain.
///
/// One definition, used by the bake and by the staleness check, so the two
/// cannot disagree about what the bake was over.
///
/// These are *geometry*, not obstacles: they are rasterized into the same
/// heightfield the ground is, so a flat roof at a walkable slope comes back
/// walkable and a wall comes back as the gap around it. Left out are a body the
/// simulation moves, a mesh hidden in the viewport, and a trigger volume
/// nothing collides with.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SceneGeometry<'w, 's> {
    meshes: Query<
        'w,
        's,
        (
            Entity,
            &'static GlobalTransform,
            &'static Mesh3d,
            Option<&'static avian3d::prelude::RigidBody>,
            Option<&'static avian3d::prelude::Sensor>,
        ),
        (Without<EditorEntity>, Without<TerrainSurface>),
    >,
    /// Colliders standing on their own, with no mesh to rasterize.
    ///
    /// A shape authored straight onto an entity (a capsule, a box, a collider
    /// built from bounds rather than from geometry) is an obstacle in the
    /// running game and nothing to a bake that reads meshes. Kept as its own
    /// query so the bake can warn about it.
    mesh_less_colliders: Query<
        'w,
        's,
        (
            Entity,
            Option<&'static avian3d::prelude::RigidBody>,
            Option<&'static avian3d::prelude::Sensor>,
        ),
        (
            With<avian3d::prelude::Collider>,
            Without<Mesh3d>,
            Without<EditorEntity>,
            // The ground reaches the bake through its heights rather than
            // through a mesh, so a collider on it is not a gap.
            Without<jackdaw_scene_types::Terrain>,
        ),
    >,
    visibility: Query<'w, 's, (Option<&'static Visibility>, Option<&'static ChildOf>)>,
    names: Query<'w, 's, &'static Name>,
    assets: Res<'w, Assets<Mesh>>,
}

impl SceneGeometry<'_, '_> {
    /// Every mesh that belongs in the bake, with where it stands.
    fn baked(&self) -> impl Iterator<Item = (Entity, Affine3A, &Mesh3d)> + '_ {
        self.meshes
            .iter()
            .filter(|(entity, _, _, body, sensor)| {
                // A body that moves is not ground: baking it would freeze one
                // frame of a simulation into the navmesh. Nothing stands on a
                // trigger volume either. A collider with no body is static,
                // like an unsimulated scene mesh.
                !moves(*body) && sensor.is_none() && !self.hidden(*entity)
            })
            .map(|(entity, transform, handle, _, _)| (entity, transform.affine(), handle))
    }

    /// Whether any mesh in the bake is still waiting on its asset.
    ///
    /// A bake over half a scene decodes and draws, with the missing building as
    /// walkable ground.
    fn still_loading(&self) -> bool {
        self.baked()
            .any(|(_, _, handle)| self.assets.get(handle).is_none())
    }

    /// Where each piece stands and how much of it there is.
    ///
    /// Reads the mesh's counts rather than its vertices, so the staleness check
    /// can run it several times a second against a scene the bake itself takes
    /// seconds over.
    fn placements(&self) -> impl Iterator<Item = (Affine3A, u32, u32)> + '_ {
        self.baked().filter_map(|(_, placement, handle)| {
            let mesh = self.assets.get(handle)?;
            let triangles = match mesh.indices() {
                Some(indices) => indices.len() / 3,
                None => mesh.count_vertices() / 3,
            };
            Some((placement, mesh.count_vertices() as u32, triangles as u32))
        })
    }

    /// Every piece as the baker reads it, in world space.
    ///
    /// Lifted out of the asset collection here because a bake runs off the main
    /// thread and cannot hold one. A mesh the baker cannot read (a strip, a
    /// fan, anything without an index buffer) is named in a warning rather than
    /// dropped silently, since it will be missing from the navmesh.
    fn placed(&self) -> Vec<TriMesh> {
        let mut out = Vec::new();
        for (entity, placement, handle) in self.baked() {
            let Some(mesh) = self.assets.get(handle) else {
                continue;
            };
            match placed_trimesh(mesh, placement) {
                Some(tri) => out.push(tri),
                None => warn!(
                    "{} is not a triangle list with an index buffer, so it is not in the navmesh",
                    self.describe(entity)
                ),
            }
        }
        for (entity, body, sensor) in self.mesh_less_colliders.iter() {
            if moves(body) || sensor.is_some() || self.hidden(entity) {
                continue;
            }
            warn!(
                "{} has a collider but no mesh, so it is not in the navmesh -- \
                 the bake reads geometry, not collision shapes",
                self.describe(entity)
            );
        }
        out
    }

    /// Bevy's visibility rule, walked on demand: hidden wins, visible stops the
    /// walk, inherited asks the parent. Read here rather than off
    /// `InheritedVisibility` so the answer does not depend on where in the
    /// frame the bake was asked for.
    fn hidden(&self, entity: Entity) -> bool {
        let mut at = entity;
        loop {
            let Ok((visibility, parent)) = self.visibility.get(at) else {
                return false;
            };
            match visibility {
                Some(Visibility::Hidden) => return true,
                Some(Visibility::Visible) => return false,
                Some(Visibility::Inherited) | None => {}
            }
            match parent {
                Some(parent) => at = parent.parent(),
                None => return false,
            }
        }
    }

    fn describe(&self, entity: Entity) -> String {
        match self.names.get(entity) {
            Ok(name) => name.to_string(),
            Err(_) => format!("{entity}"),
        }
    }
}

/// One mesh in world space, or `None` if the baker cannot read it.
///
/// The affine is applied to the vertices directly rather than through a
/// `Transform`: a `GlobalTransform` under a non-uniform parent scale carries
/// shear that no translation-rotation-scale can express.
fn placed_trimesh(mesh: &Mesh, placement: Affine3A) -> Option<TriMesh> {
    let mut tri = TriMesh::from_mesh(mesh)?;
    for vertex in &mut tri.vertices {
        *vertex = placement.transform_point3a(*vertex);
    }
    // A mirrored placement turns every face inside out, and a face pointing
    // down is not ground.
    if placement.matrix3.determinant() < 0.0 {
        for triangle in &mut tri.indices {
            std::mem::swap(&mut triangle.y, &mut triangle.z);
        }
    }
    Some(tri)
}

/// One hash per piece of scene geometry.
///
/// Split from [`world_source_hash`] because the two halves are paid for in
/// different places: this one needs the world, the other walks megabytes of
/// heights and runs inside the bake.
fn geometry_hashes(placements: impl Iterator<Item = (Affine3A, u32, u32)>) -> Vec<u64> {
    placements
        .map(|(placement, vertices, triangles)| {
            let mut one = navmesh::SourceHasher::default();
            one.eat_placement(placement, vertices, triangles);
            one.finish()
        })
        .collect()
}

/// Hash of everything this terrain's bake depends on.
///
/// `placement` is the terrain's own transform. A bake is taken in world space,
/// so ground that is moved, turned, or scaled after one leaves every polygon
/// hanging where the ground stood, a change the heights and the shape say
/// nothing about.
///
/// The shape it eats is the cell size and the cells the terrain stores, not a
/// declared rectangle. Both move the ground: a wider cell size rescales it, and
/// allocating a region adds to it.
fn world_source_hash(
    cell: Vec2,
    extent: (u32, u32),
    placement: Affine3A,
    regions: &jackdaw_terrain::TerrainRegions,
    geometry: Vec<u64>,
) -> u64 {
    let mut hasher = navmesh::SourceHasher::default();
    hasher.eat_regions(regions);
    hasher.eat_shape(cell, extent.0.max(extent.1));
    hasher.eat_affine(placement);
    hasher.eat_unordered(geometry);
    hasher.finish()
}

/// Whether an agent of `now` would get the navmesh `baked` describes.
///
/// The voxel is not compared: it is derived from the terrain's own cell and its
/// extent, both of which the source hash covers.
fn params_match(baked: BakeParams, now: BakeParams) -> bool {
    baked.agent_radius == now.agent_radius
        && baked.agent_height == now.agent_height
        && baked.max_slope_degrees == now.max_slope_degrees
}

/// A body that moves is not ground: baking it would freeze one frame of a
/// simulation into the navmesh. A collider with no body is static, like an
/// unsimulated scene mesh.
fn moves(body: Option<&avian3d::prelude::RigidBody>) -> bool {
    matches!(
        body,
        Some(avian3d::prelude::RigidBody::Dynamic | avian3d::prelude::RigidBody::Kinematic)
    )
}

/// World size of one of this terrain's cells: the authored cell size, not one
/// derived from a rectangle.
fn terrain_cell(terrain: &jackdaw_scene_types::Terrain) -> Vec2 {
    Vec2::splat(terrain.cell_size)
}

fn to_trimesh(surface: &jackdaw_terrain::SurfaceMesh, placement: Affine3A) -> TriMesh {
    TriMesh {
        vertices: surface
            .vertices
            .iter()
            .map(|v| placement.transform_point3a((*v).into()))
            .collect(),
        indices: surface
            .triangles
            .iter()
            .map(|t| UVec3::new(t[0], t[1], t[2]))
            .collect(),
        area_types: vec![AreaType::NOT_WALKABLE; surface.triangles.len()],
    }
}

/// Everything a bake needs, lifted out of the world.
///
/// Assembling the triangles is most of what a bake costs at editor scale, a
/// kilometre of ground being a million cells to walk, so the assembly happens
/// inside the bake task. What is gathered on the main thread is only what
/// cannot leave it: the stored ground, the terrain's shape, and the scene's
/// meshes as the baker reads them.
struct BakeInput {
    /// Local-space position of the grid's first vertex.
    origin: Vec2,
    regions: jackdaw_terrain::TerrainRegions,
    cell: Vec2,
    placement: Affine3A,
    data_path: String,
    /// Scene geometry, already in world space.
    geometry: Vec<TriMesh>,
    /// The same geometry as the staleness check counts it.
    geometry_hashes: Vec<u64>,
    params: BakeParams,
}

/// Assembles the input and rasterizes it, off the main thread.
///
/// The voxel is sized against the assembled bounds, not the terrain's. The
/// terrain's footprint is the smaller box in any scene with a prop past its
/// edge, a region outside its span, or a scale on its transform, and the baker
/// rasterizes the box it is handed.
fn bake(input: BakeInput) -> Result<NavmeshArtifact, String> {
    let BakeInput {
        regions,
        cell,
        origin,
        placement,
        data_path: terrain,
        geometry,
        geometry_hashes,
        params,
    } = input;

    let source_hash = world_source_hash(
        cell,
        regions.stored_extent().unwrap_or((0, 0)),
        placement,
        &regions,
        geometry_hashes,
    );
    let surface = navmesh::surface_mesh(&regions, cell, origin);
    let mut assembled = to_trimesh(&surface, placement);
    for piece in geometry {
        assembled.extend(piece);
    }
    rasterize(assembled, params, source_hash, &terrain)
}

/// The whole rasterize-to-polygons pipeline over an assembled input.
fn rasterize(
    mut input: TriMesh,
    params: BakeParams,
    source_hash: u64,
    terrain: &str,
) -> Result<NavmeshArtifact, String> {
    let aabb = input
        .compute_aabb()
        .ok_or_else(|| "nothing to bake".to_string())?;
    let extent = Vec2::new(aabb.max.x - aabb.min.x, aabb.max.z - aabb.min.z);
    let params = params.fit_to_extent(extent);

    let first = match bake_once(&mut input, aabb, params, source_hash, terrain) {
        Ok(artifact) => return Ok(artifact),
        Err(reason) => reason,
    };
    // The baker refuses a contour it cannot triangulate, which is what a voxel
    // too coarse for the ground under it produces. Halving the voxel fixes that
    // at four times the work, so it is a single retry and is not attempted once
    // the grid budget is spent.
    let Some(finer) = params.halved_within_budget(extent) else {
        return Err(first);
    };
    bake_once(&mut input, aabb, finer, source_hash, terrain)
        .map_err(|second| format!("{first} A finer bake failed too: {second}"))
}

fn bake_once(
    input: &mut TriMesh,
    aabb: Aabb3d,
    params: BakeParams,
    source_hash: u64,
    terrain: &str,
) -> Result<NavmeshArtifact, String> {
    let extent = Vec2::new(aabb.max.x - aabb.min.x, aabb.max.z - aabb.min.z);
    let span = params.grid_span(extent).ok_or_else(|| {
        format!(
            "{:.0} x {:.0} units at a {:.3} voxel is more grid than the baker can address -- \
             move whatever is out past the ground, or bake a smaller area",
            extent.x, extent.y, params.cell_size
        )
    })?;
    let config = ConfigBuilder {
        agent_height: params.agent_height,
        agent_radius: params.agent_radius,
        walkable_slope_angle: params.max_slope_degrees.to_radians(),
        // The builder derives both voxel dimensions from the agent radius by
        // division, so a voxel size is expressed as the divisor that produces
        // it. Height is voxelized twice as finely as the plane, which is the
        // baker's recommendation.
        cell_size_fraction: params.agent_radius / params.cell_size,
        cell_height_fraction: 2.0 * params.agent_radius / params.cell_size,
        aabb: Aabb3d {
            min: aabb.min,
            max: aabb.max,
        },
        ..default()
    }
    .build();
    refuse_unbakeable_config(&config, params, span)?;

    input.mark_walkable_triangles(config.walkable_slope_angle);

    let mut heightfield = HeightfieldBuilder {
        aabb: config.aabb,
        cell_size: config.cell_size,
        cell_height: config.cell_height,
    }
    .build()
    .map_err(|err| err.to_string())?;
    heightfield
        .rasterize_triangles(input, config.walkable_climb)
        .map_err(|err| err.to_string())?;
    heightfield.filter_low_hanging_walkable_obstacles(config.walkable_climb);
    heightfield.filter_ledge_spans(config.walkable_height, config.walkable_climb);
    heightfield.filter_walkable_low_height_spans(config.walkable_height);

    let mut compact = heightfield
        .into_compact(config.walkable_height, config.walkable_climb)
        .map_err(|err| err.to_string())?;
    compact.erode_walkable_area(config.walkable_radius);
    compact.build_distance_field();
    // No border ring. `Config` sizes one because its defaults are written for a
    // tiled bake, where the heightfield is padded past the tile and the padding
    // is discarded. This bake is one field over the whole terrain, so a border
    // ring would be carved out of the ground and would make the terrain's
    // perimeter a portal into another region rather than a solid wall.
    // Contouring does not subdivide a portal edge, which collapses the whole
    // walkable surface into one polygon.
    compact
        .build_regions(0, config.min_region_area, config.merge_region_area)
        .map_err(|err| err.to_string())?;

    let contours = compact.build_contours(
        config.max_simplification_error,
        config.max_edge_len,
        config.contour_flags,
    );
    let polygons = contours
        .into_polygon_mesh(config.max_vertices_per_polygon)
        .map_err(|err| err.to_string())?;
    let detail = DetailNavmesh::new(
        &polygons,
        &compact,
        config.detail_sample_dist,
        config.detail_sample_max_error,
    )
    .map_err(|err| err.to_string())?;

    Ok(to_artifact(
        &polygons,
        &detail,
        params,
        source_hash,
        terrain,
    ))
}

/// Refuses a config whose derived voxel counts the baker would not survive.
///
/// The baker takes these as plain `u16` arithmetic with no checks of its own:
/// an edge limit past 255 overflows when squared for the length test, a panic
/// in a debug build and silence in a release one. An agent wider than the
/// ground is caught here too, where it can be named, rather than as an empty
/// bake further down.
fn refuse_unbakeable_config(
    config: &bevy_rerecast::rerecast::Config,
    params: BakeParams,
    span: UVec2,
) -> Result<(), String> {
    let eroded = u32::from(config.walkable_radius) * 2;
    if span.x <= eroded || span.y <= eroded {
        return Err(format!(
            "an agent {:.2} wide needs more ground than this to stand on: \
             it erodes {eroded} of a {} x {} voxel grid",
            params.agent_radius, span.x, span.y
        ));
    }
    if config.max_edge_len > 255 {
        return Err(format!(
            "a {:.2} agent at a {:.3} voxel asks for {}-voxel navmesh edges, \
             past what the baker measures them with -- a larger voxel or a \
             smaller agent bakes",
            params.agent_radius, config.cell_size, config.max_edge_len
        ));
    }
    Ok(())
}

/// Turns the baker's types into the plain vertices and indices the artifact
/// carries, so nothing downstream has to link the baker.
fn to_artifact(
    polygons: &PolygonNavmesh,
    detail: &DetailNavmesh,
    params: BakeParams,
    source_hash: u64,
    terrain: &str,
) -> NavmeshArtifact {
    let origin = polygons.aabb.min;
    let (cell_size, cell_height) = (polygons.cell_size, polygons.cell_height);
    let vertices: Vec<Vec3> = polygons
        .vertices
        .iter()
        .map(|v| {
            Vec3::new(
                origin.x + v.x as f32 * cell_size,
                origin.y + v.y as f32 * cell_height,
                origin.z + v.z as f32 * cell_size,
            )
        })
        .collect();

    let nvp = polygons.max_vertices_per_polygon as usize;
    let corners_of = |index: usize| -> Vec<u32> {
        polygons.polygons[index * nvp..(index + 1) * nvp]
            .iter()
            .take_while(|i| **i != PolygonNavmesh::NO_INDEX)
            .map(|i| u32::from(*i))
            .collect()
    };

    // A polygon the baker left with fewer than three corners is dropped, which
    // renumbers the rest, so the edge links are resolved through a remap rather
    // than carried across as the baker's own indices.
    let mut remap = vec![navmesh::NO_NEIGHBOR; polygons.polygon_count()];
    let mut kept = 0u32;
    for (index, slot) in remap.iter_mut().enumerate() {
        if corners_of(index).len() >= 3 {
            *slot = kept;
            kept += 1;
        }
    }

    let mut out = Vec::with_capacity(kept as usize);
    for index in 0..polygons.polygon_count() {
        let corners = corners_of(index);
        if corners.len() < 3 {
            continue;
        }
        let neighbor_entry = &polygons.polygon_neighbors[index * nvp..(index + 1) * nvp];
        let neighbors = neighbor_entry[..corners.len()]
            .iter()
            .map(|n| {
                // The baker sets the high bit on an edge that leaves the field
                // it built, which is a border here: this artifact is one field,
                // not a tile in a set of them.
                if *n == PolygonNavmesh::NO_CONNECTION || *n & 0x8000 != 0 {
                    return navmesh::NO_NEIGHBOR;
                }
                remap
                    .get(usize::from(*n))
                    .copied()
                    .unwrap_or(navmesh::NO_NEIGHBOR)
            })
            .collect();
        out.push(NavPolygon {
            area: polygons.areas.get(index).map_or(0, |area| **area),
            corners,
            neighbors,
        });
    }

    // Detail triangle indices are local to their sub-mesh; the artifact carries
    // one flat mesh, so they are rebased on the way out.
    let mut surface_triangles = Vec::new();
    for submesh in &detail.meshes {
        let base_vertex = submesh.base_vertex_index;
        let base_triangle = submesh.base_triangle_index as usize;
        let count = submesh.triangle_count as usize;
        for triangle in &detail.triangles[base_triangle..base_triangle + count] {
            surface_triangles.push([
                base_vertex + u32::from(triangle[0]),
                base_vertex + u32::from(triangle[1]),
                base_vertex + u32::from(triangle[2]),
            ]);
        }
    }

    NavmeshArtifact {
        params,
        source_hash,
        terrain: terrain.to_string(),
        vertices,
        polygons: out,
        surface_vertices: detail.vertices.clone(),
        surface_triangles,
    }
}

fn poll_running_bake(
    mut commands: Commands,
    mut running: ResMut<RunningBake>,
    mut state: ResMut<TerrainNavmeshState>,
    terrains: Query<(&jackdaw_scene_types::Terrain, &GlobalTransform)>,
    geometry: SceneGeometry,
    store: Res<TerrainDataStore>,
) {
    let Some(result) = future::block_on(future::poll_once(&mut running.handle)) else {
        return;
    };
    let seconds = running.started.elapsed().as_secs_f32();
    let scene = running.scene.clone();
    let destination = running.destination.clone();
    commands.remove_resource::<RunningBake>();

    let artifact = match result {
        Ok(artifact) => artifact,
        Err(reason) => {
            state.status = BakeStatus::Failed(reason);
            return;
        }
    };

    // A bake that produced nothing is reported rather than written out: an
    // empty artifact on disk reads as a bake with no walkable ground.
    if artifact.is_empty() {
        state.status = BakeStatus::Failed("no walkable ground at these params".to_string());
        state.baked = None;
        return;
    }

    let path = match destination {
        Some(path) => match crate::scene_io::save::write_atomic(&path, &encoded(&artifact)) {
            Ok(()) => Some(path),
            Err(err) => {
                state.status = BakeStatus::Failed(format!("could not write the artifact: {err}"));
                return;
            }
        },
        None => None,
    };

    // Measured against the world rather than assumed fresh: a bake takes
    // seconds and nothing stops the ground being sculpted through them, so a
    // result can be behind by the time it lands.
    state.staleness = staleness_against_world(&artifact, &terrains, &store, &geometry);
    state.status = BakeStatus::Done;
    state.baked = Some(BakedNavmesh {
        artifact,
        path,
        scene,
        seconds: Some(seconds),
    });
}

/// Whether `artifact` still describes the world, and if not, which half of it
/// stopped matching: the agent asked for, or the world itself.
///
/// A terrain absent from the scene counts as fresh, there being nothing to
/// compare against.
///
/// A scene whose meshes have not all arrived counts as fresh too. Geometry that
/// is still loading hashes as geometry that is not there, so answering mid-load
/// would report every props-heavy scene as stale until the last asset landed.
/// The question is asked again [`STALENESS_INTERVAL`] later.
fn staleness_against_world(
    artifact: &NavmeshArtifact,
    terrains: &Query<(&jackdaw_scene_types::Terrain, &GlobalTransform)>,
    store: &TerrainDataStore,
    geometry: &SceneGeometry,
) -> Staleness {
    if geometry.still_loading() {
        return Staleness::Fresh;
    }
    let data_path = artifact.terrain.as_str();
    let Some((terrain, transform)) = terrains.iter().find(|(t, _)| t.data_path == data_path) else {
        return Staleness::Fresh;
    };
    // Asked first and reported on its own: an agent scrubbed to a new size
    // makes the world hash irrelevant, and blaming the scene sends the reader
    // looking for geometry that never moved.
    if !params_match(artifact.params, bake_params(terrain)) {
        return Staleness::AgentChanged;
    }
    let Some(document) = store.get(data_path) else {
        return Staleness::Fresh;
    };
    if world_source_hash(
        terrain_cell(terrain),
        document.regions.stored_extent().unwrap_or((0, 0)),
        transform.affine(),
        &document.regions,
        geometry_hashes(geometry.placements()),
    ) == artifact.source_hash
    {
        Staleness::Fresh
    } else {
        Staleness::SceneMoved
    }
}

/// How often the world is re-hashed to see whether a bake still matches it. A
/// sculpt stroke writes the store every frame it moves, and hashing megabytes
/// of heights at that rate costs more than the answer is worth.
const STALENESS_INTERVAL: f32 = 0.25;

fn refresh_staleness(
    time: Res<Time>,
    mut since: Local<f32>,
    terrains: Query<(&jackdaw_scene_types::Terrain, &GlobalTransform)>,
    geometry: SceneGeometry,
    store: Res<TerrainDataStore>,
    mut state: ResMut<TerrainNavmeshState>,
) {
    let Some(baked) = state.baked.as_ref() else {
        return;
    };
    *since += time.delta_secs();
    if *since < STALENESS_INTERVAL {
        return;
    }
    *since = 0.0;

    let staleness = staleness_against_world(&baked.artifact, &terrains, &store, &geometry);
    if state.staleness != staleness {
        state.staleness = staleness;
    }
}

// --- Overlay ---

/// How far above the baked surface the overlay sits, as a fraction of the
/// terrain's extent. The region grid scales its lift the same way: a line
/// running the length of a large terrain is seen at a grazing angle for most of
/// it, and a constant lift loses the depth test out there.
const LIFT_FRACTION: f32 = 0.002;
/// Floor for that lift, matching the region grid's own.
const MIN_LIFT: f32 = 0.1;

/// The drawn navmesh. One entity, rebuilt whenever the bake behind it changes.
#[derive(Component)]
pub(crate) struct NavmeshOverlay;

/// What the drawn overlay was built from, so a frame that changed nothing does
/// not rebuild a mesh with tens of thousands of triangles in it.
#[derive(PartialEq, Clone, Copy)]
struct OverlayBuild {
    source_hash: u64,
    triangles: usize,
    lift: u32,
}

/// Draws the walkable surface over the ground.
///
/// The surface, not the polygons. A baked navmesh has two levels: the coarse
/// convex polygons a pathfinder walks, and the detail mesh those were
/// simplified from. A hull edge is a straight line in world space between two
/// corners that may be a hundred metres and eight metres of height apart, so on
/// rolling ground it cuts under the hills it spans. The detail triangles follow
/// the ground they were rasterized from.
///
/// A mesh rather than gizmo lines: the detail surface runs to tens of thousands
/// of triangles on a real zone, and their edges would be hundreds of thousands
/// of lines to submit every frame.
fn sync_navmesh_overlay(
    mut commands: Commands,
    state: Res<TerrainNavmeshState>,
    terrains: Query<&jackdaw_scene_types::Terrain>,
    store: Res<TerrainDataStore>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    existing: Query<(Entity, &MeshMaterial3d<StandardMaterial>), With<NavmeshOverlay>>,
    mut built: Local<Option<OverlayBuild>>,
) {
    let baked = state.baked.as_ref().filter(|_| state.show_overlay);
    let Some(baked) = baked else {
        for (entity, _) in &existing {
            commands.entity(entity).despawn();
        }
        *built = None;
        return;
    };

    // Lifted off the ground it describes, by the size of that ground, found by
    // the name in the bake rather than by what is selected. A surface mesh
    // sitting where the terrain sits z-fights it across the whole zone, so the
    // lift must not collapse to the floor when someone clicks away.
    let lift = terrains
        .iter()
        .find(|terrain| terrain.data_path == baked.artifact.terrain)
        .map_or(MIN_LIFT, |terrain| {
            // The lift scales with the footprint, which is however far the
            // stored regions reach rather than a declared rectangle.
            (store.grid_shape(terrain).size.max_element() * LIFT_FRACTION).max(MIN_LIFT)
        });
    // Amber once the bake stops describing the world, matching the bar's
    // summary.
    let color = if state.staleness.is_stale() {
        default_style::TERRAIN_NAVMESH_STALE
    } else {
        default_style::TERRAIN_NAVMESH_OVERLAY
    };

    let want = OverlayBuild {
        source_hash: baked.artifact.source_hash,
        triangles: baked.artifact.surface_triangles.len(),
        lift: lift.to_bits(),
    };
    if *built == Some(want)
        && let Some((_, material)) = existing.iter().next()
    {
        // Same surface, so only the tint can have moved, and it moves only when
        // staleness flips. Read first because `get_mut` raises
        // `AssetEvent::Modified` whether or not the value changes, which would
        // re-prepare the material every frame the overlay is drawn.
        let tinted = materials
            .get(&material.0)
            .is_some_and(|material| material.base_color == color);
        if !tinted && let Some(mut material) = materials.get_mut(&material.0) {
            material.base_color = color;
        }
        return;
    }

    for (entity, _) in &existing {
        commands.entity(entity).despawn();
    }
    let Some(mesh) = surface_mesh(&baked.artifact, lift) else {
        *built = None;
        return;
    };
    commands.spawn((
        Mesh3d(meshes.add(mesh)),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: color,
            unlit: true,
            double_sided: true,
            cull_mode: None,
            alpha_mode: AlphaMode::Blend,
            ..default()
        })),
        NotShadowCaster,
        NotShadowReceiver,
        NavmeshOverlay,
        // `SceneGeometry` is `Without<EditorEntity>`, which keeps the drawing
        // of a bake out of the next one's input and out of the hash that
        // decides whether it is stale.
        EditorEntity,
    ));
    *built = Some(want);
}

/// The artifact's detail surface as a mesh, lifted clear of the ground.
///
/// `None` when the bake produced no surface: an artifact carrying polygons and
/// nothing else, or a bake over nothing.
fn surface_mesh(artifact: &NavmeshArtifact, lift: f32) -> Option<Mesh> {
    if artifact.surface_triangles.is_empty() || artifact.surface_vertices.is_empty() {
        return None;
    }
    let up = Vec3::Y * lift;
    let positions: Vec<[f32; 3]> = artifact
        .surface_vertices
        .iter()
        .map(|v| (*v + up).to_array())
        .collect();
    let mut indices = Vec::with_capacity(artifact.surface_triangles.len() * 3);
    for triangle in &artifact.surface_triangles {
        if triangle
            .iter()
            .any(|i| *i as usize >= artifact.surface_vertices.len())
        {
            continue;
        }
        indices.extend_from_slice(triangle);
    }
    if indices.is_empty() {
        return None;
    }
    let mut mesh = Mesh::new(
        bevy::render::mesh::PrimitiveTopology::TriangleList,
        bevy::asset::RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_indices(bevy::render::mesh::Indices::U32(indices));
    Some(mesh)
}

/// Drops the drawn overlay along with the bake behind it.
///
/// The mesh is a live entity, not something a resource reset reaches, so a
/// scene leaving the tab has to take it down explicitly or its navmesh keeps
/// drawing over the incoming scene's ground.
pub(crate) fn despawn_overlay(world: &mut World) {
    let drawn: Vec<Entity> = world
        .query_filtered::<Entity, With<NavmeshOverlay>>()
        .iter(world)
        .collect();
    for entity in drawn {
        world.despawn(entity);
    }
}

/// Re-writes the baked artifact beside the scene at `scene_path`.
///
/// A bake writes its own file when it finishes, so this covers a scene that has
/// since moved: a first save after a bake, where the ground was baked before it
/// had a file to sit beside. An artifact already in place and a scene never
/// baked are both no-ops, and a write failure warns rather than failing the
/// scene save, the artifact being derived data a re-bake reproduces.
///
/// Saving under a *different* name is refused. The bake names the scene it was
/// taken from, and a navmesh copied onto a file it was not baked from is the
/// one failure this format cannot detect on the way back in: the bytes decode
/// and the polygons are real, but they describe other ground. A re-bake under
/// the new name writes one that belongs there.
pub(crate) fn export_beside_scene(world: &mut World, scene_path: &str) {
    let scene = PathBuf::from(scene_path);
    let destination = scene.with_extension(navmesh::EXTENSION);
    let Some(state) = world.get_resource::<TerrainNavmeshState>() else {
        return;
    };
    let Some(baked) = state.baked.as_ref() else {
        return;
    };
    if let Some(baked_scene) = baked.scene.as_ref()
        && baked_scene != &scene
    {
        let from = baked_scene
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        // Reported and left alone. The bake is valid and still draws; only this
        // destination is wrong, and writing that into the bake's status would
        // make a good navmesh read as a failed one until someone baked again.
        warn!(
            "refused to write the baked navmesh beside {scene_path}: baked from {from}, \
             not this scene -- bake again to write one here"
        );
        return;
    }
    if baked.path.as_deref() == Some(destination.as_path()) {
        return;
    }
    let bytes = encoded(&baked.artifact);
    match crate::scene_io::save::write_atomic(&destination, &bytes) {
        Ok(()) => {
            if let Some(baked) = world.resource_mut::<TerrainNavmeshState>().baked.as_mut() {
                baked.path = Some(destination);
                baked.scene = Some(scene);
            }
        }
        Err(err) => warn!("could not write the baked navmesh beside the scene: {err}"),
    }
}

/// The artifact's bytes, checked in a debug build against what reading them
/// back gives.
///
/// A bake produces shapes a hand-built test sample does not: a polygon the
/// baker left short, a neighbour index the remap did not cover.
fn encoded(artifact: &NavmeshArtifact) -> Vec<u8> {
    let bytes = navmesh::encode(artifact);
    debug_assert_eq!(
        navmesh::decode(&bytes).as_ref(),
        Ok(artifact),
        "a freshly baked navmesh has to read back as itself"
    );
    bytes
}

/// One-line summary of the navmesh this terrain has, for the options bar.
///
/// "Not baked yet" means there is no navmesh, on disk or in memory: an artifact
/// beside the scene is read at open, so an absent line is an absent navmesh
/// rather than one nobody has looked for.
pub(super) fn summary(state: &TerrainNavmeshState) -> String {
    match &state.status {
        BakeStatus::Idle => "Not baked yet".to_string(),
        BakeStatus::Baking => "Baking...".to_string(),
        BakeStatus::Failed(reason) => format!("Bake failed: {reason}"),
        BakeStatus::Done => {
            let Some(baked) = state.baked.as_ref() else {
                return "Not baked yet".to_string();
            };
            let count = baked.artifact.polygons.len();
            let polygons = if count == 1 {
                "1 polygon".to_string()
            } else {
                format!("{count} polygons")
            };
            let where_to = match (
                baked.seconds,
                baked.path.as_ref().and_then(|p| p.file_name()),
            ) {
                (Some(seconds), Some(name)) => {
                    format!(" in {seconds:.1}s to {}", name.to_string_lossy())
                }
                (Some(seconds), None) => {
                    format!(" in {seconds:.1}s (save the scene to write it out)")
                }
                (None, Some(name)) => format!(" from {}", name.to_string_lossy()),
                (None, None) => String::new(),
            };
            // Named by cause: a scrubbed agent and moved ground are fixed in
            // different places.
            let stale = match state.staleness {
                Staleness::Fresh => "",
                Staleness::AgentChanged => " -- out of date, the agent changed since the bake",
                Staleness::SceneMoved => " -- out of date, the scene moved since",
            };
            format!("{polygons}{where_to}{stale}")
        }
    }
}

/// Reads the navmesh beside `scene_path` into the tab, if there is one and the
/// tab has none of its own.
///
/// A scene that was baked and saved comes back with its navmesh drawn and
/// measured against the ground it opens onto, rather than reading as unbaked.
/// An artifact this build cannot read is left on disk and reported; overwriting
/// it with a fresh bake is the author's decision, not the loader's.
pub(crate) fn import_beside_scene(world: &mut World, scene_path: &str) {
    let Some(state) = world.get_resource::<TerrainNavmeshState>() else {
        return;
    };
    // An in-memory bake is newer than anything on disk, and a reactivated tab
    // brings its own back with it.
    if state.baked.is_some() {
        return;
    }
    let scene = PathBuf::from(scene_path);
    let path = scene.with_extension(navmesh::EXTENSION);
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    let artifact = match navmesh::decode(&bytes) {
        Ok(artifact) => artifact,
        Err(err) => {
            warn!("{} is not readable: {err}", path.display());
            world.resource_mut::<TerrainNavmeshState>().status =
                BakeStatus::Failed(format!("{err} -- bake again to replace it"));
            return;
        }
    };

    let staleness = world
        .run_system_cached_with(staleness_of_loaded_world, artifact.clone())
        .unwrap_or(Staleness::Fresh);
    let mut state = world.resource_mut::<TerrainNavmeshState>();
    state.staleness = staleness;
    state.status = BakeStatus::Done;
    state.baked = Some(BakedNavmesh {
        artifact,
        path: Some(path),
        scene: Some(scene),
        seconds: None,
    });
}

/// [`staleness_against_world`] as a one-shot system, so the loader can ask it
/// from an exclusive context.
fn staleness_of_loaded_world(
    artifact: In<NavmeshArtifact>,
    terrains: Query<(&jackdaw_scene_types::Terrain, &GlobalTransform)>,
    store: Res<TerrainDataStore>,
    geometry: SceneGeometry,
) -> Staleness {
    staleness_against_world(&artifact, &terrains, &store, &geometry)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The data path every test terrain is keyed by.
    fn ground() -> String {
        "zone.terrain-0.jdterrain".to_string()
    }
    use jackdaw_terrain::region::{RegionCoord, RegionSize, TerrainRegions};

    fn flat_input(side: u32, cell: f32) -> TriMesh {
        let mut regions = TerrainRegions::new(RegionSize::new(side).expect("power of two"));
        regions.ensure_region(RegionCoord::ORIGIN);
        let extent = Vec2::splat(cell * (side - 1) as f32);
        let surface = navmesh::surface_mesh(&regions, Vec2::splat(cell), -extent / 2.0);
        to_trimesh(&surface, Affine3A::IDENTITY)
    }

    #[test]
    fn a_flat_terrain_bakes_to_walkable_polygons() {
        let params = BakeParams::default().with_terrain_cell(Vec2::splat(1.0));
        let artifact =
            rasterize(flat_input(32, 1.0), params, 7, &ground()).expect("a flat plane is bakeable");
        assert!(!artifact.is_empty(), "flat ground has to be walkable");
        assert!(!artifact.vertices.is_empty());
        assert_eq!(artifact.source_hash, 7);
        assert_eq!(artifact.params, params);
        for polygon in &artifact.polygons {
            assert!(polygon.corners.len() >= 3);
            assert_eq!(polygon.corners.len(), polygon.neighbors.len());
            for corner in &polygon.corners {
                assert!((*corner as usize) < artifact.vertices.len());
            }
        }

        // The navmesh stands over the ground it was baked from. Contouring
        // offsets the polygon origin by whatever border the regions were built
        // with, so a navmesh a couple of metres off the terrain means a border
        // ring was sized.
        let half = 31.0 / 2.0 + params.cell_size;
        for vertex in &artifact.vertices {
            assert!(
                vertex.x.abs() <= half && vertex.z.abs() <= half,
                "{vertex} is off the terrain it was baked from"
            );
        }
    }

    /// A kilometre of rolling, noisy ground, as the add-terrain operator makes.
    fn rolling_kilometre(resolution: u32) -> (TerrainRegions, Vec2, Vec2) {
        use jackdaw_terrain::{GenerateSettings, generate_heightmap};

        let extent = Vec2::splat(1024.0);
        let mut regions = TerrainRegions::new(RegionSize::new(resolution).expect("power of two"));
        regions.ensure_region(RegionCoord::ORIGIN);
        regions.write_grid_heights(
            resolution,
            &generate_heightmap(resolution, &GenerateSettings::default()),
        );
        (regions, extent, extent / (resolution - 1) as f32)
    }

    /// The bake has to come back describing the ground, and the polygon and
    /// vertex counts are what say whether it did.
    ///
    /// A bake whose walkable contour borders the baker's border region rather
    /// than open space comes back as one four-cornered polygon spanning the
    /// whole kilometre, since contouring does not subdivide an edge onto
    /// another region. That artifact is not empty, so only the counts catch it.
    #[test]
    fn a_kilometre_of_rolling_ground_bakes_to_a_navmesh_that_describes_it() {
        let (regions, extent, cell) = rolling_kilometre(256);
        let surface = navmesh::surface_mesh(&regions, cell, -extent / 2.0);
        let params = BakeParams::default().with_terrain_cell(cell);
        let artifact = rasterize(
            to_trimesh(&surface, Affine3A::IDENTITY),
            params,
            0,
            &ground(),
        )
        .expect("rolling ground is bakeable");

        // The perimeter is a solid wall, so contouring splits it every
        // `max_edge_len` voxels: a few hundred vertices around four kilometres
        // of edge, and a polygon mesh over them rather than one rectangle.
        assert!(
            artifact.polygons.len() > 100,
            "a kilometre of ground came back as {} polygons",
            artifact.polygons.len()
        );
        assert!(
            artifact.vertices.len() > 200,
            "{} vertices is a bounding box, not a boundary",
            artifact.vertices.len()
        );
        for polygon in &artifact.polygons {
            assert!((3..=6).contains(&polygon.corners.len()));
            assert_eq!(polygon.corners.len(), polygon.neighbors.len());
        }
        assert!(
            artifact.polygons.iter().any(|p| p
                .neighbors
                .iter()
                .any(|n| *n != jackdaw_terrain::navmesh::NO_NEIGHBOR)),
            "polygons that never reach each other are not walkable"
        );

        // The detail mesh is where the height lives, capped at 127 vertices per
        // polygon, so a bake that carries the ground's shape spreads it across
        // many polygons rather than one.
        assert!(
            artifact.surface_triangles.len() > 10 * artifact.polygons.len(),
            "{} detail triangles over {} polygons is not a surface",
            artifact.surface_triangles.len(),
            artifact.polygons.len()
        );

        let baked_span = height_span(&artifact.surface_vertices);
        let ground_span = height_span(&surface.vertices);
        assert!(
            baked_span > ground_span * 0.8,
            "the navmesh spans {baked_span:.2} of the ground's {ground_span:.2} of relief"
        );
    }

    fn height_span(vertices: &[Vec3]) -> f32 {
        let (low, high) = vertices.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| {
            (lo.min(v.y), hi.max(v.y))
        });
        high - low
    }

    /// One triangle, far enough out to dwarf the ground it is added to.
    fn distant_triangle(at: f32) -> TriMesh {
        TriMesh {
            vertices: vec![
                Vec3::new(at, 0.0, at).into(),
                Vec3::new(at + 1.0, 0.0, at).into(),
                Vec3::new(at, 0.0, at + 1.0).into(),
            ],
            indices: vec![UVec3::new(0, 1, 2)],
            area_types: vec![AreaType::NOT_WALKABLE],
        }
    }

    /// The grid is spent on the box the baker is handed, and one prop out past
    /// the terrain makes that box the larger one. The voxel grows to cover it,
    /// not the grid.
    #[test]
    fn a_prop_out_past_the_ground_coarsens_the_voxel_rather_than_the_grid() {
        let params = BakeParams::default().with_terrain_cell(Vec2::splat(1.0));
        let near =
            rasterize(flat_input(32, 1.0), params, 0, &ground()).expect("a flat plane is bakeable");
        assert_eq!(near.params.cell_size, params.cell_size);

        let mut input = flat_input(32, 1.0);
        input.extend(distant_triangle(3_000.0));
        let far = rasterize(input, params, 0, &ground()).expect("a distant prop is still bakeable");
        assert!(
            far.params.cell_size > near.params.cell_size * 4.0,
            "the voxel has to pay for the wider box: {} vs {}",
            far.params.cell_size,
            near.params.cell_size
        );
        let extent = Vec2::splat(3_001.0);
        assert!(far.params.grid_span(extent).is_some(), "and still fit");
    }

    #[test]
    fn a_bake_the_baker_could_not_address_is_refused_rather_than_run() {
        let mut input = flat_input(32, 1.0);
        input.extend(distant_triangle(f32::INFINITY));
        let reason = rasterize(input, BakeParams::default(), 0, &ground())
            .expect_err("an unbounded box has no grid behind it");
        assert!(
            reason.contains("address"),
            "the refusal has to say what it could not do: {reason}"
        );
    }

    #[test]
    fn an_agent_wider_than_the_ground_is_refused_rather_than_baked() {
        let params = BakeParams {
            agent_radius: 30.0,
            ..BakeParams::default()
        }
        .with_terrain_cell(Vec2::splat(1.0));
        let reason = rasterize(flat_input(32, 1.0), params, 0, &ground())
            .expect_err("nothing that wide stands on this");
        assert!(
            reason.contains("stand on"),
            "the refusal has to say the agent is the problem: {reason}"
        );
    }

    #[test]
    fn a_bake_round_trips_through_the_artifact_bytes() {
        let params = BakeParams::default().with_terrain_cell(Vec2::splat(1.0));
        let artifact = rasterize(flat_input(32, 1.0), params, 11, &ground())
            .expect("a flat plane is bakeable");
        let decoded = navmesh::decode(&navmesh::encode(&artifact)).expect("a fresh bake decodes");
        assert_eq!(decoded, artifact);
    }

    #[test]
    fn an_empty_input_is_refused_rather_than_baked() {
        let reason = rasterize(TriMesh::default(), BakeParams::default(), 0, &ground())
            .expect_err("there is no ground in an empty mesh");
        assert!(
            reason.contains("nothing to bake"),
            "a refusal has to say what was missing: {reason}"
        );
    }

    /// A world with one selected terrain of `side` cells, one region, and
    /// a scene path in `dir` for the artifact to land beside.
    fn world_with_terrain(dir: &std::path::Path) -> World {
        use jackdaw_terrain::sidecar::RegionTerrainData;

        let side = 64u32;
        let mut app = App::new();
        app.add_plugins((bevy::app::TaskPoolPlugin::default(), AssetPlugin::default()));
        app.init_asset::<Mesh>();
        app.init_asset::<StandardMaterial>();
        app.init_resource::<Selection>();
        app.init_resource::<TerrainDataStore>();
        app.init_resource::<TerrainNavmeshState>();
        app.insert_resource(SceneFilePath {
            path: Some(dir.join("zone.bsn").to_string_lossy().into_owned()),
            ..default()
        });

        let data_path = "zone.terrain-0.jdterrain";
        let mut document = RegionTerrainData {
            regions: TerrainRegions::new(RegionSize::new(side).expect("power of two")),
            ..default()
        };
        document.regions.ensure_region(RegionCoord::ORIGIN);
        app.world_mut()
            .resource_mut::<TerrainDataStore>()
            .insert(data_path, document);

        let entity = app
            .world_mut()
            .spawn((
                jackdaw_scene_types::Terrain {
                    resolution: side,
                    size: Vec2::splat(side as f32),
                    data_path: data_path.to_string(),
                    ..default()
                },
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        app.world_mut().resource_mut::<Selection>().entities = vec![entity];
        std::mem::take(app.world_mut())
    }

    fn no_params() -> OperatorParameters {
        OperatorParameters(std::collections::BTreeMap::new())
    }

    /// Drives the bake through the same operator and polling system the editor
    /// runs, and waits for the background bake to land.
    fn run_bake_to_completion(world: &mut World) {
        let started = world
            .run_system_cached_with(terrain_navmesh_bake, no_params())
            .expect("the bake operator runs");
        assert_eq!(started, OperatorResult::Finished);
        assert_eq!(
            world.resource::<TerrainNavmeshState>().status,
            BakeStatus::Baking
        );

        for _ in 0..2_000 {
            if world.contains_resource::<RunningBake>() {
                world
                    .run_system_cached(poll_running_bake)
                    .expect("the poll system runs");
                world.flush();
            }
            if world.resource::<TerrainNavmeshState>().status != BakeStatus::Baking {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the bake never finished");
    }

    #[test]
    fn baking_a_terrain_lands_a_navmesh_and_its_artifact_on_disk() {
        let dir = std::env::temp_dir().join(format!("jd_bake_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a place to write the artifact");
        let mut world = world_with_terrain(&dir);

        run_bake_to_completion(&mut world);

        let state = world.resource::<TerrainNavmeshState>();
        assert_eq!(state.status, BakeStatus::Done, "{}", summary(state));
        assert_eq!(
            state.staleness,
            Staleness::Fresh,
            "a bake is never stale the moment it lands"
        );
        let baked = state.baked.as_ref().expect("a finished bake has a result");
        assert!(
            !baked.artifact.is_empty(),
            "flat ground has to come back walkable"
        );

        let artifact_path = baked
            .path
            .clone()
            .expect("the scene had a path to write beside");
        assert_eq!(artifact_path, dir.join("zone.jdnav"));
        let bytes = std::fs::read(&artifact_path).expect("the artifact reached disk");
        let decoded = navmesh::decode(&bytes).expect("the artifact on disk decodes");
        assert_eq!(&decoded, &baked.artifact);

        let _ = std::fs::remove_file(&artifact_path);
    }

    #[test]
    fn a_sculpt_after_a_bake_marks_it_out_of_date() {
        let dir = std::env::temp_dir().join(format!("jd_stale_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a place to write the artifact");
        let mut world = world_with_terrain(&dir);
        run_bake_to_completion(&mut world);
        assert_eq!(
            world.resource::<TerrainNavmeshState>().status,
            BakeStatus::Done
        );

        world.insert_resource(Time::<()>::default());
        world
            .resource_mut::<Time<()>>()
            .advance_by(std::time::Duration::from_secs(1));

        let data_path = "zone.terrain-0.jdterrain".to_string();
        world
            .resource_mut::<TerrainDataStore>()
            .entry_for(&jackdaw_scene_types::Terrain {
                resolution: 64,
                size: Vec2::splat(64.0),
                data_path: data_path.clone(),
                ..default()
            })
            .expect("the document is loaded")
            .heights_mut()[100] = 9.0;

        world
            .run_system_cached(refresh_staleness)
            .expect("the staleness check runs");
        assert_eq!(
            world.resource::<TerrainNavmeshState>().staleness,
            Staleness::SceneMoved,
            "moving the heights has to show up as out of date"
        );
        assert!(summary(world.resource::<TerrainNavmeshState>()).contains("the scene moved since"));

        let _ = std::fs::remove_file(dir.join("zone.jdnav"));
    }

    /// Bakes, runs the staleness check once against whatever `change` did to
    /// the world, and reports what it decided.
    fn stale_after(name: &str, change: impl FnOnce(&mut World)) -> Staleness {
        let dir = std::env::temp_dir().join(format!("jd_{name}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a place to write the artifact");
        let mut world = world_with_terrain(&dir);
        run_bake_to_completion(&mut world);
        assert_eq!(
            world.resource::<TerrainNavmeshState>().status,
            BakeStatus::Done,
            "{}",
            summary(world.resource::<TerrainNavmeshState>())
        );
        assert_eq!(
            world.resource::<TerrainNavmeshState>().staleness,
            Staleness::Fresh
        );

        change(&mut world);
        world.flush();
        world.insert_resource(Time::<()>::default());
        world
            .resource_mut::<Time<()>>()
            .advance_by(std::time::Duration::from_secs(1));
        world
            .run_system_cached(refresh_staleness)
            .expect("the staleness check runs");

        let _ = std::fs::remove_file(dir.join("zone.jdnav"));
        world.resource::<TerrainNavmeshState>().staleness
    }

    /// Applying a quantization with no height step resizes a terrain and leaves
    /// every stored height byte-identical. The navmesh under it is stale all
    /// the same.
    #[test]
    fn a_wider_cell_size_marks_a_bake_out_of_date() {
        assert_eq!(
            Staleness::SceneMoved,
            stale_after("cellsize", |world| {
                let mut terrains = world.query::<&mut jackdaw_scene_types::Terrain>();
                for mut terrain in terrains.iter_mut(world) {
                    // The same heights spread over twice the ground puts every
                    // polygon the bake laid in the wrong place.
                    terrain.cell_size *= 2.0;
                }
            })
        );
    }

    /// Sculpting past the edge brings a region into being, ground the navmesh
    /// does not cover.
    #[test]
    fn allocating_a_region_marks_a_bake_out_of_date() {
        assert_eq!(
            Staleness::SceneMoved,
            stale_after("allocate", |world| {
                let subjects: Vec<jackdaw_scene_types::Terrain> = {
                    let mut terrains = world.query::<&jackdaw_scene_types::Terrain>();
                    terrains.iter(world).cloned().collect()
                };
                let mut store = world.resource_mut::<TerrainDataStore>();
                for terrain in &subjects {
                    let reach = store.grid_shape(terrain).resolution;
                    store
                        .entry_for(terrain)
                        .expect("keyed")
                        .ensure_extent(reach * 2)
                        .expect("inside the region cap");
                }
            })
        );
    }

    /// The agent is authored on the terrain, so asking for a different one is a
    /// scene edit and the navmesh under it is stale.
    #[test]
    fn asking_for_a_different_agent_marks_a_bake_out_of_date() {
        let set_radius = |radius: f32| {
            move |world: &mut World| {
                let mut terrains = world.query::<&mut jackdaw_scene_types::Terrain>();
                for mut terrain in terrains.iter_mut(world) {
                    terrain.navmesh.agent_radius = radius;
                }
            }
        };
        assert_eq!(
            Staleness::AgentChanged,
            stale_after("agent", set_radius(1.5))
        );
        assert_eq!(
            Staleness::Fresh,
            stale_after(
                "same_agent",
                set_radius(jackdaw_scene_types::TerrainNavmesh::default().agent_radius)
            )
        );
    }

    /// Scrubbing the slope moves no scene geometry, so the bar names the agent
    /// rather than the scene.
    #[test]
    fn scrubbing_the_slope_blames_the_agent_and_not_the_scene() {
        let dir = std::env::temp_dir().join(format!("jd_slope_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a place to write the artifact");
        let mut world = world_with_terrain(&dir);
        run_bake_to_completion(&mut world);

        let mut terrains = world.query::<&mut jackdaw_scene_types::Terrain>();
        for mut terrain in terrains.iter_mut(&mut world) {
            terrain.navmesh.max_slope_degrees = 48.25;
        }
        world.insert_resource(Time::<()>::default());
        world
            .resource_mut::<Time<()>>()
            .advance_by(std::time::Duration::from_secs(1));
        world
            .run_system_cached(refresh_staleness)
            .expect("the staleness check runs");

        let state = world.resource::<TerrainNavmeshState>();
        assert_eq!(state.staleness, Staleness::AgentChanged);
        let summary = summary(state);
        assert!(
            summary.contains("the agent changed since the bake"),
            "{summary}"
        );
        assert!(
            !summary.contains("the scene moved"),
            "nothing in the scene moved: {summary}"
        );

        let _ = std::fs::remove_file(dir.join("zone.jdnav"));
    }

    /// Scene meshes are baked as ground, so moving one is a change to the
    /// navmesh even though no height moved.
    #[test]
    fn moving_a_scene_mesh_marks_a_bake_out_of_date() {
        let spawn_prop = |world: &mut World| -> Entity {
            let mesh = world
                .resource_mut::<Assets<Mesh>>()
                .add(Mesh::from(Cuboid::new(2.0, 2.0, 2.0)));
            world
                .spawn((
                    Mesh3d(mesh),
                    Transform::default(),
                    GlobalTransform::default(),
                ))
                .id()
        };
        assert_eq!(
            Staleness::SceneMoved,
            stale_after("prop_added", |world| {
                spawn_prop(world);
            })
        );
    }

    /// A scene comes back before its props do. Until they land, the world
    /// hashes as one with fewer meshes in it than the bake had, a difference
    /// that says nothing about whether anything moved.
    #[test]
    fn a_bake_is_not_out_of_date_because_the_scene_is_still_loading() {
        let dir = std::env::temp_dir().join(format!("jd_loading_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a place to write the artifact");
        let mut world = world_with_terrain(&dir);

        // A prop that was there when the bake was taken.
        let mesh = world
            .resource_mut::<Assets<Mesh>>()
            .add(Mesh::from(Cuboid::new(2.0, 2.0, 2.0)));
        world.spawn((
            Mesh3d(mesh.clone()),
            Transform::default(),
            GlobalTransform::default(),
        ));
        run_bake_to_completion(&mut world);
        assert_eq!(
            world.resource::<TerrainNavmeshState>().staleness,
            Staleness::Fresh
        );

        // The same prop, as a handle whose asset has not arrived.
        world.resource_mut::<Assets<Mesh>>().remove(&mesh);
        world.insert_resource(Time::<()>::default());
        world
            .resource_mut::<Time<()>>()
            .advance_by(std::time::Duration::from_secs(1));
        world
            .run_system_cached(refresh_staleness)
            .expect("the staleness check runs");

        let state = world.resource::<TerrainNavmeshState>();
        assert_eq!(
            state.staleness,
            Staleness::Fresh,
            "a half-arrived scene is not a scene that moved: {}",
            summary(state)
        );

        let _ = std::fs::remove_file(dir.join("zone.jdnav"));
    }

    /// The overlay is the ground-following detail surface, not the coarse
    /// hulls: a hull edge is a straight line between corners that can be a
    /// hundred metres and several metres of height apart, and on rolling ground
    /// it passes under the hills it spans.
    #[test]
    fn the_overlay_is_built_from_the_surface_that_follows_the_ground() {
        let dir = std::env::temp_dir().join(format!("jd_overlay_mesh_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a place to write the artifact");
        let mut world = world_with_terrain(&dir);
        run_bake_to_completion(&mut world);

        let artifact = &world
            .resource::<TerrainNavmeshState>()
            .baked
            .as_ref()
            .expect("a finished bake")
            .artifact;
        assert!(
            artifact.surface_triangles.len() > artifact.polygons.len(),
            "the detail surface is the finer of the two"
        );
        let mesh = surface_mesh(artifact, 0.5).expect("a bake with ground has a surface");
        assert_eq!(
            mesh.count_vertices(),
            artifact.surface_vertices.len(),
            "every surface vertex is drawn"
        );
        assert_eq!(
            mesh.indices().map(bevy::render::mesh::Indices::len),
            Some(artifact.surface_triangles.len() * 3),
            "every surface triangle is drawn"
        );

        // An artifact carrying no surface draws nothing rather than falling
        // back to the hulls.
        let mut bare = artifact.clone();
        bare.surface_triangles.clear();
        assert!(surface_mesh(&bare, 0.5).is_none());

        let _ = std::fs::remove_file(dir.join("zone.jdnav"));
    }

    /// A surface mesh lying where the ground lies z-fights it over the whole
    /// zone, so the lift comes from the ground the bake names rather than from
    /// whatever is selected.
    #[test]
    fn the_overlay_keeps_its_lift_when_the_terrain_is_deselected() {
        let dir = std::env::temp_dir().join(format!("jd_overlay_lift_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a place to write the artifact");
        let mut world = world_with_terrain(&dir);
        run_bake_to_completion(&mut world);
        world
            .run_system_cached(sync_navmesh_overlay)
            .expect("the overlay sync runs");
        world.flush();
        let selected = overlay_lift(&mut world);

        world.commands().queue(|world: &mut World| {
            let mut selection = world.resource_mut::<Selection>();
            *selection = Selection::default();
        });
        world.flush();
        world
            .run_system_cached(sync_navmesh_overlay)
            .expect("the overlay sync runs");
        world.flush();

        assert_eq!(
            overlay_lift(&mut world),
            selected,
            "clicking away must not drop the overlay onto the ground"
        );
        assert!(
            selected > MIN_LIFT,
            "a terrain this size lifts the overlay clear of it, not by a tenth of a metre"
        );

        let _ = std::fs::remove_file(dir.join("zone.jdnav"));
    }

    /// The lowest vertex of the drawn overlay, standing in for how far off the
    /// ground the whole sheet sits.
    fn overlay_lift(world: &mut World) -> f32 {
        let floor = world
            .resource::<TerrainNavmeshState>()
            .baked
            .as_ref()
            .expect("a finished bake")
            .artifact
            .surface_vertices
            .iter()
            .map(|v| v.y)
            .fold(f32::INFINITY, f32::min);
        let mesh = world
            .query_filtered::<&Mesh3d, With<NavmeshOverlay>>()
            .iter(world)
            .next()
            .expect("the overlay is drawn")
            .0
            .clone();
        let meshes = world.resource::<Assets<Mesh>>();
        let mesh = meshes.get(&mesh).expect("the overlay has a mesh");
        let Some(bevy::render::mesh::VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("the overlay mesh has positions");
        };
        positions.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min) - floor
    }

    /// The tint moves only when staleness flips, and `get_mut` raises
    /// `AssetEvent::Modified` whether or not the value changed, so a steady
    /// overlay must not touch its material.
    #[test]
    fn a_steady_overlay_stops_rewriting_its_own_tint() {
        let dir = std::env::temp_dir().join(format!("jd_overlay_tint_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a place to write the artifact");
        let mut world = world_with_terrain(&dir);
        run_bake_to_completion(&mut world);
        for _ in 0..2 {
            world
                .run_system_cached(sync_navmesh_overlay)
                .expect("the overlay sync runs");
            world.flush();
        }

        world.clear_trackers();
        world
            .run_system_cached(sync_navmesh_overlay)
            .expect("the overlay sync runs");
        world.flush();
        assert!(
            !world.is_resource_changed::<Assets<StandardMaterial>>(),
            "a frame that changed nothing must not touch the material store, \
             whose every `get_mut` re-prepares the material on the GPU"
        );

        // A flip to stale does repaint, without rebuilding the mesh.
        world.resource_mut::<TerrainNavmeshState>().staleness = Staleness::SceneMoved;
        world
            .run_system_cached(sync_navmesh_overlay)
            .expect("the overlay sync runs");
        world.flush();
        assert_eq!(
            overlay_color(&mut world),
            default_style::TERRAIN_NAVMESH_STALE,
            "a stale bake draws amber"
        );

        let _ = std::fs::remove_file(dir.join("zone.jdnav"));
    }

    fn overlay_material(world: &mut World) -> Handle<StandardMaterial> {
        world
            .query_filtered::<&MeshMaterial3d<StandardMaterial>, With<NavmeshOverlay>>()
            .iter(world)
            .next()
            .expect("the overlay is drawn")
            .0
            .clone()
    }

    fn overlay_color(world: &mut World) -> Color {
        let handle = overlay_material(world);
        world
            .resource::<Assets<StandardMaterial>>()
            .get(&handle)
            .expect("the overlay has a material")
            .base_color
    }

    /// The drawn overlay is a mesh in the world and the bake reads meshes, so
    /// it has to be invisible to the bake.
    #[test]
    fn the_drawn_overlay_is_never_baked_into_the_next_navmesh() {
        let dir = std::env::temp_dir().join(format!("jd_overlay_bake_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a place to write the artifact");
        let mut world = world_with_terrain(&dir);
        run_bake_to_completion(&mut world);
        let before = world
            .run_system_cached(count_bakeable_meshes)
            .expect("the count runs");

        world
            .run_system_cached(sync_navmesh_overlay)
            .expect("the overlay sync runs");
        world.flush();

        let drawn = world
            .query_filtered::<Entity, With<NavmeshOverlay>>()
            .iter(&world)
            .count();
        assert_eq!(drawn, 1, "the overlay is drawn");
        let after = world
            .run_system_cached(count_bakeable_meshes)
            .expect("the count runs");
        assert_eq!(before, after, "and the bake cannot see it");

        // It goes when the scene it describes does.
        forget_scene_navmesh(&mut world);
        assert_eq!(
            world
                .query_filtered::<Entity, With<NavmeshOverlay>>()
                .iter(&world)
                .count(),
            0
        );

        let _ = std::fs::remove_file(dir.join("zone.jdnav"));
    }

    fn count_bakeable_meshes(geometry: SceneGeometry) -> usize {
        geometry.placed().len()
    }

    /// Whether the overlay is drawn is the author's preference. Opening a scene
    /// drops the bake without turning the setting back on.
    #[test]
    fn forgetting_a_scene_keeps_the_overlay_where_the_author_put_it() {
        let dir = std::env::temp_dir().join(format!("jd_overlay_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a place to write the artifact");
        let mut world = world_with_terrain(&dir);
        run_bake_to_completion(&mut world);
        world.resource_mut::<TerrainNavmeshState>().show_overlay = false;

        forget_scene_navmesh(&mut world);

        let state = world.resource::<TerrainNavmeshState>();
        assert!(!state.show_overlay, "the overlay stays where it was put");
        assert!(state.baked.is_none(), "the bake still goes");
        assert_eq!(state.status, BakeStatus::Idle);
        assert_eq!(state.staleness, Staleness::Fresh);

        let _ = std::fs::remove_file(dir.join("zone.jdnav"));
    }

    /// The ground can move without a single height changing, and a bake
    /// is taken in world space.
    #[test]
    fn moving_the_terrain_itself_marks_a_bake_out_of_date() {
        assert_eq!(
            Staleness::SceneMoved,
            stale_after("moved_terrain", |world| {
                let mut q = world
                    .query_filtered::<&mut GlobalTransform, With<jackdaw_scene_types::Terrain>>();
                for mut transform in q.iter_mut(world) {
                    *transform = GlobalTransform::from(Transform::from_xyz(40.0, 0.0, 0.0));
                }
            })
        );
    }

    #[test]
    fn scaling_the_terrain_marks_a_bake_out_of_date() {
        assert_eq!(
            Staleness::SceneMoved,
            stale_after("scaled_terrain", |world| {
                let mut q = world
                    .query_filtered::<&mut GlobalTransform, With<jackdaw_scene_types::Terrain>>();
                for mut transform in q.iter_mut(world) {
                    *transform = GlobalTransform::from(Transform::from_scale(Vec3::splat(2.0)));
                }
            })
        );
    }

    /// An agent no bake could be run for reaches the editor through scene text
    /// rather than the options bar, and is turned away at the click: a bake at
    /// these numbers succeeds and writes a file nothing will read back.
    #[test]
    fn an_agent_nothing_could_walk_at_is_refused_before_the_bake_starts() {
        for broken in [
            jackdaw_scene_types::TerrainNavmesh {
                agent_radius: -0.5,
                ..default()
            },
            jackdaw_scene_types::TerrainNavmesh {
                max_slope_degrees: 95.0,
                ..default()
            },
            jackdaw_scene_types::TerrainNavmesh {
                agent_height: 0.0,
                ..default()
            },
        ] {
            let dir = std::env::temp_dir().join("jd_bad_agent");
            let mut world = world_with_terrain(&dir);
            let entity = world.resource::<Selection>().entities[0];
            world
                .get_mut::<jackdaw_scene_types::Terrain>(entity)
                .expect("the test terrain")
                .navmesh = broken.clone();

            let result = world
                .run_system_cached_with(terrain_navmesh_bake, no_params())
                .expect("the bake operator runs");

            assert_eq!(result, OperatorResult::Cancelled, "{broken:?}");
            assert!(!world.contains_resource::<RunningBake>(), "{broken:?}");
            let summary = summary(world.resource::<TerrainNavmeshState>());
            assert!(
                summary.contains("not a size anything can walk at"),
                "{summary}"
            );
        }
    }

    /// Opening another scene into the same tab is not a tab switch, so nothing
    /// moves the replaced scene's bake anywhere. It has to be dropped, or it
    /// draws over the new ground and reports itself fresh, the terrain it was
    /// measured against being absent to disagree.
    #[test]
    fn opening_another_scene_in_the_same_tab_drops_the_old_bake() {
        let dir = std::env::temp_dir().join(format!("jd_same_tab_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a place to write the artifact");
        let mut world = world_with_terrain(&dir);
        run_bake_to_completion(&mut world);
        assert!(world.resource::<TerrainNavmeshState>().baked.is_some());

        forget_scene_navmesh(&mut world);

        let state = world.resource::<TerrainNavmeshState>();
        assert!(state.baked.is_none());
        assert_eq!(state.status, BakeStatus::Idle);
        assert_eq!(summary(state), "Not baked yet");

        // The tab is then free to read the incoming scene's own.
        import_beside_scene(&mut world, &dir.join("zone.bsn").to_string_lossy());
        assert!(world.resource::<TerrainNavmeshState>().baked.is_some());

        let _ = std::fs::remove_file(dir.join("zone.jdnav"));
    }

    /// Save As is the same ground under a new name. The bake follows it,
    /// and the guard that stops one scene's navmesh landing beside
    /// another's does not fire on a scene that never changed.
    #[test]
    fn saving_a_baked_scene_under_a_new_name_writes_the_navmesh_there() {
        let dir = std::env::temp_dir().join(format!("jd_save_as_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a place to write the artifact");
        let mut world = world_with_terrain(&dir);
        world.insert_resource(crate::scenes::Scenes::default());
        world
            .resource_mut::<crate::scenes::Scenes>()
            .push_tab(crate::scenes::SceneTab::new_untitled(1));
        run_bake_to_completion(&mut world);

        let renamed = dir.join("harbour.bsn");
        crate::scene_io::retarget_active_scene(&mut world, &renamed.to_string_lossy());
        export_beside_scene(&mut world, &renamed.to_string_lossy());

        let artifact = dir.join("harbour.jdnav");
        assert!(artifact.is_file(), "the navmesh followed the rename");
        let state = world.resource::<TerrainNavmeshState>();
        assert_eq!(state.status, BakeStatus::Done, "{}", summary(state));
        assert_eq!(
            state.baked.as_ref().and_then(|b| b.path.as_deref()),
            Some(artifact.as_path())
        );

        // A plain Save afterwards is a no-op rather than a refusal.
        export_beside_scene(&mut world, &renamed.to_string_lossy());
        assert_eq!(
            world.resource::<TerrainNavmeshState>().status,
            BakeStatus::Done
        );

        let _ = std::fs::remove_file(&artifact);
        let _ = std::fs::remove_file(dir.join("zone.jdnav"));
    }

    /// With two terrains in one scene, a bake of the first is not stale because
    /// the second was clicked on or sculpted.
    #[test]
    fn a_bake_is_measured_against_its_own_terrain() {
        assert_eq!(
            Staleness::Fresh,
            stale_after("two_terrains", |world| {
                use jackdaw_terrain::region::{RegionCoord, RegionSize, TerrainRegions};
                use jackdaw_terrain::sidecar::RegionTerrainData;

                let other = "other.terrain-0.jdterrain";
                let mut document = RegionTerrainData {
                    regions: TerrainRegions::new(RegionSize::new(64).expect("power of two")),
                    ..default()
                };
                document.regions.ensure_region(RegionCoord::ORIGIN);
                document.regions.set_height(3, 3, 12.0);
                world
                    .resource_mut::<TerrainDataStore>()
                    .insert(other, document);
                let entity = world
                    .spawn((
                        jackdaw_scene_types::Terrain {
                            resolution: 64,
                            size: Vec2::splat(64.0),
                            data_path: other.to_string(),
                            ..default()
                        },
                        Transform::from_xyz(500.0, 0.0, 0.0),
                        GlobalTransform::default(),
                    ))
                    .id();
                world.resource_mut::<Selection>().entities = vec![entity];
            })
        );
    }

    /// Bake, drop the editor's memory of it, and reopen the scene: the navmesh
    /// comes back off disk rather than reading as one nobody baked.
    #[test]
    fn reopening_a_scene_finds_the_navmesh_baked_beside_it() {
        let dir = std::env::temp_dir().join(format!("jd_reopen_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a place to write the artifact");
        let mut world = world_with_terrain(&dir);
        run_bake_to_completion(&mut world);
        let baked_count = world
            .resource::<TerrainNavmeshState>()
            .baked
            .as_ref()
            .expect("a finished bake")
            .artifact
            .polygons
            .len();

        // The same scene with no memory of the bake.
        let mut reopened = world_with_terrain(&dir);
        assert!(summary(reopened.resource::<TerrainNavmeshState>()).contains("Not baked"));
        import_beside_scene(&mut reopened, &dir.join("zone.bsn").to_string_lossy());

        let state = reopened.resource::<TerrainNavmeshState>();
        assert_eq!(state.status, BakeStatus::Done);
        assert_eq!(
            state.staleness,
            Staleness::Fresh,
            "nothing moved: {}",
            summary(state)
        );
        let loaded = state.baked.as_ref().expect("the artifact was read back");
        assert_eq!(loaded.artifact.polygons.len(), baked_count);
        assert_eq!(loaded.data_path(), "zone.terrain-0.jdterrain");
        assert!(
            summary(state).contains("from zone.jdnav"),
            "{}",
            summary(state)
        );

        let _ = std::fs::remove_file(dir.join("zone.jdnav"));
    }

    #[test]
    fn a_navmesh_read_back_is_measured_against_the_ground_as_it_is_now() {
        let dir = std::env::temp_dir().join(format!("jd_reopen_stale_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a place to write the artifact");
        let mut world = world_with_terrain(&dir);
        run_bake_to_completion(&mut world);

        let mut reopened = world_with_terrain(&dir);
        reopened
            .resource_mut::<TerrainDataStore>()
            .entry_for(&jackdaw_scene_types::Terrain {
                resolution: 64,
                size: Vec2::splat(64.0),
                data_path: "zone.terrain-0.jdterrain".to_string(),
                ..default()
            })
            .expect("the document is loaded")
            .heights_mut()[100] = 9.0;
        import_beside_scene(&mut reopened, &dir.join("zone.bsn").to_string_lossy());

        let state = reopened.resource::<TerrainNavmeshState>();
        assert_eq!(
            state.staleness,
            Staleness::SceneMoved,
            "the ground moved while the scene was shut"
        );
        assert!(summary(state).contains("out of date"));

        let _ = std::fs::remove_file(dir.join("zone.jdnav"));
    }

    #[test]
    fn a_navmesh_this_build_cannot_read_is_reported_rather_than_replaced() {
        let dir = std::env::temp_dir().join(format!("jd_reopen_bad_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a place to write the artifact");
        let artifact = dir.join("zone.jdnav");
        std::fs::write(&artifact, b"JDNAVMS\0\xff\xff").expect("a broken artifact");
        let mut world = world_with_terrain(&dir);

        import_beside_scene(&mut world, &dir.join("zone.bsn").to_string_lossy());

        let state = world.resource::<TerrainNavmeshState>();
        assert!(state.baked.is_none());
        assert!(summary(state).contains("Bake failed"), "{}", summary(state));
        assert!(artifact.exists(), "the file on disk is the author's");

        let _ = std::fs::remove_file(&artifact);
    }

    /// Spawns a cube carrying `extra` and reports whether the bake counts it as
    /// scene geometry.
    fn geometry_takes(extra: impl Bundle) -> bool {
        geometry_takes_cube(|cube| {
            cube.insert(extra);
        })
    }

    /// Spawns a bare cube and reports whether the bake counts it as scene
    /// geometry.
    fn geometry_takes_plain_cube() -> bool {
        geometry_takes_cube(|_| {})
    }

    /// Spawns a cube, lets `dress` add to it, and reports whether the bake
    /// counts it as scene geometry.
    fn geometry_takes_cube(dress: impl FnOnce(&mut EntityWorldMut)) -> bool {
        let dir = std::env::temp_dir().join(format!("jd_geom_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a place to write the artifact");
        let mut world = world_with_terrain(&dir);
        let mesh = world
            .resource_mut::<Assets<Mesh>>()
            .add(Mesh::from(Cuboid::new(2.0, 2.0, 2.0)));
        let mut cube = world.spawn((
            Mesh3d(mesh),
            Transform::default(),
            GlobalTransform::default(),
        ));
        dress(&mut cube);
        world.flush();
        world
            .run_system_cached(|geometry: SceneGeometry| geometry.placements().count())
            .expect("the geometry gather runs")
            == 1
    }

    #[test]
    fn what_the_bake_leaves_out_of_the_scene() {
        assert!(geometry_takes_plain_cube(), "an ordinary mesh is ground");
        assert!(
            geometry_takes(avian3d::prelude::RigidBody::Static),
            "so is a static body"
        );
        assert!(
            !geometry_takes(avian3d::prelude::RigidBody::Dynamic),
            "a body the simulation moves is one frame of a simulation"
        );
        assert!(
            !geometry_takes(avian3d::prelude::Sensor),
            "nothing stands on a trigger volume"
        );
        assert!(
            !geometry_takes(Visibility::Hidden),
            "a mesh hidden in the viewport is not solid ground"
        );
        assert!(
            geometry_takes(Visibility::Visible),
            "and a visible one still is"
        );
    }

    #[test]
    fn a_mesh_hidden_by_its_parent_is_not_baked() {
        let dir = std::env::temp_dir().join(format!("jd_hidden_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a place to write the artifact");
        let mut world = world_with_terrain(&dir);
        let mesh = world
            .resource_mut::<Assets<Mesh>>()
            .add(Mesh::from(Cuboid::new(2.0, 2.0, 2.0)));
        let parent = world.spawn((Transform::default(), Visibility::Hidden)).id();
        world.spawn((
            Mesh3d(mesh),
            Transform::default(),
            GlobalTransform::default(),
            ChildOf(parent),
        ));
        world.flush();

        let count = world
            .run_system_cached(|geometry: SceneGeometry| geometry.placements().count())
            .expect("the geometry gather runs");
        assert_eq!(count, 0);
    }

    #[test]
    fn a_bake_waits_for_the_scene_to_finish_loading() {
        let dir = std::env::temp_dir().join(format!("jd_loading_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a place to write the artifact");
        let mut world = world_with_terrain(&dir);
        // A handle to an asset that never arrives stands in for the first
        // frames after a scene opens.
        world.spawn((
            Mesh3d(Handle::<Mesh>::default()),
            Transform::default(),
            GlobalTransform::default(),
        ));
        world.flush();

        let result = world
            .run_system_cached_with(terrain_navmesh_bake, no_params())
            .expect("the bake operator runs");
        assert_eq!(result, OperatorResult::Cancelled);
        let summary = summary(world.resource::<TerrainNavmeshState>());
        assert!(summary.contains("still loading"), "{summary}");
    }

    #[test]
    fn a_mirrored_placement_keeps_its_faces_pointing_the_way_they_did() {
        let mesh = Mesh::from(Plane3d::default().mesh().size(4.0, 4.0));
        let upright = placed_trimesh(&mesh, Affine3A::IDENTITY).expect("a plane converts");
        let mirrored = placed_trimesh(&mesh, Affine3A::from_scale(Vec3::new(-1.0, 1.0, 1.0)))
            .expect("a plane converts");

        let up = |tri: &TriMesh, index: usize| {
            let [a, b, c] = tri.indices[index]
                .to_array()
                .map(|i| tri.vertices[i as usize]);
            (b - a).cross(c - a).y
        };
        for index in 0..upright.indices.len() {
            assert!(up(&upright, index) > 0.0);
            assert!(
                up(&mirrored, index) > 0.0,
                "a mirrored plane is still ground"
            );
        }
    }

    /// A world holding a finished bake said to belong to `scene`.
    fn world_with_baked(scene: Option<&std::path::Path>) -> World {
        let params = BakeParams::default().with_terrain_cell(Vec2::splat(1.0));
        let artifact =
            rasterize(flat_input(32, 1.0), params, 5, &ground()).expect("a flat plane is bakeable");
        let mut world = World::new();
        world.insert_resource(TerrainNavmeshState {
            status: BakeStatus::Done,
            baked: Some(BakedNavmesh {
                artifact,
                path: scene.map(|s| s.with_extension(navmesh::EXTENSION)),
                scene: scene.map(std::path::Path::to_path_buf),
                seconds: Some(0.5),
            }),
            ..Default::default()
        });
        world
    }

    #[test]
    fn a_bake_is_not_written_beside_a_scene_it_was_not_taken_from() {
        let dir = std::env::temp_dir().join(format!("jd_cross_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a place to write the artifact");
        let mine = dir.join("mine.bsn");
        let theirs = dir.join("theirs.bsn");
        let mut world = world_with_baked(Some(&mine));

        export_beside_scene(&mut world, &theirs.to_string_lossy());

        assert!(
            !theirs.with_extension(navmesh::EXTENSION).exists(),
            "another scene's navmesh must not land beside this one"
        );
        // The bake is untouched by where it was not written. A refused
        // destination is a fact about this save, and reporting it as a failed
        // bake would leave a good navmesh reading as a broken one.
        let state = world.resource::<TerrainNavmeshState>();
        assert_eq!(state.status, BakeStatus::Done);
        assert!(summary(state).contains("mine.jdnav"), "{}", summary(state));
        assert_eq!(
            state.baked.as_ref().and_then(|b| b.scene.as_deref()),
            Some(mine.as_path()),
            "the bake still belongs to the scene it came from"
        );
    }

    #[test]
    fn a_bake_made_before_the_scene_had_a_name_lands_on_the_first_save() {
        let dir = std::env::temp_dir().join(format!("jd_firstsave_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a place to write the artifact");
        let scene = dir.join("named.bsn");
        let mut world = world_with_baked(None);

        export_beside_scene(&mut world, &scene.to_string_lossy());

        let artifact = scene.with_extension(navmesh::EXTENSION);
        let bytes = std::fs::read(&artifact).expect("the artifact reached disk");
        assert!(navmesh::decode(&bytes).is_ok());
        let baked = world
            .resource::<TerrainNavmeshState>()
            .baked
            .as_ref()
            .expect("still baked");
        assert_eq!(baked.scene.as_deref(), Some(scene.as_path()));
        let _ = std::fs::remove_file(&artifact);
    }

    #[test]
    fn a_bake_travels_with_its_own_tab_and_not_with_the_editor() {
        let mut world = world_with_baked(Some(std::path::Path::new("a.bsn")));
        let stashed = take_from_world(&mut world).expect("the state is there to take");
        assert!(
            world.resource::<TerrainNavmeshState>().baked.is_none(),
            "the tab left with its bake; the next tab starts unbaked"
        );

        install_in_world(&mut world, stashed);
        let baked = world
            .resource::<TerrainNavmeshState>()
            .baked
            .as_ref()
            .expect("coming back to the tab brings its bake back");
        assert_eq!(baked.scene.as_deref(), Some(std::path::Path::new("a.bsn")));
    }

    #[test]
    fn leaving_the_editor_does_not_wedge_the_bake_button() {
        let mut world = World::new();
        world.insert_resource(TerrainNavmeshState {
            status: BakeStatus::Baking,
            ..Default::default()
        });
        world.insert_resource(RunningBake {
            handle: AsyncComputeTaskPool::get_or_init(Default::default)
                .spawn(async { Err("never polled".to_string()) }),
            started: Instant::now(),
            scene: None,
            destination: None,
        });

        world
            .run_system_cached(drop_running_bake)
            .expect("the exit system runs");
        world.flush();

        assert!(!world.contains_resource::<RunningBake>());
        assert_eq!(
            world.resource::<TerrainNavmeshState>().status,
            BakeStatus::Idle
        );
    }

    #[test]
    fn a_moving_body_is_not_ground() {
        use avian3d::prelude::RigidBody;
        assert!(!moves(None));
        assert!(!moves(Some(&RigidBody::Static)));
        assert!(moves(Some(&RigidBody::Dynamic)));
        assert!(moves(Some(&RigidBody::Kinematic)));
    }

    #[test]
    fn the_summary_says_which_way_a_bake_went() {
        let mut state = TerrainNavmeshState::default();
        assert!(summary(&state).contains("Not baked"));
        state.status = BakeStatus::Baking;
        assert!(summary(&state).contains("Baking"));
        state.status = BakeStatus::Failed("no ground".to_string());
        assert!(summary(&state).contains("no ground"));
    }

    #[test]
    fn a_stale_bake_says_so() {
        let params = BakeParams::default().with_terrain_cell(Vec2::splat(1.0));
        let artifact =
            rasterize(flat_input(32, 1.0), params, 3, &ground()).expect("a flat plane is bakeable");
        let mut state = TerrainNavmeshState {
            status: BakeStatus::Done,
            staleness: Staleness::SceneMoved,
            baked: Some(BakedNavmesh {
                artifact,
                path: Some(PathBuf::from("scene.jdnav")),
                scene: Some(PathBuf::from("scene.bsn")),
                seconds: Some(0.5),
            }),
            ..Default::default()
        };
        assert!(summary(&state).contains("out of date"));
        state.staleness = Staleness::Fresh;
        let fresh = summary(&state);
        assert!(!fresh.contains("out of date"));
        assert!(fresh.contains("scene.jdnav"));
    }
}
