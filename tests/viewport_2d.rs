//! 2D viewport panel and navigation coverage.
//!
//! `build_viewport_2d_panel` is what the dock-tree reconciler calls when
//! it instantiates a `jackdaw.viewport_2d` leaf. These tests call it
//! directly against a headless editor app (the reconciler itself only
//! runs after entering `AppState::Editor`) and pin these contracts:
//!
//! 1. The panel wires a `Camera2d` rendering into its own render-target
//!    image, and records the camera plus the stage node it projects into
//!    on a `Viewport2dPanelHost` sitting on the panel content entity.
//! 2. Tearing the panel down despawns that camera, so a closed or
//!    re-split panel doesn't leak a camera and its image.
//! 3. The pan/zoom math is pure: zooming keeps the authored point under
//!    the cursor fixed, panning keeps it under the moving cursor, and
//!    zoom stays inside its usable range. No camera involved.
//! 4. The cursor reaches that math in the stage area's logical pixels,
//!    and reaches the *canvas* in render-target pixels, so both hold at
//!    any UI scale factor and at any reference resolution.
//! 5. That view is per-scene-tab state, and the stage is placed from it:
//!    pan and zoom are the stage node's position and size, because a
//!    camera cannot move a UI scene at all.
//! 6. An authored UI scene is routed into the panel and laid out at its
//!    own reference resolution, and the routing is editor state that
//!    neither outlives the panel nor reaches disk.
//! 7. The header's Edit|Interact control decides what a pointer over the
//!    stage means, per panel: Edit authors the scene and the live widgets
//!    never see the pointer, Interact hands it to them and the authoring
//!    chrome stands down.
//! 8. Opening a UI scene frames it: the canvas is fitted to the stage
//!    area rather than shown at 100% and cropped, and `viewport2d.frame`
//!    puts a panned and zoomed panel back there. The header names the
//!    scene being edited and reads the zoom back.
//! 9. The two screenshot operators aim at their own surfaces and write
//!    real PNGs.

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
    viewport::ViewportPanelHost,
    viewport_2d::{
        DEFAULT_UI_GRID, MAX_UI_GRID, MAX_ZOOM, MIN_UI_GRID, MIN_ZOOM, Scene2dViewport, Ui2dView,
        Viewport2dCamera, Viewport2dGridReadout, Viewport2dGridStep, Viewport2dMode,
        Viewport2dModeSegment, Viewport2dPanelHost, Viewport2dTitle, Viewport2dZoomReadout,
        build_viewport_2d_panel, cursor_area_offset, cursor_stage_offset, fit_view, pan_by,
        request_2d_fit, stepped_ui_grid, target_pixels_per_stage_pixel, world_at, zoom_toward,
    },
    viewport_host::{ViewportHost, ViewportMode},
};

mod util;
use util::OperatorResultExt as _;

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

    // The panel is one thing shown two ways: the 2D presentation's state sits
    // beside the panel's own identity, which opens on the canvas here and lets
    // only that camera render.
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
    // The stage shows the camera's image with an `ImageNode`, not a
    // `ViewportNode`: the stage node carries the view's zoom, and Bevy
    // resizes a `ViewportNode`'s render target to its node's size, which
    // would reallocate the image to `reference * zoom` mid-gesture.
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

    // The canvas edge is the one line saying where the authored scene
    // stops, drawn over a dark letterbox and often over a dark scene, so
    // it is the strong border rather than a separator.
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

    // Whatever was under the cursor when the drag started is under the
    // cursor where the drag ended.
    let start = Vec2::new(5.0, -8.0);
    assert!((world_at(view, start) - world_at(after, start + drag)).length() < 1e-3);
}

