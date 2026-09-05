//! 2D viewport panel and navigation: building and tearing the panel down,
//! the pan/zoom and cursor math, routing an authored UI scene into it, the
//! Edit/Interact mode, framing, and the screenshot operators.

use crate::util;

use bevy::{
    camera::{NormalizedRenderTarget, RenderTarget},
    image::ToExtents,
    picking::{
        backend::HitData,
        events::{Click, Pointer, Press},
        hover::PickingInteraction,
        pointer::{Location, PointerAction, PointerButton, PointerId, PointerInput, PointerPress},
    },
    prelude::*,
    render::{
        render_resource::{TextureDimension, TextureFormat},
        view::screenshot::{Screenshot, ScreenshotCaptured},
    },
    window::{PrimaryWindow, WindowRef},
};
use jackdaw_api::{op::OperatorWorldExt as _, prelude::JackdawExtension as _};
use jackdaw_scene_types::UiSceneRoot;

use jackdaw::{
    selection::Selection,
    ui_stage::UiSelectionOverlay,
    viewport::{ActiveViewport, VIEWPORT_WINDOW_ID, ViewportPanelHost},
    viewport_2d::{
        DEFAULT_UI_GRID, MAX_UI_GRID, MAX_ZOOM, MIN_UI_GRID, MIN_ZOOM, Scene2dViewport, Ui2dView,
        Viewport2dCamera, Viewport2dGridReadout, Viewport2dGridStep, Viewport2dMode,
        Viewport2dModeSegment, Viewport2dPanelHost, Viewport2dTitle, Viewport2dZoomReadout,
        build_viewport_2d_panel, cursor_area_offset, cursor_stage_offset, fit_view, pan_by,
        request_2d_fit, stepped_ui_grid, target_pixels_per_stage_pixel, world_at, zoom_toward,
    },
    viewport_host::{ViewportHost, ViewportMode},
};

use crate::util::OperatorResultExt as _;

#[test]
fn building_the_panel_wires_camera_target_and_host() {
    let mut app = util::editor_test_app();
    let parent = app.world_mut().spawn(Node::default()).id();
    build_viewport_2d_panel(app.world_mut(), parent);

    let host = app
        .world()
        .get::<Viewport2dPanelHost>(parent)
        .expect("host on panel parent");
    assert_eq!(host.mode, Viewport2dMode::Edit);

    // The 2D presentation's state sits beside the panel's own identity.
    let panel = app
        .world()
        .get::<ViewportHost>(parent)
        .copied()
        .expect("the panel's own state on the same entity");
    assert_eq!(panel.mode, ViewportMode::TwoD);
    assert!(
        app.world().get::<Camera>(host.camera).unwrap().is_active,
        "the shown presentation's camera renders",
    );
    let camera_3d = app
        .world()
        .get::<ViewportPanelHost>(parent)
        .expect("the 3D presentation is built too")
        .camera;
    assert!(
        !app.world().get::<Camera>(camera_3d).unwrap().is_active,
        "the hidden presentation's camera does not",
    );

    let cam = app.world().entity(host.camera);
    assert!(cam.contains::<Camera2d>());
    assert!(cam.contains::<Viewport2dCamera>());
    // Bevy 0.19 keeps the render target in its own component rather
    // than a `Camera::target` field.
    let target = cam.get::<RenderTarget>().expect("camera render target");
    assert!(
        matches!(target, RenderTarget::Image(_)),
        "the 2D viewport camera must render into an image, got {target:?}",
    );

    let stage = app.world().entity(host.stage);
    assert!(stage.contains::<Scene2dViewport>());
    // An `ImageNode`, not a `ViewportNode`: Bevy resizes a `ViewportNode`'s
    // render target to its node's size, reallocating the image mid-gesture.
    let shown = stage
        .get::<ImageNode>()
        .expect("stage shows the camera's image");
    let RenderTarget::Image(rendered) = target else {
        panic!("the 2D viewport camera must render into an image");
    };
    assert_eq!(
        shown.image, rendered.handle,
        "the stage shows exactly the image its camera draws into",
    );

    // The canvas edge is the one line saying where the authored scene stops,
    // so it is the strong border rather than a separator.
    let frame = app
        .world()
        .get::<Children>(host.stage)
        .and_then(|children| children.iter().next())
        .expect("the stage has its frame child");
    assert_eq!(
        app.world()
            .get::<BorderColor>(frame)
            .map(|border| border.top),
        Some(jackdaw_feathers::tokens::BORDER_STRONG),
        "the canvas boundary has to read against the scene behind it",
    );
}

#[test]
fn despawning_the_panel_despawns_its_camera_and_pointer() {
    let mut app = util::editor_test_app();
    let parent = app.world_mut().spawn(Node::default()).id();
    build_viewport_2d_panel(app.world_mut(), parent);

    let (camera, pointer) = app
        .world()
        .get::<Viewport2dPanelHost>(parent)
        .map(|host| (host.camera, host.pointer))
        .expect("host on panel parent");

    app.world_mut().entity_mut(parent).despawn();
    app.update();

    assert!(
        app.world().get_entity(camera).is_err(),
        "panel teardown must despawn the viewport camera",
    );
    assert!(
        app.world().get_entity(pointer).is_err(),
        "and the pointer Interact drives, which would otherwise be left \
         hovering a render target nothing draws into",
    );
}

#[test]
fn zoom_steps_scale_the_projection_and_keep_the_cursor_point_fixed() {
    let view = Ui2dView::default();
    let cursor = Vec2::new(100.0, 50.0);

    let after = zoom_toward(view, cursor, 1.0);
    assert!(
        after.zoom > view.zoom,
        "a positive tick zooms in: {} -> {}",
        view.zoom,
        after.zoom,
    );

    let before = world_at(view, cursor);
    let anchored = world_at(after, cursor);
    assert!(
        (before - anchored).length() < 1e-3,
        "the world point under the cursor must not move: {before:?} -> {anchored:?}",
    );

    let out = zoom_toward(view, cursor, -1.0);
    assert!(out.zoom < view.zoom, "a negative tick zooms out");
    assert!((world_at(view, cursor) - world_at(out, cursor)).length() < 1e-3);
}

#[test]
fn panning_drags_the_scene_along_with_the_cursor() {
    let view = Ui2dView {
        pan: Vec2::ZERO,
        zoom: 2.0,
        ..default()
    };
    let drag = Vec2::new(20.0, 10.0);
    let after = pan_by(view, drag);
    assert_eq!(after.zoom, view.zoom, "a pan never changes the zoom");

    let start = Vec2::new(5.0, -8.0);
    assert!((world_at(view, start) - world_at(after, start + drag)).length() < 1e-3);
}

/// An image target renders at scale factor 1, so a cursor offset left in
/// ui-logical pixels would be off by exactly `scale_factor`.
#[test]
fn the_cursor_lifts_into_render_target_pixels_before_it_reaches_the_canvas() {
    // A stage 400x200 physical px centred at (200, 100), at UI scale 2. The
    // image is the stage's own size, so only the scale factor is in play.
    let inverse_scale = 0.5;
    let centre = Vec2::new(200.0, 100.0);
    let size = Vec2::new(400.0, 200.0);

    // 50 logical px right of the centre is 100 render-target px right.
    let offset = cursor_stage_offset(Vec2::new(150.0, 50.0), centre, size, inverse_scale, 1.0)
        .expect("the cursor is inside the stage");
    assert_eq!(offset, Vec2::new(100.0, 0.0));

    // Bounded by the node's logical rect, as `update_active_viewport` does.
    assert!(
        cursor_stage_offset(Vec2::new(200.0, 50.0), centre, size, inverse_scale, 1.0).is_some()
    );
    assert!(
        cursor_stage_offset(Vec2::new(201.0, 50.0), centre, size, inverse_scale, 1.0).is_none()
    );
    assert!(
        cursor_stage_offset(Vec2::new(150.0, 101.0), centre, size, inverse_scale, 1.0).is_none()
    );
}

/// The panel's image is held at the authored reference size, not the stage's,
/// so a stage measurement is one factor short of authored pixels. That second
/// factor is also where the view's zoom lives, because `place_stage` sizes
/// the stage node to `reference * zoom`.
#[test]
fn the_cursor_also_lifts_through_the_reference_resolution_scale() {
    // A 1280x720 reference shown in a 640x360 stage: every stage pixel is
    // two render-target pixels.
    let stage = Vec2::new(640.0, 360.0);
    let target = UVec2::new(1280, 720);
    let scale = target_pixels_per_stage_pixel(stage, target);
    assert_eq!(scale, 2.0);

    // 100 logical px right of centre, at a UI scale factor of 2, is 200
    // stage px, which is 400 render-target px.
    let centre = Vec2::new(320.0, 180.0);
    let offset = cursor_stage_offset(Vec2::new(260.0, 90.0), centre, stage, 0.5, scale)
        .expect("the cursor is inside the stage");
    assert_eq!(offset, Vec2::new(400.0, 0.0));

    // The hit test is unaffected: the stage's own rect still bounds it.
    assert!(cursor_stage_offset(Vec2::new(320.0, 90.0), centre, stage, 0.5, scale).is_some());
    assert!(cursor_stage_offset(Vec2::new(321.0, 90.0), centre, stage, 0.5, scale).is_none());

    // Runs on every cursor move, including a panel's first layout frame.
    assert_eq!(target_pixels_per_stage_pixel(Vec2::ZERO, target), 1.0);
    // With no UI scene open the image is the stage's own size, so the
    // factor drops out.
    assert_eq!(
        target_pixels_per_stage_pixel(stage, stage.as_uvec2()),
        1.0,
        "an unscaled panel is still 1:1",
    );
}

/// `MouseMotion` reports stage physical pixels and the view is driven in
/// logical ones, so dropping the factor pans twice as fast at 2x scale.
#[test]
fn a_drag_pans_by_the_authored_pixels_the_cursor_crossed() {
    // A display at scale factor 2: two physical pixels per logical one.
    let inverse_scale = 0.5;

    let view = Ui2dView {
        pan: Vec2::ZERO,
        zoom: 2.0,
        ..default()
    };
    // A raw MouseMotion delta, in stage physical pixels.
    let drag = Vec2::new(40.0, -24.0);
    let after = pan_by(view, drag * inverse_scale);

    // 40 physical px right is 20 logical px right; at 2 logical pixels
    // per authored pixel that is 10 authored px, and the scene follows
    // the cursor, so the canvas travels the other way.
    assert_eq!(after.pan, Vec2::new(-10.0, -6.0));

    // The size of the mistake: forgetting the factor pans twice as far.
    assert_eq!(
        pan_by(view, drag).pan,
        after.pan * 2.0,
        "an unconverted delta pans by exactly the scale factor too much",
    );

    let start = Vec2::new(12.0, 5.0);
    assert!(
        (world_at(view, start * inverse_scale) - world_at(after, (start + drag) * inverse_scale))
            .length()
            < 1e-3,
    );
}

/// The area never moves with the view, which is what makes it a usable anchor.
#[test]
fn the_cursor_reaches_the_view_in_the_areas_logical_pixels() {
    // An area 400x200 physical px centred at (200, 100), on a display at
    // scale factor 2: 200x100 logical px, logical x running 0..200.
    let inverse_scale = 0.5;
    let centre = Vec2::new(200.0, 100.0);
    let size = Vec2::new(400.0, 200.0);

    assert_eq!(
        cursor_area_offset(Vec2::new(150.0, 50.0), centre, size, inverse_scale),
        Some(Vec2::new(50.0, 0.0)),
        "50 logical px right of the centre is 50 logical px of pan",
    );
    assert_eq!(
        cursor_area_offset(Vec2::new(100.0, 25.0), centre, size, inverse_scale),
        Some(Vec2::new(0.0, -25.0)),
    );

    // Bounded by the area's own logical rect, so a cursor over another panel
    // never drives this one.
    assert!(cursor_area_offset(Vec2::new(200.0, 50.0), centre, size, inverse_scale).is_some());
    assert!(cursor_area_offset(Vec2::new(201.0, 50.0), centre, size, inverse_scale).is_none());
    assert!(cursor_area_offset(Vec2::new(150.0, 101.0), centre, size, inverse_scale).is_none());
}

