//! Extrude operators:
//!
//! - `brush.mesh.extrude_region` (non-modal): one-shot extrusion at a fixed
//!   depth, available via menu / command palette.
//! - `brush.mesh.extrude` (modal, bound to `E`): modal where the
//!   cursor's projected motion along the face normal drives a signed extrusion
//!   amount. The brush mesh is mutated each frame so the user sees the live
//!   extrusion as a real mesh edit.
//!
//! Both share the same `HalfedgeMesh` op (`extrude_face_region`) and the same
//! chained selection behavior: post-commit, `BrushSelection.faces` is updated
//! to the new top face indices.

use bevy::prelude::*;
use bevy::ui::ui_transform::UiGlobalTransform;
use bevy::window::PrimaryWindow;
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;
use jackdaw_geometry::halfedge::ops::extrude_face_region::extrude_face_region;
use jackdaw_geometry::halfedge::{FaceKey, HalfedgeMesh};
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode, SetBrush};
use crate::commands::CommandHistory;
use crate::core_extension::CoreExtensionInputContext;
use crate::snapping::SnapSettings;
use crate::viewport::{MainViewportCamera, SceneViewport};
use crate::viewport_util::ViewportRemap;

const DEFAULT_EXTRUDE_DEPTH: f32 = 0.5;

/// Pixels-per-world-unit sensitivity for the modal extrude.
/// At this value 100 pixels of cursor movement along the screen-projected face
/// normal corresponds to 1 world-unit of extrusion. Tune as needed.
const EXTRUDE_SENSITIVITY: f32 = 0.01;

/// Modal state for the `brush.mesh.extrude` operator.
#[derive(Resource, Default)]
pub struct ExtrudeModalState {
    pub active: bool,
    pub brush_entity: Option<Entity>,
    /// `HalfedgeMesh` `FaceKeys` of the faces being extruded. Resolved against
    /// `start_mesh`; we re-resolve them from `start_mesh` each frame
    /// because the live mesh is reset to the snapshot before running the op.
    pub face_keys: Vec<FaceKey>,
    /// Brush face indices of the faces being extruded, captured at modal
    /// entry. Used by the live-preview path to clone the correct template
    /// (preserving the parent face's UV scale/rotation/offset) when growing
    /// brush.faces for new wall faces, rather than picking up some unrelated
    /// face's UV settings via `start_brush.faces.last()`.
    pub face_indices: Vec<usize>,
    /// Window-space cursor position at the moment the modal started.
    pub start_cursor: Vec2,
    /// Unit-length screen-space direction corresponding to "+1 unit along the
    /// representative face normal" (in window pixels). Used to map cursor
    /// motion onto a signed scalar amount.
    pub screen_normal_dir: Vec2,
    /// Current signed extrude amount in world-space units. Positive = along
    /// face normal; negative = against face normal.
    pub current_amount: f32,
    pub start_brush: Option<Brush>,
    pub start_mesh: Option<HalfedgeMesh>,
}

