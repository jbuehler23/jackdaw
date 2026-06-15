//! Numeric transform entry: type an axis and a number to apply a delta
//! transform along a world axis, to an object selection or to edit-mode
//! sub-elements (vertices / edges / faces) across every edit brush.
//!
//! With something selected, X / Y / Z arm an axis; digits, a decimal point,
//! and a leading minus accumulate; Enter applies and Escape cancels. The
//! delta by tool:
//!
//! - Translate: move by `value` world units along the axis.
//! - Rotate: rotate by `value` degrees about the axis, around the pivot.
//! - Scale: scale by `value` along the axis, about the pivot.
//!
//! Select applies a translate. An armed axis also constrains a direct
//! viewport drag of an object selection, with the same projection as the
//! gizmo handle drag, and clears when the drag ends.
//!
//! The input system only gates and accumulates; `transform.numeric_apply`
//! applies the delta as a single undo entry.
//!
//! Typed digit / decimal / minus input is raw text input and is deliberately
//! outside the keymap engine; it is not preset-bindable.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use jackdaw_api::prelude::*;

use crate::active_tool::ActiveTool;
use crate::brush::{BrushSelection, BrushSubSelection, EditMode};
use crate::brush_drag_ops::{
    apply_vertex_deltas, broadcast_drag_to_captures, capture_edit_brushes,
};
use crate::default_style;
use crate::gizmos::{GizmoAxis, GizmoDragState};
use crate::keybind_focus::KeybindFocus;
use crate::modal_transform::{ModalTransformState, ViewportDragState};
use crate::selection::{Selected, Selection};

/// Floor for any single scale component, matching the gizmo drag.
const MIN_SCALE: f32 = 0.01;

/// Half-length of the numeric-entry axis guide line, matching the brush drag
/// constraint line.
const AXIS_GUIDE_HALF_LENGTH: f32 = 50.0;

/// Active numeric transform entry. `axis` is `Some` while an entry is in
/// progress (only X / Y / Z, never `Uniform`); `input` is the accumulated
/// text such as `"-12.5"`.
#[derive(Resource, Default)]
pub struct NumericTransformState {
    pub axis: Option<GizmoAxis>,
    pub input: String,
}

impl NumericTransformState {
    /// End the current entry.
    pub(crate) fn clear(&mut self) {
        self.axis = None;
        self.input.clear();
    }

    /// Parse the accumulated text as a delta value. A bare sign or empty
    /// string does not parse.
    fn value(&self) -> Option<f32> {
        self.input.parse::<f32>().ok()
    }
}

pub struct NumericTransformPlugin;

impl Plugin for NumericTransformPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NumericTransformState>().add_systems(
            Update,
            (
                numeric_transform_input.in_set(crate::EditorInteractionSystems),
                draw_numeric_axis_guide,
            ),
        );
    }
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<NumericTransformApplyOp>();
}

/// World unit vector for a numeric-entry axis. `Uniform` is never stored in
/// the entry state, so it maps to zero defensively.
pub(crate) fn axis_direction(axis: GizmoAxis) -> Vec3 {
    match axis {
        GizmoAxis::X => Vec3::X,
        GizmoAxis::Y => Vec3::Y,
        GizmoAxis::Z => Vec3::Z,
        GizmoAxis::Uniform => Vec3::ZERO,
    }
}

/// Map a digit / decimal key to the character it contributes, or `None` for
/// any other key. Numpad and top-row digits both map to their digit; the
/// decimal keys map to '.'.
fn digit_char(key: KeyCode) -> Option<char> {
    match key {
        KeyCode::Digit0 | KeyCode::Numpad0 => Some('0'),
        KeyCode::Digit1 | KeyCode::Numpad1 => Some('1'),
        KeyCode::Digit2 | KeyCode::Numpad2 => Some('2'),
        KeyCode::Digit3 | KeyCode::Numpad3 => Some('3'),
        KeyCode::Digit4 | KeyCode::Numpad4 => Some('4'),
        KeyCode::Digit5 | KeyCode::Numpad5 => Some('5'),
        KeyCode::Digit6 | KeyCode::Numpad6 => Some('6'),
        KeyCode::Digit7 | KeyCode::Numpad7 => Some('7'),
        KeyCode::Digit8 | KeyCode::Numpad8 => Some('8'),
        KeyCode::Digit9 | KeyCode::Numpad9 => Some('9'),
        KeyCode::Period | KeyCode::NumpadDecimal => Some('.'),
        _ => None,
    }
}