#[test]
fn the_zoom_anchor_holds_over_repeated_ticks_at_a_non_unit_scale_factor() {
    let centre = Vec2::new(200.0, 100.0);
    let size = Vec2::new(400.0, 200.0);
    let offset = cursor_area_offset(Vec2::new(160.0, 30.0), centre, size, 0.5)
        .expect("the cursor is inside the area");

    let view = Ui2dView {
        pan: Vec2::new(12.0, -3.0),
        zoom: 1.7,
        ..default()
    };
    let anchored = world_at(view, offset);

    let mut zoomed = view;
    for _ in 0..8 {
        zoomed = zoom_toward(zoomed, offset, 1.0);
        let now = world_at(zoomed, offset);
        assert!(
            (now - anchored).length() < 1e-3,
            "the world point under the cursor drifted to {now:?} from {anchored:?}",
        );
    }
    assert!(zoomed.zoom > view.zoom, "eight ticks zoomed in");
}

#[test]
fn zoom_is_clamped_to_the_usable_range() {
    let cursor = Vec2::new(-30.0, 12.0);
    let far_in = zoom_toward(Ui2dView::default(), cursor, 1000.0);
    assert_eq!(far_in.zoom, MAX_ZOOM);
    let far_out = zoom_toward(Ui2dView::default(), cursor, -1000.0);
    assert_eq!(far_out.zoom, MIN_ZOOM);
}

#[test]
fn world_at_maps_area_pixels_through_pan_and_zoom() {
    // Screen y runs down, the pan runs up, so the sign flips; zoom is
    // logical pixels per authored pixel.
    let view = Ui2dView {
        pan: Vec2::new(10.0, 20.0),
        zoom: 2.0,
        ..default()
    };
    assert_eq!(world_at(view, Vec2::ZERO), Vec2::new(10.0, 20.0));
    assert_eq!(world_at(view, Vec2::new(40.0, 40.0)), Vec2::new(30.0, 0.0));
}

/// The 2D view is per-tab state; the 3D camera round-trip runs alongside it.
#[test]
fn tab_switch_round_trips_the_2d_view() {
    use jackdaw::scenes::{SceneTab, Scenes};

    let mut app = util::editor_test_app();
    let parent = app
        .world_mut()
        .spawn((jackdaw::EditorEntity, Node::default()))
        .id();
    build_viewport_2d_panel(app.world_mut(), parent);

    // The panel's own 3D camera: every panel builds both presentations, so
    // "the first viewport camera" could find a different one.
    let camera_3d = app
        .world()
        .get::<ViewportPanelHost>(parent)
        .expect("the 3D presentation's state")
        .camera;
    app.world_mut()
        .entity_mut(camera_3d)
        .insert(Transform::from_xyz(1.0, 2.0, 3.0));

    {
        let mut scenes = app.world_mut().resource_mut::<Scenes>();
        scenes.tabs.clear();
        scenes.tabs.push(SceneTab::new_untitled(1));
        scenes.tabs.push(SceneTab::new_untitled(2));
        scenes.active = 0;
    }

    let tab_a_view = Ui2dView {
        pan: Vec2::new(120.0, -40.0),
        zoom: 2.5,
        ..default()
    };
    // Through `set_view`, because a framing only travels with a tab once
    // something has chosen it.
    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(parent)
        .expect("host on panel parent")
        .set_view(tab_a_view);

    jackdaw::scenes::swap::swap_active_tab(app.world_mut(), 1);
    assert_eq!(
        app.world().get::<Viewport2dPanelHost>(parent).unwrap().view,
        Ui2dView::default(),
        "a tab that has never been panned starts from the default view",
    );

    let tab_b_view = Ui2dView {
        pan: Vec2::new(-7.0, 3.0),
        zoom: 0.5,
        ..default()
    };
    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(parent)
        .unwrap()
        .set_view(tab_b_view);
    if let Some(mut tf) = app.world_mut().get_mut::<Transform>(camera_3d) {
        *tf = Transform::from_xyz(10.0, 20.0, 30.0);
    }

    jackdaw::scenes::swap::swap_active_tab(app.world_mut(), 0);
    assert_eq!(
        app.world().get::<Viewport2dPanelHost>(parent).unwrap().view,
        tab_a_view,
        "tab A's 2D view comes back on swap-back",
    );
    assert_eq!(
        app.world().get::<Transform>(camera_3d).unwrap().translation,
        Vec3::new(1.0, 2.0, 3.0),
        "the 3D camera round-trip is untouched by the 2D branch",
    );

    jackdaw::scenes::swap::swap_active_tab(app.world_mut(), 1);
    assert_eq!(
        app.world().get::<Viewport2dPanelHost>(parent).unwrap().view,
        tab_b_view,
        "tab B keeps the view it was left with",
    );
}

/// `apply_2d_view` is the only writer of the 2D camera's transform and ortho scale.
#[test]
fn applying_the_view_moves_the_camera_and_scales_the_projection() {
    let mut app = util::editor_test_app();
    let parent = app
        .world_mut()
        .spawn((jackdaw::EditorEntity, Node::default()))
        .id();
    build_viewport_2d_panel(app.world_mut(), parent);

    let camera = app
        .world_mut()
        .get_mut::<Viewport2dPanelHost>(parent)
        .map(|mut host| {
            host.view = Ui2dView {
                pan: Vec2::new(64.0, -16.0),
                zoom: 4.0,
                ..default()
            };
            host.camera
        })
        .expect("host on panel parent");
    // Scheduled inside `EditorInteractionSystems`, gated on `AppState::Editor`,
    // which this app never enters, so run the system directly.
    app.world_mut()
        .run_system_cached(jackdaw::viewport_2d::apply_2d_view)
        .expect("apply_2d_view ran");

    let translation = app.world().get::<Transform>(camera).unwrap().translation;
    assert_eq!(translation.truncate(), Vec2::new(64.0, -16.0));
    let projection = app.world().get::<Projection>(camera).unwrap();
    let Projection::Orthographic(ortho) = projection else {
        panic!("the 2D viewport camera is orthographic");
    };
    assert_eq!(
        ortho.scale, 0.25,
        "ortho scale is the reciprocal of the view's zoom",
    );
}

/// The canvas is a mode of the viewport panel rather than a dock window of
/// its own, but its old id still has to reach the panel that answers for it.
#[test]
fn the_old_2d_window_id_opens_the_viewport_panel() {
    let mut app = op_app();
    let leaf = dock_leaf(&mut app, &["jackdaw.outliner", VIEWPORT_WINDOW_ID]);

    assert!(
        app.world()
            .resource::<jackdaw_panels::WindowRegistry>()
            .get("jackdaw.viewport_2d")
            .is_none(),
        "the canvas is a viewport mode, not a dock window of its own",
    );

    app.world_mut()
        .operator("window.open")
        .param("window_id", "jackdaw.viewport_2d")
        .call()
        .expect("window.open dispatches")
        .assert_finished();
    app.update();

    assert_eq!(
        active_window(&app, leaf).as_deref(),
        Some(VIEWPORT_WINDOW_ID),
        "the old id fronts the panel that answers for it",
    );
}

/// The same id on a workspace where the panel is not docked at all docks it,
/// once, and brings it forward.
#[test]
fn the_old_2d_window_id_docks_the_viewport_panel_when_none_is_open() {
    let mut app = op_app();
    let leaf = dock_leaf(&mut app, &["jackdaw.outliner"]);

    app.world_mut()
        .operator("window.open")
        .param("window_id", "jackdaw.viewport_2d")
        .call()
        .expect("window.open dispatches")
        .assert_finished();
    app.update();

    let windows: Vec<String> = {
        let tree = app.world().resource::<jackdaw_panels::tree::DockTree>();
        tree.get(leaf)
            .and_then(jackdaw_panels::tree::DockNode::as_leaf)
            .expect("the dock still has its one leaf")
            .tabs()
            .map(|(window, _)| window.to_string())
            .collect()
    };
    assert_eq!(
        windows
            .iter()
            .filter(|id| id.as_str() == VIEWPORT_WINDOW_ID)
            .count(),
        1,
        "one viewport tab was docked, got {windows:?}",
    );
    assert_eq!(
        active_window(&app, leaf).as_deref(),
        Some(VIEWPORT_WINDOW_ID),
        "and it is the tab in front",
    );
}

/// The panel's render-target image is held at the scene's
/// `UiSceneRoot::reference_size`, and Bevy derives a root's layout viewport
/// from its target camera's physical size, so half of a 1280-wide reference
/// is 640, not half of the panel.
#[test]
fn a_routed_ui_scene_lays_out_at_reference_resolution() {
    let mut app = util::editor_test_app();
    let parent = app
        .world_mut()
        .spawn((jackdaw::EditorEntity, Node::default()))
        .id();
    build_viewport_2d_panel(app.world_mut(), parent);

    let root = app
        .world_mut()
        .spawn((
            UiSceneRoot {
                reference_size: UVec2::new(1280, 720),
            },
            Node {
                width: percent(100),
                height: percent(100),
                ..default()
            },
        ))
        .id();
    let child = app
        .world_mut()
        .spawn((
            Node {
                width: percent(50),
                height: percent(25),
                ..default()
            },
            ChildOf(root),
        ))
        .id();

    // Routing lands in the frame it is inserted; the resize reaches the
    // camera's target info on the next one, and layout the one after.
    for _ in 0..3 {
        app.update();
    }

    let camera = app
        .world()
        .get::<Viewport2dPanelHost>(parent)
        .expect("host on panel parent")
        .camera;
    assert_eq!(
        app.world()
            .get::<bevy::ui::UiTargetCamera>(root)
            .map(bevy::ui::UiTargetCamera::entity),
        Some(camera),
        "an unparented UI scene root is routed to the 2D panel's camera",
    );

    let size = app
        .world()
        .get::<ComputedNode>(child)
        .expect("the routed child is laid out")
        .size();
    assert_eq!(
        size,
        Vec2::new(640.0, 180.0),
        "a 50%x25% child of a 1280x720 reference computes 640x180",
    );
}

