//! Clip-tool operators. Replace the keybind/click branches in
//! `brush::interaction::handle_clip_mode`. The remaining clip-mode
//! work (recomputing the preview plane and drawing the gizmo
//! overlay) stays in `interaction.rs`.
//!
//! Default keybinds: LMB places a point, Tab cycles mode, Enter
//! applies, Escape clears.

use bevy::{prelude::*, ui::ui_transform::UiGlobalTransform, window::PrimaryWindow};
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;
use jackdaw_jsn::{Brush, BrushFaceData, BrushGroup, BrushPlane};

use crate::brush::{
    BrushEditMode, BrushMeshCache, BrushSelection, ClipMode, ClipState, EditMode, SetBrush,
};
use crate::commands::{CommandGroup, CommandHistory};
use crate::core_extension::CoreExtensionInputContext;
use crate::draw_brush::{CreateBrushCommand, brush_data_from_entity};
use crate::viewport::{ActiveViewport, MainViewportCamera, SceneViewport};
use crate::viewport_util::window_to_viewport_cursor_for;
use jackdaw_geometry::{EPSILON, compute_face_tangent_axes, point_inside_all_planes};

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ClipPlacePointOp>()
        .register_operator::<ClipCycleModeOp>()
        .register_operator::<ClipApplyOp>()
        .register_operator::<ClipClearOp>();

    let ext = ctx.id();
    ctx.entity_mut().world_scope(|world| {
        world.spawn((
            Action::<ClipCycleModeOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(KeyCode::Tab, Press::default())],
        ));
        world.spawn((
            Action::<ClipApplyOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(KeyCode::Enter, Press::default())],
        ));
        world.spawn((
            Action::<ClipClearOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(KeyCode::Escape, Press::default())],
        ));
    });
}

/// LMB in clip mode dispatches `brush.clip.place_point`. Mouse-button
/// gestures aren't expressible as BEI key bindings.
pub(crate) fn place_point_invoke_trigger(
    mouse: Res<ButtonInput<MouseButton>>,
    edit_mode: Res<EditMode>,
    keybind_focus: crate::keybind_focus::KeybindFocus,
    clip_state: Res<ClipState>,
    mut commands: Commands,
) {
    if !mouse.just_pressed(MouseButton::Left)
        || !is_clip_mode_value(&edit_mode)
        || keybind_focus.is_typing()
        || clip_state.points.len() >= 3
    {
        return;
    }
    commands.queue(|world: &mut World| {
        let _ = world
            .operator(ClipPlacePointOp::ID)
            .settings(CallOperatorSettings {
                execution_context: ExecutionContext::Invoke,
                creates_history_entry: false,
            })
            .call();
    });
}

fn is_clip_mode_value(edit_mode: &EditMode) -> bool {
    matches!(edit_mode, EditMode::BrushEdit(BrushEditMode::Clip))
}

fn is_clip_mode_open(
    edit_mode: &EditMode,
    keybind_focus: &crate::keybind_focus::KeybindFocus,
) -> bool {
    !keybind_focus.is_typing() && is_clip_mode_value(edit_mode)
}

fn can_place_point(
    edit_mode: Res<EditMode>,
    keybind_focus: crate::keybind_focus::KeybindFocus,
    brush_selection: Res<BrushSelection>,
    clip_state: Res<ClipState>,
) -> bool {
    is_clip_mode_open(&edit_mode, &keybind_focus)
        && brush_selection.entity.is_some()
        && clip_state.points.len() < 3
}

fn can_apply_or_cycle(
    edit_mode: Res<EditMode>,
    keybind_focus: crate::keybind_focus::KeybindFocus,
    clip_state: Res<ClipState>,
) -> bool {
    is_clip_mode_open(&edit_mode, &keybind_focus) && clip_state.preview_plane.is_some()
}

fn can_clear(
    edit_mode: Res<EditMode>,
    keybind_focus: crate::keybind_focus::KeybindFocus,
    clip_state: Res<ClipState>,
    active: ActiveModalQuery,
) -> bool {
    if active.is_modal_running() {
        return false;
    }
    is_clip_mode_open(&edit_mode, &keybind_focus)
        && (!clip_state.points.is_empty() || clip_state.mode != ClipMode::KeepFront)
}

