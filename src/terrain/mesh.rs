use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use super::{CHUNK_SIZE, TerrainChunk, TerrainDataStore, TerrainDirtyChunks, TerrainPaintState};

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (
            rebuild_on_channel_view_change,
            initialize_terrain_chunks,
            rebuild_dirty_chunks,
        )
            .chain()
            .run_if(in_state(crate::AppState::Editor)),
    );
}

/// Shared material for terrain chunks.
///
/// `base_color` is white and every chunk carries a vertex-colour
/// attribute, so the tint lives entirely in the mesh: unpainted ground
/// writes [`UNPAINTED`] and reads exactly as the flat grey it always did,
/// while the channel debug view writes palette colours into the same
/// attribute. One material, no per-terrain variants, and toggling the
/// view is a mesh rebuild rather than a material swap.
#[derive(Resource)]
struct TerrainMaterialHandle(Handle<StandardMaterial>);

/// Vertex colour for ground with nothing painted on it, and for every
/// vertex when the channel view is off. Matches the material's previous
/// hardcoded `base_color`, so the default look is unchanged.
const UNPAINTED: [f32; 4] = [0.5, 0.5, 0.5, 1.0];

/// Rebuild every terrain when the channel being visualised changes.
///
/// [`TerrainPaintState`] also carries the brush position, which changes
/// on almost every frame, so a bare `is_changed()` here would rebuild the
/// world continuously. Only the two fields the mesh actually depends on
/// are compared.
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

/// When a Terrain component is added or `rebuild_all` is set, despawn old chunks and create new ones.
fn initialize_terrain_chunks(
    mut commands: Commands,
    mut terrains: Query<
        (
            Entity,
            &jackdaw_scene_types::Terrain,
            &mut TerrainDirtyChunks,
        ),
        Or<(
            Added<jackdaw_scene_types::Terrain>,
            Changed<TerrainDirtyChunks>,
        )>,
    >,
    existing_chunks: Query<(Entity, &TerrainChunk)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    material_res: Option<Res<TerrainMaterialHandle>>,
    store: Res<TerrainDataStore>,
    paint: Res<TerrainPaintState>,
) {
    for (terrain_entity, terrain, mut dirty) in &mut terrains {
        if !dirty.rebuild_all {
            continue;
        }
        dirty.rebuild_all = false;
        dirty.dirty.clear();

        // Despawn old chunks for this terrain
        for (chunk_entity, chunk) in &existing_chunks {
            if chunk.terrain_entity == terrain_entity {
                commands.entity(chunk_entity).despawn();
            }
        }

        // Ensure terrain material exists
        let mat_handle = if let Some(ref res) = material_res {
            res.0.clone()
        } else {
            let handle = materials.add(StandardMaterial {
                base_color: Color::WHITE,
                perceptual_roughness: 0.9,
                metallic: 0.0,
                ..default()
            });
            commands.insert_resource(TerrainMaterialHandle(handle.clone()));
            handle
        };

        let heightmap = heightmap_from_terrain(terrain, &store);
        let (cx_count, cz_count) = heightmap.chunk_count(CHUNK_SIZE);

        for cz in 0..cz_count {
            for cx in 0..cx_count {
                let mesh_data = chunk_mesh_data(&heightmap, terrain, cx, cz);
                let colors = chunk_vertex_colors(terrain, &store, &paint, &mesh_data.grid);
                let mesh = build_bevy_mesh(mesh_data, colors);
                let mesh_handle = meshes.add(mesh);

                // `TerrainChunk` requires `EditorHidden +
                // NonSerializable`; nothing to insert here.
                commands.spawn((
                    TerrainChunk {
                        terrain_entity,
                        chunk_x: cx,
                        chunk_z: cz,
                    },
                    Mesh3d(mesh_handle),
                    MeshMaterial3d(mat_handle.clone()),
                    Transform::default(),
                    Visibility::default(),
                    ChildOf(terrain_entity),
                ));
            }
        }
    }
}

