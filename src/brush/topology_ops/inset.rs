//! `brush.mesh.inset` operator: modal inset.
//!
//! Press `I` in Face mode. The inset amount is controlled by mouse displacement
//! magnitude: drag any direction to grow the inset proportionally. The brush
//! mesh is mutated each frame so the user sees the live inset as a real mesh
//! edit. Ctrl snaps to the translate grid increment. LMB commits; Esc / RMB
//! cancels and restores the pre-modal mesh.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;
use jackdaw_geometry::halfedge::ops::inset_face::inset_face;
use jackdaw_geometry::halfedge::{FaceKey, HalfedgeMesh};
use jackdaw_jsn::Brush;

use super::modal_edit::ModalTopologyEdit;
use crate::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode};
use crate::core_extension::CoreExtensionInputContext;
use crate::snapping::SnapSettings;

/// Pixels-per-world-unit sensitivity for the inset modal.
/// At this value 100 pixels of cursor movement corresponds to 1 world-unit of inset.
/// Tune as needed.
const INSET_SENSITIVITY: f32 = 0.01;

/// Modal state for the inset operator.
#[derive(Resource, Default)]
pub struct InsetModalState {
    pub active: bool,
    pub brush_entity: Option<Entity>,
    /// `HalfedgeMesh` `FaceKeys` of the faces being inset. Slotmap keys stay
    /// valid across the snapshot's clones, so they re-resolve each frame after
    /// the snapshot is restored.
    pub face_keys: Vec<FaceKey>,
    /// Brush face indices of the faces being inset, captured at modal entry.
    /// The live-preview path seeds new spoke faces from the first of these so
    /// they inherit the parent face's material and UV projection rather than an
    /// unrelated face's.
    pub face_indices: Vec<usize>,
    /// Window-space cursor position at the moment the modal started.
    pub start_cursor: Vec2,
    /// Current inset amount in world-space units.
    pub current_amount: f32,
    /// Pre-edit snapshot driving restore and per-frame re-apply.
    pub edit: Option<ModalTopologyEdit>,
    /// Maximum valid inset amount: minimum vertex-to-centroid distance across
    /// all selected faces, with a small safety factor so the inner ring stays
    /// non-degenerate at the cap.
    pub max_inset: f32,
}

/// Maximum valid inset amount for a face: the minimum distance from the face
/// centroid to any ring vertex. `inset_face` moves each ring vertex by `amount`
/// along the inward direction toward the centroid, so the closest vertex
/// collapses to the centroid at `amount = min_vertex_to_centroid`. Beyond that
/// the inner ring inverts.
fn compute_face_max_inset(mesh: &HalfedgeMesh, face_key: FaceKey) -> f32 {
    let face = &mesh.faces[face_key];
    let n = face.loop_count as usize;

    if n < 3 {
        return f32::MAX;
    }

    let mut verts = Vec::with_capacity(n);
    let mut cur = face.loop_first;
    for _ in 0..n {
        let lp = &mesh.loops[cur];
        verts.push(mesh.verts[lp.vert].co);
        cur = lp.next;
    }

    let centroid = verts.iter().sum::<Vec3>() / n as f32;

    let mut min_dist = f32::MAX;
    for v in &verts {
        let d = (centroid - *v).length();
        if d > 1e-6 {
            min_dist = min_dist.min(d);
        }
    }

    min_dist
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushInsetOp>();

    ctx.bind_operator::<CoreExtensionInputContext, BrushInsetOp>([PresetInput::key("KeyI")]);
}

