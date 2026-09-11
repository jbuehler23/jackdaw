//! Drawing a terrain's detail layers as one instanced call per layer per tile.
//! A host writes [`TerrainDetailSource`] and [`DetailDirty`] onto the terrain.

use std::sync::Arc;

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::primitives::Aabb;
use bevy::core_pipeline::core_3d::{Opaque3d, Opaque3dBatchSetKey, Opaque3dBinKey};
use bevy::ecs::query::QueryItem;
use bevy::ecs::system::SystemParamItem;
use bevy::ecs::system::lifetimeless::{Read, SRes};
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::math::{Affine3A, Mat3A};
use bevy::mesh::{
    Indices, MeshVertexAttribute, MeshVertexBufferLayoutRef, PrimitiveTopology, VertexBufferLayout,
};
use bevy::pbr::{
    MeshPipeline, MeshPipelineKey, MeshPipelineSystems, RenderMeshInstances, SetMeshBindGroup,
    SetMeshViewBindGroup, SetMeshViewBindingArrayBindGroup,
};
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::mesh::allocator::MeshAllocator;
use bevy::render::mesh::{RenderMesh, RenderMeshBufferInfo};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_phase::{
    AddRenderCommand, BinnedRenderPhase, BinnedRenderPhaseType, DrawFunctions, InputUniformIndex,
    PhaseItem, RenderCommand, RenderCommandResult, SetItemPipeline, TrackedRenderPass,
    ViewBinnedRenderPhases,
};
use bevy::render::render_resource::{
    AsBindGroup, BindGroup, BindGroupLayoutDescriptor, BufferUsages, Extent3d, PipelineCache,
    RawBufferVec, RenderPipelineDescriptor, SpecializedMeshPipeline, SpecializedMeshPipelineError,
    SpecializedMeshPipelines, TextureDimension, TextureFormat, VertexAttribute, VertexFormat,
    VertexStepMode,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::storage::GpuShaderBuffer;
use bevy::render::sync_component::SyncComponent;
use bevy::render::sync_world::MainEntity;
use bevy::render::texture::{FallbackImage, GpuImage};
use bevy::render::view::ExtractedView;
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::Shader;
use jackdaw_scene_types::{DetailLayer, DetailMesh, DetailPresser, NavmeshExclude, Terrain};

use crate::channel::ChannelElement;
use crate::detail::{
    DetailInstance, DetailLod, detail_lod_at, detail_tiles_around, place_detail,
    tile_centre_distance, tileable_value_noise,
};
use crate::heightmap::Heightmap;
use crate::rect::GridRect;
use crate::sidecar::{GridShape, RegionTerrainData};

use super::scatter::{ScatterAssets, ScatterPrimitive, ScatterSystems};

const SHADER_PATH: &str = "embedded://jackdaw_terrain/render/shaders/detail.wgsl";

/// Pressers the shader reads. Past this the nearest to the viewer win.
pub const MAX_DETAIL_PRESSERS: usize = 16;

/// Grid cells along one edge of a detail tile.
pub const DETAIL_TILE_CELLS: u32 = 16;

/// Tiles seeded per frame.
pub const DETAIL_TILE_BUDGET: usize = 2;

/// How far up its own mesh a vertex stands, in `0..1`. The shader bends and
/// shades by it.
pub const ATTRIBUTE_HEIGHT_FRACTION: MeshVertexAttribute =
    MeshVertexAttribute::new("HeightFraction", 1_903_552_411, VertexFormat::Float32);

/// Card segments at each detail level.
const NEAR_SEGMENTS: u32 = 3;
const FAR_SEGMENTS: u32 = 1;

/// Edge of the generated wind texture, in texels.
const WIND_TEXTURE_SIZE: u32 = 32;
/// Lattice cells across that texture, which wraps on the lattice.
const WIND_LATTICE: f32 = 4.0;

/// What a terrain's detail is grown from: its ground, and one entry per layer.
/// A projection of the terrain's document and component, shared in one `Arc`.
#[derive(Component, Clone, Debug)]
pub struct TerrainDetailSource(Arc<DetailSource>);

/// The body of a [`TerrainDetailSource`].
#[derive(Debug)]
pub struct DetailSource {
    /// The ground the instances stand on.
    pub heightmap: Heightmap,
    /// Where the grid sits and how far it reaches.
    pub grid: GridShape,
    /// One entry per layer, in the terrain's own order.
    pub layers: Vec<DetailLayerSource>,
}

/// What one layer is grown from.
#[derive(Debug)]
pub struct DetailLayerSource {
    /// The density channel as a dense row-major plane at the grid's resolution.
    pub density: Vec<u16>,
    /// The density channel's ceiling: a cell reading this is full coverage.
    pub max: u16,
    /// How the layer reads.
    pub layer: DetailLayer,
    /// What the placement is seeded from: the grid's placement and the density
    /// channel's name.
    pub seed: u64,
}

impl TerrainDetailSource {
    /// The detail a document and its terrain component grow between them, or
    /// `None` for a terrain with no layers.
    pub fn from_document(data: &RegionTerrainData, terrain: &Terrain) -> Option<Self> {
        if terrain.detail.is_empty() {
            return None;
        }
        let grid = data.grid_shape(terrain.size, terrain.resolution);
        let cells = (grid.resolution as usize) * (grid.resolution as usize);
        let layers = terrain
            .detail
            .iter()
            .map(|layer| {
                let index = data
                    .channels
                    .iter()
                    .position(|channel| channel.name == layer.density_channel);
                let (density, max) = match index {
                    Some(index) => (
                        data.regions.read_grid_channel(index, grid.resolution),
                        data.channels[index].element.max_value(),
                    ),
                    None => (vec![0; cells], ChannelElement::U8.max_value()),
                };
                DetailLayerSource {
                    density,
                    max,
                    seed: seed_of(&grid, &layer.density_channel),
                    layer: layer.clone(),
                }
            })
            .collect();

        let mut heights = Vec::new();
        data.regions
            .sample_grid_heights_into(grid.resolution, &mut heights);
        Some(Self(Arc::new(DetailSource {
            heightmap: Heightmap {
                resolution: grid.resolution,
                size: grid.size,
                origin: grid.origin,
                max_height: terrain.max_height,
                heights,
            },
            grid,
            layers,
        })))
    }

    /// What the tiles are seeded from.
    pub fn source(&self) -> &DetailSource {
        &self.0
    }

    /// World units per grid cell edge.
    pub fn cell_size(&self) -> f32 {
        let grid = &self.0.grid;
        grid.size.x / (grid.resolution.max(2) - 1) as f32
    }
}

/// A seed stable across runs: the ground the grid covers, plus the name of
/// the channel that grows the layer.
fn seed_of(grid: &GridShape, density_channel: &str) -> u64 {
    let mut seed = u64::from(grid.resolution);
    for word in [
        grid.origin.x.to_bits(),
        grid.origin.y.to_bits(),
        grid.size.x.to_bits(),
    ] {
        seed = seed.rotate_left(17) ^ u64::from(word);
    }
    for byte in density_channel.bytes() {
        seed = seed.rotate_left(7) ^ u64::from(byte);
    }
    seed
}

/// Which of a terrain's ground the renderer has yet to catch up with. The rect
/// covers the cells edits have touched; `None` is ground in step.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct DetailDirty {
    pub rect: Option<GridRect>,
}

impl DetailDirty {
    /// Mark the ground a rect covers stale. An uncaught mark widens to cover
    /// both rather than being replaced.
    pub fn touch(&mut self, rect: GridRect) {
        self.rect = Some(match self.rect {
            Some(held) => union(held, rect),
            None => rect,
        });
    }
}

/// The smallest rect covering both.
fn union(a: GridRect, b: GridRect) -> GridRect {
    let x = a.x.min(b.x);
    let z = a.z.min(b.z);
    let max_x = (a.x + a.width).max(b.x + b.width);
    let max_z = (a.z + a.height).max(b.z + b.height);
    GridRect {
        x,
        z,
        width: max_x - x,
        height: max_z - z,
    }
}

/// One seeded tile of one layer, and every instance standing on it.
#[derive(Component, Clone, Debug)]
pub struct DetailTile {
    /// The terrain this grew from, whose layers the instances draw with.
    pub terrain: Entity,
    /// Which of that terrain's layers this tile belongs to.
    pub layer: usize,
    /// Which tile of that terrain's grid this is.
    pub tile: IVec2,
    pub lod: DetailLod,
    /// The instances, in world space.
    pub instances: Arc<Vec<DetailInstance>>,
    /// What the instances occupy, for the frustum test.
    pub bounds: Aabb,
}

impl SyncComponent for DetailTile {
    type Target = Self;
}

impl ExtractComponent for DetailTile {
    type QueryData = &'static DetailTile;
    type QueryFilter = ();
    type Out = Self;

    fn extract_component(item: QueryItem<'_, '_, Self::QueryData>) -> Option<Self> {
        (!item.instances.is_empty()).then(|| item.clone())
    }
}

/// How far and how thickly detail draws, over what every layer declares.
/// Not persisted.
#[derive(Resource, Clone, Copy, Debug)]
pub struct DetailSettings {
    /// Multiplies every layer's instances per square metre.
    pub density_scale: f32,
    /// Multiplies every layer's cull distance.
    pub cull_scale: f32,
}

