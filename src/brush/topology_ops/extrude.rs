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
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;
use jackdaw_geometry::halfedge::ops::extrude_face_region::extrude_face_region;
use jackdaw_geometry::halfedge::{FaceKey, HalfedgeMesh, apply_topology_edit};
use jackdaw_jsn::Brush;

use super::modal_edit::ModalTopologyEdit;
use crate::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode};
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
    /// `HalfedgeMesh` `FaceKeys` of the faces being extruded. Slotmap keys stay
    /// valid across the snapshot's clones, so they re-resolve each frame after
    /// the snapshot is restored.
    pub face_keys: Vec<FaceKey>,
    /// Brush face indices of the faces being extruded, captured at modal entry.
    /// The live-preview path seeds new wall faces from the first of these so
    /// they inherit the parent face's material and UV projection rather than an
    /// unrelated face's.
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
    /// Pre-edit snapshot driving restore and per-frame re-apply.
    pub edit: Option<ModalTopologyEdit>,
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
) -> OperatorResult {
    if *edit_mode != EditMode::BrushEdit(BrushEditMode::Face) {
        return OperatorResult::Cancelled;
    }
    let brush_entity = selection.active_brush?;
    let sel_faces: Vec<usize> = selection
        .sub(brush_entity)
        .map(|s| s.faces.clone())
        .unwrap_or_default();
    if sel_faces.is_empty() {
        return OperatorResult::Cancelled;
    }

    // Map cache face indices to HalfedgeMesh FaceKeys via face_keys parallel array.
    let mut halfedge = halfedge_q.get_mut(brush_entity)?;
    let mut mesh_faces: Vec<FaceKey> = Vec::with_capacity(sel_faces.len());
    for &face_idx in &sel_faces {
        if let Some(&fk) = halfedge.face_keys.get(face_idx) {
            mesh_faces.push(fk);
        }
    }
    if mesh_faces.is_empty() {
        return OperatorResult::Cancelled;
    }

    // Extrude each selected face and reconcile, capturing each new top face's
    // `material_idx` so the chained selection (below) can resolve its post-
    // flatten index. `extrude_face_region` reuses the input face as the new top
    // cap (so `result.top_face == fk`), and its side quads inherit that
    // `material_idx`; the top face therefore lands at the topology index
    // `count(faces with material_idx < M)` after flatten.
    let brush = brushes.get_mut(brush_entity)?.into_inner();
    let source = brush.faces.last().cloned().unwrap_or_default();
    let original_face_count = brush.faces.len();
    let top_material_idxs: Vec<u32> = apply_topology_edit(
        &mut brush.faces,
        &mut brush.topology,
        &mut halfedge.0,
        |mesh| {
            let mut idxs = Vec::with_capacity(mesh_faces.len());
            for fk in &mesh_faces {
                if let Ok(result) = extrude_face_region(mesh, *fk, DEFAULT_EXTRUDE_DEPTH) {
                    idxs.push(mesh.faces[result.top_face].material_idx);
                }
            }
            idxs
        },
    );

    // New side faces inherit the previous last face's appearance.
    for new_face in original_face_count..brush.faces.len() {
        brush.faces[new_face].copy_appearance_from(&source);
        brush.faces[new_face].ensure_uv_axes();
    }

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
        selection.sub_mut(brush_entity).faces = new_top_indices;
    }

    OperatorResult::Finished
}

