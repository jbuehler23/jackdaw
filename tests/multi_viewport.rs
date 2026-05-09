//! Multi-viewport behavioural coverage.
//!
//! These tests stand up a headless editor app, attach `MainViewportCamera`
//! entities by hand (the dock-tree reconciler that normally spawns them
//! only runs after entering `AppState::Editor`, which a test wouldn't
//! otherwise drive), point `ActiveViewport` at one of them, and dispatch
//! the per-viewport view operators. The point is to pin two contracts:
//!
//! 1. `view.set_axis`, `view.toggle_persp_ortho`, and `view.frame_*`
//!    only mutate the camera referenced by `ActiveViewport`, never any
//!    sibling viewport. With multiple `MainViewportCamera`s in the
//!    world, a `Single<>`-based op would have panicked here before the
//!    multi-viewport refactor.
//! 2. `view.set_axis` rotates the active viewport's per-instance
//!    `InfiniteGrid`, so axis-aligned views aren't edge-on. Other
//!    viewports' grids must stay put.

use bevy::prelude::*;
use bevy_infinite_grid::InfiniteGridBundle;
use jackdaw::viewport::{ActiveViewport, MainViewportCamera, ViewportConfig, ViewportGrid};
use jackdaw_api::prelude::*;
use jackdaw_jsn::PropertyValue;

mod util;

use crate::util::OperatorResultExt as _;

/// Parameter helper: build a typed `i64` for the axis / sign params
/// that `view.set_axis` expects. `OperatorParameters` carries
/// `PropertyValue`s, not raw ints.
fn int_param(name: &str, value: i64) -> (String, PropertyValue) {
    (name.to_string(), PropertyValue::Int(value))
}

/// Spawn a `MainViewportCamera` carrying a `ViewportGrid` link to a
/// fresh per-viewport `InfiniteGrid` and a `ViewportConfig` (so view
/// ops can read/write its bookmarks without panicking on a missing
/// component). Returns `(camera, grid)`.
fn spawn_viewport(world: &mut World, position: Vec3) -> (Entity, Entity) {
    let grid = world.spawn(InfiniteGridBundle::default()).id();
    let camera = world
        .spawn((
            MainViewportCamera,
            Transform::from_translation(position).looking_at(Vec3::ZERO, Vec3::Y),
            Projection::Perspective(PerspectiveProjection::default()),
            ViewportGrid(grid),
            ViewportConfig::default(),
        ))
        .id();
    (camera, grid)
}

/// Drive `view.set_axis` on the active viewport with the given axis
/// (0 = X / 1 = Y / 2 = Z) and sign (positive when omitted).
fn dispatch_set_axis(world: &mut World, axis: i64) {
    world
        .operator("view.set_axis")
        .param("axis", axis)
        .param("sign", 1_i64)
        .call()
        .expect("view.set_axis dispatch resolved")
        .assert_finished();
}

#[test]
fn set_axis_only_touches_active_viewport() {
    let mut app = util::editor_test_app();
    let world = app.world_mut();

    let (cam_a, _) = spawn_viewport(world, Vec3::new(5.0, 5.0, 10.0));
    let (cam_b, _) = spawn_viewport(world, Vec3::new(-5.0, 5.0, 10.0));

    let cam_a_pose_before = *world.get::<Transform>(cam_a).unwrap();
    let cam_b_pose_before = *world.get::<Transform>(cam_b).unwrap();

    // Point ActiveViewport at A and snap to top view.
    world.resource_mut::<ActiveViewport>().camera = Some(cam_a);
    let _ = (int_param, dispatch_set_axis); // silence unused-helper warnings if reused later
    dispatch_set_axis(world, 1); // Y axis

    // A moved (top view repositions the camera along +Y).
    let cam_a_pose_after = *world.get::<Transform>(cam_a).unwrap();
    assert_ne!(
        cam_a_pose_before.translation, cam_a_pose_after.translation,
        "view.set_axis must reposition the active viewport's camera",
    );
    assert!(matches!(
        world.get::<Projection>(cam_a).unwrap(),
        Projection::Orthographic(_)
    ));

    // B was never targeted; its transform/projection must be untouched.
    let cam_b_pose_after = *world.get::<Transform>(cam_b).unwrap();
    assert_eq!(
        cam_b_pose_before.translation, cam_b_pose_after.translation,
        "view.set_axis must not move sibling viewports",
    );
    assert!(matches!(
        world.get::<Projection>(cam_b).unwrap(),
        Projection::Perspective(_),
    ));
}