impl Default for DetailSettings {
    fn default() -> Self {
        Self {
            density_scale: 1.0,
            cull_scale: 1.0,
        }
    }
}

/// What the detail is currently bending away from, nearest the viewer first.
/// Filled from [`DetailPresser`] entities, capped at [`MAX_DETAIL_PRESSERS`].
#[derive(Resource, Clone, Debug, Default)]
pub struct DetailPressers {
    /// World position and reach, one entry per presser.
    pub pressers: Vec<(Vec3, f32)>,
}

/// The card meshes and the two shared textures, used by every terrain.
#[derive(Resource, Clone, Debug)]
pub struct DetailAssets {
    pub near_card: Handle<Mesh>,
    pub far_card: Handle<Mesh>,
    pub wind_noise: Handle<Image>,
    /// The one-texel white a layer with no texture of its own multiplies by.
    pub white: Handle<Image>,
}

impl DetailAssets {
    fn card(&self, lod: DetailLod) -> Handle<Mesh> {
        match lod {
            DetailLod::Near => self.near_card.clone(),
            DetailLod::Far => self.far_card.clone(),
        }
    }
}

/// What one asset path flattened to, keyed by that path.
#[derive(Resource, Default, Debug)]
pub struct DetailMeshes(HashMap<String, BuiltDetailMesh>);

/// One glTF, merged into the single mesh a layer instances.
#[derive(Clone, Debug)]
pub struct BuiltDetailMesh {
    pub mesh: Handle<Mesh>,
    /// The base colour texture the file's first textured primitive carries.
    pub color: Option<Handle<Image>>,
}

impl DetailMeshes {
    /// What a layer's asset path draws, or `None` until it has resolved.
    pub fn get(&self, asset: &str) -> Option<&BuiltDetailMesh> {
        self.0.get(asset)
    }
}

/// The bind group one layer of one terrain draws with. A plain
/// [`AsBindGroup`] rather than a `Material`, with a pipeline of its own.
#[derive(AsBindGroup, Clone, Debug)]
pub struct DetailBindings {
    /// Linear colour at the foot of an instance. `w` is unused.
    #[uniform(0)]
    pub color_base: Vec4,
    /// Linear colour at the top of an instance. `w` is unused.
    #[uniform(0)]
    pub color_tip: Vec4,
    #[uniform(0)]
    pub wind_direction: Vec2,
    #[uniform(0)]
    pub wind_speed: f32,
    #[uniform(0)]
    pub wind_strength: f32,
    #[uniform(0)]
    pub wind_vertical_strength: f32,
    #[uniform(0)]
    pub wind_tile_size: f32,
    #[uniform(0)]
    pub bend: f32,
    #[uniform(0)]
    pub push_strength: f32,
    /// Shortest and tallest an instance stands, in world units.
    #[uniform(0)]
    pub height_range: Vec2,
    /// Narrowest and widest an instance is drawn, over the mesh's own width.
    #[uniform(0)]
    pub width_range: Vec2,
    #[uniform(0)]
    pub cull_distance: f32,
    #[uniform(0)]
    pub presser_count: u32,
    /// Whether the layer draws the built-in card, whose taper the fragment
    /// stage carves.
    #[uniform(0)]
    pub is_card: u32,
    /// `xyz` is a presser's world position, `w` how far it flattens detail.
    #[uniform(0)]
    pub pressers: [Vec4; MAX_DETAIL_PRESSERS],
    #[texture(1)]
    #[sampler(2)]
    pub wind_noise: Handle<Image>,
    #[texture(3)]
    #[sampler(4)]
    pub color_texture: Handle<Image>,
}

impl DetailBindings {
    /// The bindings one layer's look and the pressers around it come to.
    pub fn new(
        layer: &DetailLayer,
        settings: &DetailSettings,
        pressers: &DetailPressers,
        wind_noise: Handle<Image>,
        color_texture: Handle<Image>,
    ) -> Self {
        let mut packed = [Vec4::ZERO; MAX_DETAIL_PRESSERS];
        for (slot, (position, radius)) in packed.iter_mut().zip(&pressers.pressers) {
            *slot = position.extend(radius * layer.push_radius);
        }
        Self {
            color_base: linear_of(layer.color_base),
            color_tip: linear_of(layer.color_tip),
            wind_direction: Vec2::from(layer.wind_direction).normalize_or(Vec2::X),
            wind_speed: layer.wind_speed,
            wind_strength: layer.wind_strength,
            wind_vertical_strength: layer.wind_vertical_strength,
            wind_tile_size: layer.wind_tile_size,
            bend: layer.bend,
            push_strength: layer.push_strength,
            height_range: Vec2::new(layer.height[0], layer.height[1]),
            width_range: Vec2::new(layer.width[0], layer.width[1]),
            cull_distance: layer.cull_distance * settings.cull_scale,
            presser_count: pressers.pressers.len().min(MAX_DETAIL_PRESSERS) as u32,
            is_card: u32::from(layer.mesh == DetailMesh::Card),
            pressers: packed,
            wind_noise,
            color_texture,
        }
    }
}

/// Which terrain's layer a bind group belongs to.
pub type DetailKey = (Entity, usize);

/// The bindings every layer currently drawing is asking for.
#[derive(Resource, Clone, Debug, Default, ExtractResource)]
pub struct DetailLooks(pub Vec<(DetailKey, DetailBindings)>);

/// A card of `segments` stacked quads, one unit tall and wide, its foot at the
/// origin and its face on +Z. Straight-sided; the fragment stage carves it.
pub fn card_mesh(segments: u32) -> Mesh {
    let segments = segments.max(1);
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut fractions = Vec::new();
    let mut indices = Vec::new();

    for row in 0..=segments {
        let v = row as f32 / segments as f32;
        for u in [0.0f32, 1.0] {
            positions.push([u - 0.5, v, 0.0]);
            normals.push([0.0, 0.0, 1.0]);
            uvs.push([u, v]);
            fractions.push(v);
        }
    }
    for row in 0..segments {
        let bottom = row * 2;
        indices.extend_from_slice(&[
            bottom,
            bottom + 1,
            bottom + 3,
            bottom,
            bottom + 3,
            bottom + 2,
        ]);
    }

    let readable_on_the_main_world = RenderAssetUsages::default();
    Mesh::new(PrimitiveTopology::TriangleList, readable_on_the_main_world)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_attribute(ATTRIBUTE_HEIGHT_FRACTION, fractions)
        .with_inserted_indices(Indices::U32(indices))
}

/// A tileable value-noise texture for the shader to read the wind from.
/// Sampled at world coordinates over a tile size, so the sampler repeats.
pub fn wind_noise_image() -> Image {
    let side = WIND_TEXTURE_SIZE;
    let scale = WIND_LATTICE / side as f32;
    let mut data = Vec::with_capacity((side * side) as usize);
    for z in 0..side {
        for x in 0..side {
            let at = Vec2::new(x as f32, z as f32) * scale;
            let value = tileable_value_noise(at, WIND_LATTICE);
            data.push((value.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: side,
            height: side,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        ..ImageSamplerDescriptor::linear()
    });
    image
}

/// One opaque white texel, which a layer with no texture multiplies by.
pub fn white_image() -> Image {
    Image::new(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        vec![255, 255, 255, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    )
}

/// Seeds and retires the tiles around the viewer, and draws them. Independent
/// of [`super::TerrainRenderPlugin`]; either can be added alone.
pub struct DetailRenderPlugin;

/// The stages a host orders its own detail work against.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DetailSystems {
    /// Reseeding the tiles a dirty mark names and the ones the viewer has
    /// walked into. A host that writes [`TerrainDetailSource`] runs before this.
    Rebuild,
}

impl Plugin for DetailRenderPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "shaders/detail.wgsl");
        super::scatter::add_asset_plugin(app);
        if !app.world().contains_resource::<Assets<Mesh>>() {
            app.init_asset::<Mesh>();
        }
        app.init_resource::<DetailSettings>()
            .init_resource::<DetailPressers>()
            .init_resource::<DetailLooks>()
            .init_resource::<DetailMeshes>()
            .add_plugins((
                ExtractComponentPlugin::<DetailTile>::extract_visible(),
                ExtractResourcePlugin::<DetailLooks>::default(),
            ))
            .add_systems(Startup, init_detail_assets)
            .add_systems(
                Update,
                (
                    request_detail_assets.before(ScatterSystems::Resolve),
                    (
                        build_detail_meshes,
                        collect_detail_pressers,
                        rebuild_detail_tiles,
                        build_detail_looks,
                    )
                        .chain()
                        .after(ScatterSystems::Resolve),
                )
                    .in_set(DetailSystems::Rebuild)
                    .run_if(resource_exists::<DetailAssets>),
            );

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<SpecializedMeshPipelines<DetailPipeline>>()
            .init_resource::<DetailBindGroups>()
            .init_resource::<QueuedDetailTiles>()
            .add_render_command::<Opaque3d, DrawDetail>()
            .add_systems(
                RenderStartup,
                init_detail_pipeline.after(MeshPipelineSystems),
            )
            .add_systems(
                Render,
                (
                    prepare_detail_buffers.in_set(RenderSystems::PrepareResources),
                    prepare_detail_bind_groups.in_set(RenderSystems::PrepareBindGroups),
                    queue_detail_tiles.in_set(RenderSystems::QueueMeshes),
                ),
            );
    }
}

