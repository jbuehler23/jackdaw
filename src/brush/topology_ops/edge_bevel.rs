//! `brush.mesh.edge_bevel` operator: modal edge bevel.
//!
//! Press `Ctrl+B` in Edge mode with at least one edge selected. Cursor
//! displacement magnitude drives a positive bevel width; Ctrl snaps the world
//! width to the translate grid increment. The brush mesh is mutated each
//! frame so the user sees the live chamfer as they drag. LMB commits; Esc /
//! RMB cancels and restores the pre-modal mesh.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;
use jackdaw_geometry::halfedge::ops::edge_bevel::edge_bevel;
use jackdaw_geometry::halfedge::{EdgeKey, HalfedgeMesh, VertKey};
use jackdaw_jsn::Brush;

use super::modal_edit::ModalTopologyEdit;
use crate::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode};
use crate::core_extension::CoreExtensionInputContext;
use crate::snapping::SnapSettings;

/// Pixels-per-world-unit sensitivity for the edge bevel modal. At this value
/// 100 pixels of cursor motion correspond to 1 world-unit of bevel width.
const BEVEL_SENSITIVITY: f32 = 0.01;

/// Modal state for the `brush.mesh.edge_bevel` operator.
#[derive(Resource, Default)]
pub struct EdgeBevelModalState {
    pub active: bool,
    pub brush_entity: Option<Entity>,
    /// `HalfedgeMesh` `EdgeKeys` of the edges being beveled. Slotmap keys stay
    /// valid across the snapshot's clones, so they re-resolve each frame after
    /// the snapshot is restored.
    pub edge_keys: Vec<EdgeKey>,
    /// Window-space cursor position at the moment the modal started.
    pub start_cursor: Vec2,
    /// Current bevel width in world-space units.
    pub current_width: f32,
    /// Pre-edit snapshot driving restore and per-frame re-apply.
    pub edit: Option<ModalTopologyEdit>,
    /// Maximum valid bevel width: minimum over each input edge of half the
    /// length of every parallel edge at its endpoints, with a small safety
    /// factor. Past this point an offset overshoots its parallel edge and
    /// the rebuilt face collapses.
    pub max_width: f32,
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushEdgeBevelOp>();

    ctx.bind_operator::<CoreExtensionInputContext, BrushEdgeBevelOp>([
        PresetInput::key("KeyB").ctrl()
    ]);
}

/// Chamfer each selected edge into a quad, controlled by cursor displacement
/// magnitude. Ctrl snaps to the translate grid increment. The live brush mesh
/// is updated each frame so the chamfer is visible as a real mesh edit. LMB
/// commits; Esc / RMB cancels and reverts.
///
/// Requires Edge mode with at least one edge selected.
#[operator(
    id = "brush.mesh.edge_bevel",
    label = "Edge Bevel",
    is_available = can_run_edge_bevel,
    modal = true,
    allows_undo = false,
    cancel = cancel_edge_bevel,
)]
pub(crate) fn brush_edge_bevel(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut halfedge_q: Query<&mut BrushHalfedge>,
    mut modal_state: ResMut<EdgeBevelModalState>,
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
        if *edit_mode != EditMode::BrushEdit(BrushEditMode::Edge) {
            return OperatorResult::Cancelled;
        }
        let brush_entity = selection.active_brush?;
        let sel_edges: Vec<(usize, usize)> = selection
            .sub(brush_entity)
            .map(|s| s.edges.clone())
            .unwrap_or_default();
        if sel_edges.is_empty() {
            return OperatorResult::Cancelled;
        }

        let brush_before = brushes.get(brush_entity).cloned()?;
        let halfedge = halfedge_q.get(brush_entity)?;

        // Resolve HalfedgeMesh EdgeKeys for every selected cache edge pair.
        let mut edge_keys: Vec<EdgeKey> = Vec::with_capacity(sel_edges.len());
        for &(a, b) in &sel_edges {
            let Some(&va) = halfedge.vert_keys.get(a) else {
                continue;
            };
            let Some(&vb) = halfedge.vert_keys.get(b) else {
                continue;
            };
            if let Some(ek) = find_edge_between(&halfedge.mesh, va, vb) {
                edge_keys.push(ek);
            }
        }
        if edge_keys.is_empty() {
            return OperatorResult::Cancelled;
        }

        let max_width = compute_max_bevel_width(&halfedge.mesh, &edge_keys);

        modal_state.active = true;
        modal_state.brush_entity = Some(brush_entity);
        modal_state.edge_keys = edge_keys;
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
        *modal_state = EdgeBevelModalState::default();
        return OperatorResult::Cancelled;
    }

    // Cursor distance from the initial position drives the width. Any drag
    // direction grows the bevel proportionally to how far you've moved from
    // where you started.
    let delta = cursor_pos - modal_state.start_cursor;
    let raw_width = delta.length() * BEVEL_SENSITIVITY;
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
    // mesh edit. The op result is discarded; the chamfer is visible through
    // the regular brush mesh pipeline picking up `Changed<Brush>`.
    apply_live_bevel(&mut modal_state, &mut brushes, &mut halfedge_q);

    // Commit on LMB.
    if mouse.just_pressed(MouseButton::Left) {
        // Zero-width commit: treat as cancel so we don't write a no-op undo.
        // The live brush should already be back to the snapshot in this case
        // (apply_live_bevel resets to the snapshot when width is sub-threshold).
        if modal_state.current_width < 1e-5 {
            restore_brush_from_snapshot(&modal_state, &mut brushes, &mut halfedge_q);
            *modal_state = EdgeBevelModalState::default();
            return OperatorResult::Cancelled;
        }

        *modal_state = EdgeBevelModalState::default();
        return OperatorResult::Finished;
    }

    OperatorResult::Running
}

