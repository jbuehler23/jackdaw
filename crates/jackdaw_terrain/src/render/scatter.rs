//! Drawing a terrain's stored scatter.
//!
//! A placement is data, not an entity: what is spawned is the least a draw needs
//! -- a mesh, a material and a transform -- so bevy's automatic instancing
//! batches every placement of the same asset into one draw.
//!
//! A glTF is resolved once per palette entry rather than once per placement: the
//! file's node graph is flattened into a list of [`ScatterPrimitive`]s, each a
//! mesh, a material and the transform that node sat at inside the file.
//!
//! Placements are spawned under one chunk entity per region. Bevy frustum culls
//! per entity and skips a hidden parent's children, so hiding an off-screen
//! chunk keeps the per-frame visibility cost proportional to the regions in view
//! rather than to the placements in the document.
//!
//! Both hosts -- an editor holding its documents in a store, a game holding one
//! per terrain -- write [`TerrainScatter`] onto the terrain entity and mark what
//! changed in [`ScatterDirty`].

use bevy::asset::LoadState;
use bevy::camera::primitives::{Aabb, Frustum, MeshAabb};
use bevy::camera::visibility::VisibilityRange;
use bevy::gltf::{Gltf, GltfMesh, GltfNode};
use bevy::log::warn;
use bevy::math::Affine3A;
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;

use crate::placement::{ScatterPalette, ScatterPlacement};
use crate::region::RegionCoord;
use crate::sidecar::RegionTerrainData;

/// Tallest an asset can be and still count as ground cover: not an obstacle a
/// path has to go round, and not worth drawing once its pixels are smaller than
/// the batch is wide.
pub const GROUND_COVER_HEIGHT: f32 = 1.5;

/// Distance past which ground cover stops drawing when a palette entry
/// states no cutoff of its own.
pub const GROUND_COVER_CULL_DISTANCE: f32 = 80.0;

/// Fraction of a cutoff distance the fade to nothing spans.
const CULL_FADE: f32 = 0.1;

/// The stored scatter one terrain draws.
///
/// A projection of the terrain's document rather than the document itself, so
/// the renderer is the same however the host keeps its documents.
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub struct TerrainScatter {
    /// The assets and stamp identities the placements index into.
    pub palette: ScatterPalette,
    /// Placements by the region holding them, in region-coordinate order.
    pub regions: Vec<ScatterRegion>,
}

/// One region's placements, with where that region sits.
#[derive(Clone, Debug, PartialEq)]
pub struct ScatterRegion {
    pub coord: RegionCoord,
    /// Terrain-local position of the region's minimum corner: what a
    /// placement's offsets are measured from.
    pub origin: Vec3,
    pub placements: Vec<ScatterPlacement>,
}

impl TerrainScatter {
    /// The scatter a document holds, in the space of the terrain it sits
    /// on.
    pub fn from_document(data: &RegionTerrainData) -> Self {
        let mut regions = Vec::new();
        for (coord, region) in data.regions.iter_sorted() {
            if region.placements().is_empty() {
                continue;
            }
            let origin = data.placement_position(
                coord,
                &ScatterPlacement {
                    group: 0,
                    asset: 0,
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    yaw: 0.0,
                    scale: 1.0,
                },
            );
            regions.push(ScatterRegion {
                coord,
                origin,
                placements: region.placements().to_vec(),
            });
        }
        Self {
            palette: data.scatter.clone(),
            regions,
        }
    }

    /// Whether there is nothing to draw.
    pub fn is_empty(&self) -> bool {
        self.regions.iter().all(|r| r.placements.is_empty())
    }

    /// How many placements this draws.
    pub fn placement_count(&self) -> usize {
        self.regions.iter().map(|r| r.placements.len()).sum()
    }
}

/// Which of a terrain's regions the renderer has yet to catch up with.
///
/// A host that edited one stroke's worth of placements names the regions
/// it touched; one that has just loaded a document sets `all`. Empty and
/// not `all` means every chunk on screen is already the data.
#[derive(Component, Clone, Debug, Default)]
pub struct ScatterDirty {
    /// Every chunk is stale, including ones no region backs any more.
    pub all: bool,
    /// Chunks stale by coordinate.
    pub regions: HashSet<RegionCoord>,
}

