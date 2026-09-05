//! Multi-viewport coverage: the per-viewport view operators touch only the
//! camera `ActiveViewport` names, and `view.set_axis` rotates only that
//! viewport's own `InfiniteGrid`.
//!
//! `MainViewportCamera` entities are attached by hand, because the dock-tree
//! reconciler that normally spawns them only runs after entering
//! `AppState::Editor`.

use crate::util;

use bevy::{dev_tools::infinite_grid::InfiniteGrid, prelude::*};
use jackdaw::viewport::{ActiveViewport, MainViewportCamera, ViewportConfig, ViewportGrid};
use jackdaw_api::prelude::*;
use jackdaw_scene_types::PropertyValue;

use crate::util::OperatorResultExt as _;

/// `OperatorParameters` carries `PropertyValue`s, not raw ints.
fn int_param(name: &str, value: i64) -> (String, PropertyValue) {
    (name.to_string(), PropertyValue::Int(value))
}

/// Spawn a `MainViewportCamera` with a `ViewportGrid` link to a fresh
/// per-viewport `InfiniteGrid` and a `ViewportConfig`. Returns `(camera, grid)`.
fn spawn_viewport(world: &mut World, position: Vec3) -> (Entity, Entity) {
    let grid = world
        .spawn((InfiniteGrid, Transform::default(), Visibility::Inherited))
        .id();
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
/// (0 = X / 1 = Y / 2 = Z).
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

    world.resource_mut::<ActiveViewport>().camera = Some(cam_a);
    let _ = (int_param, dispatch_set_axis); // silence unused-helper warnings if reused later
    dispatch_set_axis(world, 1); // Y axis

    // Top view repositions the camera along +Y.
    let cam_a_pose_after = *world.get::<Transform>(cam_a).unwrap();
    assert_ne!(
        cam_a_pose_before.translation, cam_a_pose_after.translation,
        "view.set_axis must reposition the active viewport's camera",
    );
    assert!(matches!(
        world.get::<Projection>(cam_a).unwrap(),
        Projection::Orthographic(_)
    ));

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
    let mut app = util::editor_test_app();
    let world = app.world_mut();

    let (cam_a, grid_a) = spawn_viewport(world, Vec3::new(5.0, 5.0, 10.0));
    let (_, grid_b) = spawn_viewport(world, Vec3::new(-5.0, 5.0, 10.0));

    world.resource_mut::<ActiveViewport>().camera = Some(cam_a);
    dispatch_set_axis(world, 2); // Z axis (front view)

    // Front view is the world XY plane, ~90 degrees around X.
    let grid_a_rot = world.get::<Transform>(grid_a).unwrap().rotation;
    assert!(
        (grid_a_rot.x.abs() - (std::f32::consts::FRAC_PI_2 / 2.0).sin()).abs() < 1e-3,
        "active viewport's grid should rotate to face the front view; got {grid_a_rot:?}",
    );

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
    // `ActiveViewport` defaults to None, which must surface as `Cancelled`.
    let mut app = util::editor_test_app();
    let world = app.world_mut();

    let (_, _) = spawn_viewport(world, Vec3::new(5.0, 5.0, 10.0));
    let (_, _) = spawn_viewport(world, Vec3::new(-5.0, 5.0, 10.0));
    world.resource_mut::<ActiveViewport>().camera = None;

    // `view.set_axis` is gated on an availability predicate that requires an
    // active viewport, so the dispatch never runs the body.
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
    // With four cameras in the world, a `Single<MainViewportCamera>` system
    // errors out.
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
