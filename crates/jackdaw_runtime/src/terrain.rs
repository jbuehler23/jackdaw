//! Terrain rendering in a built game.
//!
//! A scene's `Terrain` entities carry a scene-relative `data_path` naming a
//! `.jdterrain` sidecar beside the scene file. The sidecar decoder, splat
//! material and clipmap mesher all live in `jackdaw_terrain`, shared with the
//! editor. This module locates the sidecar, resolves the terrain's material
//! names against the project catalog, and follows the game's camera.
//!
//! With the `physics` feature enabled, the decoded heights also become a
//! heightfield collider.
//!
//! A sidecar that exists but fails to decode draws nothing, rather than
//! building ground from partial data. A missing sidecar is the flat-ground
//! case and is not an error.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bevy::asset::LoadState;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use jackdaw_scene_types::Terrain;
use jackdaw_terrain::render::{
    SplatArrayHandles, SplatBuildError, TerrainRenderPlugin, TerrainSplatMaterial,
    TextureSetImages, control_image_from_bytes, resolve_with, slope_image, splat_images,
};
use jackdaw_terrain::sidecar::{self, TerrainMaterialSlot};
use jackdaw_terrain::splat::ControlTexels;
use jackdaw_terrain::texture_set::TextureSet;
use jackdaw_terrain::{ClipmapLevel, Heightmap, RegionTerrainData};

use crate::JackdawCatalog;

#[cfg(feature = "physics")]
use avian3d::prelude::{Collider, RigidBody};

/// Colour of a terrain drawn without a texture set.
///
/// Linear, to match the editor, which multiplies a white base colour by a
/// `[0.5, 0.5, 0.5]` vertex colour consumed linearly. `Color::srgb(0.5, ..)`
/// would upload as linear 0.214 and draw more than twice as dark.
const UNTEXTURED: Color = Color::linear_rgb(0.5, 0.5, 0.5);

pub(crate) fn plugin(app: &mut App) {
    app.add_plugins(TerrainRenderPlugin).add_systems(
        Update,
        (
            load_sidecars,
            refresh_heightmaps,
            resolve_material_slots,
            build_ready_materials,
            sync_surfaces,
        )
            .chain()
            .after(crate::spawn_loaded_scenes),
    );
    #[cfg(feature = "physics")]
    app.add_systems(Update, build_ground_colliders.after(refresh_heightmaps));
}

/// The sidecar file one terrain draws, resolved beneath the directory of the
/// scene that spawned it.
#[derive(Component)]
struct TerrainSidecar(PathBuf);

/// A terrain's decoded document, plus the heights the mesher samples.
///
/// The heightmap derives from the document and the terrain's dimensions, and
/// is rebuilt when either changes.
#[derive(Component)]
struct TerrainDocument {
    data: RegionTerrainData,
    heightmap: Arc<Heightmap>,
    shape: jackdaw_terrain::GridShape,
    max_height: f32,
}

impl TerrainDocument {
    fn matches(&self, terrain: &Terrain) -> bool {
        self.shape == self.data.grid_shape(terrain.size, terrain.resolution)
            && self.max_height == terrain.max_height
    }
}

/// A terrain whose sidecar was found but could not be decoded. It draws
/// nothing.
#[derive(Component)]
struct TerrainUnreadable;

/// What one terrain's material names resolved to, and the splat material
/// built from them once every texture has decoded.
#[derive(Component, Default)]
struct TerrainSplat {
    /// The slot list this was resolved from. A different list makes
    /// everything below stale.
    slots: Vec<TerrainMaterialSlot>,
    set: TextureSet,
    images: TextureSetImages,
    /// Slot names with no material behind them in this project.
    missing: Vec<String>,
    material: Option<Handle<TerrainSplatMaterial>>,
    /// Terrain geometry the material was built against: the control map's
    /// size and the shader's UV scaling both depend on it.
    built_size: Vec2,
    built_resolution: u32,
    /// Whether a problem has already been logged for this list, so a broken
    /// set is reported once rather than every frame.
    reported: bool,
    /// Whether the surfaces are carrying a material this no longer stands
    /// behind.
    resurface: bool,
}

/// Marks the camera a terrain lays its LOD levels around.
///
/// Optional. With no camera marked, the viewer is the active `Camera3d` with
/// the highest `order`. A game whose UI overlay camera has a higher `order`
/// marks its world camera instead, so the overlay does not pull the finest
/// ring away from the player. Among several marked cameras, the highest
/// `order` wins.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct TerrainViewer;

/// One LOD level of a terrain's surface, rebuilt as the camera moves.
#[derive(Component)]
struct TerrainSurface {
    terrain: Entity,
    /// 0 is the finest ring.
    level: u32,
}

/// The level a surface entity is drawing, which may lag the level it should be
/// drawing while [`jackdaw_terrain::plan_rebuilds`] brings it up.
#[derive(Component)]
struct BuiltLevel(ClipmapLevel);

