use crate::core_extension::CoreExtensionInputContext;
use crate::keybind_focus::KeybindFocus;
use crate::prelude::*;
use crate::{selection::Selection, viewport::ViewportCursor};
use bevy::{input_focus::InputFocus, prelude::*};
use bevy_enhanced_input::prelude::Press;
use jackdaw_api_internal::keymap::PresetInput;
use jackdaw_jsn::Brush;

mod build;
mod csg_ops;
mod extend_face;
mod interaction;
mod plane_math;
mod preview_mesh;
mod stable_id;
mod state;

pub(crate) use self::build::*;
pub(crate) use self::csg_ops::*;
pub(crate) use self::extend_face::*;
pub(crate) use self::interaction::*;
pub(crate) use self::plane_math::*;
pub(crate) use self::preview_mesh::*;
pub(crate) use self::stable_id::*;
pub(crate) use self::state::*;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    let ext = ctx.id();
    // ConfirmDrawBrushOp: deferred (MouseButton::Left binding).
    ctx.spawn((
        Action::<ConfirmDrawBrushOp>::new(),
        ActionOf::<CoreExtensionInputContext>::new(ext),
        bindings![(MouseButton::Left, Press::default()),],
    ));
    // ActivateDrawBrushModalOp: deferred (MouseButton::Back binding present).
    ctx.spawn((
        Action::<ActivateDrawBrushModalOp>::new(),
        ActionOf::<CoreExtensionInputContext>::new(ext),
        bindings![
            (MouseButton::Back, Press::default()),
            (KeyCode::KeyB, Press::default()),
        ],
    ));
    // DrawBrushCancelCutOp: deferred (MouseButton::Right binding).
    ctx.spawn((
        Action::<DrawBrushCancelCutOp>::new(),
        ActionOf::<CoreExtensionInputContext>::new(ext),
        bindings![(MouseButton::Right, Press::default())],
    ));
    // StartDrawBrushAddAppendAction / StartDrawBrushCutAction: not operators; left unchanged.
    ctx.spawn((
        Action::<StartDrawBrushAddAppendAction>::new(),
        ActionOf::<CoreExtensionInputContext>::new(ext),
        bindings![(KeyCode::KeyB.with_mod_keys(ModKeys::ALT), Press::default(),)],
    ));
    ctx.spawn((
        Action::<StartDrawBrushCutAction>::new(),
        ActionOf::<CoreExtensionInputContext>::new(ext),
        bindings![(KeyCode::KeyC, Press::default())],
    ));

    ctx.bind_operator::<CoreExtensionInputContext, BrushJoinOp>([PresetInput::key("KeyJ")]);
    ctx.bind_operator::<CoreExtensionInputContext, BrushCsgSubtractOp>([
        PresetInput::key("KeyK").ctrl()
    ]);
    ctx.bind_operator::<CoreExtensionInputContext, BrushCsgIntersectOp>([PresetInput::key("KeyK")
        .ctrl()
        .shift()]);
    ctx.bind_operator::<CoreExtensionInputContext, BrushExtendFaceToBrushOp>([PresetInput::key(
        "KeyE",
    )
    .ctrl()]);
    ctx.bind_operator::<CoreExtensionInputContext, DrawBrushToggleModeOp>([PresetInput::key(
        "Tab",
    )]);
    ctx.bind_operator::<CoreExtensionInputContext, DrawBrushCommitPolygonOp>([PresetInput::key(
        "Enter",
    )]);
    ctx.bind_operator::<CoreExtensionInputContext, DrawBrushRemoveLastVertexOp>([
        PresetInput::key("Backspace"),
    ]);

    ctx.register_operator::<ActivateDrawBrushModalOp>()
        .register_operator::<AddBrushOp>()
        .register_operator::<ConfirmDrawBrushOp>()
        .register_operator::<BrushJoinOp>()
        .register_operator::<BrushCsgSubtractOp>()
        .register_operator::<BrushCsgIntersectOp>()
        .register_operator::<BrushExtendFaceToBrushOp>()
        .register_operator::<DrawBrushToggleModeOp>()
        .register_operator::<DrawBrushCommitPolygonOp>()
        .register_operator::<DrawBrushRemoveLastVertexOp>()
        .register_operator::<DrawBrushCancelCutOp>()
        .register_menu_entry::<ActivateDrawBrushModalOp>(TopLevelMenu::Add);

    ctx.init_resource::<DrawBrushState>()
        .init_resource::<StableIdCounter>();
}

