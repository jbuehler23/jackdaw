//! `brush.mesh.edge_slide_modal` operator -- Blender-style modal edge slide.
//!
//! Press `Shift+E` in Edge mode with at least one edge selected. Cursor motion
//! projected onto the screen-space slide direction drives a signed factor in
//! `[-1, +1]`: 0 = no slide, +1 = collapse toward one neighbor loop, -1 = the
//! other. Cyan preview lines show the proposed slid edge positions. Ctrl snaps
//! to 0.25 increments. LMB commits; Esc / RMB cancels.
//!
//! Coexists with the non-modal `brush.mesh.edge_slide` operator (which slides
//! by a fixed amount) -- this is a separate entry point.

use bevy::prelude::*;
use bevy::ui::ui_transform::UiGlobalTransform;
use bevy::window::PrimaryWindow;
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;
use jackdaw_geometry::editmesh::cycles::radial_walk;
use jackdaw_geometry::editmesh::ops::edge_slide::edge_slide;
use jackdaw_geometry::editmesh::{EdgeKey, EditMesh, VertKey};
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMesh, BrushEditMode, BrushSelection, EditMode, SetBrush};
use crate::commands::CommandHistory;
use crate::core_extension::CoreExtensionInputContext;
use crate::snapping::SnapSettings;
use crate::viewport::{MainViewportCamera, SceneViewport};
use crate::viewport_util::ViewportRemap;

/// World-space line segments for the modal edge slide preview gizmo.
/// Each element is a (start, end) pair drawn as a cyan line each frame.
#[derive(Resource, Default)]
pub struct EdgeSlidePreviewLines {
    pub lines: Vec<(Vec3, Vec3)>,
}

/// Per-side projection info for cursor-tracks-edge mapping. Sides correspond
/// to the edge's two adjacent quad faces; `pos` is the face whose direction
/// the underlying op uses for positive `t`, `neg` is the other face.
#[derive(Default, Clone, Copy)]
pub struct SlideSideInfo {
    /// Unit screen-space direction from the representative end vertex toward
    /// the parallel-edge neighbor on this side.
    pub dir_unit: Vec2,
    /// Length of that screen-space segment in pixels (factor = `cursor_proj
    /// / len_window` puts 1 pixel of cursor = 1 pixel of edge motion).
    pub len_window: f32,
    /// World-space length of the corresponding parallel edge. Used to convert
    /// `factor` into a world distance for grid snapping.
    pub len_world: f32,
}

/// Modal state for the `brush.mesh.edge_slide_modal` operator.
#[derive(Resource, Default)]
pub struct EdgeSlideModalState {
    pub active: bool,
    pub brush_entity: Option<Entity>,
    /// EditMesh EdgeKeys of the edges being slid.
    pub edge_keys: Vec<EdgeKey>,
    /// Window-space cursor position at the moment the modal started.
    pub start_cursor: Vec2,
    /// Cursor-tracks-edge info for the positive side (face[0] in radial cycle).
    pub pos_side: Option<SlideSideInfo>,
    /// Cursor-tracks-edge info for the negative side (face[1] in radial cycle).
    /// `None` for boundary edges with only one adjacent quad face.
    pub neg_side: Option<SlideSideInfo>,
    /// Current factor in `[-1, +1]`. Sign flips slide side; 0 is no slide.
    pub current_factor: f32,
    pub start_brush: Option<Brush>,
    pub start_editmesh: Option<EditMesh>,
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushEdgeSlideModalOp>();

    let ext = ctx.id();
    ctx.entity_mut().world_scope(|world| {
        world.spawn((
            Action::<BrushEdgeSlideModalOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(
                KeyCode::KeyE.with_mod_keys(ModKeys::SHIFT),
                Press::default(),
            )],
        ));
    });
}