/// An unrouted UI root falls back to `DefaultUiCamera`, the editor's own
/// window, and draws over the editor chrome, so it parks on an inactive 1x1
/// image camera instead.
#[test]
fn closing_the_last_panel_parks_the_ui_scene_root() {
    let mut app = util::editor_test_app();
    let parent = app
        .world_mut()
        .spawn((jackdaw::EditorEntity, Node::default()))
        .id();
    build_viewport_2d_panel(app.world_mut(), parent);

    let root = app
        .world_mut()
        .spawn((
            UiSceneRoot::default(),
            Node {
                width: percent(100),
                height: percent(100),
                ..default()
            },
        ))
        .id();
    let child = app
        .world_mut()
        .spawn((
            Node {
                width: percent(50),
                height: percent(50),
                ..default()
            },
            ChildOf(root),
        ))
        .id();
    for _ in 0..3 {
        app.update();
    }

    let panel_camera = app
        .world()
        .get::<Viewport2dPanelHost>(parent)
        .expect("host on panel parent")
        .camera;
    assert_eq!(
        routed_camera(&mut app, root),
        Some(panel_camera),
        "the root routes to the panel while one is open",
    );
    assert_eq!(
        parking_cameras(&mut app),
        Vec::<Entity>::new(),
        "the parking camera stays unspawned while a panel is open",
    );
    let routed_size = app
        .world()
        .get::<ComputedNode>(child)
        .expect("the routed child is laid out")
        .size();
    assert_eq!(routed_size, Vec2::new(640.0, 360.0));

    app.world_mut().entity_mut(parent).despawn();
    for _ in 0..3 {
        app.update();
    }

    let parking = *parking_cameras(&mut app)
        .first()
        .expect("closing the last panel spawns the parking camera");
    assert_eq!(
        routed_camera(&mut app, root),
        Some(parking),
        "the root must stay routed, on the parking camera",
    );

    // The parking camera can never be picked as the window camera.
    let camera = app.world().get::<Camera>(parking).expect("parking camera");
    assert!(!camera.is_active, "the parking camera must never draw");
    assert!(
        matches!(
            app.world().get::<RenderTarget>(parking),
            Some(RenderTarget::Image(_))
        ),
        "an image target keeps it out of DefaultUiCamera, which only picks window cameras",
    );
    for window_camera in window_cameras(&mut app) {
        assert_ne!(
            routed_camera(&mut app, root),
            Some(window_camera),
            "a parked UI scene must never target a window camera",
        );
    }

    let parked_size = app
        .world()
        .get::<ComputedNode>(child)
        .expect("the parked child is still laid out")
        .size();
    assert!(
        parked_size.x <= 1.0 && parked_size.y <= 1.0,
        "a parked scene collapses to the 1x1 target, was {routed_size:?}, now {parked_size:?}",
    );
}

fn routed_camera(app: &mut App, entity: Entity) -> Option<Entity> {
    app.world()
        .get::<bevy::ui::UiTargetCamera>(entity)
        .map(bevy::ui::UiTargetCamera::entity)
}

fn parking_cameras(app: &mut App) -> Vec<Entity> {
    app.world_mut()
        .query_filtered::<Entity, With<jackdaw::viewport_2d::UiSceneParkingCamera>>()
        .iter(app.world())
        .collect()
}

fn window_cameras(app: &mut App) -> Vec<Entity> {
    app.world_mut()
        .query::<(Entity, &RenderTarget)>()
        .iter(app.world())
        .filter(|(_, target)| matches!(target, RenderTarget::Window(_)))
        .map(|(entity, _)| entity)
        .collect()
}

/// `UiTargetCamera` names a camera entity the editor spawned, so saved into a
/// document it would point at nothing on the next load.
#[test]
fn a_routed_ui_scene_saves_without_its_routing() {
    let mut app = util::editor_test_app();
    let parent = app
        .world_mut()
        .spawn((jackdaw::EditorEntity, Node::default()))
        .id();
    build_viewport_2d_panel(app.world_mut(), parent);

    let root = app
        .world_mut()
        .spawn((
            Name::new("Overlay"),
            UiSceneRoot::default(),
            Node {
                width: percent(100),
                height: percent(100),
                ..default()
            },
        ))
        .id();
    // Routed first, registered second: `register_entity_in_ast` is where the
    // skip policy is applied.
    app.update();
    assert!(
        app.world().get::<bevy::ui::UiTargetCamera>(root).is_some(),
        "the root is routed before the save",
    );
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), root);

    let text = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        std::path::Path::new(""),
    );
    assert!(
        text.contains("UiSceneRoot"),
        "the authored marker still saves:\n{text}",
    );
    assert!(
        !text.contains("UiTargetCamera"),
        "editor-managed routing must not serialize:\n{text}",
    );
}

/// Pan and zoom are the stage's size and position, because the camera cannot
/// move a UI scene at all: Bevy renders UI through its own origin-parked view.
#[test]
fn the_view_places_and_scales_the_stage() {
    let mut app = util::editor_test_app();
    // An absolute panel frame, so the stage's computed size below is the placement result.
    let parent = app
        .world_mut()
        .spawn((
            jackdaw::EditorEntity,
            Node {
                width: px(1000),
                height: px(500),
                ..default()
            },
        ))
        .id();
    build_viewport_2d_panel(app.world_mut(), parent);
    // Stated at zoom 1, so the placement reads as the authored numbers it names.
    hold_view(&mut app, parent);

    let (stage, area) = app
        .world()
        .get::<Viewport2dPanelHost>(parent)
        .map(|host| (host.stage, host.area))
        .expect("host on panel parent");

    let reference = UVec2::new(800, 400);
    app.world_mut().spawn((
        UiSceneRoot {
            reference_size: reference,
        },
        Node::default(),
    ));
    for _ in 0..3 {
        app.update();
    }

    let area_size = app
        .world()
        .get::<ComputedNode>(area)
        .expect("area is laid out")
        .size();

    let size = stage_size(&app, stage);
    assert_eq!(size, Vec2::new(800.0, 400.0));
    assert_eq!(
        stage_centre(&app, stage) - area_centre(&app, area),
        Vec2::ZERO,
        "the default view centres the canvas in its area",
    );
    assert_eq!(
        target_pixels_per_stage_pixel(size, reference),
        1.0,
        "at zoom 1 a stage pixel is an authored pixel",
    );

    // Zoom doubles the stage and halves the authored-pixels-per-stage-pixel
    // factor, which is why the cursor mapping needs no zoom term of its own.
    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(parent)
        .expect("host on panel parent")
        .view = Ui2dView {
        pan: Vec2::new(100.0, -50.0),
        zoom: 2.0,
        ..default()
    };
    for _ in 0..2 {
        app.update();
    }

    let size = stage_size(&app, stage);
    assert_eq!(size, Vec2::new(1600.0, 800.0));
    assert_eq!(
        target_pixels_per_stage_pixel(size, reference),
        0.5,
        "at zoom 2 one authored pixel covers two stage pixels",
    );
    assert!(
        (size.x / size.y - 2.0).abs() < 1e-3,
        "the stage keeps the reference aspect at any zoom, got {size:?}",
    );

    // The pan names the authored point at the centre of the area, y up, so
    // looking at (100, -50) slides the canvas 100 left and 50 up, times the zoom.
    assert_eq!(
        stage_centre(&app, stage) - area_centre(&app, area),
        Vec2::new(-200.0, -100.0),
        "the pan places the canvas against the area's centre",
    );
    assert_eq!(
        place_offset(&mut app, parent, stage, area, Vec2::new(0.0, 50.0)),
        Vec2::new(0.0, 100.0),
        "panning up the canvas slides it down, by the zoom",
    );
    assert!(
        size.x > area_size.x,
        "a zoomed canvas runs past its area and is clipped by it, not fitted to it",
    );
}

/// Re-pan the panel to `pan` (keeping its zoom) and report where that
/// puts the stage relative to its area.
fn place_offset(app: &mut App, panel: Entity, stage: Entity, area: Entity, pan: Vec2) -> Vec2 {
    if let Some(mut host) = app.world_mut().get_mut::<Viewport2dPanelHost>(panel) {
        host.view.pan = pan;
    }
    for _ in 0..2 {
        app.update();
    }
    stage_centre(app, stage) - area_centre(app, area)
}

fn stage_size(app: &App, stage: Entity) -> Vec2 {
    app.world()
        .get::<ComputedNode>(stage)
        .expect("stage is laid out")
        .size()
}

fn stage_centre(app: &App, stage: Entity) -> Vec2 {
    app.world()
        .get::<bevy::ui::UiGlobalTransform>(stage)
        .expect("stage is laid out")
        .translation
}

fn area_centre(app: &App, area: Entity) -> Vec2 {
    app.world()
        .get::<bevy::ui::UiGlobalTransform>(area)
        .expect("area is laid out")
        .translation
}

/// Reference resolution for the mode tests: exactly the panel's stage area,
/// at zoom 1, so an authored pixel is a window pixel.
const MODE_REFERENCE: UVec2 = UVec2::new(1200, 600);

/// Centre of the authored button the mode tests aim at.
const BUTTON_CENTRE: Vec2 = Vec2::new(500.0, 250.0);

/// Lifts the test panel over the project-select screen; see `mode_panel`.
const PANEL_ON_TOP: i32 = 1000;

/// What the authored button has heard from the pointer. `bevy_ui_widgets`
/// buttons are observers on `Pointer<Press>` and `Pointer<Click>`, not readers
/// of editor state.
#[derive(Resource, Default)]
struct WidgetEvents {
    presses: usize,
    clicks: usize,
}

/// Edit is the authoring mode: the live widget never hears the pointer.
#[test]
fn edit_mode_selects_the_authored_node_and_never_wakes_the_widget() {
    let mut app = mode_app();
    let panel = mode_panel(&mut app);
    let (_, button) = authored_button(&mut app);
    settle(&mut app);

    assert_eq!(
        mode_of(&app, panel),
        Viewport2dMode::Edit,
        "Edit is default"
    );
    press_over_authored(&mut app, panel, BUTTON_CENTRE);

    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![button],
        "a press on the stage in Edit mode selects the authored node under it",
    );
    assert_eq!(
        app.world().resource::<WidgetEvents>().presses,
        0,
        "a live widget must not react while the scene is being authored",
    );
    assert_ne!(
        app.world().get::<PickingInteraction>(button).copied(),
        Some(PickingInteraction::Pressed),
        "no pointer is over the authored tree in Edit mode",
    );
    assert_eq!(
        overlays(&mut app),
        1,
        "the selection outline is Edit chrome"
    );
}

/// Interact hands the same press to the scene, and the authoring overlay
/// gets out of the way.
#[test]
fn interact_mode_hands_the_pointer_to_the_live_widget() {
    let mut app = mode_app();
    let panel = mode_panel(&mut app);
    let (root, _) = authored_button(&mut app);
    set_mode(&mut app, panel, Viewport2dMode::Interact);
    app.world_mut().resource_mut::<Selection>().entities = vec![root];
    settle(&mut app);

    press_over_authored(&mut app, panel, BUTTON_CENTRE);

    assert_eq!(
        app.world().resource::<WidgetEvents>().presses,
        1,
        "Interact forwards the pointer into the panel's render target, \
         so the authored widget's own observer runs",
    );
    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![root],
        "a press meant for the scene must not re-select behind it",
    );
    assert_eq!(
        overlays(&mut app),
        0,
        "the outline and its handles are authoring chrome, not decoration",
    );
}

/// A stream cut off at the stage's edge leaves the widget latched down:
/// `PointerPress` never clears and no `Click` ever resolves.
#[test]
fn a_press_dragged_off_the_stage_is_released_rather_than_stranded() {
    let mut app = mode_app();
    let panel = mode_panel(&mut app);
    let (_, button) = authored_button(&mut app);
    set_mode(&mut app, panel, Viewport2dMode::Interact);
    settle(&mut app);

    let on_button = screen_position_of(&mut app, panel, BUTTON_CENTRE);
    // Well past the panel's right edge, where the stage rect test fails.
    let off_stage = on_button + Vec2::new(MODE_REFERENCE.x as f32, 0.0);

    drive_pointer(&mut app, moved(), on_button);
    drive_pointer(&mut app, pressed(), on_button);
    drive_pointer(&mut app, moved(), off_stage);
    drive_pointer(&mut app, released(), off_stage);
    settle(&mut app);

    assert!(
        !pointer_is_pressed(&app, panel),
        "the release has to reach the panel's own pointer wherever it \
         happens; a `PointerPress` left set is a scene stuck mid-gesture",
    );
    assert_ne!(
        app.world().get::<PickingInteraction>(button).copied(),
        Some(PickingInteraction::Pressed),
        "letting go outside the stage must still let go",
    );
    assert_eq!(
        app.world().resource::<WidgetEvents>().clicks,
        0,
        "a press that wandered off the widget is not a click on it",
    );

    // ... and the next real click is a click, not the tail of the last one.
    drive_pointer(&mut app, moved(), on_button);
    drive_pointer(&mut app, pressed(), on_button);
    drive_pointer(&mut app, released(), on_button);
    settle(&mut app);

    assert_eq!(
        app.world().resource::<WidgetEvents>().clicks,
        1,
        "exactly one click, from the gesture that was actually a click",
    );
}

