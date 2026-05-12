//! `brush.mesh.vertex_slide_modal` operator -- Blender-style modal vertex slide.
//!
//! Press `Shift+V` in Vertex mode with exactly one vertex selected. Cursor
//! motion picks both the target incident edge (whichever screen-space
//! direction from the vertex outward best aligns with the cursor delta) and
//! the slide factor in `[0, 1]`: 0 = no slide, 1 = collapsed onto the chosen
//! neighbor. A cyan preview line goes from the original vertex position to
//! the speculative slid position. LMB commits; Esc / RMB cancels.
//!
//! Coexists with the non-modal `brush.mesh.vertex_slide` operator (which
//! slides by a fixed amount along the first disk-cycle edge) -- this is a
//! separate entry point.

use bevy::prelude::*;
use bevy::ui::ui_transform::UiGlobalTransform;
use bevy::window::PrimaryWindow;
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;
use jackdaw_geometry::editmesh::cycles::disk_walk;
use jackdaw_geometry::editmesh::ops::vertex_slide::vertex_slide;
use jackdaw_geometry::editmesh::{EdgeKey, EditMesh, VertKey};
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMesh, BrushEditMode, BrushSelection, EditMode, SetBrush};
use crate::commands::CommandHistory;
use crate::core_extension::CoreExtensionInputContext;
use crate::snapping::SnapSettings;
use crate::viewport::{MainViewportCamera, SceneViewport};
use crate::viewport_util::ViewportRemap;

/// Below this cursor-delta magnitude (in window pixels), no candidate edge
/// is preferred and the slide factor stays at 0. Avoids jittery target
/// switching when the user has barely moved the cursor.
const VERTEX_SLIDE_DEADZONE: f32 = 2.0;

/// World-space line segments for the modal vertex slide preview gizmo.
/// Each element is a (start, end) pair drawn as a cyan line each frame.
#[derive(Resource, Default)]
pub struct VertexSlidePreviewLines {
    pub lines: Vec<(Vec3, Vec3)>,
}

/// One incident edge and its screen-space direction from the slid vertex.
#[derive(Clone, Copy)]
struct CandidateEdge {
    edge_key: EdgeKey,
    other_vert: VertKey,
    /// Unit-length screen-space direction from the slid vertex toward
    /// `other_vert`, in window pixels. `None` if both endpoints projected
    /// to the same point (degenerate / behind-camera edge).
    screen_dir: Option<Vec2>,
    /// Length of the (non-normalized) screen vector from the slid vertex
    /// to `other_vert`. Used to map projection distance to a [0, 1] factor.
    screen_len: f32,
    /// World-space length of the candidate edge. Used to convert factor into
    /// a world distance for grid snapping.
    world_len: f32,
}

/// Modal state for the `brush.mesh.vertex_slide_modal` operator.
#[derive(Resource, Default)]
pub struct VertexSlideModalState {
    pub active: bool,
    pub brush_entity: Option<Entity>,
    /// EditMesh VertKey of the vertex being slid.
    pub vert_key: Option<VertKey>,
    /// Window-space cursor position at the moment the modal started.
    pub start_cursor: Vec2,
    /// Incident edges of the slid vertex, snapshotted at modal entry along
    /// with their screen-space directions.
    candidates: Vec<CandidateEdge>,
    /// Index into `candidates` of the currently chosen slide-target edge.
    /// `None` until the cursor leaves the deadzone.
    chosen_idx: Option<usize>,
    /// Current factor in `[0, 1]`. 0 = no slide, 1 = collapsed onto neighbor.
    pub current_factor: f32,
    pub start_brush: Option<Brush>,
    pub start_editmesh: Option<EditMesh>,
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushVertexSlideModalOp>();

    let ext = ctx.id();
    ctx.entity_mut().world_scope(|world| {
        world.spawn((
            Action::<BrushVertexSlideModalOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(
                KeyCode::KeyV.with_mod_keys(ModKeys::SHIFT),
                Press::default(),
            )],
        ));
    });
}