/// Shared material for a terrain with no texture set.
#[derive(Resource)]
struct UntexturedTerrain(Handle<StandardMaterial>);

/// Record the sidecar each freshly spawned terrain draws.
///
/// Called from the scene spawn, the only place the scene's directory is known:
/// `data_path` is relative to it and the entity keeps no other trace of where
/// it came from. A path that names nothing loadable is logged and the terrain
/// draws flat.
pub(crate) fn attach_sidecars(world: &mut World, spawned: &[Entity], parent_path: &Path) {
    let terrains: Vec<(Entity, String)> = spawned
        .iter()
        .filter_map(|&entity| {
            world
                .get::<Terrain>(entity)
                .filter(|terrain| !terrain.data_path.is_empty())
                .map(|terrain| (entity, terrain.data_path.clone()))
        })
        .collect();
    if terrains.is_empty() {
        return;
    }
    let Some(root) = crate::assets_root(world) else {
        warn!("no asset root found; terrain sidecars cannot be located");
        return;
    };

    let scene_dir = root.join(parent_path);
    for (entity, data_path) in terrains {
        match sidecar::resolve_path(&scene_dir, &data_path) {
            Ok(path) => {
                world.entity_mut(entity).insert(TerrainSidecar(path));
            }
            Err(err) => warn!("terrain data path {data_path:?} is unusable: {err}"),
        }
    }
}

/// Read and decode every terrain sidecar that has not been read yet.
fn load_sidecars(
    mut commands: Commands,
    terrains: Query<
        (Entity, &Terrain, Option<&TerrainSidecar>),
        (Without<TerrainDocument>, Without<TerrainUnreadable>),
    >,
) {
    for (entity, terrain, sidecar_path) in &terrains {
        let data = match sidecar_path {
            Some(TerrainSidecar(path)) => match std::fs::read(path) {
                Ok(bytes) => match sidecar::load(&bytes) {
                    Ok(data) => data,
                    Err(err) => {
                        error!(
                            "terrain data {} is unreadable ({err}); this terrain draws nothing \
                             until the file is fixed",
                            path.display()
                        );
                        commands.entity(entity).insert(TerrainUnreadable);
                        continue;
                    }
                },
                // A missing file is the flat-ground case, not a fault.
                Err(_) => RegionTerrainData::default(),
            },
            None => RegionTerrainData::default(),
        };

        commands
            .entity(entity)
            .insert((document_of(data, terrain), TerrainSplat::default()));
    }
}

fn document_of(data: RegionTerrainData, terrain: &Terrain) -> TerrainDocument {
    // The same resolver the editor uses, so a scene draws the same ground.
    let shape = data.grid_shape(terrain.size, terrain.resolution);
    let mut heights = Vec::new();
    data.regions
        .sample_grid_heights_into(shape.resolution, &mut heights);
    TerrainDocument {
        data,
        heightmap: Arc::new(Heightmap {
            resolution: shape.resolution,
            size: shape.size,
            origin: shape.origin,
            max_height: terrain.max_height,
            heights,
        }),
        shape,
        max_height: terrain.max_height,
    }
}

/// The `StandardMaterial` a slot's name addresses, through the project catalog
/// that `materials/<name>.material.bsn` and `catalog.bsn` both feed.
///
/// [`resolve_with`] decides what a resolved material means: which of its slots
/// is albedo, and that a vacated or unfound name keeps its texture id.
fn catalog_material(catalog: &JackdawCatalog, name: &str) -> Option<Handle<StandardMaterial>> {
    catalog
        .get(&format!("@{name}"))
        .cloned()
        .and_then(|handle| handle.try_typed::<StandardMaterial>().ok())
}

/// Keep every terrain's resolved set following its material list.
///
/// Runs every frame rather than on a change signal: resolution is one catalog
/// lookup per slot and the result is compared before it is stored, so an
/// unchanged terrain writes nothing, and a material file that finishes loading
/// late is picked up on the next frame without invalidation bookkeeping.
fn resolve_material_slots(
    catalog: Res<JackdawCatalog>,
    standard: Res<Assets<StandardMaterial>>,
    assets: Res<AssetServer>,
    mut terrains: Query<(&TerrainDocument, &mut TerrainSplat)>,
) {
    for (document, mut splat) in &mut terrains {
        let slots = &document.data.materials;
        let resolved = resolve_with(
            slots,
            |name| catalog_material(&catalog, name).and_then(|handle| standard.get(&handle)),
            &assets,
        );
        if splat.slots == *slots && splat.set == resolved.set && splat.missing == resolved.missing {
            continue;
        }
        if !resolved.missing.is_empty() {
            warn!(
                "terrain materials not found in this project: {}",
                resolved.missing.join(", ")
            );
        }
        splat.slots = slots.clone();
        splat.set = resolved.set;
        splat.images = resolved.images;
        splat.missing = resolved.missing;
        splat.material = None;
        splat.reported = false;
        splat.resurface = true;
    }
}

