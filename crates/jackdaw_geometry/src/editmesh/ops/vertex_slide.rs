//! Slide selected vertices along their first incident edge by parameter `t ∈ [0, 1]`.
//! Pure transform: no topology change.
//!
//! MVP: each vert slides toward the FIRST edge in its disk cycle. A modal
//! UX where the user picks the slide target by hovering an incident edge is
//! deferred to a future preview-aware operator.

use crate::editmesh::types::*;

#[derive(Debug)]
pub enum SlideError {
    EmptyInput,
}

pub struct SlideResult {
    pub moved_verts: Vec<VertKey>,
}

pub fn vertex_slide(bmesh: &mut EditMesh, verts: &[VertKey], t: f32) -> Result<SlideResult, SlideError> {
    if verts.is_empty() {
        return Err(SlideError::EmptyInput);
    }
    if t == 0.0 {
        return Ok(SlideResult { moved_verts: Vec::new() });
    }

    // Snapshot start positions and target positions FIRST (before mutating). If we mutate
    // a vert that's used as a target by another vert in the selection, we'd corrupt the
    // remaining slides.
    let mut moves: Vec<(VertKey, bevy::math::Vec3)> = Vec::with_capacity(verts.len());
    for &v in verts {
        let Some(vert) = bmesh.verts.get(v) else { continue };
        let Some(first_edge) = vert.edge else { continue };
        let edge = &bmesh.edges[first_edge];
        let other_vert_key = if edge.v[0] == v { edge.v[1] } else { edge.v[0] };
        let v_start = vert.co;
        let target = bmesh.verts[other_vert_key].co;
        let new_pos = v_start.lerp(target, t);
        moves.push((v, new_pos));
    }

    let mut moved: Vec<VertKey> = Vec::with_capacity(moves.len());
    for (vk, new_pos) in moves {
        bmesh.verts[vk].co = new_pos;
        moved.push(vk);
    }
    Ok(SlideResult { moved_verts: moved })
}