/// The axis a freshly pressed key selects, if any. Bare X / Y / Z only:
/// a modified press is some other binding (Ctrl+Z undo, Ctrl+Y redo,
/// Alt+Z x-ray) and must not arm an axis.
fn pressed_axis(keyboard: &ButtonInput<KeyCode>) -> Option<GizmoAxis> {
    let modifier = keyboard.any_pressed([
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
        KeyCode::AltLeft,
        KeyCode::AltRight,
    ]);
    if modifier {
        return None;
    }
    if keyboard.just_pressed(KeyCode::KeyX) {
        Some(GizmoAxis::X)
    } else if keyboard.just_pressed(KeyCode::KeyY) {
        Some(GizmoAxis::Y)
    } else if keyboard.just_pressed(KeyCode::KeyZ) {
        Some(GizmoAxis::Z)
    } else {
        None
    }
}

/// Toggle a leading '-' on the accumulated text.
fn toggle_sign(input: &mut String) {
    if let Some(stripped) = input.strip_prefix('-') {
        *input = stripped.to_string();
    } else {
        input.insert(0, '-');
    }
}

/// True when there is a target the numeric entry can act on: a primary object
/// selection in object mode, or a non-empty sub-selection in brush-edit mode.
/// Works with any tool, including Select (which applies a translate).
pub(crate) fn numeric_entry_eligible(
    edit_mode: &EditMode,
    selection: &Selection,
    brush_selection: &BrushSelection,
) -> bool {
    match edit_mode {
        EditMode::Object => selection.primary().is_some(),
        EditMode::BrushEdit(_) => brush_selection
            .brushes
            .values()
            .any(|s| !s.vertices.is_empty() || !s.edges.is_empty() || !s.faces.is_empty()),
        EditMode::Physics => false,
    }
}

/// Gate, start, accumulate, and dispatch numeric transform entry. Holds no
/// behaviour beyond editing [`NumericTransformState`] and calling the apply
/// operator; the transform itself lives in `numeric_transform_apply`.
fn numeric_transform_input(
    keyboard: Res<ButtonInput<KeyCode>>,
    edit_mode: Res<EditMode>,
    selection: Res<Selection>,
    brush_selection: Res<BrushSelection>,
    keybind_focus: KeybindFocus,
    modal: Res<ModalTransformState>,
    gizmo_drag: Res<GizmoDragState>,
    edit_gizmo_active: Res<crate::gizmos::EditGizmoDragState>,
    viewport_drag: Res<ViewportDragState>,
    mut state: ResMut<NumericTransformState>,
    mut commands: Commands,
) {
    // Drop the armed axis entirely when the context can no longer host an
    // entry: text focus, a running modal op, or no eligible target.
    let hard_reset = keybind_focus.is_typing()
        || modal.active.is_some()
        || !numeric_entry_eligible(&edit_mode, &selection, &brush_selection);
    if hard_reset {
        if state.axis.is_some() {
            state.clear();
        }
        return;
    }

    // While a drag runs the armed axis is consumed by the drag to constrain
    // it (see `viewport_drag_update`); keep it armed, but do not accumulate
    // digits or start a new entry until the drag ends.
    if gizmo_drag.active
        || edit_gizmo_active.active
        || viewport_drag.active.is_some()
        || viewport_drag.pending.is_some()
    {
        return;
    }

    let Some(axis) = state.axis else {
        if let Some(axis) = pressed_axis(&keyboard) {
            state.axis = Some(axis);
            state.input.clear();
        }
        return;
    };

    // Switching axis keeps the accumulated number.
    if let Some(new_axis) = pressed_axis(&keyboard) {
        if new_axis != axis {
            state.axis = Some(new_axis);
        }
        return;
    }

    if keyboard.just_pressed(KeyCode::Escape) {
        state.clear();
        return;
    }

    if keyboard.just_pressed(KeyCode::Enter) || keyboard.just_pressed(KeyCode::NumpadEnter) {
        // Apply only when the text parses; an unparseable entry is ignored
        // so a stray Enter does not clear in-progress input.
        if state.value().is_some() {
            commands.queue(|world: &mut World| {
                let _ = world
                    .operator(NumericTransformApplyOp::ID)
                    .settings(CallOperatorSettings {
                        execution_context: ExecutionContext::Invoke,
                        creates_history_entry: true,
                    })
                    .call();
            });
        }
        return;
    }

    if keyboard.just_pressed(KeyCode::Backspace) {
        if state.input.pop().is_none() {
            state.clear();
        }
        return;
    }

    if keyboard.just_pressed(KeyCode::Minus) || keyboard.just_pressed(KeyCode::NumpadSubtract) {
        toggle_sign(&mut state.input);
        return;
    }

    for key in keyboard.get_just_pressed() {
        if let Some(c) = digit_char(*key) {
            // Only one decimal point.
            if c == '.' && state.input.contains('.') {
                continue;
            }
            state.input.push(c);
            break;
        }
    }
}

