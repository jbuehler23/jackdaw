//! View-mode toggles and per-viewport view operators.
//!
//! - Toggle ops (`view.toggle_*`, `view.cycle_*`) flip a resource.
//!   Only `view.toggle_wireframe` has a default keybind
//!   (`Ctrl+Shift+W`); the rest are menu-only.
//! - Per-viewport ops (`view.set_axis`, `view.toggle_persp_ortho`,
//!   `view.frame_selected`, `view.frame_all`) act on the camera of
//!   the hovered viewport (via [`crate::viewport::ActiveViewport`])
//!   so quad-view / stacked viewport setups respond to whichever
//!   panel the cursor is in.

use bevy::prelude::*;
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;

use crate::core_extension::CoreExtensionInputContext;
use crate::selection::{Selected, Selection};
use crate::viewport::{ActiveViewport, MainViewportCamera, ViewportGrid};

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ViewToggleWireframeOp>()
        .register_operator::<ViewToggleBoundingBoxesOp>()
        .register_operator::<ViewCycleBoundingBoxModeOp>()
        .register_operator::<ViewToggleFaceGridOp>()
        .register_operator::<ViewToggleBrushWireframeOp>()
        .register_operator::<ViewToggleBrushOutlineOp>()
        .register_operator::<ViewToggleAlignmentGuidesOp>()
        .register_operator::<ViewToggleColliderGizmosOp>()
        .register_operator::<ViewToggleHierarchyArrowsOp>()
        .register_operator::<ViewSetAxisOp>()
        .register_operator::<ViewTogglePerspOrthoOp>()
        .register_operator::<ViewFrameSelectedOp>()
        .register_operator::<ViewFrameAllOp>();

    let ext = ctx.id();
    ctx.entity_mut().world_scope(|world| {
        world.spawn((
            Action::<ViewToggleWireframeOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(
                KeyCode::KeyW.with_mod_keys(ModKeys::CONTROL | ModKeys::SHIFT),
                Press::default(),
            )],
        ));
        world.spawn((
            Action::<ViewTogglePerspOrthoOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(KeyCode::Numpad5, Press::default())],
        ));
        world.spawn((
            Action::<ViewFrameSelectedOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(KeyCode::NumpadDecimal, Press::default())],
        ));
        world.spawn((
            Action::<ViewFrameAllOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(KeyCode::Home, Press::default())],
        ));
    });
}

#[operator(id = "view.toggle_wireframe", label = "Toggle Wireframe")]
pub(crate) fn view_toggle_wireframe(
    _: In<OperatorParameters>,
    mut settings: ResMut<crate::view_modes::ViewModeSettings>,
) -> OperatorResult {
    settings.wireframe = !settings.wireframe;
    OperatorResult::Finished
}

#[operator(id = "view.toggle_bounding_boxes", label = "Toggle Bounding Boxes")]
pub(crate) fn view_toggle_bounding_boxes(
    _: In<OperatorParameters>,
    mut settings: ResMut<crate::viewport_overlays::OverlaySettings>,
) -> OperatorResult {
    settings.show_bounding_boxes = !settings.show_bounding_boxes;
    OperatorResult::Finished
}

#[operator(id = "view.cycle_bounding_box_mode", label = "Cycle Bounding Box Mode")]
pub(crate) fn view_cycle_bounding_box_mode(
    _: In<OperatorParameters>,
    mut settings: ResMut<crate::viewport_overlays::OverlaySettings>,
) -> OperatorResult {
    settings.bounding_box_mode = match settings.bounding_box_mode {
        crate::viewport_overlays::BoundingBoxMode::Aabb => {
            crate::viewport_overlays::BoundingBoxMode::ConvexHull
        }
        crate::viewport_overlays::BoundingBoxMode::ConvexHull => {
            crate::viewport_overlays::BoundingBoxMode::Aabb
        }
    };
    OperatorResult::Finished
}

#[operator(id = "view.toggle_face_grid", label = "Toggle Face Grid")]
pub(crate) fn view_toggle_face_grid(
    _: In<OperatorParameters>,
    mut settings: ResMut<crate::viewport_overlays::OverlaySettings>,
) -> OperatorResult {
    settings.show_face_grid = !settings.show_face_grid;
    OperatorResult::Finished
}

