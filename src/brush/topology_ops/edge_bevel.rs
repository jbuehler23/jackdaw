//! `brush.mesh.edge_bevel` operator: modal edge bevel.
//!
//! Press `Ctrl+B` in Edge mode with at least one edge selected. Cursor
//! displacement magnitude drives a positive bevel width; Ctrl snaps the world
//! width to the translate grid increment. The brush mesh is mutated each
//! frame so the user sees the live chamfer as they drag. LMB commits; Esc /
//! RMB cancels and restores the pre-modal mesh.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;
use jackdaw_geometry::halfedge::ops::edge_bevel::edge_bevel;
use jackdaw_geometry::halfedge::{EdgeKey, HalfedgeMesh, VertKey};
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode, SetBrush};
use crate::commands::CommandHistory;
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
    /// `HalfedgeMesh` `EdgeKeys` of the edges being beveled. These are resolved
    /// against `start_mesh`; we re-resolve them from `start_mesh`
    /// each frame because the live mesh is reset to the snapshot before
    /// running the op.
    pub edge_keys: Vec<EdgeKey>,
    /// Window-space cursor position at the moment the modal started.
    pub start_cursor: Vec2,
    /// Current bevel width in world-space units.
    pub current_width: f32,
    pub start_brush: Option<Brush>,
    pub start_mesh: Option<HalfedgeMesh>,
    /// Maximum valid bevel width: minimum over each input edge of half the
    /// length of every parallel edge at its endpoints, with a small safety
    /// factor. Past this point an offset overshoots its parallel edge and
    /// the rebuilt face collapses.
    pub max_width: f32,
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushEdgeBevelOp>();

    let ext = ctx.id();
    ctx.entity_mut().world_scope(|world| {
        world.spawn((
            Action::<BrushEdgeBevelOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(
                KeyCode::KeyB.with_mod_keys(ModKeys::CONTROL),
                Press::default(),
            )],
        ));
    });
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
    mut history: ResMut<CommandHistory>,
    mut modal_state: ResMut<EdgeBevelModalState>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
    snap_settings: Res<SnapSettings>,
    modal_entity: Option<Single<Entity, With<ActiveModalOperator>>>,
) -> OperatorResult {
    let window = primary_window.single()?;
    let cursor_pos = window.cursor_position()?;

    // --- First invoke: snapshot and enter modal ---
    if modal_entity.is_none() {
        if *edit_mode != EditMode::BrushEdit(BrushEditMode::Edge) {
            return OperatorResult::Cancelled;
        }
        let brush_entity = selection.entity?;
        if selection.edges.is_empty() {
            return OperatorResult::Cancelled;
        }

        let brush_before = brushes.get(brush_entity).cloned()?;
        let halfedge = halfedge_q.get(brush_entity)?;

        // Resolve HalfedgeMesh EdgeKeys for every selected cache edge pair.
        let mut edge_keys: Vec<EdgeKey> = Vec::with_capacity(selection.edges.len());
        for &(a, b) in &selection.edges {
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

        let mesh_snapshot = halfedge.mesh.clone();
        let max_width = compute_max_bevel_width(&mesh_snapshot, &edge_keys);

        modal_state.active = true;
        modal_state.brush_entity = Some(brush_entity);
        modal_state.edge_keys = edge_keys;
        modal_state.start_cursor = cursor_pos;
        modal_state.current_width = 0.0;
        modal_state.start_brush = Some(brush_before);
        modal_state.start_mesh = Some(mesh_snapshot);
        modal_state.max_width = max_width;

        return OperatorResult::Running;
    }

    // --- Subsequent invokes: cancel, update width, mutate preview, or commit ---

    let escape = keyboard.just_pressed(KeyCode::Escape);
    let rmb = mouse.just_pressed(MouseButton::Right);
    if escape || rmb {
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
        let Some(brush_entity) = modal_state.brush_entity else {
            *modal_state = EdgeBevelModalState::default();
            return OperatorResult::Cancelled;
        };
        let Some(start_brush) = modal_state.start_brush.clone() else {
            *modal_state = EdgeBevelModalState::default();
            return OperatorResult::Cancelled;
        };

        // Zero-width commit: treat as cancel so we don't write a no-op undo.
        // The live brush should already be back to the snapshot in this case
        // (apply_live_bevel resets to the snapshot when width is sub-threshold).
        if modal_state.current_width < 1e-5 {
            restore_brush_from_snapshot(&modal_state, &mut brushes, &mut halfedge_q);
            *modal_state = EdgeBevelModalState::default();
            return OperatorResult::Cancelled;
        }

        let Ok(brush) = brushes.get(brush_entity).cloned() else {
            *modal_state = EdgeBevelModalState::default();
            return OperatorResult::Cancelled;
        };

        history.push_executed(Box::new(SetBrush {
            entity: brush_entity,
            old: start_brush,
            new: brush,
            label: "Edge Bevel".to_string(),
        }));

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
    let Some(ref start_mesh) = modal_state.start_mesh else {
        return;
    };
    let Some(ref start_brush) = modal_state.start_brush else {
        return;
    };
    let Ok(mut halfedge) = halfedge_q.get_mut(brush_entity) else {
        return;
    };

    // Sub-threshold widths: snap the live mesh back to the start state.
    if modal_state.current_width < 1e-5 {
        let Ok(mut brush) = brushes.get_mut(brush_entity) else {
            return;
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
        return;
    }

    // Always start the per-frame op from the clean snapshot.
    halfedge.mesh = start_mesh.clone();

    if edge_bevel(
        &mut halfedge.mesh,
        &modal_state.edge_keys,
        modal_state.current_width,
    )
    .is_err()
    {
        return;
    }

    // Re-cache all face normals; bevel reshapes the rebuilt faces.
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
        return;
    };

    // Grow brush.faces to cover any chamfer faces we just added. Seed each
    // new slot from the start brush's last face (for material + uv_scale /
    // rotation), then zero out the `uv_u_axis` / `uv_v_axis` so
    // `ensure_uv_axes` below derives proper tangents from the chamfer's own
    // plane normal (inheriting axes from a different-orientation template
    // stretches the texture).
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

    // Re-lift HalfedgeMesh from the new topology so vert_keys / face_keys are
    // consistent with the brush. This also keeps subsequent input-edge lookup
    // working because next frame we reset halfedge.mesh back to the
    // snapshot before running the op again.
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
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Edge) && !selection.edges.is_empty()
}
