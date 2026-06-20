//! Convex-hull brush math.
//!
//! The pure geometry lives in the engine-agnostic `jackdaw_hull` crate. This
//! module re-exports it and wraps `rebuild_brush_from_vertices` so callers keep
//! passing and receiving the editor's `Brush` type.

use bevy::math::Vec3;

use jackdaw_jsn::Brush;

pub use jackdaw_hull::{HullFace, merge_hull_triangles};

/// Rebuild a `Brush` from a new set of vertices using convex hull.
///
/// Thin wrapper over `jackdaw_hull::rebuild_brush_from_vertices` that unpacks
/// the old brush's faces and repacks the rebuilt faces plus topology into a
/// `Brush`. Returns the rebuilt brush and the old-to-new face index map.
pub(crate) fn rebuild_brush_from_vertices(
    old_brush: &Brush,
    old_vertices: &[Vec3],
    old_face_polygons: &[Vec<usize>],
    new_vertices: &[Vec3],
) -> Option<(Brush, Vec<usize>)> {
    let (faces, topology, old_to_new) = jackdaw_hull::rebuild_brush_from_vertices(
        &old_brush.faces,
        old_vertices,
        old_face_polygons,
        new_vertices,
    )?;
    Some((Brush { faces, topology }, old_to_new))
}
