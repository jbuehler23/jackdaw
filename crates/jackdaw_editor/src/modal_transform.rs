use bevy::{
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings, RayCastVisibility},
    prelude::*,
    ui::UiGlobalTransform,
    window::{CursorGrabMode, CursorOptions},
};

use crate::{
    commands::{CommandHistory, SetTransform},
    gizmos::{GizmoAxis, GizmoDragState, GizmoHoverState, GizmoMode},
    selection::Selection,
    snapping::SnapSettings,
    viewport::{MainViewportCamera, SceneViewport},
    viewport_util::window_to_viewport_cursor,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModalOp {
    Grab,
    Rotate,
    Scale,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ModalConstraint {
    #[default]
    Free,
    Axis(GizmoAxis),
    /// Constrains to a plane by excluding this axis.
    Plane(GizmoAxis),
}

#[derive(Resource, Debug, Default)]
pub struct ModalTransformState {
    pub active: Option<ActiveModal>,
}

#[derive(Debug)]
pub struct ActiveModal {
    pub op: ModalOp,
    pub entity: Entity,
    pub start_transform: Transform,
    pub constraint: ModalConstraint,
    pub start_cursor: Vec2,
}

#[derive(Resource, Default)]
pub struct ViewportDragState {
    pub pending: Option<PendingDrag>,
    pub active: Option<ActiveDrag>,
}

pub struct PendingDrag {
    pub entity: Entity,
    pub start_transform: Transform,
    pub click_pos: Vec2,
    /// Viewport-local cursor position at drag start.
    pub start_viewport_cursor: Vec2,
}

pub struct ActiveDrag {
    pub entity: Entity,
    pub start_transform: Transform,
    /// Viewport-local cursor position at drag start.
    pub start_viewport_cursor: Vec2,
}

pub struct ModalTransformPlugin;

impl Plugin for ModalTransformPlugin {
    fn build(&self, app: &mut App) {
        // ModalTransformState is kept so other systems can check `modal.active.is_some()`.
        // Modal activate/constrain/update/confirm/cancel/draw systems are disabled
        // (G/R/S no longer trigger modal transforms, TrenchBroom-style keybinds instead.)
        // The code is preserved in this file for a future Blender keymap option.
        app.init_resource::<ModalTransformState>()
            .init_resource::<ViewportDragState>()
            .add_systems(
                Update,
                (
                    snap_toggle,
                    viewport_drag_detect.after(crate::viewport_select::handle_viewport_click),
                    viewport_drag_update,
                    viewport_drag_finish,
                )
                    .chain()
                    .in_set(crate::EditorInteractionSystems),
            );
    }
}

fn snap_toggle(
    mouse: Res<ButtonInput<MouseButton>>,
    mode: Res<GizmoMode>,
    modal: Res<ModalTransformState>,
    mut snap_settings: ResMut<SnapSettings>,
) {
    if modal.active.is_some() {
        return;
    }

    if mouse.just_pressed(MouseButton::Middle) {
        match *mode {
            GizmoMode::Translate => snap_settings.translate_snap = !snap_settings.translate_snap,
            GizmoMode::Rotate => snap_settings.rotate_snap = !snap_settings.rotate_snap,
            GizmoMode::Scale => snap_settings.scale_snap = !snap_settings.scale_snap,
        }
    }
}

fn viewport_drag_detect(
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    selection: Res<Selection>,
    transforms: Query<(&GlobalTransform, &Transform)>,
    gizmo_drag: Res<GizmoDragState>,
    modal: Res<ModalTransformState>,
    gizmo_hover: Res<GizmoHoverState>,
    mut drag_state: ResMut<ViewportDragState>,
    (edit_mode, draw_state, terrain_edit_mode): (
        Res<crate::brush::EditMode>,
        Res<crate::draw_brush::DrawBrushState>,
        Res<crate::terrain::TerrainEditMode>,
    ),
    mut ray_cast: MeshRayCast,
    parents: Query<&ChildOf>,
    brushes: Query<(), With<jackdaw_jsn::Brush>>,
) {
    if modal.active.is_some() || gizmo_drag.active || gizmo_hover.hovered_axis.is_some() {
        return;
    }

    // Skip detect if there's already an active drag
    if drag_state.active.is_some() {
        return;
    }

    // Block viewport drag during brush edit mode or draw mode
    if *edit_mode != crate::brush::EditMode::Object || draw_state.active.is_some() {
        return;
    }

    // Block viewport drag during terrain sculpt mode
    if matches!(
        *terrain_edit_mode,
        crate::terrain::TerrainEditMode::Sculpt(_)
    ) {
        return;
    }

    // Shift+click on a brush is always face interaction, not viewport drag
    // (follows TrenchBroom pattern: modifier keys define non-overlapping input contexts)
    let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    if shift
        && let Some(primary) = selection.primary()
        && brushes.contains(primary)
    {
        return;
    }

    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }

    let Some(primary) = selection.primary() else {
        return;
    };
    let Ok((_, local_tf)) = transforms.get(primary) else {
        return;
    };

    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let Ok((camera, cam_tf)) = camera_query.single() else {
        return;
    };

    let Some(viewport_cursor) = window_to_viewport_cursor(cursor_pos, camera, &viewport_query)
    else {
        return;
    };

    // Raycast to check if click hits the primary selection's mesh
    let Ok(ray) = camera.viewport_to_world(cam_tf, viewport_cursor) else {
        return;
    };
    let settings = MeshRayCastSettings::default().with_visibility(RayCastVisibility::Any);
    let hits = ray_cast.cast_ray(ray, &settings);

    let mut hit_primary = false;
    for (hit_entity, _) in hits {
        let mut entity = *hit_entity;
        loop {
            if entity == primary {
                hit_primary = true;
                break;
            }
            if let Ok(child_of) = parents.get(entity) {
                entity = child_of.0;
            } else {
                break;
            }
        }
        if hit_primary {
            break;
        }
    }

    if hit_primary {
        drag_state.pending = Some(PendingDrag {
            entity: primary,
            start_transform: *local_tf,
            click_pos: cursor_pos,
            start_viewport_cursor: viewport_cursor,
        });
    }
}

fn viewport_drag_update(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    snap_settings: Res<SnapSettings>,
    mut drag_state: ResMut<ViewportDragState>,
    mut transforms: Query<&mut Transform>,
    mut cursor_query: Query<&mut CursorOptions, With<Window>>,
    edit_mode: Res<crate::brush::EditMode>,
    terrain_edit_mode: Res<crate::terrain::TerrainEditMode>,
) {
    if !mouse.pressed(MouseButton::Left) {
        drag_state.pending = None;
        return;
    }

    // Cancel pending drag if terrain sculpt mode became active
    if matches!(
        *terrain_edit_mode,
        crate::terrain::TerrainEditMode::Sculpt(_)
    ) {
        drag_state.pending = None;
        return;
    }

    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };

    // Check pending -> active promotion
    if let Some(ref pending) = drag_state.pending {
        // Cancel pending drag if we're no longer in Object mode
        // (e.g. brush_face_interact entered Face mode on the same click)
        if *edit_mode != crate::brush::EditMode::Object {
            drag_state.pending = None;
            return;
        }
        let dist = (cursor_pos - pending.click_pos).length();
        if dist > 5.0 {
            let active = ActiveDrag {
                entity: pending.entity,
                start_transform: pending.start_transform,
                start_viewport_cursor: pending.start_viewport_cursor,
            };
            drag_state.active = Some(active);
            drag_state.pending = None;
            // Confine cursor during viewport drag
            if let Ok(mut cursor_opts) = cursor_query.single_mut() {
                cursor_opts.grab_mode = CursorGrabMode::Confined;
            }
        } else {
            return;
        }
    }

    // Update active drag
    let Some(ref active) = drag_state.active else {
        return;
    };
    let Ok((camera, cam_tf)) = camera_query.single() else {
        return;
    };
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let alt = keyboard.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);

    let viewport_cursor =
        window_to_viewport_cursor(cursor_pos, camera, &viewport_query).unwrap_or(cursor_pos);

    let start_pos = active.start_transform.translation;
    let cam_dist = (cam_tf.translation() - start_pos).length();
    let scale = cam_dist * 0.003;
    let mouse_delta = viewport_cursor - active.start_viewport_cursor;

    let offset = if alt {
        // Alt+drag: move along Y axis only (vertical)
        Vec3::Y * (-mouse_delta.y) * scale
    } else {
        // Normal drag: move in XZ plane
        let cam_right = cam_tf.right().as_vec3();
        let cam_forward = cam_tf.forward().as_vec3();
        let right_h = Vec3::new(cam_right.x, 0.0, cam_right.z).normalize_or_zero();
        let forward_h = Vec3::new(cam_forward.x, 0.0, cam_forward.z).normalize_or_zero();

        let raw = right_h * mouse_delta.x * scale + forward_h * (-mouse_delta.y) * scale;

        if shift {
            // Shift+drag: restrict to dominant axis
            if raw.x.abs() > raw.z.abs() {
                Vec3::new(raw.x, 0.0, 0.0)
            } else {
                Vec3::new(0.0, 0.0, raw.z)
            }
        } else {
            raw
        }
    };

    let snapped_offset = snap_settings.snap_translate_vec3_if(offset, ctrl);

    if let Ok(mut transform) = transforms.get_mut(active.entity) {
        transform.translation = start_pos + snapped_offset;
    }
}

fn viewport_drag_finish(
    mouse: Res<ButtonInput<MouseButton>>,
    mut drag_state: ResMut<ViewportDragState>,
    transforms: Query<&Transform>,
    mut history: ResMut<CommandHistory>,
    mut cursor_query: Query<&mut CursorOptions, With<Window>>,
) {
    if !mouse.just_released(MouseButton::Left) {
        return;
    }

    drag_state.pending = None;

    let Some(active) = drag_state.active.take() else {
        return;
    };

    if let Ok(transform) = transforms.get(active.entity) {
        let cmd = SetTransform {
            entity: active.entity,
            old_transform: active.start_transform,
            new_transform: *transform,
        };
        history.push_executed(Box::new(cmd));
    }

    // Release cursor confinement
    if let Ok(mut cursor_opts) = cursor_query.single_mut() {
        cursor_opts.grab_mode = CursorGrabMode::None;
    }
}