/// Build the splat material for every terrain whose textures have all decoded.
///
/// Runs every frame and does nothing once a material exists, so retrying while
/// images load costs a comparison.
fn build_ready_materials(
    mut splat_materials: ResMut<Assets<TerrainSplatMaterial>>,
    mut images: ResMut<Assets<Image>>,
    assets: Res<AssetServer>,
    mut terrains: Query<(&TerrainDocument, &mut TerrainSplat)>,
) {
    for (document, mut splat) in &mut terrains {
        let shape = document.shape;
        if splat.material.is_some()
            && (splat.built_size != shape.size || splat.built_resolution != shape.resolution)
        {
            splat.material = None;
        }
        if splat.set.is_empty() || splat.material.is_some() {
            continue;
        }

        let built = match splat_images(&splat.set, &splat.images, &images) {
            Ok(built) => built,
            Err(SplatBuildError::NotReady) => {
                // Still loading, or permanently failed. Only the asset
                // server can tell the two apart, and only a failure is
                // logged.
                if let Some(failed) = first_failed(&splat.images, &assets)
                    && !splat.reported
                {
                    splat.reported = true;
                    error!("terrain texture {failed:?} could not be loaded; check the material");
                }
                continue;
            }
            Err(other) => {
                if !splat.reported {
                    splat.reported = true;
                    error!("terrain materials: {other}");
                }
                continue;
            }
        };

        let texels =
            ControlTexels::from_control(&document.data.grid_control(), shape.resolution).to_bytes();
        let control = images.add(control_image_from_bytes(&texels, shape.resolution));
        let slope = images.add(slope_image(&document.heightmap));
        let arrays = SplatArrayHandles {
            albedo: images.add(built.albedo),
            normal: images.add(built.normal),
            height: images.add(built.height),
        };
        splat.material = Some(splat_materials.add(TerrainSplatMaterial::new(
            &splat.set,
            arrays,
            control,
            slope,
            shape.size,
            shape.resolution,
            document.data.autoterrain,
        )));
        splat.built_size = shape.size;
        splat.built_resolution = shape.resolution;
        splat.reported = false;
        splat.resurface = true;
    }
}

/// The path of the first texture the asset server has given up on.
fn first_failed(handles: &TextureSetImages, assets: &AssetServer) -> Option<String> {
    handles.handles().find_map(|handle| {
        matches!(
            assets.get_load_state(handle.id()),
            Some(LoadState::Failed(_))
        )
        .then(|| {
            assets
                .get_path(handle.id())
                .map(|path| path.to_string())
                .unwrap_or_default()
        })
    })
}