/// The unit contract between the cursor and the canvas, half of it: an
/// image target renders at scale factor 1, so the authored tree measures
/// in the image's own pixels. A cursor offset left in ui-logical pixels
/// would be off by a factor of `scale_factor`, and every click would land
/// off the node the user aimed at on any high-DPI display or after
/// `view.ui_zoom_in`.
#[test]
fn the_cursor_lifts_into_render_target_pixels_before_it_reaches_the_canvas() {
    // A stage 400x200 physical px centred at (200, 100) physical. At a UI
    // scale factor of 2 that is 200x100 logical px: logical x runs 0..200,
    // logical y runs 0..100. The image is the stage's own size here, so
    // the second factor is 1 and only the scale factor is in play.
    let inverse_scale = 0.5;
    let centre = Vec2::new(200.0, 100.0);
    let size = Vec2::new(400.0, 200.0);

    // 50 logical px right of the centre is 100 render-target px right.
    let offset = cursor_stage_offset(Vec2::new(150.0, 50.0), centre, size, inverse_scale, 1.0)
        .expect("the cursor is inside the stage");
    assert_eq!(offset, Vec2::new(100.0, 0.0));

    // The hit test still bounds the cursor by the node's logical rect,
    // exactly as `update_active_viewport` does.
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

/// The other half: the panel's image is held at the authored reference
/// size, not the stage's size, so a stage measurement is one factor short
/// of the authored pixels a click has to name. The two factors compose in
/// `cursor_stage_offset` and nowhere else, and the bounds test stays in
/// stage pixels because it is a question about the node.
///
/// This second factor is also where the view's zoom lives, because
/// `place_stage` sizes the stage node to `reference * zoom`: a 2x zoom
/// halves this scale, and every consumer of it follows without being
/// told.
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

    // A degenerate stage must not hand the view an infinity: this runs on
    // every cursor move, including the frame a panel is first laid out.
    assert_eq!(target_pixels_per_stage_pixel(Vec2::ZERO, target), 1.0);
    // With no UI scene open the image is the stage's own size, so the
    // factor drops out.
    assert_eq!(
        target_pixels_per_stage_pixel(stage, stage.as_uvec2()),
        1.0,
        "an unscaled panel is still 1:1",
    );
}

/// The scale-factor conversion on the pan path. `MouseMotion` reports
/// stage *physical* pixels and the view is driven in *logical* ones, so a
/// middle-drag that crosses N physical pixels crosses `N * inverse_scale`
/// logical ones. Dropping that factor leaves panning wrong by exactly the
/// display's scale factor: twice as fast on a `HiDPI` screen.
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

    // The size of the mistake, not just the formula: forgetting the
    // factor pans twice as far.
    assert_eq!(
        pan_by(view, drag).pan,
        after.pan * 2.0,
        "an unconverted delta pans by exactly the scale factor too much",
    );

    // The property under test: whatever was under the cursor when the
    // drag started is under the cursor where it ended, with both cursor
    // positions measured in the logical pixels the view is driven in.
    let start = Vec2::new(12.0, 5.0);
    assert!(
        (world_at(view, start * inverse_scale) - world_at(after, (start + drag) * inverse_scale))
            .length()
            < 1e-3,
    );
}

/// The view is driven against the stage *area*, which is a plain logical
/// rect test: the area never moves with the view, which is what makes it
/// a usable anchor (see `the_zoom_anchor_holds_over_repeated_ticks...`).
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

    // Bounded by the area's own logical rect, so a cursor over another
    // panel never drives this one.
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

