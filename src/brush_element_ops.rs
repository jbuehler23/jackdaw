//! One-shot brush-element operators: delete the active sub-element
//! and nudge selected vertices/edges/faces along Y by one grid step.
//! Dispatch by current `BrushEditMode`.
//!
//! Replace the keybind branches in `interaction::handle_brush_delete`,
//! `brush_face_interact`, `brush_vertex_interact`, and
//! `brush_edge_interact`.

use std::collections::HashSet;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;
use jackdaw_geometry::halfedge::ops::delete_elements::{
    DeleteResult, delete_edges, delete_faces, delete_verts,
};
use jackdaw_geometry::halfedge::{EdgeKey, FaceKey, HalfedgeMesh, VertKey, apply_topology_edit};
use jackdaw_jsn::Brush;

use crate::brush::{
    BrushDragState, BrushEditMode, BrushHalfedge, BrushMeshCache, BrushSelection, EdgeDragState,
    EditMode, VertexDragState, rebuild_brush_from_vertices,
};
use crate::core_extension::CoreExtensionInputContext;
use crate::keybind_focus::KeybindFocus;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushDeleteElementOp>()
        .register_operator::<BrushNudgeUpOp>()
        .register_operator::<BrushNudgeDownOp>();

    ctx.bind_operator::<CoreExtensionInputContext, BrushDeleteElementOp>([
        PresetInput::key("Delete"),
        PresetInput::key("Backspace"),
    ]);
    ctx.bind_operator::<CoreExtensionInputContext, BrushNudgeUpOp>([PresetInput::key("PageUp")]);
    ctx.bind_operator::<CoreExtensionInputContext, BrushNudgeDownOp>([PresetInput::key(
        "PageDown",
    )]);
}

/// True when the operator is allowed to mutate brush elements: brush-edit
/// mode active, no text field focused, no drag in flight.
fn can_run_element_op(
    edit_mode: Res<EditMode>,
    keybind_focus: KeybindFocus,
    face_drag: Res<BrushDragState>,
    vertex_drag: Res<VertexDragState>,
    edge_drag: Res<EdgeDragState>,
) -> bool {
    matches!(*edit_mode, EditMode::BrushEdit(_))
        && !keybind_focus.is_typing()
        && !face_drag.active
        && !vertex_drag.active
        && !edge_drag.active
        && face_drag.pending.is_none()
        && vertex_drag.pending.is_none()
        && edge_drag.pending.is_none()
}

#[operator(
    id = "brush.delete_element",
    label = "Delete Element",
    description = "Delete the selected vertex / edge / face from the active brush, \
                   destroying the incident geometry and leaving a hole (no merge or \
                   heal, unlike dissolve). Dispatch follows the current \
                   `BrushEditMode`. Available whenever the brush-edit gate \
                   (`can_run_element_op`) passes; the per-mode guard keeps at least \
                   one polygon after a vertex / edge delete so the brush is never \
                   left a loose vertex cloud, and allows a face delete down to the \
                   last face.",
    is_available = can_run_element_op,
)]
pub(crate) fn brush_delete_element(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    mut brush_selection: ResMut<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut halfedge_q: Query<&mut BrushHalfedge>,
) -> OperatorResult {
    let EditMode::BrushEdit(mode) = *edit_mode else {
        return OperatorResult::Cancelled;
    };
    if matches!(mode, BrushEditMode::Clip | BrushEditMode::Knife) {
        return OperatorResult::Cancelled;
    }

    let edit_entities: Vec<Entity> = brush_selection.edit_brushes().collect();
    let mut any_modified = false;
    for entity in edit_entities {
        let Some(sub) = brush_selection.sub(entity) else {
            continue;
        };
        let sub_verts = sub.vertices.clone();
        let sub_edges = sub.edges.clone();
        let sub_faces = sub.faces.clone();
        let has_selection = match mode {
            BrushEditMode::Vertex => !sub_verts.is_empty(),
            BrushEditMode::Edge => !sub_edges.is_empty(),
            BrushEditMode::Face => !sub_faces.is_empty(),
            BrushEditMode::Clip | BrushEditMode::Knife => false,
        };
        if !has_selection {
            continue;
        }

        let Ok(mut halfedge) = halfedge_q.get_mut(entity) else {
            continue;
        };
        let Ok(mut brush) = brushes.get_mut(entity) else {
            continue;
        };

        // Guard: a vertex / edge delete must leave at least one polygon so the
        // brush is never reduced to a loose vertex cloud. A face delete may go
        // down to the last face. The gate runs the delete on a clone; the live
        // delete runs inside the reconcile closure below.
        let edit: Box<dyn FnOnce(&mut HalfedgeMesh) -> DeleteResult> = match mode {
            BrushEditMode::Vertex => {
                let keys: Vec<VertKey> = sub_verts
                    .iter()
                    .filter_map(|&v| halfedge.vert_keys.get(v).copied())
                    .collect();
                if keys.is_empty() {
                    continue;
                }
                let preview = preview_face_count(&halfedge.mesh, |mesh| delete_verts(mesh, &keys));
                if preview == 0 {
                    continue;
                }
                Box::new(move |mesh| delete_verts(mesh, &keys))
            }
            BrushEditMode::Edge => {
                let keys: Vec<EdgeKey> = sub_edges
                    .iter()
                    .filter_map(|&(a, b)| edge_key_between(&halfedge, a, b))
                    .collect();
                if keys.is_empty() {
                    continue;
                }
                let preview = preview_face_count(&halfedge.mesh, |mesh| delete_edges(mesh, &keys));
                if preview == 0 {
                    continue;
                }
                Box::new(move |mesh| delete_edges(mesh, &keys))
            }
            BrushEditMode::Face => {
                let keys: Vec<FaceKey> = sub_faces
                    .iter()
                    .filter_map(|&f| halfedge.face_keys.get(f).copied())
                    .collect();
                if keys.is_empty() {
                    continue;
                }
                Box::new(move |mesh| delete_faces(mesh, &keys))
            }
            BrushEditMode::Clip | BrushEditMode::Knife => continue,
        };

        apply_delete_to_brush(&mut brush, &mut halfedge, edit);

        if let Some(sub) = brush_selection.brushes.get_mut(&entity) {
            sub.vertices.clear();
            sub.edges.clear();
            sub.faces.clear();
        }
        any_modified = true;
    }

    if any_modified {
        OperatorResult::Finished
    } else {
        OperatorResult::Cancelled
    }
}