/// Keep every terrain's LOD levels following the game's camera.
///
/// One entity per level holds that level's mesh, rebuilt when the level snaps
/// to a new origin or when the material under it changes. [`jackdaw_terrain`]
/// owns the rebuild budget, the hole bookkeeping and the mesh construction: a
/// level that only handed a different square to the finer one takes a new
/// index buffer rather than a new surface, and a rebuild writes over the mesh
/// asset the level already owns.
fn sync_surfaces(
    mut commands: Commands,
    mut terrains: Query<(
        Entity,
        &Terrain,
        &TerrainDocument,
        &mut TerrainSplat,
        &GlobalTransform,
    )>,
    surfaces: Query<(Entity, &TerrainSurface, &BuiltLevel, &Mesh3d)>,
    cameras: Query<(&Camera, &GlobalTransform, Has<TerrainViewer>), With<Camera3d>>,
    // A dedicated server builds these types with no rendering plugins, so
    // there is no mesh store to build a surface into.
    meshes: Option<ResMut<Assets<Mesh>>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    untextured: Option<Res<UntexturedTerrain>>,
) {
    let Some(mut meshes) = meshes else {
        return;
    };
    let Some(viewer) = viewer_position(&cameras) else {
        // The levels are laid out around a viewer, so with no camera the
        // terrain never draws.
        bevy::log::warn_once!(
            "no active Camera3d, so terrain has no viewer to lay its levels around and \
             will not draw"
        );
        return;
    };

    let fallback = match &untextured {
        Some(res) => res.0.clone(),
        None => {
            let handle = materials.add(StandardMaterial {
                base_color: UNTEXTURED,
                perceptual_roughness: 0.9,
                metallic: 0.0,
                ..default()
            });
            commands.insert_resource(UntexturedTerrain(handle.clone()));
            handle
        }
    };

    for (terrain_entity, terrain, document, mut splat, terrain_transform) in &mut terrains {
        // The camera in this terrain's grid coordinates, so a moved or
        // rotated terrain follows the camera the same way an unmoved one does.
        let local = terrain_transform
            .affine()
            .inverse()
            .transform_point3(viewer);
        // Extent and placement both come from the stored cells.
        let shape = document.shape;
        let cell = shape.size / (shape.resolution.max(2) - 1) as f32;
        let grid = (Vec2::new(local.x, local.z) - shape.origin) / cell;
        let levels = jackdaw_terrain::clipmap_levels(shape.resolution, grid);

        let resurface = std::mem::take(&mut splat.resurface);

        let mut existing: Vec<(Entity, u32, ClipmapLevel, Handle<Mesh>)> = surfaces
            .iter()
            .filter(|(_, surface, _, _)| surface.terrain == terrain_entity)
            .map(|(entity, surface, built, mesh)| (entity, surface.level, built.0, mesh.0.clone()))
            .collect();
        existing.retain(|(entity, level, _, _)| {
            let stale = *level as usize >= levels.len();
            if stale {
                commands.entity(*entity).despawn();
            }
            !stale
        });

        let held_at = |index: u32| existing.iter().find(|(_, level, _, _)| *level == index);
        let standing: Vec<Option<ClipmapLevel>> = levels
            .iter()
            .map(|level| held_at(level.level).map(|(_, _, built, _)| *built))
            .collect();
        // Nothing edits the ground at runtime, so a material swap is the only
        // thing that forces a level to remesh where it stands.
        let forced = vec![resurface; levels.len()];
        let plan = jackdaw_terrain::plan_rebuilds(
            &levels,
            &standing,
            &forced,
            jackdaw_terrain::REBUILD_BUDGET,
        );

        for (index, level) in plan.iter().enumerate() {
            let Some(level) = level else {
                continue;
            };
            let held = held_at(level.level);

            let present = |x: i32, z: i32| {
                // A terrain with no regions was never authored region by
                // region, and draws whole.
                if document.data.regions.region_count() > 0 {
                    document.data.regions.covers(x, z)
                } else {
                    true
                }
            };
            // Only the square left to the finer level moved. A hole decides
            // triangles, not vertices, so the surface on the GPU keeps its
            // positions, normals and UVs and takes a new index buffer.
            let hole_only = !forced[index]
                && !terrain.quantization.enabled
                && held.is_some_and(|(_, _, built, handle)| {
                    built.same_lattice(level) && meshes.contains(handle)
                });
            if hole_only {
                let (entity, _, _, handle) = held.expect("a hole-only rebuild has a surface");
                let indices =
                    jackdaw_terrain::build_clipmap_indices(shape.resolution, level, present);
                if let Some(mut mesh) = meshes.get_mut(handle) {
                    mesh.insert_indices(Indices::U32(indices));
                }
                commands.entity(*entity).insert(BuiltLevel(*level));
                continue;
            }

            let data =
                jackdaw_terrain::build_clipmap_mesh_data(&document.heightmap, level, present);
            let data = if terrain.quantization.enabled {
                jackdaw_terrain::flat_shaded(data)
            } else {
                data
            };
            let mut rebuilt = Some(surface_mesh(data));

            // Write over the mesh this level already owns rather than adding
            // another asset: a new handle costs the renderer a fresh buffer
            // allocation and a rebuilt bind group, and nothing else refers
            // to this mesh.
            if let Some((_, _, _, handle)) = held
                && let Some(mut slot) = meshes.get_mut(handle)
            {
                *slot = rebuilt.take().expect("the mesh was just built");
            }
            let fresh = rebuilt.map(|mesh| meshes.add(mesh));

            let mut entity = match held {
                Some((entity, _, _, _)) => commands.entity(*entity),
                None => commands.spawn((
                    TerrainSurface {
                        terrain: terrain_entity,
                        level: level.level,
                    },
                    Transform::default(),
                    Visibility::default(),
                    ChildOf(terrain_entity),
                )),
            };
            entity.insert(BuiltLevel(*level));
            if let Some(handle) = fresh {
                entity.insert(Mesh3d(handle));
            }
            // The two material types are different components, so the unused
            // one has to be removed.
            match &splat.material {
                Some(handle) => {
                    entity
                        .remove::<MeshMaterial3d<StandardMaterial>>()
                        .insert(MeshMaterial3d(handle.clone()));
                }
                None => {
                    entity
                        .remove::<MeshMaterial3d<TerrainSplatMaterial>>()
                        .insert(MeshMaterial3d(fallback.clone()));
                }
            }
        }
    }
}

/// Where the terrain is being looked at from.
fn viewer_position(
    cameras: &Query<(&Camera, &GlobalTransform, Has<TerrainViewer>), With<Camera3d>>,
) -> Option<Vec3> {
    viewer_of(
        cameras
            .iter()
            .filter(|(camera, _, _)| camera.is_active)
            .map(|(camera, transform, marked)| (marked, camera.order, transform.translation()))
            .collect::<Vec<_>>(),
    )
}

/// The viewer among the active 3D cameras, as `(marked, order, position)`.
///
/// If any camera is marked with [`TerrainViewer`], only marked cameras are
/// considered. Otherwise every camera is, and the highest `order` wins.
fn viewer_of(cameras: Vec<(bool, isize, Vec3)>) -> Option<Vec3> {
    let any_marked = cameras.iter().any(|(marked, _, _)| *marked);
    cameras
        .iter()
        .filter(|(marked, _, _)| *marked || !any_marked)
        .max_by_key(|(_, order, _)| *order)
        .map(|(_, _, position)| *position)
}

