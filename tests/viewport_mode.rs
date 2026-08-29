//! One viewport panel, two modes.
//!
//! A panel builds both presentations and shows one. These tests pin what the
//! mode does: which column is in layout, which camera renders, that the switch
//! is reachable from either bar and reads back the mode it is in, and that the
//! `viewport.mode` operator moves every open panel and records the choice as
//! the user's rather than the scene kind's.

use bevy::prelude::*;
use jackdaw::{
    viewport::{ViewportPanelHost, build_viewport_panel},
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