/// Build the card meshes and the shared textures once, for every layer to use.
fn init_detail_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
) {
    commands.insert_resource(DetailAssets {
        near_card: meshes.add(card_mesh(NEAR_SEGMENTS)),
        far_card: meshes.add(card_mesh(FAR_SEGMENTS)),
        wind_noise: images.add(wind_noise_image()),
        white: images.add(white_image()),
    });
}

/// Start loading the glTF every asset-backed layer names.
fn request_detail_assets(
    mut assets: ResMut<ScatterAssets>,
    server: Res<AssetServer>,
    terrains: Query<&TerrainDetailSource, Changed<TerrainDetailSource>>,
) {
    for source in &terrains {
        for entry in &source.source().layers {
            if let DetailMesh::Asset(path) = &entry.layer.mesh {
                assets.request(&server, path);
            }
        }
    }
}

/// Merge the primitives of every asset-backed layer whose glTF has resolved.
fn build_detail_meshes(
    mut built: ResMut<DetailMeshes>,
    scatter: Res<ScatterAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    materials: Res<Assets<StandardMaterial>>,
    terrains: Query<&TerrainDetailSource>,
) {
    let mut wanted: Vec<&String> = Vec::new();
    for source in &terrains {
        for entry in &source.source().layers {
            if let DetailMesh::Asset(path) = &entry.layer.mesh
                && !built.0.contains_key(path)
                && !wanted.contains(&path)
            {
                wanted.push(path);
            }
        }
    }
    for path in wanted {
        let Some(primitives) = scatter.primitives(path) else {
            continue;
        };
        let Some((merged, color)) = merge_primitives(path, primitives, &meshes, &materials) else {
            continue;
        };
        let mesh = meshes.add(merged);
        built
            .0
            .insert(path.clone(), BuiltDetailMesh { mesh, color });
    }
}

/// The matrix that carries a normal through `affine`: the inverse transpose,
/// so that a non-uniform scale tilts the normal the way the surface turns.
fn normal_matrix_of(affine: Affine3A) -> Mat3A {
    affine.matrix3.inverse().transpose()
}

/// Every primitive of a flattened glTF as one mesh, with the base colour
/// texture the first textured primitive carries. `None` while any is loading.
fn merge_primitives(
    asset: &str,
    primitives: &[ScatterPrimitive],
    meshes: &Assets<Mesh>,
    materials: &Assets<StandardMaterial>,
) -> Option<(Mesh, Option<Handle<Image>>)> {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut color = None;
    let mut textures: Vec<Option<Handle<Image>>> = Vec::new();

    for primitive in primitives {
        if primitive.material.path().is_some() {
            let material = materials.get(&primitive.material)?;
            let texture = material.base_color_texture.clone();
            color = color.or_else(|| texture.clone());
            if !textures.contains(&texture) {
                textures.push(texture);
            }
        }
        let mesh = meshes.get(&primitive.mesh)?;
        let Some(source) = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(|values| values.as_float3())
        else {
            continue;
        };
        let affine = primitive.local.compute_affine();
        let normal_matrix = normal_matrix_of(affine);
        let base = positions.len() as u32;

        let source_normals = mesh
            .attribute(Mesh::ATTRIBUTE_NORMAL)
            .and_then(|values| values.as_float3());
        let source_uvs = match mesh.attribute(Mesh::ATTRIBUTE_UV_0) {
            Some(bevy::mesh::VertexAttributeValues::Float32x2(values)) => Some(values.as_slice()),
            _ => None,
        };
        for (vertex, point) in source.iter().enumerate() {
            positions.push(affine.transform_point3(Vec3::from(*point)).to_array());
            let normal = source_normals
                .and_then(|values| values.get(vertex))
                .map(|normal| normal_matrix.mul_vec3(Vec3::from(*normal)))
                .unwrap_or(Vec3::Y);
            normals.push(normal.normalize_or(Vec3::Y).to_array());
            uvs.push(
                source_uvs
                    .and_then(|values| values.get(vertex))
                    .copied()
                    .unwrap_or([0.0, 0.0]),
            );
        }

        match mesh.indices() {
            Some(read) => indices.extend(read.iter().map(|index| base + index as u32)),
            None => indices.extend(base..base + source.len() as u32),
        }
    }

    if positions.is_empty() {
        return None;
    }
    if textures.len() > 1 {
        warn!(
            "terrain detail: {asset} carries more than one base colour texture; \
             the whole layer samples the first, so every part shares that look"
        );
    }
    let lowest = positions
        .iter()
        .map(|point| point[1])
        .fold(f32::MAX, f32::min);
    let highest = positions
        .iter()
        .map(|point| point[1])
        .fold(f32::MIN, f32::max);
    let span = highest - lowest;
    let fractions: Vec<f32> = positions
        .iter()
        .map(|point| match span > f32::EPSILON {
            true => ((point[1] - lowest) / span).clamp(0.0, 1.0),
            false => 0.0,
        })
        .collect();

    let merged = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_attribute(ATTRIBUTE_HEIGHT_FRACTION, fractions)
    .with_inserted_indices(Indices::U32(indices));
    Some((merged, color))
}

/// Cameras the field is laid out around, as both detail systems ask for them.
type DetailViewers<'w, 's> =
    Query<'w, 's, (Entity, &'static Camera, &'static GlobalTransform), With<Camera3d>>;

/// Where the field is centred: the active 3D camera with the highest order.
fn viewer_position(cameras: &DetailViewers<'_, '_>) -> Option<Vec3> {
    cameras
        .iter()
        .filter(|(_, camera, _)| camera.is_active)
        .max_by_key(|(entity, camera, _)| (camera.order, *entity))
        .map(|(_, _, transform)| transform.translation())
}

/// Gather the pressers nearest the viewer.
fn collect_detail_pressers(
    mut pressers: ResMut<DetailPressers>,
    viewers: DetailViewers,
    pressing: Query<(&GlobalTransform, &DetailPresser)>,
) {
    let viewer = viewer_position(&viewers).unwrap_or(Vec3::ZERO);
    let mut found: Vec<(Vec3, f32)> = pressing
        .iter()
        .map(|(transform, presser)| (transform.translation(), presser.radius))
        .collect();
    found.sort_by(|a, b| {
        a.0.distance_squared(viewer)
            .total_cmp(&b.0.distance_squared(viewer))
    });
    found.truncate(MAX_DETAIL_PRESSERS);
    if pressers.pressers != found {
        pressers.pressers = found;
    }
}

/// The mesh one layer instances, or `None` while its asset is still resolving.
fn layer_mesh(
    assets: &DetailAssets,
    built: &DetailMeshes,
    layer: &DetailLayer,
    lod: DetailLod,
) -> Option<Handle<Mesh>> {
    match &layer.mesh {
        DetailMesh::Card => Some(assets.card(lod)),
        DetailMesh::Asset(path) => built.get(path).map(|built| built.mesh.clone()),
    }
}

/// Reseed the tiles around the viewer, layer by layer. Retiring is unbudgeted;
/// seeding is capped at [`DETAIL_TILE_BUDGET`] a frame.
fn rebuild_detail_tiles(
    mut commands: Commands,
    assets: Res<DetailAssets>,
    built: Res<DetailMeshes>,
    settings: Res<DetailSettings>,
    viewers: DetailViewers,
    mut terrains: Query<(
        Entity,
        &TerrainDetailSource,
        &GlobalTransform,
        &mut DetailDirty,
    )>,
    tiles: Query<(Entity, &DetailTile)>,
) {
    for (tile_entity, tile) in &tiles {
        let layers = terrains
            .get(tile.terrain)
            .map(|(_, source, _, _)| source.source().layers.len())
            .unwrap_or(0);
        if tile.layer >= layers {
            commands.entity(tile_entity).despawn();
        }
    }

    let Some(viewer) = viewer_position(&viewers) else {
        return;
    };

    for (entity, source, placed, mut dirty) in &mut terrains {
        let cell_size = source.cell_size();
        if cell_size <= 0.0 || !cell_size.is_finite() {
            continue;
        }
        let world_from_local = placed.affine();
        let local = world_from_local.inverse().transform_point3(viewer);
        let grid = &source.source().grid;
        let viewer_cell = ((Vec2::new(local.x, local.z) - grid.origin) / cell_size)
            .round()
            .as_ivec2();
        let stale = std::mem::take(&mut dirty.rect);
        let mut spent = 0;

        for (index, entry) in source.source().layers.iter().enumerate() {
            let cull_cells = entry.layer.cull_distance * settings.cull_scale / cell_size;
            let wanted = detail_tiles_around(viewer_cell, cull_cells, DETAIL_TILE_CELLS);

            let mut standing = HashSet::new();
            for (tile_entity, tile) in &tiles {
                if tile.terrain != entity || tile.layer != index {
                    continue;
                }
                let level = detail_lod_at(
                    tile_centre_distance(viewer_cell, tile.tile, DETAIL_TILE_CELLS),
                    cull_cells,
                    Some(tile.lod),
                );
                let retired = !wanted.iter().any(|(coord, _)| *coord == tile.tile)
                    || level != tile.lod
                    || stale.is_some_and(|rect| touches(rect, tile.tile));
                if retired {
                    commands.entity(tile_entity).despawn();
                } else {
                    standing.insert(tile.tile);
                }
            }

            for (coord, lod) in wanted {
                if spent >= DETAIL_TILE_BUDGET {
                    break;
                }
                if standing.contains(&coord) {
                    continue;
                }
                let Some(mesh) = layer_mesh(&assets, &built, &entry.layer, lod) else {
                    break;
                };
                spent += 1;
                let instances = seed_tile(source, &settings, index, coord, lod, world_from_local);
                let bounds = match instances.is_empty() {
                    true => footprint(source, coord, world_from_local),
                    false => tile_bounds(&instances, &entry.layer),
                };
                commands.spawn((
                    DetailTile {
                        terrain: entity,
                        layer: index,
                        tile: coord,
                        lod,
                        instances: Arc::new(instances),
                        bounds,
                    },
                    Mesh3d(mesh),
                    Transform::IDENTITY,
                    Visibility::default(),
                    bounds,
                    NavmeshExclude,
                ));
            }
        }
    }
}