/// The forwarder is not a rect test: a popup drawn over the stage, or a drag
/// already in flight, would otherwise leak clicks into the live scene.
#[test]
fn a_click_over_editor_chrome_never_reaches_the_scene() {
    let mut app = mode_app();
    let panel = mode_panel(&mut app);
    authored_button(&mut app);
    set_mode(&mut app, panel, Viewport2dMode::Interact);

    // A popup over the whole panel: what the editor's hit test finds.
    let popup = app
        .world_mut()
        .spawn((
            jackdaw::EditorEntity,
            GlobalZIndex(PANEL_ON_TOP + 1),
            Node {
                position_type: PositionType::Absolute,
                left: px(0),
                top: px(0),
                width: percent(100),
                height: percent(100),
                ..default()
            },
        ))
        .id();
    settle(&mut app);

    press_over_authored(&mut app, panel, BUTTON_CENTRE);
    assert_eq!(
        app.world().resource::<WidgetEvents>().presses,
        0,
        "a press that landed on editor chrome must not fall through it",
    );

    // The other half of the gate: an editor interaction already running.
    app.world_mut().entity_mut(popup).despawn();
    app.world_mut()
        .resource_mut::<jackdaw::gizmos::GizmoDragState>()
        .active = true;
    settle(&mut app);

    press_over_authored(&mut app, panel, BUTTON_CENTRE);
    assert_eq!(
        app.world().resource::<WidgetEvents>().presses,
        0,
        "a gesture the editor is already running owns the pointer",
    );
}

/// A popup or a docked panel drawn over the stage is what the user is
/// scrolling; a rect test alone cannot tell the two apart.
#[test]
fn a_panel_over_the_stage_takes_the_scroll_from_the_canvas() {
    let mut app = mode_app();
    let panel = mode_panel(&mut app);
    authored_button(&mut app);
    settle(&mut app);

    let on_stage = screen_position_of(&mut app, panel, BUTTON_CENTRE);
    let before = view_of(&app, panel).zoom;
    drive_pointer(&mut app, moved(), on_stage);
    place_cursor(&mut app, on_stage);
    scroll_up(&mut app);
    run_pan_zoom(&mut app);
    let zoomed = view_of(&app, panel).zoom;
    assert!(
        zoomed > before,
        "the wheel over the canvas zooms it: {before} -> {zoomed}",
    );

    // A popup over the whole panel: what the editor's hit test finds.
    app.world_mut().spawn((
        jackdaw::EditorEntity,
        GlobalZIndex(PANEL_ON_TOP + 1),
        Node {
            position_type: PositionType::Absolute,
            left: px(0),
            top: px(0),
            width: percent(100),
            height: percent(100),
            ..default()
        },
    ));
    settle(&mut app);

    drive_pointer(&mut app, moved(), on_stage);
    place_cursor(&mut app, on_stage);
    scroll_up(&mut app);
    run_pan_zoom(&mut app);
    assert_eq!(
        view_of(&app, panel).zoom,
        zoomed,
        "a scroll over chrome must not zoom the canvas behind it",
    );
}

/// A pan resolved by hover every frame stops dead at the panel's edge under
/// a cursor that is still moving.
#[test]
fn a_middle_drag_keeps_panning_after_the_cursor_leaves_the_panel() {
    let mut app = mode_app();
    let panel = mode_panel(&mut app);
    authored_button(&mut app);
    settle(&mut app);

    let on_stage = screen_position_of(&mut app, panel, BUTTON_CENTRE);
    drive_pointer(&mut app, moved(), on_stage);
    place_cursor(&mut app, on_stage);
    let before = view_of(&app, panel).pan;

    // The press latches the gesture onto this panel.
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Middle);
    run_pan_zoom(&mut app);

    // ... and the cursor leaves it, still holding the button down.
    let off_panel = on_stage + Vec2::new(0.0, MODE_REFERENCE.y as f32 * 2.0);
    drive_pointer(&mut app, moved(), off_panel);
    place_cursor(&mut app, off_panel);
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Middle);
    mouse_travelled(&mut app, Vec2::new(30.0, -20.0));
    run_pan_zoom(&mut app);

    let panned = view_of(&app, panel).pan;
    assert_ne!(
        panned, before,
        "the drag belongs to the panel it started on until the button is up",
    );

    // Letting go ends it cleanly: the next motion moves nothing.
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Middle);
    run_pan_zoom(&mut app);
    mouse_travelled(&mut app, Vec2::new(30.0, -20.0));
    run_pan_zoom(&mut app);
    assert_eq!(
        view_of(&app, panel).pan,
        panned,
        "a released pan is over, even with the cursor still off the panel",
    );
}

/// Put the real cursor where the pointer is: the pan/zoom pass reads the
/// window, not the picking pointer.
fn place_cursor(app: &mut App, position: Vec2) {
    let mut windows = app
        .world_mut()
        .query_filtered::<&mut Window, With<PrimaryWindow>>();
    let mut window = windows
        .single_mut(app.world_mut())
        .expect("headless apps still have a primary window");
    window.set_physical_cursor_position(Some(position.as_dvec2()));
}

fn mouse_travelled(app: &mut App, delta: Vec2) {
    app.world_mut()
        .write_message(bevy::input::mouse::MouseMotion { delta });
}

/// Run the panel's navigation pass and whatever it wrote. Frames are not
/// stepped around it: `just_pressed` lasts only until the next input pass.
fn run_pan_zoom(app: &mut App) {
    jackdaw::viewport_2d::run_2d_pan_zoom(app.world_mut());
}

/// The 3D grid stepper is a modifier-gated wheel handler with no viewport of
/// its own, so the gate is the hover authority: the panel under the cursor,
/// and what it is showing.
#[test]
fn scrolling_over_the_2d_viewport_leaves_the_world_grid_alone() {
    let mut app = mode_app();
    let panel = mode_panel(&mut app);
    authored_button(&mut app);
    settle(&mut app);

    let power = grid_power(&app);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::ShiftLeft);

    let on_stage = screen_position_of(&mut app, panel, BUTTON_CENTRE);
    hover_the_cursor(&mut app, on_stage);
    assert_eq!(
        app.world().resource::<ActiveViewport>().mode,
        Some(ViewportMode::TwoD),
        "the cursor is over a panel showing its canvas",
    );
    scroll_up(&mut app);
    step_the_world_grid(&mut app);
    assert_eq!(
        grid_power(&app),
        power,
        "the wheel over the canvas is the canvas's, not the world grid's",
    );
    drop_ignored_scroll(&mut app);

    // The same chord anywhere else still steps the world grid.
    let off_panel = on_stage + Vec2::new(0.0, MODE_REFERENCE.y as f32 * 2.0);
    hover_the_cursor(&mut app, off_panel);
    assert_eq!(
        app.world().resource::<ActiveViewport>().mode,
        None,
        "off the panel the cursor is over no viewport at all",
    );
    scroll_up(&mut app);
    step_the_world_grid(&mut app);
    assert_eq!(
        grid_power(&app),
        power + 1,
        "away from the panel the chord is the grid stepper it always was",
    );
}

/// Drop a wheel tick the handler ignored. It returns at the gate without
/// reading the stream, so the next pass would otherwise read it too.
fn drop_ignored_scroll(app: &mut App) {
    app.world_mut()
        .resource_mut::<bevy::ecs::message::Messages<bevy::input::mouse::MouseWheel>>()
        .clear();
}

/// Put the cursor somewhere and let the hover authority read it, the way the
/// schedule would if this app were in the editor state.
fn hover_the_cursor(app: &mut App, position: Vec2) {
    place_cursor(app, position);
    jackdaw::viewport::run_active_viewport_update(app.world_mut());
}

fn grid_power(app: &App) -> i32 {
    app.world()
        .resource::<jackdaw::snapping::SnapSettings>()
        .grid_power
}

fn scroll_up(app: &mut App) {
    app.world_mut()
        .write_message(bevy::input::mouse::MouseWheel {
            unit: bevy::input::mouse::MouseScrollUnit::Line,
            x: 0.0,
            y: 1.0,
            window: Entity::PLACEHOLDER,
            phase: bevy::input::touch::TouchPhase::Moved,
        });
}

/// Run the 3D grid's scroll handler. It is scheduled inside
/// `EditorInteractionSystems`, which never runs in `AppState::ProjectSelect`.
fn step_the_world_grid(app: &mut App) {
    app.world_mut()
        .run_system_cached(jackdaw::snapping::handle_grid_size_scroll)
        .expect("the grid scroll handler ran");
    settle(app);
}

/// The mode lives on the panel, so two viewports can show the same scene with
/// one being authored and the other tried out.
#[test]
fn each_panel_holds_its_own_mode() {
    let mut app = mode_app();
    let first = mode_panel(&mut app);
    let second = mode_panel(&mut app);
    settle(&mut app);

    click_segment(&mut app, first, Viewport2dMode::Interact);
    settle(&mut app);

    assert_eq!(mode_of(&app, first), Viewport2dMode::Interact);
    assert_eq!(
        mode_of(&app, second),
        Viewport2dMode::Edit,
        "the mode is per panel; a second viewport keeps authoring",
    );
    assert_eq!(
        segment_background(&mut app, first, Viewport2dMode::Interact),
        jackdaw_feathers::tokens::TOOLBAR_ACTIVE_BG,
        "the active segment is highlighted",
    );
    assert_eq!(
        segment_background(&mut app, second, Viewport2dMode::Interact),
        Color::NONE,
        "the other panel's Interact segment stays quiet",
    );

    click_segment(&mut app, second, Viewport2dMode::Interact);
    click_segment(&mut app, first, Viewport2dMode::Edit);
    settle(&mut app);

    assert_eq!(mode_of(&app, first), Viewport2dMode::Edit);
    assert_eq!(mode_of(&app, second), Viewport2dMode::Interact);
}

/// The Edit|Interact control is a radio group, and the mode the panel is in
/// carries `Checked`.
#[test]
fn the_mode_control_is_a_radio_group_and_checks_the_current_mode() {
    use bevy::ui::Checked;
    use bevy::ui_widgets::{RadioButton, RadioGroup};

    let mut app = mode_app();
    let panel = mode_panel(&mut app);
    settle(&mut app);

    for mode in [Viewport2dMode::Edit, Viewport2dMode::Interact] {
        let segment = segment_entity(&mut app, panel, mode);
        assert!(
            app.world().get::<RadioButton>(segment).is_some(),
            "a segment is a radio button",
        );
        assert!(
            app.world().get::<Interaction>(segment).is_none(),
            "and not a hand-rolled interaction control",
        );
        let bar = app
            .world()
            .get::<ChildOf>(segment)
            .expect("a segment sits in a bar")
            .parent();
        assert!(
            app.world().get::<RadioGroup>(bar).is_some(),
            "the bar the segments share is the radio group",
        );
        assert_eq!(
            app.world().get::<Checked>(segment).is_some(),
            mode == Viewport2dMode::Edit,
            "the mode the panel is in is the checked segment",
        );
    }

    click_segment(&mut app, panel, Viewport2dMode::Interact);
    settle(&mut app);
    let interact = segment_entity(&mut app, panel, Viewport2dMode::Interact);
    assert!(
        app.world().get::<Checked>(interact).is_some(),
        "and the check follows the mode",
    );
}

fn mode_app() -> App {
    let mut app = util::editor_test_app();
    app.init_resource::<WidgetEvents>();
    app
}