/// Duplicate each selected face along its normal by a fixed depth and
/// connect the old and new rings with side quads. Operates on the current
/// face selection. Available in Face mode with at least one face selected.
#[operator(
    id = "brush.mesh.extrude_region",
    label = "Extrude Region",
    is_available = can_run_extrude_region,
    allows_undo = true
)]
pub(crate) fn brush_extrude_region(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    mut selection: ResMut<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut halfedge_q: Query<&mut BrushHalfedge>,
    mut history: ResMut<CommandHistory>,
) -> OperatorResult {
    if *edit_mode != EditMode::BrushEdit(BrushEditMode::Face) {
        return OperatorResult::Cancelled;
    }
    let Some(brush_entity) = selection.entity else {
        return OperatorResult::Cancelled;
    };
    if selection.faces.is_empty() {
        return OperatorResult::Cancelled;
    }

    // Snapshot before mutation for undo.
    let Ok(brush_before) = brushes.get(brush_entity).cloned() else {
        return OperatorResult::Cancelled;
    };

    // Map cache face indices to HalfedgeMesh FaceKeys via face_keys parallel array.
    let Ok(mut halfedge) = halfedge_q.get_mut(brush_entity) else {
        return OperatorResult::Cancelled;
    };
    let mut mesh_faces: Vec<FaceKey> = Vec::with_capacity(selection.faces.len());
    for &face_idx in &selection.faces {
        if let Some(&fk) = halfedge.face_keys.get(face_idx) {
            mesh_faces.push(fk);
        }
    }
    if mesh_faces.is_empty() {
        return OperatorResult::Cancelled;
    }

    // Run extrude_face_region on each selected face. Capture each successful
    // extrusion's top-face `material_idx` so we can resolve the post-flatten
    // face index for the new top ring (see chaining block below).
    //
    // `extrude_face_region` reuses the input face as the new top cap (i.e.
    // `result.top_face == fk`), preserving its `material_idx`. The N side
    // quads it adds inherit the same `material_idx`. Among ties on a given M
    // the original (now top) face sits first in slotmap iteration order, so
    // the top face lands at the topology index equal to
    // `count(faces with material_idx < M)` after flatten.
    let mut top_material_idxs: Vec<u32> = Vec::with_capacity(mesh_faces.len());
    for fk in mesh_faces {
        if let Ok(result) = extrude_face_region(&mut halfedge.mesh, fk, DEFAULT_EXTRUDE_DEPTH) {
            let mtx = halfedge.mesh.faces[result.top_face].material_idx;
            top_material_idxs.push(mtx);
        }
    }

    // Re-cache all face normals.
    let face_keys_all: Vec<_> = halfedge.mesh.faces.keys().collect();
    for fk in face_keys_all {
        let face = &halfedge.mesh.faces[fk];
        let mut ring_positions = Vec::with_capacity(face.loop_count as usize);
        let mut cur = face.loop_first;
        for _ in 0..face.loop_count {
            let lp = &halfedge.mesh.loops[cur];
            ring_positions.push(halfedge.mesh.verts[lp.vert].co);
            cur = lp.next;
        }
        let new_normal = jackdaw_geometry::newell_normal(&ring_positions);
        halfedge.mesh.faces[fk].normal_cache = new_normal;
    }

    // Flatten HalfedgeMesh -> topology, sync Brush.faces[i].plane + Brush.topology.
    let new_topology = halfedge.mesh.flatten_to_topology();
    let Ok(mut brush) = brushes.get_mut(brush_entity) else {
        return OperatorResult::Cancelled;
    };

    // Extrude adds new faces. Extend brush.faces with copies of the last
    // existing face data as a default; material_idx from the parent face is
    // inherited during flatten.
    let new_face_count = new_topology.polygons.len();
    while brush.faces.len() < new_face_count {
        let template = brush.faces.last().cloned().unwrap_or_default();
        brush.faces.push(template);
    }

    // Update plane data per face from new topology.
    let positions: Vec<Vec3> = new_topology.vertices.iter().map(|v| v.position).collect();
    for (face_idx, face_data) in brush.faces.iter_mut().enumerate() {
        if face_idx < new_topology.polygons.len() {
            let normal = new_topology.face_normal_with(&positions, face_idx);
            let v0_idx = new_topology.loops[new_topology.polygons[face_idx].loop_start as usize]
                .vert as usize;
            let distance = positions[v0_idx].dot(normal);
            face_data.plane.normal = normal;
            face_data.plane.distance = distance;
        }
    }
    brush.topology = new_topology;

    // Re-lift HalfedgeMesh from new topology so vert_keys / face_keys are consistent.
    let new_mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let new_vert_keys: Vec<_> = new_mesh.verts.keys().collect();
    let mut new_face_keys = vec![Default::default(); new_mesh.faces.len()];
    for (k, f) in new_mesh.faces.iter() {
        let slot = f.material_idx as usize;
        if slot < new_face_keys.len() {
            new_face_keys[slot] = k;
        }
    }
    halfedge.mesh = new_mesh;
    halfedge.vert_keys = new_vert_keys;
    halfedge.face_keys = new_face_keys;

    // Push undo entry.
    history.push_executed(Box::new(SetBrush {
        entity: brush_entity,
        old: brush_before,
        new: brush.clone(),
        label: "Extrude Region".to_string(),
    }));

    // Chain selection: write the new top face(s) into `BrushSelection.faces`
    // so a follow-up gesture (drag-along-normal, inset, extrude again) can
    // operate on the freshly created top ring immediately.
    let face_count = brush.faces.len();
    let new_top_indices: Vec<usize> = top_material_idxs
        .into_iter()
        .map(|mtx| {
            halfedge
                .mesh
                .faces
                .values()
                .filter(|f| f.material_idx < mtx)
                .count()
        })
        .filter(|&i| i < face_count)
        .collect();
    if !new_top_indices.is_empty() {
        selection.faces = new_top_indices;
    }

    OperatorResult::Finished
}

