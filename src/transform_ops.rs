//! Transform-shortcut operators: reset, 90 deg rotate, and nudge.
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

use bevy::prelude::*;
use bevy_enhanced_input::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;

use crate::core_extension::CoreExtensionInputContext;
use crate::entity_ops::{
    TransformReset, camera_snapped_rotation_axes, can_act_on_entities, nudge_selected,
    reset_transform_selected, rotate_selected,
};
use jackdaw_api_internal::lifecycle::ActiveModalQuery;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
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

    // Reset: Alt + G / R / S (Press).
    ctx.bind_operator::<CoreExtensionInputContext, TransformResetPositionOp>([PresetInput::key(
        "KeyG",
    )
    .alt()]);
    ctx.bind_operator::<CoreExtensionInputContext, TransformResetRotationOp>([PresetInput::key(
        "KeyR",
    )
    .alt()]);
    ctx.bind_operator::<CoreExtensionInputContext, TransformResetScaleOp>([PresetInput::key(
        "KeyS",
    )
    .alt()]);

    // Rotate 90: Alt + Arrow / PageUp / PageDown (Press).
    ctx.bind_operator::<CoreExtensionInputContext, TransformRotate90YawCcwOp>([PresetInput::key(
        "ArrowLeft",
    )
    .alt()]);
    ctx.bind_operator::<CoreExtensionInputContext, TransformRotate90YawCwOp>([PresetInput::key(
        "ArrowRight",
    )
    .alt()]);
    ctx.bind_operator::<CoreExtensionInputContext, TransformRotate90PitchCcwOp>([
        PresetInput::key("ArrowUp").alt(),
    ]);
    ctx.bind_operator::<CoreExtensionInputContext, TransformRotate90PitchCwOp>([PresetInput::key(
        "ArrowDown",
    )
    .alt()]);
    ctx.bind_operator::<CoreExtensionInputContext, TransformRotate90RollCcwOp>([PresetInput::key(
        "PageUp",
    )
    .alt()]);
    ctx.bind_operator::<CoreExtensionInputContext, TransformRotate90RollCwOp>([PresetInput::key(
        "PageDown",
    )
    .alt()]);

    // Nudge: plain Arrow / PageUp / PageDown without Press (hold-to-repeat).
    // Deferred: condition is NOT bare Press::default().
    let ext = ctx.id();
    ctx.entity_mut().world_scope(|world| {
        world.spawn((
            Action::<TransformNudgeXNegOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![KeyCode::ArrowLeft],
        ));
        world.spawn((
            Action::<TransformNudgeXPosOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![KeyCode::ArrowRight],
        ));
        world.spawn((
            Action::<TransformNudgeZNegOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![KeyCode::ArrowUp],
        ));
        world.spawn((
            Action::<TransformNudgeZPosOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![KeyCode::ArrowDown],
        ));
        world.spawn((
            Action::<TransformNudgeYPosOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![KeyCode::PageUp],
        ));
        world.spawn((
            Action::<TransformNudgeYNegOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![KeyCode::PageDown],
        ));
    });
}

// -- Reset ops ---------------------------------------------------

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

// -- Rotate 90 deg ops ----------------------------------------------

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
    label = "Rotate 90 deg Yaw CCW",
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
    label = "Rotate 90 deg Yaw CW",
    is_available = can_act_on_entities
)]
fn transform_rotate_90_yaw_cw(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| rotate_by_camera_axis(world, CameraAxis::Yaw, 1.0));
    OperatorResult::Finished
}

#[operator(
    id = "transform.rotate_90_pitch_ccw",
    label = "Rotate 90 deg Pitch CCW",
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
    label = "Rotate 90 deg Pitch CW",
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
    label = "Rotate 90 deg Roll CCW",
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
    label = "Rotate 90 deg Roll CW",
    is_available = can_act_on_entities
)]
fn transform_rotate_90_roll_cw(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| rotate_by_camera_axis(world, CameraAxis::Pitch, -1.0));
    OperatorResult::Finished
}

// -- Nudge ops ---------------------------------------------------