/// Slide the selected vertex along whichever incident edge the cursor is
/// pointing toward, by a factor in `[0, 1]` derived from cursor distance.
/// LMB commits; Esc / RMB cancels.
///
/// Requires Vertex mode with exactly one vertex selected.
#[operator(
    id = "brush.mesh.vertex_slide_modal",
    label = "Vertex Slide (Modal)",
    is_available = can_run_vertex_slide_modal,
    modal = true,
    allows_undo = false,
    cancel = cancel_vertex_slide,
)]
pub(crate) fn brush_vertex_slide_modal(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut bmesh_q: Query<&mut BrushEditMesh>,
    brush_transforms: Query<&GlobalTransform>,
    mut history: ResMut<CommandHistory>,
    mut modal_state: ResMut<VertexSlideModalState>,
    mut preview_lines: ResMut<VertexSlidePreviewLines>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    snap_settings: Res<SnapSettings>,
    modal_entity: Option<Single<Entity, With<ActiveModalOperator>>>,
) -> OperatorResult {
    // Raw window-space cursor; dragging outside the viewport panel should not
    // cancel the modal (matches inset / extrude / edge_slide_modal behavior).
    let Ok(window) = primary_window.single() else {
        return OperatorResult::Cancelled;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return OperatorResult::Cancelled;
    };

    // --- First invoke: snapshot and enter modal ---
    if modal_entity.is_none() {
        if *edit_mode != EditMode::BrushEdit(BrushEditMode::Vertex) {
            return OperatorResult::Cancelled;
        }
        let Some(brush_entity) = selection.entity else {
            return OperatorResult::Cancelled;
        };
        if selection.vertices.len() != 1 {
            return OperatorResult::Cancelled;
        }

        let Ok(brush_before) = brushes.get(brush_entity).cloned() else {
            return OperatorResult::Cancelled;
        };
        let Ok(bmesh_component) = bmesh_q.get(brush_entity) else {
            return OperatorResult::Cancelled;
        };

        let Some(&vert_idx) = selection.vertices.first() else {
            return OperatorResult::Cancelled;
        };
        let Some(&vert_key) = bmesh_component.vert_keys.get(vert_idx) else {
            return OperatorResult::Cancelled;
        };

        let mesh_snapshot = bmesh_component.mesh.clone();
        let brush_xform = brush_transforms.get(brush_entity).ok();

        let candidates = collect_candidate_edges(
            &mesh_snapshot,
            vert_key,
            brush_xform,
            &camera_query,
            &viewport_query,
        );
        if candidates.is_empty() {
            return OperatorResult::Cancelled;
        }

        modal_state.active = true;
        modal_state.brush_entity = Some(brush_entity);
        modal_state.vert_key = Some(vert_key);
        modal_state.start_cursor = cursor_pos;
        modal_state.candidates = candidates;
        modal_state.chosen_idx = None;
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

    let cursor_delta = cursor_pos - modal_state.start_cursor;
    let delta_len = cursor_delta.length();

    if delta_len < VERTEX_SLIDE_DEADZONE {
        modal_state.chosen_idx = None;
        modal_state.current_factor = 0.0;
    } else {
        // Pick the candidate whose screen-space outward direction is most
        // aligned with the cursor delta. We compare unit-dir dot products
        // against the unit cursor direction; candidates without a usable
        // screen direction are skipped.
        let cursor_dir = cursor_delta / delta_len;
        let mut best: Option<(usize, f32)> = None;
        for (idx, cand) in modal_state.candidates.iter().enumerate() {
            let Some(dir) = cand.screen_dir else {
                continue;
            };
            let align = dir.dot(cursor_dir);
            if align <= 0.0 {
                continue;
            }
            if best.is_none_or(|(_, best_align)| align > best_align) {
                best = Some((idx, align));
            }
        }

        if let Some((idx, _)) = best {
            modal_state.chosen_idx = Some(idx);
            let cand = modal_state.candidates[idx];
            // Project cursor delta onto the (non-normalized) screen edge
            // vector and divide by its squared length to get the [0, 1]
            // parameter along the edge. Equivalent to (delta . dir) / len
            // where dir is unit-length, but using the raw vector avoids a
            // potential div-by-zero if `screen_len` is tiny.
            let raw_factor = if cand.screen_len > 1e-4 {
                let dir_unit = cand.screen_dir.unwrap_or(Vec2::X);
                let proj = cursor_delta.dot(dir_unit);
                (proj / cand.screen_len).clamp(0.0, 1.0)
            } else {
                0.0
            };
            // Snap respects the global translate_snap toggle; Ctrl flips it.
            let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
            modal_state.current_factor = if snap_settings.translate_active(ctrl)
                && snap_settings.translate_increment > 0.0
                && cand.world_len > 1e-6
            {
                // Snap the world-space slide distance to the translate grid,
                // then back-convert to a [0, 1] factor along this edge.
                let inc = snap_settings.translate_increment;
                let world_disp = raw_factor * cand.world_len;
                let snapped_world = (world_disp / inc).round() * inc;
                (snapped_world / cand.world_len).clamp(0.0, 1.0)
            } else {
                raw_factor
            };
        } else {
            modal_state.chosen_idx = None;
            modal_state.current_factor = 0.0;
        }
    }

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
        let Some(vert_key) = modal_state.vert_key else {
            clear_modal(&mut modal_state, &mut preview_lines);
            return OperatorResult::Cancelled;
        };
        let Some(chosen_idx) = modal_state.chosen_idx else {
            // No chosen edge: treat as no-op cancel.
            clear_modal(&mut modal_state, &mut preview_lines);
            return OperatorResult::Cancelled;
        };
        let chosen_edge = modal_state.candidates[chosen_idx].edge_key;

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

        // Steer the `vertex_slide` op toward our chosen edge. The op reads
        // `verts[v].edge` to pick the slide target; the disk cycle's
        // doubly-linked structure (via `disk_next` / `disk_prev` on edges)
        // is independent of this anchor, so retargeting it does not break
        // any invariants. We have to do this after restoring the snapshot
        // since `verts[v].edge` is part of the EditMesh state.
        if let Some(vert) = bmesh_component.mesh.verts.get_mut(vert_key) {
            vert.edge = Some(chosen_edge);
        }

        if vertex_slide(&mut bmesh_component.mesh, &[vert_key], factor).is_err() {
            clear_modal(&mut modal_state, &mut preview_lines);
            return OperatorResult::Cancelled;
        }

        // Re-cache all face normals (slid vert can rotate face planes).
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

        // Vertex slide does not add new faces; no need to grow brush.faces.
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
            label: "Vertex Slide".to_string(),
        }));

        // selection.vertices intentionally untouched: vertex_slide preserves
        // vertex identity (no add/remove), and slotmap iteration order is
        // stable when no verts are added or removed.

        clear_modal(&mut modal_state, &mut preview_lines);
        return OperatorResult::Finished;
    }

    OperatorResult::Running
}