/// Cancel handler: restore the brush to its pre-modal state. Called when the
/// modal lifecycle is force-cancelled from outside the operator (e.g. a
/// higher-priority operator preempts us).
fn cancel_edge_bevel(
    mut modal_state: ResMut<EdgeBevelModalState>,
    mut brushes: Query<&mut Brush>,
    mut halfedge_q: Query<&mut BrushHalfedge>,
) {
    restore_brush_from_snapshot(&modal_state, &mut brushes, &mut halfedge_q);
    *modal_state = EdgeBevelModalState::default();
}

/// Reset the live brush + `HalfedgeMesh` to the snapshot captured at modal start.
fn restore_brush_from_snapshot(
    modal_state: &EdgeBevelModalState,
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

/// Re-run `edge_bevel` against the snapshot at the current width and write
/// the resulting topology back into the live `Brush` + `BrushHalfedge`. The
/// brush mesh pipeline picks up `Changed<Brush>` and regenerates the GPU
/// mesh on the next frame.
fn apply_live_bevel(
    modal_state: &mut EdgeBevelModalState,
    brushes: &mut Query<&mut Brush>,
    halfedge_q: &mut Query<&mut BrushHalfedge>,
) {
    let Some(brush_entity) = modal_state.brush_entity else {
        return;
    };
    let Some(edit) = modal_state.edit.as_ref() else {
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

    // Seed new chamfer faces from the snapshot's last face (for material +
    // uv_scale / rotation), then re-derive each chamfer's UV axes from its own
    // plane (inheriting axes from a different-orientation template stretches the
    // texture).
    let source = edit
        .snapshot_brush()
        .faces
        .last()
        .cloned()
        .unwrap_or_default();
    let original_face_count = edit.snapshot_brush().faces.len();

    // Restore to the snapshot, re-run the bevel at the current width, and
    // reconcile. Non-manifold edges are skipped inside the op.
    let edge_keys = modal_state.edge_keys.clone();
    let width = modal_state.current_width;
    edit.apply(brush, &mut halfedge, |mesh| {
        let _ = edge_bevel(mesh, &edge_keys, width);
    });

    // Each new chamfer face inherits the source appearance with freshly derived
    // UV axes.
    for new_face in original_face_count..brush.faces.len() {
        brush.faces[new_face].copy_appearance_from(&source);
        brush.faces[new_face].uv_u_axis = Vec3::ZERO;
        brush.faces[new_face].uv_v_axis = Vec3::ZERO;
        brush.faces[new_face].ensure_uv_axes();
    }
}

/// Geometric cap on bevel width: half the length of the shortest parallel
/// edge at any endpoint of any input edge, times a 0.99 safety factor.
///
/// Rationale: the bevel offsets each endpoint of the parallel edge toward the
/// other by `width`. Both endpoints offset symmetrically (when their incident
/// edges are also beveled, they collapse from both ends), so the parallel
/// edge collapses at `width == length / 2`. We back off slightly so the
/// rebuilt face is never exactly degenerate.
fn compute_max_bevel_width(mesh: &HalfedgeMesh, edges: &[EdgeKey]) -> f32 {
    let mut min_half_len = f32::MAX;
    for &edge_key in edges {
        let Some(edge) = mesh.edges.get(edge_key) else {
            continue;
        };
        // Both adjacent face loops on this edge.
        let radial: Vec<_> =
            jackdaw_geometry::halfedge::cycles::radial_walk(mesh, edge_key).collect();
        if radial.len() != 2 {
            continue;
        }
        let v0 = edge.v[0];
        let v1 = edge.v[1];
        for lp in radial {
            let face = mesh.loops[lp].face;
            if mesh.faces[face].loop_count < 4 {
                continue;
            }
            // Find loops at v0 and v1 within this face and inspect the
            // parallel edge at each endpoint.
            for target in [v0, v1] {
                let loop_at_v = if mesh.loops[lp].vert == target {
                    lp
                } else {
                    mesh.loops[lp].next
                };
                // Parallel edge at this endpoint = the OTHER ring edge at v.
                let lp_data = &mesh.loops[loop_at_v];
                let par_edge = if lp_data.edge == edge_key {
                    mesh.loops[lp_data.prev].edge
                } else {
                    lp_data.edge
                };
                let Some(par) = mesh.edges.get(par_edge) else {
                    continue;
                };
                let length = (mesh.verts[par.v[0]].co - mesh.verts[par.v[1]].co).length();
                let half = length * 0.5;
                if half > 1e-6 && half < min_half_len {
                    min_half_len = half;
                }
            }
        }
    }
    if min_half_len.is_finite() {
        min_half_len * 0.99
    } else {
        f32::MAX
    }
}

fn find_edge_between(mesh: &HalfedgeMesh, va: VertKey, vb: VertKey) -> Option<EdgeKey> {
    mesh.edges
        .iter()
        .find(|(_, e)| (e.v[0] == va && e.v[1] == vb) || (e.v[0] == vb && e.v[1] == va))
        .map(|(k, _)| k)
}

pub(crate) fn can_run_edge_bevel(edit_mode: Res<EditMode>, selection: Res<BrushSelection>) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Edge)
        && selection.active_sub().is_some_and(|s| !s.edges.is_empty())
}
