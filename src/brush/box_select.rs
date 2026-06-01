//! Rubber-band box-select for brush sub-elements (vertices / edges /
//! faces). Active only in vertex / edge / face edit mode. Dragging a
//! rectangle over empty space selects every sub-element of the current
//! mode inside it, across all edit brushes. Plain drag replaces the
//! sub-selection; Shift+drag adds to it. A plain click on empty space
//! deselects all sub-elements while staying in edit mode.
//!
//! The per-element drag operators in `brush_drag_ops` hit-test on their
//! first invoke. When nothing is hit they hand the press to this module
//! by recording [`BrushBoxSelectState::pending`] instead of dropping to
//! Object mode. From there the lifecycle mirrors the object-mode
//! box-select: [`brush_box_select_promote`] watches the pending press
//! and either promotes it to an active drag once the cursor crosses the
//! threshold or resolves it as a plain click on release.

use bevy::picking::prelude::Pickable;
use bevy::prelude::*;
use jackdaw_api::prelude::*;

use crate::brush::{BrushEditMode, BrushMeshCache, BrushSelection, EditMode};
use crate::default_style;
use crate::viewport::ViewportCursor;
use crate::viewport_select::cursor_dragged_past_threshold;
use crate::viewport_util::ViewportRemap;

/// Marker for the brush box-select visual overlay node.
#[derive(Component)]
pub(crate) struct BrushBoxSelectOverlay;

/// State for the edit-mode rubber-band box-select. Mirrors the object
/// `BoxSelectState` but is scoped to brush sub-element editing.
#[derive(Resource, Default)]
pub struct BrushBoxSelectState {
    /// Cursor position recorded at LMB-down by a drag operator that hit
    /// nothing. Stays set until promoted to an active drag or cleared on
    /// release without crossing the threshold.
    pub pending: Option<Vec2>,
    /// Whether Shift was held at the press that set `pending`. Shift adds
    /// to the existing sub-selection instead of replacing it.
    pub shift: bool,
    pub active: bool,
    pub start: Vec2,
    pub current: Vec2,
    /// Camera entity of the viewport the drag started in, captured at
    /// modal start so the operator keeps querying the same viewport.
    pub camera: Option<Entity>,
    /// `SceneViewport` UI-node entity of the same viewport.
    pub viewport: Option<Entity>,
}

impl BrushBoxSelectState {
    /// Begin an active session, anchoring at the pending press position
    /// recorded by a drag operator if any, otherwise at `cursor_pos`.
    fn activate(&mut self, cursor_pos: Vec2) {
        let start = self.pending.take().unwrap_or(cursor_pos);
        self.active = true;
        self.start = start;
        self.current = cursor_pos;
    }
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushBoxSelectOp>();
}

/// True when the current edit mode is one that supports box-select.
fn box_select_mode(edit_mode: &EditMode) -> bool {
    matches!(
        edit_mode,
        EditMode::BrushEdit(BrushEditMode::Vertex | BrushEditMode::Edge | BrushEditMode::Face)
    )
}

/// Promotes a pending edit-mode press to an active box-select once the
/// cursor moves past the drag threshold, or resolves it as a plain
/// empty-click on release. Mirrors `box_select_promote_pending`.
pub(crate) fn brush_box_select_promote(
    mouse: Res<ButtonInput<MouseButton>>,
    edit_mode: Res<EditMode>,
    vp: ViewportCursor,
    mut box_state: ResMut<BrushBoxSelectState>,
    mut brush_selection: ResMut<BrushSelection>,
    mut commands: Commands,
) {
    let Some(start) = box_state.pending else {
        return;
    };
    if !box_select_mode(&edit_mode) {
        box_state.pending = None;
        box_state.shift = false;
        return;
    }
    let released = mouse.just_released(MouseButton::Left) || !mouse.pressed(MouseButton::Left);
    let Some(cursor_pos) = vp.cursor() else {
        if released {
            box_state.pending = None;
            box_state.shift = false;
        }
        return;
    };

    if !released && cursor_dragged_past_threshold(start, cursor_pos) {
        commands.queue(|world: &mut World| {
            if let Err(err) = world.operator(BrushBoxSelectOp::ID).call() {
                error!("brush box-select dispatch failed: {err}");
            }
        });
        return;
    }

    if released {
        // Plain empty-click: deselect all sub-elements but stay in edit
        // mode. Shift empty-click leaves the selection untouched.
        let shift = box_state.shift;
        box_state.pending = None;
        box_state.shift = false;
        if !shift {
            brush_selection.clear_sub_selections();
        }
    }
}

