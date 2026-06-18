//! `brush.mesh.vertex_bevel` operator: modal vertex bevel.
//!
//! Press `Ctrl+Shift+B` in Vertex mode with exactly one vertex selected.
//! Cursor displacement magnitude drives a positive bevel width; Ctrl snaps
//! the world width to the translate grid increment. The brush mesh is mutated
//! each frame so the user sees the live N-gon bevel as they drag. LMB
//! commits; Esc / RMB cancels and restores the pre-modal mesh.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;
use jackdaw_geometry::halfedge::cycles::disk_walk;
use jackdaw_geometry::halfedge::ops::vertex_bevel::vertex_bevel;
use jackdaw_geometry::halfedge::{HalfedgeMesh, VertKey};
use jackdaw_jsn::Brush;

use super::modal_edit::ModalTopologyEdit;
use crate::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode};
use crate::core_extension::CoreExtensionInputContext;
use crate::snapping::SnapSettings;

/// Pixels-per-world-unit sensitivity for the vertex bevel modal. At this
/// value 100 pixels of cursor motion correspond to 1 world-unit of bevel
/// width. Matches the edge bevel modal.
const VERTEX_BEVEL_SENSITIVITY: f32 = 0.01;

/// Modal state for the `brush.mesh.vertex_bevel` operator.
#[derive(Resource, Default)]
pub struct VertexBevelModalState {
    pub active: bool,
    pub brush_entity: Option<Entity>,
    /// `HalfedgeMesh` `VertKey` of the vertex being beveled. The slotmap key
    /// stays valid across the snapshot's clones, so it re-resolves each frame
    /// after the snapshot is restored.
    pub vert_key: Option<VertKey>,
    /// Window-space cursor position at the moment the modal started.
    pub start_cursor: Vec2,
    /// Current bevel width in world-space units.
    pub current_width: f32,
    /// Pre-edit snapshot driving restore and per-frame re-apply.
    pub edit: Option<ModalTopologyEdit>,
    /// Maximum valid bevel width: 0.99 * half the length of the shortest
    /// incident edge at the beveled vertex. Past this point the offset would
    /// overshoot the neighbor and the rebuilt face collapses.
    pub max_width: f32,
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushVertexBevelOp>();

    ctx.bind_operator::<CoreExtensionInputContext, BrushVertexBevelOp>([
        PresetInput::key("KeyB").ctrl()
    ]);
}

