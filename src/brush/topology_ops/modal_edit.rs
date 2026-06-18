//! Snapshot, restore, and re-apply lifecycle for interactive (modal) topology
//! edits. An interactive edit (extrude, inset, bevel, loop cut) snapshots the
//! brush when it begins, then each frame restores to that snapshot and
//! re-applies the edit at the current parameter, so the edit never accumulates.
//! Pure data over the kernel `apply_topology_edit` seam, so it is testable
//! without an ECS world.

use jackdaw_geometry::halfedge::{HalfedgeMesh, apply_topology_edit};
use jackdaw_jsn::Brush;

use crate::brush::BrushHalfedge;

/// The pre-edit snapshot of a brush and its half-edge binding, captured when a
/// modal op begins.
pub struct ModalTopologyEdit {
    brush: Brush,
    halfedge: BrushHalfedge,
}

impl ModalTopologyEdit {
    /// Snapshot the brush and its binding at modal start.
    pub fn begin(brush: &Brush, halfedge: &BrushHalfedge) -> Self {
        Self {
            brush: brush.clone(),
            halfedge: halfedge.clone(),
        }
    }

    /// Restore to the snapshot, then apply `edit` and reconcile. Returns the
    /// edit's result so the op can update selection or appearance from it.
    pub fn apply<R>(
        &self,
        brush: &mut Brush,
        halfedge: &mut BrushHalfedge,
        edit: impl FnOnce(&mut HalfedgeMesh) -> R,
    ) -> R {
        self.restore(brush, halfedge);
        apply_topology_edit(&mut brush.faces, &mut brush.topology, &mut halfedge.0, edit)
    }

    /// Reset the live brush and binding to the snapshot. Used on cancel and
    /// before each frame's re-apply.
    pub fn restore(&self, brush: &mut Brush, halfedge: &mut BrushHalfedge) {
        *brush = self.brush.clone();
        *halfedge = self.halfedge.clone();
    }

    /// The pre-edit brush. A modal op reads this to size and source the
    /// appearance of faces its edit creates (the snapshot face count marks
    /// where new faces begin; a snapshot face supplies their material and UV).
    pub fn snapshot_brush(&self) -> &Brush {
        &self.brush
    }

    /// The pre-edit half-edge binding. A modal op that previews speculatively
    /// off the snapshot (rather than mutating the live brush every frame) reads
    /// this so the keys it resolved at `begin` stay valid for the whole modal.
    pub fn snapshot_halfedge(&self) -> &BrushHalfedge {
        &self.halfedge
    }
}

#[cfg(test)]
mod tests {
    use super::ModalTopologyEdit;
    use crate::brush::BrushHalfedge;
    use jackdaw_jsn::Brush;

    #[test]
    fn restore_returns_the_brush_to_its_snapshot() {
        let brush = Brush::cuboid(1.0, 1.0, 1.0);
        let halfedge = BrushHalfedge::from_topology(&brush.topology);
        let snapshot_faces = brush.faces.len();

        let modal = ModalTopologyEdit::begin(&brush, &halfedge);

        let mut live = brush.clone();
        let mut live_he = halfedge.clone();
        live.faces.truncate(0);

        modal.restore(&mut live, &mut live_he);
        assert_eq!(
            live.faces.len(),
            snapshot_faces,
            "restore rewinds to snapshot"
        );
    }

    #[test]
    fn apply_reconciles_and_returns_the_edit_result() {
        let brush = Brush::cuboid(1.0, 1.0, 1.0);
        let halfedge = BrushHalfedge::from_topology(&brush.topology);
        let face_count = brush.faces.len();

        let modal = ModalTopologyEdit::begin(&brush, &halfedge);
        let mut live = brush.clone();
        let mut live_he = halfedge.clone();

        // A no-op edit restores the snapshot, runs the reconcile, and returns
        // the closure's value; the face count is unchanged.
        let returned = modal.apply(&mut live, &mut live_he, |_mesh| 42);
        assert_eq!(returned, 42, "apply returns the edit closure's result");
        assert_eq!(
            live.faces.len(),
            face_count,
            "no-op edit preserves face count"
        );
    }
}
