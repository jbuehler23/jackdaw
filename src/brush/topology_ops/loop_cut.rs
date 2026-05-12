//! `brush.mesh.loop_cut` operator.

use bevy::prelude::*;
use bevy::ui::ui_transform::UiGlobalTransform;
use bevy::window::PrimaryWindow;
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;
use jackdaw_geometry::editmesh::ops::loop_cut::loop_cut;
use jackdaw_geometry::editmesh::{EdgeKey, EditMesh, VertKey};
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMesh, BrushEditMode, BrushSelection, EditMode, SetBrush};
use crate::commands::CommandHistory;
use crate::core_extension::CoreExtensionInputContext;
use crate::snapping::SnapSettings;
use crate::viewport::{MainViewportCamera, SceneViewport};
use crate::viewport_util::ViewportRemap;

/// World-space line segments for the loop cut preview gizmo overlay.
/// Each element is a (start, end) pair drawn as a cyan line each frame.
#[derive(Resource, Default)]
pub struct LoopCutPreviewLines {
    pub lines: Vec<(Vec3, Vec3)>,
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushLoopCutOp>();

    let ext = ctx.id();
    ctx.entity_mut().world_scope(|world| {
        world.spawn((
            Action::<BrushLoopCutOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(
                KeyCode::KeyR.with_mod_keys(ModKeys::CONTROL),
                Press::default(),
            )],
        ));
    });
}

/// Modal state for the loop cut operator.
#[derive(Resource, Default)]
pub struct LoopCutModalState {
    pub active: bool,
    pub brush_entity: Option<Entity>,
    pub start_edge_key: Option<EdgeKey>,
    pub current_t: f32,
    pub start_brush: Option<Brush>,
    pub start_editmesh: Option<EditMesh>,
    /// Window-space pixel position of the start edge's canonical v[0].
    pub start_v0_window: Vec2,
    /// Window-space pixel position of the start edge's canonical v[1].
    pub start_v1_window: Vec2,
}