/// Shrink each selected face inward along its plane, controlled by mouse displacement.
/// The amount grows proportionally with cursor movement magnitude; Ctrl snaps
/// to the translate grid increment. The live brush mesh is updated each frame
/// so the inset is visible as a real mesh edit. LMB commits; Esc / RMB cancels.
///
/// Requires Face mode with at least one face selected.
#[operator(
    id = "brush.mesh.inset",
    label = "Inset",
    is_available = can_run_inset,
    modal = true,
    allows_undo = false,
    cancel = cancel_inset,
)]
pub(crate) fn brush_inset(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    mut selection: ResMut<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut halfedge_q: Query<&mut BrushHalfedge>,
    mut modal_state: ResMut<InsetModalState>,
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
        if *edit_mode != EditMode::BrushEdit(BrushEditMode::Face) {
            return OperatorResult::Cancelled;
        }
        let brush_entity = selection.active_brush?;
        let modal_sel_faces: Vec<usize> = selection
            .sub(brush_entity)
            .map(|s| s.faces.clone())
            .unwrap_or_default();
        if modal_sel_faces.is_empty() {
            return OperatorResult::Cancelled;
        }

        let brush_before = brushes.get(brush_entity).cloned()?;
        let halfedge = halfedge_q.get(brush_entity)?;

        // Collect FaceKeys for every selected face index.
        let mut face_keys: Vec<FaceKey> = Vec::with_capacity(modal_sel_faces.len());
        for &face_idx in &modal_sel_faces {
            if let Some(&fk) = halfedge.face_keys.get(face_idx) {
                face_keys.push(fk);
            }
        }
        if face_keys.is_empty() {
            return OperatorResult::Cancelled;
        }

        // Compute geometric max inset: the minimum vertex-to-centroid distance
        // across selected faces, with a tiny safety margin so the inner ring
        // stays non-degenerate at the cap.
        let mut min_reach = f32::MAX;
        for &fk in &face_keys {
            let reach = compute_face_max_inset(&halfedge.mesh, fk);
            min_reach = min_reach.min(reach);
        }
        let max_inset = if min_reach.is_finite() && min_reach > 0.0 {
            min_reach * 0.99
        } else {
            f32::MAX
        };

        modal_state.active = true;
        modal_state.brush_entity = Some(brush_entity);
        modal_state.face_keys = face_keys;
        modal_state.face_indices = modal_sel_faces;
        modal_state.start_cursor = cursor_pos;
        modal_state.current_amount = 0.0;
        modal_state.edit = Some(ModalTopologyEdit::begin(&brush_before, halfedge));
        modal_state.max_inset = max_inset;

        return OperatorResult::Running;
    }

    // --- Subsequent invokes: cancel, update amount, mutate preview, or commit ---

    let rmb = mouse.just_pressed(MouseButton::Right);
    if modal_inputs.cancel() || rmb {
        // Live brush has been mutated each frame, so restore from the snapshot
        // before clearing modal state.
        restore_brush_from_snapshot(&modal_state, &mut brushes, &mut halfedge_q);
        *modal_state = InsetModalState::default();
        return OperatorResult::Cancelled;
    }

    // Compute raw amount from total mouse displacement magnitude.
    // Any movement in any direction grows the inset proportionally.
    let delta = cursor_pos - modal_state.start_cursor;
    let raw_amount = delta.length() * INSET_SENSITIVITY;

    // Clamp to maximum valid inset to prevent inner ring inversion.
    let clamped_amount = raw_amount.min(modal_state.max_inset);

    // Snap respects the global translate_snap toggle; Ctrl flips the current
    // snap state (anti-modifier).
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    modal_state.current_amount =
        if snap_settings.translate_active(ctrl) && snap_settings.translate_increment > 0.0 {
            let inc = snap_settings.translate_increment;
            (clamped_amount / inc).round() * inc
        } else {
            clamped_amount
        };

    // Apply the inset to the live brush mesh so the user sees it as a real
    // mesh edit. The result is discarded; the inset is visible through the
    // regular brush mesh pipeline picking up `Changed<Brush>`. The returned
    // indices identify the post-flatten inner-face slots so the commit path
    // can chain selection without recomputing them.
    let inner_face_indices = apply_live_inset(&mut modal_state, &mut brushes, &mut halfedge_q);

    // Commit on LMB.
    if mouse.just_pressed(MouseButton::Left) {
        let Some(brush_entity) = modal_state.brush_entity else {
            *modal_state = InsetModalState::default();
            return OperatorResult::Cancelled;
        };

        // Zero-amount commit: treat as cancel so we don't write a no-op undo.
        // The live brush should already be back to the snapshot in this case
        // (apply_live_inset resets to the snapshot when amount is sub-threshold).
        if modal_state.current_amount < 1e-5 {
            restore_brush_from_snapshot(&modal_state, &mut brushes, &mut halfedge_q);
            *modal_state = InsetModalState::default();
            return OperatorResult::Cancelled;
        }

        let Ok(brush) = brushes.get(brush_entity).cloned() else {
            *modal_state = InsetModalState::default();
            return OperatorResult::Cancelled;
        };

        // Chain selection: write the newly created inner-ring face indices
        // into BrushSelection so a follow-up gesture (notably a drag along
        // the face normal) can extrude immediately without another hotkey
        // press. Filter out indices that landed past the brush face array;
        // in practice every entry should be in range, but a defensive clamp
        // avoids panicking the operator if any future op change perturbs
        // the index math.
        let face_count = brush.faces.len();
        let inner_indices: Vec<usize> = inner_face_indices
            .into_iter()
            .filter(|&i| i < face_count)
            .collect();
        if !inner_indices.is_empty() {
            selection.sub_mut(brush_entity).faces = inner_indices;
        }

        *modal_state = InsetModalState::default();
        return OperatorResult::Finished;
    }

    OperatorResult::Running
}