fn surface_mesh(data: jackdaw_terrain::SurfaceMeshData) -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, data.positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, data.normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, data.uvs);
    mesh.insert_indices(Indices::U32(data.indices));
    mesh
}

/// Rebuild a terrain's sampling heights when the component's dimensions change
/// under an already-decoded document.
fn refresh_heightmaps(mut terrains: Query<(&Terrain, &mut TerrainDocument), Changed<Terrain>>) {
    for (terrain, mut document) in &mut terrains {
        if document.matches(terrain) {
            continue;
        }
        let data = std::mem::take(&mut document.data);
        *document = document_of(data, terrain);
    }
}

/// Marks the entity carrying one terrain's ground collider, and names the
/// terrain it stands under.
///
/// A child rather than the terrain itself, because the collider has to be
/// offset: parry's heightfield is centred on its own origin while a terrain's
/// cells start at its grid anchor. The child's `Transform` holds the
/// difference and follows the terrain.
#[cfg(feature = "physics")]
#[derive(Component)]
struct TerrainGround(Entity);

/// The collider that stands under a terrain's drawn ground, and where it
/// sits in the terrain's own space.
///
/// Built from the heightmap the mesher draws from, so the two cannot disagree
/// about where the ground is. Parry's heightfield is a unit grid stretched by
/// a scale vector, which requires:
///
/// - an XZ scale of the grid's whole span, `(resolution - 1) * cell`, which is
///   [`GridShape::size`](jackdaw_terrain::GridShape);
/// - a Y scale of `1`, because the stored heights are already metres, the same
///   numbers [`jackdaw_terrain::build_clipmap_mesh_data`] writes into vertex
///   positions;
/// - and a grid centred on the shape's origin, which is not where the cells
///   are. Cells start at the grid anchor: the terrain's own origin for a
///   terrain authored against this format, and `-size/2` for one migrated from
///   a declared rectangle. The returned offset is that difference, and applying
///   it puts grid vertex `(x, z)` at `anchor + (x, z) * cell`, where the mesher
///   puts it.
///
/// Avian takes the heights as rows of a square array and hands them to parry
/// column-major, leaving the outer index advancing X and the inner one Z.
///
/// `None` for a terrain with no cells: parry needs two rows and two columns,
/// and a terrain holding no regions is the unsculpted case rather than a fault.
#[cfg(feature = "physics")]
fn ground_collider(heightmap: &Heightmap) -> Option<(Collider, Vec3)> {
    let resolution = heightmap.resolution;
    if resolution < 2 {
        return None;
    }
    let heights: Vec<Vec<f32>> = (0..resolution)
        .map(|x| {
            (0..resolution)
                .map(|z| heightmap.get_height(x, z))
                .collect()
        })
        .collect();
    let centre = heightmap.origin + heightmap.size / 2.0;
    Some((
        Collider::heightfield(heights, Vec3::new(heightmap.size.x, 1.0, heightmap.size.y)),
        Vec3::new(centre.x, 0.0, centre.y),
    ))
}