impl ScatterDirty {
    /// Mark every chunk stale.
    pub fn all() -> Self {
        Self {
            all: true,
            regions: HashSet::new(),
        }
    }

    /// Mark one region's chunk stale.
    pub fn touch(&mut self, coord: RegionCoord) {
        self.regions.insert(coord);
    }

    /// Fold another mark into this one: a mark the renderer has not caught up
    /// with must not be overwritten by the next edit's, or the regions between
    /// them never rebuild.
    pub fn merge(&mut self, other: &Self) {
        self.all |= other.all;
        self.regions.extend(other.regions.iter().copied());
    }

    fn is_clean(&self) -> bool {
        !self.all && self.regions.is_empty()
    }
}

/// One region's placements, as one parent whose visibility the chunk cull
/// toggles.
#[derive(Component, Clone, Copy, Debug)]
pub struct ScatterChunk {
    /// The terrain this belongs to, kept so a cull can look it up without
    /// walking the hierarchy.
    pub terrain: Entity,
    pub coord: RegionCoord,
    /// Bounds of everything drawn under this chunk, in the terrain's local
    /// space.
    pub bounds: Aabb,
}

/// One drawn placement, by where in the document it came from.
///
/// The only link back to the data: nothing else about the entity says it is
/// scatter.
#[derive(Component, Clone, Copy, Debug)]
pub struct ScatterRendered {
    pub region: RegionCoord,
    /// Index into that region's placement list.
    pub index: usize,
}

/// One drawable part of a palette asset: the smallest thing a batch can be.
#[derive(Clone, Debug)]
pub struct ScatterPrimitive {
    pub mesh: Handle<Mesh>,
    pub material: Handle<StandardMaterial>,
    /// Where this part sat inside the glTF, flattened through the node
    /// graph above it.
    pub local: Transform,
}

/// What one palette asset resolved to.
#[derive(Clone, Debug)]
enum ScatterAsset {
    /// The glTF is loading.
    Loading(Handle<Gltf>),
    Ready(ReadyAsset),
    /// The glTF failed to load. Placements of it draw nothing, and the
    /// failure is reported once.
    Failed,
}

#[derive(Clone, Debug)]
struct ReadyAsset {
    primitives: Vec<ScatterPrimitive>,
    /// Bounds of every primitive together, at scale 1.
    bounds: Aabb,
}

impl ReadyAsset {
    /// How tall this asset stands at scale 1.
    fn height(&self) -> f32 {
        self.bounds.half_extents.y * 2.0
    }
}

/// Palette assets resolved once each, shared by every terrain naming them.
#[derive(Resource, Default)]
pub struct ScatterAssets {
    entries: HashMap<String, ScatterAsset>,
    /// Assets that finished resolving since the last rebuild: every
    /// terrain naming one of these is stale.
    settled: Vec<String>,
}

impl ScatterAssets {
    /// The primitives a palette entry draws, or `None` while it is loading
    /// or after it failed.
    pub fn primitives(&self, asset: &str) -> Option<&[ScatterPrimitive]> {
        match self.entries.get(asset) {
            Some(ScatterAsset::Ready(ready)) => Some(&ready.primitives),
            _ => None,
        }
    }

    /// How tall a palette entry stands at scale 1, or `None` while it is
    /// unresolved.
    pub fn height(&self, asset: &str) -> Option<f32> {
        Some(self.bounds(asset)?.half_extents.y * 2.0)
    }

    /// A palette entry's bounding box at scale 1, or `None` while it is
    /// unresolved. What a navmesh bake stands an obstacle in.
    pub fn bounds(&self, asset: &str) -> Option<Aabb> {
        match self.entries.get(asset) {
            Some(ScatterAsset::Ready(ready)) => Some(ready.bounds),
            _ => None,
        }
    }
}

/// Resolves palette assets, draws stored scatter, and culls it by chunk.
///
/// Independent of [`super::TerrainRenderPlugin`]: a host that draws the ground
/// without scatter adds one and not the other.
pub struct ScatterRenderPlugin;