/// Draw a guide line through the selection pivot along the armed axis while a
/// numeric entry is active: red / green / blue for X / Y / Z, through the
/// object selection's world centroid or the selected sub-elements' centroid
/// (the pivot the apply transforms about).
fn draw_numeric_axis_guide(
    state: Res<NumericTransformState>,
    edit_mode: Res<EditMode>,
    selection: Res<Selection>,
    parents: Query<&ChildOf>,
    selected_globals: Query<&GlobalTransform, With<Selected>>,
    brush_selection: Res<BrushSelection>,
    brush_caches: Query<&crate::brush::BrushMeshCache>,
    brush_globals: Query<&GlobalTransform>,
    mut gizmos: Gizmos,
) {
    let Some(axis) = state.axis else {
        return;
    };
    let (axis_dir, color) = match axis {
        GizmoAxis::X => (Vec3::X, default_style::AXIS_X),
        GizmoAxis::Y => (Vec3::Y, default_style::AXIS_Y),
        GizmoAxis::Z => (Vec3::Z, default_style::AXIS_Z),
        GizmoAxis::Uniform => return,
    };

    let pivot = match *edit_mode {
        EditMode::Object => {
            let positions: Vec<Vec3> = topmost_selected(&selection, &parents)
                .iter()
                .filter_map(|&e| {
                    selected_globals
                        .get(e)
                        .ok()
                        .map(GlobalTransform::translation)
                })
                .collect();
            if positions.is_empty() {
                return;
            }
            centroid(&positions)
        }
        EditMode::BrushEdit(_) => {
            let mut positions: Vec<Vec3> = Vec::new();
            let edit_brushes: Vec<Entity> = brush_selection.edit_brushes().collect();
            for entity in edit_brushes {
                let Ok(cache) = brush_caches.get(entity) else {
                    continue;
                };
                let Ok(global) = brush_globals.get(entity) else {
                    continue;
                };
                let Some(sub) = brush_selection.sub(entity) else {
                    continue;
                };
                for vi in sub_selection_vertices(sub, &cache.face_polygons) {
                    if let Some(v) = cache.vertices.get(vi) {
                        positions.push(global.transform_point(*v));
                    }
                }
            }
            if positions.is_empty() {
                return;
            }
            centroid(&positions)
        }
        EditMode::Physics => return,
    };

    gizmos.line(
        pivot - axis_dir * AXIS_GUIDE_HALF_LENGTH,
        pivot + axis_dir * AXIS_GUIDE_HALF_LENGTH,
        color,
    );
}