/// A panel whose stage area is exactly `MODE_REFERENCE`, so the canvas
/// is shown 1:1 and an authored pixel is a window pixel.
fn mode_panel(app: &mut App) -> Entity {
    let parent = app
        .world_mut()
        .spawn((
            jackdaw::EditorEntity,
            // A docked panel is the topmost thing under the cursor:
            // `AppState::ProjectSelect`'s screen would otherwise take the hover.
            GlobalZIndex(PANEL_ON_TOP),
            Node {
                position_type: PositionType::Absolute,
                left: px(0),
                top: px(0),
                width: px(MODE_REFERENCE.x as f32),
                height: px(MODE_REFERENCE.y as f32 + jackdaw_feathers::tokens::TOOLBAR_HEIGHT),
                ..default()
            },
        ))
        .id();
    build_viewport_2d_panel(app.world_mut(), parent);
    // The panel is sized 1:1, so the arriving scene must not be fitted into it.
    hold_view(app, parent);
    parent
}

/// A canvas-filling root with one button at authored (400, 200) 200x100,
/// watching for the presses a live widget would act on. Returns the root
/// and the button.
fn authored_button(app: &mut App) -> (Entity, Entity) {
    let root = app
        .world_mut()
        .spawn((
            UiSceneRoot {
                reference_size: MODE_REFERENCE,
            },
            Node {
                width: percent(100),
                height: percent(100),
                ..default()
            },
        ))
        .id();
    let button = app
        .world_mut()
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(400),
                top: px(200),
                width: px(200),
                height: px(100),
                ..default()
            },
            ChildOf(root),
        ))
        .id();
    app.world_mut()
        .entity_mut(button)
        .observe(|_: On<Pointer<Press>>, mut events: ResMut<WidgetEvents>| {
            events.presses += 1;
        })
        .observe(|_: On<Pointer<Click>>, mut events: ResMut<WidgetEvents>| {
            events.clicks += 1;
        });
    (root, button)
}

fn settle(app: &mut App) {
    for _ in 0..4 {
        app.update();
    }
}

fn mode_of(app: &App, panel: Entity) -> Viewport2dMode {
    app.world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .mode
}

fn set_mode(app: &mut App, panel: Entity, mode: Viewport2dMode) {
    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .mode = mode;
}

fn overlays(app: &mut App) -> usize {
    app.world_mut()
        .query_filtered::<Entity, With<UiSelectionOverlay>>()
        .iter(app.world())
        .count()
}

/// Move the mouse over the point on screen showing `authored`, then press it,
/// over the whole production path. The press is also delivered to the stage
/// node directly, because what the editor's pointer reaches through its own
/// window camera cannot be reproduced headless.
fn press_over_authored(app: &mut App, panel: Entity, authored: Vec2) {
    let position = screen_position_of(app, panel, authored);
    send_pointer(app, position, PointerAction::Move { delta: Vec2::ZERO });
    app.update();
    send_pointer(app, position, PointerAction::Press(PointerButton::Primary));
    app.update();

    let (stage, camera) = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .map(|host| (host.stage, host.camera))
        .expect("host on panel parent");
    let event = Press {
        button: PointerButton::Primary,
        hit: HitData::new(camera, 0.0, None, None),
        count: 1,
    };
    let target = window_target(app);
    app.world_mut().trigger(Pointer::new(
        PointerId::Mouse,
        Location { target, position },
        event,
        stage,
    ));
    settle(app);
}

/// Whether the panel's own pointer still has a button down.
fn pointer_is_pressed(app: &App, panel: Entity) -> bool {
    let pointer = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .pointer;
    app.world()
        .get::<PointerPress>(pointer)
        .is_some_and(PointerPress::is_any_pressed)
}

fn moved() -> PointerAction {
    PointerAction::Move { delta: Vec2::ZERO }
}

fn pressed() -> PointerAction {
    PointerAction::Press(PointerButton::Primary)
}

fn released() -> PointerAction {
    PointerAction::Release(PointerButton::Primary)
}

/// One real mouse input at a window position, and the frame that carries
/// it through the picking pipeline.
fn drive_pointer(app: &mut App, action: PointerAction, position: Vec2) {
    send_pointer(app, position, action);
    app.update();
}

/// Write one real mouse `PointerInput`, exactly as `bevy_picking`'s input
/// pass would.
fn send_pointer(app: &mut App, position: Vec2, action: PointerAction) {
    let target = window_target(app);
    app.world_mut().write_message(PointerInput::new(
        PointerId::Mouse,
        Location { target, position },
        action,
    ));
}

// ---------------------------------------------------------------------------
// The header's canvas-grid stepper
// ---------------------------------------------------------------------------

/// Powers of two, both ends held, and an off-ladder size pulled back onto the
/// ladder by the first press rather than doubled as it stands.
#[test]
fn the_grid_stepper_walks_the_power_of_two_ladder() {
    assert_eq!(stepped_ui_grid(8.0, 1), 16.0);
    assert_eq!(stepped_ui_grid(8.0, -1), 4.0);
    assert_eq!(
        stepped_ui_grid(MAX_UI_GRID, 1),
        MAX_UI_GRID,
        "the top holds"
    );
    assert_eq!(
        stepped_ui_grid(MIN_UI_GRID, -1),
        MIN_UI_GRID,
        "and so does the bottom",
    );
    assert_eq!(
        stepped_ui_grid(10.0, 1),
        16.0,
        "a size off the ladder lands on the ladder, not on 20",
    );
    assert_eq!(
        stepped_ui_grid(0.0, -1),
        4.0,
        "a grid of nothing is the default, stepped",
    );
}

/// The grid is per panel, and rides with the tab's view state from there.
#[test]
fn the_header_stepper_edits_its_own_panels_grid() {
    let mut app = util::editor_test_app();
    let first = fit_panel(&mut app);
    let second = fit_panel(&mut app);
    app.update();
    app.update();
    assert_eq!(grid_text(&mut app, first), "8 px");

    click_grid_step(&mut app, first, 1);
    app.update();
    assert_eq!(view_of(&app, first).grid, 16.0);
    assert_eq!(
        view_of(&app, second).grid,
        DEFAULT_UI_GRID,
        "one panel's stepper never moves another panel's canvas",
    );
    app.update();
    assert_eq!(grid_text(&mut app, first), "16 px", "the readout follows");

    click_grid_step(&mut app, first, -1);
    click_grid_step(&mut app, first, -1);
    app.update();
    assert_eq!(view_of(&app, first).grid, 4.0);
    assert!(
        app.world()
            .get::<Viewport2dPanelHost>(first)
            .expect("host on panel parent")
            .view_touched,
        "a stepped grid is a framing the panel chose, so the tab captures it",
    );
}

/// The operator says the same thing for a scripted run, and takes a size off
/// the ladder as given.
#[test]
fn the_grid_operator_sets_the_canvas_lattice() {
    let mut app = util::editor_test_app();
    let panel = fit_panel(&mut app);
    app.update();

    app.world_mut()
        .operator("viewport2d.grid")
        .param("size", 10.0)
        .call()
        .expect("dispatch")
        .assert_finished();
    assert_eq!(view_of(&app, panel).grid, 10.0);

    app.world_mut()
        .operator("viewport2d.grid")
        .param("size", 4096.0)
        .call()
        .expect("dispatch")
        .assert_finished();
    assert_eq!(
        view_of(&app, panel).grid,
        MAX_UI_GRID,
        "a size past the ladder's end is held there",
    );

    app.world_mut()
        .operator("viewport2d.grid")
        .param("size", 0.0)
        .call()
        .expect("dispatch")
        .assert_cancelled();
    assert_eq!(
        view_of(&app, panel).grid,
        MAX_UI_GRID,
        "a grid of nothing is refused rather than guessed at",
    );
}

/// An operator has no panel to be called on, so all three answer for every
/// open panel.
#[test]
fn the_per_panel_operators_reach_every_open_panel() {
    let mut app = util::editor_test_app();
    let first = fit_panel(&mut app);
    let second = fit_panel(&mut app);
    app.update();

    app.world_mut()
        .operator("viewport2d.grid")
        .param("size", 32.0)
        .call()
        .expect("dispatch")
        .assert_finished();
    assert_eq!(view_of(&app, first).grid, 32.0);
    assert_eq!(
        view_of(&app, second).grid,
        32.0,
        "the second panel is not a panel the operator forgot about",
    );

    app.world_mut()
        .operator("viewport2d.mode")
        .param("mode", "interact")
        .call()
        .expect("dispatch")
        .assert_finished();
    for panel in [first, second] {
        assert_eq!(
            app.world()
                .get::<Viewport2dPanelHost>(panel)
                .expect("host on panel parent")
                .mode,
            Viewport2dMode::Interact,
        );
    }
}

fn grid_text(app: &mut App, panel: Entity) -> String {
    let mut query = app.world_mut().query::<(&Viewport2dGridReadout, &Text)>();
    let found: Vec<String> = query
        .iter(app.world())
        .filter(|(readout, _)| readout.host == panel)
        .map(|(_, text)| text.0.clone())
        .collect();
    assert_eq!(found.len(), 1, "one grid readout per panel");
    found[0].clone()
}

/// Click one end of a panel's grid stepper, the way a user does.
fn click_grid_step(app: &mut App, panel: Entity, steps: i32) {
    let mut query = app.world_mut().query::<(Entity, &Viewport2dGridStep)>();
    let found: Vec<Entity> = query
        .iter(app.world())
        .filter(|(_, step)| step.host == panel && step.steps == steps)
        .map(|(entity, _)| entity)
        .collect();
    assert_eq!(found.len(), 1, "one stepper end per direction per panel");
    let camera = camera_of(app, panel);
    let target = window_target(app);
    app.world_mut().trigger(Pointer::new(
        PointerId::Mouse,
        Location {
            target,
            position: Vec2::ZERO,
        },
        Click {
            button: PointerButton::Primary,
            hit: HitData::new(camera, 0.0, None, None),
            duration: core::time::Duration::ZERO,
            count: 1,
        },
        found[0],
    ));
}

fn window_target(app: &mut App) -> NormalizedRenderTarget {
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(app.world())
        .expect("headless apps still have a primary window");
    RenderTarget::Window(WindowRef::Primary)
        .normalize(Some(window))
        .expect("the primary window normalizes")
}

/// Where on screen the panel is currently showing authored point
/// `authored`, in window pixels. Derived from the stage *area* and the
/// view, rather than from the numbers the production path measures.
fn screen_position_of(app: &mut App, panel: Entity, authored: Vec2) -> Vec2 {
    let (area, view, target_size) = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .map(|host| (host.area, host.view, host.target_size))
        .expect("host on panel parent");
    let computed = *app
        .world()
        .get::<ComputedNode>(area)
        .expect("the stage area is laid out");
    let centre = app
        .world()
        .get::<bevy::ui::UiGlobalTransform>(area)
        .expect("the stage area is laid out")
        .translation;

    let focus = target_size.as_vec2() / 2.0 + Vec2::new(view.pan.x, -view.pan.y);
    let logical = centre * computed.inverse_scale_factor() + (authored - focus) * view.zoom;
    logical * app.world().resource::<UiScale>().0
}

fn segment_entity(app: &mut App, panel: Entity, mode: Viewport2dMode) -> Entity {
    let mut query = app.world_mut().query::<(Entity, &Viewport2dModeSegment)>();
    let found: Vec<Entity> = query
        .iter(app.world())
        .filter(|(_, segment)| segment.host == panel && segment.mode == mode)
        .map(|(entity, _)| entity)
        .collect();
    assert_eq!(found.len(), 1, "one segment per mode per panel");
    found[0]
}

fn segment_background(app: &mut App, panel: Entity, mode: Viewport2dMode) -> Color {
    let segment = segment_entity(app, panel, mode);
    app.world()
        .get::<BackgroundColor>(segment)
        .expect("a segment paints its own highlight")
        .0
}