pub(crate) fn can_run_extrude_region(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Face) && !selection.faces.is_empty()
}

// --- Modal extrude (`brush.mesh.extrude`, bound to `E`) ---

/// Extrude each selected face along its normal by a signed amount controlled
/// by cursor motion projected onto the screen-space face normal. Positive
/// values push outward; negative values pull inward. The live brush mesh is
/// updated each frame so the extrusion is visible as a real mesh edit. Ctrl
/// snaps to the translate grid increment. LMB commits; Esc / RMB cancels and
/// reverts.
///
/// Requires Face mode with at least one face selected.
#[operator(
    id = "brush.mesh.extrude",
    label = "Extrude",
    is_available = can_run_extrude,
    modal = true,
    allows_undo = false,
    cancel = cancel_extrude,
)]
pub(crate) fn brush_extrude(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    mut selection: ResMut<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut halfedge_q: Query<&mut BrushHalfedge>,
    brush_transforms: Query<&GlobalTransform>,
    mut history: ResMut<CommandHistory>,
    mut modal_state: ResMut<ExtrudeModalState>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    snap_settings: Res<SnapSettings>,
    modal_entity: Option<Single<Entity, With<ActiveModalOperator>>>,
) -> OperatorResult {
    // Raw window-space cursor; dragging outside the viewport panel should not
    // cancel the modal (matches inset / loop_cut behavior).
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
        let Ok(halfedge) = halfedge_q.get(brush_entity) else {
            return OperatorResult::Cancelled;
        };

        let mut face_keys: Vec<FaceKey> = Vec::with_capacity(selection.faces.len());
        for &face_idx in &selection.faces {
            if let Some(&fk) = halfedge.face_keys.get(face_idx) {
                face_keys.push(fk);
            }
        }
        if face_keys.is_empty() {
            return OperatorResult::Cancelled;
        }

        let mesh_snapshot = halfedge.mesh.clone();
        let brush_xform = brush_transforms.get(brush_entity).ok();

        // Derive the screen-space direction corresponding to "+1 world-unit
        // along the representative face normal". Fall back to (0, -1) (cursor
        // up = positive amount) when the camera projection isn't available,
        // matching the heuristic mentioned in the spec.
        let screen_normal_dir = compute_screen_normal_dir(
            &mesh_snapshot,
            &face_keys,
            brush_xform,
            &camera_query,
            &viewport_query,
        );

        modal_state.active = true;
        modal_state.brush_entity = Some(brush_entity);
        modal_state.face_keys = face_keys;
        modal_state.face_indices = selection.faces.clone();
        modal_state.start_cursor = cursor_pos;
        modal_state.screen_normal_dir = screen_normal_dir;
        modal_state.current_amount = 0.0;
        modal_state.start_brush = Some(brush_before);
        modal_state.start_mesh = Some(mesh_snapshot);

        return OperatorResult::Running;
    }

    // --- Subsequent invokes: cancel, update amount, mutate preview, or commit ---

    let escape = keyboard.just_pressed(KeyCode::Escape);
    let rmb = mouse.just_pressed(MouseButton::Right);
    if escape || rmb {
        // Live brush has been mutated each frame, so restore from the snapshot
        // before clearing modal state.
        restore_brush_from_snapshot(&modal_state, &mut brushes, &mut halfedge_q);
        *modal_state = ExtrudeModalState::default();
        return OperatorResult::Cancelled;
    }

    // Signed projection of cursor motion onto the screen-normal direction.
    let cursor_delta = cursor_pos - modal_state.start_cursor;
    let raw_amount = cursor_delta.dot(modal_state.screen_normal_dir) * EXTRUDE_SENSITIVITY;

    // Snap respects the global translate_snap toggle; Ctrl flips the current
    // snap state (anti-modifier).
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    modal_state.current_amount =
        if snap_settings.translate_active(ctrl) && snap_settings.translate_increment > 0.0 {
            let inc = snap_settings.translate_increment;
            (raw_amount / inc).round() * inc
        } else {
            raw_amount
        };

    // Apply the extrude to the live brush mesh so the user sees it as a real
    // mesh edit. The op result is discarded; the extrusion is visible through
    // the regular brush mesh pipeline picking up `Changed<Brush>`. The returned
    // indices identify the post-flatten top-face slots so the commit path can
    // chain selection without recomputing them.
    let top_face_indices = apply_live_extrude(&mut modal_state, &mut brushes, &mut halfedge_q);

    // Commit on LMB.
    if mouse.just_pressed(MouseButton::Left) {
        let Some(brush_entity) = modal_state.brush_entity else {
            *modal_state = ExtrudeModalState::default();
            return OperatorResult::Cancelled;
        };
        let Some(start_brush) = modal_state.start_brush.clone() else {
            *modal_state = ExtrudeModalState::default();
            return OperatorResult::Cancelled;
        };

        // Degenerate zero-amount commit: treat as no-op cancel so we don't
        // record a useless undo entry. The live brush should already be back
        // to the snapshot (apply_live_extrude resets when amount is sub-threshold).
        if modal_state.current_amount.abs() < 1e-4 {
            restore_brush_from_snapshot(&modal_state, &mut brushes, &mut halfedge_q);
            *modal_state = ExtrudeModalState::default();
            return OperatorResult::Cancelled;
        }

        let Ok(brush) = brushes.get(brush_entity).cloned() else {
            *modal_state = ExtrudeModalState::default();
            return OperatorResult::Cancelled;
        };

        // Chain selection: write the new top face indices into
        // `BrushSelection.faces` so a follow-up gesture (drag-along-normal,
        // inset, extrude again) can act on them immediately. Filter out
        // indices that landed past the brush face array; defensive clamp.
        let face_count = brush.faces.len();
        let new_top_indices: Vec<usize> = top_face_indices
            .into_iter()
            .filter(|&i| i < face_count)
            .collect();
        if !new_top_indices.is_empty() {
            selection.faces = new_top_indices;
        }

        history.push_executed(Box::new(SetBrush {
            entity: brush_entity,
            old: start_brush,
            new: brush,
            label: "Extrude".to_string(),
        }));

        *modal_state = ExtrudeModalState::default();
        return OperatorResult::Finished;
    }

    OperatorResult::Running
}

