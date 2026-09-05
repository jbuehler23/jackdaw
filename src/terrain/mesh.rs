use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use jackdaw_terrain::ClipmapLevel;

use super::regions::{TerrainRegionView, region_of};
use super::{CHUNK_SIZE, TerrainDataStore, TerrainDirtyChunks, TerrainPaintState, TerrainSurface};
use crate::viewport::{ActiveViewport, MainViewportCamera};

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (
            rebuild_on_channel_view_change,
            rebuild_on_region_view_change,
            sync_terrain_surface,
        )
            .chain()
            .run_if(in_state(crate::AppState::Editor)),
    );
}

/// Shared material for terrain with no texture set. `base_color` is white;
/// the paint tint lives in each level's vertex-colour attribute, so
/// toggling the channel debug view is a mesh rebuild, not a material swap.
///
/// A terrain with a texture set draws with the splat material instead (see
/// [`super::splat`]).
#[derive(Resource)]
struct TerrainMaterialHandle(Handle<StandardMaterial>);

/// Vertex colour for unpainted ground and for every vertex when the
/// channel view is off.
const UNPAINTED: [f32; 4] = [0.5, 0.5, 0.5, 1.0];

/// The LOD level one surface entity is drawing.
///
/// This is what the entity holds, which may differ from the level the
/// camera position calls for while [`jackdaw_terrain::plan_rebuilds`]
/// brings a lagging level up over the following frames.
#[derive(Component)]
struct BuiltLevel(ClipmapLevel);

/// Rebuild every terrain when the visualised channel changes.
///
/// [`TerrainPaintState`] also carries brush position, which changes almost
/// every frame, so a bare `is_changed()` here would rebuild continuously;
/// only the two relevant fields are compared.
fn rebuild_on_channel_view_change(
    paint: Res<TerrainPaintState>,
    mut last: Local<Option<(usize, bool)>>,
    mut terrains: Query<&mut TerrainDirtyChunks, With<jackdaw_scene_types::Terrain>>,
) {
    let current = (paint.active_channel, paint.show_channel);
    if *last == Some(current) {
        return;
    }
    let first_run = last.is_none();
    *last = Some(current);
    if first_run {
        return;
    }
    for mut dirty in &mut terrains {
        dirty.rebuild_all = true;
    }
}

/// Rebuild every terrain when a region view change moves geometry.
///
/// Only `Hidden` filters quads, so entering or leaving it, or moving the
/// active region while in it, is what changes what is meshed; see
/// [`TerrainRegionView::geometry_key`]. Toggling the grid overlay or
/// switching to `Base only` reaches the screen without a remesh.
fn rebuild_on_region_view_change(
    view: Res<TerrainRegionView>,
    mut last: Local<Option<Option<Option<(Entity, jackdaw_terrain::RegionCoord)>>>>,
    mut terrains: Query<&mut TerrainDirtyChunks, With<jackdaw_scene_types::Terrain>>,
) {
    let current = view.geometry_key();
    if *last == Some(current) {
        return;
    }
    let first_run = last.is_none();
    *last = Some(current);
    if first_run {
        return;
    }
    for mut dirty in &mut terrains {
        dirty.rebuild_all = true;
    }
}