/// The 2D view is per-tab state: switching scenes parks it with the
/// outgoing tab and brings the incoming tab's back. The 3D camera
/// round-trip runs alongside it and must be unaffected.
#[test]
fn tab_switch_round_trips_the_2d_view() {
    use jackdaw::scenes::{SceneTab, Scenes};

    let mut app = util::editor_test_app();
    let parent = app
        .world_mut()
        .spawn((jackdaw::EditorEntity, Node::default()))
        .id();
    build_viewport_2d_panel(app.world_mut(), parent);

    // The panel's own 3D camera, rather than one spawned beside it: every panel
    // builds both presentations now, so "the first viewport camera" would find
    // the panel's and leave a second one untouched.
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
    // something has chosen it; see `an_unframed_tab_is_not_stamped_...`.
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

/// `apply_2d_view` is what makes the stored view visible: it is the only
/// writer of the 2D camera's transform and ortho scale.
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
    // The plugin schedules this inside `EditorInteractionSystems`, which
    // is gated on `AppState::Editor`; the test app never leaves
    // `ProjectSelect`, so run the system directly.
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

#[test]
fn the_panel_registers_as_a_dock_window() {
    let mut app = util::editor_test_app();
    // The startup resolver only enables what the developer's on-disk
    // extensions.json lists, so force this one on rather than depending
    // on the machine the test runs on.
    jackdaw_api_internal::lifecycle::enable_extension(
        app.world_mut(),
        &jackdaw::builtin_extensions::Viewport2dExtension.id(),
    );
    app.update();

    let registry = app.world().resource::<jackdaw_panels::WindowRegistry>();
    let descriptor = registry
        .get("jackdaw.viewport_2d")
        .expect("jackdaw.viewport_2d registered as a dock window");
    assert_eq!(descriptor.name, "2D Viewport");
}

/// The reference-resolution contract. The panel's render-target image is
/// held at the active UI scene's `UiSceneRoot::reference_size`, so the
/// authored tree lays out at exactly the resolution it was designed for,
/// whatever shape the dock leaf happens to be. Bevy derives a root's
/// layout viewport from its target camera's physical viewport size, and
/// an image target's is the image's own size, so the assertion below is
/// the whole contract in one number: half of a 1280-wide reference is
/// 640, not half of the panel.
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

/// Closing the last panel must not *unroute* the scene. An unrouted UI
/// root falls back to `DefaultUiCamera`, the editor's own window, and
/// draws the authored scene straight over the editor chrome. So it parks
/// on an inactive 1x1 image camera instead: still routed, still off the
/// window, and collapsed small enough that nothing of it is on screen.
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

    // The parking camera is not, and can never be picked as, the window
    // camera Bevy would otherwise hand this root back to.
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

/// Routing must never reach disk. `UiTargetCamera` names a camera entity
/// the editor spawned for a panel; saved into a document it would point
/// at nothing on the next load.
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
    // Routed first, registered second: `register_entity_in_ast` is where
    // the skip policy is applied, so registering before the routing
    // landed would prove nothing.
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

/// Pan and zoom are the stage's size and position, because the camera
/// cannot move a UI scene at all: Bevy renders UI through its own
/// origin-parked view (`bevy_ui_render::extract_ui_camera_view`), so
/// `apply_2d_view` moves nothing the user is looking at here.
///
/// Two things follow, and both are load-bearing for `ui_stage`: the stage
/// carries the reference aspect exactly, and `target_pixels_per_stage_pixel`
/// reads the zoom straight back off the laid-out node, so the cursor
/// mapping never has to name it.
#[test]
fn the_view_places_and_scales_the_stage() {
    let mut app = util::editor_test_app();
    // An absolute panel frame, so the stage's computed size below is the
    // placement result rather than an empty headless render target.
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
    // Stated at zoom 1, so the placement below reads as the authored
    // numbers it names rather than as whatever a fit picked.
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

    // At the default view the canvas is drawn at its authored size,
    // centred in the area.
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

    // Zoom doubles the stage and halves the authored-pixels-per-stage-
    // pixel factor, which is the whole reason the cursor mapping needs no
    // zoom term of its own.
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

    // The pan names the authored point at the centre of the area, in
    // authored pixels from the canvas centre, y up. Looking at (100, -50)
    // means looking right and *down* the canvas, so the canvas itself
    // slides the other way: 100 authored px left and 50 up, times the
    // zoom.
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

/// Reference resolution for the mode tests. It is exactly the panel's
/// stage area, and the view is left at zoom 1, so an authored pixel is a
/// window pixel and every position below reads as the number it names.
const MODE_REFERENCE: UVec2 = UVec2::new(1200, 600);

/// Centre of the authored button the mode tests aim at.
const BUTTON_CENTRE: Vec2 = Vec2::new(500.0, 250.0);

/// Lifts the test panel over the project-select screen; see `mode_panel`.
const PANEL_ON_TOP: i32 = 1000;

/// What the authored button under test has heard from the pointer. The
/// signal a live widget runs on: `bevy_ui_widgets` buttons are observers
/// on `Pointer<Press>` and `Pointer<Click>`, not readers of editor state.
#[derive(Resource, Default)]
struct WidgetEvents {
    presses: usize,
    clicks: usize,
}

/// Edit is the authoring mode: a press on the stage selects the authored
/// node under it and outlines it, and the live widget never hears about
/// the pointer at all.
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

/// Interact hands the same press to the scene being edited: the widget
/// reacts, the selection the user made stands, and the authoring overlay
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

/// A press the scene is holding runs to its release wherever the cursor
/// goes, because a stream cut off at the stage's edge leaves the widget
/// latched down: `PointerPress` never clears, no `Click` ever resolves,
/// and the next entry onto the stage opens mid-drag.
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

/// The forwarder is not a rect test. The pointer only enters the scene
/// when the *editor's* own hit test says the stage is what is under the
/// cursor, and when no editor interaction is in flight. Otherwise a popup
/// drawn over the stage, or a drag the user is in the middle of, would
/// leak clicks into the live scene behind it.
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

/// The wheel over the panel is the canvas's only while the canvas is what
/// the cursor is over. A popup or a docked panel drawn over the stage is
/// what the user is scrolling, and the canvas underneath it must hold
/// still; a rect test alone cannot tell the two apart.
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

/// A pan runs to the button coming up, wherever the pointer goes.
/// Reaching the far corner of a canvas larger than its window means
/// dragging past the panel's edge, and a pan resolved by hover every
/// frame stops dead there under a cursor that is still moving.
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
/// stepped around it: `ButtonInput::just_pressed` lasts exactly until the
/// next input pass, and a press has to still be one when this reads it.
fn run_pan_zoom(app: &mut App) {
    jackdaw::viewport_2d::run_2d_pan_zoom(app.world_mut());
}

/// The wheel over a 2D viewport belongs to the canvas. The 3D grid
/// stepper is a modifier-gated wheel handler with no viewport of its own,
/// so without a gate the user zooming a canvas with Shift+Scroll retunes
/// the world grid of the scene in the other panel at the same time.
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
    drive_pointer(&mut app, moved(), on_stage);
    scroll_up(&mut app);
    step_the_world_grid(&mut app);
    assert_eq!(
        grid_power(&app),
        power,
        "the wheel over the canvas is the canvas's, not the world grid's",
    );

    // The same chord anywhere else still steps the world grid, so the
    // gate is the panel and not the whole editor.
    let off_panel = on_stage + Vec2::new(0.0, MODE_REFERENCE.y as f32 * 2.0);
    drive_pointer(&mut app, moved(), off_panel);
    scroll_up(&mut app);
    step_the_world_grid(&mut app);
    assert_eq!(
        grid_power(&app),
        power + 1,
        "away from the panel the chord is the grid stepper it always was",
    );
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

/// Run the 3D grid's scroll handler and whatever it queued. The plugin
/// schedules it inside `EditorInteractionSystems`, which never runs while
/// a test app sits in `AppState::ProjectSelect`.
fn step_the_world_grid(app: &mut App) {
    app.world_mut()
        .run_system_cached(jackdaw::snapping::handle_grid_size_scroll)
        .expect("the grid scroll handler ran");
    settle(app);
}

/// The mode lives on the panel, not in a resource: two 2D viewports can
/// show the same scene with one being authored and the other tried out.
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

fn mode_app() -> App {
    let mut app = util::editor_test_app();
    app.init_resource::<WidgetEvents>();
    app
}

/// A panel whose stage area is exactly [`MODE_REFERENCE`], so the canvas
/// is shown 1:1 and an authored pixel is a window pixel.
fn mode_panel(app: &mut App) -> Entity {
    let parent = app
        .world_mut()
        .spawn((
            jackdaw::EditorEntity,
            // A docked panel is the topmost thing under the cursor. This
            // app is parked in `AppState::ProjectSelect`, whose screen
            // would otherwise sit over the panel and take the hover that
            // decides whether the scene may be reached at all.
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
    // The panel is sized 1:1, so the arriving scene must not be fitted
    // (and margined) into it.
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

/// Move the mouse over the point on screen showing `authored`, then press
/// it, over the whole production path, starting from the `PointerInput`
/// the winit backend writes.
///
/// The press is also delivered to the stage node directly, because what
/// the *editor's* pointer reaches through its own window camera is not
/// something a headless app can be made to reproduce. That second
/// delivery is what puts `on_stage_press` under test in both modes: in
/// Edit it must select, in Interact it must stand down.
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

/// The ladder: powers of two, both ends held, and an off-ladder size
/// pulled back onto it by the first press rather than doubled as it
/// stands.
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

/// The stepper edits the panel it sits in, the readout says what it
/// edited, and a second panel's grid is left alone: the grid is per
/// panel, and rides with the tab's view state from there.
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

/// The operator says the same thing for a scripted run, and takes a size
/// off the ladder as given.
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

/// Gap 4: `viewport2d.frame` framed every open panel while
/// `viewport2d.mode` and `viewport2d.grid` moved whichever one the query
/// happened to yield first. An operator has no panel to be called on, so
/// all three answer for all of them.
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

/// The fit is pure: the canvas at the largest zoom that leaves it and its
/// margin inside the area, centred, and never outside the usable zoom
/// range.
#[test]
fn fitting_sizes_the_canvas_to_the_smaller_axis_of_the_area() {
    // A 16:9 canvas in a square area: width is what runs out first, so
    // the fit is set by the width and the height has slack to spare.
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

    // A canvas far larger than any area still lands inside the range, and
    // a degenerate area must not produce a zero or negative zoom.
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

/// Opening a UI scene frames it. Shown at 100% instead, a 1920x1080
/// reference in a small dock leaf gives the user the top-left corner of
/// their scene and nothing else.
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

/// The `viewport2d.frame` operator is the Fit control: it puts a panel
/// panned and zoomed away back where opening the scene put it.
#[test]
fn the_frame_op_returns_a_panned_and_zoomed_panel_to_the_fit() {
    let mut app = util::editor_test_app();
    let parent = fit_panel(&mut app);
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

/// The header says what is being edited and how far in the view is. The
/// dock tab already says "2D Viewport", so repeating it in the header
/// spends the one line of chrome the panel has on nothing.
#[test]
fn the_header_names_the_scene_and_reads_the_zoom_back() {
    use jackdaw::scenes::{SceneTab, Scenes};

    let mut app = util::editor_test_app();
    let parent = fit_panel(&mut app);
    app.update();

    assert_eq!(
        title_text(&mut app),
        "2D Viewport",
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
    // A framing of the panel's own, so the readout reports a zoom nobody
    // is about to fit away.
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

/// The panel's two captures: the whole editor window, for the chrome
/// around the stage, and the panel's own render target, for what the
/// stage is showing. Both resolve their path parameter, aim at their own
/// surface, and write a PNG through the shared encoder.
///
/// A headless app has no render device, so no GPU readback ever lands.
/// `ScreenshotCaptured` is triggered directly in place of the frame the
/// renderer would have handed back, which puts the dispatch, the aim, the
/// observer and the encoder under test.
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

/// Session restore opens its scenes before it has a workspace to show
/// them in, so both halves of "show me the UI scene" have to survive the
/// panel arriving late.
///
/// `project_select::transition_to_editor` runs `scene_open_system`
/// synchronously right after setting `AppState::Editor`: the dock
/// reconciler has not built a leaf, so the focus the scene asks for finds
/// no tab, and no panel exists, so the fit it asks for finds no host.
/// The focus is held until there is a leaf to honour it on, and a new
/// panel starts with a fit pending, so a restored session opens with the
/// UI scene in front and its canvas framed.
#[test]
fn a_ui_scene_opened_before_the_workspace_exists_is_still_fronted_and_framed() {
    use jackdaw::viewport_2d::{Pending2dFocus, VIEWPORT_2D_WINDOW_ID, focus_2d_viewport_tab};

    let mut app = util::editor_test_app();
    // The dock as restore finds it: no 2D viewport leaf built yet.
    dock_leaf(&mut app, &["jackdaw.outliner"]);

    app.world_mut().spawn((
        UiSceneRoot {
            reference_size: UVec2::new(1920, 1080),
        },
        Node::default(),
    ));
    focus_2d_viewport_tab(app.world_mut());
    request_2d_fit(app.world_mut());
    app.update();

    assert!(
        app.world().get_resource::<Pending2dFocus>().is_some(),
        "a focus the dock cannot honour yet is held, not dropped",
    );

    // Then the workspace materialises: the leaf, with the viewport behind
    // another tab, and the panel the reconciler builds into it.
    let leaf = dock_leaf(&mut app, &["jackdaw.outliner", VIEWPORT_2D_WINDOW_ID]);
    let panel = fit_panel(&mut app);
    for _ in 0..3 {
        app.update();
    }

    assert_eq!(
        active_window(&app, leaf).as_deref(),
        Some(VIEWPORT_2D_WINDOW_ID),
        "the tab the restored scene asked for has to come forward when it can",
    );
    assert!(
        app.world().get_resource::<Pending2dFocus>().is_none(),
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

/// A held focus is owed to the tab that asked for it, and to no other.
///
/// Restore opens *every* persisted tab in turn, activating each as it
/// goes, and only then switches to the one the user left in front. So a
/// UI-scene tab that is not the last active one still asks for a focus
/// while the dock is empty. Left standing, that request is honoured once
/// the leaf is built, and the 2D viewport comes forward over the ordinary
/// scene the user restored.
#[test]
fn a_focus_asked_for_by_a_tab_the_user_left_does_not_front_the_panel() {
    use jackdaw::scenes::swap::swap_active_tab;
    use jackdaw::scenes::{SceneTab, Scenes, TabContent};
    use jackdaw::viewport_2d::{Pending2dFocus, VIEWPORT_2D_WINDOW_ID};

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
        app.world().get_resource::<Pending2dFocus>().is_some(),
        "the UI-scene tab asks for a focus the empty dock cannot honour",
    );

    swap_active_tab(app.world_mut(), 1);
    assert!(
        app.world().get_resource::<Pending2dFocus>().is_none(),
        "activating a tab that is not a UI scene settles the debt: nothing \
         is owed to a tab the user has left",
    );

    // The workspace materialises afterwards, as it does on restore.
    let leaf = dock_leaf(&mut app, &["jackdaw.outliner", VIEWPORT_2D_WINDOW_ID]);
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

/// The two operators a scripted capture of this panel needs. Both are
/// gestures a headless run cannot perform: the mode is a click on a
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

/// `selection.select` resolves a name against the authored scene, and
/// refuses anything it cannot resolve to exactly one node: a capture of
/// the wrong node is worse than a capture that did not happen.
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

    // A second `Play` makes the name ambiguous, and an ambiguous name is
    // refused rather than resolved to whichever entity comes first.
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

/// An editor app with the 2D viewport extension on.
///
/// Its operators are registered by `Viewport2dExtension`, and the startup
/// resolver only enables what the developer's on-disk extensions.json
/// lists, so an op test that skips this passes or fails by the machine it
/// runs on, and by which other tests started beside it.
fn op_app() -> App {
    let mut app = util::editor_test_app();
    jackdaw_api_internal::lifecycle::enable_extension(
        app.world_mut(),
        &jackdaw::builtin_extensions::Viewport2dExtension.id(),
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

/// Withdraw the fit a new panel starts with, as a restored framing does:
/// the tests below that state their positions in authored pixels want the
/// view they set, not the one an arriving scene would frame for them.
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

/// A view nobody moved is not a framing the tab chose, and capturing it
/// as one is how a tab loses its framing permanently.
///
/// The scenario that costs a user their fit: a UI scene opens while no 2D
/// viewport is docked, so the fit its activation asks for finds no panel
/// and is lost. A panel docked afterwards sits at 100%, untouched.
/// Capturing *that* on the way out as the tab's chosen framing, which the
/// restore then honours, leaves every later activation withdrawing the
/// fit it just asked for and the tab stuck at 100% for the session.
///
/// So `ViewState::ui_view` stays `None` until something moves the view.
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
        // Tab 1 is the UI scene, carried as a document so activating it
        // goes through the real spawn-and-frame path.
        scenes.tabs[1].content = TabContent::Scene(Some(Box::new(
            jackdaw_bsn::parse_bsn_text("#Overlay\njackdaw_scene_types::UiSceneRoot\n")
                .expect("the fixture parses"),
        )));
    }

    // Opened with no 2D panel docked: the fit this activation asks for
    // has nothing to land on, and is lost.
    swap_active_tab(app.world_mut(), 1);
    app.update();
    assert_eq!(ui_scene_roots(&mut app), 1, "the UI scene spawned");

    // The panel arrives afterwards, and frames what it finds: a panel
    // that came late is still a panel nobody has framed.
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

/// The op's selection is a real selection, not a resource write nobody
/// hears: `Selection::select_single` inserts `Selected`, and the panels
/// that show a selection (the inspector's card list, the outliner's row
/// highlight) are both `On<Add, Selected>` observers, so a scripted
/// selection lights the editor up as a click does.
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
