//! Contract tests for the `brush.*` operators. Each op gates on global editor
//! state (`Selection`, `BrushSelection`, `EditMode`) through its own
//! `is_available` check, so the Edit menu and the command palette can grey the
//! entry when it would be a no-op.
use crate::util;

use bevy::prelude::*;
use jackdaw_api::prelude::*;

fn spawn_cuboid_brush(app: &mut App, offset: Vec3) -> Entity {
    use jackdaw_scene_types::Brush;
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
    // The headless app starts with `InputFocus = Some(placeholder)`, which the
    // brush ops read as a text field owning the keyboard.
    app.world_mut().resource_mut::<InputFocus>().clear();
    f(&mut app);
}

#[test]
fn brush_join_unavailable_without_two_brushes() {
    use jackdaw::selection::Selection;

    with_headless_brush_env(|app| {
        assert!(
            !app.world_mut()
                .operator("brush.join")
                .is_available()
                .unwrap()
        );

        let b1 = spawn_cuboid_brush(app, Vec3::ZERO);
        app.world_mut().resource_mut::<Selection>().entities = vec![b1];
        app.update();
        assert!(
            !app.world_mut()
                .operator("brush.join")
                .is_available()
                .unwrap()
        );

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

        assert!(!app.world_mut().operator(op).is_available().unwrap());

        // No remembered face: the op needs either a face-mode pick or a remembered
        // face on the primary.
        let b1 = spawn_cuboid_brush(app, Vec3::ZERO);
        let b2 = spawn_cuboid_brush(app, Vec3::X);
        app.world_mut().resource_mut::<Selection>().entities = vec![b1, b2];
        app.update();
        assert!(!app.world_mut().operator(op).is_available().unwrap());

        {
            let mut brush_selection = app.world_mut().resource_mut::<BrushSelection>();
            brush_selection.last_face_entity = Some(b1);
            brush_selection.last_face_index = Some(0);
        }
        app.update();
        assert!(app.world_mut().operator(op).is_available().unwrap());

        *app.world_mut().resource_mut::<EditMode>() = EditMode::BrushEdit(BrushEditMode::Face);
        {
            let mut brush_selection = app.world_mut().resource_mut::<BrushSelection>();
            brush_selection.active_brush = Some(b1);
            brush_selection.sub_mut(b1).faces = vec![0];
        }
        app.update();
        assert!(app.world_mut().operator(op).is_available().unwrap());
    });
}