/// The stages a host orders its own scatter work against.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScatterSystems {
    /// Resolving palette assets and respawning the chunks a dirty mark
    /// names. A host that writes [`TerrainScatter`] runs before this.
    Rebuild,
    /// Hiding the chunks no camera can see.
    Cull,
}

impl Plugin for ScatterRenderPlugin {
    fn build(&self, app: &mut App) {
        // The asset types this reads, declared rather than assumed: these
        // systems are also scheduled in a headless build with no glTF loader,
        // where a missing collection would fail their validation.
        app.init_asset::<Gltf>()
            .init_asset::<GltfNode>()
            .init_asset::<GltfMesh>()
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>();
        app.init_resource::<ScatterAssets>().add_systems(
            Update,
            (
                request_palette_assets,
                resolve_palette_assets,
                rebuild_chunks,
            )
                .chain()
                .in_set(ScatterSystems::Rebuild),
        );
        // After the frusta are written and before they are read: culling against
        // last frame's frustum pops chunks in at the screen edge.
        app.configure_sets(
            PostUpdate,
            ScatterSystems::Cull
                .after(bevy::camera::visibility::VisibilitySystems::UpdateFrusta)
                .before(bevy::camera::visibility::VisibilitySystems::CheckVisibility),
        );
        app.add_systems(PostUpdate, cull_chunks.in_set(ScatterSystems::Cull));
    }
}

/// Start loading every palette asset no terrain has asked for yet.
fn request_palette_assets(
    assets: ResMut<ScatterAssets>,
    server: Res<AssetServer>,
    terrains: Query<&TerrainScatter, Changed<TerrainScatter>>,
) {
    let assets = assets.into_inner();
    for scatter in &terrains {
        for entry in &scatter.palette.assets {
            if entry.is_tombstone() || assets.entries.contains_key(&entry.asset) {
                continue;
            }
            assets.entries.insert(
                entry.asset.clone(),
                ScatterAsset::Loading(server.load(&entry.asset)),
            );
        }
    }
}

/// Flatten every glTF that has finished loading into primitives.
fn resolve_palette_assets(
    assets: ResMut<ScatterAssets>,
    server: Res<AssetServer>,
    gltfs: Res<Assets<Gltf>>,
    nodes: Res<Assets<GltfNode>>,
    meshes: Res<Assets<GltfMesh>>,
    mesh_assets: Res<Assets<Mesh>>,
) {
    let assets = assets.into_inner();
    let mut settled = Vec::new();
    for (path, entry) in assets.entries.iter_mut() {
        let ScatterAsset::Loading(handle) = entry else {
            continue;
        };
        if matches!(server.load_state(handle.id()), LoadState::Failed(_)) {
            warn!("terrain scatter: {path} failed to load; its placements draw nothing");
            *entry = ScatterAsset::Failed;
            settled.push(path.clone());
            continue;
        }
        let Some(gltf) = gltfs.get(handle) else {
            continue;
        };
        let Some(ready) = flatten(gltf, &nodes, &meshes, &mesh_assets, &server) else {
            continue;
        };
        *entry = ScatterAsset::Ready(ready);
        settled.push(path.clone());
    }
    assets.settled = settled;
}