/// Keep every terrain's LOD levels following the camera.
///
/// One entity per level, holding that level's mesh. A level is rebuilt
/// when it snaps to a new origin, when the heights under it were edited, or
/// when the whole terrain is asked to rebuild; otherwise it keeps its mesh.
///
/// Three things bound the cost of a gesture that crosses many snap
/// boundaries in a row, such as a zoom-out:
/// [`jackdaw_terrain::plan_rebuilds`] rations how many levels follow the
/// camera per frame, a level that only handed a different square to the
/// finer one takes a new index buffer rather than a new surface, and a
/// rebuild writes over the mesh asset the level owns instead of minting
/// another.
#[expect(
    clippy::too_many_arguments,
    reason = "the surface is built from the store, the camera and both materials"
)]
fn sync_terrain_surface(
    mut commands: Commands,
    mut terrains: Query<(
        Entity,
        &jackdaw_scene_types::Terrain,
        &mut TerrainDirtyChunks,
    )>,
    surfaces: Query<(Entity, &TerrainSurface, &BuiltLevel, &Mesh3d)>,
    cameras: Query<(Entity, &GlobalTransform, Has<MainViewportCamera>), With<Camera3d>>,
    active: Res<ActiveViewport>,
    transforms: Query<&GlobalTransform>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    material_res: Option<Res<TerrainMaterialHandle>>,
    store: Res<TerrainDataStore>,
    paint: Res<TerrainPaintState>,
    splat: Res<super::splat::TerrainSplatMaterials>,
    region_view: Res<TerrainRegionView>,
) {
    let Some(viewer) = viewer_position(&active, &cameras) else {
        return;
    };

    let fallback = match &material_res {
        Some(res) => res.0.clone(),
        None => {
            // No depth_bias needed: the editor grid yields to opaque
            // geometry at its own plane (see `crate::editor_grid`).
            let handle = materials.add(StandardMaterial {
                base_color: Color::WHITE,
                perceptual_roughness: 0.9,
                metallic: 0.0,
                ..default()
            });
            commands.insert_resource(TerrainMaterialHandle(handle.clone()));
            handle
        }
    };

    for (terrain_entity, terrain, mut dirty) in &mut terrains {
        // The camera position in this terrain's grid coordinates: the
        // levels are laid out on the grid, so a moved or rotated terrain
        // follows the camera the same way an untransformed one does.
        let local = transforms
            .get(terrain_entity)
            .map(|transform| transform.affine().inverse().transform_point3(viewer))
            .unwrap_or(viewer);
        // Extent and placement both come from the stored cells, so the
        // levels ring the camera over the ground the terrain holds.
        let shape = store.grid_shape(terrain);
        let cell = shape.size / (shape.resolution.max(2) - 1) as f32;
        let grid = (Vec2::new(local.x, local.z) - shape.origin) / cell;
        let levels = jackdaw_terrain::clipmap_levels(shape.resolution, grid);

        let rebuild_all = std::mem::take(&mut dirty.rebuild_all);
        let edited: Vec<(u32, u32)> = dirty.dirty.drain().collect();
        let heightmap = store.heightmap(terrain);
        let document = store.get(&terrain.data_path);
        let splat_handle = splat.material(&terrain.data_path);

        let mut existing: Vec<(Entity, u32, ClipmapLevel, Handle<Mesh>)> = surfaces
            .iter()
            .filter(|(_, surface, _, _)| surface.terrain_entity == terrain_entity)
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
        // A level whose heights or region cover changed is remeshed; the
        // budget rations only following the camera.
        //
        // Each level is asked about the ground it stands on as well as the
        // ground it is headed for: a lagging level still draws its previous
        // square, and an edit landing there would otherwise be dropped.
        let forced: Vec<bool> = levels
            .iter()
            .zip(&standing)
            .map(|(level, standing)| {
                rebuild_all
                    || edited.iter().any(|chunk| {
                        chunk_touches(*chunk, level, shape.resolution)
                            || standing.is_some_and(|standing| {
                                chunk_touches(*chunk, &standing, shape.resolution)
                            })
                    })
            })
            .collect();
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

            // A hidden region reads as absent, through the same filter an
            // unallocated one goes through: it changes which quads are
            // emitted, never a stored word. Saving, exporting and the brush
            // read the document, which no view mode touches.
            let present = |x: i32, z: i32| match document {
                // A terrain with no regions was not authored region by
                // region, and draws whole.
                Some(data) if data.regions.region_count() > 0 => {
                    data.regions.covers(x, z)
                        && region_view.draws_geometry(
                            terrain_entity,
                            region_of(x, z, data.regions.region_size().get()),
                        )
                }
                _ => true,
            };
            // When only the square left to the finer level moved, the hole
            // decides triangles rather than vertices: the surface on the
            // GPU keeps every position, normal, UV and colour and takes a
            // new index buffer.
            //
            // Not available to a quantized terrain: `flat_shaded` emits
            // three vertices per triangle, so its vertex set is its index
            // set and a moved hole rewrites both.
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

            let data = jackdaw_terrain::build_clipmap_mesh_data(&heightmap.map, level, present);
            let data = if terrain.quantization.enabled {
                jackdaw_terrain::flat_shaded(data)
            } else {
                data
            };
            let colors = surface_vertex_colors(terrain, &store, &paint, &data.grid);
            let mut rebuilt = Some(build_bevy_mesh(data, colors));

            // Write over the mesh this level owns rather than minting
            // another asset: a new handle costs the renderer a buffer
            // allocation and a rebuilt bind group every frame of a gesture,
            // and nothing else refers to this one. Bevy's
            // `calculate_bounds` watches `AssetChanged<Mesh3d>`, so the
            // `Aabb` follows a write through the handle.
            if let Some((_, _, _, handle)) = held
                && let Some(mut slot) = meshes.get_mut(handle)
            {
                *slot = rebuilt.take().expect("the mesh was just built");
            }
            let fresh = rebuilt.map(|mesh| meshes.add(mesh));

            let mut entity = match held {
                Some((entity, _, _, _)) => commands.entity(*entity),
                None => {
                    // `TerrainSurface` requires `EditorHidden +
                    // NonSerializable`; nothing to insert here.
                    commands.spawn((
                        TerrainSurface {
                            terrain_entity,
                            level: level.level,
                        },
                        Transform::default(),
                        Visibility::default(),
                        ChildOf(terrain_entity),
                    ))
                }
            };
            entity.insert(BuiltLevel(*level));
            if let Some(handle) = fresh {
                entity.insert(Mesh3d(handle));
            }
            // The two material types are different components, so the
            // unused one is removed: a terrain whose last texture slot was
            // removed would otherwise keep drawing the arrays.
            match &splat_handle {
                Some(handle) => {
                    entity
                        .remove::<MeshMaterial3d<StandardMaterial>>()
                        .insert(MeshMaterial3d(handle.clone()));
                }
                None => {
                    entity
                        .remove::<MeshMaterial3d<jackdaw_terrain::render::TerrainSplatMaterial>>()
                        .insert(MeshMaterial3d(fallback.clone()));
                }
            }
        }
    }
}