/// Click a header segment the way a user does: the `Pointer<Click>` its
/// inline observer is watching for.
fn click_segment(app: &mut App, panel: Entity, mode: Viewport2dMode) {
    let segment = segment_entity(app, panel, mode);
    let camera = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .camera;
    let target = window_target(app);
    app.world_mut().trigger(Pointer::new(
        PointerId::Mouse,
        Location {
            target,
            position: Vec2::ZERO,
        },
        Click {
            button: PointerButton::Primary,
            hit: HitData::new(camera, 0.0, None, None),
            duration: core::time::Duration::ZERO,
            count: 1,
        },
        segment,
    ));
}

/// The fit is pure: the largest zoom that leaves the canvas and its margin
/// inside the area, centred, and inside the usable zoom range.
#[test]
fn fitting_sizes_the_canvas_to_the_smaller_axis_of_the_area() {
    // A 16:9 canvas in a square area: width is what runs out first.
    let view = fit_view(
        Ui2dView::default(),
        UVec2::new(1600, 900),
        Vec2::splat(500.0),
    );
    assert!(view.zoom < 500.0 / 1600.0, "the fit leaves a margin");
    assert!(
        view.zoom > 500.0 / 1600.0 * 0.9,
        "and the margin is small, got {}",
        view.zoom,
    );
    assert!(
        900.0 * view.zoom < 500.0,
        "the other axis fits with room over",
    );
    assert_eq!(view.pan, Vec2::ZERO, "a fit centres the canvas");

    // The height binds instead when the area is the tall, narrow one.
    let tall = fit_view(
        Ui2dView::default(),
        UVec2::new(900, 1600),
        Vec2::new(2000.0, 400.0),
    );
    assert!(tall.zoom < 400.0 / 1600.0);

    // A canvas far larger than any area still lands inside the range, and a
    // degenerate area must not produce a zero or negative zoom.
    assert!(
        fit_view(
            Ui2dView::default(),
            UVec2::new(100_000, 100_000),
            Vec2::splat(10.0)
        )
        .zoom
            >= MIN_ZOOM
    );
    assert!(fit_view(Ui2dView::default(), UVec2::ONE, Vec2::splat(100_000.0)).zoom <= MAX_ZOOM);
    assert!(fit_view(Ui2dView::default(), UVec2::new(1600, 900), Vec2::ZERO).zoom >= MIN_ZOOM);
}

/// Shown at 100% instead, a 1920x1080 reference in a small dock leaf gives
/// the user the top-left corner of their scene and nothing else.
#[test]
fn opening_a_ui_scene_fits_the_canvas_into_the_stage_area() {
    let mut app = util::editor_test_app();
    let parent = fit_panel(&mut app);
    app.world_mut().spawn((
        UiSceneRoot {
            reference_size: UVec2::new(1920, 1080),
        },
        Node::default(),
    ));
    for _ in 0..3 {
        app.update();
    }

    let (stage, area) = stage_and_area(&app, parent);
    assert!(
        view_of(&app, parent).zoom < 1.0,
        "a new panel frames the first scene it is given, without being asked",
    );

    // ... and an explicit request lands on the same framing.
    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(parent)
        .expect("host on panel parent")
        .set_view(Ui2dView {
            pan: Vec2::new(120.0, -80.0),
            zoom: 4.0,
            ..default()
        });
    request_2d_fit(app.world_mut());
    for _ in 0..2 {
        app.update();
    }

    let stage_size = stage_size(&app, stage);
    let area_size = app
        .world()
        .get::<ComputedNode>(area)
        .expect("the stage area is laid out")
        .size();
    assert!(
        stage_size.x <= area_size.x && stage_size.y <= area_size.y,
        "the whole canvas has to be inside the area, {stage_size:?} in {area_size:?}",
    );
    assert!(
        stage_size.x > area_size.x * 0.8 || stage_size.y > area_size.y * 0.8,
        "and fill it, rather than sitting small in the middle: {stage_size:?}",
    );
    assert_eq!(
        stage_centre(&app, stage) - area_centre(&app, area),
        Vec2::ZERO,
        "a fitted canvas is centred in its area",
    );
    assert!(
        (stage_size.x / stage_size.y - 1920.0 / 1080.0).abs() < 1e-2,
        "the fit keeps the reference aspect, got {stage_size:?}",
    );
}

/// The `viewport2d.frame` operator is the Fit control. The viewport panel is
/// made the active tab first: Home is bound to this operator and to the
/// timeline's jump-to-start, and the canvas answers only while it is in front.
#[test]
fn the_frame_op_returns_a_panned_and_zoomed_panel_to_the_fit() {
    let mut app = util::editor_test_app();
    let parent = fit_panel(&mut app);
    fronted_viewport(&mut app, jackdaw::viewport_host::ViewportMode::TwoD);
    app.world_mut().spawn((
        UiSceneRoot {
            reference_size: UVec2::new(1280, 720),
        },
        Node::default(),
    ));
    request_2d_fit(app.world_mut());
    for _ in 0..3 {
        app.update();
    }
    let fitted = view_of(&app, parent);
    assert_ne!(fitted, Ui2dView::default(), "the fit moved the view");

    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(parent)
        .expect("host on panel parent")
        .view = Ui2dView {
        pan: Vec2::new(400.0, -250.0),
        zoom: 6.0,
        ..default()
    };
    for _ in 0..2 {
        app.update();
    }

    app.world_mut()
        .operator("viewport2d.frame")
        .call()
        .expect("viewport2d.frame dispatches")
        .assert_finished();
    for _ in 0..2 {
        app.update();
    }

    assert_eq!(
        view_of(&app, parent),
        fitted,
        "framing is idempotent: the op lands on the same view opening the scene did",
    );
}

/// The dock tab already says "Viewport", so the header names the scene being
/// edited and reads the zoom back instead.
#[test]
fn the_header_names_the_scene_and_reads_the_zoom_back() {
    use jackdaw::scenes::{SceneTab, Scenes};

    let mut app = util::editor_test_app();
    let parent = fit_panel(&mut app);
    app.update();

    assert_eq!(
        title_text(&mut app),
        "Viewport",
        "with no UI scene open the header falls back to the panel's name",
    );
    assert_eq!(zoom_text(&mut app, parent), "100%");

    {
        let mut scenes = app.world_mut().resource_mut::<Scenes>();
        scenes.tabs.clear();
        let mut tab = SceneTab::new_untitled(1);
        tab.display_name = "main-menu".to_string();
        scenes.tabs.push(tab);
        scenes.active = 0;
    }
    app.world_mut().spawn((
        UiSceneRoot {
            reference_size: UVec2::new(1280, 720),
        },
        Node::default(),
    ));
    // A framing of the panel's own, so the readout reports a zoom nobody is
    // about to fit away.
    hold_view(&mut app, parent);
    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(parent)
        .expect("host on panel parent")
        .set_view(Ui2dView {
            pan: Vec2::ZERO,
            zoom: 2.5,
            ..default()
        });
    for _ in 0..2 {
        app.update();
    }

    assert_eq!(title_text(&mut app), "main-menu");
    assert_eq!(
        zoom_text(&mut app, parent),
        "250%",
        "zoom 1.0 is 100%, whatever the fit did to it",
    );
}

