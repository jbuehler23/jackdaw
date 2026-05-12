//! `brush.mesh.inset` operator — Blender-style modal.
//!
//! Press `I` in Face mode. The inset amount is controlled by mouse displacement
//! magnitude: drag any direction to grow the inset proportionally. Cyan preview
//! lines show the proposed inner ring and spoke edges. Ctrl snaps to the translate
//! grid increment. LMB commits; Esc / RMB cancels.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;
use jackdaw_geometry::editmesh::{EditMesh, FaceKey};
use jackdaw_geometry::editmesh::ops::inset_face::inset_face;
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMesh, BrushEditMode, BrushSelection, EditMode, SetBrush};
use crate::commands::CommandHistory;
use crate::core_extension::CoreExtensionInputContext;
use crate::snapping::SnapSettings;

/// Pixels-per-world-unit sensitivity for the inset modal.
/// At this value 100 pixels of cursor movement corresponds to 1 world-unit of inset.
/// Tune as needed.
const INSET_SENSITIVITY: f32 = 0.01;

/// World-space line segments for the inset preview gizmo.
/// Each element is a (start, end) pair drawn as a cyan line each frame.
#[derive(Resource, Default)]
pub struct InsetPreviewLines {
    pub lines: Vec<(Vec3, Vec3)>,
}

/// Modal state for the inset operator.
#[derive(Resource, Default)]
pub struct InsetModalState {
    pub active: bool,
    pub brush_entity: Option<Entity>,
    /// EditMesh FaceKeys of the faces being inset.
    pub face_keys: Vec<FaceKey>,
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
/// to the translate grid increment. LMB commits; Esc / RMB cancels.
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
    brush_transforms: Query<&GlobalTransform>,
    mut history: ResMut<CommandHistory>,
    mut modal_state: ResMut<InsetModalState>,
    mut preview_lines: ResMut<InsetPreviewLines>,
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
        modal_state.start_cursor = cursor_pos;
        modal_state.current_amount = 0.0;
        modal_state.start_brush = Some(brush_before);
        modal_state.start_editmesh = Some(mesh_snapshot);
        modal_state.max_inset = max_inset;

        update_preview_lines(&modal_state, &brush_transforms, &mut preview_lines);
        return OperatorResult::Running;
    }

    // --- Subsequent invokes: cancel, update amount, preview, or commit ---

    let escape = keyboard.just_pressed(KeyCode::Escape);
    let rmb = mouse.just_pressed(MouseButton::Right);
    if escape || rmb {
        clear_modal(&mut modal_state, &mut preview_lines);
        return OperatorResult::Cancelled;
    }

    // Compute raw amount from total mouse displacement magnitude.
    // Any movement in any direction grows the inset proportionally.
    let delta = cursor_pos - modal_state.start_cursor;
    let raw_amount = delta.length() * INSET_SENSITIVITY;

    // Clamp to maximum valid inset to prevent inner ring inversion.
    let clamped_amount = raw_amount.min(modal_state.max_inset);

    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    modal_state.current_amount = if snap_settings.translate_active(ctrl) {
        let inc = snap_settings.translate_increment;
        if inc > 0.0 {
            (clamped_amount / inc).round() * inc
        } else {
            clamped_amount
        }
    } else {
        clamped_amount
    };

    update_preview_lines(&modal_state, &brush_transforms, &mut preview_lines);

    // Commit on LMB.
    if mouse.just_pressed(MouseButton::Left) {
        let Some(brush_entity) = modal_state.brush_entity else {
            clear_modal(&mut modal_state, &mut preview_lines);
            return OperatorResult::Cancelled;
        };
        let Some(ref start_brush) = modal_state.start_brush else {
            clear_modal(&mut modal_state, &mut preview_lines);
            return OperatorResult::Cancelled;
        };
        let brush_before = start_brush.clone();
        let amount = modal_state.current_amount;
        let face_keys = modal_state.face_keys.clone();

        let Ok(mut bmesh_component) = bmesh_q.get_mut(brush_entity) else {
            clear_modal(&mut modal_state, &mut preview_lines);
            return OperatorResult::Cancelled;
        };

        // Restore to snapshot before committing so we start from clean state.
        if let Some(ref snap) = modal_state.start_editmesh {
            bmesh_component.mesh = snap.clone();
        }

        // Run inset on each selected face; skip degenerate faces silently.
        // Capture each successful inset's inner-face material_idx so we can
        // resolve the post-flatten face index for the new inner ring (see
        // `compute_new_inner_face_indices` below).
        let mut inner_material_idxs: Vec<u32> = Vec::with_capacity(face_keys.len());
        for fk in face_keys {
            if let Ok(result) = inset_face(&mut bmesh_component.mesh, fk, amount) {
                // `inset_face` rewrites the input face's ring as the inner
                // ring (i.e. `result.inner_face == fk`), preserving its
                // `material_idx`. That value is what we look up post-flatten.
                let mtx = bmesh_component.mesh.faces[result.inner_face].material_idx;
                inner_material_idxs.push(mtx);
            }
        }

        // Re-cache all face normals.
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

        // Resolve the post-flatten face index for each new inner face.
        // `flatten_to_topology` uses a stable sort by `material_idx`, and
        // every face here inserted by `inset_face` (the N side-quads) was
        // added to the SlotMap AFTER the original face. SlotMap iteration
        // order is slot order, which is insertion order absent removals,
        // and `inset_face` never removes faces. Therefore, among ties on
        // a given material_idx M, the original (now inner) face sits first
        // in the iteration order, so the inner face lands at the topology
        // index equal to `count(faces with material_idx < M)`.
        let mut new_inner_face_indices: Vec<usize> = Vec::with_capacity(inner_material_idxs.len());
        for &mtx in &inner_material_idxs {
            let idx = bmesh_component
                .mesh
                .faces
                .values()
                .filter(|f| f.material_idx < mtx)
                .count();
            new_inner_face_indices.push(idx);
        }

        // Flatten EditMesh → topology, sync Brush.
        let new_topology = bmesh_component.mesh.flatten_to_topology();
        let Ok(mut brush) = brushes.get_mut(brush_entity) else {
            clear_modal(&mut modal_state, &mut preview_lines);
            return OperatorResult::Cancelled;
        };

        let new_face_count = new_topology.polygons.len();
        while brush.faces.len() < new_face_count {
            let template = brush.faces.last().cloned().unwrap_or_default();
            brush.faces.push(template);
        }

        let positions: Vec<Vec3> = new_topology.vertices.iter().map(|v| v.position).collect();
        for (face_idx, face_data) in brush.faces.iter_mut().enumerate() {
            if face_idx < new_topology.polygons.len() {
                let normal = new_topology.face_normal_with(&positions, face_idx);
                let v0_idx =
                    new_topology.loops[new_topology.polygons[face_idx].loop_start as usize].vert
                        as usize;
                let distance = positions[v0_idx].dot(normal);
                face_data.plane.normal = normal;
                face_data.plane.distance = distance;
            }
        }
        brush.topology = new_topology;

        // Re-lift EditMesh from new topology so vert_keys / face_keys are consistent.
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

        history.push_executed(Box::new(SetBrush {
            entity: brush_entity,
            old: brush_before,
            new: brush.clone(),
            label: "Inset".to_string(),
        }));

        // Update the brush selection to the newly created inner ring
        // face(s) so a follow-up gesture (notably a drag along the
        // face normal, which triggers off any face in
        // `BrushSelection.faces`) can extrude immediately without
        // another hotkey press. Filter out indices that landed past
        // the brush face array; in practice every entry should be in
        // range, but a defensive clamp avoids panicking the operator
        // if any future op change perturbs the index math.
        let face_count = brush.faces.len();
        let inner_indices: Vec<usize> = new_inner_face_indices
            .into_iter()
            .filter(|&i| i < face_count)
            .collect();

        if !inner_indices.is_empty() {
            selection.faces = inner_indices;
        }

        clear_modal(&mut modal_state, &mut preview_lines);
        return OperatorResult::Finished;
    }

    OperatorResult::Running
}

