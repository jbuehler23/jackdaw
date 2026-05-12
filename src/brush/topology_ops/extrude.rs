//! Extrude operators:
//!
//! - `brush.mesh.extrude_region` (non-modal): one-shot extrusion at a fixed
//!   depth, available via menu / command palette.
//! - `brush.mesh.extrude` (modal, bound to `E`): Blender-style modal where the
//!   cursor's projected motion along the face normal drives a signed extrusion
//!   amount. Cyan preview lines show the proposed top ring + wall edges.
//!
//! Both share the same EditMesh op (`extrude_face_region`) and the same
//! chained selection behavior: post-commit, `BrushSelection.faces` is updated
//! to the new top face indices.

use bevy::prelude::*;
use bevy::ui::ui_transform::UiGlobalTransform;
use bevy::window::PrimaryWindow;
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;
use jackdaw_geometry::editmesh::ops::extrude_face_region::extrude_face_region;
use jackdaw_geometry::editmesh::{EditMesh, FaceKey};
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMesh, BrushEditMode, BrushSelection, EditMode, SetBrush};
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

/// World-space line segments for the modal extrude preview gizmo.
/// Each element is a (start, end) pair drawn as a cyan line each frame.
#[derive(Resource, Default)]
pub struct ExtrudePreviewLines {
    pub lines: Vec<(Vec3, Vec3)>,
}

/// Modal state for the `brush.mesh.extrude` operator.
#[derive(Resource, Default)]
pub struct ExtrudeModalState {
    pub active: bool,
    pub brush_entity: Option<Entity>,
    /// EditMesh FaceKeys of the faces being extruded.
    pub face_keys: Vec<FaceKey>,
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
    pub start_editmesh: Option<EditMesh>,
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
    mut bmesh_q: Query<&mut BrushEditMesh>,
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