/// Topology vertex indices a sub-selection touches: the union of its
/// vertices, both ends of every edge, and every vertex of each selected face
/// polygon. Mirrors `gizmos::selected_sub_vertices`, kept local here.
fn sub_selection_vertices(sub: &BrushSubSelection, face_polygons: &[Vec<usize>]) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    let push = |v: usize, out: &mut Vec<usize>| {
        if !out.contains(&v) {
            out.push(v);
        }
    };
    for &v in &sub.vertices {
        push(v, &mut out);
    }
    for &(a, b) in &sub.edges {
        push(a, &mut out);
        push(b, &mut out);
    }
    for &f in &sub.faces {
        if let Some(polygon) = face_polygons.get(f) {
            for &v in polygon {
                push(v, &mut out);
            }
        }
    }
    out
}

/// Mean of a set of positions; the origin for an empty set.
fn centroid(positions: &[Vec3]) -> Vec3 {
    if positions.is_empty() {
        return Vec3::ZERO;
    }
    positions.iter().copied().sum::<Vec3>() / positions.len() as f32
}

/// Entities to transform as a group: selected entities minus any whose
/// ancestor is also selected, so a child moves once via its parent.
fn topmost_selected(selection: &Selection, parents: &Query<&ChildOf>) -> Vec<Entity> {
    selection
        .entities
        .iter()
        .copied()
        .filter(|&e| {
            let mut cur = parents.get(e).ok().map(|c| c.0);
            while let Some(a) = cur {
                if selection.entities.contains(&a) {
                    return false;
                }
                cur = parents.get(a).ok().map(|c| c.0);
            }
            true
        })
        .collect()
}

/// Brush-geometry queries the apply operator mutates for the edit-mode path.
/// Bundled to mirror `EditGizmoBrushParams` and keep the operator under
/// Bevy's system-param ceiling.
#[derive(SystemParam)]
struct NumericBrushParams<'w, 's> {
    caches: Query<'w, 's, &'static crate::brush::BrushMeshCache>,
    globals: Query<'w, 's, &'static GlobalTransform>,
    brushes: Query<'w, 's, &'static mut jackdaw_jsn::Brush>,
    halfedges: Query<'w, 's, &'static mut crate::brush::BrushHalfedge>,
    mirrors: Query<'w, 's, &'static jackdaw_geometry::ModifierStack>,
}

#[operator(
    id = "transform.numeric_apply",
    label = "Apply Numeric Transform",
    description = "Apply the in-progress numeric transform: a delta translate \
                   / rotate / scale along the chosen axis by the typed amount, \
                   to the object selection or edit-mode sub-elements.",
    allows_undo = true
)]
pub fn numeric_transform_apply(
    _: In<OperatorParameters>,
    mut state: ResMut<NumericTransformState>,
    mode: Res<ActiveTool>,
    edit_mode: Res<EditMode>,
    selection: Res<Selection>,
    parents: Query<&ChildOf>,
    transforms: Query<&mut Transform, With<Selected>>,
    brush_selection: Res<BrushSelection>,
    brush_params: NumericBrushParams,
) -> OperatorResult {
    let Some(axis) = state.axis else {
        return OperatorResult::Cancelled;
    };
    let Some(value) = state.value() else {
        return OperatorResult::Cancelled;
    };
    let axis_dir = axis_direction(axis);

    // Select has no transform of its own, so a numeric entry in Select mode
    // applies a translate.
    let op = if matches!(*mode, ActiveTool::Select) {
        ActiveTool::Translate
    } else {
        *mode
    };

    match *edit_mode {
        EditMode::Object => {
            apply_object(&op, axis_dir, value, &selection, &parents, transforms);
        }
        EditMode::BrushEdit(_) => {
            apply_sub_elements(&op, axis_dir, value, &brush_selection, brush_params);
        }
        EditMode::Physics => {}
    }

    state.clear();
    OperatorResult::Finished
}