/// Cancel handler: restore the brush to its pre-modal state and clear the preview.
fn cancel_inset(
    mut modal_state: ResMut<InsetModalState>,
    mut preview_lines: ResMut<InsetPreviewLines>,
    mut brushes: Query<&mut Brush>,
    mut bmesh_q: Query<&mut BrushEditMesh>,
) {
    if let Some(brush_entity) = modal_state.brush_entity
        && let Some(ref start_brush) = modal_state.start_brush
        && let Ok(mut brush) = brushes.get_mut(brush_entity)
    {
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
    clear_modal(&mut modal_state, &mut preview_lines);
}

/// Reset modal state and clear preview lines.
fn clear_modal(modal_state: &mut InsetModalState, preview_lines: &mut InsetPreviewLines) {
    *modal_state = InsetModalState::default();
    preview_lines.lines.clear();
}

/// Speculatively run `inset_face` on each face at `current_amount` and write
/// the resulting new inner-ring and wall (spoke) edges into `InsetPreviewLines`.
fn update_preview_lines(
    modal_state: &InsetModalState,
    brush_transforms: &Query<&GlobalTransform>,
    preview_lines: &mut InsetPreviewLines,
) {
    preview_lines.lines.clear();

    let Some(brush_entity) = modal_state.brush_entity else {
        return;
    };
    let Some(ref start_mesh) = modal_state.start_editmesh else {
        return;
    };
    let Ok(brush_xform) = brush_transforms.get(brush_entity) else {
        return;
    };

    let mut speculative = start_mesh.clone();
    for &fk in &modal_state.face_keys {
        let Ok(result) = inset_face(&mut speculative, fk, modal_state.current_amount) else {
            continue;
        };

        // Inner ring: walk the loops of the inner face after inset.
        // `result.inner_face` is the face key of the shrunken inner face.
        let inner_face_data = &speculative.faces[result.inner_face];
        let n = inner_face_data.loop_count as usize;
        let mut inner_verts: Vec<Vec3> = Vec::with_capacity(n);
        {
            let mut cur = inner_face_data.loop_first;
            for _ in 0..n {
                let lp = &speculative.loops[cur];
                inner_verts.push(brush_xform.transform_point(speculative.verts[lp.vert].co));
                cur = lp.next;
            }
        }
        for i in 0..n {
            let a = inner_verts[i];
            let b = inner_verts[(i + 1) % n];
            preview_lines.lines.push((a, b));
        }

        // Spoke edges: the new inner verts vs the corresponding old outer verts.
        // Walk each side quad's first loop (which is the reused old-ring loop)
        // and the last loop (which starts at the corresponding new vert).
        for &sf in &result.side_faces {
            let side = &speculative.faces[sf];
            // Side quad loops: old[i], old[i+1], new[i+1], new[i] (L0..L3).
            // L3 is the spoke connecting new[i] → old[i].
            let l0 = side.loop_first;
            let l3 = speculative.loops[l0].prev;
            let old_v = brush_xform
                .transform_point(speculative.verts[speculative.loops[l0].vert].co);
            let new_v = brush_xform
                .transform_point(speculative.verts[speculative.loops[l3].vert].co);
            preview_lines.lines.push((old_v, new_v));
        }
    }
}

pub(crate) fn can_run_inset(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Face) && !selection.faces.is_empty()
}
