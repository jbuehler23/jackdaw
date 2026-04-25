//! One-shot brush-element operators: delete the active sub-element
//! and nudge selected vertices/edges/faces along Y by one grid step.
//! Dispatch by current `BrushEditMode`.
//!
//! Replace the keybind branches in `interaction::handle_brush_delete`,
//! `brush_face_interact`, `brush_vertex_interact`, and
//! `brush_edge_interact`.

use std::collections::HashSet;

use bevy::{input_focus::InputFocus, prelude::*};
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;
use jackdaw_jsn::Brush;

use crate::brush::{
    BrushDragState, BrushEditMode, BrushMeshCache, BrushSelection, EdgeDragState, EditMode,
    SetBrush, VertexDragState, rebuild_brush_from_vertices,
};
use crate::commands::CommandHistory;
use crate::core_extension::CoreExtensionInputContext;

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
    input_focus: Res<InputFocus>,
    face_drag: Res<BrushDragState>,
    vertex_drag: Res<VertexDragState>,
    edge_drag: Res<EdgeDragState>,
) -> bool {
    matches!(*edit_mode, EditMode::BrushEdit(_))
        && input_focus.0.is_none()
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
    mut history: ResMut<CommandHistory>,
) -> OperatorResult {
    let EditMode::BrushEdit(mode) = *edit_mode else {
        return OperatorResult::Cancelled;
    };
    let Some(brush_entity) = brush_selection.entity else {
        return OperatorResult::Cancelled;
    };
    let Ok(mut brush) = brushes.get_mut(brush_entity) else {
        return OperatorResult::Cancelled;
    };

    match mode {
        BrushEditMode::Vertex => {
            if brush_selection.vertices.is_empty() {
                return OperatorResult::Cancelled;
            }
            let Ok(cache) = brush_caches.get(brush_entity) else {
                return OperatorResult::Cancelled;
            };
            let removed: HashSet<usize> = brush_selection.vertices.iter().copied().collect();
            if !rebuild_after_remove(
                &mut brush,
                cache,
                &removed,
                "Remove brush vertex",
                brush_entity,
                &mut history,
            ) {
                return OperatorResult::Cancelled;
            }
            brush_selection.vertices.clear();
        }
        BrushEditMode::Edge => {
            if brush_selection.edges.is_empty() {
                return OperatorResult::Cancelled;
            }
            let Ok(cache) = brush_caches.get(brush_entity) else {
                return OperatorResult::Cancelled;
            };
            let removed: HashSet<usize> = brush_selection
                .edges
                .iter()
                .flat_map(|&(a, b)| [a, b])
                .collect();
            if !rebuild_after_remove(
                &mut brush,
                cache,
                &removed,
                "Remove brush edge",
                brush_entity,
                &mut history,
            ) {
                return OperatorResult::Cancelled;
            }
            brush_selection.edges.clear();
        }
        BrushEditMode::Face => {
            if brush_selection.faces.is_empty() {
                return OperatorResult::Cancelled;
            }
            if brush.faces.len() - brush_selection.faces.len() < 4 {
                return OperatorResult::Cancelled;
            }
            let removed: HashSet<usize> = brush_selection.faces.iter().copied().collect();
            let old = brush.clone();
            brush.faces = brush
                .faces
                .iter()
                .enumerate()
                .filter(|(i, _)| !removed.contains(i))
                .map(|(_, f)| f.clone())
                .collect();
            history.push_executed(Box::new(SetBrush {
                entity: brush_entity,
                old,
                new: brush.clone(),
                label: "Remove brush face".to_string(),
            }));
            brush_selection.faces.clear();
        }
        BrushEditMode::Clip => return OperatorResult::Cancelled,
    }
    OperatorResult::Finished
}

fn rebuild_after_remove(
    brush: &mut Brush,
    cache: &BrushMeshCache,
    removed: &HashSet<usize>,
    label: &str,
    entity: Entity,
    history: &mut CommandHistory,
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
    let old = brush.clone();
    let Some((new_brush, _)) =
        rebuild_brush_from_vertices(&old, &cache.vertices, &cache.face_polygons, &remaining)
    else {
        return false;
    };
    *brush = new_brush;
    history.push_executed(Box::new(SetBrush {
        entity,
        old,
        new: brush.clone(),
        label: label.to_string(),
    }));
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
    history: ResMut<CommandHistory>,
    snap: Res<crate::snapping::SnapSettings>,
) -> OperatorResult {
    nudge_brush_element(
        1.0,
        edit_mode,
        &mut brush_selection,
        brushes,
        brush_caches,
        history,
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
    history: ResMut<CommandHistory>,
    snap: Res<crate::snapping::SnapSettings>,
) -> OperatorResult {
    nudge_brush_element(
        -1.0,
        edit_mode,
        &mut brush_selection,
        brushes,
        brush_caches,
        history,
        snap,
    )
}

fn nudge_brush_element(
    direction: f32,
    edit_mode: Res<EditMode>,
    brush_selection: &mut BrushSelection,
    mut brushes: Query<&mut Brush>,
    brush_caches: Query<&BrushMeshCache>,
    mut history: ResMut<CommandHistory>,
    snap: Res<crate::snapping::SnapSettings>,
) -> OperatorResult {
    let EditMode::BrushEdit(mode) = *edit_mode else {
        return OperatorResult::Cancelled;
    };
    let Some(brush_entity) = brush_selection.entity else {
        return OperatorResult::Cancelled;
    };
    let Ok(cache) = brush_caches.get(brush_entity) else {
        return OperatorResult::Cancelled;
    };
    let Ok(mut brush) = brushes.get_mut(brush_entity) else {
        return OperatorResult::Cancelled;
    };

    let affected: HashSet<usize> = match mode {
        BrushEditMode::Vertex if !brush_selection.vertices.is_empty() => {
            brush_selection.vertices.iter().copied().collect()
        }
        BrushEditMode::Edge if !brush_selection.edges.is_empty() => brush_selection
            .edges
            .iter()
            .flat_map(|&(a, b)| [a, b])
            .collect(),
        BrushEditMode::Face if !brush_selection.faces.is_empty() => brush_selection
            .faces
            .iter()
            .filter_map(|&fi| cache.face_polygons.get(fi))
            .flat_map(|poly| poly.iter().copied())
            .collect(),
        _ => return OperatorResult::Cancelled,
    };

    let offset = Vec3::new(0.0, direction * snap.grid_size(), 0.0);
    let mut new_verts = cache.vertices.clone();
    for &vi in &affected {
        if vi < new_verts.len() {
            new_verts[vi] += offset;
        }
    }
    let old = brush.clone();
    let Some((new_brush, old_to_new)) =
        rebuild_brush_from_vertices(&old, &cache.vertices, &cache.face_polygons, &new_verts)
    else {
        return OperatorResult::Cancelled;
    };
    *brush = new_brush;

    let label = match mode {
        BrushEditMode::Vertex => "Nudge brush vertex",
        BrushEditMode::Edge => "Nudge brush edge",
        BrushEditMode::Face => {
            // Face indices may have been remapped during rebuild.
            brush_selection.faces = brush_selection
                .faces
                .iter()
                .filter_map(|&fi| old_to_new.get(fi).copied())
                .collect();
            "Nudge brush face"
        }
        BrushEditMode::Clip => unreachable!(),
    };
    history.push_executed(Box::new(SetBrush {
        entity: brush_entity,
        old,
        new: brush.clone(),
        label: label.to_string(),
    }));
    OperatorResult::Finished
}
