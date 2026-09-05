//! Aiming the viewport camera without a pointer.
//!
//! `view.frame_all` and `view.frame_selected` keep the orientation they
//! find, so a camera left level with the ground frames a terrain edge-on
//! and shows nothing. Orbit, pan and dolly are camera-controller input
//! rather than operators, so aiming one otherwise takes a pointer.

use bevy::prelude::*;
use jackdaw::viewport::MainViewportCamera;
use jackdaw_api::prelude::*;

use crate::util;
use crate::util::OperatorResultExt as _;

/// An editor with one viewport camera, which is what
/// `resolve_frame_camera` needs to answer without a hovered panel.
fn app_with_a_camera() -> (App, Entity) {
    let mut app = util::editor_test_app();
    let camera = app
        .world_mut()
        .spawn((
            MainViewportCamera,
            Transform::from_xyz(0.0, 0.0, 10.0),
            Projection::Orthographic(OrthographicProjection::default_3d()),
        ))
        .id();
    // `view.set_axis` reads `ActiveViewport` directly rather than falling
    // back to the sole camera, so a headless test has to say which
    // viewport is in front.
    app.world_mut()
        .resource_mut::<jackdaw::viewport::ActiveViewport>()
        .camera = Some(camera);
    app.update();
    (app, camera)
}

fn camera_transform(app: &App, camera: Entity) -> Transform {
    *app.world()
        .get::<Transform>(camera)
        .expect("the camera has a transform")
}

/// `view.look_at` puts the camera where it is told and points it where
/// it is told, which is the whole point: an eye above the ground looking
/// down at it is the shot a terrain needs.
#[test]
fn look_at_places_the_camera_and_aims_it() {
    let (mut app, camera) = app_with_a_camera();
    app.world_mut()
        .operator("view.look_at")
        .param("eye_x", 100.0)
        .param("eye_y", 80.0)
        .param("eye_z", 100.0)
        .param("target_x", 0.0)
        .param("target_y", 0.0)
        .param("target_z", 0.0)
        .call()
        .expect("view.look_at dispatches")
        .assert_finished();
    app.update();

    let transform = camera_transform(&app, camera);
    assert_eq!(transform.translation, Vec3::new(100.0, 80.0, 100.0));
    let to_target = (Vec3::ZERO - transform.translation).normalize();
    assert!(
        transform.forward().as_vec3().dot(to_target) > 0.999,
        "the camera is not looking at the target: forward {:?}",
        transform.forward()
    );
    assert!(
        transform.forward().y < 0.0,
        "an eye above the target must look down, not level: {:?}",
        transform.forward()
    );
}

/// Aiming somewhere arbitrary is a perspective shot. The orthographic
/// projection this viewport may be left in belongs to the axis snaps,
/// and an eye at an angle is not one of those.
#[test]
fn look_at_switches_to_perspective() {
    let (mut app, camera) = app_with_a_camera();
    app.world_mut()
        .operator("view.look_at")
        .param("eye_x", 10.0)
        .param("eye_y", 10.0)
        .param("eye_z", 10.0)
        .call()
        .expect("view.look_at dispatches")
        .assert_finished();
    app.update();

    assert!(
        matches!(
            app.world().get::<Projection>(camera),
            Some(Projection::Perspective(_))
        ),
        "the camera is still orthographic"
    );
}

/// An eye that is also the target names no direction, so the call is
/// refused rather than leaving the camera pointing at whatever
/// `looking_at` makes of a zero-length vector.
#[test]
fn look_at_refuses_an_eye_that_is_the_target() {
    let (mut app, camera) = app_with_a_camera();
    let before = camera_transform(&app, camera);
    let result = app
        .world_mut()
        .operator("view.look_at")
        .param("eye_x", 0.0)
        .param("eye_y", 0.0)
        .param("eye_z", 0.0)
        .call()
        .expect("view.look_at dispatches");
    assert_eq!(result, OperatorResult::Cancelled);
    app.update();
    assert_eq!(
        camera_transform(&app, camera).translation,
        before.translation
    );
}

/// `view.orbit` stands the camera at an angle and a distance from the
/// focus point and looks back at it.
#[test]
fn orbit_stands_at_the_angle_and_distance_it_is_given() {
    let (mut app, camera) = app_with_a_camera();
    app.world_mut()
        .operator("view.look_at")
        .param("eye_x", 0.0)
        .param("eye_y", 50.0)
        .param("eye_z", 50.0)
        .param("target_x", 10.0)
        .param("target_y", 0.0)
        .param("target_z", 20.0)
        .call()
        .expect("view.look_at dispatches")
        .assert_finished();
    app.update();

    app.world_mut()
        .operator("view.orbit")
        .param("yaw", 90.0)
        .param("pitch", 0.0)
        .param("distance", 30.0)
        .call()
        .expect("view.orbit dispatches")
        .assert_finished();
    app.update();

    // Yaw 90, pitch 0: due +X of the focus, level with it.
    let focus = Vec3::new(10.0, 0.0, 20.0);
    let transform = camera_transform(&app, camera);
    assert!(
        (transform.translation - (focus + Vec3::X * 30.0)).length() < 0.01,
        "the camera stands at {:?}, not 30 m due +X of {focus:?}",
        transform.translation
    );
    assert!(
        (transform.translation.distance(focus) - 30.0).abs() < 0.01,
        "the camera is not 30 m from the focus"
    );
}

