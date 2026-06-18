//! Edit-mode lifecycle: lift `BrushTopology` to `HalfedgeMesh` on enter, flatten back
//! and remove on exit.
//!
//! `BrushHalfedge` is the in-memory edit-time mesh. Present on the entity
//! while that brush is in Vertex / Edge / Face / Knife mode. Clip mode does
//! not lift an `HalfedgeMesh` (it operates on the plane representation
//! directly).

use bevy::prelude::*;
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMode, BrushSelection, EditMode};

/// In-memory `HalfedgeBinding` edit form for the brush currently in V/E/F edit
/// mode. A thin component wrapper over the kernel binding (see ADR 0001). Field
/// accesses (`mesh`, `vert_keys`, `face_keys`) resolve through `Deref`.
#[derive(Component, Deref, DerefMut, Clone)]
pub struct BrushHalfedge(pub jackdaw_geometry::halfedge::HalfedgeBinding);

impl BrushHalfedge {
    /// Lift a brush topology into an edit-time binding.
    pub fn from_topology(topology: &jackdaw_jsn::BrushTopology) -> Self {
        Self(jackdaw_geometry::halfedge::HalfedgeBinding::lift_from_topology(topology))
    }
}

/// When entering Vertex / Edge / Face mode, lift each edit brush's topology
/// into `HalfedgeMesh` and insert the component on those entities. Every edit
/// brush (not just the active one) gets a live mesh so cross-brush sub-element
/// edits stay index-stable and concavity-preserving; the convex-hull rebuild
/// fallback would reorder vertices and convexify a brush. When the resource
/// value changes (mode toggle, selection change), stale `BrushHalfedge`
/// components are removed first.
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

    let targets: Vec<Entity> = match *edit_mode {
        EditMode::BrushEdit(BrushEditMode::Vertex)
        | EditMode::BrushEdit(BrushEditMode::Edge)
        | EditMode::BrushEdit(BrushEditMode::Face)
        | EditMode::BrushEdit(BrushEditMode::Knife) => selection.edit_brushes().collect(),
        _ => Vec::new(),
    };

    // Remove BrushHalfedge from any entity that is no longer an edit brush.
    for e in &existing {
        if !targets.contains(&e) {
            commands.entity(e).remove::<BrushHalfedge>();
        }
    }

    // Add BrushHalfedge to each edit brush not already carrying one.
    for &e in &targets {
        if existing.contains(e) {
            continue;
        }
        let Ok(brush) = brush_q.get(e) else {
            continue;
        };
        // Guard against the degenerate empty-brush case (no faces, no
        // topology). A legacy unmigrated brush keeps working via the plane
        // path; HalfedgeMesh-driven ops wait for topology to be populated.
        if brush.topology.polygons.is_empty() {
            continue;
        }
        commands
            .entity(e)
            .insert(BrushHalfedge::from_topology(&brush.topology));
    }
    Ok(())
}
