//! Edit-mode lifecycle: lift `BrushTopology` to `HalfedgeMesh` on enter, flatten back
//! and remove on exit.
//!
//! `BrushHalfedge` is the in-memory edit-time mesh. Present on the entity
//! while that brush is in Vertex / Edge / Face / Knife mode. Clip mode does
//! not lift an `HalfedgeMesh` (it operates on the plane representation directly
//! until A.4.x).

use bevy::prelude::*;
use jackdaw_geometry::halfedge::{FaceKey, HalfedgeMesh, VertKey};
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMode, BrushSelection, EditMode};

/// In-memory `HalfedgeMesh` edit form for the brush currently in V/E/F edit mode.
#[derive(Component)]
pub struct BrushHalfedge {
    pub mesh: HalfedgeMesh,
    /// Parallel to `BrushTopology::vertices` index at lift time.
    pub vert_keys: Vec<VertKey>,
    /// Parallel to `BrushTopology::polygons` index at lift time.
    pub face_keys: Vec<FaceKey>,
}

/// When entering Vertex / Edge / Face mode, lift the selected brush's topology
/// into `HalfedgeMesh` and insert the component on that entity. When the resource value
/// changes (mode toggle, brush switch), remove any stale `BrushHalfedge` first.
pub fn sync_brush_halfedge_on_edit_mode(
    mut commands: Commands,
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
    brush_q: Query<&Brush>,
    existing: Query<Entity, With<BrushHalfedge>>,
) -> Result<(), BevyError> {
    if !edit_mode.is_changed() && !selection.is_changed() {
        return Ok(());
    }

    let target_entity: Option<Entity> = match *edit_mode {
        EditMode::BrushEdit(BrushEditMode::Vertex)
        | EditMode::BrushEdit(BrushEditMode::Edge)
        | EditMode::BrushEdit(BrushEditMode::Face)
        | EditMode::BrushEdit(BrushEditMode::Knife) => selection.entity,
        _ => None,
    };

    // Remove BrushHalfedge from any entity that should NOT have it.
    for e in &existing {
        if Some(e) != target_entity {
            commands.entity(e).remove::<BrushHalfedge>();
        }
    }

    // Add BrushHalfedge to target if not already present.
    if let Some(e) = target_entity
        && !existing.contains(e)
        && let Ok(brush) = brush_q.get(e)
    {
        // Guard against the degenerate empty-brush case
        // (no faces, no topology).
        if !brush.topology.polygons.is_empty() {
            let mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
            let vert_keys: Vec<VertKey> = mesh.verts.keys().collect();
            let mut face_keys: Vec<FaceKey> = vec![FaceKey::default(); mesh.faces.len()];
            for (k, f) in mesh.faces.iter() {
                let slot = f.material_idx as usize;
                if slot < face_keys.len() {
                    face_keys[slot] = k;
                }
            }
            commands.entity(e).insert(BrushHalfedge {
                mesh,
                vert_keys,
                face_keys,
            });
        }
        // If topology is empty (legacy unmigrated brush), don't lift -
        // wait for A.17.1 migration to populate topology first. Edit mode
        // can still partially work via the legacy plane path, but HalfedgeMesh-driven
        // ops won't be available until migration.
    }
    Ok(())
}