/// The focus is the point the orbit turns around, so two orbits circle
/// one place rather than each picking their own and drifting away.
#[test]
fn orbits_keep_turning_around_the_same_focus() {
    let (mut app, camera) = app_with_a_camera();
    app.world_mut()
        .operator("view.look_at")
        .param("eye_x", 0.0)
        .param("eye_y", 40.0)
        .param("eye_z", 40.0)
        .param("target_x", 5.0)
        .param("target_y", 1.0)
        .param("target_z", -5.0)
        .call()
        .expect("view.look_at dispatches")
        .assert_finished();
    app.update();

    let focus = Vec3::new(5.0, 1.0, -5.0);
    let mut distances = Vec::new();
    for yaw in [0.0, 45.0, 200.0] {
        app.world_mut()
            .operator("view.orbit")
            .param("yaw", yaw)
            .param("pitch", 35.0)
            .param("distance", 60.0)
            .call()
            .expect("view.orbit dispatches")
            .assert_finished();
        app.update();
        let transform = camera_transform(&app, camera);
        distances.push(transform.translation.distance(focus));
        let to_focus = (focus - transform.translation).normalize();
        assert!(
            transform.forward().as_vec3().dot(to_focus) > 0.999,
            "orbit at yaw {yaw} is not looking at the focus"
        );
    }
    for distance in distances {
        assert!(
            (distance - 60.0).abs() < 0.01,
            "an orbit drifted off the focus: {distance} m away, not 60"
        );
    }
}

/// Straight down is a legal thing to ask for. Clamped short of the pole
/// so the orientation still has a roll to pick, rather than degenerating
/// into whatever `looking_at` does with a parallel up vector.
#[test]
fn orbit_looking_straight_down_stays_a_valid_orientation() {
    let (mut app, camera) = app_with_a_camera();
    app.world_mut()
        .operator("view.orbit")
        .param("yaw", 0.0)
        .param("pitch", 90.0)
        .param("distance", 100.0)
        .call()
        .expect("view.orbit dispatches")
        .assert_finished();
    app.update();

    let transform = camera_transform(&app, camera);
    assert!(
        transform.rotation.is_finite() && transform.rotation.is_normalized(),
        "the orientation is not a rotation: {:?}",
        transform.rotation
    );
    assert!(
        transform.forward().y < -0.99,
        "pitch 90 should look very nearly straight down: {:?}",
        transform.forward()
    );
    assert!(
        transform.translation.y > 99.0,
        "{:?}",
        transform.translation
    );
}

/// The axis snaps read `axis` and `sign`, and now declare them, so a
/// caller can discover them and a value arrives typed by the schema.
#[test]
fn set_axis_takes_the_axis_and_sign_it_declares() {
    let (mut app, camera) = app_with_a_camera();
    app.world_mut()
        .operator("view.set_axis")
        .param("axis", 1_i64)
        .param("sign", -1_i64)
        .call()
        .expect("view.set_axis dispatches")
        .assert_finished();
    app.update();

    let transform = camera_transform(&app, camera);
    assert!(
        transform.translation.y < 0.0,
        "sign -1 on the Y axis stands the camera below the origin: {:?}",
        transform.translation
    );
    assert!(
        transform.forward().y > 0.99,
        "a camera below the origin looks up at it: {:?}",
        transform.forward()
    );
}

/// `view.orbit` circles the focus point, so every gesture that moves the
/// camera has to leave a current one behind. Framing something is the
/// clearest case: the thing just framed is what the next orbit is about.
#[test]
fn frame_selected_leaves_the_orbit_focus_on_what_it_framed() {
    let (mut app, camera) = app_with_a_camera();
    let subject = app
        .world_mut()
        .spawn((Name::new("Subject"), Transform::from_xyz(40.0, 0.0, -25.0)))
        .id();
    jackdaw::selection::select_only(app.world_mut(), subject);
    app.update();

    app.world_mut()
        .operator("view.frame_selected")
        .call()
        .expect("view.frame_selected dispatches")
        .assert_finished();
    app.update();

    let focus = app
        .world()
        .get::<jackdaw::view_ops::ViewportFocus>(camera)
        .expect("framing names a focus point");
    assert!(
        focus.0.distance(Vec3::new(40.0, 0.0, -25.0)) < 0.001,
        "the focus is not on what was framed: {:?}",
        focus.0
    );
}