/// Bevel the selected vertex into an N-gon face, controlled by cursor
/// displacement magnitude. Ctrl snaps to the translate grid increment. The
/// live brush mesh is updated each frame so the bevel is visible as a real
/// mesh edit. LMB commits; Esc / RMB cancels and reverts.
///
/// Requires Vertex mode with exactly one vertex selected.
#[operator(
    id = "brush.mesh.vertex_bevel",
    label = "Vertex Bevel",
    is_available = can_run_vertex_bevel,
    modal = true,
    allows_undo = false,
    cancel = cancel_vertex_bevel,
)]
pub(crate) fn brush_vertex_bevel(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    mut selection: ResMut<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut halfedge_q: Query<&mut BrushHalfedge>,
    mut modal_state: ResMut<VertexBevelModalState>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    modal_inputs: crate::modal_inputs::ModalInputs,
    cursor: crate::viewport::UiCursorPos,
    snap_settings: Res<SnapSettings>,
    modal_entity: Option<Single<Entity, With<ActiveModalOperator>>>,
) -> OperatorResult {
    let cursor_pos = cursor.get()?;

    // --- First invoke: snapshot and enter modal ---
    if modal_entity.is_none() {
        if *edit_mode != EditMode::BrushEdit(BrushEditMode::Vertex) {
            return OperatorResult::Cancelled;
        }
        let brush_entity = selection.active_brush?;
        let sel_verts: Vec<usize> = selection
            .sub(brush_entity)
            .map(|s| s.vertices.clone())
            .unwrap_or_default();
        if sel_verts.len() != 1 {
            return OperatorResult::Cancelled;
        }

        let brush_before = brushes.get(brush_entity).cloned()?;
        let halfedge = halfedge_q.get(brush_entity)?;

        let &vert_idx = sel_verts.first()?;
        let &vert_key = halfedge.vert_keys.get(vert_idx)?;

        let max_width = compute_max_bevel_width(&halfedge.mesh, vert_key);

        modal_state.active = true;
        modal_state.brush_entity = Some(brush_entity);
        modal_state.vert_key = Some(vert_key);
        modal_state.start_cursor = cursor_pos;
        modal_state.current_width = 0.0;
        modal_state.edit = Some(ModalTopologyEdit::begin(&brush_before, halfedge));
        modal_state.max_width = max_width;

        return OperatorResult::Running;
    }

    // --- Subsequent invokes: cancel, update width, mutate preview, or commit ---

    let rmb = mouse.just_pressed(MouseButton::Right);
    if modal_inputs.cancel() || rmb {
        // Live brush has been mutated each frame, so restore from the snapshot
        // before clearing modal state.
        restore_brush_from_snapshot(&modal_state, &mut brushes, &mut halfedge_q);
        *modal_state = VertexBevelModalState::default();
        return OperatorResult::Cancelled;
    }

    // Cursor distance from the initial position drives the width.
    let delta = cursor_pos - modal_state.start_cursor;
    let raw_width = delta.length() * VERTEX_BEVEL_SENSITIVITY;
    let clamped_width = raw_width.min(modal_state.max_width);

    // Snap respects the global translate_snap toggle; Ctrl flips the current
    // snap state (anti-modifier).
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    modal_state.current_width =
        if snap_settings.translate_active(ctrl) && snap_settings.translate_increment > 0.0 {
            let inc = snap_settings.translate_increment;
            (clamped_width / inc).round() * inc
        } else {
            clamped_width
        };

    // Apply the bevel to the live brush mesh so the user sees it as a real
    // mesh edit. The op result is discarded; the bevel face is visible
    // through the regular brush mesh pipeline picking up `Changed<Brush>`.
    apply_live_bevel(&mut modal_state, &mut brushes, &mut halfedge_q);

    // Commit on LMB.
    if mouse.just_pressed(MouseButton::Left) {
        let Some(brush_entity) = modal_state.brush_entity else {
            *modal_state = VertexBevelModalState::default();
            return OperatorResult::Cancelled;
        };

        // Zero-width commit: treat as cancel so we don't write a no-op undo.
        // The live brush should already be back to the snapshot in this case
        // (apply_live_bevel resets to the snapshot when width is sub-threshold).
        if modal_state.current_width < 1e-5 {
            restore_brush_from_snapshot(&modal_state, &mut brushes, &mut halfedge_q);
            *modal_state = VertexBevelModalState::default();
            return OperatorResult::Cancelled;
        }

        let Ok(brush) = brushes.get(brush_entity).cloned() else {
            *modal_state = VertexBevelModalState::default();
            return OperatorResult::Cancelled;
        };

        // Chain selection: the new bevel face is the last face in the
        // topology since vertex_bevel appends one face. Select it for any
        // follow-up gestures (the user stays in Vertex mode, but if they
        // switch to Face mode the bevel face will be the active selection).
        let new_face_idx = brush.faces.len().saturating_sub(1);
        selection.sub_mut(brush_entity).faces = vec![new_face_idx];

        *modal_state = VertexBevelModalState::default();
        return OperatorResult::Finished;
    }

    OperatorResult::Running
}

/// Cancel handler: restore the brush to its pre-modal state. Called when the
/// modal lifecycle is force-cancelled from outside the operator.
fn cancel_vertex_bevel(
    mut modal_state: ResMut<VertexBevelModalState>,
    mut brushes: Query<&mut Brush>,
    mut halfedge_q: Query<&mut BrushHalfedge>,
) {
    restore_brush_from_snapshot(&modal_state, &mut brushes, &mut halfedge_q);
    *modal_state = VertexBevelModalState::default();
}

