//! View-mode toggles and per-viewport view operators.
//!
//! - Toggle ops (`view.toggle_*`, `view.cycle_*`) flip a resource.
//!   Only `view.toggle_wireframe` (`Ctrl+Shift+W`) and
//!   `view.toggle_x_ray` (`Alt+Z`) have default keybinds; the rest are
//!   menu-only.
//! - Per-viewport ops (`view.set_axis`, `view.toggle_persp_ortho`,
//!   `view.frame_selected`, `view.frame_all`) act on the camera of
//!   the hovered viewport (via [`crate::viewport::ActiveViewport`])
//!   so quad-view / stacked viewport setups respond to whichever
//!   panel the cursor is in.

use bevy::camera::primitives::Aabb;
use bevy::{dev_tools::infinite_grid::InfiniteGrid, prelude::*};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;

use crate::core_extension::CoreExtensionInputContext;
use crate::selection::{Selected, Selection};
use crate::viewport::{ActiveViewport, MainViewportCamera, ViewportGrid};

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ViewToggleWireframeOp>()
        .register_operator::<ViewToggleXrayOp>()
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
        .register_operator::<ViewFrameAllOp>()
        .register_operator::<ViewDollyOp>()
        .register_operator::<ViewUiZoomInOp>()
        .register_operator::<ViewUiZoomOutOp>()
        .register_operator::<ViewUiZoomResetOp>();

    ctx.bind_operator::<CoreExtensionInputContext, ViewToggleWireframeOp>([PresetInput::key(
        "KeyW",
    )
    .ctrl()
    .shift()]);
    ctx.bind_operator::<CoreExtensionInputContext, ViewToggleXrayOp>([
        PresetInput::key("KeyZ").alt()
    ]);
    ctx.bind_operator::<CoreExtensionInputContext, ViewTogglePerspOrthoOp>([PresetInput::key(
        "Numpad5",
    )]);
    ctx.bind_operator::<CoreExtensionInputContext, ViewFrameSelectedOp>([PresetInput::key(
        "NumpadDecimal",
    )]);
    ctx.bind_operator::<CoreExtensionInputContext, ViewFrameAllOp>([PresetInput::key("Home")]);
    ctx.bind_operator::<CoreExtensionInputContext, ViewUiZoomInOp>([
        PresetInput::key("Equal").ctrl(),
        PresetInput::key("NumpadAdd").ctrl(),
    ]);
    ctx.bind_operator::<CoreExtensionInputContext, ViewUiZoomOutOp>([
        PresetInput::key("Minus").ctrl(),
        PresetInput::key("NumpadSubtract").ctrl(),
    ]);
    ctx.bind_operator::<CoreExtensionInputContext, ViewUiZoomResetOp>([
        PresetInput::key("Digit0").ctrl()
    ]);
}

#[operator(id = "view.toggle_wireframe", label = "Toggle Wireframe")]
pub(crate) fn view_toggle_wireframe(
    _: In<OperatorParameters>,
    mut settings: ResMut<crate::view_modes::ViewModeSettings>,
) -> OperatorResult {
    settings.wireframe = !settings.wireframe;
    OperatorResult::Finished
}

#[operator(id = "view.toggle_x_ray", label = "Toggle X-Ray")]
pub(crate) fn view_toggle_xray(
    _: In<OperatorParameters>,
    mut settings: ResMut<crate::view_modes::ViewModeSettings>,
) -> OperatorResult {
    settings.x_ray = !settings.x_ray;
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

const UI_SCALE_MIN: f32 = 0.5;
const UI_SCALE_MAX: f32 = 3.0;
const UI_SCALE_STEP: f32 = 1.1;
const UI_SCALE_DEFAULT: f32 = 1.0;

#[operator(
    id = "view.ui_zoom_in",
    label = "Zoom UI In",
    description = "Make the editor UI larger.",
    allows_undo = false
)]
pub(crate) fn view_ui_zoom_in(
    _: In<OperatorParameters>,
    mut ui_scale: ResMut<bevy::ui::UiScale>,
) -> OperatorResult {
    ui_scale.0 = (ui_scale.0 * UI_SCALE_STEP).min(UI_SCALE_MAX);
    OperatorResult::Finished
}