/// Slide each selected edge along its parallel-edge direction in adjacent quad
/// faces, controlled by cursor motion projected onto the screen-space slide
/// direction. Factor is normalized to `[-1, +1]`; Ctrl snaps to 0.25 increments.
/// LMB commits; Esc / RMB cancels.
///
/// Requires Edge mode with at least one edge selected.
#[operator(
    id = "brush.mesh.edge_slide_modal",
    label = "Edge Slide (Modal)",
    is_available = can_run_edge_slide_modal,
    modal = true,
    allows_undo = false,
    cancel = cancel_edge_slide,
)]
pub(crate) fn brush_edge_slide_modal(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut bmesh_q: Query<&mut BrushEditMesh>,
    brush_transforms: Query<&GlobalTransform>,
    mut history: ResMut<CommandHistory>,
    mut modal_state: ResMut<EdgeSlideModalState>,
    mut preview_lines: ResMut<EdgeSlidePreviewLines>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    snap_settings: Res<SnapSettings>,
    modal_entity: Option<Single<Entity, With<ActiveModalOperator>>>,
) -> OperatorResult {
    // Raw window-space cursor; dragging outside the viewport panel should not
    // cancel the modal (matches inset / extrude / loop_cut behavior).
    let Ok(window) = primary_window.single() else {
        return OperatorResult::Cancelled;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return OperatorResult::Cancelled;
    };

    // --- First invoke: snapshot and enter modal ---
    if modal_entity.is_none() {
        if *edit_mode != EditMode::BrushEdit(BrushEditMode::Edge) {
            return OperatorResult::Cancelled;
        }
        let Some(brush_entity) = selection.entity else {
            return OperatorResult::Cancelled;
        };
        if selection.edges.is_empty() {
            return OperatorResult::Cancelled;
        }

        let Ok(brush_before) = brushes.get(brush_entity).cloned() else {
            return OperatorResult::Cancelled;
        };
        let Ok(bmesh_component) = bmesh_q.get(brush_entity) else {
            return OperatorResult::Cancelled;
        };

        // Map cache edge pairs to EditMesh EdgeKeys via vert_keys.
        let mut edge_keys: Vec<EdgeKey> = Vec::with_capacity(selection.edges.len());
        for &(a, b) in &selection.edges {
            let Some(&va) = bmesh_component.vert_keys.get(a) else {
                continue;
            };
            let Some(&vb) = bmesh_component.vert_keys.get(b) else {
                continue;
            };
            if let Some(ek) = find_edge_between(&bmesh_component.mesh, va, vb) {
                edge_keys.push(ek);
            }
        }
        if edge_keys.is_empty() {
            return OperatorResult::Cancelled;
        }

        let mesh_snapshot = bmesh_component.mesh.clone();
        let brush_xform = brush_transforms.get(brush_entity).ok();

        // Compute cursor-tracks-edge projection info for both adjacent faces of
        // the first selected edge. Each side carries its own world + screen
        // lengths so that 1 pixel of cursor = 1 pixel of edge movement on that
        // side, and grid snap can convert factor back to a world distance.
        let (pos_side, neg_side) = compute_slide_sides(
            &mesh_snapshot,
            edge_keys[0],
            brush_xform,
            &camera_query,
            &viewport_query,
        );

        modal_state.active = true;
        modal_state.brush_entity = Some(brush_entity);
        modal_state.edge_keys = edge_keys;
        modal_state.start_cursor = cursor_pos;
        modal_state.pos_side = pos_side;
        modal_state.neg_side = neg_side;
        modal_state.current_factor = 0.0;
        modal_state.start_brush = Some(brush_before);
        modal_state.start_editmesh = Some(mesh_snapshot);

        update_preview_lines(&modal_state, &brush_transforms, &mut preview_lines);
        return OperatorResult::Running;
    }

    // --- Subsequent invokes: cancel, update factor, preview, or commit ---

    let escape = keyboard.just_pressed(KeyCode::Escape);
    let rmb = mouse.just_pressed(MouseButton::Right);
    if escape || rmb {
        clear_modal(&mut modal_state, &mut preview_lines);
        return OperatorResult::Cancelled;
    }

    // Cursor-tracks-edge: project cursor delta onto each adjacent face's screen
    // slide direction; the side with the larger positive projection wins, and
    // factor magnitude = projection_pixels / side_screen_length. This makes 1
    // pixel of cursor motion = 1 pixel of edge motion on the chosen side.
    let cursor_delta = cursor_pos - modal_state.start_cursor;

    let pos_proj_px = modal_state
        .pos_side
        .map(|s| cursor_delta.dot(s.dir_unit))
        .unwrap_or(f32::NEG_INFINITY);
    let neg_proj_px = modal_state
        .neg_side
        .map(|s| cursor_delta.dot(s.dir_unit))
        .unwrap_or(f32::NEG_INFINITY);

    let (factor, world_len) = if pos_proj_px >= neg_proj_px && pos_proj_px > 0.0 {
        let s = modal_state.pos_side.unwrap();
        let f = if s.len_window > 1e-4 {
            (pos_proj_px / s.len_window).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (f, s.len_world)
    } else if neg_proj_px > 0.0 {
        let s = modal_state.neg_side.unwrap();
        let f = if s.len_window > 1e-4 {
            (neg_proj_px / s.len_window).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (-f, s.len_world)
    } else {
        (0.0, 0.0)
    };

    // Snap respects the global translate_snap toggle; Ctrl flips it.
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    modal_state.current_factor = if snap_settings.translate_active(ctrl)
        && snap_settings.translate_increment > 0.0
        && world_len > 1e-6
    {
        // Snap world-space displacement to translate_increment, then back-
        // convert to a factor. Preserves direction sign.
        let inc = snap_settings.translate_increment;
        let world_disp = factor.abs() * world_len;
        let snapped_world = (world_disp / inc).round() * inc;
        let snapped_factor = (snapped_world / world_len).clamp(0.0, 1.0);
        if factor < 0.0 {
            -snapped_factor
        } else {
            snapped_factor
        }
    } else {
        factor
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
        let factor = modal_state.current_factor;
        let edge_keys = modal_state.edge_keys.clone();

        // Degenerate zero-factor commit: treat as no-op cancel so we don't
        // record a useless undo entry.
        if factor.abs() < 1e-4 {
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

        // Run edge_slide on the full edge set in one call. The op is a pure
        // transform: it moves vertex positions, never adding or removing
        // edges/faces, so edge identity (and the cache pair (a, b)) survives.
        if edge_slide(&mut bmesh_component.mesh, &edge_keys, factor).is_err() {
            clear_modal(&mut modal_state, &mut preview_lines);
            return OperatorResult::Cancelled;
        }

        // Re-cache all face normals (slid verts can rotate face planes).
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

        // Edge slide does not add new faces; no need to grow brush.faces.
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
            label: "Edge Slide".to_string(),
        }));

        // selection.edges intentionally untouched: edge_slide preserves edge
        // identity (no add/remove), and vertex indices are preserved across
        // flatten/re-lift since slotmap iteration order is stable when no
        // verts are added or removed.

        clear_modal(&mut modal_state, &mut preview_lines);
        return OperatorResult::Finished;
    }

    OperatorResult::Running
}