/// Where the terrain is being looked at from: the hovered viewport's
/// camera, the main one when nothing is hovered, or any 3D camera when
/// neither is around, which is all a headless app has.
fn viewer_position(
    active: &ActiveViewport,
    cameras: &Query<(Entity, &GlobalTransform, Has<MainViewportCamera>), With<Camera3d>>,
) -> Option<Vec3> {
    if let Some(camera) = active.camera
        && let Some((_, transform, _)) = cameras.iter().find(|(entity, _, _)| *entity == camera)
    {
        return Some(transform.translation());
    }
    cameras
        .iter()
        .find(|(_, _, main)| *main)
        .or_else(|| cameras.iter().next())
        .map(|(_, transform, _)| transform.translation())
}

/// Whether an edited chunk overlaps the ground a level draws.
///
/// The brush marks chunks, which are a fixed grid over the terrain; a
/// level is a square of grid points around the camera. A stroke rebuilds
/// the levels it is under and leaves the rest alone.
fn chunk_touches(chunk: (u32, u32), level: &ClipmapLevel, resolution: u32) -> bool {
    let last = resolution.saturating_sub(1) as i32;
    let min = IVec2::new(chunk.0 as i32, chunk.1 as i32) * CHUNK_SIZE as i32;
    let max = (min + IVec2::splat(CHUNK_SIZE as i32)).min(IVec2::splat(last));
    let square = level.square();
    min.x <= square.max.x && max.x >= square.min.x && min.y <= square.max.y && max.y >= square.min.y
}

/// Colour every vertex by what is painted under it. Off, or with no
/// channel selected, every vertex is [`UNPAINTED`].
///
/// Colours are derived from `grid`, the grid coordinate the mesher recorded
/// per vertex, rather than by re-walking the level's lattice: the smooth
/// and flat meshers emit different vertex counts and orders. Values the
/// palette does not name draw as unpainted.
fn surface_vertex_colors(
    terrain: &jackdaw_scene_types::Terrain,
    store: &TerrainDataStore,
    paint: &TerrainPaintState,
    grid: &[[u32; 2]],
) -> Vec<[f32; 4]> {
    let descriptor = terrain.channels.get(paint.active_channel);
    let document = store.get(&terrain.data_path);
    let (Some(descriptor), Some(data)) = (descriptor, document) else {
        return vec![UNPAINTED; grid.len()];
    };
    if !paint.show_channel || data.channels.len() <= paint.active_channel {
        return vec![UNPAINTED; grid.len()];
    }

    // Read through the regions: a vertex whose region is absent carries no
    // paint, which the accessor answers as zero.
    grid.iter()
        .map(|[gx, gz]| {
            descriptor
                .color_of(data.regions.grid_channel(paint.active_channel, *gx, *gz))
                .map(|color| color.to_linear().to_f32_array())
                .unwrap_or(UNPAINTED)
        })
        .collect()
}