#[operator(
    id = "brush.clip.place_point",
    label = "Place Clip Point",
    description = "Raycast the cursor against the selected brush, snap, and add the \
                   resulting local-space point to `ClipState`. Availability \
                   (`can_place_point`) requires clip mode, a selected brush, and \
                   fewer than three existing points.",
    is_available = can_place_point,
    allows_undo = false,
)]
pub(crate) fn clip_place_point(
    _: In<OperatorParameters>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    active: Res<ActiveViewport>,
    keyboard: Res<ButtonInput<KeyCode>>,
    brush_selection: Res<BrushSelection>,
    brushes: Query<&Brush>,
    brush_transforms: Query<&GlobalTransform>,
    brush_caches: Query<&BrushMeshCache>,
    snap_settings: Res<crate::snapping::SnapSettings>,
    mut clip_state: ResMut<ClipState>,
) -> OperatorResult {
    let Some(brush_entity) = brush_selection.entity else {
        return OperatorResult::Cancelled;
    };
    let Ok(brush_global) = brush_transforms.get(brush_entity) else {
        return OperatorResult::Cancelled;
    };
    let Ok(brush) = brushes.get(brush_entity) else {
        return OperatorResult::Cancelled;
    };
    let Ok(cache) = brush_caches.get(brush_entity) else {
        return OperatorResult::Cancelled;
    };
    let Ok(window) = primary_window.single() else {
        return OperatorResult::Cancelled;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return OperatorResult::Cancelled;
    };
    let Some(camera_entity) = active.camera else {
        return OperatorResult::Cancelled;
    };
    let Some(viewport_entity) = active.ui_node else {
        return OperatorResult::Cancelled;
    };
    let Ok((camera, cam_tf)) = camera_query.get(camera_entity) else {
        return OperatorResult::Cancelled;
    };
    let Some(viewport_cursor) =
        window_to_viewport_cursor_for(cursor_pos, camera, viewport_entity, &viewport_query)
    else {
        return OperatorResult::Cancelled;
    };
    let Ok(ray) = camera.viewport_to_world(cam_tf, viewport_cursor) else {
        return OperatorResult::Cancelled;
    };

    let (_, brush_rot, brush_trans) = brush_global.to_scale_rotation_translation();
    let mut best_t = f32::MAX;
    let mut best_point = None;

    for (face_idx, polygon) in cache.face_polygons.iter().enumerate() {
        if polygon.len() < 3 {
            continue;
        }
        let face = &brush.faces[face_idx];
        let world_normal = brush_rot * face.plane.normal;
        let face_centroid: Vec3 =
            polygon.iter().map(|&vi| cache.vertices[vi]).sum::<Vec3>() / polygon.len() as f32;
        let world_centroid = brush_global.transform_point(face_centroid);

        let denom = world_normal.dot(*ray.direction);
        if denom.abs() < EPSILON {
            continue;
        }
        let t = (world_centroid - ray.origin).dot(world_normal) / denom;
        if t > 0.0 && t < best_t {
            let hit = ray.origin + *ray.direction * t;
            let local_hit = brush_rot.inverse() * (hit - brush_trans);
            if point_inside_all_planes(local_hit, &brush.faces) {
                best_t = t;
                best_point = Some(local_hit);
            }
        }
    }

    let Some(local_hit) = best_point else {
        return OperatorResult::Cancelled;
    };

    let world_point = brush_global.transform_point(local_hit);
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let snapped = snap_settings.snap_translate_vec3_if(world_point, ctrl);
    let local_snapped = brush_rot.inverse() * (snapped - brush_trans);
    clip_state.points.push(local_snapped);
    OperatorResult::Finished
}

#[operator(
    id = "brush.clip.cycle_mode",
    label = "Cycle Clip Mode",
    description = "Cycle `ClipState.mode` through KeepFront → KeepBack → Split. \
                   Availability (`can_apply_or_cycle`) requires clip mode and a \
                   computed preview plane.",
    is_available = can_apply_or_cycle,
    allows_undo = false,
)]
pub(crate) fn clip_cycle_mode(
    _: In<OperatorParameters>,
    mut clip_state: ResMut<ClipState>,
) -> OperatorResult {
    clip_state.mode = match clip_state.mode {
        ClipMode::KeepFront => ClipMode::KeepBack,
        ClipMode::KeepBack => ClipMode::Split,
        ClipMode::Split => ClipMode::KeepFront,
    };
    OperatorResult::Finished
}

#[operator(
    id = "brush.clip.clear",
    label = "Clear Clip Points",
    description = "Reset `ClipState` to its default (no points, KeepFront mode). \
                   Availability (`can_clear`) requires clip mode with non-default \
                   state and no active modal.",
    is_available = can_clear,
    allows_undo = false,
)]
pub(crate) fn clip_clear(
    _: In<OperatorParameters>,
    mut clip_state: ResMut<ClipState>,
) -> OperatorResult {
    *clip_state = ClipState::default();
    OperatorResult::Finished
}