/// Every drawable part of a glTF, with the transform of the node it hung under
/// folded in.
///
/// `None` while any part is still loading: a half-resolved asset would have to
/// be rebuilt when the rest arrived, moving the placements under the camera.
fn flatten(
    gltf: &Gltf,
    nodes: &Assets<GltfNode>,
    meshes: &Assets<GltfMesh>,
    mesh_assets: &Assets<Mesh>,
    server: &AssetServer,
) -> Option<ReadyAsset> {
    // The roots are the nodes nothing lists as a child; glTF holds the node
    // table flat.
    let mut child_ids = HashSet::new();
    for handle in &gltf.nodes {
        let node = nodes.get(handle)?;
        for child in &node.children {
            child_ids.insert(child.id());
        }
    }

    let mut primitives = Vec::new();
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    let mut pending: Vec<(Handle<GltfNode>, Transform)> = gltf
        .nodes
        .iter()
        .filter(|handle| !child_ids.contains(&handle.id()))
        .map(|handle| (handle.clone(), Transform::IDENTITY))
        .collect();

    while let Some((handle, parent)) = pending.pop() {
        let node = nodes.get(&handle)?;
        let local = parent * node.transform;
        for child in &node.children {
            pending.push((child.clone(), local));
        }
        let Some(mesh_handle) = &node.mesh else {
            continue;
        };
        let mesh = meshes.get(mesh_handle)?;
        for primitive in &mesh.primitives {
            let material = primitive
                .material
                .as_ref()
                .and_then(|handle| standard_material(server, handle))
                .unwrap_or_default();
            // A mesh with no positions has no size to stand a placement in, so
            // it is left out rather than waited for: waiting would re-walk the
            // graph every frame for a bound that never arrives.
            let Some(bounds) = mesh_assets.get(&primitive.mesh)?.compute_aabb() else {
                warn!(
                    "terrain scatter: a primitive of {:?} has no positions and draws nothing",
                    gltf.default_scene
                );
                continue;
            };
            let affine = local.compute_affine();
            let centre = affine.transform_point3(Vec3::from(bounds.center));
            let radius = affine
                .matrix3
                .abs()
                .mul_vec3(Vec3::from(bounds.half_extents));
            min = min.min(centre - radius);
            max = max.max(centre + radius);
            primitives.push(ScatterPrimitive {
                mesh: primitive.mesh.clone(),
                material,
                local,
            });
        }
    }

    if primitives.is_empty() {
        min = Vec3::ZERO;
        max = Vec3::ZERO;
    }
    Some(ReadyAsset {
        primitives,
        bounds: Aabb::from_min_max(min, max),
    })
}

/// The `StandardMaterial` the glTF loader wrote beside a primitive's
/// `GltfMaterial`, under the same asset path with a `/std` label.
///
/// `None` for a primitive whose material has no path to hang that label on,
/// which the caller draws with the default material.
fn standard_material(
    server: &AssetServer,
    material: &Handle<bevy::gltf::GltfMaterial>,
) -> Option<Handle<StandardMaterial>> {
    let path = material.path()?;
    let label = path.label()?;
    Some(server.load(path.clone_owned().with_label(format!("{label}/std"))))
}

/// Respawn the chunks a terrain's dirty regions name.
fn rebuild_chunks(
    mut commands: Commands,
    mut assets: ResMut<ScatterAssets>,
    mut terrains: Query<(
        Entity,
        &TerrainScatter,
        &mut ScatterDirty,
        Option<&Children>,
    )>,
    chunks: Query<&ScatterChunk>,
) {
    let settled = std::mem::take(&mut assets.settled);
    for (terrain, scatter, mut dirty, children) in &mut terrains {
        // An asset that has just resolved makes every chunk drawing it
        // stale, and a chunk that drew nothing is exactly the one waiting
        // for it.
        if !settled.is_empty()
            && scatter
                .palette
                .assets
                .iter()
                .any(|entry| settled.contains(&entry.asset))
        {
            dirty.all = true;
        }
        if dirty.is_clean() {
            continue;
        }

        let stale: Vec<Entity> = children
            .map(RelationshipTarget::iter)
            .into_iter()
            .flatten()
            .filter(|child| {
                chunks
                    .get(*child)
                    .is_ok_and(|chunk| dirty.all || dirty.regions.contains(&chunk.coord))
            })
            .collect();
        for entity in stale {
            commands.entity(entity).despawn();
        }

        for region in &scatter.regions {
            if !dirty.all && !dirty.regions.contains(&region.coord) {
                continue;
            }
            spawn_chunk(&mut commands, &assets, terrain, scatter, region);
        }

        *dirty = ScatterDirty::default();
    }
}