/// Insert a new edge loop across a strip of quad faces. Walks the edge ring
/// from the first selected edge until it hits a non-quad or boundary. The
/// loop position is controlled by mouse drag after Ctrl+R is pressed.
/// LMB commits, Escape or RMB cancels.
///
/// Requires Edge mode with at least one edge selected.
#[operator(
    id = "brush.mesh.loop_cut",
    label = "Loop Cut",
    is_available = can_run_loop_cut,
    modal = true,
    allows_undo = false,
    cancel = cancel_loop_cut,
)]
pub(crate) fn brush_loop_cut(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    mut selection: ResMut<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut bmesh_q: Query<&mut BrushEditMesh>,
    brush_transforms: Query<&GlobalTransform>,
    mut history: ResMut<CommandHistory>,
    mut modal_state: ResMut<LoopCutModalState>,
    mut preview_lines: ResMut<LoopCutPreviewLines>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    snap_settings: Res<SnapSettings>,
    modal_entity: Option<Single<Entity, With<ActiveModalOperator>>>,
) -> OperatorResult {
    // --- Cursor position ---
    // Use raw window-space cursor so dragging outside the viewport panel
    // doesn't cancel the modal (the bounds check in window_to_viewport_cursor
    // returns None when the cursor leaves the UI node, which previously caused
    // the modal to cancel mid-drag).
    let Ok(window) = primary_window.single() else {
        return OperatorResult::Cancelled;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return OperatorResult::Cancelled;
    };
    let Ok((camera, cam_tf)) = camera_query.single() else {
        return OperatorResult::Cancelled;
    };

    // --- First invoke: snapshot and enter modal ---
    if modal_entity.is_none() {
        // Validate preconditions.
        if *edit_mode != EditMode::BrushEdit(BrushEditMode::Edge) {
            return OperatorResult::Cancelled;
        }
        let Some(brush_entity) = selection.entity else {
            return OperatorResult::Cancelled;
        };
        let Some(&(a, b)) = selection.edges.first() else {
            return OperatorResult::Cancelled;
        };

        let Ok(brush_before) = brushes.get(brush_entity).cloned() else {
            return OperatorResult::Cancelled;
        };
        let Ok(bmesh_component) = bmesh_q.get(brush_entity) else {
            return OperatorResult::Cancelled;
        };

        // Resolve cache pair -> EdgeKey.
        let va: VertKey = match bmesh_component.vert_keys.get(a) {
            Some(&k) => k,
            None => return OperatorResult::Cancelled,
        };
        let vb: VertKey = match bmesh_component.vert_keys.get(b) {
            Some(&k) => k,
            None => return OperatorResult::Cancelled,
        };
        let edge_key = match find_edge_between(&bmesh_component.mesh, va, vb) {
            Some(k) => k,
            None => return OperatorResult::Cancelled,
        };

        // Project the start edge's canonical endpoints to window space so each
        // subsequent frame can compute t directly from cursor position.
        // Falls back to (0, 0)/(1, 0) dummy endpoints on projection failure;
        // the modal will still work, just with degenerate tracking.
        let (v0_window, v1_window) = edge_endpoints_window(
            &bmesh_component.mesh,
            edge_key,
            brush_transforms.get(brush_entity).ok(),
            camera,
            cam_tf,
            &viewport_query,
        );

        // Snapshot the EditMesh before any mutation.
        let mesh_snapshot = bmesh_component.mesh.clone();

        modal_state.active = true;
        modal_state.brush_entity = Some(brush_entity);
        modal_state.start_edge_key = Some(edge_key);
        modal_state.current_t = 0.5;
        modal_state.start_brush = Some(brush_before);
        modal_state.start_editmesh = Some(mesh_snapshot);
        modal_state.start_v0_window = v0_window;
        modal_state.start_v1_window = v1_window;

        // Draw the initial preview lines at t=0.5.
        update_preview_lines(&modal_state, &brush_transforms, &mut preview_lines);

        return OperatorResult::Running;
    }

    // --- Subsequent invokes: cancel, update t, preview, or commit ---

    // Cancel on Escape or RMB.
    let escape = keyboard.just_pressed(KeyCode::Escape);
    let rmb = mouse.just_pressed(MouseButton::Right);
    if escape || rmb {
        clear_modal(&mut modal_state, &mut preview_lines);
        return OperatorResult::Cancelled;
    }

    // Project the cursor directly onto the start edge in window space to get t.
    // cursor_pos, start_v0_window, and start_v1_window are all in window space,
    // so no coordinate conversion is needed.
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let edge_vec = modal_state.start_v1_window - modal_state.start_v0_window;
    let edge_len_sq = edge_vec.length_squared();
    let raw_t = if edge_len_sq > 1e-6 {
        let cursor_offset = cursor_pos - modal_state.start_v0_window;
        (cursor_offset.dot(edge_vec) / edge_len_sq).clamp(0.0, 1.0)
    } else {
        0.5
    };
    // Snap respects the global translate_snap toggle; Ctrl flips it.
    modal_state.current_t = if snap_settings.translate_active(ctrl) {
        snap_to_fractions(raw_t)
    } else {
        raw_t
    };

    // Refresh preview lines every frame.
    update_preview_lines(&modal_state, &brush_transforms, &mut preview_lines);

    // Commit on LMB press.
    if mouse.just_pressed(MouseButton::Left) {
        let Some(brush_entity) = modal_state.brush_entity else {
            clear_modal(&mut modal_state, &mut preview_lines);
            return OperatorResult::Cancelled;
        };
        let Some(edge_key) = modal_state.start_edge_key else {
            clear_modal(&mut modal_state, &mut preview_lines);
            return OperatorResult::Cancelled;
        };
        let Some(ref start_brush) = modal_state.start_brush else {
            clear_modal(&mut modal_state, &mut preview_lines);
            return OperatorResult::Cancelled;
        };
        let brush_before = start_brush.clone();
        let t = modal_state.current_t;

        let Ok(mut bmesh_component) = bmesh_q.get_mut(brush_entity) else {
            clear_modal(&mut modal_state, &mut preview_lines);
            return OperatorResult::Cancelled;
        };

        // Restore EditMesh to start snapshot before running the real cut.
        if let Some(ref snap) = modal_state.start_editmesh {
            bmesh_component.mesh = snap.clone();
        }

        // Run the EditMesh op at the chosen t.
        let result = loop_cut(&mut bmesh_component.mesh, edge_key, t);
        let Ok(loop_cut_result) = result else {
            clear_modal(&mut modal_state, &mut preview_lines);
            return OperatorResult::Cancelled;
        };

        // Resolve the topology vertex index for each EdgeKey in the new loop
        // ring so we can write the result into `BrushSelection.edges` after the
        // flatten/re-lift roundtrip. Topology vertex order matches EditMesh
        // slotmap iteration order (see `flatten_to_topology`), and loop_cut
        // never removes verts, so the slotmap position taken now is the same
        // index the post-flatten topology will use.
        let mut new_loop_edge_pairs: Vec<(usize, usize)> =
            Vec::with_capacity(loop_cut_result.new_loop_edges.len());
        {
            let mut vk_to_topo: std::collections::HashMap<VertKey, usize> =
                std::collections::HashMap::with_capacity(bmesh_component.mesh.verts.len());
            for (i, (k, _)) in bmesh_component.mesh.verts.iter().enumerate() {
                vk_to_topo.insert(k, i);
            }
            for ek in &loop_cut_result.new_loop_edges {
                let edge = &bmesh_component.mesh.edges[*ek];
                let Some(&a) = vk_to_topo.get(&edge.v[0]) else {
                    continue;
                };
                let Some(&b) = vk_to_topo.get(&edge.v[1]) else {
                    continue;
                };
                let pair = if a < b { (a, b) } else { (b, a) };
                new_loop_edge_pairs.push(pair);
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

        // Extend brush.faces for any new faces added by the loop cut.
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

        // Re-lift EditMesh from the new topology so vert_keys / face_keys are consistent.
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
            label: "Loop Cut".to_string(),
        }));

        // Chain selection: write the newly created loop ring edges into
        // `BrushSelection.edges` so a follow-up gesture (loop cut again,
        // edge slide, etc.) can operate on the new ring immediately.
        let vert_count = brush.topology.vertices.len();
        let inbounds: Vec<(usize, usize)> = new_loop_edge_pairs
            .into_iter()
            .filter(|(a, b)| *a < vert_count && *b < vert_count)
            .collect();
        if !inbounds.is_empty() {
            selection.edges = inbounds;
        }

        clear_modal(&mut modal_state, &mut preview_lines);
        return OperatorResult::Finished;
    }

    OperatorResult::Running
}