fn build_bevy_mesh(data: jackdaw_terrain::SurfaceMeshData, colors: Vec<[f32; 4]>) -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, default());
    debug_assert_eq!(
        colors.len(),
        data.positions.len(),
        "vertex colours must be emitted in the same order and count as positions",
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, data.positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, data.normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, data.uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(data.indices));
    mesh
}

#[cfg(test)]
mod tests {
    use jackdaw_terrain::{RegionTerrainData, TerrainData};

    use super::*;

    /// A document at `resolution`, holding `heights`.
    fn document(resolution: u32, heights: Vec<f32>) -> RegionTerrainData {
        RegionTerrainData::from_legacy_v1(&TerrainData {
            resolution,
            heights,
            channels: vec![],
        })
        .expect("any resolution migrates")
    }

    fn terrain(resolution: u32, data_path: &str) -> jackdaw_scene_types::Terrain {
        jackdaw_scene_types::Terrain {
            resolution,
            size: Vec2::splat((resolution - 1) as f32),
            data_path: data_path.to_string(),
            ..default()
        }
    }

    /// A world with one camera and one terrain, marked for a full rebuild.
    fn world_with_terrain(resolution: u32, heights: Vec<f32>) -> (World, Entity) {
        let mut world = World::new();
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<StandardMaterial>::default());
        world.insert_resource(TerrainPaintState::default());
        world.insert_resource(ActiveViewport::default());
        world.insert_resource(super::super::splat::TerrainSplatMaterials::default());
        world.insert_resource(super::super::regions::TerrainRegionView::default());

        let mut store = TerrainDataStore::default();
        store.insert("a.jdterrain", document(resolution, heights));
        world.insert_resource(store);