/// Object-mode apply: mirror the `gizmo_drag` arms about the selection's
/// world / local pivots.
fn apply_object(
    mode: &ActiveTool,
    axis_dir: Vec3,
    value: f32,
    selection: &Selection,
    parents: &Query<&ChildOf>,
    mut transforms: Query<&mut Transform, With<Selected>>,
) {
    let targets = topmost_selected(selection, parents);
    let local_positions: Vec<Vec3> = targets
        .iter()
        .filter_map(|&e| transforms.get(e).ok().map(|t| t.translation))
        .collect();
    if local_positions.is_empty() {
        return;
    }
    let pivot_local = centroid(&local_positions);

    match mode {
        ActiveTool::Translate => {
            let delta = axis_dir * value;
            for &e in &targets {
                if let Ok(mut tf) = transforms.get_mut(e) {
                    tf.translation += delta;
                }
            }
        }
        ActiveTool::Rotate => {
            let r = Quat::from_axis_angle(axis_dir, value.to_radians());
            for &e in &targets {
                if let Ok(mut tf) = transforms.get_mut(e) {
                    tf.translation = pivot_local + r * (tf.translation - pivot_local);
                    tf.rotation = r * tf.rotation;
                }
            }
        }
        ActiveTool::Scale => {
            let mut factor = Vec3::ONE;
            match axis_dir {
                d if d == Vec3::X => factor.x = value,
                d if d == Vec3::Y => factor.y = value,
                d if d == Vec3::Z => factor.z = value,
                _ => {}
            }
            for &e in &targets {
                if let Ok(mut tf) = transforms.get_mut(e) {
                    tf.translation = pivot_local + factor * (tf.translation - pivot_local);
                    tf.scale = (tf.scale * factor).max(Vec3::splat(MIN_SCALE));
                }
            }
        }
        ActiveTool::Select => {}
    }
}

/// Brush-edit apply: transform the selected sub-elements across every edit
/// brush about their shared world centroid, reusing the gizmo edit-drag math.
fn apply_sub_elements(
    mode: &ActiveTool,
    axis_dir: Vec3,
    value: f32,
    brush_selection: &BrushSelection,
    mut brush_params: NumericBrushParams,
) {
    let captures = capture_edit_brushes(
        brush_selection,
        &brush_params.brushes,
        &brush_params.caches,
        &brush_params.globals,
        sub_selection_vertices,
    );
    if captures.is_empty() {
        return;
    }
    let pivot = centroid(
        &captures
            .iter()
            .flat_map(|c| c.start_world.iter().copied())
            .collect::<Vec<_>>(),
    );

    match mode {
        ActiveTool::Translate => {
            broadcast_drag_to_captures(
                &captures,
                axis_dir * value,
                &mut brush_params.brushes,
                &mut brush_params.halfedges,
                &brush_params.mirrors,
            );
        }
        ActiveTool::Rotate => {
            let r = Quat::from_axis_angle(axis_dir, value.to_radians());
            apply_capture_plan(&captures, &mut brush_params, |w| pivot + r * (w - pivot));
        }
        ActiveTool::Scale => {
            let mut factor = Vec3::ONE;
            match axis_dir {
                d if d == Vec3::X => factor.x = value,
                d if d == Vec3::Y => factor.y = value,
                d if d == Vec3::Z => factor.z = value,
                _ => {}
            }
            apply_capture_plan(&captures, &mut brush_params, |w| {
                pivot + factor * (w - pivot)
            });
        }
        ActiveTool::Select => {}
    }
}