#[operator(
    id = "view.ui_zoom_out",
    label = "Zoom UI Out",
    description = "Make the editor UI smaller.",
    allows_undo = false
)]
pub(crate) fn view_ui_zoom_out(
    _: In<OperatorParameters>,
    mut ui_scale: ResMut<bevy::ui::UiScale>,
) -> OperatorResult {
    ui_scale.0 = (ui_scale.0 / UI_SCALE_STEP).max(UI_SCALE_MIN);
    OperatorResult::Finished
}

#[operator(
    id = "view.ui_zoom_reset",
    label = "Reset UI Zoom",
    description = "Restore the editor UI to its default size.",
    allows_undo = false
)]
pub(crate) fn view_ui_zoom_reset(
    _: In<OperatorParameters>,
    mut ui_scale: ResMut<bevy::ui::UiScale>,
) -> OperatorResult {
    ui_scale.0 = UI_SCALE_DEFAULT;
    OperatorResult::Finished
}

fn active_viewport_ready(active: Res<ActiveViewport>) -> bool {
    active.camera.is_some()
}

fn read_int_param(params: &OperatorParameters, name: &str) -> Option<i64> {
    params.get(name).and_then(|v| match v {
        jackdaw_scene_types::PropertyValue::Int(i) => Some(*i),
        _ => None,
    })
}

/// Resolve which viewport's camera a framing op acts on: the hovered viewport,
/// otherwise the sole `MainViewportCamera` if there is exactly one. More than
/// one and unhovered is ambiguous, so no camera is returned.
///
/// `ActiveViewport` reflects a literal cursor-over-the-panel hover
/// (`update_active_viewport`), which never happens from `JACKDAW_RUN_OP` (no
/// synthetic pointer) and can miss a keypress fired while the cursor sits over
/// other UI in a single-viewport layout. Only `view.frame_selected` and
/// `view.frame_all` use this fallback; `view.set_axis` and
/// `view.toggle_persp_ortho` keep the hover-only gate (`active_viewport_ready`).
fn resolve_frame_camera(
    active: &ActiveViewport,
    cameras: &Query<Entity, With<MainViewportCamera>>,
) -> Option<Entity> {
    if let Some(camera) = active.camera {
        return Some(camera);
    }
    let mut iter = cameras.iter();
    let first = iter.next()?;
    iter.next().is_none().then_some(first)
}

fn frame_selected_available(
    active: Res<ActiveViewport>,
    cameras: Query<Entity, With<MainViewportCamera>>,
    selection: Res<Selection>,
) -> bool {
    selection.primary().is_some() && resolve_frame_camera(&active, &cameras).is_some()
}

fn frame_all_available(
    active: Res<ActiveViewport>,
    cameras: Query<Entity, With<MainViewportCamera>>,
) -> bool {
    resolve_frame_camera(&active, &cameras).is_some()
}

const ORTHO_DISTANCE: f32 = 50.0;
/// World-space height the orthographic viewport shows by default. The
/// `FixedVertical` scaling mode keeps this constant regardless of
/// window size, so a fresh ortho switch frames a consistent slice of
/// scene around the origin rather than the resolution-dependent
/// extents that `WindowSize` (Bevy's default) would give.
const ORTHO_VIEWPORT_HEIGHT: f32 = 10.0;
const FRAME_SELECTED_MIN_DIST: f32 = 5.0;
/// How far back from a framed radius the camera sits. Above 1.0 so the subject
/// has room around it rather than filling the frame edge to edge.
const FRAME_MARGIN: f32 = 2.5;

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
    mut grids: Query<&mut Transform, (With<InfiniteGrid>, Without<MainViewportCamera>)>,
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

    let camera_entity = active.camera?;
    let (mut transform, mut projection, grid_link) = cameras.get_mut(camera_entity)?;

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

