//! Edit-mode switch operators: Object / Vertex / Edge / Face / Clip /
//! Physics. Each one either enters the named mode or, if already in
//! it, toggles back out to Object.
//!
//! These replace what `handle_edit_mode_keys` and the toolbar's
//! per-variant `.observe(...)` closures did before. All `allows_undo =
//! false` because edit-mode is a UI state, not a scene mutation.
//!
//! Default keybinds: `1` vertex, `2` edge, `3` face, `4` clip.

use bevy::{input_focus::InputFocus, prelude::*};
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;

use crate::brush::{
    BrushDragState, BrushEditMode, BrushSelection, ClipState, EdgeDragState, EditMode,
    VertexDragState,
};
use crate::core_extension::CoreExtensionInputContext;
use crate::draw_brush::DrawBrushState;
use crate::selection::Selection;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<EditModeObjectOp>()
        .register_operator::<EditModeVertexOp>()
        .register_operator::<EditModeEdgeOp>()
        .register_operator::<EditModeFaceOp>()
        .register_operator::<EditModeClipOp>()
        .register_operator::<BrushExitEditModeOp>();

    let ext = ctx.id();
    ctx.spawn((
        Action::<EditModeVertexOp>::new(),
        ActionOf::<CoreExtensionInputContext>::new(ext),
        bindings![(KeyCode::Digit1, Press::default())],
    ));
    ctx.spawn((
        Action::<EditModeEdgeOp>::new(),
        ActionOf::<CoreExtensionInputContext>::new(ext),
        bindings![(KeyCode::Digit2, Press::default())],
    ));
    ctx.spawn((
        Action::<EditModeFaceOp>::new(),
        ActionOf::<CoreExtensionInputContext>::new(ext),
        bindings![(KeyCode::Digit3, Press::default())],
    ));
    ctx.spawn((
        Action::<EditModeClipOp>::new(),
        ActionOf::<CoreExtensionInputContext>::new(ext),
        bindings![(KeyCode::Digit4, Press::default())],
    ));
    ctx.spawn((
        Action::<BrushExitEditModeOp>::new(),
        ActionOf::<CoreExtensionInputContext>::new(ext),
        bindings![(KeyCode::Escape, Press::default())],
    ));
}

/// True only when the user is in `BrushEdit` and an Escape press
/// should drop them back to Object mode (no modal running, no drag
/// in flight, not in Clip mode with pending points). Clip-with-points
/// is owned by `brush.clip.clear`'s Escape binding instead.
fn can_exit_brush_edit(
    edit_mode: Res<EditMode>,
    active: ActiveModalQuery,
    face_drag: Res<BrushDragState>,
    vertex_drag: Res<VertexDragState>,
    edge_drag: Res<EdgeDragState>,
    clip_state: Res<ClipState>,
) -> bool {
    if active.is_modal_running() {
        return false;
    }
    if face_drag.active || vertex_drag.active || edge_drag.active {
        return false;
    }
    if face_drag.pending.is_some() || vertex_drag.pending.is_some() || edge_drag.pending.is_some() {
        return false;
    }
    match *edit_mode {
        EditMode::BrushEdit(BrushEditMode::Clip) if !clip_state.points.is_empty() => false,
        EditMode::BrushEdit(_) => true,
        _ => false,
    }
}

/// Drop out of brush-edit mode back to Object.
#[operator(
    id = "brush.exit_edit_mode",
    label = "Exit Edit Mode",
    description = "Stop editing the brush and return to selecting whole entities.",
    is_available = can_exit_brush_edit,
    allows_undo = false,
)]
pub(crate) fn brush_exit_edit_mode(
    _: In<OperatorParameters>,
    mut edit_mode: ResMut<EditMode>,
    mut brush_selection: ResMut<BrushSelection>,
) -> OperatorResult {
    *edit_mode = EditMode::Object;
    brush_selection.clear();
    OperatorResult::Finished
}

/// True when switching edit modes is safe — no text field has focus,
/// no modal is running, and no brush sub-element drag is in flight or
/// pending. Keybind-triggered mode changes would otherwise yank the
/// drag target out from under the active system.
fn can_change_edit_mode(
    input_focus: Res<InputFocus>,
    active: ActiveModalQuery,
    face_drag: Res<BrushDragState>,
    vertex_drag: Res<VertexDragState>,
    edge_drag: Res<EdgeDragState>,
) -> bool {
    if input_focus.0.is_some() || active.is_modal_running() {
        return false;
    }
    if face_drag.active || vertex_drag.active || edge_drag.active {
        return false;
    }
    if face_drag.pending.is_some() || vertex_drag.pending.is_some() || edge_drag.pending.is_some() {
        return false;
    }
    true
}

