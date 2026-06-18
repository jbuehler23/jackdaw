//! `brush.mesh.make_edge_face` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::ops::contextual_create::{ContextualResult, contextual_create};
use jackdaw_geometry::halfedge::{VertKey, apply_topology_edit};
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode};

/// Captured selection target for the F-key `contextual_create` result. Held
/// across the flatten/re-lift roundtrip so the post-commit selection update
/// can map back to a topology index (or vertex index pair).
enum ChainTarget {
    /// Topology vertex index pair (a < b) for the newly created edge.
    Edge((usize, usize)),
    /// `material_idx` of the newly created face; resolved to a topology face
    /// index via `count(faces with material_idx < this)` (mirrors inset logic).
    Face(u32),
}

/// Fill the current vertex selection with a new edge or face. Two verts -> edge.
/// Three or more -> face whose ring is the selected verts in selection order.
/// Available in Vertex mode.
#[operator(
    id = "brush.mesh.make_edge_face",
    label = "Make Edge / Face",
    is_available = can_run_make_edge_face,
    allows_undo = true
)]
pub(crate) fn brush_make_edge_face(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    mut selection: ResMut<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut halfedge_q: Query<&mut BrushHalfedge>,
) -> OperatorResult {
    if *edit_mode != EditMode::BrushEdit(BrushEditMode::Vertex) {
        return OperatorResult::Cancelled;
    }
    let brush_entity = selection.active_brush?;
    let sel_verts: Vec<usize> = selection
        .sub(brush_entity)
        .map(|s| s.vertices.clone())
        .unwrap_or_default();
    if sel_verts.len() < 2 {
        return OperatorResult::Cancelled;
    }

    // Map cache vertex indices to HalfedgeMesh VertKeys via vert_keys parallel array.
    let mut halfedge = halfedge_q.get_mut(brush_entity)?;
    let mut vert_keys: Vec<VertKey> = Vec::with_capacity(sel_verts.len());
    for &vert_idx in &sel_verts {
        if let Some(&vk) = halfedge.vert_keys.get(vert_idx) {
            vert_keys.push(vk);
        }
    }
    if vert_keys.len() < 2 {
        return OperatorResult::Cancelled;
    }

    // Create the edge or face and reconcile, capturing the new element so the
    // post-commit selection can target it. The target is read from the mesh
    // before reconcile; topology vertex order matches the mesh slotmap order
    // (see `flatten_to_topology`) and `contextual_create` never removes verts.
    let brush = brushes.get_mut(brush_entity)?.into_inner();
    let source = brush.faces.last().cloned().unwrap_or_default();
    let original_face_count = brush.faces.len();
    let chain_target: Option<ChainTarget> = apply_topology_edit(
        &mut brush.faces,
        &mut brush.topology,
        &mut halfedge.0,
        |mesh| match contextual_create(mesh, &vert_keys) {
            Ok(ContextualResult::Edge(ek)) => {
                let edge = mesh.edges.get(ek)?;
                let mut a_idx: Option<usize> = None;
                let mut b_idx: Option<usize> = None;
                for (i, (k, _)) in mesh.verts.iter().enumerate() {
                    if k == edge.v[0] {
                        a_idx = Some(i);
                    }
                    if k == edge.v[1] {
                        b_idx = Some(i);
                    }
                }
                let (a, b) = (a_idx?, b_idx?);
                let pair = if a < b { (a, b) } else { (b, a) };
                Some(ChainTarget::Edge(pair))
            }
            Ok(ContextualResult::Face(fk)) => mesh
                .faces
                .get(fk)
                .map(|f| ChainTarget::Face(f.material_idx)),
            Err(_) => None,
        },
    );

    // Any face the fill created inherits the previous last face's appearance.
    for new_face in original_face_count..brush.faces.len() {
        brush.faces[new_face].copy_appearance_from(&source);
        brush.faces[new_face].ensure_uv_axes();
    }
    // Chain selection: write the new edge or face into `BrushSelection` so the
    // user can immediately act on it (e.g. toggle to Edge / Face mode and drag).
    match chain_target {
        Some(ChainTarget::Edge((a, b))) => {
            let vert_count = brush.topology.vertices.len();
            if a < vert_count && b < vert_count {
                selection.sub_mut(brush_entity).edges = vec![(a, b)];
            }
        }
        Some(ChainTarget::Face(mtx)) => {
            let face_idx = halfedge
                .mesh
                .faces
                .values()
                .filter(|f| f.material_idx < mtx)
                .count();
            if face_idx < brush.faces.len() {
                selection.sub_mut(brush_entity).faces = vec![face_idx];
            }
        }
        None => {}
    }

    OperatorResult::Finished
}

pub(crate) fn can_run_make_edge_face(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Vertex)
        && selection
            .active_sub()
            .is_some_and(|s| s.vertices.len() >= 2)
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushMakeEdgeFaceOp>();
    // No keybind; operator is available via menu / command palette only for MVP.
}