fn spawn_chunk(
    commands: &mut Commands,
    assets: &ScatterAssets,
    terrain: Entity,
    scatter: &TerrainScatter,
    region: &ScatterRegion,
) {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    let mut instances = Vec::new();

    for (index, placement) in region.placements.iter().enumerate() {
        let Some(entry) = scatter.palette.asset(placement.asset) else {
            continue;
        };
        let Some(ScatterAsset::Ready(ready)) = assets.entries.get(&entry.asset) else {
            continue;
        };
        let stand = Transform {
            translation: region.origin + placement.offset(),
            rotation: Quat::from_rotation_y(placement.yaw),
            scale: Vec3::splat(placement.scale),
        };
        let range = cull_range(entry.cull_distance, ready.height() * placement.scale);
        for primitive in &ready.primitives {
            instances.push((
                (
                    Mesh3d(primitive.mesh.clone()),
                    MeshMaterial3d(primitive.material.clone()),
                    stand * primitive.local,
                    ScatterRendered {
                        region: region.coord,
                        index,
                    },
                ),
                range.clone(),
            ));
        }
        let affine = stand.compute_affine();
        let centre = affine.transform_point3(Vec3::from(ready.bounds.center));
        let radius = affine
            .matrix3
            .abs()
            .mul_vec3(Vec3::from(ready.bounds.half_extents));
        min = min.min(centre - radius);
        max = max.max(centre + radius);
    }

    if instances.is_empty() {
        return;
    }
    let bounds = Aabb::from_min_max(min, max);
    commands
        .spawn((
            ScatterChunk {
                terrain,
                coord: region.coord,
                bounds,
            },
            Transform::IDENTITY,
            Visibility::default(),
            ChildOf(terrain),
        ))
        .with_children(|chunk| {
            for (instance, range) in instances {
                let mut drawn = chunk.spawn(instance);
                if let Some(range) = range {
                    drawn.insert(range);
                }
            }
        });
}

/// How far a placement of this asset draws, or `None` for one that draws at
/// every distance.
///
/// A palette entry that states a cutoff is taken at its word. One that does not
/// gets the ground-cover default when the asset is short enough to be ground
/// cover, and no cutoff otherwise; nothing is attached in that case, which keeps
/// the instance out of the render world's visibility-range bookkeeping.
///
/// The cutoff fades rather than snaps, so a batch of ground cover does not pop
/// in as the camera creeps over the distance.
fn cull_range(cull_distance: f32, height: f32) -> Option<VisibilityRange> {
    let distance = if cull_distance > 0.0 {
        cull_distance
    } else if height > 0.0 && height <= GROUND_COVER_HEIGHT {
        GROUND_COVER_CULL_DISTANCE
    } else {
        return None;
    };
    if !distance.is_finite() {
        return None;
    }
    let margin = distance * CULL_FADE;
    Some(VisibilityRange {
        start_margin: 0.0..0.0,
        end_margin: (distance - margin)..distance,
        use_aabb: false,
    })
}

