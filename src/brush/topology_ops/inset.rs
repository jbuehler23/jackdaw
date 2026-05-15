//! `brush.mesh.inset` operator: modal inset.
//!
//! Press `I` in Face mode. The inset amount is controlled by mouse displacement
//! magnitude: drag any direction to grow the inset proportionally. The brush
//! mesh is mutated each frame so the user sees the live inset as a real mesh
//! edit. Ctrl snaps to the translate grid increment. LMB commits; Esc / RMB
//! cancels and restores the pre-modal mesh.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;
use jackdaw_geometry::editmesh::ops::inset_face::inset_face;
use jackdaw_geometry::editmesh::{EditMesh, FaceKey};
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMesh, BrushEditMode, BrushSelection, EditMode, SetBrush};
use crate::commands::CommandHistory;
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
    /// EditMesh FaceKeys of the faces being inset. Resolved against
    /// `start_editmesh`; we re-resolve them from `start_editmesh` each frame
    /// because the live mesh is reset to the snapshot before running the op.
    pub face_keys: Vec<FaceKey>,
    /// Brush face indices of the faces being inset, captured at modal entry.
    /// Used by the live-preview path to clone the correct template (preserving
    /// the original face's UV scale/rotation/offset) when growing brush.faces
    /// for the new spoke faces, rather than picking up some unrelated face's
    /// UV settings via `start_brush.faces.last()`.
    pub face_indices: Vec<usize>,
    /// Window-space cursor position at the moment the modal started.
    pub start_cursor: Vec2,
    /// Current inset amount in world-space units.
    pub current_amount: f32,
    pub start_brush: Option<Brush>,
    pub start_editmesh: Option<EditMesh>,
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
fn compute_face_max_inset(mesh: &EditMesh, face_key: FaceKey) -> f32 {
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

    let ext = ctx.id();
    ctx.entity_mut().world_scope(|world| {
        world.spawn((
            Action::<BrushInsetOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(KeyCode::KeyI, Press::default())],
        ));
    });
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
    mut bmesh_q: Query<&mut BrushEditMesh>,
    mut history: ResMut<CommandHistory>,
    mut modal_state: ResMut<InsetModalState>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
    snap_settings: Res<SnapSettings>,
    modal_entity: Option<Single<Entity, With<ActiveModalOperator>>>,
) -> OperatorResult {
    // Cursor position in window space (raw, so dragging outside the panel does
    // not abort the modal the way a bounds-clipped viewport cursor would).
    let Ok(window) = primary_window.single() else {
        return OperatorResult::Cancelled;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return OperatorResult::Cancelled;
    };

    // --- First invoke: snapshot and enter modal ---
    if modal_entity.is_none() {
        if *edit_mode != EditMode::BrushEdit(BrushEditMode::Face) {
            return OperatorResult::Cancelled;
        }
        let Some(brush_entity) = selection.entity else {
            return OperatorResult::Cancelled;
        };
        if selection.faces.is_empty() {
            return OperatorResult::Cancelled;
        }

        let Ok(brush_before) = brushes.get(brush_entity).cloned() else {
            return OperatorResult::Cancelled;
        };
        let Ok(bmesh_component) = bmesh_q.get(brush_entity) else {
            return OperatorResult::Cancelled;
        };

        // Collect FaceKeys for every selected face index.
        let mut face_keys: Vec<FaceKey> = Vec::with_capacity(selection.faces.len());
        for &face_idx in &selection.faces {
            if let Some(&fk) = bmesh_component.face_keys.get(face_idx) {
                face_keys.push(fk);
            }
        }
        if face_keys.is_empty() {
            return OperatorResult::Cancelled;
        }

        let mesh_snapshot = bmesh_component.mesh.clone();

        // Compute geometric max inset: the minimum vertex-to-centroid distance
        // across selected faces, with a tiny safety margin so the inner ring
        // stays non-degenerate at the cap.
        let mut min_reach = f32::MAX;
        for &fk in &face_keys {
            let reach = compute_face_max_inset(&mesh_snapshot, fk);
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
        modal_state.face_indices = selection.faces.clone();
        modal_state.start_cursor = cursor_pos;
        modal_state.current_amount = 0.0;
        modal_state.start_brush = Some(brush_before);
        modal_state.start_editmesh = Some(mesh_snapshot);
        modal_state.max_inset = max_inset;

        return OperatorResult::Running;
    }

    // --- Subsequent invokes: cancel, update amount, mutate preview, or commit ---

    let escape = keyboard.just_pressed(KeyCode::Escape);
    let rmb = mouse.just_pressed(MouseButton::Right);
    if escape || rmb {
        // Live brush has been mutated each frame, so restore from the snapshot
        // before clearing modal state.
        restore_brush_from_snapshot(&modal_state, &mut brushes, &mut bmesh_q);
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
    let inner_face_indices = apply_live_inset(&mut modal_state, &mut brushes, &mut bmesh_q);

    // Commit on LMB.
    if mouse.just_pressed(MouseButton::Left) {
        let Some(brush_entity) = modal_state.brush_entity else {
            *modal_state = InsetModalState::default();
            return OperatorResult::Cancelled;
        };
        let Some(start_brush) = modal_state.start_brush.clone() else {
            *modal_state = InsetModalState::default();
            return OperatorResult::Cancelled;
        };

        // Zero-amount commit: treat as cancel so we don't write a no-op undo.
        // The live brush should already be back to the snapshot in this case
        // (apply_live_inset resets to the snapshot when amount is sub-threshold).
        if modal_state.current_amount < 1e-5 {
            restore_brush_from_snapshot(&modal_state, &mut brushes, &mut bmesh_q);
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
            selection.faces = inner_indices;
        }

        history.push_executed(Box::new(SetBrush {
            entity: brush_entity,
            old: start_brush,
            new: brush,
            label: "Inset".to_string(),
        }));

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
    mut bmesh_q: Query<&mut BrushEditMesh>,
) {
    restore_brush_from_snapshot(&modal_state, &mut brushes, &mut bmesh_q);
    *modal_state = InsetModalState::default();
}

/// Reset the live brush + EditMesh to the snapshot captured at modal start.
fn restore_brush_from_snapshot(
    modal_state: &InsetModalState,
    brushes: &mut Query<&mut Brush>,
    bmesh_q: &mut Query<&mut BrushEditMesh>,
) {
    let Some(brush_entity) = modal_state.brush_entity else {
        return;
    };
    let Some(ref start_brush) = modal_state.start_brush else {
        return;
    };
    let Ok(mut brush) = brushes.get_mut(brush_entity) else {
        return;
    };
    *brush = start_brush.clone();
    if let Ok(mut bmesh_component) = bmesh_q.get_mut(brush_entity) {
        let bmesh = EditMesh::lift_from_topology(&start_brush.topology);
        let vert_keys: Vec<_> = bmesh.verts.keys().collect();
        let mut face_keys: Vec<jackdaw_geometry::editmesh::FaceKey> =
            vec![Default::default(); bmesh.faces.len()];
        for (k, f) in bmesh.faces.iter() {
            let slot = f.material_idx as usize;
            if slot < face_keys.len() {
                face_keys[slot] = k;
            }
        }
        bmesh_component.mesh = bmesh;
        bmesh_component.vert_keys = vert_keys;
        bmesh_component.face_keys = face_keys;
    }
}

/// Re-run `inset_face` against the snapshot at the current amount and write
/// the resulting topology back into the live `Brush` + `BrushEditMesh`. Returns
/// the post-flatten face indices of the new inner-ring faces (one per
/// successful inset), in the same order as `modal_state.face_keys`. The commit
/// path uses these for chained selection.
fn apply_live_inset(
    modal_state: &mut InsetModalState,
    brushes: &mut Query<&mut Brush>,
    bmesh_q: &mut Query<&mut BrushEditMesh>,
) -> Vec<usize> {
    let Some(brush_entity) = modal_state.brush_entity else {
        return Vec::new();
    };
    let Some(ref start_mesh) = modal_state.start_editmesh else {
        return Vec::new();
    };
    let Some(ref start_brush) = modal_state.start_brush else {
        return Vec::new();
    };
    let Ok(mut bmesh_component) = bmesh_q.get_mut(brush_entity) else {
        return Vec::new();
    };

    // Sub-threshold amounts: snap the live mesh back to the start state.
    if modal_state.current_amount < 1e-5 {
        let Ok(mut brush) = brushes.get_mut(brush_entity) else {
            return Vec::new();
        };
        *brush = start_brush.clone();
        let bmesh = EditMesh::lift_from_topology(&start_brush.topology);
        let vert_keys: Vec<_> = bmesh.verts.keys().collect();
        let mut face_keys: Vec<jackdaw_geometry::editmesh::FaceKey> =
            vec![Default::default(); bmesh.faces.len()];
        for (k, f) in bmesh.faces.iter() {
            let slot = f.material_idx as usize;
            if slot < face_keys.len() {
                face_keys[slot] = k;
            }
        }
        bmesh_component.mesh = bmesh;
        bmesh_component.vert_keys = vert_keys;
        bmesh_component.face_keys = face_keys;
        return Vec::new();
    }

    // Always start the per-frame op from the clean snapshot.
    bmesh_component.mesh = start_mesh.clone();

    // Run inset on each selected face; skip degenerate faces silently.
    // `inset_face` rewrites the input face's ring as the inner ring (i.e.
    // `result.inner_face == fk`), preserving its `material_idx`. We capture
    // that material_idx so we can resolve the post-flatten face index for
    // each new inner ring (see `inner_face_indices` below). The side quads
    // it adds inherit the same `material_idx` and are inserted AFTER the
    // original face in slotmap order, so among ties on a given M the original
    // (now inner) face sits first; therefore the inner face lands at the
    // topology index equal to `count(faces with material_idx < M)`.
    let mut inner_material_idxs: Vec<u32> = Vec::with_capacity(modal_state.face_keys.len());
    for &fk in &modal_state.face_keys {
        if let Ok(result) = inset_face(&mut bmesh_component.mesh, fk, modal_state.current_amount) {
            let mtx = bmesh_component.mesh.faces[result.inner_face].material_idx;
            inner_material_idxs.push(mtx);
        }
    }

    // Resolve post-flatten inner-face indices BEFORE flatten/re-lift, while
    // the bmesh still holds the original material_idx values.
    let inner_face_indices: Vec<usize> = inner_material_idxs
        .iter()
        .map(|&mtx| {
            bmesh_component
                .mesh
                .faces
                .values()
                .filter(|f| f.material_idx < mtx)
                .count()
        })
        .collect();

    // Re-cache all face normals; inset reshapes the rebuilt inner + side faces.
    let face_keys_all: Vec<_> = bmesh_component.mesh.faces.keys().collect();
    for fk in face_keys_all {
        let face = &bmesh_component.mesh.faces[fk];
        let mut ring_positions = Vec::with_capacity(face.loop_count as usize);
        let mut cur = face.loop_first;
        for _ in 0..face.loop_count {
            let lp = &bmesh_component.mesh.loops[cur];
            ring_positions.push(bmesh_component.mesh.verts[lp.vert].co);
            cur = lp.next;
        }
        let new_normal = jackdaw_geometry::newell_normal(&ring_positions);
        bmesh_component.mesh.faces[fk].normal_cache = new_normal;
    }

    // Flatten EditMesh -> topology, sync Brush.
    let new_topology = bmesh_component.mesh.flatten_to_topology();
    let Ok(mut brush) = brushes.get_mut(brush_entity) else {
        return inner_face_indices;
    };

    // Grow brush.faces to cover the new spoke faces. Seed each new slot from
    // the face being inset (so material + uv_scale + uv_rotation + uv_offset
    // inherit from the parent face), then zero out the `uv_u_axis` /
    // `uv_v_axis` so `ensure_uv_axes` below derives proper tangents from each
    // spoke face's own plane normal. Picking the parent face as the template
    // rather than `start_brush.faces.last()` keeps the inset's checker
    // tiling continuous with the original face on the spoke walls.
    let new_face_count = new_topology.polygons.len();
    let original_face_count = start_brush.faces.len();
    let inset_template = modal_state
        .face_indices
        .first()
        .and_then(|&idx| start_brush.faces.get(idx).cloned())
        .or_else(|| start_brush.faces.last().cloned());
    while brush.faces.len() < new_face_count {
        let mut template = inset_template
            .clone()
            .or_else(|| brush.faces.last().cloned())
            .unwrap_or_default();
        template.uv_u_axis = Vec3::ZERO;
        template.uv_v_axis = Vec3::ZERO;
        brush.faces.push(template);
    }

    let positions: Vec<Vec3> = new_topology.vertices.iter().map(|v| v.position).collect();
    for (face_idx, face_data) in brush.faces.iter_mut().enumerate() {
        if face_idx < new_topology.polygons.len() {
            let normal = new_topology.face_normal_with(&positions, face_idx);
            let v0_idx = new_topology.loops[new_topology.polygons[face_idx].loop_start as usize]
                .vert as usize;
            let distance = positions[v0_idx].dot(normal);
            face_data.plane.normal = normal;
            face_data.plane.distance = distance;
            if face_idx >= original_face_count {
                face_data.ensure_uv_axes();
            }
        }
    }
    brush.topology = new_topology;

    // Re-lift EditMesh from the new topology so vert_keys / face_keys are
    // consistent with the brush. Next frame we reset bmesh_component.mesh
    // back to the snapshot before running the op again.
    let new_bmesh = EditMesh::lift_from_topology(&brush.topology);
    let new_vert_keys: Vec<_> = new_bmesh.verts.keys().collect();
    let mut new_face_keys = vec![Default::default(); new_bmesh.faces.len()];
    for (k, f) in new_bmesh.faces.iter() {
        let slot = f.material_idx as usize;
        if slot < new_face_keys.len() {
            new_face_keys[slot] = k;
        }
    }
    bmesh_component.mesh = new_bmesh;
    bmesh_component.vert_keys = new_vert_keys;
    bmesh_component.face_keys = new_face_keys;

    inner_face_indices
}

pub(crate) fn can_run_inset(edit_mode: Res<EditMode>, selection: Res<BrushSelection>) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Face) && !selection.faces.is_empty()
}