/// Draw a new brush in the viewport.
#[operator(
    id = "viewport.draw_brush_modal",
    label = "Draw Brush",
    cancel = cancel_draw_brush_modal,
    modal = true,
    params(
        mode(String, default = "Add", doc = "Draw mode: \"Add\" or \"Cut\"."),
        append(bool, default = false, doc = "When true and mode = Add, fold the new brush into the selected one."),
    ),
)]
pub fn activate_draw_brush_modal(
    params: In<OperatorParameters>,
    mut input_focus: ResMut<InputFocus>,
    mut draw_state: ResMut<DrawBrushState>,
    mut edit_mode: ResMut<crate::brush::EditMode>,
    mut brush_selection: ResMut<crate::brush::BrushSelection>,
    selection: Res<Selection>,
    brush_query: Query<(), With<Brush>>,
    active: ActiveModalQuery,
) -> OperatorResult {
    if !active.is_modal_running() {
        let mode = match params.as_str("mode") {
            Some("Cut") => DrawMode::Cut,
            _ => DrawMode::Add,
        };
        let append = params.as_bool("append").unwrap_or(false);
        let append_target = if mode == DrawMode::Add && append {
            selection.primary().filter(|&e| brush_query.contains(e))
        } else {
            None
        };
        input_focus.0 = None;

        // Exit brush edit mode if active
        if *edit_mode != crate::brush::EditMode::Object {
            *edit_mode = crate::brush::EditMode::Object;
            brush_selection.clear();
        }

        draw_state.active = Some(ActiveDraw {
            corner1: Vec3::ZERO,
            corner2: Vec3::ZERO,
            depth: 0.0,
            phase: DrawPhase::PlacingFirstCorner,
            mode,
            plane: DrawPlane {
                origin: Vec3::ZERO,
                normal: Vec3::Y,
                axis_u: Vec3::X,
                axis_v: Vec3::Z,
            },
            extrude_start_cursor: Vec2::ZERO,
            plane_locked: false,
            cursor_on_plane: None,
            append_target,
            drag_footprint: false,
            press_screen_pos: None,
            polygon_vertices: Vec::new(),
            polygon_cursor: None,
            diagonal_snap: false,
            cached_face_hit: None,
            // The viewport that owns this modal is captured the first
            // time a per-frame system needs to bind to one (typically
            // when the user places the first corner). Until then the
            // modal hovers on whatever viewport the cursor is over.
            camera: None,
            viewport: None,
        });
    }
    if draw_state.active.is_none() {
        return OperatorResult::Finished;
    }
    OperatorResult::Running
}

fn cancel_draw_brush_modal(mut draw_state: ResMut<DrawBrushState>) {
    draw_state.active = None;
}

/// True only while a draw is in progress and the input field isn't
/// focused. Used as `is_available` for the in-modal keybinds.
fn is_drawing(keybind_focus: KeybindFocus, draw_state: Res<DrawBrushState>) -> bool {
    !keybind_focus.is_typing() && draw_state.active.is_some()
}

/// True while a draw's polygon is being placed (multi-vertex Add/Cut
/// before Enter commits the shape).
fn is_drawing_polygon(keybind_focus: KeybindFocus, draw_state: Res<DrawBrushState>) -> bool {
    if keybind_focus.is_typing() {
        return false;
    }
    draw_state
        .active
        .as_ref()
        .is_some_and(|a| a.phase == DrawPhase::DrawingPolygon)
}