#[operator(
    id = "brush.clip.apply",
    label = "Apply Clip",
    description = "Apply the preview plane to the selected brush per the current \
                   `ClipState.mode` (KeepFront / KeepBack / Split). Availability \
                   (`can_apply_or_cycle`) requires clip mode and a computed \
                   preview plane.",
    is_available = can_apply_or_cycle,
    allows_undo = false,
)]
pub(crate) fn clip_apply(
    _: In<OperatorParameters>,
    brush_selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    brush_transforms: Query<&GlobalTransform>,
    mut clip_state: ResMut<ClipState>,
    mut history: ResMut<CommandHistory>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(brush_entity) = brush_selection.entity else {
        return OperatorResult::Cancelled;
    };
    let Ok(mut brush) = brushes.get_mut(brush_entity) else {
        return OperatorResult::Cancelled;
    };
    let Some(plane) = clip_state.preview_plane.clone() else {
        return OperatorResult::Cancelled;
    };
    let Ok(brush_global) = brush_transforms.get(brush_entity) else {
        return OperatorResult::Cancelled;
    };

    let clip_face = clip_face_from_plane(&plane);
    let flipped_face = clip_face_from_plane(&BrushPlane {
        normal: -plane.normal,
        distance: -plane.distance,
    });

    match clip_state.mode {
        ClipMode::KeepFront => {
            push_face_command(
                &mut history,
                brush_entity,
                &mut brush,
                clip_face,
                "Clip brush (keep front)",
            );
        }
        ClipMode::KeepBack => {
            push_face_command(
                &mut history,
                brush_entity,
                &mut brush,
                flipped_face,
                "Clip brush (keep back)",
            );
        }
        ClipMode::Split => {
            let old = brush.clone();
            let mut front = old.clone();
            front.faces.push(clip_face);
            let mut back = old.clone();
            back.faces.push(flipped_face);
            *brush = front.clone();

            let set_cmd = SetBrush {
                entity: brush_entity,
                old,
                new: front,
                label: "Clip brush (split - front)".to_string(),
            };

            let (_, brush_rot, brush_trans) = brush_global.to_scale_rotation_translation();
            let spawn_transform = Transform {
                translation: brush_trans,
                rotation: brush_rot,
                scale: Vec3::ONE,
            };
            commands.queue(move |world: &mut World| {
                let parent_group = world
                    .get::<ChildOf>(brush_entity)
                    .map(|c| c.0)
                    .filter(|&p| world.get::<BrushGroup>(p).is_some());
                let actual_transform = if parent_group.is_some() {
                    *world.get::<Transform>(brush_entity).unwrap()
                } else {
                    spawn_transform
                };

                let mut spawner = world.spawn((
                    Name::new("Brush"),
                    back,
                    actual_transform,
                    Visibility::default(),
                ));
                if let Some(parent) = parent_group {
                    spawner.insert(ChildOf(parent));
                }
                let entity = spawner.id();
                crate::scene_io::register_entity_in_ast(world, entity);

                let create_cmd = CreateBrushCommand {
                    data: brush_data_from_entity(world, entity),
                };
                let group = CommandGroup {
                    commands: vec![Box::new(set_cmd), Box::new(create_cmd)],
                    label: "Split brush".to_string(),
                };
                world
                    .resource_mut::<CommandHistory>()
                    .push_executed(Box::new(group));
            });
        }
    }

    *clip_state = ClipState::default();
    OperatorResult::Finished
}

fn clip_face_from_plane(plane: &BrushPlane) -> BrushFaceData {
    let (u, v) = compute_face_tangent_axes(plane.normal);
    BrushFaceData {
        plane: plane.clone(),
        uv_offset: Vec2::ZERO,
        uv_scale: Vec2::ONE,
        uv_rotation: 0.0,
        uv_u_axis: u,
        uv_v_axis: v,
        ..default()
    }
}

fn push_face_command(
    history: &mut CommandHistory,
    entity: Entity,
    brush: &mut Brush,
    face: BrushFaceData,
    label: &str,
) {
    let old = brush.clone();
    brush.faces.push(face);
    let cmd = SetBrush {
        entity,
        old,
        new: brush.clone(),
        label: label.to_string(),
    };
    history.push_executed(Box::new(cmd));
}