pub(crate) fn can_run_extrude_region(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Face)
        && selection.active_sub().is_some_and(|s| !s.faces.is_empty())
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
    mut modal_state: ResMut<ExtrudeModalState>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    modal_inputs: crate::modal_inputs::ModalInputs,
    cursor: crate::viewport::UiCursorPos,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    snap_settings: Res<SnapSettings>,
    modal_entity: Option<Single<Entity, With<ActiveModalOperator>>>,
) -> OperatorResult {
    // Ui-logical cursor; dragging outside the viewport panel should not
    // cancel the modal (matches inset / loop_cut behavior).
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

        let mut face_keys: Vec<FaceKey> = Vec::with_capacity(modal_sel_faces.len());
        for &face_idx in &modal_sel_faces {
            if let Some(&fk) = halfedge.face_keys.get(face_idx) {
                face_keys.push(fk);
            }
        }
        if face_keys.is_empty() {
            return OperatorResult::Cancelled;
        }

        let brush_xform = brush_transforms.get(brush_entity).ok();

        // Derive the screen-space direction corresponding to "+1 world-unit
        // along the representative face normal". Fall back to (0, -1) (cursor
        // up = positive amount) when the camera projection isn't available.
        let screen_normal_dir = compute_screen_normal_dir(
            &halfedge.mesh,
            &face_keys,
            brush_xform,
            &camera_query,
            &viewport_query,
        );

        modal_state.active = true;
        modal_state.brush_entity = Some(brush_entity);
        modal_state.face_keys = face_keys;
        modal_state.face_indices = modal_sel_faces;
        modal_state.start_cursor = cursor_pos;
        modal_state.screen_normal_dir = screen_normal_dir;
        modal_state.current_amount = 0.0;
        modal_state.edit = Some(ModalTopologyEdit::begin(&brush_before, halfedge));

        return OperatorResult::Running;
    }

    // --- Subsequent invokes: cancel, update amount, mutate preview, or commit ---

    let rmb = mouse.just_pressed(MouseButton::Right);
    if modal_inputs.cancel() || rmb {
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
            selection.sub_mut(brush_entity).faces = new_top_indices;
        }

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

/// Reset the live brush and binding to the snapshot captured at modal start.
fn restore_brush_from_snapshot(
    modal_state: &ExtrudeModalState,
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

/// Re-run `extrude_face_region` against the snapshot at the current amount and
/// write the result into the live `Brush` + `BrushHalfedge`. Returns the
/// post-flatten face indices of the new top faces (one per successful
/// extrusion), in `modal_state.face_keys` order, for chained selection.
fn apply_live_extrude(
    modal_state: &mut ExtrudeModalState,
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
    if modal_state.current_amount.abs() < 1e-4 {
        edit.restore(brush, &mut halfedge);
        return Vec::new();
    }

    // Seed new wall faces from the face being extruded so material + UV scale /
    // rotation / offset inherit from the parent, then re-derive each wall's UV
    // axes from its own plane. Preferring the parent over the brush's last face
    // keeps the extrude's checker tiling continuous with the original face.
    let parent = modal_state
        .face_indices
        .first()
        .and_then(|&idx| edit.snapshot_brush().faces.get(idx).cloned())
        .or_else(|| edit.snapshot_brush().faces.last().cloned())
        .unwrap_or_default();
    let original_face_count = edit.snapshot_brush().faces.len();

    // Restore to the snapshot, re-run the extrude at the current amount, and
    // reconcile. The closure resolves each new top face's post-flatten index
    // while the mesh still holds the original material_idx values.
    // `extrude_face_region` reuses the input face as the new top cap (so
    // `result.top_face == fk`); its side quads share that `material_idx`, so the
    // top lands at the topology index `count(faces with material_idx < M)`.
    let amount = modal_state.current_amount;
    let face_keys = modal_state.face_keys.clone();
    let top_face_indices = edit.apply(brush, &mut halfedge, |mesh| {
        let mut top_material_idxs = Vec::with_capacity(face_keys.len());
        for &fk in &face_keys {
            if let Ok(result) = extrude_face_region(mesh, fk, amount) {
                top_material_idxs.push(mesh.faces[result.top_face].material_idx);
            }
        }
        top_material_idxs
            .iter()
            .map(|&mtx| mesh.faces.values().filter(|f| f.material_idx < mtx).count())
            .collect::<Vec<usize>>()
    });

    // Each new wall face inherits the parent's appearance with freshly derived
    // UV axes.
    for new_face in original_face_count..brush.faces.len() {
        brush.faces[new_face].copy_appearance_from(&parent);
        brush.faces[new_face].uv_u_axis = Vec3::ZERO;
        brush.faces[new_face].uv_v_axis = Vec3::ZERO;
        brush.faces[new_face].ensure_uv_axes();
    }

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
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Face)
        && selection.active_sub().is_some_and(|s| !s.faces.is_empty())
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushExtrudeRegionOp>();
    ctx.register_operator::<BrushExtrudeOp>();

    ctx.bind_operator::<CoreExtensionInputContext, BrushExtrudeOp>([PresetInput::key("KeyE")]);
}