/// Whether a tile covers any of a dirty rect.
fn touches(rect: GridRect, tile: IVec2) -> bool {
    let min = tile * DETAIL_TILE_CELLS as i32;
    let max = min + IVec2::splat(DETAIL_TILE_CELLS as i32);
    let rect_max = IVec2::new((rect.x + rect.width) as i32, (rect.z + rect.height) as i32);
    min.x < rect_max.x && max.x > rect.x as i32 && min.y < rect_max.y && max.y > rect.z as i32
}

/// Seed one tile of one layer and lift its instances into world space.
fn seed_tile(
    source: &TerrainDetailSource,
    settings: &DetailSettings,
    index: usize,
    tile: IVec2,
    lod: DetailLod,
    world_from_local: Affine3A,
) -> Vec<DetailInstance> {
    let body = source.source();
    let Some(entry) = body.layers.get(index) else {
        return Vec::new();
    };
    let mut instances = place_detail(
        &entry.density,
        entry.max,
        &body.heightmap,
        &entry.layer,
        index,
        tile,
        DETAIL_TILE_CELLS,
        source.cell_size(),
        settings.density_scale,
        lod.density_scale(),
        entry.seed,
    );
    if world_from_local != Affine3A::IDENTITY {
        for instance in &mut instances {
            instance.position = world_from_local.transform_point3(instance.position);
        }
    }
    instances
}

/// The ground one tile covers, for a tile with no instances.
fn footprint(source: &TerrainDetailSource, tile: IVec2, world_from_local: Affine3A) -> Aabb {
    let cell_size = source.cell_size();
    let origin = source.source().grid.origin;
    let min = origin + tile.as_vec2() * DETAIL_TILE_CELLS as f32 * cell_size;
    let max = min + Vec2::splat(DETAIL_TILE_CELLS as f32 * cell_size);
    let corner = |at: Vec2| world_from_local.transform_point3(Vec3::new(at.x, 0.0, at.y));
    Aabb::from_min_max(corner(min), corner(max))
}

/// What a tile's instances occupy, widened for wind, bend and presser lean.
fn tile_bounds(instances: &[DetailInstance], layer: &DetailLayer) -> Aabb {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for instance in instances {
        min = min.min(instance.position);
        max = max.max(instance.position);
    }
    let tall = layer.height[0].max(layer.height[1]);
    let lean = layer.bend + layer.wind_strength + layer.push_strength;
    Aabb::from_min_max(
        min - Vec3::new(lean, tall, lean),
        max + Vec3::new(lean, tall, lean),
    )
}

/// Collect the bindings every layer currently drawing is asking for.
fn build_detail_looks(
    mut looks: ResMut<DetailLooks>,
    assets: Res<DetailAssets>,
    built: Res<DetailMeshes>,
    settings: Res<DetailSettings>,
    pressers: Res<DetailPressers>,
    terrains: Query<(Entity, &TerrainDetailSource)>,
) {
    looks.0.clear();
    for (entity, source) in &terrains {
        for (index, entry) in source.source().layers.iter().enumerate() {
            let color = match &entry.layer.mesh {
                DetailMesh::Card => None,
                DetailMesh::Asset(path) => built.get(path).and_then(|built| built.color.clone()),
            };
            looks.0.push((
                (entity, index),
                DetailBindings::new(
                    &entry.layer,
                    &settings,
                    &pressers,
                    assets.wind_noise.clone(),
                    color.unwrap_or_else(|| assets.white.clone()),
                ),
            ));
        }
    }
}

/// One tile's instances, on the GPU.
#[derive(Component)]
pub struct DetailInstanceBuffer {
    buffer: RawBufferVec<DetailInstance>,
    length: usize,
    /// Which `Arc` this was filled from; an unchanged tile is not re-uploaded.
    source: usize,
}

/// The bind group each layer of each terrain draws with.
#[derive(Resource, Default)]
pub struct DetailBindGroups(HashMap<DetailKey, BindGroup>);

/// The bin a tile was last placed in: its batch set key and its bin key.
type DetailBin = (Opaque3dBatchSetKey, Opaque3dBinKey);

/// The bin each view holds each tile in, a binned phase retaining what it is
/// given across frames.
#[derive(Resource, Default)]
struct QueuedDetailTiles(
    HashMap<bevy::render::view::RetainedViewEntity, HashMap<MainEntity, DetailBin>>,
);

/// Place a tile in `bin`, taking it out of the bin it was last placed in when
/// that differs.
fn rebin_tile(
    phase: &mut BinnedRenderPhase<Opaque3d>,
    last: Option<&DetailBin>,
    ids: (Entity, MainEntity),
    bin: DetailBin,
    uniform_index: InputUniformIndex,
) {
    if last.is_some_and(|last| *last != bin) {
        phase.remove(ids.1);
    }
    let draws_its_own_instances = BinnedRenderPhaseType::NonMesh;
    phase.add(bin.0, bin.1, ids, uniform_index, draws_its_own_instances);
}

/// The detail pipeline: the mesh pipeline with a per-instance vertex buffer, a
/// bind group of its own, and both stages replaced.
#[derive(Resource)]
pub struct DetailPipeline {
    shader: Handle<Shader>,
    mesh_pipeline: MeshPipeline,
    layout: BindGroupLayoutDescriptor,
}

fn init_detail_pipeline(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mesh_pipeline: Res<MeshPipeline>,
    render_device: Res<RenderDevice>,
) {
    commands.insert_resource(DetailPipeline {
        shader: assets.load(SHADER_PATH),
        mesh_pipeline: mesh_pipeline.clone(),
        layout: DetailBindings::bind_group_layout_descriptor(&render_device),
    });
}

/// The per-instance vertex buffer one [`DetailInstance`] is read through.
/// Locations start past the four the mesh carries.
pub fn detail_instance_layout() -> VertexBufferLayout {
    VertexBufferLayout {
        array_stride: size_of::<DetailInstance>() as u64,
        step_mode: VertexStepMode::Instance,
        attributes: vec![
            VertexAttribute {
                format: VertexFormat::Float32x3,
                offset: 0,
                shader_location: 4,
            },
            VertexAttribute {
                format: VertexFormat::Uint32,
                offset: VertexFormat::Float32x3.size(),
                shader_location: 5,
            },
            VertexAttribute {
                format: VertexFormat::Float32x2,
                offset: VertexFormat::Float32x3.size() + VertexFormat::Uint32.size(),
                shader_location: 6,
            },
        ],
    }
}

/// The four attributes every detail mesh carries, at the locations the shader
/// declares them.
fn detail_mesh_layout(
    layout: &MeshVertexBufferLayoutRef,
) -> Result<VertexBufferLayout, SpecializedMeshPipelineError> {
    Ok(layout.0.get_layout(&[
        Mesh::ATTRIBUTE_POSITION.at_shader_location(0),
        Mesh::ATTRIBUTE_NORMAL.at_shader_location(1),
        Mesh::ATTRIBUTE_UV_0.at_shader_location(2),
        ATTRIBUTE_HEIGHT_FRACTION.at_shader_location(3),
    ])?)
}

impl SpecializedMeshPipeline for DetailPipeline {
    type Key = MeshPipelineKey;

    fn specialize(
        &self,
        key: Self::Key,
        layout: &MeshVertexBufferLayoutRef,
    ) -> Result<RenderPipelineDescriptor, SpecializedMeshPipelineError> {
        let mut descriptor = self.mesh_pipeline.specialize(key, layout)?;
        descriptor.vertex.shader = self.shader.clone();
        descriptor.vertex.buffers = vec![detail_mesh_layout(layout)?, detail_instance_layout()];
        if let Some(fragment) = descriptor.fragment.as_mut() {
            fragment.shader = self.shader.clone();
        }
        descriptor.layout.push(self.layout.clone());
        let draw_both_faces_of_a_sheet = None;
        descriptor.primitive.cull_mode = draw_both_faces_of_a_sheet;
        Ok(descriptor)
    }
}