/// Nudge the selection, by whichever of the two writers it is made of.
///
/// The arrow keys are one binding over two kinds of selection. A 3D
/// selection is translated through `Transform`; a UI selection has none, and
/// a canvas moves its nodes through `Node` in authored pixels. The canvas
/// answers first and reports whether the selection was its to move. Routing
/// is by the selection, not by which panel has focus.
///
/// [`crate::ui_stage::nudge_ui_selection`] holds the canvas half,
/// including what Shift means there.
fn nudge_by_axis(world: &mut World, offset_direction: Vec3) {
    if let Some(direction) = ui_nudge_direction(offset_direction)
        && crate::ui_stage::nudge_ui_selection(world, direction)
    {
        return;
    }
    let grid_size = world
        .resource::<crate::snapping::SnapSettings>()
        .grid_size();
    nudge_selected(world, offset_direction * grid_size);
}

/// The canvas direction a world-space nudge axis means, or `None` for
/// the one that means nothing there.
///
/// The arrows map onto the two axes a canvas has: world x is left and right,
/// and the ground plane's z is up and down the screen. `PageUp`/`PageDown`
/// nudge along world y, which a flat canvas has no axis for.
fn ui_nudge_direction(offset_direction: Vec3) -> Option<Vec2> {
    if offset_direction.x != 0.0 {
        Some(Vec2::new(offset_direction.x.signum(), 0.0))
    } else if offset_direction.z != 0.0 {
        Some(Vec2::new(0.0, offset_direction.z.signum()))
    } else {
        None
    }
}

/// `is_available` for the nudge ops: everything [`can_act_on_entities`]
/// asks, under a bare key and no other.
///
/// `bevy_enhanced_input` matches a binding on the modifiers it *names* and
/// says nothing about the ones it does not, so a binding on a bare arrow
/// answers Ctrl+Arrow too. Ctrl+Arrow is the outliner's reorder and
/// Alt+Arrow the 90-degree rotate, and both of them moved the selection
/// twice: once the way the chord asked, once a grid step sideways.
pub(crate) fn can_nudge(
    keybind_focus: crate::keybind_focus::KeybindFocus,
    active: ActiveModalQuery,
    modal: Res<crate::modal_transform::ModalTransformState>,
    draw_state: Res<crate::draw_brush::DrawBrushState>,
    edit_mode: Res<crate::brush::EditMode>,
    panel_focus: crate::panel_focus::PanelFocus,
    keyboard: Res<ButtonInput<KeyCode>>,
) -> bool {
    if crate::draw_brush::unwanted_modifier(&keyboard, false) {
        return false;
    }
    can_act_on_entities(
        keybind_focus,
        active,
        modal,
        draw_state,
        edit_mode,
        panel_focus,
    )
}

#[operator(
    id = "transform.nudge_x_neg",
    label = "Nudge -X",
    is_available = can_nudge
)]
fn transform_nudge_x_neg(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| nudge_by_axis(world, Vec3::NEG_X));
    OperatorResult::Finished
}

#[operator(
    id = "transform.nudge_x_pos",
    label = "Nudge +X",
    is_available = can_nudge
)]
fn transform_nudge_x_pos(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| nudge_by_axis(world, Vec3::X));
    OperatorResult::Finished
}

#[operator(
    id = "transform.nudge_y_neg",
    label = "Nudge -Y",
    is_available = can_nudge
)]
fn transform_nudge_y_neg(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| nudge_by_axis(world, Vec3::NEG_Y));
    OperatorResult::Finished
}

#[operator(
    id = "transform.nudge_y_pos",
    label = "Nudge +Y",
    is_available = can_nudge
)]
fn transform_nudge_y_pos(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| nudge_by_axis(world, Vec3::Y));
    OperatorResult::Finished
}

#[operator(
    id = "transform.nudge_z_neg",
    label = "Nudge -Z",
    is_available = can_nudge
)]
fn transform_nudge_z_neg(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| nudge_by_axis(world, Vec3::NEG_Z));
    OperatorResult::Finished
}

#[operator(
    id = "transform.nudge_z_pos",
    label = "Nudge +Z",
    is_available = can_nudge
)]
fn transform_nudge_z_pos(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| nudge_by_axis(world, Vec3::Z));
    OperatorResult::Finished
}