/// True while a Cut-mode draw is in progress. Cut doesn't go through
/// the modal-finalize path on cancel, so it gets its own RMB binding.
fn is_drawing_cut(keybind_focus: KeybindFocus, draw_state: Res<DrawBrushState>) -> bool {
    if keybind_focus.is_typing() {
        return false;
    }
    draw_state
        .active
        .as_ref()
        .is_some_and(|a| a.mode == DrawMode::Cut)
}

/// Flip the in-progress draw between Add and Cut.
#[operator(
    id = "viewport.draw_brush.toggle_mode",
    label = "Toggle Add/Cut",
    description = "Flip between adding and cutting while drawing.",
    is_available = is_drawing,
    allows_undo = false,
)]
pub(crate) fn draw_brush_toggle_mode(
    _: In<OperatorParameters>,
    mut draw_state: ResMut<DrawBrushState>,
) -> OperatorResult {
    let active = draw_state.active.as_mut()?;
    active.mode = match active.mode {
        DrawMode::Add => DrawMode::Cut,
        DrawMode::Cut => DrawMode::Add,
    };
    OperatorResult::Finished
}

/// Close the in-progress polygon (via convex hull) and switch to
/// extruding depth.
#[operator(
    id = "viewport.draw_brush.commit_polygon",
    label = "Commit Polygon",
    description = "Close the polygon and start extruding it.",
    is_available = is_drawing_polygon,
    allows_undo = false,
)]
pub(crate) fn draw_brush_commit_polygon(
    _: In<OperatorParameters>,
    mut draw_state: ResMut<DrawBrushState>,
    vp: ViewportCursor,
) -> OperatorResult {
    let active = draw_state.active.as_mut()?;
    let hull = convex_hull_on_plane(&active.polygon_vertices, &active.plane);
    if hull.len() < 3 {
        return OperatorResult::Cancelled;
    }
    active.polygon_vertices = hull;
    let viewport_cursor = (|| {
        let cursor_pos = vp.cursor()?;
        let camera_entity = active.camera.or_else(|| vp.camera_entity())?;
        let viewport_entity = active.viewport.or_else(|| vp.viewport_entity())?;
        let (camera, _) = vp.camera_for(camera_entity)?;
        vp.viewport_cursor_for(camera, viewport_entity, cursor_pos)
    })();
    active.phase = DrawPhase::ExtrudingDepth;
    active.extrude_start_cursor = viewport_cursor.unwrap_or(Vec2::ZERO);
    active.depth = 0.0;
    OperatorResult::Finished
}

/// Drop the last placed polygon vertex, falling back to first-corner
/// placement if the polygon is now empty.
#[operator(
    id = "viewport.draw_brush.remove_last_vertex",
    label = "Remove Last Vertex",
    description = "Take back the last polygon point you placed.",
    is_available = is_drawing_polygon,
    allows_undo = false,
)]
pub(crate) fn draw_brush_remove_last_vertex(
    _: In<OperatorParameters>,
    mut draw_state: ResMut<DrawBrushState>,
) -> OperatorResult {
    let active = draw_state.active.as_mut()?;
    active.polygon_vertices.pop();
    if active.polygon_vertices.is_empty() {
        active.phase = DrawPhase::PlacingFirstCorner;
    }
    OperatorResult::Finished
}

/// Cancel an in-progress Cut-mode draw. (Add-mode cancels through
/// `modal.cancel`, which routes to the modal's finalize path.)
#[operator(
    id = "viewport.draw_brush.cancel_cut",
    label = "Cancel Cut",
    description = "Bail out of the current cut.",
    is_available = is_drawing_cut,
    allows_undo = false,
)]
pub(crate) fn draw_brush_cancel_cut(
    _: In<OperatorParameters>,
    mut draw_state: ResMut<DrawBrushState>,
) -> OperatorResult {
    draw_state.active = None;
    OperatorResult::Finished
}

