//! A `PixelCanvas` on a scene camera saves with the scene, and the editor
//! never expands it: only the running game draws through the canvas.

use crate::sprite_gizmo::{settle, world_app};
use crate::util;

use bevy::camera::RenderTarget;
use bevy::prelude::*;
use jackdaw_runtime::{PixelCanvasExpanded, PixelCanvasHelper};
use jackdaw_scene_types::{CanvasScale, PixelCanvas};

fn authored() -> PixelCanvas {
    PixelCanvas {
        resolution: UVec2::new(320, 180),
        scale: CanvasScale::Stretch,
        letterbox: Color::srgb(0.25, 0.5, 0.75),
        pixel_snap: false,
    }
}

#[test]
fn a_canvas_camera_saves_reopens_and_stays_unexpanded_in_the_editor() {
    let (mut app, scene) = world_app();
    let camera = app
        .world_mut()
        .spawn((
            Name::new("Game Camera"),
            Camera2d,
            Camera {
                is_active: false,
                ..default()
            },
            authored(),
            Transform::from_xyz(10.4, -3.6, 0.0),
            ChildOf(scene.root),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), camera);
    settle(&mut app);

    assert!(app.world().get::<PixelCanvasExpanded>(camera).is_none());
    assert!(
        !matches!(
            app.world().get::<RenderTarget>(camera),
            Some(RenderTarget::Image(_))
        ),
        "the camera does not draw into a canvas"
    );
    let helpers = app
        .world_mut()
        .query_filtered::<(), With<PixelCanvasHelper>>()
        .iter(app.world())
        .count();
    assert_eq!(helpers, 0, "the editor spawns no canvas view");
    assert_eq!(
        app.world()
            .get::<GlobalTransform>(camera)
            .map(|global| global.translation().x),
        Some(10.4),
        "the editor draws the camera where it is authored"
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("canvas.bsn");
    let text = jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), tmp.path())
        .expect("the scene emits");
    assert!(text.contains("PixelCanvas"), "the canvas is saved:\n{text}");
    std::fs::write(&path, text).expect("write the scene");

    let mut app = util::editor_test_app();
    jackdaw::migrate_dialog::request_open_with_conversion(app.world_mut(), &path);
    settle(&mut app);
    let canvases: Vec<PixelCanvas> = app
        .world_mut()
        .query::<&PixelCanvas>()
        .iter(app.world())
        .copied()
        .collect();
    assert_eq!(canvases, vec![authored()]);
}
