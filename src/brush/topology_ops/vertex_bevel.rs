//! `brush.mesh.vertex_bevel` operator -- Blender-style modal vertex bevel.
//!
//! Press `Ctrl+Shift+B` in Vertex mode with exactly one vertex selected.
//! Cursor displacement magnitude drives a positive bevel width; Ctrl snaps
//! the world width to the translate grid increment. The brush mesh is mutated
//! each frame so the user sees the live N-gon bevel as they drag. LMB
//! commits; Esc / RMB cancels and restores the pre-modal mesh.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;
use jackdaw_geometry::editmesh::cycles::disk_walk;
use jackdaw_geometry::editmesh::ops::vertex_bevel::vertex_bevel;
use jackdaw_geometry::editmesh::{EditMesh, VertKey};
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMesh, BrushEditMode, BrushSelection, EditMode, SetBrush};
use crate::commands::CommandHistory;
use crate::core_extension::CoreExtensionInputContext;
use crate::snapping::SnapSettings;

/// Pixels-per-world-unit sensitivity for the vertex bevel modal. At this
/// value 100 pixels of cursor motion correspond to 1 world-unit of bevel
/// width. Matches the edge bevel modal.
const VERTEX_BEVEL_SENSITIVITY: f32 = 0.01;

/// Modal state for the `brush.mesh.vertex_bevel` operator.
#[derive(Resource, Default)]
pub struct VertexBevelModalState {
    pub active: bool,
    pub brush_entity: Option<Entity>,
    /// EditMesh VertKey of the vertex being beveled. Re-resolved against
    /// `start_editmesh` each frame because the live mesh is reset to the
    /// snapshot before running the op.
    pub vert_key: Option<VertKey>,
    /// Window-space cursor position at the moment the modal started.
    pub start_cursor: Vec2,
    /// Current bevel width in world-space units.
    pub current_width: f32,
    pub start_brush: Option<Brush>,
    pub start_editmesh: Option<EditMesh>,
    /// Maximum valid bevel width: 0.99 * half the length of the shortest
    /// incident edge at the beveled vertex. Past this point the offset would
    /// overshoot the neighbor and the rebuilt face collapses.
    pub max_width: f32,
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushVertexBevelOp>();

    let ext = ctx.id();
    ctx.entity_mut().world_scope(|world| {
        world.spawn((
            Action::<BrushVertexBevelOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(
                KeyCode::KeyB.with_mod_keys(ModKeys::CONTROL),
                Press::default(),
            )],
        ));
    });
}

/// Bevel the selected vertex into an N-gon face, controlled by cursor
/// displacement magnitude. Ctrl snaps to the translate grid increment. The
/// live brush mesh is updated each frame so the bevel is visible as a real
/// mesh edit. LMB commits; Esc / RMB cancels and reverts.
///
/// Requires Vertex mode with exactly one vertex selected.
#[operator(
    id = "brush.mesh.vertex_bevel",
    label = "Vertex Bevel",
    is_available = can_run_vertex_bevel,
    modal = true,
    allows_undo = false,
    cancel = cancel_vertex_bevel,
)]
pub(crate) fn brush_vertex_bevel(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    mut selection: ResMut<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut bmesh_q: Query<&mut BrushEditMesh>,
    mut history: ResMut<CommandHistory>,
    mut modal_state: ResMut<VertexBevelModalState>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
    snap_settings: Res<SnapSettings>,
    modal_entity: Option<Single<Entity, With<ActiveModalOperator>>>,
) -> OperatorResult {
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
        let max_width = compute_max_bevel_width(&mesh_snapshot, vert_key);

        modal_state.active = true;
        modal_state.brush_entity = Some(brush_entity);
        modal_state.vert_key = Some(vert_key);
        modal_state.start_cursor = cursor_pos;
        modal_state.current_width = 0.0;
        modal_state.start_brush = Some(brush_before);
        modal_state.start_editmesh = Some(mesh_snapshot);
        modal_state.max_width = max_width;

        return OperatorResult::Running;
    }

    // --- Subsequent invokes: cancel, update width, mutate preview, or commit ---

    let escape = keyboard.just_pressed(KeyCode::Escape);
    let rmb = mouse.just_pressed(MouseButton::Right);
    if escape || rmb {
        // Live brush has been mutated each frame, so restore from the snapshot
        // before clearing modal state.
        restore_brush_from_snapshot(&modal_state, &mut brushes, &mut bmesh_q);
        *modal_state = VertexBevelModalState::default();
        return OperatorResult::Cancelled;
    }

    // Cursor distance from the initial position drives the width.
    let delta = cursor_pos - modal_state.start_cursor;
    let raw_width = delta.length() * VERTEX_BEVEL_SENSITIVITY;
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
    // mesh edit. The op result is discarded; the bevel face is visible
    // through the regular brush mesh pipeline picking up `Changed<Brush>`.
    apply_live_bevel(&mut modal_state, &mut brushes, &mut bmesh_q);

    // Commit on LMB.
    if mouse.just_pressed(MouseButton::Left) {
        let Some(brush_entity) = modal_state.brush_entity else {
            *modal_state = VertexBevelModalState::default();
            return OperatorResult::Cancelled;
        };
        let Some(start_brush) = modal_state.start_brush.clone() else {
            *modal_state = VertexBevelModalState::default();
            return OperatorResult::Cancelled;
        };

        // Zero-width commit: treat as cancel so we don't write a no-op undo.
        // The live brush should already be back to the snapshot in this case
        // (apply_live_bevel resets to the snapshot when width is sub-threshold).
        if modal_state.current_width < 1e-5 {
            restore_brush_from_snapshot(&modal_state, &mut brushes, &mut bmesh_q);
            *modal_state = VertexBevelModalState::default();
            return OperatorResult::Cancelled;
        }

        let Ok(brush) = brushes.get(brush_entity).cloned() else {
            *modal_state = VertexBevelModalState::default();
            return OperatorResult::Cancelled;
        };

        // Chain selection: the new bevel face is the last face in the
        // topology since vertex_bevel appends one face. Select it for any
        // follow-up gestures (the user stays in Vertex mode, but if they
        // switch to Face mode the bevel face will be the active selection).
        let new_face_idx = brush.faces.len().saturating_sub(1);
        selection.faces = vec![new_face_idx];

        history.push_executed(Box::new(SetBrush {
            entity: brush_entity,
            old: start_brush,
            new: brush,
            label: "Vertex Bevel".to_string(),
        }));

        *modal_state = VertexBevelModalState::default();
        return OperatorResult::Finished;
    }

    OperatorResult::Running
}