#[operator(id = "view.toggle_brush_wireframe", label = "Toggle Brush Wireframe")]
pub(crate) fn view_toggle_brush_wireframe(
    _: In<OperatorParameters>,
    mut settings: ResMut<crate::viewport_overlays::OverlaySettings>,
) -> OperatorResult {
    settings.show_brush_wireframe = !settings.show_brush_wireframe;
    OperatorResult::Finished
}

#[operator(id = "view.toggle_brush_outline", label = "Toggle Brush Outline")]
pub(crate) fn view_toggle_brush_outline(
    _: In<OperatorParameters>,
    mut settings: ResMut<crate::viewport_overlays::OverlaySettings>,
) -> OperatorResult {
    settings.show_brush_outline = !settings.show_brush_outline;
    OperatorResult::Finished
}

#[operator(id = "view.toggle_alignment_guides", label = "Toggle Alignment Guides")]
pub(crate) fn view_toggle_alignment_guides(
    _: In<OperatorParameters>,
    mut settings: ResMut<crate::viewport_overlays::OverlaySettings>,
) -> OperatorResult {
    settings.show_alignment_guides = !settings.show_alignment_guides;
    OperatorResult::Finished
}

#[operator(id = "view.toggle_collider_gizmos", label = "Toggle Collider Gizmos")]
pub(crate) fn view_toggle_collider_gizmos(
    _: In<OperatorParameters>,
    mut config: ResMut<jackdaw_avian_integration::PhysicsOverlayConfig>,
) -> OperatorResult {
    config.show_colliders = !config.show_colliders;
    OperatorResult::Finished
}

#[operator(id = "view.toggle_hierarchy_arrows", label = "Toggle Hierarchy Arrows")]
pub(crate) fn view_toggle_hierarchy_arrows(
    _: In<OperatorParameters>,
    mut config: ResMut<jackdaw_avian_integration::PhysicsOverlayConfig>,
) -> OperatorResult {
    config.show_hierarchy_arrows = !config.show_hierarchy_arrows;
    OperatorResult::Finished
}

fn active_viewport_ready(active: Res<ActiveViewport>) -> bool {
    active.camera.is_some()
}

fn read_int_param(params: &OperatorParameters, name: &str) -> Option<i64> {
    params.get(name).and_then(|v| match v {
        jackdaw_jsn::PropertyValue::Int(i) => Some(*i),
        _ => None,
    })
}

fn primary_selection_present(selection: Res<Selection>) -> bool {
    selection.primary().is_some()
}

const ORTHO_DISTANCE: f32 = 50.0;
/// World-space height the orthographic viewport shows by default. The
/// `FixedVertical` scaling mode keeps this constant regardless of
/// window size, so a fresh ortho switch frames a consistent slice of
/// scene around the origin rather than the resolution-dependent
/// extents that `WindowSize` (Bevy's default) would give.
const ORTHO_VIEWPORT_HEIGHT: f32 = 10.0;
const FRAME_SELECTED_MIN_DIST: f32 = 5.0;

fn perspective_default() -> Projection {
    Projection::Perspective(PerspectiveProjection::default())
}

fn orthographic_default() -> Projection {
    Projection::Orthographic(OrthographicProjection {
        scaling_mode: bevy::camera::ScalingMode::FixedVertical {
            viewport_height: ORTHO_VIEWPORT_HEIGHT,
        },
        scale: 1.0,
        ..OrthographicProjection::default_3d()
    })
}