/// The panel's two captures: the whole editor window, and the panel's own
/// render target. A headless app has no render device, so `ScreenshotCaptured`
/// is triggered directly in place of the frame the renderer would hand back.
#[test]
fn the_screenshot_ops_aim_at_the_window_and_the_panel_and_write_pngs() {
    let mut app = util::editor_test_app();
    let parent = fit_panel(&mut app);
    app.update();

    let dir = std::env::temp_dir().join(format!("jackdaw-shot-ops-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let window_png = dir.join("window.png");
    let panel_png = dir.join("nested/panel.png");

    for (id, path) in [
        ("window.screenshot", &window_png),
        ("viewport2d.screenshot", &panel_png),
    ] {
        app.world_mut()
            .operator(id)
            .param("path", path.to_string_lossy().to_string())
            .call()
            .unwrap_or_else(|err| panic!("{id} dispatches: {err}"))
            .assert_finished();
    }
    app.update();

    let panel_image = match app
        .world()
        .get::<RenderTarget>(camera_of(&app, parent))
        .expect("the panel camera renders into an image")
    {
        RenderTarget::Image(target) => target.handle.clone(),
        other => panic!("the 2D viewport camera must render into an image, got {other:?}"),
    };
    let mut aimed: Vec<(Entity, RenderTarget)> = app
        .world_mut()
        .query::<(Entity, &Screenshot)>()
        .iter(app.world())
        .map(|(entity, shot)| (entity, shot.0.clone()))
        .collect();
    assert_eq!(aimed.len(), 2, "one queued capture per operator");
    assert!(
        aimed
            .iter()
            .any(|(_, target)| matches!(target, RenderTarget::Window(WindowRef::Primary))),
        "window.screenshot captures the primary window, not a camera's target",
    );
    assert!(
        aimed.iter().any(
            |(_, target)| matches!(target, RenderTarget::Image(image) if image.handle == panel_image)
        ),
        "viewport2d.screenshot captures exactly the image the panel's camera draws into",
    );

    for (entity, _) in aimed.drain(..) {
        app.world_mut().trigger(ScreenshotCaptured {
            entity,
            image: captured_frame(),
        });
    }

    for path in [&window_png, &panel_png] {
        let bytes = std::fs::read(path)
            .unwrap_or_else(|err| panic!("no capture at {}: {err}", path.display()));
        assert!(!bytes.is_empty(), "{} is empty", path.display());
        assert_eq!(
            &bytes[..8],
            b"\x89PNG\r\n\x1a\n",
            "{} is not a PNG",
            path.display(),
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Session restore opens its scenes before it has a workspace to show them
/// in, so the focus a scene asks for is held until there is a leaf to honour
/// it on, and a new panel starts with a fit pending.
#[test]
fn a_ui_scene_opened_before_the_workspace_exists_is_still_fronted_and_framed() {
    use jackdaw::viewport_host::{PendingViewportFocus, focus_viewport};

    let mut app = util::editor_test_app();
    // The dock as restore finds it: no 2D viewport leaf built yet.
    dock_leaf(&mut app, &["jackdaw.outliner"]);

    app.world_mut().spawn((
        UiSceneRoot {
            reference_size: UVec2::new(1920, 1080),
        },
        Node::default(),
    ));
    focus_viewport(app.world_mut(), ViewportMode::TwoD);
    request_2d_fit(app.world_mut());
    app.update();

    assert!(
        app.world().get_resource::<PendingViewportFocus>().is_some(),
        "a focus the dock cannot honour yet is held, not dropped",
    );

    // Then the workspace materialises, with the panel the reconciler builds.
    let leaf = dock_leaf(&mut app, &["jackdaw.outliner", VIEWPORT_WINDOW_ID]);
    let panel = fit_panel(&mut app);
    for _ in 0..3 {
        app.update();
    }

    assert_eq!(
        active_window(&app, leaf).as_deref(),
        Some(VIEWPORT_WINDOW_ID),
        "the tab the restored scene asked for has to come forward when it can",
    );
    assert_eq!(
        host_mode(&app, panel),
        ViewportMode::TwoD,
        "and it comes forward showing the canvas the scene asked for, not \
         whatever mode the panel was built in",
    );
    assert!(
        app.world().get_resource::<PendingViewportFocus>().is_none(),
        "and the request is spent once it has been honoured",
    );

    let view = view_of(&app, panel);
    assert!(
        view.zoom < 1.0,
        "a 1920x1080 reference has to shrink into a 600x400 panel, got {}",
        view.zoom,
    );
    let (stage, area) = stage_and_area(&app, panel);
    let stage_size = stage_size(&app, stage);
    let area_size = app
        .world()
        .get::<ComputedNode>(area)
        .expect("the stage area is laid out")
        .size();
    assert!(
        stage_size.x <= area_size.x && stage_size.y <= area_size.y,
        "the restored scene is framed, not cropped: {stage_size:?} in {area_size:?}",
    );
}

/// Restore opens every persisted tab in turn, so a UI-scene tab that is not
/// the last active one still asks for a focus while the dock is empty.
#[test]
fn a_focus_asked_for_by_a_tab_the_user_left_does_not_front_the_panel() {
    use jackdaw::scenes::swap::swap_active_tab;
    use jackdaw::scenes::{SceneTab, Scenes, TabContent};
    use jackdaw::viewport_host::PendingViewportFocus;

    let mut app = util::editor_test_app();
    // The dock as restore finds it: no 2D viewport leaf built yet.
    dock_leaf(&mut app, &["jackdaw.outliner"]);

    {
        let mut scenes = app.world_mut().resource_mut::<Scenes>();
        scenes.tabs.clear();
        scenes.tabs.push(SceneTab::new_untitled(1));
        scenes.tabs.push(SceneTab::new_untitled(2));
        // Tab 0 is the UI scene restore opens first; tab 1 is the
        // ordinary scene it opens second and leaves in front.
        scenes.tabs[0].content = TabContent::Scene(Some(Box::new(
            jackdaw_bsn::parse_bsn_text("#Overlay\njackdaw_scene_types::UiSceneRoot\n")
                .expect("the fixture parses"),
        )));
        scenes.tabs[1].content = TabContent::Scene(Some(Box::new(
            jackdaw_bsn::parse_bsn_text(
                "#World\nbevy_transform::components::transform::Transform\n",
            )
            .expect("the fixture parses"),
        )));
        scenes.active = 1;
    }

    swap_active_tab(app.world_mut(), 0);
    assert!(
        app.world().get_resource::<PendingViewportFocus>().is_some(),
        "the UI-scene tab asks for a focus the empty dock cannot honour",
    );

    swap_active_tab(app.world_mut(), 1);
    assert!(
        app.world().get_resource::<PendingViewportFocus>().is_none(),
        "activating a tab that is not a UI scene settles the debt: nothing \
         is owed to a tab the user has left",
    );

    // The workspace materialises afterwards, as it does on restore.
    let leaf = dock_leaf(&mut app, &["jackdaw.outliner", VIEWPORT_WINDOW_ID]);
    fit_panel(&mut app);
    for _ in 0..3 {
        app.update();
    }

    assert_eq!(
        active_window(&app, leaf).as_deref(),
        Some("jackdaw.outliner"),
        "the panel must not come forward over the scene the user restored",
    );
    assert_eq!(
        app.world().resource::<Scenes>().active,
        1,
        "and the tab the user left in front is still the active one",
    );
}

/// Both are gestures a headless run cannot perform: the mode is a click on a
/// header segment, and a stage selection is a click at an authored pixel.
#[test]
fn the_harness_ops_flip_the_mode_and_select_by_name() {
    let mut app = op_app();
    let panel = fit_panel(&mut app);
    app.update();

    for (name, mode) in [
        ("interact", Viewport2dMode::Interact),
        ("edit", Viewport2dMode::Edit),
    ] {
        app.world_mut()
            .operator("viewport2d.mode")
            .param("mode", name)
            .call()
            .expect("viewport2d.mode dispatches")
            .assert_finished();
        assert_eq!(mode_of(&app, panel), mode, "'{name}' sets the panel's mode");
    }

    app.world_mut()
        .operator("viewport2d.mode")
        .param("mode", "sideways")
        .call()
        .expect("viewport2d.mode dispatches")
        .assert_cancelled();
    assert_eq!(
        mode_of(&app, panel),
        Viewport2dMode::Edit,
        "a mode it cannot read leaves the panel where it was",
    );
}

/// A capture of the wrong node is worse than a capture that did not happen.
#[test]
fn selecting_by_name_needs_exactly_one_match() {
    let mut app = op_app();
    fit_panel(&mut app);
    let root = app
        .world_mut()
        .spawn((
            Name::new("Menu"),
            UiSceneRoot {
                reference_size: UVec2::new(1280, 720),
            },
            Node::default(),
        ))
        .id();
    let play = app
        .world_mut()
        .spawn((Name::new("Play"), Node::default(), ChildOf(root)))
        .id();
    app.update();

    app.world_mut()
        .operator("selection.select")
        .param("name", "Play")
        .call()
        .expect("selection.select dispatches")
        .assert_finished();
    assert_eq!(app.world().resource::<Selection>().entities, vec![play]);

    app.world_mut()
        .operator("selection.select")
        .param("name", "Quit")
        .call()
        .expect("selection.select dispatches")
        .assert_cancelled();

    // A second `Play` makes the name ambiguous.
    app.world_mut()
        .spawn((Name::new("Play"), Node::default(), ChildOf(root)));
    app.update();
    app.world_mut()
        .operator("selection.select")
        .param("name", "Play")
        .call()
        .expect("selection.select dispatches")
        .assert_cancelled();
    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![play],
        "a refused selection leaves the one the user had",
    );
}

/// An editor app with the viewport panel's extension on. The startup resolver
/// only enables what the developer's on-disk extensions.json lists, so an op
/// test that skips this passes or fails by the machine it runs on.
fn op_app() -> App {
    let mut app = util::editor_test_app();
    jackdaw_api_internal::lifecycle::enable_extension(
        app.world_mut(),
        &jackdaw::builtin_extensions::ViewportExtension.id(),
    );
    app.update();
    app
}

/// A panel big enough to fit a canvas inside, and small enough that a
/// 1920x1080 reference has to shrink to get there.
fn fit_panel(app: &mut App) -> Entity {
    let parent = app
        .world_mut()
        .spawn((
            jackdaw::EditorEntity,
            Node {
                width: px(600),
                height: px(400),
                ..default()
            },
        ))
        .id();
    build_viewport_2d_panel(app.world_mut(), parent);
    parent
}

/// The dock a real layout builds when a viewport panel is in front, showing
/// `mode`. The editor has one viewport window with two modes: the tab says a
/// viewport is fronted, and a `ViewportHost` says which of the two it shows.
fn fronted_viewport(app: &mut App, mode: jackdaw::viewport_host::ViewportMode) {
    let leaf = dock_leaf(app, &[jackdaw::viewport::VIEWPORT_WINDOW_ID]);
    let tab = tab_ids(app, leaf)[0];
    spawn_viewport_panel(app, tab, mode);
    app.update();
}

/// The tab ids `leaf` holds, in the order it holds them.
fn tab_ids(app: &App, leaf: jackdaw_panels::tree::NodeId) -> Vec<jackdaw_panels::tree::TabId> {
    app.world()
        .resource::<jackdaw_panels::tree::DockTree>()
        .get(leaf)
        .and_then(|node| node.as_leaf())
        .expect("a leaf")
        .windows
        .iter()
        .map(|tab| tab.id)
        .collect()
}

/// A viewport panel's content entity, bound to the dock tab it is drawn
/// in. The binding is what says which panel a fronted tab is.
fn spawn_viewport_panel(
    app: &mut App,
    tab: jackdaw_panels::tree::TabId,
    mode: jackdaw::viewport_host::ViewportMode,
) -> Entity {
    use jackdaw::viewport_host::ViewportHost;
    let three_d = app.world_mut().spawn_empty().id();
    let two_d = app.world_mut().spawn_empty().id();
    app.world_mut()
        .spawn((
            ViewportHost {
                mode,
                mode_chosen: true,
                three_d,
                two_d,
            },
            jackdaw_panels::area::DockTabContent {
                window_id: jackdaw::viewport::VIEWPORT_WINDOW_ID.to_string(),
                tab_id: tab,
            },
        ))
        .id()
}

/// Home belongs to the panel in front, not to any panel in the workspace.
#[test]
fn a_buried_canvas_does_not_answer_home_for_the_panel_in_front() {
    use jackdaw::viewport_host::ViewportMode;

    let mut app = util::editor_test_app();
    world_camera(&mut app);
    let leaf = dock_leaf(
        app_mut(&mut app),
        &[
            jackdaw::viewport::VIEWPORT_WINDOW_ID,
            jackdaw::viewport::VIEWPORT_WINDOW_ID,
        ],
    );
    let tabs = tab_ids(&app, leaf);
    spawn_viewport_panel(&mut app, tabs[0], ViewportMode::TwoD);
    spawn_viewport_panel(&mut app, tabs[1], ViewportMode::ThreeD);
    // The second tab is the one in front.
    {
        let mut tree = app
            .world_mut()
            .resource_mut::<jackdaw_panels::tree::DockTree>();
        let leaf = tree.get_mut(leaf).and_then(|node| node.as_leaf_mut());
        leaf.expect("a leaf").active = Some(tabs[1]);
    }
    app.update();

    assert!(
        !available(&mut app, "viewport2d.frame"),
        "the canvas is behind the panel in front, so it does not take Home",
    );
    assert!(
        available(&mut app, "view.frame_all"),
        "the panel in front does",
    );
}

/// A workspace can front a viewport in each of two leaves. Asking each mode
/// separately whether a fronted viewport is showing it answered yes twice.
#[test]
fn two_fronted_viewports_in_different_modes_do_not_both_answer_home() {
    use jackdaw::viewport_host::ViewportMode;
    use jackdaw_panels::tree::{DockTree, Edge};

    let mut app = util::editor_test_app();
    fit_panel(&mut app);
    world_camera(&mut app);
    let left = dock_leaf(app_mut(&mut app), &[jackdaw::viewport::VIEWPORT_WINDOW_ID]);
    let (right, right_tab) = app
        .world_mut()
        .resource_mut::<DockTree>()
        .split(
            left,
            Edge::Right,
            jackdaw::viewport::VIEWPORT_WINDOW_ID.to_string(),
        )
        .expect("the leaf splits");
    let left_tab = tab_ids(&app, left)[0];
    spawn_viewport_panel(&mut app, left_tab, ViewportMode::TwoD);
    spawn_viewport_panel(&mut app, right_tab, ViewportMode::ThreeD);
    assert_eq!(
        tab_ids(&app, right),
        vec![right_tab],
        "precondition: the split put the second viewport in its own leaf",
    );
    app.update();

    assert!(
        !available(&mut app, "viewport2d.frame"),
        "with two panels fronted and the cursor over neither, the canvas names no press",
    );
    assert!(
        !available(&mut app, "view.frame_all"),
        "and neither does the world, so one press does not do both",
    );
}

/// `dock_leaf` wants the app; naming the reborrow keeps the call above
/// readable.
fn app_mut(app: &mut App) -> &mut App {
    app
}

/// A dock whose one leaf holds `windows`, in that order, so a focus
/// change is visible as a change rather than as the starting state.
fn dock_leaf(app: &mut App, windows: &[&str]) -> jackdaw_panels::tree::NodeId {
    use jackdaw_panels::{
        area::DockAreaStyle,
        tree::{DockLeaf, DockTree},
    };

    app.init_resource::<DockTree>();
    let mut tree = app.world_mut().resource_mut::<DockTree>();
    tree.set_root_leaf(
        DockLeaf::new("center", DockAreaStyle::default())
            .with_windows(windows.iter().copied().map(String::from).collect()),
    )
}

/// The window whose tab is in front of `leaf`.
fn active_window(app: &App, leaf: jackdaw_panels::tree::NodeId) -> Option<String> {
    let tree = app.world().resource::<jackdaw_panels::tree::DockTree>();
    let leaf = tree.get(leaf)?.as_leaf()?;
    let active = leaf.active?;
    leaf.tabs()
        .find_map(|(window, tab)| (tab == active).then(|| window.to_string()))
}

/// What the renderer would have handed back: a small solid frame, in a
/// format `Image::try_into_dynamic` accepts.
fn captured_frame() -> Image {
    Image::new_fill(
        UVec2::splat(4).to_extents(),
        TextureDimension::D2,
        &[255, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        default(),
    )
}

/// Withdraw the fit a new panel starts with, as a restored framing does, so
/// the tests below get the view they set.
fn hold_view(app: &mut App, panel: Entity) {
    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .fit_pending = false;
}

fn view_of(app: &App, panel: Entity) -> Ui2dView {
    app.world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .view
}

fn host_mode(app: &App, panel: Entity) -> ViewportMode {
    app.world()
        .get::<ViewportHost>(panel)
        .expect("host on panel parent")
        .mode
}

fn camera_of(app: &App, panel: Entity) -> Entity {
    app.world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .camera
}

fn stage_and_area(app: &App, panel: Entity) -> (Entity, Entity) {
    app.world()
        .get::<Viewport2dPanelHost>(panel)
        .map(|host| (host.stage, host.area))
        .expect("host on panel parent")
}

fn title_text(app: &mut App) -> String {
    let mut query = app
        .world_mut()
        .query_filtered::<&Text, With<Viewport2dTitle>>();
    let found: Vec<String> = query.iter(app.world()).map(|text| text.0.clone()).collect();
    assert_eq!(found.len(), 1, "one title per panel");
    found[0].clone()
}

fn zoom_text(app: &mut App, panel: Entity) -> String {
    let mut query = app.world_mut().query::<(&Viewport2dZoomReadout, &Text)>();
    let found: Vec<String> = query
        .iter(app.world())
        .filter(|(readout, _)| readout.host == panel)
        .map(|(_, text)| text.0.clone())
        .collect();
    assert_eq!(found.len(), 1, "one zoom readout per panel");
    found[0].clone()
}

/// A view nobody moved is not a framing the tab chose. Capturing it as one
/// leaves every later activation withdrawing the fit it just asked for, so
/// `ViewState::ui_view` stays `None` until something moves the view.
#[test]
fn a_ui_tab_that_never_got_framed_is_framed_when_a_panel_arrives() {
    use jackdaw::scenes::swap::swap_active_tab;
    use jackdaw::scenes::{SceneTab, Scenes, TabContent};

    let mut app = util::editor_test_app();
    {
        let mut scenes = app.world_mut().resource_mut::<Scenes>();
        scenes.tabs.clear();
        scenes.tabs.push(SceneTab::new_untitled(1));
        scenes.tabs.push(SceneTab::new_untitled(2));
        scenes.active = 0;
        // Tab 1 is the UI scene, carried as a document so activating it takes
        // the real spawn-and-frame path.
        scenes.tabs[1].content = TabContent::Scene(Some(Box::new(
            jackdaw_bsn::parse_bsn_text("#Overlay\njackdaw_scene_types::UiSceneRoot\n")
                .expect("the fixture parses"),
        )));
    }

    // Opened with no 2D panel docked: the fit this activation asks for is lost.
    swap_active_tab(app.world_mut(), 1);
    app.update();
    assert_eq!(ui_scene_roots(&mut app), 1, "the UI scene spawned");

    // The panel arrives afterwards, and frames what it finds.
    let panel = fit_panel(&mut app);
    for _ in 0..2 {
        app.update();
    }
    assert_ne!(
        view_of(&app, panel),
        Ui2dView::default(),
        "a panel docked after the scene opened frames it on arrival",
    );

    // Away, and back: the swap that would stamp the default.
    swap_active_tab(app.world_mut(), 0);
    swap_active_tab(app.world_mut(), 1);
    for _ in 0..3 {
        app.update();
    }

    let view = view_of(&app, panel);
    assert_ne!(
        view,
        Ui2dView::default(),
        "the activation's fit must survive the restore: a stamped default \
         withdraws it, and then no activation can ever frame this tab",
    );
    assert!(
        view.zoom < 1.0,
        "a 1280x720 reference has to shrink to fit a 600x400 panel, got {}",
        view.zoom,
    );

    // ... and a framing that *was* chosen still travels with its tab.
    let chosen = Ui2dView {
        pan: Vec2::new(9.0, -4.0),
        zoom: 3.0,
        ..default()
    };
    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .set_view(chosen);
    swap_active_tab(app.world_mut(), 0);
    swap_active_tab(app.world_mut(), 1);
    for _ in 0..2 {
        app.update();
    }
    assert_eq!(
        view_of(&app, panel),
        chosen,
        "a framing the user chose outranks the activation's fit",
    );
}

fn ui_scene_roots(app: &mut App) -> usize {
    app.world_mut()
        .query_filtered::<Entity, (With<UiSceneRoot>, Without<ChildOf>)>()
        .iter(app.world())
        .count()
}

/// The op's selection is a real selection: the inspector's card list and the
/// outliner's row highlight are both `On<Add, Selected>` observers, so a
/// scripted selection lights the editor up as a click does.
#[test]
fn selecting_by_name_builds_the_inspector_for_the_selected_node() {
    let mut app = op_app();
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    let root = app
        .world_mut()
        .spawn((
            Name::new("Menu"),
            UiSceneRoot {
                reference_size: UVec2::new(1280, 720),
            },
            Node::default(),
        ))
        .id();
    app.world_mut()
        .spawn((Name::new("Play"), Node::default(), ChildOf(root)));
    app.update();

    assert_eq!(
        component_card_labels(&mut app).len(),
        0,
        "nothing is selected yet, so the inspector has no cards to show",
    );

    app.world_mut()
        .operator("selection.select")
        .param("name", "Play")
        .call()
        .expect("selection.select dispatches")
        .assert_finished();
    app.update();

    let labels = component_card_labels(&mut app);
    assert!(
        labels.iter().any(|label| label == "Node"),
        "the selected UI node's own `Node` has to have a card: {labels:?}",
    );
}

/// Every text drawn inside an inspector component card.
fn component_card_labels(app: &mut App) -> Vec<String> {
    use jackdaw::inspector::ComponentDisplay;

    let cards: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<ComponentDisplay>>()
        .iter(app.world())
        .collect();
    let mut labels = Vec::new();
    for card in cards {
        let mut stack = vec![card];
        while let Some(entity) = stack.pop() {
            if let Some(text) = app.world().get::<Text>(entity) {
                labels.push(text.0.clone());
            }
            if let Some(children) = app.world().get::<Children>(entity) {
                stack.extend(children.iter());
            }
        }
    }
    labels
}

/// The gate asks the dock tree for an id a layout actually registers, not
/// `jackdaw.viewport_2d`, which `canonical_window_id` maps away.
#[test]
fn the_canvas_answers_home_while_its_panel_is_in_front() {
    use jackdaw::viewport_host::ViewportMode;

    let mut app = util::editor_test_app();
    fit_panel(&mut app);
    world_camera(&mut app);
    fronted_viewport(&mut app, ViewportMode::TwoD);

    assert!(
        available(&mut app, "viewport2d.frame"),
        "the canvas takes Home while its panel is fronted in 2D"
    );
    assert!(
        !available(&mut app, "view.frame_all"),
        "and the world framing stands down, so one press does one thing"
    );
}

/// A camera for `view.frame_all` to frame the world in, so the operator's
/// availability turns on the panel rather than on there being no camera.
fn world_camera(app: &mut App) {
    app.world_mut().spawn((
        jackdaw::viewport::MainViewportCamera,
        Camera3d::default(),
        Transform::default(),
    ));
    app.update();
}

/// The other half: with the same panel in 3D, Home frames the world and the
/// canvas stands down.
#[test]
fn the_world_answers_home_while_the_panel_is_in_three_d() {
    use jackdaw::viewport_host::ViewportMode;

    let mut app = util::editor_test_app();
    world_camera(&mut app);
    fronted_viewport(&mut app, ViewportMode::ThreeD);

    assert!(
        !available(&mut app, "viewport2d.frame"),
        "the canvas does not take Home from a panel showing the world"
    );
    assert!(
        available(&mut app, "view.frame_all"),
        "the world framing takes Home instead"
    );
}

fn available(app: &mut App, id: &'static str) -> bool {
    app.world_mut()
        .operator(id)
        .is_available()
        .unwrap_or_else(|err| panic!("{id}: is_available errored: {err}"))
}

/// The chords that describe something in a world with three axes. Every one
/// is a bare letter or digit, so over the canvas they must all stand down.
const WORLD_CHORDS: [&str; 7] = [
    // KeyM
    "tools.measure_distance",
    // KeyL
    "gizmo.space.toggle",
    // KeyF
    "viewport.focus_selected",
    // Digit1 .. Digit4 and KeyK
    "edit_mode.vertex",
    "edit_mode.edge",
    "edit_mode.face",
    "edit_mode.knife",
];

/// With the canvas in front, none of the world's bare chords answer.
#[test]
fn the_worlds_bare_chords_stand_down_over_the_canvas() {
    let mut app = util::editor_test_app();
    fit_panel(&mut app);
    world_camera(&mut app);
    framing_selection(&mut app);
    fronted_viewport(&mut app, ViewportMode::TwoD);

    for id in WORLD_CHORDS {
        assert!(
            !available(&mut app, id),
            "{id} answers a bare key the canvas needs for typing",
        );
    }
}

/// And with the world in front they all answer, so the gate is the panel
/// rather than a chord that stopped working.
#[test]
fn the_worlds_bare_chords_answer_over_the_three_d_viewport() {
    let mut app = util::editor_test_app();
    world_camera(&mut app);
    framing_selection(&mut app);
    fronted_viewport(&mut app, ViewportMode::ThreeD);

    for id in WORLD_CHORDS {
        assert!(available(&mut app, id), "{id} still answers in the world");
    }
}

/// Something selected, so `viewport.focus_selected` has a thing to frame. The
/// focus is cleared with it, the way a press in a viewport does: Bevy seeds
/// the focus with the window, which the edit-mode gate reads as a field being
/// typed into.
fn framing_selection(app: &mut App) {
    let entity = app.world_mut().spawn(Transform::default()).id();
    app.world_mut().resource_mut::<Selection>().entities = vec![entity];
    app.world_mut()
        .resource_mut::<bevy::input_focus::InputFocus>()
        .clear();
    app.update();
}

/// Home frames the canvas when it is pressed, not only when the operator is
/// called by hand: a chord bound to an action nothing evaluates is dead.
#[test]
fn home_frames_the_canvas_through_the_keyboard() {
    let mut app = util::editor_test_app();
    let panel = fit_panel(&mut app);
    app.world_mut().spawn((
        UiSceneRoot {
            reference_size: UVec2::new(1920, 1080),
        },
        Node::default(),
    ));
    fronted_viewport(&mut app, ViewportMode::TwoD);
    settle(&mut app);

    // A view nothing would leave it at, so a fit is visible in the numbers.
    {
        let mut host = app
            .world_mut()
            .get_mut::<Viewport2dPanelHost>(panel)
            .expect("host on panel parent");
        host.view.zoom = 0.05;
        host.fit_pending = false;
    }
    settle(&mut app);

    press(&mut app, "input.key key=Home");

    let zoom = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .view
        .zoom;
    assert!(
        (zoom - 0.05).abs() > f32::EPSILON,
        "Home framed the canvas: the view is no longer where it was left ({zoom})",
    );
}

/// Press a key the way the window does, and let the beats play out.
fn press(app: &mut App, clause: &str) {
    jackdaw::boot_ops::run_op_clause(app.world_mut(), clause)
        .expect("the clause dispatches")
        .assert_finished();
    for _ in 0..600 {
        app.update();
        if app
            .world()
            .resource::<jackdaw::test_input::SyntheticInput>()
            .is_idle()
        {
            break;
        }
    }
    settle(app);
}