/// Cancel handler: restore the brush to its pre-modal state. Called when the
/// modal lifecycle is force-cancelled from outside the operator.
fn cancel_extrude(
    mut modal_state: ResMut<ExtrudeModalState>,
    mut brushes: Query<&mut Brush>,
    mut halfedge_q: Query<&mut BrushHalfedge>,
) {
    restore_brush_from_snapshot(&modal_state, &mut brushes, &mut halfedge_q);
    *modal_state = ExtrudeModalState::default();
}

/// Reset the live brush + `HalfedgeMesh` to the snapshot captured at modal start.
fn restore_brush_from_snapshot(
    modal_state: &ExtrudeModalState,
    brushes: &mut Query<&mut Brush>,
    halfedge_q: &mut Query<&mut BrushHalfedge>,
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
    if let Ok(mut halfedge) = halfedge_q.get_mut(brush_entity) {
        let mesh = HalfedgeMesh::lift_from_topology(&start_brush.topology);
        let vert_keys: Vec<_> = mesh.verts.keys().collect();
        let mut face_keys: Vec<jackdaw_geometry::halfedge::FaceKey> =
            vec![Default::default(); mesh.faces.len()];
        for (k, f) in mesh.faces.iter() {
            let slot = f.material_idx as usize;
            if slot < face_keys.len() {
                face_keys[slot] = k;
            }
        }
        halfedge.mesh = mesh;
        halfedge.vert_keys = vert_keys;
        halfedge.face_keys = face_keys;
    }
}

