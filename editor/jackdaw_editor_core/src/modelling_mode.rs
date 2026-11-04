//! Modelling state the core reads without the modelling tools: the knife path
//! being built, which undo and the edit-mode exit consult, and the names other
//! areas refer to the modelling tools by.

use bevy::prelude::*;

/// Catalog name of the modelling extension.
pub const MODELLING_EDITOR_EXTENSION_ID: &str = "jackdaw.modelling";

/// The operator that starts measuring a distance in the viewport.
pub const MEASURE_DISTANCE_OP: &str = "tools.measure_distance";

/// Kind of snap target chosen for the current cursor position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KnifeSnapKind {
    /// Snapped to an existing vert within tolerance.
    Vertex,
    /// Snapped to an edge midpoint (Shift held + within edge tolerance).
    EdgeMidpoint,
    /// Snapped to a grid point on the face plane (translate snap active).
    GridPoint,
    /// Snapped to the closest point on an edge.
    EdgePoint,
    /// Cursor is inside a face but not near any vert or edge. Commit
    /// will `face_poke` a center vert at this position.
    FaceInterior,
    /// Snapped to a point already placed in the current path. The
    /// referenced path-point index lets the commit reuse that point's
    /// resolved vert without creating duplicate geometry.
    PathPoint,
}

/// A snap target captured at a single frame's cursor position. Also used
/// as a path-point payload once an LMB click commits one.
#[derive(Clone, Debug)]
pub struct KnifeSnapTarget {
    /// World-space snap point (for preview).
    pub world_pos: Vec3,
    /// Brush-local snap point (for stable mesh ops).
    pub local_pos: Vec3,
    pub kind: KnifeSnapKind,
    /// Which face of the brush the snap was computed against. Authored
    /// index: the knife snaps only to authored geometry, so the live
    /// mirrored half is never a snap candidate.
    pub face_idx: usize,
    /// Canonical (min, max) vert indices of the edge the snap is on, or
    /// `None` if the snap landed on an existing vert. Authored indices.
    pub edge_pair: Option<(usize, usize)>,
    /// Existing vert index if the snap reused a corner. Authored index,
    /// resolved against `Brush::topology` at commit.
    pub vert_idx: Option<usize>,
    /// When `kind == PathPoint`, the index into the live `path` that
    /// the cursor snapped to. `None` for every other kind.
    pub path_point_idx: Option<usize>,
}

/// A point already placed by an LMB click. Holds enough information to
/// resolve the corresponding `VertKey` in a freshly-lifted `HalfedgeMesh`
/// at commit time (edge-splitting, face-poking, or vert-reuse).
#[derive(Clone, Debug)]
pub struct KnifePathPoint {
    pub world_pos: Vec3,
    pub local_pos: Vec3,
    pub face_idx: usize,
    pub kind: KnifeSnapKind,
    pub edge_pair: Option<(usize, usize)>,
    pub vert_idx: Option<usize>,
    /// When this entry was a `PathPoint` snap (re-click on an existing
    /// path point), the index of the original point it references.
    /// At commit time the resolved `VertKey` of the original point is
    /// reused, preventing duplicate geometry at the same position.
    pub source_path_idx: Option<usize>,
}

impl From<&KnifeSnapTarget> for KnifePathPoint {
    fn from(s: &KnifeSnapTarget) -> Self {
        Self {
            world_pos: s.world_pos,
            local_pos: s.local_pos,
            face_idx: s.face_idx,
            kind: s.kind,
            edge_pair: s.edge_pair,
            vert_idx: s.vert_idx,
            source_path_idx: s.path_point_idx,
        }
    }
}

/// Knife edit-mode state. Same shape as `ClipState`: cleared
/// whenever the user leaves the mode or commits the cuts.
#[derive(Resource, Default)]
pub struct KnifeMode {
    /// The brush this knife path is being built on. Cleared when the
    /// mode is exited or the active brush changes.
    pub brush_entity: Option<Entity>,
    /// Points clicked so far. Each adjacent pair becomes one (or two)
    /// face bisections at commit time.
    pub path: Vec<KnifePathPoint>,
    /// Snap target under the cursor this frame. Refreshed every frame
    /// by `handle_knife_mode`. `None` when off-brush, off-face, or
    /// outside the snap tolerance.
    pub hover_snap: Option<KnifeSnapTarget>,
    /// World-space points where the current path's segments cross
    /// existing mesh edges. Predicted every frame from the path + live
    /// cursor; rendered as small markers so the user can see, before
    /// committing, which existing edges the cut will split. Mirrors
    /// live edge-crossing preview dots during drag.
    pub preview_intersections: Vec<Vec3>,
    /// Points popped off `path` by in-modal Ctrl+Z, kept so Ctrl+Shift+Z
    /// can re-add them. Cleared when a new point is placed (a fresh
    /// branch invalidates the redo trail) or when the modal exits.
    pub undone_path: Vec<KnifePathPoint>,
}

impl KnifeMode {
    pub fn clear(&mut self) {
        self.brush_entity = None;
        self.path.clear();
        self.hover_snap = None;
        self.preview_intersections.clear();
        self.undone_path.clear();
    }

    /// Pop the most recently placed path point onto the redo stack.
    /// Returns true if a point was popped.
    pub fn undo_point(&mut self) -> bool {
        if let Some(p) = self.path.pop() {
            self.undone_path.push(p);
            true
        } else {
            false
        }
    }

    /// Re-add the most recently undone path point. Returns true if a
    /// point was re-added.
    pub fn redo_point(&mut self) -> bool {
        if let Some(p) = self.undone_path.pop() {
            self.path.push(p);
            true
        } else {
            false
        }
    }
}