/// Orient a viewport grid so its plane faces the camera for axis-aligned ortho views.
///
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
    mut grids: Query<&mut Transform, (With<InfiniteGrid>, Without<MainViewportCamera>)>,
) -> OperatorResult {
    let camera_entity = active.camera?;
    let (mut projection, grid_link) = cameras.get_mut(camera_entity)?;
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
    is_available = frame_selected_available,
)]
pub(crate) fn view_frame_selected(
    _: In<OperatorParameters>,
    active: Res<ActiveViewport>,
    selection: Res<Selection>,
    selected_transforms: Query<&GlobalTransform, With<Selected>>,
    children: Query<&Children>,
    bounded: Query<(&GlobalTransform, &Aabb), Without<crate::ViewDependentBounds>>,
    camera_entities: Query<Entity, With<MainViewportCamera>>,
    mut cameras: Query<&mut Transform, With<MainViewportCamera>>,
) -> OperatorResult {
    let camera_entity = resolve_frame_camera(&active, &camera_entities)?;
    let primary = selection.primary()?;
    let global_tf = selected_transforms.get(primary)?;
    let mut transform = cameras.get_mut(camera_entity)?;

    // The room the selection occupies when the renderer has bounds for it; otherwise its
    // transform, which is all a light or an empty has.
    let (target, dist) = match world_bounds(primary, &children, &bounded) {
        Some((min, max)) => (
            (min + max) * 0.5,
            (((max - min).length() * 0.5) * FRAME_MARGIN).max(FRAME_SELECTED_MIN_DIST),
        ),
        None => {
            let scale = global_tf.compute_transform().scale;
            (
                global_tf.translation(),
                (scale.length() * 3.0).max(FRAME_SELECTED_MIN_DIST),
            )
        }
    };
    let forward = transform.forward().as_vec3();
    transform.translation = target - forward * dist;
    *transform = transform.looking_at(target, Vec3::Y);
    OperatorResult::Finished
}

/// A dolly under way: how far is left and how far each frame travels.
///
/// Spreading the travel over frames leaves the camera at intermediate positions,
/// so a screenshot can be taken mid-move and geometry that follows the camera
/// (the terrain's LOD rings) is exercised at each step.
#[derive(Resource)]
pub(crate) struct DollyInFlight {
    camera: Entity,
    per_frame: f32,
    frames_left: u32,
}

/// Carry a spread dolly one frame further.
pub(crate) fn drive_dolly(
    dolly: Option<ResMut<DollyInFlight>>,
    mut cameras: Query<&mut Transform>,
    mut commands: Commands,
) {
    let Some(mut dolly) = dolly else {
        return;
    };
    let Ok(mut transform) = cameras.get_mut(dolly.camera) else {
        commands.remove_resource::<DollyInFlight>();
        return;
    };
    let forward = transform.forward().as_vec3();
    transform.translation += forward * dolly.per_frame;
    dolly.frames_left -= 1;
    if dolly.frames_left == 0 {
        commands.remove_resource::<DollyInFlight>();
    }
}

