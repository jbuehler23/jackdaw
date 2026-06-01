//! One-shot brush-element operators: delete the active sub-element
//! and nudge selected vertices/edges/faces along Y by one grid step.
//! Dispatch by current `BrushEditMode`.
//!
//! Replace the keybind branches in `interaction::handle_brush_delete`,
//! `brush_face_interact`, `brush_vertex_interact`, and
//! `brush_edge_interact`.

use std::collections::HashSet;

use bevy::prelude::*;
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;
use jackdaw_jsn::Brush;

use crate::brush::{
    BrushDragState, BrushEditMode, BrushMeshCache, BrushSelection, EdgeDragState, EditMode,
    VertexDragState, rebuild_brush_from_vertices,
};
use crate::core_extension::CoreExtensionInputContext;
use crate::keybind_focus::KeybindFocus;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushDeleteElementOp>()
        .register_operator::<BrushNudgeUpOp>()
        .register_operator::<BrushNudgeDownOp>();

    let ext = ctx.id();
    ctx.entity_mut().world_scope(|world| {
        world.spawn((
            Action::<BrushDeleteElementOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![
                (KeyCode::Delete, Press::default()),
                (KeyCode::Backspace, Press::default()),
            ],
        ));
        world.spawn((
            Action::<BrushNudgeUpOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(KeyCode::PageUp, Press::default())],
        ));
        world.spawn((
            Action::<BrushNudgeDownOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(KeyCode::PageDown, Press::default())],
        ));
    });
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
    description = "Delete the selected vertex / edge / face from the active brush. \
                   Dispatch follows the current `BrushEditMode`. The brush must \
                   retain at least four vertices (a tetrahedron); availability \
                   (`can_run_element_op`) is false otherwise.",
    is_available = can_run_element_op,
)]
pub(crate) fn brush_delete_element(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    mut brush_selection: ResMut<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    brush_caches: Query<&BrushMeshCache>,
) -> OperatorResult {
    let EditMode::BrushEdit(mode) = *edit_mode else {
        return OperatorResult::Cancelled;
    };
    if matches!(mode, BrushEditMode::Clip | BrushEditMode::Knife) {
        return OperatorResult::Cancelled;
    }

    // Gather per-brush work plans from immutable reads before any mutation.
    struct DeletePlan {
        entity: Entity,
        removed_verts: HashSet<usize>,
        removed_faces: HashSet<usize>,
    }

    let edit_entities: Vec<Entity> = brush_selection.edit_brushes().collect();
    let mut plans: Vec<DeletePlan> = Vec::new();
    for e in &edit_entities {
        let e = *e;
        let Some(sub) = brush_selection.sub(e) else {
            continue;
        };
        let Ok(cache) = brush_caches.get(e) else {
            continue;
        };
        let Ok(brush) = brushes.get(e) else {
            continue;
        };
        match mode {
            BrushEditMode::Vertex if !sub.vertices.is_empty() => {
                let removed: HashSet<usize> = sub.vertices.iter().copied().collect();
                let remaining = cache.vertices.len().saturating_sub(removed.len());
                if remaining >= 4 {
                    plans.push(DeletePlan {
                        entity: e,
                        removed_verts: removed,
                        removed_faces: HashSet::new(),
                    });
                }
            }
            BrushEditMode::Edge if !sub.edges.is_empty() => {
                let removed: HashSet<usize> =
                    sub.edges.iter().flat_map(|&(a, b)| [a, b]).collect();
                let remaining = cache.vertices.len().saturating_sub(removed.len());
                if remaining >= 4 {
                    plans.push(DeletePlan {
                        entity: e,
                        removed_verts: removed,
                        removed_faces: HashSet::new(),
                    });
                }
            }
            BrushEditMode::Face if !sub.faces.is_empty() => {
                let removed: HashSet<usize> = sub.faces.iter().copied().collect();
                let remaining = brush.faces.len().saturating_sub(removed.len());
                if remaining >= 4 {
                    plans.push(DeletePlan {
                        entity: e,
                        removed_verts: HashSet::new(),
                        removed_faces: removed,
                    });
                }
            }
            _ => {}
        }
    }

    if plans.is_empty() {
        return OperatorResult::Cancelled;
    }

    let mut any_modified = false;
    for plan in plans {
        match mode {
            BrushEditMode::Vertex | BrushEditMode::Edge => {
                let Ok(cache) = brush_caches.get(plan.entity) else {
                    continue;
                };
                let Ok(mut brush) = brushes.get_mut(plan.entity) else {
                    continue;
                };
                if rebuild_after_remove(&mut brush, cache, &plan.removed_verts) {
                    if let Some(sub) = brush_selection.brushes.get_mut(&plan.entity) {
                        if matches!(mode, BrushEditMode::Vertex) {
                            sub.vertices.clear();
                        } else {
                            sub.edges.clear();
                        }
                    }
                    any_modified = true;
                }
            }
            BrushEditMode::Face => {
                let Ok(mut brush) = brushes.get_mut(plan.entity) else {
                    continue;
                };
                brush.faces = brush
                    .faces
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| !plan.removed_faces.contains(i))
                    .map(|(_, f)| f.clone())
                    .collect();
                if let Some(sub) = brush_selection.brushes.get_mut(&plan.entity) {
                    sub.faces.clear();
                }
                any_modified = true;
            }
            BrushEditMode::Clip | BrushEditMode::Knife => unreachable!(),
        }
    }

    if any_modified {
        OperatorResult::Finished
    } else {
        OperatorResult::Cancelled
    }
}

fn rebuild_after_remove(
    brush: &mut Brush,
    cache: &BrushMeshCache,
    removed: &HashSet<usize>,
) -> bool {
    let remaining: Vec<Vec3> = cache
        .vertices
        .iter()
        .enumerate()
        .filter(|(i, _)| !removed.contains(i))
        .map(|(_, v)| *v)
        .collect();
    if remaining.len() < 4 {
        return false;
    }
    let Some((new_brush, _)) =
        rebuild_brush_from_vertices(brush, &cache.vertices, &cache.face_polygons, &remaining)
    else {
        return false;
    };
    *brush = new_brush;
    true
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
                .filter_map(|&fi| cache.face_polygons.get(fi))
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
        let mut new_verts = cache.vertices.clone();
        for &vi in &plan.affected_verts {
            if vi < new_verts.len() {
                new_verts[vi] += offset;
            }
        }
        let Some((new_brush, old_to_new)) = rebuild_brush_from_vertices(
            &brush,
            &cache.vertices,
            &cache.face_polygons,
            &new_verts,
        ) else {
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
