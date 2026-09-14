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

/// A brush applied as BSN has to derive a mesh like any other, or a caller
/// building geometry from outside the editor sees nothing where it built.
#[test]
fn a_brush_applied_as_bsn_derives_its_mesh() {
    use jackdaw::remote::server::apply_bsn_handler;
    use serde_json::json;

    let mut app = util::editor_test_app();
    // Brush meshing runs in the editor state only, and reads a material palette
    // that entering it sets up.
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    for _ in 0..4 {
        app.update();
    }

    let authored = spawn_cuboid_brush(&mut app, Vec3::ZERO);
    for _ in 0..4 {
        app.update();
    }
    assert!(
        mesh_children(&mut app, authored) > 0,
        "a brush spawned into the world meshes, so the comparison below means something"
    );

    let source = jackdaw_remote::bsn_methods::entity_bsn(app.world_mut(), authored)
        .expect("the brush writes back as BSN");
    let answer = app
        .world_mut()
        .run_system_cached_with(apply_bsn_handler, Some(json!({ "source": source })))
        .expect("the handler ran")
        .expect("the source applies");
    for _ in 0..4 {
        app.update();
    }

    let applied = answer["entities"]
        .as_array()
        .and_then(|entities| entities.first())
        .and_then(serde_json::Value::as_u64)
        .and_then(Entity::try_from_bits)
        .expect("one entity came back");
    assert!(
        mesh_children(&mut app, applied) > 0,
        "the applied brush has no mesh under it"
    );
}

/// How many meshes hang under an entity, which is what a brush shows.
fn mesh_children(app: &mut App, entity: Entity) -> usize {
    app.world()
        .get::<Children>(entity)
        .into_iter()
        .flatten()
        .filter(|&&child| app.world().get::<Mesh3d>(child).is_some())
        .count()
}

/// A brush spelled out by hand names its planes and nothing else, which is the
/// shape a caller writing BSN has. The mesh comes from the planes.
#[test]
fn a_brush_spelled_only_as_planes_derives_its_mesh() {
    use jackdaw::remote::server::apply_bsn_handler;
    use serde_json::json;

    let mut app = util::editor_test_app();
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    for _ in 0..4 {
        app.update();
    }

    let face = |x: f32, y: f32, z: f32, distance: f32| {
        format!(
            "jackdaw_geometry::BrushFaceData {{ plane: jackdaw_geometry::BrushPlane {{ \
             normal: glam::Vec3 {{ x: {x}, y: {y}, z: {z} }}, distance: {distance} }} }}"
        )
    };
    let faces = [
        face(1.0, 0.0, 0.0, 0.5),
        face(-1.0, 0.0, 0.0, 0.5),
        face(0.0, 1.0, 0.0, 0.5),
        face(0.0, -1.0, 0.0, 0.5),
        face(0.0, 0.0, 1.0, 0.5),
        face(0.0, 0.0, -1.0, 0.5),
    ]
    .join(",\n");
    let source = format!(
        "#Wall\n\
         bevy_transform::components::transform::Transform\n\
         bevy_camera::visibility::Visibility::Inherited\n\
         jackdaw_scene_types::types::Brush {{ faces: [\n{faces}\n] }}\n"
    );

    let answer = app
        .world_mut()
        .run_system_cached_with(apply_bsn_handler, Some(json!({ "source": source })))
        .expect("the handler ran")
        .expect("the source applies");
    for _ in 0..4 {
        app.update();
    }

    let wall = answer["entities"][0]
        .as_u64()
        .and_then(Entity::try_from_bits)
        .expect("one entity came back");
    assert!(
        mesh_children(&mut app, wall) > 0,
        "the hand-spelled brush has no mesh under it"
    );
}