/// Rebuild meshes for chunks in the dirty set.
fn rebuild_dirty_chunks(
    mut terrains: Query<(
        Entity,
        &jackdaw_scene_types::Terrain,
        &mut TerrainDirtyChunks,
    )>,
    mut chunks: Query<(&TerrainChunk, &mut Mesh3d)>,
    mut meshes: ResMut<Assets<Mesh>>,
    store: Res<TerrainDataStore>,
    paint: Res<TerrainPaintState>,
) {
    for (terrain_entity, terrain, mut dirty) in &mut terrains {
        if dirty.dirty.is_empty() {
            continue;
        }

        let heightmap = heightmap_from_terrain(terrain, &store);
        let dirty_set: Vec<(u32, u32)> = dirty.dirty.drain().collect();

        // Chunk coordinates are small integers starting at (0, 0) for
        // every terrain, so two terrains routinely share a (chunk_x,
        // chunk_z) pair. Without the owner filter, a coordinate match
        // alone would rebuild another terrain's chunk with this
        // terrain's heightmap.
        for (chunk, mut mesh3d) in &mut chunks {
            if chunk.terrain_entity == terrain_entity
                && dirty_set.contains(&(chunk.chunk_x, chunk.chunk_z))
            {
                let mesh_data = chunk_mesh_data(&heightmap, terrain, chunk.chunk_x, chunk.chunk_z);
                let colors = chunk_vertex_colors(terrain, &store, &paint, &mesh_data.grid);
                let mesh = build_bevy_mesh(mesh_data, colors);
                mesh3d.0 = meshes.add(mesh);
            }
        }
    }
}

/// Build the heightmap a chunk mesh is generated from.
///
/// Heights come from the store, never from the component: the component's
/// `heights` field is a legacy inlet that is empty on every terrain this
/// build created. A terrain the store has never heard of (a missing
/// sidecar) reads as flat, which is the documented fallback.
pub(super) fn heightmap_from_terrain(
    terrain: &jackdaw_scene_types::Terrain,
    store: &TerrainDataStore,
) -> jackdaw_terrain::Heightmap {
    let mut heights = store.heights(&terrain.data_path).to_vec();
    heights.resize(terrain.cell_count(), 0.0);
    jackdaw_terrain::Heightmap {
        resolution: terrain.resolution,
        size: terrain.size,
        max_height: terrain.max_height,
        heights,
    }
}

/// Pick the mesher this terrain is drawn with.
///
/// A quantized terrain is meshed flat: its heights sit on an elevation
/// lattice, and the smooth mesher's central-difference normals blend
/// straight across the risers, so the terracing that is in the data
/// never reaches the screen. Everything else is meshed exactly as
/// before.
fn chunk_mesh_data(
    heightmap: &jackdaw_terrain::Heightmap,
    terrain: &jackdaw_scene_types::Terrain,
    chunk_x: u32,
    chunk_z: u32,
) -> jackdaw_terrain::ChunkMeshData {
    if terrain.quantization.enabled {
        jackdaw_terrain::build_chunk_mesh_data_flat(heightmap, chunk_x, chunk_z, CHUNK_SIZE)
    } else {
        jackdaw_terrain::build_chunk_mesh_data(heightmap, chunk_x, chunk_z, CHUNK_SIZE)
    }
}

/// Colour every vertex of a chunk by what is painted under it.
///
/// This is jackdaw's answer to `Terrain3D`'s control-texture debug view:
/// without it the user is painting data they cannot see. Off, or with no
/// channel selected, every vertex is [`UNPAINTED`] and the terrain looks
/// exactly as it did before channels existed.
///
/// The colours are derived from `grid` -- the grid coordinate the mesher
/// recorded for each vertex it emitted -- rather than by re-walking the
/// chunk window here. The smooth and flat meshers emit different vertex
/// counts in different orders, and a second walk would have to guess
/// which one ran; reading their own record cannot fall out of step.
/// Values the palette does not name draw as unpainted rather than as an
/// error colour: an unnamed value is data the project has not described
/// yet, not a fault.
fn chunk_vertex_colors(
    terrain: &jackdaw_scene_types::Terrain,
    store: &TerrainDataStore,
    paint: &TerrainPaintState,
    grid: &[[u32; 2]],
) -> Vec<[f32; 4]> {
    let res = terrain.resolution;
    let descriptor = terrain.channels.get(paint.active_channel);
    let values = store
        .get(&terrain.data_path)
        .and_then(|data| data.channels.get(paint.active_channel));
    let (Some(descriptor), Some(values)) = (descriptor, values) else {
        return vec![UNPAINTED; grid.len()];
    };
    if !paint.show_channel {
        return vec![UNPAINTED; grid.len()];
    }

    grid.iter()
        .map(|[gx, gz]| {
            descriptor
                .color_of(values.get(res, *gx, *gz))
                .map(|color| color.to_linear().to_f32_array())
                .unwrap_or(UNPAINTED)
        })
        .collect()
}