#[operator(
    id = "brush.box_select",
    label = "Box Select Sub-Elements",
    description = "Drag a rectangle to select brush vertices, edges, or faces inside it.",
    modal = true,
    cancel = cancel_brush_box_select,
)]
pub fn brush_box_select(
    _: In<OperatorParameters>,
    mouse: Res<ButtonInput<MouseButton>>,
    edit_mode: Res<EditMode>,
    vp: ViewportCursor,
    mut box_state: ResMut<BrushBoxSelectState>,
    mut brush_selection: ResMut<BrushSelection>,
    brush_transforms: Query<&GlobalTransform>,
    brush_caches: Query<&BrushMeshCache>,
    active: ActiveModalQuery,
) -> OperatorResult {
    let cursor_pos = vp.cursor()?;

    if !active.is_modal_running() {
        box_state.activate(cursor_pos);
        box_state.camera = vp.camera_entity();
        box_state.viewport = vp.viewport_entity();
        return OperatorResult::Running;
    }

    box_state.current = cursor_pos;
    if !mouse.just_released(MouseButton::Left) {
        return OperatorResult::Running;
    }
    box_state.active = false;

    let shift = box_state.shift;
    box_state.shift = false;

    let Some(camera_entity) = box_state.camera else {
        return OperatorResult::Finished;
    };
    let Some(viewport_entity) = box_state.viewport else {
        return OperatorResult::Finished;
    };
    let Some((camera, cam_tf)) = vp.camera_for(camera_entity) else {
        return OperatorResult::Finished;
    };
    let Some((vp_computed, vp_tf)) = vp.viewport_for(viewport_entity) else {
        return OperatorResult::Finished;
    };
    let map = ViewportRemap::new(camera, vp_computed, vp_tf);
    let start_local = box_state.start - map.top_left;
    let current_local = box_state.current - map.top_left;
    let min = start_local.min(current_local) * map.remap;
    let max = start_local.max(current_local) * map.remap;
    let inside = |p: Vec2| p.x >= min.x && p.x <= max.x && p.y >= min.y && p.y <= max.y;

    let mode = match *edit_mode {
        EditMode::BrushEdit(m @ (BrushEditMode::Vertex | BrushEditMode::Edge | BrushEditMode::Face)) => m,
        _ => return OperatorResult::Finished,
    };

    let edit_brushes: Vec<Entity> = brush_selection.edit_brushes().collect();

    if !shift {
        brush_selection.clear_sub_selections();
    }

    let mut first_hit_brush: Option<Entity> = None;

    for entity in edit_brushes {
        let Ok(cache) = brush_caches.get(entity) else {
            continue;
        };
        let Ok(global) = brush_transforms.get(entity) else {
            continue;
        };
        let screen_of = |local: Vec3| -> Option<Vec2> {
            camera
                .world_to_viewport(cam_tf, global.transform_point(local))
                .ok()
        };

        let mut hit = false;
        match mode {
            BrushEditMode::Vertex => {
                let sub = brush_selection.sub_mut(entity);
                for (i, &v) in cache.vertices.iter().enumerate() {
                    let Some(screen) = screen_of(v) else {
                        continue;
                    };
                    if inside(screen) && !sub.vertices.contains(&i) {
                        sub.vertices.push(i);
                        hit = true;
                    }
                }
            }
            BrushEditMode::Edge => {
                let unique_edges = unique_edges_of(cache);
                let sub = brush_selection.sub_mut(entity);
                for (a, b) in unique_edges {
                    let (Some(sa), Some(sb)) = (
                        screen_of(cache.vertices[a]),
                        screen_of(cache.vertices[b]),
                    ) else {
                        continue;
                    };
                    if inside(sa) && inside(sb) && !sub.edges.contains(&(a, b)) {
                        sub.edges.push((a, b));
                        hit = true;
                    }
                }
            }
            BrushEditMode::Face => {
                let sub = brush_selection.sub_mut(entity);
                for (f, polygon) in cache.face_polygons.iter().enumerate() {
                    if polygon.is_empty() {
                        continue;
                    }
                    let centroid: Vec3 = polygon.iter().map(|&vi| cache.vertices[vi]).sum::<Vec3>()
                        / polygon.len() as f32;
                    let Some(screen) = screen_of(centroid) else {
                        continue;
                    };
                    if inside(screen) && !sub.faces.contains(&f) {
                        sub.faces.push(f);
                        hit = true;
                    }
                }
            }
            BrushEditMode::Clip | BrushEditMode::Knife => {}
        }

        if hit && first_hit_brush.is_none() {
            first_hit_brush = Some(entity);
        }
    }

    if let Some(entity) = first_hit_brush {
        brush_selection.active_brush = Some(entity);
    }

    OperatorResult::Finished
}

fn cancel_brush_box_select(mut box_state: ResMut<BrushBoxSelectState>) {
    box_state.active = false;
    box_state.pending = None;
    box_state.shift = false;
}

/// Build the unique edges of a brush from its face polygons, normalized to
/// `(min, max)` index pairs. Same construction the edge pick uses.
fn unique_edges_of(cache: &BrushMeshCache) -> Vec<(usize, usize)> {
    let mut unique_edges: Vec<(usize, usize)> = Vec::new();
    for polygon in &cache.face_polygons {
        if polygon.len() < 2 {
            continue;
        }
        for i in 0..polygon.len() {
            let a = polygon[i];
            let b = polygon[(i + 1) % polygon.len()];
            let edge = (a.min(b), a.max(b));
            if !unique_edges.contains(&edge) {
                unique_edges.push(edge);
            }
        }
    }
    unique_edges
}

/// Draw the rubber-band rectangle while a brush box-select is active.
/// Parallel to the object `update_box_select_overlay`, driven from
/// [`BrushBoxSelectState`] so the two never fight over one node.
pub(crate) fn update_brush_box_select_overlay(
    box_state: Res<BrushBoxSelectState>,
    overlay_query: Query<Entity, With<BrushBoxSelectOverlay>>,
    mut commands: Commands,
) {
    if box_state.active {
        let min = box_state.start.min(box_state.current);
        let max = box_state.start.max(box_state.current);
        let size = max - min;

        let node = (
            BrushBoxSelectOverlay,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(min.x),
                top: Val::Px(min.y),
                width: Val::Px(size.x),
                height: Val::Px(size.y),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(default_style::SELECTION_MARQUEE_BG),
            BorderColor::all(default_style::SELECTION_MARQUEE_BORDER),
            GlobalZIndex(50),
            Pickable::IGNORE,
        );

        if let Some(entity) = overlay_query.iter().next() {
            commands.entity(entity).insert(node);
        } else {
            commands.spawn(node);
        }
    } else {
        for entity in &overlay_query {
            commands.entity(entity).despawn();
        }
    }
}