/// Move the active viewport's camera along its own sightline, the scripted
/// counterpart of the scroll wheel. Rotation is untouched, so a sequence of
/// these is a straight dolly rather than a drift.
///
/// # Parameters
/// - `distance` (`f64`): metres to travel. Positive moves toward what
///   the camera is looking at, negative away from it.
/// - `frames` (`i64`): how many frames to spread the travel over.
///   Defaults to one, which arrives immediately.
#[operator(
    id = "view.dolly",
    label = "Dolly Camera",
    description = "Move the active viewport's camera along its sightline.",
    allows_undo = false,
    is_available = dolly_available,
    params(
        distance(f64, doc = "Metres to move; negative pulls back."),
        frames(i64, doc = "Frames to spread the travel over; one arrives at once.")
    )
)]
pub(crate) fn view_dolly(
    params: In<OperatorParameters>,
    active: Res<ActiveViewport>,
    camera_entities: Query<Entity, With<MainViewportCamera>>,
    mut cameras: Query<&mut Transform, With<MainViewportCamera>>,
    mut commands: Commands,
) -> OperatorResult {
    let camera_entity = resolve_frame_camera(&active, &camera_entities)?;
    let distance = params.as_float("distance").unwrap_or(0.0) as f32;
    if distance == 0.0 {
        return OperatorResult::Cancelled;
    }
    let frames = params.as_int("frames").unwrap_or(1).max(1) as u32;
    if frames > 1 {
        commands.insert_resource(DollyInFlight {
            camera: camera_entity,
            per_frame: distance / frames as f32,
            frames_left: frames,
        });
        return OperatorResult::Finished;
    }
    let mut transform = cameras.get_mut(camera_entity)?;
    let forward = transform.forward().as_vec3();
    transform.translation += forward * distance;
    OperatorResult::Finished
}

fn dolly_available(
    active: Res<ActiveViewport>,
    cameras: Query<Entity, With<MainViewportCamera>>,
) -> bool {
    resolve_frame_camera(&active, &cameras).is_some()
}

/// Frame the entire scene in the active viewport. Falls back to the
/// world origin at a generic distance if no scene entities are
/// available.
#[operator(
    id = "view.frame_all",
    label = "Frame All",
    description = "Frame the whole scene in the active viewport.",
    is_available = frame_all_available,
)]
pub(crate) fn view_frame_all(
    _: In<OperatorParameters>,
    active: Res<ActiveViewport>,
    scene_entities: Query<(Entity, &GlobalTransform), (With<Name>, Without<crate::EditorEntity>)>,
    children: Query<&Children>,
    bounded: Query<(&GlobalTransform, &Aabb), Without<crate::ViewDependentBounds>>,
    camera_entities: Query<Entity, With<MainViewportCamera>>,
    mut cameras: Query<&mut Transform, With<MainViewportCamera>>,
) -> OperatorResult {
    let camera_entity = resolve_frame_camera(&active, &camera_entities)?;
    let mut transform = cameras.get_mut(camera_entity)?;

    // The scene's extent, not just where its entities sit: a terrain is one entity at one
    // point that reaches for a kilometre, and framing it by that point puts the camera
    // inside the ground. An entity the renderer has no bounds for (a light, an empty)
    // contributes its own position.
    let (center, radius) = if scene_entities.is_empty() {
        (Vec3::ZERO, 10.0)
    } else {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for (entity, tf) in &scene_entities {
            match world_bounds(entity, &children, &bounded) {
                Some((lo, hi)) => {
                    min = min.min(lo);
                    max = max.max(hi);
                }
                None => {
                    let p = tf.translation();
                    min = min.min(p);
                    max = max.max(p);
                }
            }
        }
        let center = (min + max) * 0.5;
        let radius = ((max - min).length() * 0.5).max(5.0);
        (center, radius)
    };

    let dist = radius * FRAME_MARGIN;
    let forward = transform.forward().as_vec3();
    transform.translation = center - forward * dist;
    *transform = transform.looking_at(center, Vec3::Y);
    OperatorResult::Finished
}

/// World-space bounds of `entity` and everything under it, from the `Aabb`s the
/// renderer keeps for culling.
///
/// A transform cannot state how much room an entity takes: a terrain, a mesh
/// brush and an imported model all sit at a point and extend as far as their
/// geometry does. Reading the culling bounds rather than re-walking mesh data
/// keeps this cheap enough to run on every entity in the scene.
///
/// Descendants are included, since a terrain's LOD levels and an imported
/// model's parts are children of the entity the outliner names. `None` when
/// nothing under `entity` has bounds yet, leaving the caller to fall back to the
/// transform.
fn world_bounds(
    entity: Entity,
    children: &Query<&Children>,
    bounded: &Query<(&GlobalTransform, &Aabb), Without<crate::ViewDependentBounds>>,
) -> Option<(Vec3, Vec3)> {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    let mut found = false;
    let mut stack = vec![entity];
    while let Some(at) = stack.pop() {
        if let Ok((transform, aabb)) = bounded.get(at) {
            expand_world_aabb(transform, aabb, &mut min, &mut max);
            found = true;
        }
        if let Ok(kids) = children.get(at) {
            stack.extend(kids.iter());
        }
    }
    found.then_some((min, max))
}