#[operator(
    id = "draw_brush.confirm",
    label = "Draw Brush (Confirm)",
    description = "Confirms the current draw brush operation",
    is_available = is_in_draw_brush_modal,
    allows_undo = false
)]
fn confirm_draw_brush(
    _: In<OperatorParameters>,
    mut draw_state: ResMut<DrawBrushState>,
    vp: ViewportCursor,
    mut commands: Commands,
) -> OperatorResult {
    let active = draw_state.active.as_mut()?;

    // Verify cursor is in viewport
    let cursor_pos = vp.cursor()?;
    let camera_entity = active.camera.or_else(|| vp.camera_entity());
    let viewport_entity = active.viewport.or_else(|| vp.viewport_entity());
    let (Some(camera_entity), Some(viewport_entity)) = (camera_entity, viewport_entity) else {
        return OperatorResult::Cancelled;
    };
    let (camera, _) = vp.camera_for(camera_entity)?;
    let viewport_cursor = vp.viewport_cursor_for(camera, viewport_entity, cursor_pos)?;

    match active.phase {
        DrawPhase::PlacingFirstCorner => {
            if let Some(pos) = active.cursor_on_plane {
                active.corner1 = pos;
                active.corner2 = pos;
                active.phase = DrawPhase::DrawingFootprint;
                active.drag_footprint = true;
                active.press_screen_pos = Some(cursor_pos);
            }
        }
        DrawPhase::DrawingFootprint => {
            if active.drag_footprint {
                return OperatorResult::Cancelled;
            }
            let delta = active.corner2 - active.corner1;
            if delta.dot(active.plane.axis_u).abs() < MIN_FOOTPRINT_SIZE
                || delta.dot(active.plane.axis_v).abs() < MIN_FOOTPRINT_SIZE
            {
                return OperatorResult::Cancelled;
            }
            active.phase = DrawPhase::ExtrudingDepth;
            active.extrude_start_cursor = viewport_cursor;
            active.depth = 0.0;
        }
        DrawPhase::DrawingRotatedWidth => {
            if active.polygon_vertices.len() == 4 {
                let edge1 = (active.polygon_vertices[1] - active.polygon_vertices[0]).length();
                let edge2 = (active.polygon_vertices[3] - active.polygon_vertices[0]).length();
                if edge1 >= MIN_FOOTPRINT_SIZE && edge2 >= MIN_FOOTPRINT_SIZE {
                    active.phase = DrawPhase::ExtrudingDepth;
                    active.extrude_start_cursor = viewport_cursor;
                    active.depth = 0.0;
                }
            }
        }
        DrawPhase::DrawingPolygon => {
            if let Some(cursor) = active.polygon_cursor {
                // Accept all vertices, but skip near-duplicates
                let too_close = active
                    .polygon_vertices
                    .iter()
                    .any(|&v| (v - cursor).length() < 0.05);
                if !too_close {
                    active.polygon_vertices.push(cursor);
                }
            }
        }
        DrawPhase::ExtrudingDepth => {
            if active.depth.abs() < MIN_EXTRUDE_DEPTH {
                return OperatorResult::Cancelled; // No depth, keep extruding
            }
            let active = active.clone();
            draw_state.active = None;
            match active.mode {
                DrawMode::Add => {
                    if !active.polygon_vertices.is_empty() {
                        if active.append_target.is_some() {
                            append_to_brush(&active, &mut commands);
                        } else {
                            spawn_polygon_brush(&active, &mut commands);
                        }
                    } else if active.append_target.is_some() {
                        append_to_brush(&active, &mut commands);
                    } else {
                        spawn_drawn_brush(&active, &mut commands);
                    }
                }
                DrawMode::Cut => {
                    subtract_drawn_brush(&active, &mut commands);
                }
            }
        }
    }
    OperatorResult::Finished
}

