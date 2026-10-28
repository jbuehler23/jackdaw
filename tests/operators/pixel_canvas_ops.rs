//! `camera.pixel_canvas` and the Pixel Camera entry of the 2D Add group.

use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::prelude::*;
use jackdaw::commands::CommandHistory;
use jackdaw_api::prelude::*;
use jackdaw_scene_types::{CanvasScale, PixelCanvas, PropertyValue};

fn two_d_app() -> (App, Entity) {
    let mut app = util::editor_test_app();
    let root = jackdaw::entity_ops::seed_2d_scene_root(app.world_mut());
    jackdaw::selection::clear_selection_in_world(app.world_mut());
    app.update();
    (app, root)
}

fn with_camera() -> (App, Entity) {
    let (mut app, root) = two_d_app();
    let camera = app
        .world_mut()
        .spawn((Name::new("Game Camera"), Camera2d, ChildOf(root)))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), camera);
    jackdaw::selection::clear_selection_in_world(app.world_mut());
    app.update();
    (app, camera)
}

fn call(app: &mut App, params: &[(&'static str, PropertyValue)]) -> OperatorResult {
    let mut call = app.world_mut().operator("camera.pixel_canvas");
    for (key, value) in params {
        call = call.param(*key, value.clone());
    }
    let result = call.call().expect("camera.pixel_canvas dispatches");
    app.update();
    result
}

fn history_len(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

fn undo(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    app.update();
}

fn canvas(app: &App, camera: Entity) -> Option<PixelCanvas> {
    app.world().get::<PixelCanvas>(camera).copied()
}

#[test]
fn a_canvas_is_added_with_the_defaults_as_one_entry_and_undone_whole() {
    let (mut app, camera) = with_camera();
    let before = history_len(&app);
    call(&mut app, &[]).assert_finished();
    assert_eq!(canvas(&app, camera), Some(PixelCanvas::new(320, 180)));
    assert_eq!(history_len(&app), before + 1, "one history entry");

    undo(&mut app);
    assert_eq!(canvas(&app, camera), None, "undo takes the canvas off");
}

#[test]
fn fields_update_as_one_entry_and_undo_back() {
    let (mut app, camera) = with_camera();
    call(&mut app, &[]).assert_finished();
    let before = history_len(&app);
    call(
        &mut app,
        &[
            ("width", PropertyValue::Int(480)),
            ("height", PropertyValue::Int(270)),
            ("scale", PropertyValue::String("fit".into())),
            ("letterbox", PropertyValue::String("#ff0000".into())),
            ("pixel_snap", PropertyValue::Bool(false)),
        ],
    )
    .assert_finished();
    assert_eq!(
        canvas(&app, camera),
        Some(PixelCanvas {
            resolution: UVec2::new(480, 270),
            scale: CanvasScale::Fit,
            letterbox: Color::srgb(1.0, 0.0, 0.0),
            pixel_snap: false,
        })
    );
    assert_eq!(history_len(&app), before + 1, "one history entry");

    call(&mut app, &[("height", PropertyValue::Int(360))]).assert_finished();
    assert_eq!(
        canvas(&app, camera).map(|canvas| canvas.resolution),
        Some(UVec2::new(480, 360)),
        "a side left out keeps its value"
    );

    undo(&mut app);
    undo(&mut app);
    assert_eq!(canvas(&app, camera), Some(PixelCanvas::new(320, 180)));
}

#[test]
fn remove_takes_the_canvas_off_as_one_entry() {
    let (mut app, camera) = with_camera();
    call(&mut app, &[("width", PropertyValue::Int(384))]).assert_finished();
    let before = history_len(&app);
    call(&mut app, &[("remove", PropertyValue::Bool(true))]).assert_finished();
    assert_eq!(canvas(&app, camera), None);
    assert_eq!(history_len(&app), before + 1);

    undo(&mut app);
    assert_eq!(
        canvas(&app, camera).map(|canvas| canvas.resolution),
        Some(UVec2::new(384, 180))
    );
    assert_eq!(
        call(&mut app, &[("remove", PropertyValue::Bool(true))]),
        OperatorResult::Finished
    );
    assert_eq!(
        call(&mut app, &[("remove", PropertyValue::Bool(true))]),
        OperatorResult::Cancelled,
        "nothing to remove"
    );
}

#[test]
fn a_camera_less_scene_and_zero_sizes_are_refused() {
    let (mut app, _root) = two_d_app();
    let before = history_len(&app);
    assert_eq!(call(&mut app, &[]), OperatorResult::Cancelled);
    assert_eq!(history_len(&app), before);

    let (mut app, camera) = with_camera();
    let before = history_len(&app);
    for side in ["width", "height"] {
        assert_eq!(
            call(&mut app, &[(side, PropertyValue::Int(0))]),
            OperatorResult::Cancelled,
            "{side}=0"
        );
    }
    assert_eq!(canvas(&app, camera), None, "a refused add leaves no canvas");
    assert_eq!(history_len(&app), before);
}

#[test]
fn the_selected_camera_wins_over_the_scene_rule() {
    let (mut app, camera) = with_camera();
    let root = app
        .world()
        .get::<ChildOf>(camera)
        .map(ChildOf::parent)
        .expect("under the root");
    let second = app.world_mut().spawn((Camera2d, ChildOf(root))).id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), second);
    jackdaw::selection::clear_selection_in_world(app.world_mut());
    app.update();
    assert_eq!(
        call(&mut app, &[]),
        OperatorResult::Cancelled,
        "two cameras and none selected"
    );

    jackdaw::selection::select_only(app.world_mut(), second);
    app.update();
    call(&mut app, &[]).assert_finished();
    assert!(canvas(&app, second).is_some());
    assert!(canvas(&app, camera).is_none());
}

#[test]
fn the_pixel_camera_entry_adds_a_canvas_camera_under_the_scene_root() {
    let (mut app, root) = two_d_app();
    let before = history_len(&app);
    app.world_mut()
        .operator("entity.add.pixel_camera")
        .call()
        .expect("dispatch")
        .assert_finished();
    for _ in 0..3 {
        app.update();
    }
    let found: Vec<(Entity, PixelCanvas)> = app
        .world_mut()
        .query_filtered::<(Entity, &PixelCanvas), With<Camera2d>>()
        .iter(app.world())
        .map(|(entity, canvas)| (entity, *canvas))
        .collect();
    assert_eq!(found.len(), 1);
    let (camera, canvas) = found[0];
    assert_eq!(canvas, PixelCanvas::new(320, 180));
    assert_eq!(
        app.world().get::<ChildOf>(camera).map(ChildOf::parent),
        Some(root)
    );
    assert_eq!(history_len(&app), before + 1);
    assert!(
        app.world()
            .get::<jackdaw::entity_ops::SceneCamera>(camera)
            .is_some(),
        "it is a scene camera"
    );
    assert_eq!(
        app.world()
            .get::<Camera>(camera)
            .map(|camera| camera.is_active),
        Some(false),
        "it never draws into the editor's window"
    );

    let dir = tempfile::tempdir().expect("tempdir");
    let text = jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), dir.path())
        .expect("the scene emits");
    assert!(
        text.contains("Camera2d") && text.contains("PixelCanvas"),
        "the camera saves with its canvas:\n{text}"
    );
    assert!(!text.contains("SubEntity"), "and nothing the renderer adds");
    assert!(
        !text.contains("is_active: false") && !text.contains("RenderTarget::None"),
        "and saves active, so the game draws through it:\n{text}"
    );

    undo(&mut app);
    assert!(app.world().get_entity(camera).is_err(), "undo removes it");
}

#[test]
fn a_camera_outside_the_scene_document_is_refused() {
    let (mut app, root) = two_d_app();
    let camera = app.world_mut().spawn((Camera2d, ChildOf(root))).id();
    app.update();
    let before = history_len(&app);
    assert_eq!(
        call(&mut app, &[("entity", PropertyValue::Entity(camera))]),
        OperatorResult::Cancelled
    );
    assert_eq!(canvas(&app, camera), None);
    assert_eq!(history_len(&app), before);
}