#[test]
fn set_axis_rotates_only_active_viewports_grid() {
    // Reproduces the "axis-aligned view ate my grid" bug: with the
    // global single-grid model, snapping to front view rotated the
    // shared grid, hiding it for every other panel. After the
    // per-viewport split, only the active viewport's grid rotates.
    let mut app = util::editor_test_app();
    let world = app.world_mut();

    let (cam_a, grid_a) = spawn_viewport(world, Vec3::new(5.0, 5.0, 10.0));
    let (_, grid_b) = spawn_viewport(world, Vec3::new(-5.0, 5.0, 10.0));

    world.resource_mut::<ActiveViewport>().camera = Some(cam_a);
    dispatch_set_axis(world, 2); // Z axis (front view)

    // A's grid rotated to face the front view (XY plane, ~90° around X).
    let grid_a_rot = world.get::<Transform>(grid_a).unwrap().rotation;
    assert!(
        (grid_a_rot.x.abs() - (std::f32::consts::FRAC_PI_2 / 2.0).sin()).abs() < 1e-3,
        "active viewport's grid should rotate to face the front view; got {grid_a_rot:?}",
    );

    // B's grid is still identity (it was never targeted).
    let grid_b_rot = world.get::<Transform>(grid_b).unwrap().rotation;
    assert_eq!(
        grid_b_rot,
        Quat::IDENTITY,
        "sibling viewport's grid must keep its identity orientation",
    );
}

#[test]
fn toggle_persp_ortho_only_targets_active_viewport() {
    let mut app = util::editor_test_app();
    let world = app.world_mut();

    let (cam_a, _) = spawn_viewport(world, Vec3::new(5.0, 5.0, 10.0));
    let (cam_b, _) = spawn_viewport(world, Vec3::new(-5.0, 5.0, 10.0));

    world.resource_mut::<ActiveViewport>().camera = Some(cam_a);
    world
        .operator("view.toggle_persp_ortho")
        .call()
        .expect("view.toggle_persp_ortho dispatch resolved")
        .assert_finished();

    assert!(matches!(
        world.get::<Projection>(cam_a).unwrap(),
        Projection::Orthographic(_)
    ));
    assert!(matches!(
        world.get::<Projection>(cam_b).unwrap(),
        Projection::Perspective(_),
    ));
}

#[test]
fn no_active_viewport_makes_view_ops_cancel() {
    // ActiveViewport defaults to None. View operators that target the
    // active viewport must surface that as `Cancelled`, never panic.
    let mut app = util::editor_test_app();
    let world = app.world_mut();

    // Spawn a couple of viewports but leave ActiveViewport unset.
    let (_, _) = spawn_viewport(world, Vec3::new(5.0, 5.0, 10.0));
    let (_, _) = spawn_viewport(world, Vec3::new(-5.0, 5.0, 10.0));
    world.resource_mut::<ActiveViewport>().camera = None;

    // `view.set_axis` is gated on an `is_available` predicate that
    // requires an active viewport, so the dispatch comes back as
    // `Cancelled` rather than running the body.
    let result = world
        .operator("view.set_axis")
        .param("axis", 1_i64)
        .call()
        .expect("view.set_axis dispatch resolved");
    assert!(
        matches!(result, OperatorResult::Cancelled),
        "expected Cancelled when no viewport is active, got {result:?}",
    );
}

#[test]
fn many_viewports_dont_panic_view_ops() {
    // Regression for the `Single<MainViewportCamera>` panic: with
    // four cameras in the world, a Single-based system would have
    // errored out and caused the operator to fail. Iterating ops
    // pick the active viewport instead.
    let mut app = util::editor_test_app();
    let world = app.world_mut();

    let (cam_persp, _) = spawn_viewport(world, Vec3::new(5.0, 5.0, 10.0));
    let _ = spawn_viewport(world, Vec3::new(-5.0, 5.0, 10.0));
    let _ = spawn_viewport(world, Vec3::new(0.0, 10.0, 0.0));
    let _ = spawn_viewport(world, Vec3::new(0.0, 0.0, 10.0));

    world.resource_mut::<ActiveViewport>().camera = Some(cam_persp);
    dispatch_set_axis(world, 0); // X axis (side view)

    assert!(matches!(
        world.get::<Projection>(cam_persp).unwrap(),
        Projection::Orthographic(_),
    ));
}
