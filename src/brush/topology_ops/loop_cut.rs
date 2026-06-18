//! `brush.mesh.loop_cut` operator.

use bevy::prelude::*;
use bevy::ui::ui_transform::UiGlobalTransform;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;
use jackdaw_geometry::halfedge::ops::loop_cut::loop_cut;
use jackdaw_geometry::halfedge::{EdgeKey, HalfedgeMesh, VertKey};
use jackdaw_jsn::Brush;

use super::modal_edit::ModalTopologyEdit;
use crate::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode};
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

    ctx.bind_operator::<CoreExtensionInputContext, BrushLoopCutOp>([
        PresetInput::key("KeyR").ctrl()
    ]);
}

/// Modal state for the loop cut operator.
#[derive(Resource, Default)]
pub struct LoopCutModalState {
    pub active: bool,
    pub brush_entity: Option<Entity>,
    pub start_edge_key: Option<EdgeKey>,
    pub current_t: f32,
    /// Pre-edit snapshot driving restore and the commit-time cut. The preview
    /// gizmo also reads its snapshot half-edge mesh so `start_edge_key` stays
    /// valid for the whole modal.
    pub edit: Option<ModalTopologyEdit>,
    /// Window-space pixel position of the start edge's canonical `v[0]`.
    pub start_v0_window: Vec2,
    /// Window-space pixel position of the start edge's canonical `v[1]`.
    pub start_v1_window: Vec2,
    /// World-space endpoints of the start edge, used to place the "MID"
    /// label at the loop's position along the edge.
    pub start_a_world: Vec3,
    pub start_b_world: Vec3,
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
    mut halfedge_q: Query<&mut BrushHalfedge>,
    brush_transforms: Query<&GlobalTransform>,
    mut modal_state: ResMut<LoopCutModalState>,
    mut preview_lines: ResMut<LoopCutPreviewLines>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    modal_inputs: crate::modal_inputs::ModalInputs,
    cursor: crate::viewport::UiCursorPos,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    snap_settings: Res<SnapSettings>,
    modal_entity: Option<Single<Entity, With<ActiveModalOperator>>>,
) -> OperatorResult {
    // --- Cursor position ---
    // Use raw UI-space cursor so dragging outside the viewport panel
    // doesn't cancel the modal (the bounds check in window_to_viewport_cursor
    // returns None when the cursor leaves the UI node, which previously caused
    // the modal to cancel mid-drag).
    let (camera, cam_tf) = camera_query.single()?;
    let cursor_pos = cursor.get()?;

    // --- First invoke: snapshot and enter modal ---
    if modal_entity.is_none() {
        // Validate preconditions.
        if *edit_mode != EditMode::BrushEdit(BrushEditMode::Edge) {
            return OperatorResult::Cancelled;
        }
        let brush_entity = selection.active_brush?;
        let (a, b) = selection
            .sub(brush_entity)
            .and_then(|s| s.edges.first().copied())?;

        let brush_before = brushes.get(brush_entity).cloned()?;
        let halfedge = halfedge_q.get(brush_entity)?;

        // Resolve cache pair -> EdgeKey.
        let va: VertKey = *halfedge.vert_keys.get(a)?;
        let vb: VertKey = *halfedge.vert_keys.get(b)?;
        let edge_key = find_edge_between(&halfedge.mesh, va, vb)?;

        // Project the start edge's canonical endpoints to window space so each
        // subsequent frame can compute t directly from cursor position.
        // Falls back to (0, 0)/(1, 0) dummy endpoints on projection failure;
        // the modal will still work, just with degenerate tracking.
        let (v0_window, v1_window) = edge_endpoints_window(
            &halfedge.mesh,
            edge_key,
            brush_transforms.get(brush_entity).ok(),
            camera,
            cam_tf,
            &viewport_query,
        );

        // World-space endpoints of the start edge for the "MID" label.
        let (a_world, b_world) = match brush_transforms.get(brush_entity) {
            Ok(g) => (
                g.transform_point(halfedge.mesh.verts.get(va).map_or(Vec3::ZERO, |v| v.co)),
                g.transform_point(halfedge.mesh.verts.get(vb).map_or(Vec3::ZERO, |v| v.co)),
            ),
            Err(_) => (Vec3::ZERO, Vec3::ZERO),
        };

        modal_state.active = true;
        modal_state.brush_entity = Some(brush_entity);
        modal_state.start_edge_key = Some(edge_key);
        modal_state.current_t = 0.5;
        modal_state.edit = Some(ModalTopologyEdit::begin(&brush_before, halfedge));
        modal_state.start_v0_window = v0_window;
        modal_state.start_v1_window = v1_window;
        modal_state.start_a_world = a_world;
        modal_state.start_b_world = b_world;

        // Draw the initial preview lines at t=0.5.
        update_preview_lines(&modal_state, &brush_transforms, &mut preview_lines);

        return OperatorResult::Running;
    }

    // --- Subsequent invokes: cancel, update t, preview, or commit ---

    // Cancel on Escape or RMB.
    let rmb = mouse.just_pressed(MouseButton::Right);
    if modal_inputs.cancel() || rmb {
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
        let Some(edit) = modal_state.edit.as_ref() else {
            clear_modal(&mut modal_state, &mut preview_lines);
            return OperatorResult::Cancelled;
        };
        let t = modal_state.current_t;

        let Ok(brush_mut) = brushes.get_mut(brush_entity) else {
            clear_modal(&mut modal_state, &mut preview_lines);
            return OperatorResult::Cancelled;
        };
        let Ok(mut halfedge) = halfedge_q.get_mut(brush_entity) else {
            clear_modal(&mut modal_state, &mut preview_lines);
            return OperatorResult::Cancelled;
        };
        let brush = brush_mut.into_inner();

        // Source new split faces from the brush's last face (matching the prior
        // grow-loop template), keeping its UV axes; the seam then recomputes
        // every plane.
        let source = edit
            .snapshot_brush()
            .faces
            .last()
            .cloned()
            .unwrap_or_default();
        let original_face_count = edit.snapshot_brush().faces.len();

        // Restore to the snapshot, run the cut at the chosen t, and reconcile.
        // The closure resolves each new loop-ring edge's topology vertex pair
        // before flatten, while slotmap iteration order still matches what the
        // flatten will use (loop_cut never removes verts, so the indices hold).
        // Returns `None` if the cut fails so the commit can cancel.
        let new_loop_edge_pairs = edit.apply(brush, &mut halfedge, |mesh| {
            let Ok(loop_cut_result) = loop_cut(mesh, edge_key, t) else {
                return None;
            };
            let mut vk_to_topo: std::collections::HashMap<VertKey, usize> =
                std::collections::HashMap::with_capacity(mesh.verts.len());
            for (i, (k, _)) in mesh.verts.iter().enumerate() {
                vk_to_topo.insert(k, i);
            }
            let mut pairs: Vec<(usize, usize)> =
                Vec::with_capacity(loop_cut_result.new_loop_edges.len());
            for ek in &loop_cut_result.new_loop_edges {
                let edge = &mesh.edges[*ek];
                let Some(&a) = vk_to_topo.get(&edge.v[0]) else {
                    continue;
                };
                let Some(&b) = vk_to_topo.get(&edge.v[1]) else {
                    continue;
                };
                let pair = if a < b { (a, b) } else { (b, a) };
                pairs.push(pair);
            }
            Some(pairs)
        });

        let Some(new_loop_edge_pairs) = new_loop_edge_pairs else {
            clear_modal(&mut modal_state, &mut preview_lines);
            return OperatorResult::Cancelled;
        };

        // Each new split face inherits the source face's appearance.
        for new_face in original_face_count..brush.faces.len() {
            brush.faces[new_face].copy_appearance_from(&source);
            brush.faces[new_face].ensure_uv_axes();
        }

        // Chain selection: write the newly created loop ring edges into
        // `BrushSelection.edges` so a follow-up gesture (loop cut again,
        // edge slide, etc.) can operate on the new ring immediately.
        let vert_count = brush.topology.vertices.len();
        let inbounds: Vec<(usize, usize)> = new_loop_edge_pairs
            .into_iter()
            .filter(|(a, b)| *a < vert_count && *b < vert_count)
            .collect();
        if !inbounds.is_empty() {
            selection.sub_mut(brush_entity).edges = inbounds;
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
    mut halfedge_q: Query<&mut BrushHalfedge>,
) {
    if let Some(brush_entity) = modal_state.brush_entity
        && let Some(edit) = modal_state.edit.as_ref()
        && let Ok(mut brush) = brushes.get_mut(brush_entity)
        && let Ok(mut halfedge) = halfedge_q.get_mut(brush_entity)
    {
        edit.restore(&mut brush, &mut halfedge);
    }
    clear_modal(&mut modal_state, &mut preview_lines);
}

/// Reset modal state and clear the preview lines.
fn clear_modal(modal_state: &mut LoopCutModalState, preview_lines: &mut LoopCutPreviewLines) {
    *modal_state = LoopCutModalState::default();
    preview_lines.lines.clear();
}

/// Speculatively run `loop_cut` on a clone of the start `HalfedgeMesh` and write the
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
    let Some(edit) = modal_state.edit.as_ref() else {
        return;
    };
    let Ok(brush_xform) = brush_transforms.get(brush_entity) else {
        return;
    };

    let mut speculative = edit.snapshot_halfedge().mesh.clone();
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

/// Project the canonical `v[0]` and `v[1]` of `edge_key` to window-space pixels.
///
/// Returns `(v0_window, v1_window)` in the same coordinate system as
/// `window.cursor_position()` so the cursor can be projected directly onto
/// the edge each frame without any further conversion.
/// Falls back to `(Vec2::ZERO, Vec2::X)` on any projection failure.
fn edge_endpoints_window(
    mesh: &HalfedgeMesh,
    edge_key: EdgeKey,
    brush_xform: Option<&GlobalTransform>,
    camera: &Camera,
    cam_tf: &GlobalTransform,
    viewport_query: &Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
) -> (Vec2, Vec2) {
    let Some(brush_xform) = brush_xform else {
        return (Vec2::ZERO, Vec2::X);
    };
    let edge = &mesh.edges[edge_key];
    let v0_world = brush_xform.transform_point(mesh.verts[edge.v[0]].co);
    let v1_world = brush_xform.transform_point(mesh.verts[edge.v[1]].co);
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

fn find_edge_between(mesh: &HalfedgeMesh, va: VertKey, vb: VertKey) -> Option<EdgeKey> {
    mesh.edges
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
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Edge)
        && selection.active_sub().is_some_and(|s| !s.edges.is_empty())
}

/// Marks the floating "MID" badge shown while a loop cut is snapped to
/// the middle of its edge.
#[derive(Component)]
pub struct LoopCutMidLabel;

/// Show a small "MID" badge at the loop position while the loop cut is
/// snapped to the middle of the edge (t = 0.5). Mirrors the measure
/// tool's world-to-panel label placement.
pub fn update_loop_cut_mid_label(
    mut commands: Commands,
    modal_state: Res<LoopCutModalState>,
    cameras: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewports: Query<(Entity, &ComputedNode), With<SceneViewport>>,
    editor_font: Res<jackdaw_feathers::icons::EditorFont>,
    mut labels: Query<(&mut Node, &mut Visibility, &mut TextFont), With<LoopCutMidLabel>>,
) {
    let at_mid = modal_state.active && (modal_state.current_t - 0.5).abs() < 1.0e-3;

    if !at_mid {
        if let Ok((_, mut vis, _)) = labels.single_mut() {
            *vis = Visibility::Hidden;
        }
        return;
    }

    let Ok((camera, cam_tf)) = cameras.single() else {
        return;
    };
    let Ok((viewport_entity, viewport_node)) = viewports.single() else {
        return;
    };

    // Spawn the badge the first time it is needed, parented to the
    // viewport panel so its absolute position is panel-local.
    if labels.is_empty() {
        commands.spawn((
            LoopCutMidLabel,
            crate::EditorEntity,
            crate::NonSerializable,
            Text::new("MID"),
            TextFont {
                font: editor_font.0.clone(),
                font_size: jackdaw_feathers::tokens::FONT_SM,
                ..default()
            },
            TextColor(jackdaw_feathers::tokens::TEXT_ACCENT),
            Node {
                position_type: PositionType::Absolute,
                ..default()
            },
            Visibility::Hidden,
            ChildOf(viewport_entity),
        ));
        return;
    }
    let Ok((mut node, mut vis, mut font)) = labels.single_mut() else {
        return;
    };

    let mid = modal_state
        .start_a_world
        .lerp(modal_state.start_b_world, modal_state.current_t);
    let vp_node_size = viewport_node.size();
    let scale = viewport_node.inverse_scale_factor();
    let render_target_size = camera
        .logical_viewport_size()
        .unwrap_or(vp_node_size * scale);
    if let Ok(vp_coords) = camera.world_to_viewport(cam_tf, mid) {
        let ui_pos = vp_coords * vp_node_size / render_target_size * scale;
        node.left = Val::Px(ui_pos.x + 8.0);
        node.top = Val::Px(ui_pos.y - 8.0);
        *vis = Visibility::Inherited;
    } else {
        *vis = Visibility::Hidden;
    }
    if font.font != editor_font.0 {
        font.font = editor_font.0.clone();
    }
}