#[operator(
    id = "edit_mode.object",
    label = "Object Mode",
    is_available = can_change_edit_mode
)]
pub(crate) fn edit_mode_object(
    _: In<OperatorParameters>,
    mut edit_mode: ResMut<EditMode>,
    mut brush_selection: ResMut<BrushSelection>,
    mut draw_state: ResMut<DrawBrushState>,
) -> OperatorResult {
    *edit_mode = EditMode::Object;
    brush_selection.clear();
    draw_state.active = None;
    OperatorResult::Finished
}

#[operator(
    id = "edit_mode.vertex",
    label = "Vertex Mode",
    is_available = can_change_edit_mode
)]
pub(crate) fn edit_mode_vertex(
    _: In<OperatorParameters>,
    edit_mode: ResMut<EditMode>,
    brush_selection: ResMut<BrushSelection>,
    draw_state: ResMut<DrawBrushState>,
    selection: Res<Selection>,
    brushes: Query<(), With<jackdaw_jsn::Brush>>,
) -> OperatorResult {
    switch_brush_edit_mode(
        BrushEditMode::Vertex,
        edit_mode,
        brush_selection,
        draw_state,
        selection,
        brushes,
    )
}

#[operator(
    id = "edit_mode.edge",
    label = "Edge Mode",
    is_available = can_change_edit_mode
)]
pub(crate) fn edit_mode_edge(
    _: In<OperatorParameters>,
    edit_mode: ResMut<EditMode>,
    brush_selection: ResMut<BrushSelection>,
    draw_state: ResMut<DrawBrushState>,
    selection: Res<Selection>,
    brushes: Query<(), With<jackdaw_jsn::Brush>>,
) -> OperatorResult {
    switch_brush_edit_mode(
        BrushEditMode::Edge,
        edit_mode,
        brush_selection,
        draw_state,
        selection,
        brushes,
    )
}

#[operator(
    id = "edit_mode.face",
    label = "Face Mode",
    is_available = can_change_edit_mode
)]
pub(crate) fn edit_mode_face(
    _: In<OperatorParameters>,
    edit_mode: ResMut<EditMode>,
    brush_selection: ResMut<BrushSelection>,
    draw_state: ResMut<DrawBrushState>,
    selection: Res<Selection>,
    brushes: Query<(), With<jackdaw_jsn::Brush>>,
) -> OperatorResult {
    switch_brush_edit_mode(
        BrushEditMode::Face,
        edit_mode,
        brush_selection,
        draw_state,
        selection,
        brushes,
    )
}

#[operator(
    id = "edit_mode.clip",
    label = "Clip Mode",
    is_available = can_change_edit_mode
)]
pub(crate) fn edit_mode_clip(
    _: In<OperatorParameters>,
    edit_mode: ResMut<EditMode>,
    brush_selection: ResMut<BrushSelection>,
    draw_state: ResMut<DrawBrushState>,
    selection: Res<Selection>,
    brushes: Query<(), With<jackdaw_jsn::Brush>>,
) -> OperatorResult {
    switch_brush_edit_mode(
        BrushEditMode::Clip,
        edit_mode,
        brush_selection,
        draw_state,
        selection,
        brushes,
    )
}

fn switch_brush_edit_mode(
    target: BrushEditMode,
    mut edit_mode: ResMut<EditMode>,
    mut brush_selection: ResMut<BrushSelection>,
    mut draw_state: ResMut<DrawBrushState>,
    selection: Res<Selection>,
    brushes: Query<(), With<jackdaw_jsn::Brush>>,
) -> OperatorResult {
    draw_state.active = None;

    match *edit_mode {
        EditMode::BrushEdit(current) if current == target => {
            // Same mode pressed twice: toggle back to Object.
            *edit_mode = EditMode::Object;
            brush_selection.clear();
        }
        EditMode::BrushEdit(_) => {
            // Switching between brush sub-modes: swap the mode but
            // keep the entity selected. Drop any sub-element picks
            // (indices are per-mode and don't translate across).
            *edit_mode = EditMode::BrushEdit(target);
            brush_selection.faces.clear();
            brush_selection.vertices.clear();
            brush_selection.edges.clear();
        }
        _ => {
            // Entering from Object / Physics requires a selected
            // brush; otherwise the op is a no-op.
            let Some(entity) = selection.primary().filter(|&e| brushes.contains(e)) else {
                return OperatorResult::Cancelled;
            };
            *edit_mode = EditMode::BrushEdit(target);
            brush_selection.entity = Some(entity);
            brush_selection.faces.clear();
            brush_selection.vertices.clear();
            brush_selection.edges.clear();
        }
    }
    OperatorResult::Finished
}