/// Upload the instances of every tile that has been reseeded since its last
/// upload.
fn prepare_detail_buffers(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    tiles: Query<(Entity, &DetailTile, Option<&DetailInstanceBuffer>)>,
) {
    for (entity, tile, held) in &tiles {
        let source = Arc::as_ptr(&tile.instances) as usize;
        if held.is_some_and(|held| held.source == source) {
            continue;
        }
        let mut buffer = RawBufferVec::new(BufferUsages::VERTEX);
        for instance in tile.instances.iter() {
            buffer.push(*instance);
        }
        buffer.write_buffer(&render_device, &render_queue);
        commands.entity(entity).insert(DetailInstanceBuffer {
            buffer,
            length: tile.instances.len(),
            source,
        });
    }
}

/// Build one bind group per layer of every terrain drawing detail. A look
/// whose textures have not reached the GPU is left out.
fn prepare_detail_bind_groups(
    mut groups: ResMut<DetailBindGroups>,
    pipeline: Res<DetailPipeline>,
    render_device: Res<RenderDevice>,
    pipeline_cache: Res<PipelineCache>,
    looks: Res<DetailLooks>,
    images: Res<RenderAssets<GpuImage>>,
    fallback: Res<FallbackImage>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
) {
    groups.0.clear();
    let mut param = (images, fallback, buffers);
    for (key, bindings) in &looks.0 {
        if let Ok(prepared) = bindings.as_bind_group(
            &pipeline.layout,
            &render_device,
            &pipeline_cache,
            &mut param,
        ) {
            groups.0.insert(*key, prepared.bind_group);
        }
    }
}

/// Put every tile in view into the opaque phase. An instance writes depth and
/// discards the pixels outside its own silhouette.
#[expect(
    clippy::too_many_arguments,
    reason = "the queue reads the whole render world's mesh bookkeeping"
)]
fn queue_detail_tiles(
    pipeline: Res<DetailPipeline>,
    mut pipelines: ResMut<SpecializedMeshPipelines<DetailPipeline>>,
    pipeline_cache: Res<PipelineCache>,
    draw_functions: Res<DrawFunctions<Opaque3d>>,
    mut phases: ResMut<ViewBinnedRenderPhases<Opaque3d>>,
    mut queued: ResMut<QueuedDetailTiles>,
    view_keys: Res<bevy::pbr::ViewKeyCache>,
    meshes: Res<RenderAssets<RenderMesh>>,
    mesh_instances: Res<RenderMeshInstances>,
    mesh_allocator: Res<MeshAllocator>,
    views: Query<&ExtractedView>,
    tiles: Query<(Entity, &MainEntity), With<DetailTile>>,
) {
    let draw_function = draw_functions.read().id::<DrawDetail>();
    let live: HashSet<_> = views.iter().map(|view| view.retained_view_entity).collect();
    queued.0.retain(|view, _| live.contains(view));

    for view in &views {
        let Some(phase) = phases.get_mut(&view.retained_view_entity) else {
            continue;
        };
        let Some(&view_key) = view_keys.get(&view.retained_view_entity) else {
            continue;
        };

        let held = queued.0.entry(view.retained_view_entity).or_default();
        let mut present = HashMap::new();

        for (entity, main_entity) in &tiles {
            let Some(instance) = mesh_instances.render_mesh_queue_data(*main_entity) else {
                continue;
            };
            let Some(mesh) = meshes.get(instance.mesh_asset_id()) else {
                continue;
            };
            let Some(slabs) = mesh_allocator.mesh_slabs(&instance.mesh_asset_id()) else {
                continue;
            };
            let key = view_key
                | MeshPipelineKey::from_primitive_topology_and_strip_index(
                    mesh.primitive_topology(),
                    mesh.index_format(),
                );
            let Ok(pipeline_id) =
                pipelines.specialize(&pipeline_cache, &pipeline, key, &mesh.layout)
            else {
                continue;
            };
            let bin = (
                Opaque3dBatchSetKey {
                    draw_function,
                    pipeline: pipeline_id,
                    material_bind_group_index: None,
                    slabs,
                    lightmap_slab: None,
                },
                Opaque3dBinKey {
                    asset_id: instance.mesh_asset_id().into(),
                },
            );
            rebin_tile(
                phase,
                held.get(main_entity),
                (entity, *main_entity),
                bin.clone(),
                instance.current_uniform_index,
            );
            present.insert(*main_entity, bin);
        }

        for gone in held.keys().filter(|tile| !present.contains_key(*tile)) {
            phase.remove(*gone);
        }
        *held = present;
    }
}

/// Everything one tile of detail takes to draw.
type DrawDetail = (
    SetItemPipeline,
    SetMeshViewBindGroup<0>,
    SetMeshViewBindingArrayBindGroup<1>,
    SetMeshBindGroup<2>,
    SetDetailBindGroup<3>,
    DrawDetailInstanced,
);

/// Binds the look the tile's layer declared.
pub struct SetDetailBindGroup<const I: usize>;

impl<P: PhaseItem, const I: usize> RenderCommand<P> for SetDetailBindGroup<I> {
    type Param = SRes<DetailBindGroups>;
    type ViewQuery = ();
    type ItemQuery = Read<DetailTile>;

    fn render<'w>(
        _item: &P,
        _view: (),
        tile: Option<&'w DetailTile>,
        groups: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(tile) = tile else {
            return RenderCommandResult::Skip;
        };
        let Some(group) = groups.into_inner().0.get(&(tile.terrain, tile.layer)) else {
            return RenderCommandResult::Skip;
        };
        pass.set_bind_group(I, group, &[]);
        RenderCommandResult::Success
    }
}

/// Draws the layer's mesh once per instance.
pub struct DrawDetailInstanced;

impl<P: PhaseItem> RenderCommand<P> for DrawDetailInstanced {
    type Param = (
        SRes<RenderAssets<RenderMesh>>,
        SRes<RenderMeshInstances>,
        SRes<MeshAllocator>,
    );
    type ViewQuery = ();
    type ItemQuery = Read<DetailInstanceBuffer>;

    fn render<'w>(
        item: &P,
        _view: (),
        instances: Option<&'w DetailInstanceBuffer>,
        (meshes, mesh_instances, mesh_allocator): SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let mesh_allocator = mesh_allocator.into_inner();
        let Some(instances) = instances else {
            return RenderCommandResult::Skip;
        };
        let Some(buffer) = instances.buffer.buffer() else {
            return RenderCommandResult::Skip;
        };
        let Some(instance) = mesh_instances.render_mesh_queue_data(item.main_entity()) else {
            return RenderCommandResult::Skip;
        };
        let Some(mesh) = meshes.into_inner().get(instance.mesh_asset_id()) else {
            return RenderCommandResult::Skip;
        };
        let Some(vertices) = mesh_allocator.mesh_vertex_slice(&instance.mesh_asset_id()) else {
            return RenderCommandResult::Skip;
        };

        pass.set_vertex_buffer(0, vertices.buffer.slice(..));
        pass.set_vertex_buffer(1, buffer.slice(..));

        match &mesh.buffer_info {
            RenderMeshBufferInfo::Indexed {
                index_format,
                count,
            } => {
                let Some(index) = mesh_allocator.mesh_index_slice(&instance.mesh_asset_id()) else {
                    return RenderCommandResult::Skip;
                };
                pass.set_index_buffer(index.buffer.slice(..), *index_format);
                pass.draw_indexed(
                    index.range.start..(index.range.start + count),
                    vertices.range.start as i32,
                    0..instances.length as u32,
                );
            }
            RenderMeshBufferInfo::NonIndexed => {
                pass.draw(vertices.range.clone(), 0..instances.length as u32);
            }
        }
        RenderCommandResult::Success
    }
}

