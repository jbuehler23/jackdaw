//! `brush.mesh.loop_cut` operator.

use bevy::picking::prelude::Pickable;
use bevy::prelude::*;
use bevy::ui::ui_transform::UiGlobalTransform;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;
use jackdaw_commands::EditorCommand;
use jackdaw_geometry::halfedge::ops::loop_cut::loop_cut;
use jackdaw_geometry::halfedge::{
    EdgeKey, HalfedgeBinding, HalfedgeMesh, VertKey, apply_topology_edit, edge_between,
};
use jackdaw_scene_types::Brush;
use jackdaw_select::address::ElementKind;

use super::modal_edit::ModalTopologyEdit;
use super::param_edit;
use super::select_elements::{
    edit_target, element_mode_blocked, enter_element_mode, refuse, resolve_on, selector_from_params,
};
use jackdaw_editor_core::brush::{
    BrushEditMode, BrushHalfedge, BrushSelection, EditMode, SetBrush,
};
use jackdaw_editor_core::commands::CommandHistory;
use jackdaw_editor_core::snapping::SnapSettings;
use jackdaw_editor_core::viewport::{MainViewportCamera, SceneViewport};
use jackdaw_editor_core::viewport_util::ViewportRemap;

/// World-space line segments for the loop cut preview gizmo overlay.
/// Each element is a (start, end) pair drawn as a cyan line each frame.
#[derive(Resource, Default)]
pub struct LoopCutPreviewLines {
    pub lines: Vec<(Vec3, Vec3)>,
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushLoopCutOp>()
        .register_operator::<BrushLoopCutAtOp>();

    ctx.bind_operator_host::<BrushLoopCutOp>([PresetInput::key("KeyR").ctrl_or_super()]);
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
    allows_undo = true,
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
    keybind_focus: jackdaw_editor_core::keybind_focus::KeybindFocus,
    modal_inputs: jackdaw_editor_core::modal_inputs::ModalInputs,
    cursor: jackdaw_editor_core::viewport::UiCursorPos,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    snap_settings: Res<SnapSettings>,
    modal_entity: Option<Single<Entity, With<ActiveModalOperator>>>,
) -> Option<OperatorResult> {
    // --- Cursor position ---
    // Use raw UI-space cursor so dragging outside the viewport panel
    // doesn't cancel the modal.
    let (camera, cam_tf) = camera_query.single().ok()?;
    let cursor_pos = cursor.get()?;

    // --- First invoke: snapshot and enter modal ---
    if modal_entity.is_none() {
        // Validate preconditions.
        if *edit_mode != EditMode::BrushEdit(BrushEditMode::Edge) {
            return None;
        }
        let brush_entity = selection.active_brush?;
        let (a, b) = selection
            .sub(brush_entity)
            .and_then(|s| s.edges.first().copied())?;

        let brush_before = brushes.get(brush_entity).cloned().ok()?;
        let halfedge = halfedge_q.get(brush_entity).ok()?;

        // Resolve cache pair -> EdgeKey.
        let va: VertKey = *halfedge.vert_keys.get(a)?;
        let vb: VertKey = *halfedge.vert_keys.get(b)?;
        let edge_key = edge_between(&halfedge.mesh, va, vb)?;

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

        return Some(OperatorResult::Running);
    }

    // --- Subsequent invokes: cancel, update t, preview, or commit ---

    // Cancel on Escape or RMB.
    let rmb = mouse.just_pressed(MouseButton::Right);
    if modal_inputs.cancel() || rmb {
        clear_modal(&mut modal_state, &mut preview_lines);
        return None;
    }

    // Project the cursor directly onto the start edge in window space to get t.
    // cursor_pos, start_v0_window, and start_v1_window are all in window space,
    // so no coordinate conversion is needed.
    let ctrl = keybind_focus.any_pressed(&keyboard, [KeyCode::ControlLeft, KeyCode::ControlRight]);
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
            return None;
        };
        let Some(edge_key) = modal_state.start_edge_key else {
            clear_modal(&mut modal_state, &mut preview_lines);
            return None;
        };
        let Some(edit) = modal_state.edit.as_ref() else {
            clear_modal(&mut modal_state, &mut preview_lines);
            return None;
        };
        let t = modal_state.current_t;

        let Ok(brush_mut) = brushes.get_mut(brush_entity) else {
            clear_modal(&mut modal_state, &mut preview_lines);
            return None;
        };
        let Ok(mut halfedge) = halfedge_q.get_mut(brush_entity) else {
            clear_modal(&mut modal_state, &mut preview_lines);
            return None;
        };
        let brush = brush_mut.into_inner();

        edit.restore(brush, &mut halfedge);
        let new_loop_edge_pairs = cut_loops(brush, &mut halfedge.0, edge_key, &[t]);

        let Some(new_loop_edge_pairs) = new_loop_edge_pairs else {
            clear_modal(&mut modal_state, &mut preview_lines);
            return None;
        };

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
        return Some(OperatorResult::Finished);
    }

    Some(OperatorResult::Running)
}