/// Cancel handler: restore the brush to its pre-modal state. Called when the
/// modal lifecycle is force-cancelled from outside the operator.
fn cancel_vertex_bevel(
    mut modal_state: ResMut<VertexBevelModalState>,
    mut brushes: Query<&mut Brush>,
    mut bmesh_q: Query<&mut BrushEditMesh>,
) {
    restore_brush_from_snapshot(&modal_state, &mut brushes, &mut bmesh_q);
    *modal_state = VertexBevelModalState::default();
}

/// Reset the live brush + EditMesh to the snapshot captured at modal start.
fn restore_brush_from_snapshot(
    modal_state: &VertexBevelModalState,
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

/// Re-run `vertex_bevel` against the snapshot at the current width and write
/// the resulting topology back into the live `Brush` + `BrushEditMesh`.
fn apply_live_bevel(
    modal_state: &mut VertexBevelModalState,
    brushes: &mut Query<&mut Brush>,
    bmesh_q: &mut Query<&mut BrushEditMesh>,
) {
    let Some(brush_entity) = modal_state.brush_entity else {
        return;
    };
    let Some(ref start_mesh) = modal_state.start_editmesh else {
        return;
    };
    let Some(ref start_brush) = modal_state.start_brush else {
        return;
    };
    let Some(vert_key) = modal_state.vert_key else {
        return;
    };
    let Ok(mut bmesh_component) = bmesh_q.get_mut(brush_entity) else {
        return;
    };

    // Sub-threshold widths: snap the live mesh back to the start state.
    if modal_state.current_width < 1e-5 {
        let Ok(mut brush) = brushes.get_mut(brush_entity) else {
            return;
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
        return;
    }

    // Always start the per-frame op from the clean snapshot.
    bmesh_component.mesh = start_mesh.clone();

    if vertex_bevel(
        &mut bmesh_component.mesh,
        vert_key,
        modal_state.current_width,
    )
    .is_err()
    {
        return;
    }

    // Re-cache all face normals; vertex bevel reshapes the rebuilt faces.
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
        return;
    };

    // Grow brush.faces to cover the new bevel face. Seed each new slot from
    // the start brush's last face (for material + uv_scale / rotation), then
    // zero out uv_u_axis / uv_v_axis so `ensure_uv_axes` derives proper
    // tangents from the bevel face's own plane normal.
    let new_face_count = new_topology.polygons.len();
    let original_face_count = start_brush.faces.len();
    while brush.faces.len() < new_face_count {
        let mut template = start_brush
            .faces
            .last()
            .cloned()
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
}

/// Geometric cap on bevel width: half the length of the shortest incident
/// edge at the beveled vertex, times a 0.99 safety factor. Past this point
/// the offset overshoots its neighbor and the rebuilt face collapses.
fn compute_max_bevel_width(mesh: &EditMesh, vert_key: VertKey) -> f32 {
    if !mesh.verts.contains_key(vert_key) {
        return 0.0;
    }
    let v_pos = mesh.verts[vert_key].co;
    let mut min_half_len = f32::MAX;
    for edge_key in disk_walk(mesh, vert_key) {
        let edge = &mesh.edges[edge_key];
        let other = if edge.v[0] == vert_key {
            edge.v[1]
        } else {
            edge.v[0]
        };
        let other_pos = mesh.verts[other].co;
        let len = (other_pos - v_pos).length();
        let half = len * 0.5;
        if half > 1e-6 && half < min_half_len {
            min_half_len = half;
        }
    }
    if min_half_len.is_finite() {
        min_half_len * 0.99
    } else {
        f32::MAX
    }
}

pub(crate) fn can_run_vertex_bevel(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Vertex) && selection.vertices.len() == 1
}
