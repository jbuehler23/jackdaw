//! One viewport panel, two modes.
//!
//! A panel builds both presentations and shows one. These tests pin what the
//! mode does: which column is in layout, which camera renders, that the switch
//! is reachable from either bar and reads back the mode it is in, and that the
//! `viewport.mode` operator moves every open panel and records the choice as
//! the user's rather than the scene kind's.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use jackdaw::{
    viewport::{
        ActiveViewport, ViewportPanelHost, build_viewport_panel, run_active_viewport_update,
    },
    viewport_2d::Viewport2dPanelHost,
    viewport_host::{ViewportHost, ViewportMode, ViewportModeIntent, ViewportModeSegment},
};
use jackdaw_api::op::OperatorWorldExt as _;
use jackdaw_feathers::tokens;

mod util;
use util::OperatorResultExt as _;

/// A viewport panel on a fresh editor app, built the way the dock reconciler
/// builds one.
fn panel(app: &mut App) -> Entity {
    let parent = app
        .world_mut()
        .spawn((jackdaw::EditorEntity, Node::default()))
        .id();
    build_viewport_panel(app.world_mut(), parent);
    parent
}

fn host(app: &App, panel: Entity) -> ViewportHost {
    *app.world()
        .get::<ViewportHost>(panel)
        .expect("host on panel parent")
}

fn display(app: &App, column: Entity) -> Display {
    app.world()
        .get::<Node>(column)
        .expect("a presentation column is a node")
        .display
}

fn renders(app: &App, camera: Entity) -> bool {
    app.world()
        .get::<Camera>(camera)
        .expect("a presentation has a camera")
        .is_active
}

/// Is `entity` inside the subtree rooted at `root`?
fn under(app: &App, entity: Entity, root: Entity) -> bool {
    let mut current = entity;
    loop {
        if current == root {
            return true;
        }
        match app.world().get::<ChildOf>(current).map(ChildOf::parent) {
            Some(parent) => current = parent,
            None => return false,
        }
    }
}

/// Exactly one column is in layout and exactly its camera renders.
fn assert_showing(app: &App, panel: Entity, mode: ViewportMode) {
    let host = host(app, panel);
    assert_eq!(host.mode, mode);
    let shows_3d = mode == ViewportMode::ThreeD;

    assert_eq!(
        display(app, host.three_d),
        if shows_3d {
            Display::Flex
        } else {
            Display::None
        },
        "the 3D column is in layout only in 3D mode",
    );
    assert_eq!(
        display(app, host.two_d),
        if shows_3d {
            Display::None
        } else {
            Display::Flex
        },
        "the 2D column is in layout only in 2D mode",
    );

    let camera_3d = app
        .world()
        .get::<ViewportPanelHost>(panel)
        .expect("the 3D presentation's state")
        .camera;
    let camera_2d = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("the 2D presentation's state")
        .camera;
    assert_eq!(renders(app, camera_3d), shows_3d, "3D camera activity");
    assert_eq!(renders(app, camera_2d), !shows_3d, "2D camera activity");
}

/// Both presentations exist from the start and the mode picks between them,
/// rather than a switch rebuilding the panel: the camera pose, the canvas
/// framing and the per-panel chrome all have to survive a flip.
#[test]
fn switching_the_mode_shows_one_column_and_lets_only_its_camera_render() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    app.update();

    assert_showing(&app, panel, ViewportMode::ThreeD);

    for mode in [ViewportMode::TwoD, ViewportMode::ThreeD, ViewportMode::TwoD] {
        app.world_mut()
            .get_mut::<ViewportHost>(panel)
            .expect("host on panel parent")
            .mode = mode;
        app.update();
        assert_showing(&app, panel, mode);
    }
}

/// The switch is in whichever bar the mode is showing, so there is always a way
/// back out. Both bars carry one, and both read back the mode the panel is in.
#[test]
fn each_bar_carries_the_switch_and_highlights_the_current_mode() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    app.update();

    for mode in [ViewportMode::ThreeD, ViewportMode::TwoD] {
        app.world_mut()
            .get_mut::<ViewportHost>(panel)
            .expect("host on panel parent")
            .mode = mode;
        app.update();

        let host = host(&app, panel);
        let segments: Vec<(Entity, ViewportModeSegment, Color)> = app
            .world_mut()
            .query::<(Entity, &ViewportModeSegment, &BackgroundColor)>()
            .iter(app.world())
            .filter(|(_, segment, _)| segment.host == panel)
            .map(|(entity, segment, background)| (entity, *segment, background.0))
            .collect();

        for column in [host.three_d, host.two_d] {
            let in_bar: Vec<&(Entity, ViewportModeSegment, Color)> = segments
                .iter()
                .filter(|(entity, _, _)| under(&app, *entity, column))
                .collect();
            assert_eq!(
                in_bar.len(),
                2,
                "each presentation's bar carries the whole switch",
            );
            for (_, segment, background) in in_bar {
                let expected = if segment.mode == mode {
                    tokens::TOOLBAR_ACTIVE_BG
                } else {
                    Color::NONE
                };
                assert_eq!(
                    *background, expected,
                    "the {:?} segment reads back the panel's mode",
                    segment.mode,
                );
            }
        }
    }
}