/// Cut new edge loops across the strip of quads through one edge, as one undo
/// entry. The parametric form of the loop cut drag: the edge is named by index
/// or picked by rules, one cut lands at `factor` along it and several are
/// spaced evenly, and the cut is checked before the edit mode or the selection
/// change. Leaves the new loops selected in Edge mode.
#[operator(
    id = "brush.mesh.loop_cut_at",
    label = "Loop Cut at Edge",
    description = "Cut new edge loops across the strip of quads through an edge, at its middle \
                   or where `factor` puts them.",
    allows_undo = false,
    params(
        entity(
            Entity,
            doc = "Brush to cut. Defaults to the brush being edited, else the selected one."
        ),
        edge(
            i64,
            doc = "Edge to cut across, an index into the `jackdaw/brush_topology` edge list."
        ),
        r#where(
            String,
            doc = "Rules picking the edge to cut across, as `brush.select_elements` takes \
                   them; the lowest-numbered match is cut. With `edge`, both must hold."
        ),
        count(
            i64,
            doc = "How many loops to cut, evenly spaced along the edge, 1 to 32. Defaults to 1."
        ),
        factor(
            f64,
            doc = "Where a single loop crosses the edge, as a fraction from its first vertex \
                   in the `jackdaw/brush_topology` edge list, between 0 and 1. Defaults to 0.5."
        ),
    )
)]
pub(crate) fn brush_loop_cut_at(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let spacing = match LoopSpacing::read(&params) {
        Ok(spacing) => spacing,
        Err(why) => return refuse(world, "Loop Cut at Edge", why),
    };
    let (entity, edge) = match loop_cut_target(world, &params) {
        Ok(Some(target)) => target,
        Ok(None) => {
            warn_caller(world, "Loop Cut at Edge: no edge matches `where`");
            set_outcome(world, Outcome::Unchanged);
            return OperatorResult::Cancelled;
        }
        Err(why) => return refuse(world, "Loop Cut at Edge", why),
    };
    if let Some(why) = element_mode_blocked(world) {
        return refuse(world, "Loop Cut at Edge", why);
    }
    let Some(old) = world.get::<Brush>(entity).cloned() else {
        return refuse(
            world,
            "Loop Cut at Edge",
            format!("`entity` {entity} is not a brush"),
        );
    };
    let Some((brush, new_loop)) = cut_across(&old, edge, spacing) else {
        warn_caller(
            world,
            format!("Loop Cut at Edge: edge {edge} has no strip of quads to cut across"),
        );
        set_outcome(world, Outcome::Unchanged);
        return OperatorResult::Cancelled;
    };

    enter_element_mode(world, entity, ElementKind::Edge);
    let report = param_edit::Counts::of(&old.topology).added(&brush.topology);
    let vert_count = brush.topology.vertices.len();
    let mut command: Box<dyn EditorCommand> = Box::new(SetBrush {
        entity,
        old,
        new: brush,
        label: "Loop Cut".to_string(),
    });
    command.execute(world);
    world
        .resource_mut::<CommandHistory>()
        .push_executed(command);
    report_to_caller(world, format!("Loop Cut at Edge: {report}"));

    let new_loop: Vec<(usize, usize)> = new_loop
        .into_iter()
        .filter(|(a, b)| *a < vert_count && *b < vert_count)
        .collect();
    world.resource_mut::<BrushSelection>().sub_mut(entity).edges = new_loop;
    set_outcome(world, Outcome::Changed);
    OperatorResult::Finished
}

/// The brush and edge a parametric loop cut acts on; `None` when the rules
/// match no edge.
fn loop_cut_target(
    world: &World,
    params: &OperatorParameters,
) -> Result<Option<(Entity, usize)>, String> {
    let mut selector = selector_from_params(params)?;
    if params.contains_key("edge") {
        let edge = params
            .as_int("edge")
            .and_then(|edge| usize::try_from(edge).ok())
            .ok_or("`edge` must be an edge index")?;
        selector.indices = Some(vec![edge]);
    }
    if selector.is_empty() {
        return Err("give `edge` or `where` to name the edge to cut across".into());
    }
    let entity = edit_target(world, params.get("entity"))?;
    let edges = resolve_on(world, entity, ElementKind::Edge, &selector)?;
    Ok(edges.first().map(|edge| (entity, *edge)))
}

/// The most loops one parametric loop cut makes.
const MAX_LOOP_CUTS: u32 = 32;

/// Where a parametric loop cut crosses its edge.
#[derive(Clone, Copy)]
enum LoopSpacing {
    /// One loop, this fraction along the edge from its first topology vertex.
    At(f32),
    /// This many loops, evenly spaced.
    Even(u32),
}

