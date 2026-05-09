//! Edit-mode lifecycle: lift `BrushTopology` to `BMesh` on enter, flatten back
//! and remove on exit.
//!
//! `BrushBMesh` is the in-memory edit-time mesh. Only present on the entity
//! while that brush is in vertex / edge / face mode. Clip mode does not lift
//! a BMesh (it operates on the plane representation directly until A.4.x).

use bevy::prelude::*;
use jackdaw_geometry::bmesh::{BMesh, FaceKey, VertKey};
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMode, BrushSelection, EditMode};

/// In-memory BMesh edit form for the brush currently in V/E/F edit mode.
#[derive(Component)]
pub struct BrushBMesh {
    pub mesh: BMesh,
    /// Parallel to `BrushTopology::vertices` index at lift time.
    pub vert_keys: Vec<VertKey>,
    /// Parallel to `BrushTopology::polygons` index at lift time.
    pub face_keys: Vec<FaceKey>,
}

/// When entering Vertex / Edge / Face mode, lift the selected brush's topology
/// into BMesh and insert the component on that entity. When the resource value
/// changes (mode toggle, brush switch), remove any stale `BrushBMesh` first.
pub fn sync_brush_bmesh_on_edit_mode(
    mut commands: Commands,
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
    brush_q: Query<&Brush>,
    existing: Query<Entity, With<BrushBMesh>>,
) -> Result<(), BevyError> {
    if !edit_mode.is_changed() && !selection.is_changed() {
        return Ok(());
    }

    let target_entity: Option<Entity> = match *edit_mode {
        EditMode::BrushEdit(BrushEditMode::Vertex)
        | EditMode::BrushEdit(BrushEditMode::Edge)
        | EditMode::BrushEdit(BrushEditMode::Face) => selection.entity,
        _ => None,
    };

    // Remove BrushBMesh from any entity that should NOT have it.
    for e in &existing {
        if Some(e) != target_entity {
            commands.entity(e).remove::<BrushBMesh>();
        }
    }

    // Add BrushBMesh to target if not already present.
    if let Some(e) = target_entity {
        if !existing.contains(e) {
            if let Ok(brush) = brush_q.get(e) {
                if !brush.topology.polygons.is_empty() {
                    let bmesh = BMesh::lift_from_topology(&brush.topology);
                    let vert_keys: Vec<VertKey> = bmesh.verts.keys().collect();
                    let mut face_keys: Vec<FaceKey> =
                        vec![FaceKey::default(); bmesh.faces.len()];
                    for (k, f) in bmesh.faces.iter() {
                        let slot = f.material_idx as usize;
                        if slot < face_keys.len() {
                            face_keys[slot] = k;
                        }
                    }
                    commands.entity(e).insert(BrushBMesh {
                        mesh: bmesh,
                        vert_keys,
                        face_keys,
                    });
                }
                // If topology is empty (legacy unmigrated brush), don't lift —
                // wait for A.17.1 migration to populate topology first. Edit mode
                // can still partially work via the legacy plane path, but BMesh-driven
                // ops won't be available until migration.
            }
        }
    }
    Ok(())
}