/// Run a delete on a clone of the mesh and report the surviving face count,
/// used to gate a destructive vertex / edge delete without mutating the live
/// mesh.
fn preview_face_count(
    mesh: &HalfedgeMesh,
    run: impl FnOnce(&mut HalfedgeMesh) -> DeleteResult,
) -> usize {
    let mut probe = mesh.clone();
    run(&mut probe);
    probe.face_count()
}

/// Map a selected cache-edge `(a, b)` to its live `EdgeKey` via the parallel
/// `vert_keys` table.
fn edge_key_between(halfedge: &BrushHalfedge, a: usize, b: usize) -> Option<EdgeKey> {
    let &va = halfedge.vert_keys.get(a)?;
    let &vb = halfedge.vert_keys.get(b)?;
    halfedge
        .mesh
        .edges
        .iter()
        .find(|(_, e)| (e.v[0] == va && e.v[1] == vb) || (e.v[0] == vb && e.v[1] == va))
        .map(|(k, _)| k)
}

/// Run a destructive delete and reconcile the brush. Applies `edit` through the
/// topology seam (recache normals, flatten, recompute planes, re-lift binding),
/// then rebuilds `brush.faces` parallel to the surviving polygons by re-indexing
/// the old per-face data through `surviving_faces` so each face keeps its
/// material + uv source rather than being default-filled.
fn apply_delete_to_brush(
    brush: &mut Brush,
    halfedge: &mut BrushHalfedge,
    edit: impl FnOnce(&mut HalfedgeMesh) -> DeleteResult,
) {
    let old_faces = brush.faces.clone();
    let result = apply_topology_edit(&mut brush.faces, &mut brush.topology, &mut halfedge.0, edit);

    let mut new_faces: Vec<jackdaw_jsn::BrushFaceData> =
        Vec::with_capacity(result.surviving_faces.len());
    for &old_idx in &result.surviving_faces {
        let old = old_faces
            .get(old_idx)
            .cloned()
            .unwrap_or_else(|| old_faces.last().cloned().unwrap_or_default());
        new_faces.push(old);
    }
    brush.faces = new_faces;
    brush.topology.recompute_face_planes(&mut brush.faces);
}