    // Map cache face indices to EditMesh FaceKeys via face_keys parallel array.
    let Ok(mut bmesh_component) = bmesh_q.get_mut(brush_entity) else {
        return OperatorResult::Cancelled;
    };
    let mut bmesh_faces: Vec<FaceKey> = Vec::with_capacity(selection.faces.len());
    for &face_idx in &selection.faces {
        if let Some(&fk) = bmesh_component.face_keys.get(face_idx) {
            bmesh_faces.push(fk);
        }
    }
    if bmesh_faces.is_empty() {
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
    let mut top_material_idxs: Vec<u32> = Vec::with_capacity(bmesh_faces.len());
    for fk in bmesh_faces {
        if let Ok(result) =
            extrude_face_region(&mut bmesh_component.mesh, fk, DEFAULT_EXTRUDE_DEPTH)
        {
            let mtx = bmesh_component.mesh.faces[result.top_face].material_idx;
            top_material_idxs.push(mtx);
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

    // Flatten EditMesh -> topology, sync Brush.faces[i].plane + Brush.topology.
    let new_topology = bmesh_component.mesh.flatten_to_topology();
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
            bmesh_component
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
/// values push outward; negative values pull inward. Ctrl snaps to the
/// translate grid increment. LMB commits; Esc / RMB cancels.
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
    mut bmesh_q: Query<&mut BrushEditMesh>,
    brush_transforms: Query<&GlobalTransform>,
    mut history: ResMut<CommandHistory>,
    mut modal_state: ResMut<ExtrudeModalState>,
    mut preview_lines: ResMut<ExtrudePreviewLines>,
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
        let Ok(bmesh_component) = bmesh_q.get(brush_entity) else {
            return OperatorResult::Cancelled;
        };

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
        modal_state.start_cursor = cursor_pos;
        modal_state.screen_normal_dir = screen_normal_dir;
        modal_state.current_amount = 0.0;
        modal_state.start_brush = Some(brush_before);
        modal_state.start_editmesh = Some(mesh_snapshot);

        update_extrude_preview_lines(&modal_state, &brush_transforms, &mut preview_lines);
        return OperatorResult::Running;
    }

    // --- Subsequent invokes: cancel, update amount, preview, or commit ---

    let escape = keyboard.just_pressed(KeyCode::Escape);
    let rmb = mouse.just_pressed(MouseButton::Right);
    if escape || rmb {
        clear_modal(&mut modal_state, &mut preview_lines);
        return OperatorResult::Cancelled;
    }

    // Signed projection of cursor motion onto the screen-normal direction.
    let cursor_delta = cursor_pos - modal_state.start_cursor;
    let raw_amount = cursor_delta.dot(modal_state.screen_normal_dir) * EXTRUDE_SENSITIVITY;

    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    modal_state.current_amount = if snap_settings.translate_active(ctrl) {
        let inc = snap_settings.translate_increment;
        if inc > 0.0 {
            (raw_amount / inc).round() * inc
        } else {
            raw_amount
        }
    } else {
        raw_amount
    };

    update_extrude_preview_lines(&modal_state, &brush_transforms, &mut preview_lines);

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

        // Degenerate zero-amount commit: treat as no-op cancel so we don't
        // record a useless undo entry or split the face ring trivially.
        if amount.abs() < 1e-4 {
            clear_modal(&mut modal_state, &mut preview_lines);
            return OperatorResult::Cancelled;
        }

        let Ok(mut bmesh_component) = bmesh_q.get_mut(brush_entity) else {
            clear_modal(&mut modal_state, &mut preview_lines);
            return OperatorResult::Cancelled;
        };

        // Restore to snapshot before committing so we start from clean state.
        if let Some(ref snap) = modal_state.start_editmesh {
            bmesh_component.mesh = snap.clone();
        }

        // Run extrude on each selected face; capture each successful op's
        // top-face `material_idx` for the post-flatten selection update.
        // `extrude_face_region` reuses the input face as the new top cap
        // (i.e. `result.top_face == fk`), preserving its `material_idx`.
        // The N side quads it adds inherit the same `material_idx`. Among
        // ties on a given M the original (now top) face sits first in
        // slotmap iteration order, so the top face lands at the topology
        // index equal to `count(faces with material_idx < M)` after flatten.
        let mut top_material_idxs: Vec<u32> = Vec::with_capacity(face_keys.len());
        for fk in face_keys {
            if let Ok(result) = extrude_face_region(&mut bmesh_component.mesh, fk, amount) {
                let mtx = bmesh_component.mesh.faces[result.top_face].material_idx;
                top_material_idxs.push(mtx);
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

        // Flatten EditMesh -> topology, sync Brush.
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
                let v0_idx = new_topology.loops[new_topology.polygons[face_idx].loop_start as usize]
                    .vert as usize;
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
            label: "Extrude".to_string(),
        }));

        // Chain selection: write the new top face indices into
        // `BrushSelection.faces` so a follow-up gesture (drag-along-normal,
        // inset, extrude again) can act on them immediately.
        let face_count = brush.faces.len();
        let new_top_indices: Vec<usize> = top_material_idxs
            .into_iter()
            .map(|mtx| {
                bmesh_component
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

        clear_modal(&mut modal_state, &mut preview_lines);
        return OperatorResult::Finished;
    }

    OperatorResult::Running
}

/// Cancel handler: restore the brush to its pre-modal state and clear preview.
fn cancel_extrude(
    mut modal_state: ResMut<ExtrudeModalState>,
    mut preview_lines: ResMut<ExtrudePreviewLines>,
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

fn clear_modal(modal_state: &mut ExtrudeModalState, preview_lines: &mut ExtrudePreviewLines) {
    *modal_state = ExtrudeModalState::default();
    preview_lines.lines.clear();
}

/// Speculatively run `extrude_face_region` on each face at `current_amount`
/// and collect line segments for the cyan preview gizmo. Drawn segments:
/// the new top ring (closed loop per face) plus the wall edges connecting
/// each old ring vert to its new top vert.
fn update_extrude_preview_lines(
    modal_state: &ExtrudeModalState,
    brush_transforms: &Query<&GlobalTransform>,
    preview_lines: &mut ExtrudePreviewLines,
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
        let Ok(result) = extrude_face_region(&mut speculative, fk, modal_state.current_amount)
        else {
            continue;
        };

        // Top ring: walk the loops of the (re-keyed) top face. The op
        // rewrites `face`'s ring to the new top ring, so reading
        // `result.top_face`'s loops gives the new closed loop.
        let top_face_data = &speculative.faces[result.top_face];
        let n = top_face_data.loop_count as usize;
        let mut top_verts: Vec<Vec3> = Vec::with_capacity(n);
        {
            let mut cur = top_face_data.loop_first;
            for _ in 0..n {
                let lp = &speculative.loops[cur];
                top_verts.push(brush_xform.transform_point(speculative.verts[lp.vert].co));
                cur = lp.next;
            }
        }
        for i in 0..n {
            preview_lines
                .lines
                .push((top_verts[i], top_verts[(i + 1) % n]));
        }

        // Wall edges: each side quad's `L3` loop walks `new[i] -> old[i]`,
        // i.e. the wall edge between the new top vert and the corresponding
        // old ring vert. See `extrude_face_region.rs` step 6 for the loop
        // wiring (L0..L3 = old[i], old[i+1], new[i+1], new[i]).
        for &sf in &result.side_faces {
            let side = &speculative.faces[sf];
            let l0 = side.loop_first;
            let l3 = speculative.loops[l0].prev;
            let old_v =
                brush_xform.transform_point(speculative.verts[speculative.loops[l0].vert].co);
            let new_v =
                brush_xform.transform_point(speculative.verts[speculative.loops[l3].vert].co);
            preview_lines.lines.push((old_v, new_v));
        }
    }
}

/// Project a representative face's world-space centroid and `centroid + normal`
/// through the active camera into window-space pixels; return the normalized
/// pixel-space direction (length 1) corresponding to "+1 unit along the face
/// normal in screen space". Falls back to `(0, -1)` (cursor-up = positive)
/// if anything in the projection pipeline is unavailable, matching the
/// heuristic spelled out in the spec.
fn compute_screen_normal_dir(
    mesh: &EditMesh,
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
