//! Transform-shortcut operators: reset, 90° rotate, and nudge.
//!
//! `reset_*` snap translation / rotation / scale on the selection back
//! to defaults. `rotate_90_*` rotate the selection by a quarter-turn
//! around camera-snapped yaw / pitch / roll axes (matches the legacy
//! TrenchBroom-style rotation shortcut). `nudge_*` translate the
//! selection by one grid step along a world-space axis.
//!
//! Default keybinds follow the editor's long-standing bindings:
//! Alt+G/R/S for reset, Alt+Arrow and Alt+PageUp/Down for `rotate_90`,
//! plain Arrow and PageUp/Down for nudge.

use bevy::{input_focus::InputFocus, prelude::*};
use bevy_enhanced_input::prelude::{Chord, Press, *};
use jackdaw_api::prelude::*;

use crate::core_extension::{CoreExtensionInputContext, Modifiers};
use crate::entity_ops::{
    TransformReset, camera_snapped_rotation_axes, nudge_selected, reset_transform_selected,
    rotate_selected,
};

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext, modifiers: &Modifiers) {
    ctx.register_operator::<TransformResetPositionOp>()
        .register_operator::<TransformResetRotationOp>()
        .register_operator::<TransformResetScaleOp>()
        .register_operator::<TransformRotate90YawCcwOp>()
        .register_operator::<TransformRotate90YawCwOp>()
        .register_operator::<TransformRotate90PitchCcwOp>()
        .register_operator::<TransformRotate90PitchCwOp>()
        .register_operator::<TransformRotate90RollCcwOp>()
        .register_operator::<TransformRotate90RollCwOp>()
        .register_operator::<TransformNudgeXNegOp>()
        .register_operator::<TransformNudgeXPosOp>()
        .register_operator::<TransformNudgeYNegOp>()
        .register_operator::<TransformNudgeYPosOp>()
        .register_operator::<TransformNudgeZNegOp>()
        .register_operator::<TransformNudgeZPosOp>();

    let ext = ctx.id();
    let alt = modifiers.alt;
    ctx.entity_mut().world_scope(|world| {
        // Reset: Alt + G / R / S
        world.spawn((
            Action::<TransformResetPositionOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            Chord::single(alt),
            bindings![(KeyCode::KeyG, Press::default())],
        ));
        world.spawn((
            Action::<TransformResetRotationOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            Chord::single(alt),
            bindings![(KeyCode::KeyR, Press::default())],
        ));
        world.spawn((
            Action::<TransformResetScaleOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            Chord::single(alt),
            bindings![(KeyCode::KeyS, Press::default())],
        ));

        // Rotate 90: Alt + Arrow / PageUp / PageDown
        world.spawn((
            Action::<TransformRotate90YawCcwOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            Chord::single(alt),
            bindings![(KeyCode::ArrowLeft, Press::default())],
        ));
        world.spawn((
            Action::<TransformRotate90YawCwOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            Chord::single(alt),
            bindings![(KeyCode::ArrowRight, Press::default())],
        ));
        world.spawn((
            Action::<TransformRotate90PitchCcwOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            Chord::single(alt),
            bindings![(KeyCode::ArrowUp, Press::default())],
        ));
        world.spawn((
            Action::<TransformRotate90PitchCwOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            Chord::single(alt),
            bindings![(KeyCode::ArrowDown, Press::default())],
        ));
        world.spawn((
            Action::<TransformRotate90RollCcwOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            Chord::single(alt),
            bindings![(KeyCode::PageUp, Press::default())],
        ));
        world.spawn((
            Action::<TransformRotate90RollCwOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            Chord::single(alt),
            bindings![(KeyCode::PageDown, Press::default())],
        ));

        // Nudge: plain Arrow / PageUp / PageDown. No modifier chord —
        // but BEI still fires our Alt+Arrow rotate bindings when Alt
        // is held, so in practice the two don't collide.
        world.spawn((
            Action::<TransformNudgeXNegOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(KeyCode::ArrowLeft, Press::default())],
        ));
        world.spawn((
            Action::<TransformNudgeXPosOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(KeyCode::ArrowRight, Press::default())],
        ));
        world.spawn((
            Action::<TransformNudgeZNegOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(KeyCode::ArrowUp, Press::default())],
        ));
        world.spawn((
            Action::<TransformNudgeZPosOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(KeyCode::ArrowDown, Press::default())],
        ));
        world.spawn((
            Action::<TransformNudgeYPosOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(KeyCode::PageUp, Press::default())],
        ));
        world.spawn((
            Action::<TransformNudgeYNegOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(KeyCode::PageDown, Press::default())],
        ));
    });
}

/// Shared availability check for transform operators. Matches the
/// guards the legacy `handle_entity_keys` applied.
fn can_act_on_entities(
    input_focus: Res<InputFocus>,
    active: ActiveModalQuery,
    modal: Res<crate::modal_transform::ModalTransformState>,
    draw_state: Res<crate::draw_brush::DrawBrushState>,
    edit_mode: Res<crate::brush::EditMode>,
) -> bool {
    if input_focus.0.is_some() || active.is_modal_running() || modal.active.is_some() {
        return false;
    }
    if draw_state.active.is_some() {
        return false;
    }
    matches!(*edit_mode, crate::brush::EditMode::Object)
}

// ── Reset ops ───────────────────────────────────────────────────

#[operator(
    id = "transform.reset_position",
    label = "Reset Position",
    is_available = can_act_on_entities
)]
fn transform_reset_position(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        reset_transform_selected(world, TransformReset::Position);
    });
    OperatorResult::Finished
}