/// Map each capture's start world positions through `world_map`, convert back
/// to that brush's local space, and rebuild via `apply_vertex_deltas`. The
/// per-brush plan is built from immutable capture data first, then written, so
/// the mutable `Brush` / `BrushHalfedge` borrows do not overlap the reads.
fn apply_capture_plan(
    captures: &[crate::brush::BrushDragCapture],
    brush_params: &mut NumericBrushParams,
    world_map: impl Fn(Vec3) -> Vec3,
) {
    let plan: Vec<(Entity, Vec<Vec3>)> = captures
        .iter()
        .map(|capture| {
            let new_local: Vec<Vec3> = capture
                .start_world
                .iter()
                .map(|&w| capture.start_world_to_local.transform_point3(world_map(w)))
                .collect();
            (capture.entity, new_local)
        })
        .collect();

    for (entity, new_local) in plan {
        let Some(capture) = captures.iter().find(|c| c.entity == entity) else {
            continue;
        };
        let Ok(mut brush) = brush_params.brushes.get_mut(entity) else {
            continue;
        };
        let mut halfedge_opt = brush_params.halfedges.get_mut(entity).ok();
        apply_vertex_deltas(
            &mut brush,
            halfedge_opt.as_deref_mut(),
            brush_params
                .mirrors
                .get(entity)
                .ok()
                .and_then(|stack| stack.first_enabled_mirror()),
            &capture.start_brush,
            &capture.start_all_vertices,
            &capture.start_face_polygons,
            &capture.indices,
            &new_local,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digit_char_maps_top_row_and_numpad() {
        assert_eq!(digit_char(KeyCode::Digit0), Some('0'));
        assert_eq!(digit_char(KeyCode::Digit9), Some('9'));
        assert_eq!(digit_char(KeyCode::Numpad0), Some('0'));
        assert_eq!(digit_char(KeyCode::Numpad5), Some('5'));
        assert_eq!(digit_char(KeyCode::Period), Some('.'));
        assert_eq!(digit_char(KeyCode::NumpadDecimal), Some('.'));
        assert_eq!(digit_char(KeyCode::KeyX), None);
        assert_eq!(digit_char(KeyCode::Enter), None);
    }

    #[test]
    fn value_parses_signed_decimals_and_rejects_bare_signs() {
        let value_of = |text: &str| {
            NumericTransformState {
                axis: Some(GizmoAxis::X),
                input: text.to_string(),
            }
            .value()
        };
        assert_eq!(value_of("12"), Some(12.0));
        assert_eq!(value_of("-12.5"), Some(-12.5));
        assert_eq!(value_of("0.25"), Some(0.25));
        assert_eq!(value_of("-"), None);
        assert_eq!(value_of(""), None);
        assert_eq!(value_of("."), None);
    }

    #[test]
    fn toggle_sign_adds_and_removes_leading_minus() {
        let mut s = "5".to_string();
        toggle_sign(&mut s);
        assert_eq!(s, "-5");
        toggle_sign(&mut s);
        assert_eq!(s, "5");
        let mut empty = String::new();
        toggle_sign(&mut empty);
        assert_eq!(empty, "-");
    }

    #[test]
    fn clear_ends_entry() {
        let mut state = NumericTransformState {
            axis: Some(GizmoAxis::Y),
            input: "3.5".to_string(),
        };
        state.clear();
        assert!(state.axis.is_none());
        assert!(state.input.is_empty());
    }

    #[test]
    fn pressed_axis_arms_on_bare_key_and_ignores_modified_presses() {
        let mut keyboard = ButtonInput::<KeyCode>::default();

        keyboard.press(KeyCode::KeyZ);
        assert_eq!(pressed_axis(&keyboard), Some(GizmoAxis::Z));

        // Ctrl+Z is undo: it must not arm the Z axis.
        keyboard.press(KeyCode::ControlLeft);
        assert_eq!(pressed_axis(&keyboard), None);
        keyboard.release(KeyCode::ControlLeft);

        // Alt+Z is the x-ray toggle: also no axis.
        keyboard.press(KeyCode::AltLeft);
        assert_eq!(pressed_axis(&keyboard), None);
        keyboard.release(KeyCode::AltLeft);

        keyboard.clear();
        keyboard.press(KeyCode::KeyX);
        assert_eq!(pressed_axis(&keyboard), Some(GizmoAxis::X));
    }

    #[test]
    fn axis_direction_maps_to_unit_vectors() {
        assert_eq!(axis_direction(GizmoAxis::X), Vec3::X);
        assert_eq!(axis_direction(GizmoAxis::Y), Vec3::Y);
        assert_eq!(axis_direction(GizmoAxis::Z), Vec3::Z);
        assert_eq!(axis_direction(GizmoAxis::Uniform), Vec3::ZERO);
    }

    #[test]
    fn sub_selection_vertices_unions_and_dedups() {
        let face_polygons = vec![vec![0, 1, 2], vec![2, 1, 3]];
        let sub = BrushSubSelection {
            vertices: vec![1],
            edges: vec![(1, 2)],
            faces: vec![0, 1],
        };
        // vertices: 1; edge (1,2): 2; face 0: 0; face 1 (2,1,3): 3.
        assert_eq!(
            sub_selection_vertices(&sub, &face_polygons),
            vec![1, 2, 0, 3]
        );
    }
}
