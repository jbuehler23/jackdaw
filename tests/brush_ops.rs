//! Headless contract tests for the `brush.*` operators.
//!
//! The operators gate on global editor state (`Selection`,
//! `BrushSelection`, `EditMode`) via each op's `is_available` check.
//! These tests pin that contract down: every `brush.*` op must own an
//! `is_available` system that reports true/false based on the state
//! the op documents, so the Edit menu and the command palette can
//! grey the entry when it would be a no-op.
//!
//! A happy-path "spawn brushes, run op, inspect the result" test is
//! tracked as a followup — it needs a full render + transform
//! pipeline that `headless_app()` doesn't spin up.
use bevy::prelude::*;
use jackdaw_api::prelude::*;

mod util;

fn spawn_cuboid_brush(app: &mut App, offset: Vec3) -> Entity {
    use jackdaw_jsn::Brush;
    app.world_mut()
        .spawn((
            Name::new("TestBrush"),
            Brush::cuboid(0.5, 0.5, 0.5),
            Transform::from_translation(offset),
            Visibility::default(),
        ))
        .id()
}

fn with_headless_brush_env<F: FnOnce(&mut App)>(f: F) {
    use bevy::input_focus::InputFocus;
    let mut app = util::headless_app();
    app.finish();
    app.update();
    // The headless app starts with `InputFocus = Some(placeholder)`;
    // the brush ops' availability checks treat that as "a text field
    // owns the keyboard" and refuse to run. Clear it so tests see the
    // same state as an editor with the viewport focused.
    app.world_mut().resource_mut::<InputFocus>().0 = None;
    f(&mut app);
}

#[test]
fn brush_join_unavailable_without_two_brushes() {
    use jackdaw::selection::Selection;

    with_headless_brush_env(|app| {
        // Empty selection → not available.
        assert!(
            !app.world_mut()
                .operator("brush.join")
                .is_available()
                .unwrap()
        );

        // One selected brush → still not available.
        let b1 = spawn_cuboid_brush(app, Vec3::ZERO);
        app.world_mut().resource_mut::<Selection>().entities = vec![b1];
        app.update();
        assert!(
            !app.world_mut()
                .operator("brush.join")
                .is_available()
                .unwrap()
        );

        // Two selected brushes → available.
        let b2 = spawn_cuboid_brush(app, Vec3::X);
        app.world_mut().resource_mut::<Selection>().entities = vec![b1, b2];
        app.update();
        assert!(
            app.world_mut()
                .operator("brush.join")
                .is_available()
                .unwrap()
        );
    });
}

#[test]
fn brush_csg_subtract_unavailable_without_two_brushes() {
    use jackdaw::selection::Selection;

    with_headless_brush_env(|app| {
        let b1 = spawn_cuboid_brush(app, Vec3::ZERO);
        app.world_mut().resource_mut::<Selection>().entities = vec![b1];
        app.update();
        assert!(
            !app.world_mut()
                .operator("brush.csg_subtract")
                .is_available()
                .unwrap()
        );

        let b2 = spawn_cuboid_brush(app, Vec3::X);
        app.world_mut().resource_mut::<Selection>().entities = vec![b1, b2];
        app.update();
        assert!(
            app.world_mut()
                .operator("brush.csg_subtract")
                .is_available()
                .unwrap()
        );
    });
}

#[test]
fn brush_csg_intersect_unavailable_without_two_brushes() {
    use jackdaw::selection::Selection;

    with_headless_brush_env(|app| {
        let b1 = spawn_cuboid_brush(app, Vec3::ZERO);
        app.world_mut().resource_mut::<Selection>().entities = vec![b1];
        app.update();
        assert!(
            !app.world_mut()
                .operator("brush.csg_intersect")
                .is_available()
                .unwrap()
        );

        let b2 = spawn_cuboid_brush(app, Vec3::X);
        app.world_mut().resource_mut::<Selection>().entities = vec![b1, b2];
        app.update();
        assert!(
            app.world_mut()
                .operator("brush.csg_intersect")
                .is_available()
                .unwrap()
        );
    });
}

#[test]
fn brush_extend_face_unavailable_without_resolvable_face() {
    use jackdaw::brush::{BrushEditMode, BrushSelection, EditMode};
    use jackdaw::selection::Selection;

    with_headless_brush_env(|app| {
        let op = "brush.extend_face_to_brush";

        // Object mode + empty selection: no primary, no face.
        assert!(!app.world_mut().operator(op).is_available().unwrap());

        // Object mode + 2 brushes but no remembered face: still not
        // available (the op needs either a face-mode pick or a
        // remembered face on the primary).
        let b1 = spawn_cuboid_brush(app, Vec3::ZERO);
        let b2 = spawn_cuboid_brush(app, Vec3::X);
        app.world_mut().resource_mut::<Selection>().entities = vec![b1, b2];
        app.update();
        assert!(!app.world_mut().operator(op).is_available().unwrap());

        // Object mode + 2 brushes + a remembered face on the primary:
        // now available.
        {
            let mut brush_selection = app.world_mut().resource_mut::<BrushSelection>();
            brush_selection.last_face_entity = Some(b1);
            brush_selection.last_face_index = Some(0);
        }
        app.update();
        assert!(app.world_mut().operator(op).is_available().unwrap());

        // Face mode with a face picked on the primary and ≥ 1 other
        // brush selected: also available.
        *app.world_mut().resource_mut::<EditMode>() = EditMode::BrushEdit(BrushEditMode::Face);
        {
            let mut brush_selection = app.world_mut().resource_mut::<BrushSelection>();
            brush_selection.entity = Some(b1);
            brush_selection.faces = vec![0];
        }
        app.update();
        assert!(app.world_mut().operator(op).is_available().unwrap());
    });
}