/// Cancel handler: restore the brush to its pre-modal state and clear the preview.
fn cancel_loop_cut(
    mut modal_state: ResMut<LoopCutModalState>,
    mut preview_lines: ResMut<LoopCutPreviewLines>,
    mut brushes: Query<&mut Brush>,
    mut bmesh_q: Query<&mut BrushEditMesh>,
) {
    if let Some(brush_entity) = modal_state.brush_entity
        && let Some(ref start_brush) = modal_state.start_brush
        && let Ok(mut brush) = brushes.get_mut(brush_entity)
    {
        *brush = start_brush.clone();
        // Re-lift EditMesh from the restored topology to keep keys consistent.
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

/// Reset modal state and clear the preview lines.
fn clear_modal(modal_state: &mut LoopCutModalState, preview_lines: &mut LoopCutPreviewLines) {
    *modal_state = LoopCutModalState::default();
    preview_lines.lines.clear();
}

/// Speculatively run loop_cut on a clone of the start EditMesh and write the
/// resulting new-edge world-space endpoints into `LoopCutPreviewLines`.
fn update_preview_lines(
    modal_state: &LoopCutModalState,
    brush_transforms: &Query<&GlobalTransform>,
    preview_lines: &mut LoopCutPreviewLines,
) {
    preview_lines.lines.clear();

    let Some(brush_entity) = modal_state.brush_entity else {
        return;
    };
    let Some(edge_key) = modal_state.start_edge_key else {
        return;
    };
    let Some(ref start_mesh) = modal_state.start_editmesh else {
        return;
    };
    let Ok(brush_xform) = brush_transforms.get(brush_entity) else {
        return;
    };

    let mut speculative = start_mesh.clone();
    let Ok(cut_result) = loop_cut(&mut speculative, edge_key, modal_state.current_t) else {
        return;
    };

    for ek in &cut_result.new_loop_edges {
        let edge = &speculative.edges[*ek];
        let p0 = brush_xform.transform_point(speculative.verts[edge.v[0]].co);
        let p1 = brush_xform.transform_point(speculative.verts[edge.v[1]].co);
        preview_lines.lines.push((p0, p1));
    }
}

/// Project the canonical v[0] and v[1] of `edge_key` to window-space pixels.
///
/// Returns `(v0_window, v1_window)` in the same coordinate system as
/// `window.cursor_position()` so the cursor can be projected directly onto
/// the edge each frame without any further conversion.
/// Falls back to `(Vec2::ZERO, Vec2::X)` on any projection failure.
fn edge_endpoints_window(
    bmesh: &EditMesh,
    edge_key: EdgeKey,
    brush_xform: Option<&GlobalTransform>,
    camera: &Camera,
    cam_tf: &GlobalTransform,
    viewport_query: &Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
) -> (Vec2, Vec2) {
    let Some(brush_xform) = brush_xform else {
        return (Vec2::ZERO, Vec2::X);
    };
    let edge = &bmesh.edges[edge_key];
    let v0_world = brush_xform.transform_point(bmesh.verts[edge.v[0]].co);
    let v1_world = brush_xform.transform_point(bmesh.verts[edge.v[1]].co);
    let Ok(v0_rt) = camera.world_to_viewport(cam_tf, v0_world) else {
        return (Vec2::ZERO, Vec2::X);
    };
    let Ok(v1_rt) = camera.world_to_viewport(cam_tf, v1_world) else {
        return (Vec2::ZERO, Vec2::X);
    };
    // Convert render-target coords to window space.
    // If the viewport query fails, treat remap as identity (render-target == window).
    if let Ok((computed, vp_transform)) = viewport_query.single() {
        let map = ViewportRemap::new(camera, computed, vp_transform);
        let v0_local = v0_rt / map.remap;
        let v1_local = v1_rt / map.remap;
        (map.top_left + v0_local, map.top_left + v1_local)
    } else {
        (v0_rt, v1_rt)
    }
}

fn find_edge_between(bmesh: &EditMesh, va: VertKey, vb: VertKey) -> Option<EdgeKey> {
    bmesh
        .edges
        .iter()
        .find(|(_, e)| (e.v[0] == va && e.v[1] == vb) || (e.v[0] == vb && e.v[1] == va))
        .map(|(k, _)| k)
}

/// Snap `t` to the nearest of {0, 1/4, 1/3, 1/2, 2/3, 3/4, 1}.
fn snap_to_fractions(t: f32) -> f32 {
    const CANDIDATES: [f32; 7] = [0.0, 0.25, 0.333_333, 0.5, 0.666_667, 0.75, 1.0];
    let mut best = CANDIDATES[0];
    let mut best_dist = (t - best).abs();
    for &c in &CANDIDATES[1..] {
        let d = (t - c).abs();
        if d < best_dist {
            best = c;
            best_dist = d;
        }
    }
    best
}

pub(crate) fn can_run_loop_cut(edit_mode: Res<EditMode>, selection: Res<BrushSelection>) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Edge) && !selection.edges.is_empty()
}