/// Re-run `extrude_face_region` against the snapshot at the current amount and
/// write the resulting topology back into the live `Brush` + `BrushHalfedge`.
/// Returns the post-flatten face indices of the new top faces (one per
/// successful extrusion), in the same order as `modal_state.face_keys`. The
/// commit path uses these for chained selection.
fn apply_live_extrude(
    modal_state: &mut ExtrudeModalState,
    brushes: &mut Query<&mut Brush>,
    halfedge_q: &mut Query<&mut BrushHalfedge>,
) -> Vec<usize> {
    let Some(brush_entity) = modal_state.brush_entity else {
        return Vec::new();
    };
    let Some(ref start_mesh) = modal_state.start_mesh else {
        return Vec::new();
    };
    let Some(ref start_brush) = modal_state.start_brush else {
        return Vec::new();
    };
    let Ok(mut halfedge) = halfedge_q.get_mut(brush_entity) else {
        return Vec::new();
    };

    // Sub-threshold amounts: snap the live mesh back to the start state.
    if modal_state.current_amount.abs() < 1e-4 {
        let Ok(mut brush) = brushes.get_mut(brush_entity) else {
            return Vec::new();
        };
        *brush = start_brush.clone();
        let mesh = HalfedgeMesh::lift_from_topology(&start_brush.topology);
        let vert_keys: Vec<_> = mesh.verts.keys().collect();
        let mut face_keys: Vec<jackdaw_geometry::halfedge::FaceKey> =
            vec![Default::default(); mesh.faces.len()];
        for (k, f) in mesh.faces.iter() {
            let slot = f.material_idx as usize;
            if slot < face_keys.len() {
                face_keys[slot] = k;
            }
        }
        halfedge.mesh = mesh;
        halfedge.vert_keys = vert_keys;
        halfedge.face_keys = face_keys;
        return Vec::new();
    }

    // Always start the per-frame op from the clean snapshot.
    halfedge.mesh = start_mesh.clone();

    // Run extrude on each selected face; capture each successful op's
    // top-face `material_idx` for the chained-selection index math.
    // `extrude_face_region` reuses the input face as the new top cap (i.e.
    // `result.top_face == fk`), preserving its `material_idx`. The N side
    // quads it adds inherit the same `material_idx`. Among ties on a given M
    // the original (now top) face sits first in slotmap iteration order, so
    // the top face lands at the topology index equal to
    // `count(faces with material_idx < M)` after flatten.
    let mut top_material_idxs: Vec<u32> = Vec::with_capacity(modal_state.face_keys.len());
    for &fk in &modal_state.face_keys {
        if let Ok(result) = extrude_face_region(&mut halfedge.mesh, fk, modal_state.current_amount)
        {
            let mtx = halfedge.mesh.faces[result.top_face].material_idx;
            top_material_idxs.push(mtx);
        }
    }

    // Resolve post-flatten top-face indices BEFORE flatten/re-lift, while
    // the mesh still holds the original material_idx values.
    let top_face_indices: Vec<usize> = top_material_idxs
        .iter()
        .map(|&mtx| {
            halfedge
                .mesh
                .faces
                .values()
                .filter(|f| f.material_idx < mtx)
                .count()
        })
        .collect();

    // Re-cache all face normals; extrude reshapes the top + side faces.
    let face_keys_all: Vec<_> = halfedge.mesh.faces.keys().collect();
    for fk in face_keys_all {
        let face = &halfedge.mesh.faces[fk];
        let mut ring_positions = Vec::with_capacity(face.loop_count as usize);
        let mut cur = face.loop_first;
        for _ in 0..face.loop_count {
            let lp = &halfedge.mesh.loops[cur];
            ring_positions.push(halfedge.mesh.verts[lp.vert].co);
            cur = lp.next;
        }
        let new_normal = jackdaw_geometry::newell_normal(&ring_positions);
        halfedge.mesh.faces[fk].normal_cache = new_normal;
    }

    // Flatten HalfedgeMesh -> topology, sync Brush.
    let new_topology = halfedge.mesh.flatten_to_topology();
    let Ok(mut brush) = brushes.get_mut(brush_entity) else {
        return top_face_indices;
    };

    // Grow brush.faces to cover the new side faces. Seed each new slot from
    // the face being extruded (so material + uv_scale + uv_rotation +
    // uv_offset inherit from the parent face), then zero out
    // `uv_u_axis` / `uv_v_axis` so `ensure_uv_axes` derives proper tangents
    // from each wall face's own plane normal. Picking the parent face as
    // template (rather than `start_brush.faces.last()`) keeps the extrude's
    // checker tiling continuous with the original face on the wall sides.
    let new_face_count = new_topology.polygons.len();
    let original_face_count = start_brush.faces.len();
    let extrude_template = modal_state
        .face_indices
        .first()
        .and_then(|&idx| start_brush.faces.get(idx).cloned())
        .or_else(|| start_brush.faces.last().cloned());
    while brush.faces.len() < new_face_count {
        let mut template = extrude_template
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

    // Re-lift HalfedgeMesh from new topology so vert_keys / face_keys are consistent.
    let new_mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let new_vert_keys: Vec<_> = new_mesh.verts.keys().collect();
    let mut new_face_keys = vec![Default::default(); new_mesh.faces.len()];
    for (k, f) in new_mesh.faces.iter() {
        let slot = f.material_idx as usize;
        if slot < new_face_keys.len() {
            new_face_keys[slot] = k;
        }
    }
    halfedge.mesh = new_mesh;
    halfedge.vert_keys = new_vert_keys;
    halfedge.face_keys = new_face_keys;

    top_face_indices
}

/// Project a representative face's world-space centroid and `centroid + normal`
/// through the active camera into window-space pixels; return the normalized
/// pixel-space direction (length 1) corresponding to "+1 unit along the face
/// normal in screen space". Falls back to `(0, -1)` (cursor-up = positive)
/// if anything in the projection pipeline is unavailable, matching the
/// heuristic spelled out in the spec.
fn compute_screen_normal_dir(
    mesh: &HalfedgeMesh,
    face_keys: &[FaceKey],
    brush_xform: Option<&GlobalTransform>,
    camera_query: &Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: &Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
) -> Vec2 {
    const FALLBACK: Vec2 = Vec2::new(0.0, -1.0);

    let Some(&first_fk) = face_keys.first() else {
        return FALLBACK;
    };
    let Some(face) = mesh.faces.get(first_fk) else {
        return FALLBACK;
    };
    let Some(brush_xform) = brush_xform else {
        return FALLBACK;
    };
    let Ok((camera, cam_tf)) = camera_query.single() else {
        return FALLBACK;
    };

    // Walk the face ring once for both centroid (local space) and average
    // ring normal via Newell. We prefer `normal_cache` if it looks set
    // (matches inset's robustness pattern).
    let n = face.loop_count as usize;
    if n < 3 {
        return FALLBACK;
    }
    let mut ring: Vec<Vec3> = Vec::with_capacity(n);
    let mut cur = face.loop_first;
    for _ in 0..n {
        let lp = &mesh.loops[cur];
        ring.push(mesh.verts[lp.vert].co);
        cur = lp.next;
    }
    let centroid_local = ring.iter().copied().sum::<Vec3>() / n as f32;
    let normal_local = if face.normal_cache.length_squared() > 0.5 {
        face.normal_cache
    } else {
        jackdaw_geometry::newell_normal(&ring)
    };

    let centroid_world = brush_xform.transform_point(centroid_local);
    // Direction-only: transform by rotation/scale, NOT translation. Using
    // `transform_point` on `(centroid + normal)` then subtracting works
    // identically and keeps the math obvious.
    let tip_world = brush_xform.transform_point(centroid_local + normal_local);

    let Ok(p0_rt) = camera.world_to_viewport(cam_tf, centroid_world) else {
        return FALLBACK;
    };
    let Ok(p1_rt) = camera.world_to_viewport(cam_tf, tip_world) else {
        return FALLBACK;
    };

    // Render-target coords -> window-space pixels (the same space the cursor
    // lives in). On HiDPI/fractional-scaling viewports these differ; loop_cut
    // uses the identical shape.
    let (p0_win, p1_win) = if let Ok((computed, vp_transform)) = viewport_query.single() {
        let map = ViewportRemap::new(camera, computed, vp_transform);
        (
            map.top_left + p0_rt / map.remap,
            map.top_left + p1_rt / map.remap,
        )
    } else {
        (p0_rt, p1_rt)
    };

    let dir = p1_win - p0_win;
    let len = dir.length();
    if len > 1e-4 { dir / len } else { FALLBACK }
}

pub(crate) fn can_run_extrude(edit_mode: Res<EditMode>, selection: Res<BrushSelection>) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Face) && !selection.faces.is_empty()
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushExtrudeRegionOp>();
    ctx.register_operator::<BrushExtrudeOp>();

    let ext = ctx.id();
    ctx.entity_mut().world_scope(|world| {
        world.spawn((
            Action::<BrushExtrudeOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(KeyCode::KeyE, Press::default())],
        ));
    });
}
