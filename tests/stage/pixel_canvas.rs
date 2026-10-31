//! A `PixelCanvas` on a scene camera saves with the scene, and the editor
//! never expands it: only the running game draws through the canvas. The 2D
//! view outlines the frame it shows, behind a toggle.

use crate::sprite_gizmo::{settle, world_app};
use crate::util;
use crate::util::OperatorResultExt as _;
use crate::world_guides::mixed_document;

use bevy::camera::RenderTarget;
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use jackdaw::canvas_snap::CanvasSnap;
use jackdaw::pixel_camera::GameFrameOverlay;
use jackdaw::viewport_2d::Viewport2dPanelHost;
use jackdaw_api::prelude::*;
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

fn view_rows(app: &mut App) -> Vec<(String, String)> {
    app.world_mut()
        .query::<&jackdaw_widgets::menu_bar::MenuBarItem>()
        .iter(app.world())
        .find(|item| item.label == jackdaw::toolbar_2d::CANVAS_VIEW_MENU)
        .expect("the 2D panel's toolbar carries a canvas view menu")
        .actions
        .clone()
}

/// The canvas view menu's game-frame row, read by opening the menu, as the rows are
/// built when the menu is looked at.
fn game_frame_row(app: &mut App) -> Option<String> {
    app.world_mut()
        .operator("menu.open")
        .param("name", jackdaw::toolbar_2d::CANVAS_VIEW_MENU)
        .call()
        .expect("menu.open dispatches")
        .assert_finished();
    app.update();
    view_rows(app)
        .into_iter()
        .find(|(_, label)| label == "Show Game Frame")
        .map(|(action, _)| action)
}

fn set_game_frame(app: &mut App, on: Option<bool>) {
    let mut call = app.world_mut().operator("canvas.game_frame");
    if let Some(on) = on {
        call = call.param("on", on);
    }
    call.call()
        .expect("canvas.game_frame dispatches")
        .assert_finished();
    settle(app);
}

#[test]
fn the_game_frame_toggle_round_trips_through_its_view_row() {
    let (mut app, _scene) = world_app();
    assert!(
        app.world().resource::<CanvasSnap>().show_game_frame,
        "on by default"
    );
    assert_eq!(
        game_frame_row(&mut app).as_deref(),
        Some("[x]op:canvas.game_frame?on=false")
    );

    set_game_frame(&mut app, Some(false));
    assert!(!app.world().resource::<CanvasSnap>().show_game_frame);
    assert_eq!(
        game_frame_row(&mut app).as_deref(),
        Some("[ ]op:canvas.game_frame?on=true")
    );

    set_game_frame(&mut app, None);
    assert!(
        app.world().resource::<CanvasSnap>().show_game_frame,
        "no parameter flips it back"
    );
}

/// The overlay entities drawing game frames, by the panel each draws into.
fn overlays(app: &mut App) -> Vec<(Entity, RenderLayers)> {
    app.world_mut()
        .query::<(&GameFrameOverlay, &RenderLayers)>()
        .iter(app.world())
        .map(|(overlay, layers)| (overlay.host, layers.clone()))
        .collect()
}

fn camera_layers(app: &App, panel: Entity) -> RenderLayers {
    let camera = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .camera;
    app.world()
        .get::<RenderLayers>(camera)
        .cloned()
        .unwrap_or_default()
}

#[test]
fn only_a_panel_showing_the_world_draws_the_game_frame_on_its_own_layer() {
    let (mut app, scene) = world_app();
    let (_, ui_panel, world_panel) = mixed_document(&mut app, &scene);
    let camera = app
        .world_mut()
        .spawn((Camera2d, PixelCanvas::new(320, 180), ChildOf(scene.root)))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), camera);
    settle(&mut app);

    let drawn = overlays(&mut app);
    assert_eq!(drawn.len(), 1, "one overlay, for the world panel");
    let (host, layers) = &drawn[0];
    assert_eq!(*host, world_panel);
    let ui = camera_layers(&app, ui_panel);
    let world = camera_layers(&app, world_panel);
    assert!(layers.intersects(&world), "the world panel draws it");
    assert!(
        !layers.iter().any(|layer| layer == 0) && !layers.intersects(&ui),
        "the UI panel beside it does not: {layers:?} against {ui:?}"
    );

    set_game_frame(&mut app, Some(false));
    assert!(overlays(&mut app).is_empty(), "hidden, the overlay goes");
}