/// Cancel handler: restore the brush to its pre-modal state. Called when the
/// modal lifecycle is force-cancelled from outside the operator.
fn cancel_inset(
    mut modal_state: ResMut<InsetModalState>,
    mut brushes: Query<&mut Brush>,
    mut halfedge_q: Query<&mut BrushHalfedge>,
) {
    restore_brush_from_snapshot(&modal_state, &mut brushes, &mut halfedge_q);
    *modal_state = InsetModalState::default();
}

/// Reset the live brush + `HalfedgeMesh` to the snapshot captured at modal start.
fn restore_brush_from_snapshot(
    modal_state: &InsetModalState,
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

/// Re-run `inset_face` against the snapshot at the current amount and write
/// the resulting topology back into the live `Brush` + `BrushHalfedge`. Returns
/// the post-flatten face indices of the new inner-ring faces (one per
/// successful inset), in the same order as `modal_state.face_keys`. The commit
/// path uses these for chained selection.
fn apply_live_inset(
    modal_state: &mut InsetModalState,
    brushes: &mut Query<&mut Brush>,
    halfedge_q: &mut Query<&mut BrushHalfedge>,
) -> Vec<usize> {
    let Some(brush_entity) = modal_state.brush_entity else {
        return Vec::new();
    };
    let Some(edit) = modal_state.edit.as_ref() else {
        return Vec::new();
    };
    let Ok(brush_mut) = brushes.get_mut(brush_entity) else {
        return Vec::new();
    };
    let Ok(mut halfedge) = halfedge_q.get_mut(brush_entity) else {
        return Vec::new();
    };
    let brush = brush_mut.into_inner();

    // Sub-threshold amounts: snap the live brush back to the snapshot.
    if modal_state.current_amount < 1e-5 {
        edit.restore(brush, &mut halfedge);
        return Vec::new();
    }

    // Seed new spoke faces from the face being inset so material + UV scale /
    // rotation / offset inherit from the parent, then re-derive each spoke's UV
    // axes from its own plane. Preferring the parent over the brush's last face
    // keeps the inset's checker tiling continuous with the original face.
    let inset_source = modal_state
        .face_indices
        .first()
        .and_then(|&idx| edit.snapshot_brush().faces.get(idx).cloned())
        .or_else(|| edit.snapshot_brush().faces.last().cloned())
        .unwrap_or_default();
    let original_face_count = edit.snapshot_brush().faces.len();

    // Restore to the snapshot, re-run the inset at the current amount, and
    // reconcile. The closure resolves each new inner-ring face's post-flatten
    // index while the mesh still holds the original material_idx values.
    // `inset_face` rewrites the input face's ring as the inner ring (so
    // `result.inner_face == fk`), preserving its `material_idx`; its side quads
    // share that `material_idx` and are inserted AFTER the original face in
    // slotmap order, so the inner face lands at the topology index
    // `count(faces with material_idx < M)`.
    let amount = modal_state.current_amount;
    let face_keys = modal_state.face_keys.clone();
    let inner_face_indices = edit.apply(brush, &mut halfedge, |mesh| {
        let mut inner_material_idxs: Vec<u32> = Vec::with_capacity(face_keys.len());
        for &fk in &face_keys {
            if let Ok(result) = inset_face(mesh, fk, amount) {
                inner_material_idxs.push(mesh.faces[result.inner_face].material_idx);
            }
        }
        inner_material_idxs
            .iter()
            .map(|&mtx| mesh.faces.values().filter(|f| f.material_idx < mtx).count())
            .collect::<Vec<usize>>()
    });

    // Each new spoke face inherits the parent's appearance with freshly derived
    // UV axes.
    for new_face in original_face_count..brush.faces.len() {
        brush.faces[new_face].copy_appearance_from(&inset_source);
        brush.faces[new_face].uv_u_axis = Vec3::ZERO;
        brush.faces[new_face].uv_v_axis = Vec3::ZERO;
        brush.faces[new_face].ensure_uv_axes();
    }

    inner_face_indices
}

pub(crate) fn can_run_inset(edit_mode: Res<EditMode>, selection: Res<BrushSelection>) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Face)
        && selection.active_sub().is_some_and(|s| !s.faces.is_empty())
}