fn is_in_draw_brush_modal(active: ActiveModalQuery) -> bool {
    active.is_operator(ActivateDrawBrushModalOp::ID)
}

#[operator(id = "mesh.add_brush")]
pub fn add_brush(_params: In<OperatorParameters>) -> OperatorResult {
    // TODO: make this add / finalize the geometry that was previewed by the draw model
    // The reason for this operator to exist is to be called by user extensions.
    OperatorResult::Finished
}

pub(crate) const EXTRUDE_DEPTH_SENSITIVITY: f32 = 0.003;
pub(crate) const MIN_FOOTPRINT_SIZE: f32 = 0.01;
pub(crate) const MIN_EXTRUDE_DEPTH: f32 = 0.01;
pub(crate) const MIN_FRAGMENT_SIZE: f32 = 0.005;
/// Punch-through depth used by box-cut subtract: large enough to traverse any
/// reasonably-sized target so the user never needs to drag for depth.
/// Matches BoxCutter-style default behavior.
pub(crate) const PUNCH_THROUGH_DEPTH: f32 = 1000.0;

#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct DrawBrushGizmoGroup;

pub struct DrawBrushPlugin;

impl Plugin for DrawBrushPlugin {
    fn build(&self, app: &mut App) {
        // TODO: Move *all* of this into the `extension` method and turn systems into ops on the way.
        app.register_type::<BrushStableId>()
            .init_resource::<StableIdCounter>()
            .add_systems(Update, assign_missing_brush_stable_ids)
            .init_gizmo_group::<DrawBrushGizmoGroup>()
            .add_systems(Startup, configure_draw_brush_gizmos)
            .add_systems(
                Update,
                (draw_brush_update, draw_brush_release, draw_brush_confirm)
                    .chain()
                    .in_set(crate::EditorInteractionSystems),
            )
            .add_systems(
                Update,
                (
                    draw_brush_preview.after(draw_brush_confirm),
                    manage_draw_preview_mesh.after(crate::brush::mesh::regenerate_brush_meshes),
                )
                    .run_if(in_state(crate::AppState::Editor)),
            )
            .add_observer(dispatch_start_add_append)
            .add_observer(dispatch_start_cut);
    }
}

fn configure_draw_brush_gizmos(mut config_store: ResMut<GizmoConfigStore>) {
    let (config, _) = config_store.config_mut::<DrawBrushGizmoGroup>();
    config.depth_bias = -1.0;
}

/// Marker action: Alt+B starts a draw that appends the new brush to
/// the selected one. Observed by [`dispatch_start_add_append`] which
/// fires `viewport.draw_brush_modal` with `append=true`.
#[derive(Default, InputAction)]
#[action_output(bool)]
pub(crate) struct StartDrawBrushAddAppendAction;

/// Marker action: C starts a Cut-mode draw. Observed by
/// [`dispatch_start_cut`] which fires `viewport.draw_brush_modal` with
/// `mode="Cut"`.
#[derive(Default, InputAction)]
#[action_output(bool)]
pub(crate) struct StartDrawBrushCutAction;

fn dispatch_start_add_append(_: On<Start<StartDrawBrushAddAppendAction>>, mut commands: Commands) {
    commands
        .operator(ActivateDrawBrushModalOp::ID)
        .param("mode", "Add")
        .param("append", true)
        .call();
}

fn dispatch_start_cut(
    _: On<Start<StartDrawBrushCutAction>>,
    edit_mode: Res<crate::brush::EditMode>,
    mut commands: Commands,
) {
    // In a brush edit sub-mode, C opens the mesh quick-menu instead. Starting a
    // cut brush there would force Object mode and pull the user out of the edit
    // they are in; the cut gesture still works from object mode.
    if matches!(*edit_mode, crate::brush::EditMode::BrushEdit(_)) {
        return;
    }
    commands
        .operator(ActivateDrawBrushModalOp::ID)
        .param("mode", "Cut")
        .call();
}