#[operator(
    id = "transform.reset_rotation",
    label = "Reset Rotation",
    is_available = can_act_on_entities
)]
fn transform_reset_rotation(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        reset_transform_selected(world, TransformReset::Rotation);
    });
    OperatorResult::Finished
}

#[operator(
    id = "transform.reset_scale",
    label = "Reset Scale",
    is_available = can_act_on_entities
)]
fn transform_reset_scale(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        reset_transform_selected(world, TransformReset::Scale);
    });
    OperatorResult::Finished
}

// ── Rotate 90° ops ──────────────────────────────────────────────

#[derive(Clone, Copy)]
enum CameraAxis {
    Yaw,
    Pitch,
    Roll,
}

fn rotate_by_camera_axis(world: &mut World, axis: CameraAxis, direction: f32) {
    let (yaw_axis, roll_axis, pitch_axis) = {
        let mut query =
            world.query_filtered::<&GlobalTransform, With<crate::viewport::MainViewportCamera>>();
        query
            .iter(world)
            .next()
            .map(camera_snapped_rotation_axes)
            .unwrap_or((Vec3::Y, Vec3::NEG_Z, Vec3::X))
    };
    let angle = std::f32::consts::FRAC_PI_2 * direction;
    let rotation_axis = match axis {
        CameraAxis::Yaw => yaw_axis,
        CameraAxis::Pitch => pitch_axis,
        CameraAxis::Roll => roll_axis,
    };
    let rotation = Quat::from_axis_angle(rotation_axis, angle);
    rotate_selected(world, rotation);
}

#[operator(
    id = "transform.rotate_90_yaw_ccw",
    label = "Rotate 90° Yaw CCW",
    is_available = can_act_on_entities
)]
fn transform_rotate_90_yaw_ccw(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| rotate_by_camera_axis(world, CameraAxis::Yaw, -1.0));
    OperatorResult::Finished
}

#[operator(
    id = "transform.rotate_90_yaw_cw",
    label = "Rotate 90° Yaw CW",
    is_available = can_act_on_entities
)]
fn transform_rotate_90_yaw_cw(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| rotate_by_camera_axis(world, CameraAxis::Yaw, 1.0));
    OperatorResult::Finished
}

#[operator(
    id = "transform.rotate_90_pitch_ccw",
    label = "Rotate 90° Pitch CCW",
    is_available = can_act_on_entities
)]
fn transform_rotate_90_pitch_ccw(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| rotate_by_camera_axis(world, CameraAxis::Roll, -1.0));
    OperatorResult::Finished
}

#[operator(
    id = "transform.rotate_90_pitch_cw",
    label = "Rotate 90° Pitch CW",
    is_available = can_act_on_entities
)]
fn transform_rotate_90_pitch_cw(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| rotate_by_camera_axis(world, CameraAxis::Roll, 1.0));
    OperatorResult::Finished
}

#[operator(
    id = "transform.rotate_90_roll_ccw",
    label = "Rotate 90° Roll CCW",
    is_available = can_act_on_entities
)]
fn transform_rotate_90_roll_ccw(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| rotate_by_camera_axis(world, CameraAxis::Pitch, 1.0));
    OperatorResult::Finished
}

#[operator(
    id = "transform.rotate_90_roll_cw",
    label = "Rotate 90° Roll CW",
    is_available = can_act_on_entities
)]
fn transform_rotate_90_roll_cw(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| rotate_by_camera_axis(world, CameraAxis::Pitch, -1.0));
    OperatorResult::Finished
}

// ── Nudge ops ───────────────────────────────────────────────────

fn nudge_by_axis(world: &mut World, offset_direction: Vec3) {
    let grid_size = world
        .resource::<crate::snapping::SnapSettings>()
        .grid_size();
    nudge_selected(world, offset_direction * grid_size);
}

#[operator(
    id = "transform.nudge_x_neg",
    label = "Nudge −X",
    is_available = can_act_on_entities
)]
fn transform_nudge_x_neg(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| nudge_by_axis(world, Vec3::NEG_X));
    OperatorResult::Finished
}

#[operator(
    id = "transform.nudge_x_pos",
    label = "Nudge +X",
    is_available = can_act_on_entities
)]
fn transform_nudge_x_pos(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| nudge_by_axis(world, Vec3::X));
    OperatorResult::Finished
}

#[operator(
    id = "transform.nudge_y_neg",
    label = "Nudge −Y",
    is_available = can_act_on_entities
)]
fn transform_nudge_y_neg(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| nudge_by_axis(world, Vec3::NEG_Y));
    OperatorResult::Finished
}

#[operator(
    id = "transform.nudge_y_pos",
    label = "Nudge +Y",
    is_available = can_act_on_entities
)]
fn transform_nudge_y_pos(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| nudge_by_axis(world, Vec3::Y));
    OperatorResult::Finished
}

#[operator(
    id = "transform.nudge_z_neg",
    label = "Nudge −Z",
    is_available = can_act_on_entities
)]
fn transform_nudge_z_neg(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| nudge_by_axis(world, Vec3::NEG_Z));
    OperatorResult::Finished
}

#[operator(
    id = "transform.nudge_z_pos",
    label = "Nudge +Z",
    is_available = can_act_on_entities
)]
fn transform_nudge_z_pos(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| nudge_by_axis(world, Vec3::Z));
    OperatorResult::Finished
}
