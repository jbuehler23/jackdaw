//! ActivePreview: translucent ghost-geometry overlay for modal operations.
//!
//! During modal ops (loop cut, knife, inset, extrude, bevel, slide, drag),
//! operators populate `ActivePreview` each frame with the proposed BrushTopology.
//! This module spawns/updates/despawns a `PreviewMesh` entity rendering that
//! topology as a translucent overlay.
//!
//! When `ActivePreview.brush_entity` is `None`, any existing preview mesh
//! entity is despawned.

use bevy::math::Vec3;
use bevy::prelude::*;

use jackdaw_geometry::{triangulate_polygon, BrushTopology};

#[derive(Resource, Default)]
pub struct ActivePreview {
    pub brush_entity: Option<Entity>,
    pub preview_topology: Option<BrushTopology>,
    pub state: PreviewState,
}

#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum PreviewState {
    #[default]
    Valid,
    Warning,
    Invalid,
}

#[derive(Component)]
pub struct PreviewMesh;

pub struct PreviewPlugin;

impl Plugin for PreviewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActivePreview>()
            .add_systems(Update, update_preview_mesh);
    }
}

/// Each frame: if `ActivePreview` has a populated topology, spawn or refresh
/// a translucent ghost mesh on the brush entity. If empty, despawn any
/// existing preview meshes.
fn update_preview_mesh(
    mut commands: Commands,
    preview: Res<ActivePreview>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    existing: Query<Entity, With<PreviewMesh>>,
) -> Result<(), BevyError> {
    // If preview is empty, despawn any existing preview entities.
    let Some(brush_entity) = preview.brush_entity else {
        for e in &existing {
            commands.entity(e).despawn();
        }
        return Ok(());
    };
    let Some(topology) = preview.preview_topology.as_ref() else {
        for e in &existing {
            commands.entity(e).despawn();
        }
        return Ok(());
    };
    if topology.polygons.is_empty() || topology.vertices.is_empty() {
        for e in &existing {
            commands.entity(e).despawn();
        }
        return Ok(());
    }

    // Despawn old preview entities (rebuilt fresh each frame; simple, no incremental updates).
    for e in &existing {
        commands.entity(e).despawn();
    }

    // Build a Bevy Mesh from the preview topology using per-triangle flat shading.
    // Use triangulate_polygon (earcut-backed, concave-aware) to handle non-convex faces.
    let positions_world: Vec<Vec3> = topology.vertices.iter().map(|v| v.position).collect();
    let mut mesh_positions: Vec<[f32; 3]> = Vec::new();
    let mut mesh_normals: Vec<[f32; 3]> = Vec::new();
    let mut mesh_indices: Vec<u32> = Vec::new();

    for face_idx in 0..topology.polygons.len() {
        let ring: Vec<u32> = topology.face_ring(face_idx).collect();
        if ring.len() < 3 {
            continue;
        }
        let normal = topology.face_normal_with(&positions_world, face_idx);
        let triangles = triangulate_polygon(&positions_world, &ring, normal);
        for [ia, ib, ic] in triangles {
            let a = positions_world[ia as usize];
            let b = positions_world[ib as usize];
            let c = positions_world[ic as usize];
            let base = mesh_positions.len() as u32;
            mesh_positions.push(a.to_array());
            mesh_positions.push(b.to_array());
            mesh_positions.push(c.to_array());
            mesh_normals.push(normal.to_array());
            mesh_normals.push(normal.to_array());
            mesh_normals.push(normal.to_array());
            mesh_indices.push(base);
            mesh_indices.push(base + 1);
            mesh_indices.push(base + 2);
        }
    }

    if mesh_positions.is_empty() {
        return Ok(());
    }

    let mut mesh = Mesh::new(
        bevy::mesh::PrimitiveTopology::TriangleList,
        bevy::asset::RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, mesh_positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, mesh_normals);
    mesh.insert_indices(bevy::mesh::Indices::U32(mesh_indices));

    let mesh_handle = meshes.add(mesh);

    let color = match preview.state {
        PreviewState::Valid => Color::srgba(0.3, 0.85, 1.0, 0.4),   // cyan
        PreviewState::Warning => Color::srgba(1.0, 0.75, 0.2, 0.4), // amber
        PreviewState::Invalid => Color::srgba(1.0, 0.3, 0.3, 0.4),  // red
    };
    let material = materials.add(StandardMaterial {
        base_color: color,
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        cull_mode: None, // visible from both sides
        depth_bias: 0.5, // slightly forward so it doesn't z-fight with the original brush
        ..default()
    });

    // Spawn as a child of the brush entity. Mesh positions are in the same local
    // space as the brush (topology coords match the brush transform), so we use
    // an identity local transform and let the parent's transform do the work.
    commands.spawn((
        Mesh3d(mesh_handle),
        MeshMaterial3d(material),
        Transform::default(),
        PreviewMesh,
        ChildOf(brush_entity),
    ));

    Ok(())
}
