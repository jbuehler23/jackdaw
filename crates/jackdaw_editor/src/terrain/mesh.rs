use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use super::{CHUNK_SIZE, TerrainChunk, TerrainDirtyChunks};
use crate::{EditorHidden, NonSerializable};

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (initialize_terrain_chunks, rebuild_dirty_chunks)
            .chain()
            .run_if(in_state(crate::AppState::Editor)),
    );
}

/// Default gray material for terrain chunks.
#[derive(Resource)]
struct TerrainMaterialHandle(Handle<StandardMaterial>);

/// When a Terrain component is added or `rebuild_all` is set, despawn old chunks and create new ones.
fn initialize_terrain_chunks(
    mut commands: Commands,
    mut terrains: Query<
        (Entity, &jackdaw_jsn::Terrain, &mut TerrainDirtyChunks),
        Or<(Added<jackdaw_jsn::Terrain>, Changed<TerrainDirtyChunks>)>,
    >,
    existing_chunks: Query<(Entity, &TerrainChunk)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    material_res: Option<Res<TerrainMaterialHandle>>,
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
                base_color: Color::srgb(0.5, 0.5, 0.5),
                perceptual_roughness: 0.9,
                metallic: 0.0,
                ..default()
            });
            commands.insert_resource(TerrainMaterialHandle(handle.clone()));
            handle
        };

        let heightmap = heightmap_from_terrain(terrain);
        let (cx_count, cz_count) = heightmap.chunk_count(CHUNK_SIZE);

        for cz in 0..cz_count {
            for cx in 0..cx_count {
                let mesh_data =
                    jackdaw_terrain::build_chunk_mesh_data(&heightmap, cx, cz, CHUNK_SIZE);

                let mesh = build_bevy_mesh(mesh_data);
                let mesh_handle = meshes.add(mesh);

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
                    EditorHidden,
                    NonSerializable,
                ));
            }
        }
    }
}

/// Rebuild meshes for chunks in the dirty set.
fn rebuild_dirty_chunks(
    mut terrains: Query<(&jackdaw_jsn::Terrain, &mut TerrainDirtyChunks)>,
    mut chunks: Query<(&TerrainChunk, &mut Mesh3d)>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    for (terrain, mut dirty) in &mut terrains {
        if dirty.dirty.is_empty() {
            continue;
        }

        let heightmap = heightmap_from_terrain(terrain);
        let dirty_set: Vec<(u32, u32)> = dirty.dirty.drain().collect();

        for (chunk, mut mesh3d) in &mut chunks {
            if dirty_set.contains(&(chunk.chunk_x, chunk.chunk_z)) {
                let mesh_data = jackdaw_terrain::build_chunk_mesh_data(
                    &heightmap,
                    chunk.chunk_x,
                    chunk.chunk_z,
                    CHUNK_SIZE,
                );

                let mesh = build_bevy_mesh(mesh_data);
                mesh3d.0 = meshes.add(mesh);
            }
        }
    }
}

fn heightmap_from_terrain(terrain: &jackdaw_jsn::Terrain) -> jackdaw_terrain::Heightmap {
    jackdaw_terrain::Heightmap {
        resolution: terrain.resolution,
        size: terrain.size,
        max_height: terrain.max_height,
        heights: terrain.heights.clone(),
    }
}

fn build_bevy_mesh(data: jackdaw_terrain::ChunkMeshData) -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, data.positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, data.normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, data.uvs);
    mesh.insert_indices(Indices::U32(data.indices));
    mesh
}