/// An authored sRGB colour as the linear vector the shader multiplies by.
fn linear_of(rgb: [f32; 3]) -> Vec4 {
    let linear = Color::srgb(rgb[0], rgb[1], rgb[2]).to_linear();
    Vec4::new(linear.red, linear.green, linear.blue, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::ChannelDescriptor;
    use crate::region::{RegionSize, TerrainRegions};

    /// A terrain of flat ground, `side` cells on an edge, with one fully
    /// painted channel per name given.
    fn document(side: u32, channels: &[&str]) -> RegionTerrainData {
        let mut data = RegionTerrainData {
            channels: channels
                .iter()
                .map(|name| ChannelDescriptor::new(*name, ChannelElement::U8))
                .collect(),
            regions: TerrainRegions::new(RegionSize::new(side).unwrap()),
            ..RegionTerrainData::default()
        };
        data.regions.set_channel_count(channels.len());
        for z in 0..side as i32 {
            for x in 0..side as i32 {
                data.regions.set_height(x, z, 0.0);
                for channel in 0..channels.len() {
                    data.regions.set_channel(channel, x, z, 255);
                }
            }
        }
        data
    }

    fn layer(name: &str, channel: &str) -> DetailLayer {
        DetailLayer {
            name: name.to_string(),
            density_channel: channel.to_string(),
            cull_distance: 24.0,
            density_per_m2: 4.0,
            ..DetailLayer::default()
        }
    }

    fn terrain(layers: Vec<DetailLayer>) -> Terrain {
        Terrain {
            cell_size: 1.0,
            detail: layers,
            ..Terrain::default()
        }
    }

    /// An app with everything the rebuild reads and no renderer at all.
    fn detail_app() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<Mesh>()
            .init_asset::<Image>()
            .init_resource::<DetailSettings>()
            .init_resource::<DetailPressers>()
            .init_resource::<DetailMeshes>()
            .add_systems(Startup, init_detail_assets)
            .add_systems(Update, rebuild_detail_tiles);
        app
    }

    fn spawn_terrain(app: &mut App, terrain: &Terrain, data: &RegionTerrainData) -> Entity {
        let source =
            TerrainDetailSource::from_document(data, terrain).expect("the terrain grows detail");
        app.world_mut()
            .spawn((
                source,
                DetailDirty::default(),
                Transform::IDENTITY,
                GlobalTransform::IDENTITY,
            ))
            .id()
    }

    fn spawn_grassy_terrain(app: &mut App) -> Entity {
        spawn_terrain(
            app,
            &terrain(vec![layer("grass", "grass")]),
            &document(64, &["grass"]),
        )
    }

    fn spawn_viewer(app: &mut App, at: Vec3) -> Entity {
        app.world_mut()
            .spawn((
                Camera3d::default(),
                Transform::from_translation(at),
                GlobalTransform::from_translation(at),
            ))
            .id()
    }

    fn tiles(app: &mut App) -> Vec<DetailTile> {
        app.world_mut()
            .query::<&DetailTile>()
            .iter(app.world())
            .cloned()
            .collect()
    }

    /// Enough frames for the per-frame budget to fill the field.
    fn settle(app: &mut App) {
        for _ in 0..400 {
            app.update();
        }
    }

    #[test]
    fn a_terrain_with_no_layers_has_no_source() {
        assert!(
            TerrainDetailSource::from_document(&document(8, &["grass"]), &Terrain::default())
                .is_none()
        );
    }

    #[test]
    fn an_unpainted_density_channel_is_bare_ground() {
        let data = document(8, &[]);
        let source = TerrainDetailSource::from_document(&data, &terrain(vec![layer("g", "grass")]))
            .expect("the terrain still grows detail");
        assert!(
            source.source().layers[0]
                .density
                .iter()
                .all(|value| *value == 0)
        );
    }

    #[test]
    fn a_rebuild_seeds_the_tiles_inside_the_cull_distance_and_no_further() {
        let mut app = detail_app();
        spawn_grassy_terrain(&mut app);
        spawn_viewer(&mut app, Vec3::new(32.0, 5.0, 32.0));
        settle(&mut app);

        let placed = tiles(&mut app);
        assert!(!placed.is_empty(), "the field is seeded");
        let cull = layer("grass", "grass").cull_distance;
        for tile in &placed {
            let min = tile.tile.as_vec2() * DETAIL_TILE_CELLS as f32;
            let nearest = Vec2::splat(32.0).clamp(min, min + Vec2::splat(DETAIL_TILE_CELLS as f32));
            assert!(
                Vec2::splat(32.0).distance(nearest) <= cull + 1.0,
                "tile {} stands inside the cull distance",
                tile.tile
            );
            assert!(!tile.instances.is_empty(), "a seeded tile has instances");
        }
    }

    #[test]
    fn two_layers_seed_their_own_tiles_from_their_own_channels() {
        let mut app = detail_app();
        let mut data = document(64, &["grass", "flowers"]);
        let painted_corner = 32;
        for z in 0..64 {
            for x in 0..64 {
                if x >= painted_corner || z >= painted_corner {
                    data.regions.set_channel(1, x, z, 0);
                }
            }
        }
        spawn_terrain(
            &mut app,
            &terrain(vec![layer("grass", "grass"), layer("flowers", "flowers")]),
            &data,
        );
        spawn_viewer(&mut app, Vec3::new(32.0, 5.0, 32.0));
        settle(&mut app);

        let placed = tiles(&mut app);
        assert!(placed.iter().any(|tile| tile.layer == 0));
        assert!(placed.iter().any(|tile| tile.layer == 1));
        let grown = |layer: usize| -> usize {
            placed
                .iter()
                .filter(|tile| tile.layer == layer)
                .map(|tile| tile.instances.len())
                .sum()
        };
        assert!(
            grown(1) > 0 && grown(1) < grown(0),
            "the second layer grows only where its own channel is painted: \
             {} against {}",
            grown(1),
            grown(0)
        );
    }

    #[test]
    fn removing_a_layer_retires_its_tiles() {
        let mut app = detail_app();
        let data = document(64, &["grass", "flowers"]);
        let entity = spawn_terrain(
            &mut app,
            &terrain(vec![layer("grass", "grass"), layer("flowers", "flowers")]),
            &data,
        );
        spawn_viewer(&mut app, Vec3::new(32.0, 5.0, 32.0));
        settle(&mut app);
        assert!(tiles(&mut app).iter().any(|tile| tile.layer == 1));

        let trimmed =
            TerrainDetailSource::from_document(&data, &terrain(vec![layer("grass", "grass")]))
                .expect("the terrain still grows detail");
        app.world_mut().entity_mut(entity).insert(trimmed);
        app.update();

        let placed = tiles(&mut app);
        assert!(!placed.is_empty(), "the layer that stayed still draws");
        assert!(
            placed.iter().all(|tile| tile.layer == 0),
            "no tile outlives the layer it grew from"
        );
    }

    #[test]
    fn an_asset_layer_draws_nothing_until_its_mesh_has_resolved() {
        let mut app = detail_app();
        let mut grown = layer("ferns", "grass");
        grown.mesh = DetailMesh::Asset("models/fern.gltf".to_string());
        spawn_terrain(&mut app, &terrain(vec![grown]), &document(64, &["grass"]));
        spawn_viewer(&mut app, Vec3::new(32.0, 5.0, 32.0));
        settle(&mut app);
        assert!(
            tiles(&mut app).is_empty(),
            "a layer whose asset has not loaded draws nothing"
        );

        let mesh = app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(card_mesh(1));
        app.world_mut().resource_mut::<DetailMeshes>().0.insert(
            "models/fern.gltf".to_string(),
            BuiltDetailMesh { mesh, color: None },
        );
        settle(&mut app);
        assert!(
            !tiles(&mut app).is_empty(),
            "and draws once the asset has resolved"
        );
    }

    #[test]
    fn an_asset_layer_merges_the_scatter_primitives_into_one_mesh() {
        let mut meshes = Assets::<Mesh>::default();
        let materials = Assets::<StandardMaterial>::default();
        let mut part = |offset: f32| {
            let mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            )
            .with_inserted_attribute(
                Mesh::ATTRIBUTE_POSITION,
                vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            )
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; 3])
            .with_inserted_indices(Indices::U32(vec![0, 1, 2]));
            ScatterPrimitive {
                mesh: meshes.add(mesh),
                material: Handle::default(),
                local: Transform::from_xyz(0.0, offset, 0.0),
            }
        };
        let primitives = vec![part(0.0), part(1.0)];

        let (merged, color) =
            merge_primitives("models/fern.gltf", &primitives, &meshes, &materials)
                .expect("both parts are loaded");
        assert!(color.is_none(), "neither part carries a material");
        assert_eq!(merged.count_vertices(), 6);
        assert_eq!(
            merged.indices().expect("the merge is indexed").len(),
            6,
            "both parts keep their own triangle"
        );
        let uvs = merged
            .attribute(Mesh::ATTRIBUTE_UV_0)
            .expect("the merge carries a uv set");
        assert_eq!(uvs.len(), 6, "a part with no uvs still gets one per vertex");

        let Some(bevy::mesh::VertexAttributeValues::Float32(fractions)) =
            merged.attribute(ATTRIBUTE_HEIGHT_FRACTION)
        else {
            panic!("the merge carries a height fraction per vertex");
        };
        assert_eq!(fractions[0], 0.0);
        assert_eq!(
            fractions[2], 0.5,
            "the lower of two stacked units reaches halfway up"
        );
        assert_eq!(fractions[5], 1.0);
    }

    #[test]
    fn a_merged_asset_fills_in_the_attributes_a_primitive_left_out() {
        let mut meshes = Assets::<Mesh>::default();
        let materials = Assets::<StandardMaterial>::default();
        let bare = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        )
        .with_inserted_indices(Indices::U16(vec![0, 1, 2]));
        let primitives = vec![ScatterPrimitive {
            mesh: meshes.add(bare),
            material: Handle::default(),
            local: Transform::from_scale(Vec3::new(3.0, 1.0, 1.0)),
        }];

        let (merged, color) =
            merge_primitives("models/bare.gltf", &primitives, &meshes, &materials)
                .expect("the one part is loaded");
        assert!(color.is_none());
        assert_eq!(merged.attributes().count(), 4);
        assert_eq!(
            merged.indices().expect("a u16 source still merges indexed"),
            &Indices::U32(vec![0, 1, 2])
        );

        let positions = merged
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .expect("the merge carries positions")
            .as_float3()
            .expect("positions are three floats");
        assert_eq!(
            positions[1],
            [3.0, 0.0, 0.0],
            "the placement scale is folded in"
        );

        let normals = merged
            .attribute(Mesh::ATTRIBUTE_NORMAL)
            .expect("the merge carries normals")
            .as_float3()
            .expect("normals are three floats");
        assert!(
            normals.iter().all(|normal| *normal == [0.0, 1.0, 0.0]),
            "a part with no normals stands its own up"
        );

        let Some(bevy::mesh::VertexAttributeValues::Float32x2(uvs)) =
            merged.attribute(Mesh::ATTRIBUTE_UV_0)
        else {
            panic!("the merge carries a uv per vertex");
        };
        assert!(uvs.iter().all(|uv| *uv == [0.0, 0.0]));
    }

    #[test]
    fn a_merged_normal_survives_a_non_uniform_scale() {
        let mut meshes = Assets::<Mesh>::default();
        let materials = Assets::<StandardMaterial>::default();
        let diagonal = Vec3::new(1.0, 1.0, 0.0).normalize().to_array();
        let leaning = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![diagonal; 3])
        .with_inserted_indices(Indices::U32(vec![0, 1, 2]));
        let stretched_fourfold_up = Transform::from_scale(Vec3::new(1.0, 4.0, 1.0));
        let primitives = vec![ScatterPrimitive {
            mesh: meshes.add(leaning),
            material: Handle::default(),
            local: stretched_fourfold_up,
        }];

        let (merged, _) = merge_primitives("models/leaning.gltf", &primitives, &meshes, &materials)
            .expect("the one part is loaded");
        let normals = merged
            .attribute(Mesh::ATTRIBUTE_NORMAL)
            .expect("the merge carries normals")
            .as_float3()
            .expect("normals are three floats");
        let normal = Vec3::from(normals[0]);
        assert!((normal.length() - 1.0).abs() < 1e-5);
        assert!(
            normal.x > normal.y,
            "the scale shortens the normal's y rather than lengthening it, but it is {normal}"
        );
    }

    #[test]
    fn a_flat_asset_gives_every_vertex_the_same_height_fraction() {
        let mut meshes = Assets::<Mesh>::default();
        let materials = Assets::<StandardMaterial>::default();
        let flat = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
        )
        .with_inserted_indices(Indices::U32(vec![0, 1, 2]));
        let primitives = vec![ScatterPrimitive {
            mesh: meshes.add(flat),
            material: Handle::default(),
            local: Transform::IDENTITY,
        }];

        let (merged, _) = merge_primitives("models/flat.gltf", &primitives, &meshes, &materials)
            .expect("the one part is loaded");
        let Some(bevy::mesh::VertexAttributeValues::Float32(fractions)) =
            merged.attribute(ATTRIBUTE_HEIGHT_FRACTION)
        else {
            panic!("the merge carries a height fraction per vertex");
        };
        assert_eq!(
            fractions,
            &vec![0.0; 3],
            "an asset with no height stands flat"
        );
    }

    #[test]
    fn two_layers_over_one_channel_stand_their_instances_apart() {
        let mut app = detail_app();
        spawn_terrain(
            &mut app,
            &terrain(vec![layer("near", "grass"), layer("far", "grass")]),
            &document(64, &["grass"]),
        );
        spawn_viewer(&mut app, Vec3::new(32.0, 5.0, 32.0));
        settle(&mut app);

        let placed = tiles(&mut app);
        let of = |index: usize, at: IVec2| {
            placed
                .iter()
                .find(|tile| tile.layer == index && tile.tile == at)
                .map(|tile| tile.instances.clone())
        };
        let at = placed
            .iter()
            .find(|tile| tile.layer == 0 && !tile.instances.is_empty())
            .expect("the first layer grew somewhere")
            .tile;
        assert_ne!(
            of(0, at),
            of(1, at),
            "two layers sharing a channel cover the same ground without standing in one another"
        );
    }

    #[test]
    fn a_rebuild_clears_the_dirty_mark_it_caught_up_with() {
        let mut app = detail_app();
        let terrain_entity = spawn_grassy_terrain(&mut app);
        spawn_viewer(&mut app, Vec3::new(32.0, 5.0, 32.0));
        settle(&mut app);
        let before = tiles(&mut app).len();

        app.world_mut()
            .get_mut::<DetailDirty>(terrain_entity)
            .expect("the terrain is marked")
            .touch(GridRect::whole(64));
        app.update();

        assert!(
            app.world()
                .get::<DetailDirty>(terrain_entity)
                .expect("the terrain is marked")
                .rect
                .is_none(),
            "the mark is cleared once the tiles it named are retired"
        );
        assert!(
            tiles(&mut app).len() < before,
            "the marked tiles were retired"
        );
        settle(&mut app);
        assert_eq!(tiles(&mut app).len(), before, "and then seeded again");
    }

    #[test]
    fn walking_away_retires_the_tiles_left_behind() {
        let mut app = detail_app();
        spawn_grassy_terrain(&mut app);
        let viewer = spawn_viewer(&mut app, Vec3::new(16.0, 5.0, 16.0));
        settle(&mut app);
        let near_first = tiles(&mut app);
        assert!(!near_first.is_empty());

        let moved = Vec3::new(56.0, 5.0, 56.0);
        *app.world_mut()
            .get_mut::<GlobalTransform>(viewer)
            .expect("the viewer has a transform") = GlobalTransform::from_translation(moved);
        settle(&mut app);

        let after = tiles(&mut app);
        assert!(!after.is_empty());
        let kept: Vec<IVec2> = after.iter().map(|tile| tile.tile).collect();
        assert!(
            near_first.iter().any(|tile| !kept.contains(&tile.tile)),
            "a tile the viewer walked away from was retired"
        );
        for tile in &after {
            let min = tile.tile.as_vec2() * DETAIL_TILE_CELLS as f32;
            let nearest = Vec2::new(56.0, 56.0).clamp(min, min + Vec2::splat(16.0));
            assert!(Vec2::new(56.0, 56.0).distance(nearest) <= 25.0);
        }
    }

    #[test]
    fn bare_ground_is_tiled_once_rather_than_reseeded_every_frame() {
        let mut app = detail_app();
        let mut data = document(64, &["grass"]);
        for z in 0..64 {
            for x in 0..64 {
                data.regions.set_channel(0, x, z, 0);
            }
        }
        spawn_terrain(&mut app, &terrain(vec![layer("grass", "grass")]), &data);
        spawn_viewer(&mut app, Vec3::new(32.0, 5.0, 32.0));
        settle(&mut app);

        let placed = tiles(&mut app);
        assert!(!placed.is_empty(), "the bare ground is tiled");
        assert!(
            placed.iter().all(|tile| tile.instances.is_empty()),
            "and every tile of it draws nothing"
        );
        let settled = placed.len();
        app.update();
        assert_eq!(
            tiles(&mut app).len(),
            settled,
            "a settled field seeds nothing further"
        );
    }

    #[test]
    fn a_terrain_that_stops_growing_detail_retires_its_tiles() {
        let mut app = detail_app();
        let terrain_entity = spawn_grassy_terrain(&mut app);
        spawn_viewer(&mut app, Vec3::new(32.0, 5.0, 32.0));
        settle(&mut app);
        assert!(!tiles(&mut app).is_empty());

        app.world_mut()
            .entity_mut(terrain_entity)
            .remove::<TerrainDetailSource>();
        app.update();
        assert!(
            tiles(&mut app).is_empty(),
            "no tile outlives the terrain it grew from"
        );
    }

    #[test]
    fn a_card_stands_one_unit_tall_from_its_foot() {
        for segments in [1, 3] {
            let mesh = card_mesh(segments);
            let positions = mesh
                .attribute(Mesh::ATTRIBUTE_POSITION)
                .expect("the card has positions")
                .as_float3()
                .expect("positions are three floats");
            assert_eq!(positions.len() as u32, (segments + 1) * 2);
            let lowest = positions.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
            let highest = positions.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
            assert_eq!(lowest, 0.0);
            assert_eq!(highest, 1.0);
            assert_eq!(
                mesh.indices().expect("the card is indexed").len(),
                segments as usize * 6
            );
            let Some(bevy::mesh::VertexAttributeValues::Float32(fractions)) =
                mesh.attribute(ATTRIBUTE_HEIGHT_FRACTION)
            else {
                panic!("the card carries a height fraction per vertex");
            };
            assert_eq!(fractions.first().copied(), Some(0.0));
            assert_eq!(fractions.last().copied(), Some(1.0));
        }
    }

    #[test]
    fn the_wind_texture_is_a_repeating_single_channel_field() {
        let image = wind_noise_image();
        assert_eq!(image.texture_descriptor.format, TextureFormat::R8Unorm);
        assert_eq!(image.texture_descriptor.size.width, WIND_TEXTURE_SIZE);
        assert_eq!(image.texture_descriptor.size.height, WIND_TEXTURE_SIZE);
        let ImageSampler::Descriptor(sampler) = &image.sampler else {
            panic!("the wind texture carries a sampler of its own");
        };
        assert_eq!(sampler.address_mode_u, ImageAddressMode::Repeat);
        assert_eq!(sampler.address_mode_v, ImageAddressMode::Repeat);

        let data = image.data.as_ref().expect("the texture has texels");
        assert_eq!(data.len(), (WIND_TEXTURE_SIZE * WIND_TEXTURE_SIZE) as usize);
        assert!(
            data.iter().any(|texel| *texel != data[0]),
            "the field varies rather than being flat"
        );
    }

    #[test]
    fn a_layer_with_no_texture_multiplies_by_one_white_texel() {
        let image = white_image();
        assert_eq!(image.texture_descriptor.size.width, 1);
        assert_eq!(image.texture_descriptor.size.height, 1);
        assert_eq!(
            image.data.as_ref().expect("the texture has texels"),
            &[255, 255, 255, 255]
        );
    }

    #[test]
    fn two_edits_before_a_rebuild_cover_both() {
        let mut dirty = DetailDirty::default();
        dirty.touch(GridRect {
            x: 0,
            z: 0,
            width: 4,
            height: 4,
        });
        dirty.touch(GridRect {
            x: 20,
            z: 20,
            width: 4,
            height: 4,
        });
        let rect = dirty.rect.expect("both edits are marked");
        assert_eq!(rect.x, 0);
        assert_eq!(rect.z, 0);
        assert_eq!(rect.width, 24);
        assert_eq!(rect.height, 24);
    }

    /// The shader source, for the binding checks below. naga-oil input rather
    /// than plain WGSL, so the checks are textual.
    const SHADER_SOURCE: &str = include_str!("shaders/detail.wgsl");

    #[test]
    fn every_detail_binding_is_declared_in_the_shader_at_its_derive_index() {
        for (binding, declaration) in [
            (0, "var<uniform> detail: DetailUniform"),
            (1, "var wind_noise: texture_2d<f32>"),
            (2, "var wind_sampler: sampler"),
            (3, "var color_texture: texture_2d<f32>"),
            (4, "var color_sampler: sampler"),
        ] {
            let expected = format!("@group(3) @binding({binding}) {declaration};");
            assert!(
                SHADER_SOURCE.contains(&expected),
                "the AsBindGroup derive binds {binding} as `{declaration}`, \
                 but the shader does not declare it that way"
            );
        }
    }

    #[test]
    fn the_detail_uniform_struct_matches_the_bindings_field_order() {
        let declared: Vec<String> = shader_uniform_members()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(
            declared,
            [
                "color_base",
                "color_tip",
                "wind_direction",
                "wind_speed",
                "wind_strength",
                "wind_vertical_strength",
                "wind_tile_size",
                "bend",
                "push_strength",
                "height_range",
                "width_range",
                "cull_distance",
                "presser_count",
                "is_card",
                "pressers",
            ]
        );
    }

    /// The bytes the shader reads are the bytes the derive writes. `Mirror`
    /// repeats the `#[uniform(0)]` field types in declaration order.
    #[test]
    fn the_detail_uniform_lays_out_the_same_bytes_the_shader_reads() {
        use bevy::render::render_resource::ShaderType;

        #[derive(ShaderType)]
        struct Mirror {
            color_base: Vec4,
            color_tip: Vec4,
            wind_direction: Vec2,
            wind_speed: f32,
            wind_strength: f32,
            wind_vertical_strength: f32,
            wind_tile_size: f32,
            bend: f32,
            push_strength: f32,
            height_range: Vec2,
            width_range: Vec2,
            cull_distance: f32,
            presser_count: u32,
            is_card: u32,
            pressers: [Vec4; MAX_DETAIL_PRESSERS],
        }

        let mut offset = 0u64;
        let mut declared = Vec::new();
        for (name, ty) in shader_uniform_members() {
            let (size, align) = match ty.as_str() {
                "f32" | "u32" => (4, 4),
                "vec2<f32>" => (8, 8),
                "vec4<f32>" => (16, 16),
                "array<vec4<f32>, 16>" => (16 * MAX_DETAIL_PRESSERS as u64, 16),
                other => panic!("no layout rule for `{other}`"),
            };
            offset = offset.next_multiple_of(align);
            declared.push((name, offset));
            offset += size;
        }
        let size = offset.next_multiple_of(16);

        assert_eq!(
            declared,
            vec![
                ("color_base".to_string(), 0),
                ("color_tip".to_string(), 16),
                ("wind_direction".to_string(), 32),
                ("wind_speed".to_string(), 40),
                ("wind_strength".to_string(), 44),
                ("wind_vertical_strength".to_string(), 48),
                ("wind_tile_size".to_string(), 52),
                ("bend".to_string(), 56),
                ("push_strength".to_string(), 60),
                ("height_range".to_string(), 64),
                ("width_range".to_string(), 72),
                ("cull_distance".to_string(), 80),
                ("presser_count".to_string(), 84),
                ("is_card".to_string(), 88),
                ("pressers".to_string(), 96),
            ]
        );
        assert_eq!(size, Mirror::min_size().get());
    }

    /// The name and type of each member of the shader's uniform struct.
    fn shader_uniform_members() -> Vec<(String, String)> {
        let start = SHADER_SOURCE
            .find("struct DetailUniform {")
            .expect("DetailUniform is declared");
        let body = &SHADER_SOURCE[start..];
        let body = &body[..body.find('}').expect("DetailUniform is closed")];
        body.lines()
            .skip(1)
            .filter_map(|line| line.trim().strip_suffix(','))
            .filter(|line| !line.starts_with("//"))
            .map(|line| {
                let (name, ty) = line.split_once(':').expect("a member is `name: type`");
                (name.trim().to_string(), ty.trim().to_string())
            })
            .collect()
    }

    #[test]
    fn the_instance_and_the_mesh_take_separate_attribute_slots() {
        let layout = detail_instance_layout();
        assert_eq!(layout.step_mode, VertexStepMode::Instance);
        assert_eq!(layout.array_stride, size_of::<DetailInstance>() as u64);

        let mut locations: Vec<u32> = layout
            .attributes
            .iter()
            .map(|attribute| attribute.shader_location)
            .collect();
        locations.sort_unstable();
        locations.dedup();
        assert_eq!(
            locations.len(),
            layout.attributes.len(),
            "no slot is reused"
        );

        let card = card_mesh(NEAR_SEGMENTS);
        for attribute in [
            Mesh::ATTRIBUTE_POSITION.id,
            Mesh::ATTRIBUTE_NORMAL.id,
            Mesh::ATTRIBUTE_UV_0.id,
            ATTRIBUTE_HEIGHT_FRACTION.id,
        ] {
            assert!(card.attribute(attribute).is_some());
        }
        assert_eq!(
            card.attributes().count(),
            4,
            "the card and every merged asset carry one attribute per mesh slot"
        );
        for location in &locations {
            assert!(*location >= 4, "slot {location} is already the mesh's");
        }
    }

    #[test]
    fn requeuing_a_tile_under_a_new_pipeline_leaves_one_bin_entry() {
        use bevy::render::batching::gpu_preprocessing::GpuPreprocessingMode;
        use bevy::render::mesh::allocator::MeshSlabs;
        use bevy::render::render_phase::DrawFunctionId;
        use bevy::render::render_resource::CachedRenderPipelineId;
        use bevy::render::view::RetainedViewEntity;

        let bin = |pipeline| {
            (
                Opaque3dBatchSetKey {
                    draw_function: DrawFunctionId(0),
                    pipeline: CachedRenderPipelineId::new(pipeline),
                    material_bind_group_index: None,
                    slabs: MeshSlabs::default(),
                    lightmap_slab: None,
                },
                Opaque3dBinKey {
                    asset_id: AssetId::<Mesh>::invalid().into(),
                },
            )
        };
        let view = RetainedViewEntity::new(Entity::PLACEHOLDER.into(), None, 0);
        let tile = MainEntity::from(Entity::PLACEHOLDER);
        let ids = (Entity::PLACEHOLDER, tile);

        let mut phases = ViewBinnedRenderPhases::<Opaque3d>::default();
        phases.prepare_for_new_frame(view, GpuPreprocessingMode::None);
        let phase = phases.get_mut(&view).unwrap();
        rebin_tile(phase, None, ids, bin(0), InputUniformIndex::default());

        phases.prepare_for_new_frame(view, GpuPreprocessingMode::None);
        let phase = phases.get_mut(&view).unwrap();
        rebin_tile(
            phase,
            Some(&bin(0)),
            ids,
            bin(1),
            InputUniformIndex::default(),
        );

        let placed: Vec<_> = phase
            .non_mesh_items
            .iter()
            .filter(|(_, items)| items.entities.contains_key(&tile))
            .map(|(key, _)| key.clone())
            .collect();
        assert_eq!(placed.len(), 1, "the tile is left in its old bin as well");
        assert!(placed[0] == bin(1), "the tile is not in its new bin");
    }

    #[test]
    fn the_shader_reads_the_same_presser_count_the_bindings_write() {
        assert!(SHADER_SOURCE.contains(&format!(
            "const MAX_PRESSERS: u32 = {MAX_DETAIL_PRESSERS}u;"
        )));
        assert!(SHADER_SOURCE.contains(&format!(
            "pressers: array<vec4<f32>, {MAX_DETAIL_PRESSERS}>,"
        )));
    }
}
