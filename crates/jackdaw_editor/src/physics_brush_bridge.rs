//! Bridge between editor collider configuration and avian `Collider` components.
//!
//! The user adds `AvianCollider` via the inspector. This module builds
//! the actual `Collider` from it  -- handling both mesh-backed entities and
//! brush entities (which have `BrushMeshCache` instead of `Mesh3d`).
//!
//! `ColliderConstructor` is never placed on entities, so avian's
//! `init_collider_constructors` system never fires and can't interfere.

use avian3d::prelude::*;
use bevy::prelude::*;
use jackdaw_avian_integration::AvianCollider;

use crate::brush::BrushMeshCache;

pub struct PhysicsBrushBridgePlugin;

impl Plugin for PhysicsBrushBridgePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            sync_editor_collider_config.run_if(in_state(crate::AppState::Editor)),
        )
        .add_observer(remove_collider_when_avian_collider_removed);
    }
}

/// When the user-facing `AvianCollider` is removed (e.g. physics toggle off,
/// or undo of enable-physics), also remove the runtime `Collider` we built
/// from it. Without this, the collider gizmo keeps being drawn after undo.
fn remove_collider_when_avian_collider_removed(
    trigger: On<Remove, AvianCollider>,
    mut commands: Commands,
) {
    let entity = trigger.event_target();
    if let Ok(mut ec) = commands.get_entity(entity) {
        ec.try_remove::<Collider>();
    }
}

/// When `AvianCollider` is added or changed, build a `Collider` from
/// the inner `ColliderConstructor` and insert it directly. Handles both
/// mesh-backed entities (reads from `Mesh3d`) and brushes (reads from
/// `BrushMeshCache`).
fn sync_editor_collider_config(
    mut commands: Commands,
    changed: Query<
        (
            Entity,
            &AvianCollider,
            Option<&BrushMeshCache>,
            Option<&Mesh3d>,
        ),
        Changed<AvianCollider>,
    >,
    meshes: Res<Assets<Mesh>>,
) {
    for (entity, config, brush_cache, mesh3d) in &changed {
        let constructor = &config.0;

        let collider = if constructor.requires_mesh() {
            // Try brush geometry first, then mesh asset
            if let Some(brush_cache) = brush_cache {
                let Some(mesh) = brush_mesh_from_cache(brush_cache) else {
                    continue;
                };
                Collider::try_from_constructor(constructor.clone(), Some(&mesh))
            } else if let Some(mesh3d) = mesh3d {
                let Some(mesh) = meshes.get(&mesh3d.0) else {
                    continue;
                };
                Collider::try_from_constructor(constructor.clone(), Some(mesh))
            } else {
                continue;
            }
        } else {
            Collider::try_from_constructor(constructor.clone(), None)
        };

        if let Some(collider) = collider {
            commands.entity(entity).insert(collider);
        }
    }
}

/// Build a triangulated `Mesh` from a `BrushMeshCache`, fan-triangulating each face polygon.
fn brush_mesh_from_cache(cache: &BrushMeshCache) -> Option<Mesh> {
    if cache.vertices.is_empty() {
        return None;
    }
    let positions: Vec<[f32; 3]> = cache.vertices.iter().map(|v| [v.x, v.y, v.z]).collect();
    let mut indices: Vec<u32> = Vec::new();
    for polygon in &cache.face_polygons {
        if polygon.len() >= 3 {
            for i in 1..polygon.len() - 1 {
                indices.push(polygon[0] as u32);
                indices.push(polygon[i] as u32);
                indices.push(polygon[i + 1] as u32);
            }
        }
    }
    let mut m = Mesh::new(
        bevy::mesh::PrimitiveTopology::TriangleList,
        bevy::asset::RenderAssetUsages::default(),
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    m.insert_indices(bevy::mesh::Indices::U32(indices));
    Some(m)
}