/// Put ground under every terrain whose heights arrived, and take it away from
/// one left with no cells.
///
/// `Changed` rather than `Added`, because a `Terrain` whose dimensions change
/// has its heights rebuilt in place by [`refresh_heightmaps`].
#[cfg(feature = "physics")]
fn build_ground_colliders(
    mut commands: Commands,
    terrains: Query<(Entity, &TerrainDocument), Changed<TerrainDocument>>,
    grounds: Query<(Entity, &TerrainGround)>,
) {
    for (terrain, document) in &terrains {
        let standing = grounds
            .iter()
            .find(|(_, ground)| ground.0 == terrain)
            .map(|(entity, _)| entity);
        match ground_collider(&document.heightmap) {
            Some((collider, offset)) => {
                // Avian only simulates a collider that some rigid body owns,
                // and the ground belongs to no other.
                let ground = (
                    collider,
                    RigidBody::Static,
                    Transform::from_translation(offset),
                );
                match standing {
                    Some(entity) => {
                        commands.entity(entity).insert(ground);
                    }
                    None => {
                        commands.spawn((TerrainGround(terrain), ground, ChildOf(terrain)));
                    }
                }
            }
            None => {
                if let Some(entity) = standing {
                    commands.entity(entity).despawn();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::{AssetApp, AssetPlugin};
    use jackdaw_terrain::sidecar::{AutoTerrainSettings, RegionTerrainData};

    const GRASS: &str = "#grass\nbevy_pbr::pbr_material::StandardMaterial {\n    \
        base_color_texture: \"textures/grass_basecolor.png\",\n    \
        normal_map_texture: \"textures/grass_normal.png\",\n    \
        depth_map: \"textures/grass_height.png\",\n    \
        flip_normal_map_y: true,\n}\n";

    /// A project laid out like a built game's asset root: a scene, its sidecar
    /// beside it, and the materials the terrain names.
    fn fixture_project() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        let assets = tmp.path().join("assets");
        std::fs::create_dir_all(assets.join("materials")).expect("materials dir");
        std::fs::write(assets.join("catalog.bsn"), "// no catalog entries\n").expect("catalog");
        std::fs::write(assets.join("materials/grass.material.bsn"), GRASS).expect("material");

        let mut data = RegionTerrainData {
            // Regions sized to the fixture: sixteen cells rather than a
            // default region's quarter-million.
            regions: jackdaw_terrain::TerrainRegions::new(
                jackdaw_terrain::RegionSize::new(4).expect("a power of two"),
            ),
            materials: vec![
                TerrainMaterialSlot::new("grass"),
                TerrainMaterialSlot::tombstone(),
                TerrainMaterialSlot::new("nowhere"),
            ],
            autoterrain: AutoTerrainSettings {
                enabled: true,
                base_slot: 0,
                slope_slot: 2,
                slope_start_deg: 30.0,
                slope_end_deg: 60.0,
            },
            ..default()
        };
        let _ = data.regions.ensure_grid(4);
        std::fs::write(
            assets.join("zone.terrain-0.jdterrain"),
            sidecar::save(&data).expect("encode"),
        )
        .expect("sidecar");
        tmp
    }

    fn catalog_app(assets_root: &Path) -> App {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            AssetPlugin {
                file_path: assets_root.to_string_lossy().into_owned(),
                ..default()
            },
        ));
        app.init_asset::<Image>();
        app.init_asset::<StandardMaterial>();
        app.register_asset_reflect::<Image>();
        app.register_asset_reflect::<StandardMaterial>();
        app.init_resource::<JackdawCatalog>();
        crate::load_material_files(app.world_mut(), &assets_root.join("materials"));
        app
    }

    /// Resolution through the catalog, the way `resolve_material_slots` does
    /// it. The mapping itself lives in the shared crate and is tested there.
    fn resolve_in(
        app: &App,
        slots: &[TerrainMaterialSlot],
    ) -> jackdaw_terrain::render::ResolvedSlots {
        let catalog = app.world().resource::<JackdawCatalog>();
        let standard = app.world().resource::<Assets<StandardMaterial>>();
        resolve_with(
            slots,
            |name| catalog_material(catalog, name).and_then(|handle| standard.get(&handle)),
            app.world().resource::<AssetServer>(),
        )
    }

    #[test]
    fn a_sidecar_resolves_beside_the_scene_that_names_it() {
        let project = fixture_project();
        let scene_dir = project.path().join("assets");

        let path = sidecar::resolve_path(&scene_dir, "zone.terrain-0.jdterrain").expect("resolve");

        assert_eq!(path, scene_dir.join("zone.terrain-0.jdterrain"));
        assert!(path.is_file());
    }

    #[test]
    fn a_sidecar_path_is_relative_to_the_scenes_own_directory() {
        let root = Path::new("/project/assets");

        let path =
            sidecar::resolve_path(&root.join("zones"), "starter.jdterrain").expect("resolve");

        assert_eq!(path, Path::new("/project/assets/zones/starter.jdterrain"));
    }

    #[test]
    fn a_sidecar_path_may_not_leave_the_scene_directory() {
        assert!(
            sidecar::resolve_path(Path::new("/project/assets"), "../secrets.jdterrain").is_err()
        );
        assert!(sidecar::resolve_path(Path::new("/project/assets"), "zone.png").is_err());
    }

    /// Drive `load_sidecars` over one terrain pointed at `path`.
    fn load_one(path: &Path) -> (App, Entity) {
        let mut app = App::new();
        app.add_systems(Update, load_sidecars);
        let entity = app
            .world_mut()
            .spawn((
                Terrain {
                    resolution: 4,
                    data_path: "zone.terrain-0.jdterrain".to_string(),
                    ..default()
                },
                TerrainSidecar(path.to_owned()),
            ))
            .id();
        app.update();
        (app, entity)
    }

    /// Bytes that are not a sidecar quarantine the terrain rather than
    /// standing it up flat.
    #[test]
    fn a_sidecar_that_does_not_decode_quarantines_its_terrain() {
        let project = fixture_project();
        let path = project.path().join("assets/zone.terrain-0.jdterrain");
        std::fs::write(&path, b"not a terrain at all").expect("write");

        let (app, entity) = load_one(&path);

        assert!(
            app.world().get::<TerrainUnreadable>(entity).is_some(),
            "the terrain must be marked unreadable"
        );
        assert!(
            app.world().get::<TerrainDocument>(entity).is_none(),
            "a quarantined terrain must have no ground to draw"
        );
    }

    #[test]
    fn a_sidecar_that_decodes_gives_its_terrain_ground_to_draw() {
        let project = fixture_project();
        let path = project.path().join("assets/zone.terrain-0.jdterrain");

        let (app, entity) = load_one(&path);

        assert!(app.world().get::<TerrainUnreadable>(entity).is_none());
        let document = app
            .world()
            .get::<TerrainDocument>(entity)
            .expect("document");
        assert_eq!(document.heightmap.heights.len(), 4 * 4);
        assert_eq!(document.data.materials.len(), 3);
    }

    #[test]
    fn a_sidecar_that_is_not_there_leaves_the_terrain_flat_rather_than_quarantined() {
        let project = fixture_project();
        let path = project.path().join("assets/never-written.jdterrain");

        let (app, entity) = load_one(&path);

        assert!(app.world().get::<TerrainUnreadable>(entity).is_none());
        let document = app
            .world()
            .get::<TerrainDocument>(entity)
            .expect("document");
        assert!(document.heightmap.heights.iter().all(|h| *h == 0.0));
    }

    /// Autoterrain settings reach the shader uniform from the sidecar, with
    /// no export step between.
    #[test]
    fn a_sidecars_autoterrain_settings_reach_the_splat_material() {
        let project = fixture_project();
        let path = project.path().join("assets/zone.terrain-0.jdterrain");

        let (app, entity) = load_one(&path);

        let document = app
            .world()
            .get::<TerrainDocument>(entity)
            .expect("document");
        assert!(document.data.autoterrain.enabled);
        assert_eq!(document.data.autoterrain.slope_slot, 2);

        let material = TerrainSplatMaterial::new(
            &TextureSet::default(),
            SplatArrayHandles {
                albedo: Handle::default(),
                normal: Handle::default(),
                height: Handle::default(),
            },
            Handle::default(),
            Handle::default(),
            Vec2::splat(100.0),
            4,
            document.data.autoterrain,
        );

        assert_eq!(material.autoterrain_enabled, 1);
        assert_eq!(material.autoterrain_base_slot, 0);
        assert_eq!(material.autoterrain_slope_slot, 2);
        assert!((material.autoterrain_slope_start - core::f32::consts::FRAC_PI_6).abs() < 1e-6);
        assert!((material.autoterrain_slope_end - core::f32::consts::FRAC_PI_3).abs() < 1e-6);
    }

    /// A sidecar with no autoterrain settings leaves autoterrain off.
    #[test]
    fn a_terrain_that_never_asked_for_autoterrain_draws_from_its_control_map_alone() {
        let material = TerrainSplatMaterial::new(
            &TextureSet::default(),
            SplatArrayHandles {
                albedo: Handle::default(),
                normal: Handle::default(),
                height: Handle::default(),
            },
            Handle::default(),
            Handle::default(),
            Vec2::splat(100.0),
            4,
            RegionTerrainData::default().autoterrain,
        );

        assert_eq!(material.autoterrain_enabled, 0);
    }

    #[test]
    fn the_viewer_is_the_camera_that_renders_last() {
        let cameras = vec![
            (false, 0, Vec3::X),
            (false, 3, Vec3::Y),
            (false, 1, Vec3::Z),
        ];

        assert_eq!(viewer_of(cameras), Some(Vec3::Y));
    }

    /// A UI overlay drawn last must not pull the finest ring off the player.
    #[test]
    fn a_marked_camera_outranks_a_later_drawn_unmarked_one() {
        let cameras = vec![(true, 0, Vec3::X), (false, 9, Vec3::Y)];

        assert_eq!(viewer_of(cameras), Some(Vec3::X));
    }

    #[test]
    fn several_marked_cameras_fall_back_to_the_one_that_renders_last() {
        let cameras = vec![(true, 2, Vec3::X), (true, 5, Vec3::Y), (false, 9, Vec3::Z)];

        assert_eq!(viewer_of(cameras), Some(Vec3::Y));
    }

    #[test]
    fn no_camera_at_all_is_no_viewer() {
        assert_eq!(viewer_of(Vec::new()), None);
    }

    /// A saved material's three texture slots become the entry's albedo,
    /// normal and height.
    #[test]
    fn a_slot_resolves_through_the_catalog_to_its_materials_textures() {
        let project = fixture_project();
        let app = catalog_app(&project.path().join("assets"));

        let resolved = resolve_in(&app, &[TerrainMaterialSlot::new("grass")]);

        let entry = &resolved.set.entries[0];
        assert_eq!(entry.material, "grass");
        assert_eq!(
            entry.albedo.as_deref(),
            Some("textures/grass_basecolor.png")
        );
        assert_eq!(entry.normal.as_deref(), Some("textures/grass_normal.png"));
        assert_eq!(entry.height.as_deref(), Some("textures/grass_height.png"));
        assert!(entry.flip_normal_y);
        assert!(resolved.missing.is_empty());
        assert_eq!(resolved.images.albedo.len(), 1);
        assert!(resolved.images.height[0].is_some());
    }

    #[test]
    fn a_name_the_catalog_does_not_hold_is_reported_missing() {
        let project = fixture_project();
        let app = catalog_app(&project.path().join("assets"));

        let resolved = resolve_in(&app, &[TerrainMaterialSlot::new("nowhere")]);

        assert_eq!(resolved.missing, vec!["nowhere".to_string()]);
        assert!(resolved.set.entries[0].albedo.is_none());
    }

    /// The whole chain, from the file on disk to the entries the array
    /// builder is handed.
    #[test]
    fn a_sidecars_material_list_resolves_against_the_project_catalog() {
        let project = fixture_project();
        let assets = project.path().join("assets");
        let app = catalog_app(&assets);
        let path = sidecar::resolve_path(&assets, "zone.terrain-0.jdterrain").expect("resolve");
        let data = sidecar::load(&std::fs::read(&path).expect("read")).expect("decode");

        let resolved = resolve_in(&app, &data.materials);

        assert_eq!(resolved.set.entries.len(), 3);
        assert_eq!(
            resolved.set.entries[0].albedo.as_deref(),
            Some("textures/grass_basecolor.png")
        );
        assert_eq!(resolved.missing, vec!["nowhere".to_string()]);
    }

    /// One terrain, its levels laid out around a camera the test moves.
    fn surfaced_world(resolution: u32) -> (World, Entity) {
        let mut world = World::new();
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<StandardMaterial>::default());

        let mut regions = jackdaw_terrain::TerrainRegions::new(
            jackdaw_terrain::RegionSize::new(256).expect("a power of two"),
        );
        regions
            .ensure_grid(resolution - 1)
            .expect("inside the region cap");
        let data = RegionTerrainData {
            regions,
            ..default()
        };
        let terrain = Terrain {
            resolution,
            size: Vec2::splat((resolution - 1) as f32),
            ..default()
        };
        let document = document_of(data, &terrain);

        world.spawn((
            Camera3d::default(),
            Camera::default(),
            GlobalTransform::default(),
        ));
        let entity = world
            .spawn((
                terrain,
                document,
                TerrainSplat::default(),
                GlobalTransform::default(),
            ))
            .id();
        (world, entity)
    }

    fn run_sync(world: &mut World) {
        world.clear_trackers();
        world.run_system_cached(sync_surfaces).expect("system runs");
        world.flush();
    }

    /// Each level's triangle count, and whether it has a material to draw
    /// them with. A level with no material draws nothing.
    fn drawn_levels(world: &mut World) -> Vec<(u32, usize, bool)> {
        let mut query = world.query::<(
            &TerrainSurface,
            &Mesh3d,
            Has<MeshMaterial3d<StandardMaterial>>,
            Has<MeshMaterial3d<TerrainSplatMaterial>>,
        )>();
        let held: Vec<(u32, Handle<Mesh>, bool)> = query
            .iter(world)
            .map(|(surface, mesh, plain, splat)| (surface.level, mesh.0.clone(), plain || splat))
            .collect();
        let meshes = world.resource::<Assets<Mesh>>();
        let mut drawn: Vec<(u32, usize, bool)> = held
            .iter()
            .map(|(level, handle, material)| {
                let triangles = meshes
                    .get(handle)
                    .and_then(Mesh::indices)
                    .map_or(0, |indices| indices.len() / 3);
                (*level, triangles, *material)
            })
            .collect();
        drawn.sort_by_key(|(level, _, _)| *level);
        drawn
    }

    /// A material arriving mid-flight never costs the screen a level.
    ///
    /// A whole-terrain rebuild comes from a material swap, and levels keep
    /// drawing with the material they have until the new one is built. Tearing
    /// them down up front would leave the ground missing for as long as the
    /// per-frame rebuild budget takes to catch up.
    #[test]
    fn a_resurface_never_takes_a_level_off_the_screen() {
        let (mut world, entity) = surfaced_world(1025);
        let place = |world: &mut World, at: f32| {
            let mut cameras = world.query_filtered::<&mut GlobalTransform, With<Camera3d>>();
            for mut transform in cameras.iter_mut(world) {
                *transform = GlobalTransform::from_translation(Vec3::new(at, 200.0, at));
            }
        };
        place(&mut world, 0.0);
        run_sync(&mut world);
        let before = drawn_levels(&mut world);
        assert!(before.len() >= 4, "a kilometre of terrain needs levels");
        assert!(
            before.iter().all(|(_, triangles, _)| *triangles > 0),
            "every level must draw to begin with: {before:?}",
        );

        world
            .get_mut::<TerrainSplat>(entity)
            .expect("the terrain tracks its material")
            .resurface = true;

        for frame in 1..=8 {
            place(&mut world, frame as f32 * 7.0);
            run_sync(&mut world);
            let now = drawn_levels(&mut world);
            for (level, _, _) in &before {
                let Some((_, triangles, material)) = now.iter().find(|(at, _, _)| at == level)
                else {
                    panic!("frame {frame}: level {level} lost its mesh; {now:?}");
                };
                assert!(
                    *material,
                    "frame {frame}: level {level} has no material to draw with",
                );
                assert!(
                    *triangles > 0,
                    "frame {frame}: level {level} drew nothing; {now:?}",
                );
            }
        }
    }
}