/// Hide the chunks no camera can see.
///
/// Bevy's own frustum test is per entity and skips the children of a hidden
/// parent, so this keeps the cost proportional to the regions in view.
fn cull_chunks(
    mut chunks: Query<(&ScatterChunk, &mut Visibility)>,
    terrains: Query<&GlobalTransform>,
    cameras: Query<(&Frustum, &Camera)>,
) {
    let frustums: Vec<&Frustum> = cameras
        .iter()
        .filter(|(_, camera)| camera.is_active)
        .map(|(frustum, _)| frustum)
        .collect();
    if frustums.is_empty() {
        return;
    }
    for (chunk, mut visibility) in &mut chunks {
        let world_from_local = terrains
            .get(chunk.terrain)
            .map_or(Affine3A::IDENTITY, GlobalTransform::affine);
        let seen = frustums
            .iter()
            .any(|frustum| frustum.intersects_obb(&chunk.bounds, &world_from_local, true, false));
        let wanted = if seen {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != wanted {
            *visibility = wanted;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::placement::ScatterPaletteEntry;
    use crate::region::{RegionSize, TerrainRegions};

    fn document() -> RegionTerrainData {
        let mut data = RegionTerrainData {
            regions: TerrainRegions::new(RegionSize::new(4).unwrap()),
            scatter: ScatterPalette {
                assets: vec![ScatterPaletteEntry::new("models/tree.gltf")],
                groups: vec!["woods".to_string()],
            },
            ..RegionTerrainData::default()
        };
        data.regions.set_height(0, 0, 0.0);
        data.add_placement(Vec3::new(1.0, 0.5, 2.0), 0, 0, 0.25, 1.0)
            .expect("the region is allocated");
        data.add_placement(Vec3::new(3.0, 0.5, 1.0), 0, 0, 0.0, 2.0)
            .expect("the region is allocated");
        data
    }

    #[test]
    fn a_projection_carries_every_placement_in_terrain_local_space() {
        let data = document();
        let scatter = TerrainScatter::from_document(&data);
        assert_eq!(scatter.placement_count(), 2);
        let region = &scatter.regions[0];
        assert_eq!(region.origin, Vec3::ZERO);
        assert_eq!(
            region.origin + region.placements[0].offset(),
            Vec3::new(1.0, 0.5, 2.0)
        );
    }

    #[test]
    fn a_document_with_no_placements_projects_to_nothing_to_draw() {
        let data = RegionTerrainData::default();
        assert!(TerrainScatter::from_document(&data).is_empty());
    }

    /// A chunk holds one entity per primitive per placement, and each is
    /// only what a draw needs: no name, and nothing a document or an
    /// outliner could pick up.
    #[test]
    fn a_chunk_spawns_one_entity_per_primitive_per_placement_and_names_none_of_them() {
        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>();

        let (bark, leaves) = {
            let meshes = app.world_mut().resource_mut::<Assets<Mesh>>();
            (meshes.reserve_handle(), meshes.reserve_handle())
        };
        let material = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .reserve_handle();
        let mut assets = ScatterAssets::default();
        assets.entries.insert(
            "models/tree.gltf".to_string(),
            ScatterAsset::Ready(ReadyAsset {
                primitives: vec![
                    ScatterPrimitive {
                        mesh: bark,
                        material: material.clone(),
                        local: Transform::IDENTITY,
                    },
                    ScatterPrimitive {
                        mesh: leaves,
                        material,
                        local: Transform::from_xyz(0.0, 2.0, 0.0),
                    },
                ],
                bounds: Aabb::from_min_max(Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 4.0, 1.0)),
            }),
        );
        app.insert_resource(assets);

        let scatter = TerrainScatter::from_document(&document());
        let placements = scatter.placement_count();
        let terrain = app
            .world_mut()
            .spawn((
                Transform::IDENTITY,
                Visibility::default(),
                scatter,
                ScatterDirty::all(),
            ))
            .id();
        app.add_systems(Update, rebuild_chunks);
        app.update();

        let chunks: Vec<Entity> = app
            .world_mut()
            .query_filtered::<Entity, With<ScatterChunk>>()
            .iter(app.world())
            .collect();
        assert_eq!(chunks.len(), 1, "one chunk per region holding placements");
        assert_eq!(
            app.world().get::<ChildOf>(chunks[0]).map(ChildOf::parent),
            Some(terrain)
        );

        let drawn: Vec<Entity> = app
            .world()
            .get::<Children>(chunks[0])
            .map(|children| children.iter().collect())
            .unwrap_or_default();
        assert_eq!(drawn.len(), placements * 2, "one entity per primitive");
        for entity in drawn {
            assert!(app.world().get::<Mesh3d>(entity).is_some());
            assert!(app.world().get::<ScatterRendered>(entity).is_some());
            assert!(
                app.world().get::<Name>(entity).is_none(),
                "a drawn placement is not a named scene node"
            );
        }
    }

    /// A region marked dirty is the only one respawned, so a stroke costs
    /// its own chunk rather than the whole terrain's.
    #[test]
    fn only_a_dirty_region_is_rebuilt() {
        let mut dirty = ScatterDirty::default();
        assert!(dirty.is_clean());
        dirty.touch(RegionCoord::new(1, 2));
        assert!(!dirty.is_clean());
        assert!(!dirty.all);
        assert!(ScatterDirty::all().all);
    }

    #[test]
    fn ground_cover_fades_out_and_anything_taller_draws_at_every_distance() {
        let cover = cull_range(0.0, 0.4).expect("ground cover has a cutoff");
        assert_eq!(cover.end_margin.end, GROUND_COVER_CULL_DISTANCE);
        assert!(
            cover.end_margin.start < cover.end_margin.end,
            "the cutoff fades rather than snaps"
        );
        assert!(
            cull_range(0.0, 6.0).is_none(),
            "nothing is attached when there is no cutoff"
        );
        assert_eq!(
            cull_range(25.0, 6.0)
                .expect("a stated cutoff")
                .end_margin
                .end,
            25.0
        );
    }
}