/// Grow `min`/`max` to hold a local `Aabb` placed by `transform`.
///
/// All eight corners are transformed rather than the centre and extents:
/// a rotated box does not stay axis-aligned, and taking the extents straight
/// across would under-report it.
fn expand_world_aabb(transform: &GlobalTransform, aabb: &Aabb, min: &mut Vec3, max: &mut Vec3) {
    let center = Vec3::from(aabb.center);
    let extents = Vec3::from(aabb.half_extents);
    for corner in 0..8u32 {
        let sign = Vec3::new(
            if corner & 1 == 0 { -1.0 } else { 1.0 },
            if corner & 2 == 0 { -1.0 } else { 1.0 },
            if corner & 4 == 0 { -1.0 } else { 1.0 },
        );
        let world = transform.transform_point(center + extents * sign);
        *min = min.min(world);
        *max = max.max(world);
    }
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
    if modal.active.is_some() || input_focus.get().is_some() {
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

#[cfg(test)]
mod resolve_frame_camera_tests {
    use bevy::ecs::system::SystemState;

    use super::*;

    #[test]
    fn a_hovered_camera_wins_even_with_others_present() {
        let mut world = World::new();
        let hovered = world.spawn(MainViewportCamera).id();
        world.spawn(MainViewportCamera);
        let active = ActiveViewport {
            camera: Some(hovered),
            ui_node: None,
        };
        let mut state = SystemState::<Query<Entity, With<MainViewportCamera>>>::new(&mut world);
        let cameras = state.get(&world).unwrap();
        assert_eq!(resolve_frame_camera(&active, &cameras), Some(hovered));
    }

    /// The no-hover case `JACKDAW_RUN_OP` hits: falls back to the sole viewport camera.
    #[test]
    fn nothing_hovered_falls_back_to_the_sole_camera() {
        let mut world = World::new();
        let camera = world.spawn(MainViewportCamera).id();
        let active = ActiveViewport::default();
        let mut state = SystemState::<Query<Entity, With<MainViewportCamera>>>::new(&mut world);
        let cameras = state.get(&world).unwrap();
        assert_eq!(resolve_frame_camera(&active, &cameras), Some(camera));
    }

    /// Two viewports and nothing hovered is ambiguous, so no camera resolves.
    #[test]
    fn nothing_hovered_with_two_cameras_resolves_to_nothing() {
        let mut world = World::new();
        world.spawn(MainViewportCamera);
        world.spawn(MainViewportCamera);
        let active = ActiveViewport::default();
        let mut state = SystemState::<Query<Entity, With<MainViewportCamera>>>::new(&mut world);
        let cameras = state.get(&world).unwrap();
        assert_eq!(resolve_frame_camera(&active, &cameras), None);
    }

    #[test]
    fn nothing_hovered_and_no_camera_resolves_to_nothing() {
        let mut world = World::new();
        let active = ActiveViewport::default();
        let mut state = SystemState::<Query<Entity, With<MainViewportCamera>>>::new(&mut world);
        let cameras = state.get(&world).unwrap();
        assert_eq!(resolve_frame_camera(&active, &cameras), None);
    }

    /// A dolly travels the sightline and leaves the aim alone, so a run of them is a
    /// straight approach rather than a drift.
    #[test]
    fn a_dolly_moves_along_the_sightline_and_keeps_the_aim() {
        let mut world = World::new();
        world.init_resource::<ActiveViewport>();
        let start = Transform::from_xyz(0.0, 10.0, 40.0).looking_at(Vec3::ZERO, Vec3::Y);
        world.spawn((MainViewportCamera, start));

        let params = OperatorParameters(
            [(
                "distance".to_string(),
                jackdaw_scene_types::PropertyValue::Float(10.0),
            )]
            .into(),
        );
        let outcome = world
            .run_system_cached_with(view_dolly, params)
            .expect("the operator runs");
        assert!(matches!(outcome, OperatorResult::Finished));

        let mut cameras = world.query_filtered::<&Transform, With<MainViewportCamera>>();
        let moved = cameras.single(&world).expect("the one camera");
        assert_eq!(moved.rotation, start.rotation, "a dolly must not re-aim");
        let travelled = moved.translation - start.translation;
        assert!(
            (travelled.length() - 10.0).abs() < 1.0e-3,
            "a dolly of ten metres moved {travelled:?}",
        );
        assert!(
            travelled.normalize().dot(start.forward().as_vec3()) > 0.999,
            "a positive dolly must move toward what the camera is looking at",
        );
    }

    /// Spread over frames, the camera sits at an intermediate position on each and has
    /// travelled the whole distance at the end.
    #[test]
    fn a_spread_dolly_arrives_over_the_frames_it_was_given() {
        let mut world = World::new();
        world.init_resource::<ActiveViewport>();
        let start = Transform::from_xyz(0.0, 0.0, 40.0).looking_at(Vec3::ZERO, Vec3::Y);
        world.spawn((MainViewportCamera, start));

        let params = OperatorParameters(
            [
                (
                    "distance".to_string(),
                    jackdaw_scene_types::PropertyValue::Float(20.0),
                ),
                (
                    "frames".to_string(),
                    jackdaw_scene_types::PropertyValue::Int(4),
                ),
            ]
            .into(),
        );
        let outcome = world
            .run_system_cached_with(view_dolly, params)
            .expect("the operator runs");
        assert!(matches!(outcome, OperatorResult::Finished));

        let travelled = |world: &mut World| {
            let mut cameras = world.query_filtered::<&Transform, With<MainViewportCamera>>();
            let at = cameras.single(world).expect("the one camera").translation;
            (at - start.translation).length()
        };
        assert_eq!(
            travelled(&mut world),
            0.0,
            "nothing moves until a frame runs"
        );

        for frame in 1..=4 {
            world
                .run_system_cached(drive_dolly)
                .expect("the driver runs");
            world.flush();
            let so_far = travelled(&mut world);
            assert!(
                (so_far - frame as f32 * 5.0).abs() < 1.0e-3,
                "frame {frame} had travelled {so_far}",
            );
        }
        assert!(
            !world.contains_resource::<DollyInFlight>(),
            "a finished dolly must stop driving the camera",
        );
    }
}

#[cfg(test)]
mod world_bounds_tests {
    use bevy::ecs::system::SystemState;

    use super::*;

    /// Run [`world_bounds`] against a world, as the framing operators reach it.
    fn bounds_of(world: &mut World, entity: Entity) -> Option<(Vec3, Vec3)> {
        let mut state: SystemState<(
            Query<&Children>,
            Query<(&GlobalTransform, &Aabb), Without<crate::ViewDependentBounds>>,
        )> = SystemState::new(world);
        let (children, bounded) = state.get(world).expect("the queries resolve");
        world_bounds(entity, &children, &bounded)
    }

    fn unit_box(world: &mut World, at: Vec3, half: f32) -> Entity {
        world
            .spawn((
                GlobalTransform::from_translation(at),
                Aabb {
                    center: Vec3::ZERO.into(),
                    half_extents: Vec3::splat(half).into(),
                },
            ))
            .id()
    }

    /// An entity the renderer has no bounds for (a light, an empty) reports nothing, so the
    /// framing operators fall back to its transform.
    #[test]
    fn an_entity_with_no_bounds_anywhere_under_it_reports_none() {
        let mut world = World::new();
        let entity = world.spawn(GlobalTransform::default()).id();
        assert_eq!(bounds_of(&mut world, entity), None);
    }

    #[test]
    fn an_entitys_own_aabb_is_its_bounds() {
        let mut world = World::new();
        let entity = unit_box(&mut world, Vec3::ZERO, 2.0);
        let (min, max) = bounds_of(&mut world, entity).expect("a bounded entity has bounds");
        assert_eq!(min, Vec3::splat(-2.0));
        assert_eq!(max, Vec3::splat(2.0));
    }

    /// Geometry hangs off children (a terrain's LOD levels, an imported model's parts), so
    /// an entity with no bounds of its own reports the room they take.
    #[test]
    fn bounds_come_from_descendants_when_the_entity_has_none() {
        let mut world = World::new();
        let near = unit_box(&mut world, Vec3::new(-10.0, 0.0, 0.0), 1.0);
        let far = unit_box(&mut world, Vec3::new(10.0, 0.0, 0.0), 1.0);
        let parent = world.spawn(GlobalTransform::default()).id();
        world.entity_mut(parent).add_children(&[near, far]);

        let (min, max) = bounds_of(&mut world, parent).expect("the children have bounds");
        assert_eq!(min.x, -11.0);
        assert_eq!(max.x, 11.0);
    }

    /// A kilometre of terrain sitting at the origin measures a kilometre; framing it by its
    /// transform alone puts the camera inside the ground.
    #[test]
    fn a_large_surface_at_the_origin_reports_its_extent_not_its_point() {
        let mut world = World::new();
        let terrain = unit_box(&mut world, Vec3::ZERO, 512.0);

        let (min, max) = bounds_of(&mut world, terrain).expect("the terrain has bounds");
        assert_eq!((max - min).x, 1024.0);
        assert!(
            ((max - min).length() * 0.5) * FRAME_MARGIN > 1000.0,
            "the framing distance has to clear the terrain",
        );
    }

    /// A clipmap ring reaches as far as the viewer stands, so framing on it would walk the
    /// camera backwards further on every call.
    #[test]
    fn view_dependent_geometry_is_not_measured() {
        let mut world = World::new();
        let terrain = unit_box(&mut world, Vec3::ZERO, 512.0);
        let ring = world
            .spawn((
                GlobalTransform::from_translation(Vec3::new(0.0, 0.0, 4000.0)),
                Aabb {
                    center: Vec3::ZERO.into(),
                    half_extents: Vec3::splat(4000.0).into(),
                },
                crate::ViewDependentBounds,
            ))
            .id();
        world.entity_mut(terrain).add_children(&[ring]);

        let (min, max) = bounds_of(&mut world, terrain).expect("the terrain has bounds");
        assert_eq!(
            (max - min).z,
            1024.0,
            "the ring that follows the camera must not widen the frame",
        );
    }

    /// A rotated box does not stay axis-aligned, so its corners set the bounds rather than
    /// its extents taken straight across.
    #[test]
    fn a_rotated_box_is_measured_by_its_corners() {
        let mut world = World::new();
        let entity = world
            .spawn((
                GlobalTransform::from(Transform::from_rotation(Quat::from_rotation_y(
                    std::f32::consts::FRAC_PI_4,
                ))),
                Aabb {
                    center: Vec3::ZERO.into(),
                    half_extents: Vec3::new(1.0, 1.0, 1.0).into(),
                },
            ))
            .id();
        let (min, max) = bounds_of(&mut world, entity).expect("a bounded entity has bounds");
        let expected = std::f32::consts::SQRT_2;
        assert!((max.x - expected).abs() < 1e-4, "max.x was {}", max.x);
        assert!((min.x + expected).abs() < 1e-4, "min.x was {}", min.x);
    }
}