/// Snap the active viewport's camera to look down a world axis,
/// switching to orthographic projection.
///
/// # Parameters
/// - `axis` (`i64`): which axis to look along: `0` = X, `1` = Y, `2` = Z.
/// - `sign` (`i64`): position the camera on the positive (`1`) or
///   negative (`-1`) side of that axis. The camera looks toward the
///   origin from there.
///
/// Numpad bindings (sidecar trigger):
/// - Numpad 7 / Ctrl+Numpad 7: top / bottom view (axis = Y)
/// - Numpad 1 / Ctrl+Numpad 1: front / back view (axis = Z)
/// - Numpad 3 / Ctrl+Numpad 3: right / left view (axis = X)
#[operator(
    id = "view.set_axis",
    label = "Set Axis-Aligned View",
    description = "Snap the active viewport to look down a world axis (orthographic).",
    is_available = active_viewport_ready,
)]
pub(crate) fn view_set_axis(
    params: In<OperatorParameters>,
    active: Res<ActiveViewport>,
    mut cameras: Query<
        (&mut Transform, &mut Projection, Option<&ViewportGrid>),
        With<MainViewportCamera>,
    >,
    mut grids: Query<
        &mut Transform,
        (
            With<bevy_infinite_grid::InfiniteGrid>,
            Without<MainViewportCamera>,
        ),
    >,
) -> OperatorResult {
    let axis = read_int_param(&params, "axis").unwrap_or(1);
    let sign_int = read_int_param(&params, "sign").unwrap_or(1);
    let sign = if sign_int < 0 { -1.0 } else { 1.0 };

    let dir = match axis {
        0 => Vec3::X,
        1 => Vec3::Y,
        2 => Vec3::Z,
        _ => return OperatorResult::Cancelled,
    } * sign;

    // For top/bottom views the camera's forward is parallel to world
    // up, so `looking_at` needs a non-parallel "up" hint. -Z gives the
    // standard top-down orientation (X right, Z down on screen).
    let up = if axis == 1 { Vec3::Z * -sign } else { Vec3::Y };

    let Some(camera_entity) = active.camera else {
        return OperatorResult::Cancelled;
    };
    let Ok((mut transform, mut projection, grid_link)) = cameras.get_mut(camera_entity) else {
        return OperatorResult::Cancelled;
    };

    transform.translation = dir * ORTHO_DISTANCE;
    *transform = transform.looking_at(Vec3::ZERO, up);
    *projection = orthographic_default();

    // Rotate this viewport's private grid so its plane faces the new
    // view direction. Without this the grid stays on world XZ and
    // disappears edge-on for front (axis=2) and side (axis=0) views.
    // Top (axis=1) keeps the world XZ orientation since the floor is
    // already correct.
    if let Some(ViewportGrid(grid_entity)) = grid_link
        && let Ok(mut grid_tf) = grids.get_mut(*grid_entity)
    {
        grid_tf.rotation = grid_rotation_for_axis(axis);
    }

    OperatorResult::Finished
}

/// Quaternion that orients an infinite-grid entity so its plane faces
/// the camera looking down the given world axis. The grid's local plane
/// is XZ; we map it onto:
/// - axis 0 (X, side view): YZ plane
/// - axis 1 (Y, top view): XZ plane (no rotation)
/// - axis 2 (Z, front view): XY plane
fn grid_rotation_for_axis(axis: i64) -> Quat {
    match axis {
        0 => Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
        2 => Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
        _ => Quat::IDENTITY,
    }
}

/// Toggle the active viewport's camera between perspective and
/// orthographic projection.
#[operator(
    id = "view.toggle_persp_ortho",
    label = "Toggle Perspective / Orthographic",
    description = "Switch the active viewport between perspective and orthographic.",
    is_available = active_viewport_ready,
)]
pub(crate) fn view_toggle_persp_ortho(
    _: In<OperatorParameters>,
    active: Res<ActiveViewport>,
    mut cameras: Query<(&mut Projection, Option<&ViewportGrid>), With<MainViewportCamera>>,
    mut grids: Query<
        &mut Transform,
        (
            With<bevy_infinite_grid::InfiniteGrid>,
            Without<MainViewportCamera>,
        ),
    >,
) -> OperatorResult {
    let Some(camera_entity) = active.camera else {
        return OperatorResult::Cancelled;
    };
    let Ok((mut projection, grid_link)) = cameras.get_mut(camera_entity) else {
        return OperatorResult::Cancelled;
    };
    let now_persp = matches!(projection.as_ref(), Projection::Orthographic(_));
    *projection = if now_persp {
        perspective_default()
    } else {
        orthographic_default()
    };

    // Reset this viewport's grid to the world XZ floor when returning
    // to perspective. Ortho stays on whatever orientation a previous
    // axis snap left (top is the default after `orthographic_default`).
    if now_persp
        && let Some(ViewportGrid(grid_entity)) = grid_link
        && let Ok(mut grid_tf) = grids.get_mut(*grid_entity)
    {
        grid_tf.rotation = Quat::IDENTITY;
    }

    OperatorResult::Finished
}