/// Reset the live brush + `HalfedgeMesh` to the snapshot captured at modal start.
fn restore_brush_from_snapshot(
    modal_state: &VertexBevelModalState,
    brushes: &mut Query<&mut Brush>,
    halfedge_q: &mut Query<&mut BrushHalfedge>,
) {
    let Some(brush_entity) = modal_state.brush_entity else {
        return;
    };
    let Some(edit) = modal_state.edit.as_ref() else {
        return;
    };
    let Ok(mut brush) = brushes.get_mut(brush_entity) else {
        return;
    };
    let Ok(mut halfedge) = halfedge_q.get_mut(brush_entity) else {
        return;
    };
    edit.restore(&mut brush, &mut halfedge);
}

/// Re-run `vertex_bevel` against the snapshot at the current width and write
/// the resulting topology back into the live `Brush` + `BrushHalfedge`.
fn apply_live_bevel(
    modal_state: &mut VertexBevelModalState,
    brushes: &mut Query<&mut Brush>,
    halfedge_q: &mut Query<&mut BrushHalfedge>,
) {
    let Some(brush_entity) = modal_state.brush_entity else {
        return;
    };
    let Some(edit) = modal_state.edit.as_ref() else {
        return;
    };
    let Some(vert_key) = modal_state.vert_key else {
        return;
    };
    let Ok(brush_mut) = brushes.get_mut(brush_entity) else {
        return;
    };
    let Ok(mut halfedge) = halfedge_q.get_mut(brush_entity) else {
        return;
    };
    let brush = brush_mut.into_inner();

    // Sub-threshold widths: snap the live brush back to the snapshot.
    if modal_state.current_width < 1e-5 {
        edit.restore(brush, &mut halfedge);
        return;
    }

    // Seed the new bevel face from the snapshot's last face (for material +
    // uv_scale / rotation), then re-derive its UV axes from its own plane.
    let source = edit
        .snapshot_brush()
        .faces
        .last()
        .cloned()
        .unwrap_or_default();
    let original_face_count = edit.snapshot_brush().faces.len();

    // Restore to the snapshot, re-run the bevel at the current width, and
    // reconcile.
    let width = modal_state.current_width;
    edit.apply(brush, &mut halfedge, |mesh| {
        let _ = vertex_bevel(mesh, vert_key, width);
    });

    // The new bevel face inherits the source appearance with freshly derived
    // UV axes.
    for new_face in original_face_count..brush.faces.len() {
        brush.faces[new_face].copy_appearance_from(&source);
        brush.faces[new_face].uv_u_axis = Vec3::ZERO;
        brush.faces[new_face].uv_v_axis = Vec3::ZERO;
        brush.faces[new_face].ensure_uv_axes();
    }
}

/// Geometric cap on bevel width: half the length of the shortest incident
/// edge at the beveled vertex, times a 0.99 safety factor. Past this point
/// the offset overshoots its neighbor and the rebuilt face collapses.
fn compute_max_bevel_width(mesh: &HalfedgeMesh, vert_key: VertKey) -> f32 {
    if !mesh.verts.contains_key(vert_key) {
        return 0.0;
    }
    let v_pos = mesh.verts[vert_key].co;
    let mut min_half_len = f32::MAX;
    for edge_key in disk_walk(mesh, vert_key) {
        let edge = &mesh.edges[edge_key];
        let other = if edge.v[0] == vert_key {
            edge.v[1]
        } else {
            edge.v[0]
        };
        let other_pos = mesh.verts[other].co;
        let len = (other_pos - v_pos).length();
        let half = len * 0.5;
        if half > 1e-6 && half < min_half_len {
            min_half_len = half;
        }
    }
    if min_half_len.is_finite() {
        min_half_len * 0.99
    } else {
        f32::MAX
    }
}

pub(crate) fn can_run_vertex_bevel(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Vertex)
        && selection
            .active_sub()
            .is_some_and(|s| s.vertices.len() == 1)
}