/// An operator call names no panel, so it answers for all of them, the way
/// `viewport2d.mode` does. It also records the mode as chosen: a mode the user
/// asked for outranks the one the scene's kind implies.
#[test]
fn the_mode_operator_reaches_every_open_panel_and_records_the_choice() {
    let mut app = util::editor_test_app();
    let first = panel(&mut app);
    let second = panel(&mut app);
    app.update();

    assert!(
        !host(&app, first).mode_chosen,
        "a freshly built panel is in the mode its kind implies, not a chosen one",
    );

    app.world_mut()
        .operator("viewport.mode")
        .param("mode", "2d")
        .call()
        .expect("viewport.mode dispatches")
        .assert_finished();
    app.update();

    for panel in [first, second] {
        assert_showing(&app, panel, ViewportMode::TwoD);
        assert!(
            host(&app, panel).mode_chosen,
            "the operator is the user asking, so the mode is chosen",
        );
    }
    assert_eq!(
        *app.world().resource::<ViewportModeIntent>(),
        ViewportModeIntent {
            mode: ViewportMode::TwoD,
            chosen: true,
        },
        "the tab's intent records what was asked for, so a swap can restore it",
    );

    app.world_mut()
        .operator("viewport.mode")
        .param("mode", "sideways")
        .call()
        .expect("viewport.mode dispatches")
        .assert_cancelled();
    app.update();
    for panel in [first, second] {
        assert_showing(&app, panel, ViewportMode::TwoD);
    }
}

/// A panel filling a fixed rectangle at the window's origin, so the cursor can
/// be placed inside whichever presentation it is showing.
fn laid_out_panel(app: &mut App) -> Entity {
    let parent = app
        .world_mut()
        .spawn((
            jackdaw::EditorEntity,
            Node {
                position_type: PositionType::Absolute,
                left: px(0),
                top: px(0),
                width: px(PANEL_SIZE.x),
                height: px(PANEL_SIZE.y),
                ..default()
            },
        ))
        .id();
    build_viewport_panel(app.world_mut(), parent);
    parent
}

const PANEL_SIZE: Vec2 = Vec2::new(800.0, 600.0);

/// Low enough in the panel to clear the toolbars above the 3D viewport and the
/// header above the 2D stage, so one position is inside either presentation.
const INSIDE_THE_PANEL: Vec2 = Vec2::new(400.0, 450.0);

fn place_cursor(app: &mut App, position: Vec2) {
    let mut windows = app
        .world_mut()
        .query_filtered::<&mut Window, With<PrimaryWindow>>();
    let mut window = windows
        .single_mut(app.world_mut())
        .expect("headless apps still have a primary window");
    window.set_physical_cursor_position(Some(position.as_dvec2()));
}

fn settle(app: &mut App) {
    for _ in 0..4 {
        app.update();
    }
}

/// One hover authority answers for both modes. It always names the panel and
/// what that panel is showing; it names a camera and a viewport node only in
/// 3D, because those are the 3D presentation's and every world-space tool
/// routes through one of them. That is what makes the 3D tools stand down over
/// a canvas without a gate of their own.
#[test]
fn hovering_a_panel_reports_the_mode_it_is_in() {
    let mut app = util::editor_test_app();
    let panel = laid_out_panel(&mut app);
    settle(&mut app);

    place_cursor(&mut app, INSIDE_THE_PANEL);
    run_active_viewport_update(app.world_mut());

    let active = *app.world().resource::<ActiveViewport>();
    assert_eq!(active.host, Some(panel));
    assert_eq!(active.mode, Some(ViewportMode::ThreeD));
    assert_eq!(
        active.camera,
        Some(
            app.world()
                .get::<ViewportPanelHost>(panel)
                .expect("the 3D presentation's state")
                .camera
        ),
        "the world viewport routes input through its own camera",
    );
    assert!(
        active
            .ui_node
            .is_some_and(|node| under(&app, node, host(&app, panel).three_d)),
        "and through the leaf node inside the presentation being shown",
    );

    app.world_mut()
        .get_mut::<ViewportHost>(panel)
        .expect("host on panel parent")
        .mode = ViewportMode::TwoD;
    settle(&mut app);
    run_active_viewport_update(app.world_mut());

    let active = *app.world().resource::<ActiveViewport>();
    assert_eq!(
        active.host,
        Some(panel),
        "the same panel is under the cursor"
    );
    assert_eq!(active.mode, Some(ViewportMode::TwoD));
    assert_eq!(
        active.camera, None,
        "a hovered canvas offers no camera to aim a world-space tool through",
    );
    assert_eq!(active.ui_node, None, "and no viewport node either");

    place_cursor(&mut app, INSIDE_THE_PANEL + Vec2::new(0.0, PANEL_SIZE.y));
    run_active_viewport_update(app.world_mut());
    assert_eq!(
        app.world().resource::<ActiveViewport>().mode,
        None,
        "off every panel the cursor is over no viewport at all",
    );
}