/// Center the active viewport's camera on the primary selection,
/// keeping its current orientation but pulling back to a sensible
/// distance.
#[operator(
    id = "view.frame_selected",
    label = "Frame Selected",
    description = "Center the active viewport on the selection.",
    is_available = primary_selection_present,
)]
pub(crate) fn view_frame_selected(
    _: In<OperatorParameters>,
    active: Res<ActiveViewport>,
    selection: Res<Selection>,
    selected_transforms: Query<&GlobalTransform, With<Selected>>,
    mut cameras: Query<&mut Transform, With<MainViewportCamera>>,
) -> OperatorResult {
    let Some(camera_entity) = active.camera else {
        return OperatorResult::Cancelled;
    };
    let Some(primary) = selection.primary() else {
        return OperatorResult::Cancelled;
    };
    let Ok(global_tf) = selected_transforms.get(primary) else {
        return OperatorResult::Cancelled;
    };
    let Ok(mut transform) = cameras.get_mut(camera_entity) else {
        return OperatorResult::Cancelled;
    };

    let target = global_tf.translation();
    let scale = global_tf.compute_transform().scale;
    let dist = (scale.length() * 3.0).max(FRAME_SELECTED_MIN_DIST);
    let forward = transform.forward().as_vec3();
    transform.translation = target - forward * dist;
    *transform = transform.looking_at(target, Vec3::Y);
    OperatorResult::Finished
}

/// Frame the entire scene in the active viewport. Falls back to the
/// world origin at a generic distance if no scene entities are
/// available.
#[operator(
    id = "view.frame_all",
    label = "Frame All",
    description = "Frame the whole scene in the active viewport.",
    is_available = active_viewport_ready,
)]
pub(crate) fn view_frame_all(
    _: In<OperatorParameters>,
    active: Res<ActiveViewport>,
    scene_entities: Query<&GlobalTransform, (With<Name>, Without<crate::EditorEntity>)>,
    mut cameras: Query<&mut Transform, With<MainViewportCamera>>,
) -> OperatorResult {
    let Some(camera_entity) = active.camera else {
        return OperatorResult::Cancelled;
    };
    let Ok(mut transform) = cameras.get_mut(camera_entity) else {
        return OperatorResult::Cancelled;
    };

    // Compute scene AABB from all named non-editor entities. Empty
    // scenes fall back to (origin, 10-unit cube).
    let (center, radius) = if scene_entities.is_empty() {
        (Vec3::ZERO, 10.0)
    } else {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for tf in &scene_entities {
            let p = tf.translation();
            min = min.min(p);
            max = max.max(p);
        }
        let center = (min + max) * 0.5;
        let radius = ((max - min).length() * 0.5).max(5.0);
        (center, radius)
    };

    let dist = radius * 2.5;
    let forward = transform.forward().as_vec3();
    transform.translation = center - forward * dist;
    *transform = transform.looking_at(center, Vec3::Y);
    OperatorResult::Finished
}

/// Sidecar system that fans Numpad 1/3/7 (with optional Ctrl) into
/// `view.set_axis` calls with the right `axis`/`sign` parameters.
/// Necessary because BEI key bindings can't carry payloads.
///
/// Works in any edit mode (Numpad keys don't collide with the Digit
/// keybinds for vertex/edge/face mode), so users can snap to an axis
/// view while editing brushes too.
pub(crate) fn axis_view_keys(
    keyboard: Res<ButtonInput<KeyCode>>,
    modal: Res<crate::modal_transform::ModalTransformState>,
    input_focus: Res<bevy::input_focus::InputFocus>,
    mut commands: Commands,
) {
    if modal.active.is_some() || input_focus.0.is_some() {
        return;
    }

    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let sign = if ctrl { -1i64 } else { 1i64 };

    let axis = if keyboard.just_pressed(KeyCode::Numpad7) {
        Some(1i64)
    } else if keyboard.just_pressed(KeyCode::Numpad1) {
        Some(2i64)
    } else if keyboard.just_pressed(KeyCode::Numpad3) {
        Some(0i64)
    } else {
        None
    };

    if let Some(axis) = axis {
        commands
            .operator(ViewSetAxisOp::ID)
            .param("axis", axis)
            .param("sign", sign)
            .call();
    }
}