/// Cancel handler: restore the brush to its pre-modal state and clear preview.
fn cancel_edge_slide(
    mut modal_state: ResMut<EdgeSlideModalState>,
    mut preview_lines: ResMut<EdgeSlidePreviewLines>,
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

fn clear_modal(modal_state: &mut EdgeSlideModalState, preview_lines: &mut EdgeSlidePreviewLines) {
    *modal_state = EdgeSlideModalState::default();
    preview_lines.lines.clear();
}

/// Speculatively run `edge_slide` on a clone of the start EditMesh at
/// `current_factor` and collect line segments for the cyan preview gizmo.
/// Drawn segments: each slid edge in its preview position.
fn update_preview_lines(
    modal_state: &EdgeSlideModalState,
    brush_transforms: &Query<&GlobalTransform>,
    preview_lines: &mut EdgeSlidePreviewLines,
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
    if edge_slide(
        &mut speculative,
        &modal_state.edge_keys,
        modal_state.current_factor,
    )
    .is_err()
    {
        return;
    }

    for &ek in &modal_state.edge_keys {
        let edge = &speculative.edges[ek];
        let p0 = brush_xform.transform_point(speculative.verts[edge.v[0]].co);
        let p1 = brush_xform.transform_point(speculative.verts[edge.v[1]].co);
        preview_lines.lines.push((p0, p1));
    }
}

/// Compute cursor-tracks-edge projection info for both adjacent quad faces of
/// `edge_key`. The first quad in the edge's radial cycle is the "positive"
/// side (matches the underlying op's `t >= 0` choice); the second quad is the
/// "negative" side. Boundary edges with only one quad face return `(Some, None)`.
///
/// For each side: take that face's loop on the edge, walk `lp.next.edge` to
/// get the parallel slide-along edge for the loop's END vertex. The screen
/// direction from that vertex to the parallel edge's other endpoint, plus the
/// pixel length of that segment and the world-space length of the parallel
/// edge, is the projection info.
fn compute_slide_sides(
    mesh: &EditMesh,
    edge_key: EdgeKey,
    brush_xform: Option<&GlobalTransform>,
    camera_query: &Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: &Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
) -> (Option<SlideSideInfo>, Option<SlideSideInfo>) {
    let Some(brush_xform) = brush_xform else {
        return (None, None);
    };
    let Ok((camera, cam_tf)) = camera_query.single() else {
        return (None, None);
    };

    let quad_loops: Vec<_> = radial_walk(mesh, edge_key)
        .filter(|&lp| mesh.faces[mesh.loops[lp].face].loop_count == 4)
        .collect();

    let viewport_map = viewport_query
        .single()
        .ok()
        .map(|(computed, vp_transform)| ViewportRemap::new(camera, computed, vp_transform));
    let to_window = |rt: Vec2| match &viewport_map {
        Some(map) => map.top_left + rt / map.remap,
        None => rt,
    };

    let compute_side = |lp_key: jackdaw_geometry::editmesh::LoopKey| -> Option<SlideSideInfo> {
        let next_loop = mesh.loops[lp_key].next;
        let v_end = mesh.loops[next_loop].vert;
        let slide_edge = mesh.loops[next_loop].edge;
        let slide_edge_data = &mesh.edges[slide_edge];
        let other_vert = if slide_edge_data.v[0] == v_end {
            slide_edge_data.v[1]
        } else {
            slide_edge_data.v[0]
        };

        let v_end_world = brush_xform.transform_point(mesh.verts[v_end].co);
        let other_world = brush_xform.transform_point(mesh.verts[other_vert].co);
        let len_world = (other_world - v_end_world).length();
        if len_world < 1e-6 {
            return None;
        }

        let p_end_rt = camera.world_to_viewport(cam_tf, v_end_world).ok()?;
        let p_other_rt = camera.world_to_viewport(cam_tf, other_world).ok()?;
        let p_end = to_window(p_end_rt);
        let p_other = to_window(p_other_rt);
        let dir = p_other - p_end;
        let len_window = dir.length();
        if len_window < 1e-4 {
            return None;
        }
        Some(SlideSideInfo {
            dir_unit: dir / len_window,
            len_window,
            len_world,
        })
    };

    let pos_side = quad_loops.first().copied().and_then(compute_side);
    let neg_side = quad_loops.get(1).copied().and_then(compute_side);
    (pos_side, neg_side)
}

fn find_edge_between(bmesh: &EditMesh, va: VertKey, vb: VertKey) -> Option<EdgeKey> {
    bmesh
        .edges
        .iter()
        .find(|(_, e)| (e.v[0] == va && e.v[1] == vb) || (e.v[0] == vb && e.v[1] == va))
        .map(|(k, _)| k)
}

pub(crate) fn can_run_edge_slide_modal(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Edge) && !selection.edges.is_empty()
}