fn build_bevy_mesh(data: jackdaw_terrain::ChunkMeshData, colors: Vec<[f32; 4]>) -> Mesh {
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
mod rebuild_dirty_chunks_tests {
    use jackdaw_terrain::TerrainData;

    use super::*;

    fn terrain(data_path: &str) -> jackdaw_scene_types::Terrain {
        jackdaw_scene_types::Terrain {
            resolution: 2,
            size: Vec2::splat(2.0),
            data_path: data_path.to_string(),
            ..default()
        }
    }

    fn placeholder_mesh() -> Mesh {
        Mesh::new(PrimitiveTopology::TriangleList, default())
    }

    /// I6 pinning test, the exact scenario from the review finding: two
    /// terrains whose chunk grids both start at (0, 0) share that
    /// coordinate. Only one of them is dirty; before the owner filter,
    /// the coordinate-only match rebuilt the other terrain's chunk too,
    /// with the wrong terrain's heightmap.
    #[test]
    fn a_coordinate_match_on_another_terrains_chunk_is_left_alone() {
        let mut world = World::new();
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(TerrainPaintState::default());

        let mut store = TerrainDataStore::default();
        store.insert(
            "a.jdterrain",
            TerrainData {
                resolution: 2,
                heights: vec![1.0; 4],
                channels: vec![],
            },
        );
        store.insert(
            "b.jdterrain",
            TerrainData {
                resolution: 2,
                heights: vec![9.0; 4],
                channels: vec![],
            },
        );
        world.insert_resource(store);

        let terrain_a = world.spawn(terrain("a.jdterrain")).id();
        let terrain_b = world.spawn(terrain("b.jdterrain")).id();

        let mut dirty_a = TerrainDirtyChunks::default();
        dirty_a.dirty.insert((0, 0));
        world.entity_mut(terrain_a).insert(dirty_a);
        world
            .entity_mut(terrain_b)
            .insert(TerrainDirtyChunks::default());

        let b_original_handle = {
            let mut meshes = world.resource_mut::<Assets<Mesh>>();
            meshes.add(placeholder_mesh())
        };
        let chunk_b = world
            .spawn((
                TerrainChunk {
                    terrain_entity: terrain_b,
                    chunk_x: 0,
                    chunk_z: 0,
                },
                Mesh3d(b_original_handle.clone()),
            ))
            .id();

        let a_original_handle = {
            let mut meshes = world.resource_mut::<Assets<Mesh>>();
            meshes.add(placeholder_mesh())
        };
        let chunk_a = world
            .spawn((
                TerrainChunk {
                    terrain_entity: terrain_a,
                    chunk_x: 0,
                    chunk_z: 0,
                },
                Mesh3d(a_original_handle.clone()),
            ))
            .id();

        world
            .run_system_cached(rebuild_dirty_chunks)
            .expect("system runs");

        assert_eq!(
            world.get::<Mesh3d>(chunk_b).unwrap().0,
            b_original_handle,
            "terrain B's chunk was not dirty and must keep its original mesh",
        );
        assert_ne!(
            world.get::<Mesh3d>(chunk_a).unwrap().0,
            a_original_handle,
            "terrain A's chunk was dirty and must have been rebuilt",
        );
    }
}