#[operator(
    id = "brush.nudge_up",
    label = "Nudge Up",
    description = "Nudge the selected sub-element +Y by one grid step. \
                   Dispatch follows `BrushEditMode`; availability \
                   (`can_run_element_op`) gates on the brush-edit gate.",
    is_available = can_run_element_op,
)]
pub(crate) fn brush_nudge_up(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    mut brush_selection: ResMut<BrushSelection>,
    brushes: Query<&mut Brush>,
    brush_caches: Query<&BrushMeshCache>,
    snap: Res<crate::snapping::SnapSettings>,
) -> OperatorResult {
    nudge_brush_element(
        1.0,
        edit_mode,
        &mut brush_selection,
        brushes,
        brush_caches,
        snap,
    )
}

#[operator(
    id = "brush.nudge_down",
    label = "Nudge Down",
    description = "Nudge the selected sub-element -Y by one grid step. \
                   Dispatch follows `BrushEditMode`; availability \
                   (`can_run_element_op`) gates on the brush-edit gate.",
    is_available = can_run_element_op,
)]
pub(crate) fn brush_nudge_down(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    mut brush_selection: ResMut<BrushSelection>,
    brushes: Query<&mut Brush>,
    brush_caches: Query<&BrushMeshCache>,
    snap: Res<crate::snapping::SnapSettings>,
) -> OperatorResult {
    nudge_brush_element(
        -1.0,
        edit_mode,
        &mut brush_selection,
        brushes,
        brush_caches,
        snap,
    )
}

fn nudge_brush_element(
    direction: f32,
    edit_mode: Res<EditMode>,
    brush_selection: &mut BrushSelection,
    mut brushes: Query<&mut Brush>,
    brush_caches: Query<&BrushMeshCache>,
    snap: Res<crate::snapping::SnapSettings>,
) -> OperatorResult {
    let EditMode::BrushEdit(mode) = *edit_mode else {
        return OperatorResult::Cancelled;
    };

    // Gather per-brush affected vertex sets from immutable reads before mutation.
    struct NudgePlan {
        entity: Entity,
        affected_verts: HashSet<usize>,
        // Face nudge needs the remapped face list after rebuild.
        nudge_faces: Vec<usize>,
    }

    let offset = Vec3::new(0.0, direction * snap.grid_size(), 0.0);
    let edit_entities: Vec<Entity> = brush_selection.edit_brushes().collect();
    let mut plans: Vec<NudgePlan> = Vec::new();
    for e in &edit_entities {
        let e = *e;
        let Some(sub) = brush_selection.sub(e) else {
            continue;
        };
        let Ok(cache) = brush_caches.get(e) else {
            continue;
        };
        let affected: HashSet<usize> = match mode {
            BrushEditMode::Vertex if !sub.vertices.is_empty() => {
                sub.vertices.iter().copied().collect()
            }
            BrushEditMode::Edge if !sub.edges.is_empty() => {
                sub.edges.iter().flat_map(|&(a, b)| [a, b]).collect()
            }
            BrushEditMode::Face if !sub.faces.is_empty() => sub
                .faces
                .iter()
                .filter_map(|&fi| cache.authored_face_polygons().get(fi))
                .flat_map(|poly| poly.iter().copied())
                .collect(),
            _ => continue,
        };
        if affected.is_empty() {
            continue;
        }
        plans.push(NudgePlan {
            entity: e,
            affected_verts: affected,
            nudge_faces: sub.faces.clone(),
        });
    }

    if plans.is_empty() {
        return OperatorResult::Cancelled;
    }

    let mut any_modified = false;
    for plan in plans {
        let Ok(cache) = brush_caches.get(plan.entity) else {
            continue;
        };
        let Ok(mut brush) = brushes.get_mut(plan.entity) else {
            continue;
        };
        let authored_verts = cache.authored_vertices();
        let authored_faces = cache.authored_face_polygons();
        let mut new_verts = authored_verts.to_vec();
        for &vi in &plan.affected_verts {
            if vi < new_verts.len() {
                new_verts[vi] += offset;
            }
        }
        let Some((new_brush, old_to_new)) =
            rebuild_brush_from_vertices(&brush, authored_verts, authored_faces, &new_verts)
        else {
            continue;
        };
        *brush = new_brush;
        if matches!(mode, BrushEditMode::Face) {
            // Face indices may be remapped during rebuild.
            if let Some(sub) = brush_selection.brushes.get_mut(&plan.entity) {
                sub.faces = plan
                    .nudge_faces
                    .iter()
                    .filter_map(|&fi| old_to_new.get(fi).copied())
                    .collect();
            }
        }
        any_modified = true;
    }

    if any_modified {
        OperatorResult::Finished
    } else {
        OperatorResult::Cancelled
    }
}