        world.spawn((Camera3d::default(), GlobalTransform::default()));
        let entity = world
            .spawn((
                terrain(resolution, "a.jdterrain"),
                TerrainDirtyChunks {
                    rebuild_all: true,
                    ..default()
                },
                GlobalTransform::default(),
            ))
            .id();
        (world, entity)
    }

    fn run(world: &mut World) {
        world.clear_trackers();
        world
            .run_system_cached(sync_terrain_surface)
            .expect("system runs");
        world.flush();
    }

    /// Which levels the last [`run`] rebuilt, in level order.
    ///
    /// A rebuild writes [`BuiltLevel`], whether the level was remeshed
    /// whole or only took a new index buffer. Mesh handles cannot answer
    /// this: a rebuilt level writes over the mesh it owns, so its handle is
    /// unchanged either way.
    fn rebuilt_levels(world: &mut World) -> Vec<u32> {
        let mut query = world.query_filtered::<&TerrainSurface, Changed<BuiltLevel>>();
        let mut levels: Vec<u32> = query.iter(world).map(|surface| surface.level).collect();
        levels.sort_unstable();
        levels
    }

    /// Every level's mesh, in level order: query order is not guaranteed,
    /// and comparing two runs by position needs one.
    fn surface_meshes(world: &mut World) -> Vec<(u32, Handle<Mesh>)> {
        let mut query = world.query::<(&TerrainSurface, &Mesh3d)>();
        let mut meshes: Vec<(u32, Handle<Mesh>)> = query
            .iter(world)
            .map(|(surface, mesh)| (surface.level, mesh.0.clone()))
            .collect();
        meshes.sort_by_key(|(level, _)| *level);
        meshes
    }

    fn levels_of(world: &mut World, terrain_entity: Entity) -> Vec<u32> {
        let mut query = world.query::<&TerrainSurface>();
        let mut levels: Vec<u32> = query
            .iter(world)
            .filter(|surface| surface.terrain_entity == terrain_entity)
            .map(|surface| surface.level)
            .collect();
        levels.sort_unstable();
        levels
    }

    /// 129 vertices per edge, which no power-of-two region holds on its
    /// own, still meshes.
    #[test]
    fn a_129_resolution_terrain_meshes() {
        let (mut world, entity) = world_with_terrain(129, vec![1.0; 129 * 129]);
        run(&mut world);

        let levels = levels_of(&mut world, entity);
        assert!(!levels.is_empty(), "a 129 terrain must draw something");

        let mut query = world.query::<(&TerrainSurface, &Mesh3d)>();
        let handles: Vec<Handle<Mesh>> =
            query.iter(&world).map(|(_, mesh)| mesh.0.clone()).collect();
        let meshes = world.resource::<Assets<Mesh>>();
        let triangles: usize = handles
            .iter()
            .filter_map(|handle| meshes.get(handle))
            .filter_map(Mesh::indices)
            .map(|indices| indices.len() / 3)
            .sum();
        assert!(triangles > 0, "the surface has no triangles");
    }

    /// A level that has not moved and whose ground was not edited keeps its
    /// mesh: a coarse ring stands for as long as the camera stays inside
    /// it.
    #[test]
    fn an_unmoved_level_is_not_rebuilt() {
        let (mut world, entity) = world_with_terrain(257, vec![0.0; 257 * 257]);
        run(&mut world);
        let count = levels_of(&mut world, entity).len();
        assert!(count > 1, "this terrain needs more than one level");

        run(&mut world);
        assert!(
            rebuilt_levels(&mut world).is_empty(),
            "nothing moved, nothing should be rebuilt",
        );
        assert_eq!(levels_of(&mut world, entity).len(), count);
    }

    /// A small camera move re-snaps the fine levels and leaves the coarse
    /// ones where they are: a level's grid step doubles with every level
    /// out, so a drift that moves the finest one falls short of the
    /// outermost.
    #[test]
    fn a_small_camera_move_re_snaps_only_the_fine_levels() {
        let (mut world, _) = world_with_terrain(512, vec![0.0; 512 * 512]);
        // Cells are one world unit across and the terrain is centred, so
        // these two stations sit at grid 252 and grid 254: across the
        // finest level's two-cell snap, inside the next level's four.
        let place = |world: &mut World, x: f32| {
            let mut cameras = world.query_filtered::<&mut GlobalTransform, With<Camera3d>>();
            for mut transform in cameras.iter_mut(world) {
                *transform = GlobalTransform::from_translation(Vec3::new(x, 20.0, x));
            }
        };
        place(&mut world, -3.5);
        run(&mut world);
        let count = surface_meshes(&mut world).len();

        place(&mut world, -1.5);
        run(&mut world);

        let rebuilt = rebuilt_levels(&mut world);
        assert!(count >= 3, "this terrain needs three levels");
        assert!(
            rebuilt.contains(&0),
            "the fine level must follow the camera: {rebuilt:?}",
        );
        assert!(
            rebuilt.len() < count,
            "the coarse levels must stand: {rebuilt:?} of {count}",
        );
    }

    /// Two terrains have their own surfaces; editing one leaves the
    /// other's meshes alone.
    #[test]
    fn a_stroke_on_one_terrain_leaves_another_terrains_surface_alone() {
        let mut world = World::new();
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<StandardMaterial>::default());
        world.insert_resource(TerrainPaintState::default());
        world.insert_resource(ActiveViewport::default());
        world.insert_resource(super::super::splat::TerrainSplatMaterials::default());
        world.insert_resource(super::super::regions::TerrainRegionView::default());
        let mut store = TerrainDataStore::default();
        store.insert("a.jdterrain", document(64, vec![1.0; 64 * 64]));
        store.insert("b.jdterrain", document(64, vec![9.0; 64 * 64]));
        world.insert_resource(store);
        world.spawn((Camera3d::default(), GlobalTransform::default()));

        let mut spawn = |path: &str| {
            world
                .spawn((
                    terrain(64, path),
                    TerrainDirtyChunks {
                        rebuild_all: true,
                        ..default()
                    },
                    GlobalTransform::default(),
                ))
                .id()
        };
        let a = spawn("a.jdterrain");
        let b = spawn("b.jdterrain");
        run(&mut world);

        let rebuilt_of = |world: &mut World, subject: Entity| {
            let mut query = world.query_filtered::<&TerrainSurface, Changed<BuiltLevel>>();
            query
                .iter(world)
                .filter(|surface| surface.terrain_entity == subject)
                .count()
        };

        world
            .get_mut::<TerrainDirtyChunks>(a)
            .unwrap()
            .dirty
            .insert((0, 0));
        run(&mut world);

        assert_eq!(
            rebuilt_of(&mut world, b),
            0,
            "terrain B was not edited and must keep its surface",
        );
        assert!(
            rebuilt_of(&mut world, a) > 0,
            "terrain A was edited and must have been rebuilt",
        );
    }

    /// Ground no region owns is not drawn, so an absent region costs its
    /// triangles and nothing else.
    #[test]
    fn absent_regions_are_not_drawn() {
        let (mut world, _) = world_with_terrain(64, vec![1.0; 64 * 64]);
        run(&mut world);
        let full: usize = mesh_triangles(&mut world);

        {
            // The terrain keeps one region, a long way from the ground the
            // camera is over. Nothing under the camera is authored, so
            // nothing there draws; the far region is real ground and the
            // coarsest level reaches it, so the count is not a bare zero.
            let mut store = world.resource_mut::<TerrainDataStore>();
            let data = store.remove("a.jdterrain").expect("keyed");
            let mut regions = data.regions.clone();
            regions.remove_region(jackdaw_terrain::RegionCoord::ORIGIN);
            regions.ensure_region(jackdaw_terrain::RegionCoord::new(40, 40));
            store.insert(
                "a.jdterrain",
                RegionTerrainData {
                    regions,
                    ..RegionTerrainData::default()
                },
            );
        }
        let mut query = world.query::<&mut TerrainDirtyChunks>();
        for mut dirty in query.iter_mut(&mut world) {
            dirty.rebuild_all = true;
        }
        run(&mut world);
        assert!(full > 0);
        let drawn = mesh_triangles(&mut world);
        assert!(
            drawn < full / 10,
            "absent ground was drawn: {drawn} of {full} triangles survived \
             moving the only region out from under the camera",
        );
    }

    /// A terrain holding no regions holds no ground, and there is no
    /// declared rectangle to draw an empty grid over.
    #[test]
    fn a_terrain_with_no_regions_draws_nothing() {
        let (mut world, _) = world_with_terrain(64, vec![1.0; 64 * 64]);
        run(&mut world);
        assert!(mesh_triangles(&mut world) > 0);

        {
            let mut store = world.resource_mut::<TerrainDataStore>();
            let data = store.remove("a.jdterrain").expect("keyed");
            let mut regions = data.regions.clone();
            regions.remove_region(jackdaw_terrain::RegionCoord::ORIGIN);
            store.insert(
                "a.jdterrain",
                RegionTerrainData {
                    regions,
                    ..RegionTerrainData::default()
                },
            );
        }
        let mut query = world.query::<&mut TerrainDirtyChunks>();
        for mut dirty in query.iter_mut(&mut world) {
            dirty.rebuild_all = true;
        }
        run(&mut world);
        assert_eq!(mesh_triangles(&mut world), 0);
    }

    /// A terrain stored as a grid of regions, all of them allocated and
    /// painted, so hiding one is the only thing that can take it off the
    /// screen.
    fn world_with_region_grid() -> (World, Entity) {
        let (mut world, entity) = world_with_terrain(64, vec![1.0; 64 * 64]);
        let mut regions = jackdaw_terrain::TerrainRegions::new(
            jackdaw_terrain::RegionSize::new(32).expect("a power of two"),
        );
        regions.ensure_grid(64).expect("inside the region cap");
        for z in 0..64 {
            for x in 0..64 {
                regions.set_height(x, z, 1.0);
                regions.set_control(x, z, jackdaw_terrain::Control::default().with_base_id(3));
            }
        }
        let mut store = world.resource_mut::<TerrainDataStore>();
        store.insert(
            "a.jdterrain",
            RegionTerrainData {
                regions,
                ..RegionTerrainData::default()
            },
        );
        (world, entity)
    }

    fn rebuild_everything(world: &mut World) {
        let mut query = world.query::<&mut TerrainDirtyChunks>();
        for mut dirty in query.iter_mut(world) {
            dirty.rebuild_all = true;
        }
    }

    /// Hiding the regions that are not active takes their triangles off the
    /// screen through the same filter an absent region goes through, and
    /// leaves every stored control word in place, so a save under this mode
    /// writes what a save under Full writes.
    #[test]
    fn hiding_the_other_regions_drops_their_quads_and_no_stored_word() {
        let (mut world, entity) = world_with_region_grid();
        rebuild_everything(&mut world);
        run(&mut world);
        let full = mesh_triangles(&mut world);
        assert!(full > 0, "the whole grid must draw to begin with");

        let before = world
            .resource::<TerrainDataStore>()
            .control("a.jdterrain")
            .into_owned();

        *world.resource_mut::<super::super::regions::TerrainRegionView>() =
            super::super::regions::TerrainRegionView {
                show_grid: true,
                active: Some((entity, jackdaw_terrain::RegionCoord::ORIGIN)),
                visibility: super::super::regions::RegionVisibility::Hidden,
            };
        rebuild_everything(&mut world);
        run(&mut world);

        let hidden = mesh_triangles(&mut world);
        assert!(hidden > 0, "the active region must still draw");
        assert!(
            hidden < full,
            "hiding three of four regions must drop triangles: {hidden} of {full}",
        );
        assert_eq!(
            world
                .resource::<TerrainDataStore>()
                .control("a.jdterrain")
                .into_owned(),
            before,
            "a view mode must not touch a stored control word",
        );
    }

    /// With nothing active there is no non-active region to hide, so
    /// Hidden draws the terrain whole rather than blanking it.
    #[test]
    fn hidden_with_no_active_region_still_draws_the_whole_terrain() {
        let (mut world, _) = world_with_region_grid();
        rebuild_everything(&mut world);
        run(&mut world);
        let full = mesh_triangles(&mut world);

        world
            .resource_mut::<super::super::regions::TerrainRegionView>()
            .visibility = super::super::regions::RegionVisibility::Hidden;
        rebuild_everything(&mut world);
        run(&mut world);
        assert_eq!(mesh_triangles(&mut world), full);
    }

    fn mesh_triangles(world: &mut World) -> usize {
        let mut query = world.query::<(&TerrainSurface, &Mesh3d)>();
        let handles: Vec<Handle<Mesh>> =
            query.iter(world).map(|(_, mesh)| mesh.0.clone()).collect();
        let meshes = world.resource::<Assets<Mesh>>();
        handles
            .iter()
            .filter_map(|handle| meshes.get(handle))
            .filter_map(Mesh::indices)
            .map(|indices| indices.len() / 3)
            .sum()
    }

    /// What each level puts on the screen: its triangle count, and whether
    /// it has a material to draw them with. A level with no material draws
    /// nothing.
    fn drawn_levels(world: &mut World) -> Vec<(u32, usize, bool)> {
        let mut query = world.query::<(
            &TerrainSurface,
            &Mesh3d,
            Has<MeshMaterial3d<StandardMaterial>>,
            Has<MeshMaterial3d<jackdaw_terrain::render::TerrainSplatMaterial>>,
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

    /// A whole-terrain rebuild never costs the screen a level.
    ///
    /// `rebuild_all` says the ground under every level changed, not that
    /// the levels stop drawing while their replacements are built. A level
    /// whose mesh or material were dropped up front and restored under the
    /// per-frame budget would show background and grid through the ground
    /// for as many frames as the budget took.
    ///
    /// Checked while the camera moves, since a standing camera would let
    /// every level be rebuilt the frame it was invalidated whatever the
    /// ordering.
    #[test]
    fn a_forced_rebuild_never_takes_a_level_off_the_screen() {
        let (mut world, entity) = world_with_terrain(1025, vec![0.0; 1025 * 1025]);
        let place = |world: &mut World, at: f32| {
            let mut cameras = world.query_filtered::<&mut GlobalTransform, With<Camera3d>>();
            for mut transform in cameras.iter_mut(world) {
                *transform = GlobalTransform::from_translation(Vec3::new(at, 200.0, at));
            }
        };
        place(&mut world, 0.0);
        run(&mut world);
        let before = drawn_levels(&mut world);
        assert!(before.len() >= 4, "a kilometre of terrain needs levels");
        assert!(
            before.iter().all(|(_, triangles, _)| *triangles > 0),
            "every level must draw to begin with: {before:?}",
        );

        world
            .get_mut::<TerrainDirtyChunks>(entity)
            .expect("the terrain tracks its dirty chunks")
            .rebuild_all = true;

        for frame in 1..=8 {
            place(&mut world, frame as f32 * 7.0);
            run(&mut world);
            let now = drawn_levels(&mut world);
            for (level, was, _) in &before {
                let still = now.iter().find(|(at, _, _)| at == level);
                let Some((_, triangles, material)) = still else {
                    panic!("frame {frame}: level {level} lost its mesh; {now:?}");
                };
                assert!(
                    *material,
                    "frame {frame}: level {level} has no material to draw with",
                );
                assert!(
                    *triangles > 0,
                    "frame {frame}: level {level} drew {was} triangles before the \
                     rebuild and none now; {now:?}",
                );
            }
        }
    }
}