impl LoopSpacing {
    fn read(params: &OperatorParameters) -> Result<Self, String> {
        let count = param_edit::count(params, "count", MAX_LOOP_CUTS)?.unwrap_or(1);
        let factor = param_edit::number(params, "factor")?;
        match factor {
            Some(_) if count > 1 => Err("give `factor` only for a single loop".into()),
            Some(factor) if factor <= 0.0 || factor >= 1.0 => {
                Err(format!("`factor` {factor} must be between 0 and 1"))
            }
            Some(factor) => Ok(Self::At(factor)),
            None if count == 1 => Ok(Self::At(0.5)),
            None => Ok(Self::Even(count)),
        }
    }
}

/// `brush` with loops cut across `edge` as `spacing` places them, and the new
/// loops' vertex pairs, worked on a binding lifted fresh from its topology.
/// `None` when the edge runs through no strip of quads.
fn cut_across(
    brush: &Brush,
    edge: usize,
    spacing: LoopSpacing,
) -> Option<(Brush, Vec<(usize, usize)>)> {
    let [a, b] = brush.topology.edges.get(edge)?.v;
    let mut binding = HalfedgeBinding::lift_from_topology(&brush.topology);
    let va = *binding.vert_keys.get(a as usize)?;
    let vb = *binding.vert_keys.get(b as usize)?;
    let edge_key = edge_between(&binding.mesh, va, vb)?;
    // Each cut keeps `edge_key` on the part from its first vertex, so even
    // loops are cut from the far end inwards.
    let cuts: Vec<f32> = match spacing {
        LoopSpacing::At(factor) if binding.mesh.edges[edge_key].v[0] == va => vec![factor],
        LoopSpacing::At(factor) => vec![1.0 - factor],
        LoopSpacing::Even(count) => (1..=count)
            .rev()
            .map(|k| k as f32 / (k + 1) as f32)
            .collect(),
    };
    let mut cut = brush.clone();
    let new_loop = cut_loops(&mut cut, &mut binding, edge_key, &cuts)?;
    Some((cut, new_loop))
}

/// Cut a loop through `edge` at each fraction in `cuts`, in order, and
/// reconcile the brush. The new loops' vertex pairs, or `None` when a cut
/// fails, leaving the brush to be discarded.
fn cut_loops(
    brush: &mut Brush,
    binding: &mut HalfedgeBinding,
    edge: EdgeKey,
    cuts: &[f32],
) -> Option<Vec<(usize, usize)>> {
    apply_topology_edit(&mut brush.faces, &mut brush.topology, binding, |mesh| {
        let mut new_edges = Vec::new();
        for &t in cuts {
            new_edges.extend(loop_cut(mesh, edge, t).ok()?.new_loop_edges);
        }
        Some(topology_pairs(mesh, &new_edges))
    })
}

/// The topology vertex pairs of `edges` in `mesh` as it will flatten, each
/// once, skipping any edge or vertex no longer in the mesh. Valid for edits
/// that never remove vertices, since slot order is then the flattened vertex
/// order.
pub(super) fn topology_pairs(mesh: &HalfedgeMesh, edges: &[EdgeKey]) -> Vec<(usize, usize)> {
    let vk_to_topo: std::collections::HashMap<VertKey, usize> = mesh
        .verts
        .iter()
        .enumerate()
        .map(|(i, (k, _))| (k, i))
        .collect();
    let mut pairs: Vec<(usize, usize)> = Vec::with_capacity(edges.len());
    for ek in edges {
        let Some(edge) = mesh.edges.get(*ek) else {
            continue;
        };
        let (Some(&a), Some(&b)) = (vk_to_topo.get(&edge.v[0]), vk_to_topo.get(&edge.v[1])) else {
            continue;
        };
        let pair = if a < b { (a, b) } else { (b, a) };
        if !pairs.contains(&pair) {
            pairs.push(pair);
        }
    }
    pairs
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
            jackdaw_editor_core::EditorEntity,
            jackdaw_editor_core::NonSerializable,
            Text::new("MID"),
            TextFont {
                font: editor_font.0.clone().into(),
                font_size: jackdaw_feathers::tokens::TEXT_SIZE_SM,
                ..default()
            },
            TextColor(jackdaw_feathers::tokens::TEXT_ACCENT),
            Node {
                position_type: PositionType::Absolute,
                ..default()
            },
            // Any hovered node other than the viewport's own suppresses
            // viewport gestures; this badge is never a click target.
            Pickable::IGNORE,
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
    if font.font != editor_font.0.clone().into() {
        font.font = editor_font.0.clone().into();
    }
}

/// Draw cyan line segments for the loop cut preview, sourced from `LoopCutPreviewLines`.
pub fn draw_loop_cut_preview(
    preview_lines: Res<LoopCutPreviewLines>,
    mut gizmos: Gizmos<jackdaw_editor_core::face_grid::BrushOutlineSelectedGizmoGroup>,
) {
    for &(a, b) in &preview_lines.lines {
        gizmos.line(a, b, Color::srgb(0.3, 0.85, 1.0));
    }
}