/// Cancel handler: restore the brush to its pre-modal state and clear preview.
fn cancel_vertex_slide(
    mut modal_state: ResMut<VertexSlideModalState>,
    mut preview_lines: ResMut<VertexSlidePreviewLines>,
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

fn clear_modal(
    modal_state: &mut VertexSlideModalState,
    preview_lines: &mut VertexSlidePreviewLines,
) {
    *modal_state = VertexSlideModalState::default();
    preview_lines.lines.clear();
}

/// Compute the cyan preview line: from the original vertex position to the
/// speculative slid position based on the currently chosen edge + factor.
/// Empty if no candidate is chosen yet (cursor still in deadzone).
fn update_preview_lines(
    modal_state: &VertexSlideModalState,
    brush_transforms: &Query<&GlobalTransform>,
    preview_lines: &mut VertexSlidePreviewLines,
) {
    preview_lines.lines.clear();

    let Some(brush_entity) = modal_state.brush_entity else {
        return;
    };
    let Some(ref start_mesh) = modal_state.start_editmesh else {
        return;
    };
    let Some(vert_key) = modal_state.vert_key else {
        return;
    };
    let Some(chosen_idx) = modal_state.chosen_idx else {
        return;
    };
    let Ok(brush_xform) = brush_transforms.get(brush_entity) else {
        return;
    };
    let Some(start_vert) = start_mesh.verts.get(vert_key) else {
        return;
    };
    let Some(cand) = modal_state.candidates.get(chosen_idx) else {
        return;
    };
    let Some(target_vert) = start_mesh.verts.get(cand.other_vert) else {
        return;
    };

    let from_local = start_vert.co;
    let to_local = from_local.lerp(target_vert.co, modal_state.current_factor);
    let from_world = brush_xform.transform_point(from_local);
    let to_world = brush_xform.transform_point(to_local);
    preview_lines.lines.push((from_world, to_world));
}

/// Enumerate incident edges of `vert_key` and project each neighbor into
/// window-space pixels. Used at modal entry to pre-compute the per-frame
/// "which edge does the cursor point at" decision.
fn collect_candidate_edges(
    mesh: &EditMesh,
    vert_key: VertKey,
    brush_xform: Option<&GlobalTransform>,
    camera_query: &Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: &Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
) -> Vec<CandidateEdge> {
    let mut out = Vec::new();
    let Some(brush_xform) = brush_xform else {
        return out;
    };
    let Ok((camera, cam_tf)) = camera_query.single() else {
        return out;
    };

    let v_local = mesh.verts[vert_key].co;
    let v_world = brush_xform.transform_point(v_local);
    let Ok(v_rt) = camera.world_to_viewport(cam_tf, v_world) else {
        return out;
    };

    let viewport_remap = viewport_query.single().ok();
    let v_win = if let Some((computed, vp_transform)) = viewport_remap {
        let map = ViewportRemap::new(camera, computed, vp_transform);
        map.top_left + v_rt / map.remap
    } else {
        v_rt
    };

    let v_world = brush_xform.transform_point(mesh.verts[vert_key].co);
    for edge_key in disk_walk(mesh, vert_key) {
        let edge = &mesh.edges[edge_key];
        let other_vert = if edge.v[0] == vert_key {
            edge.v[1]
        } else {
            edge.v[0]
        };
        let other_local = mesh.verts[other_vert].co;
        let other_world = brush_xform.transform_point(other_local);
        let world_len = (other_world - v_world).length();
        let Ok(other_rt) = camera.world_to_viewport(cam_tf, other_world) else {
            out.push(CandidateEdge {
                edge_key,
                other_vert,
                screen_dir: None,
                screen_len: 0.0,
                world_len,
            });
            continue;
        };
        let other_win = if let Some((computed, vp_transform)) = viewport_remap {
            let map = ViewportRemap::new(camera, computed, vp_transform);
            map.top_left + other_rt / map.remap
        } else {
            other_rt
        };

        let vec = other_win - v_win;
        let len = vec.length();
        let (dir, screen_len) = if len > 1e-4 {
            (Some(vec / len), len)
        } else {
            (None, 0.0)
        };
        out.push(CandidateEdge {
            edge_key,
            other_vert,
            screen_dir: dir,
            screen_len,
            world_len,
        });
    }

    out
}

pub(crate) fn can_run_vertex_slide_modal(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Vertex) && selection.vertices.len() == 1
}
