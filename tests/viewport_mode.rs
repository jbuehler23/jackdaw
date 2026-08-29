//! One viewport panel, two modes.
//!
//! A panel builds both presentations and shows one. These tests pin what the
//! mode does: which column is in layout, which camera renders, that the switch
//! is reachable from either bar and reads back the mode it is in, and that the
//! `viewport.mode` operator moves every open panel and records the choice as
//! the user's rather than the scene kind's.
//!
//! Also what a layout saved while the canvas was a panel of its own loads
//! as, which panel a UI scene is routed into when several are open, and
//! which surface a capture of the viewport aims at.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use jackdaw::scenes::swap::swap_active_tab;
use jackdaw::scenes::{SceneTab, Scenes, TabContent};
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

/// Two tabs of different kinds, each carried as a document so activating one
/// goes through the real spawn-and-configure path. Tab 0 is a UI screen, tab 1
/// an ordinary world scene.
fn two_scene_tabs(app: &mut App) {
    let mut scenes = app.world_mut().resource_mut::<Scenes>();
    scenes.tabs.clear();
    scenes.tabs.push(SceneTab::new_untitled(1));
    scenes.tabs.push(SceneTab::new_untitled(2));
    scenes.tabs[0].content = TabContent::Scene(Some(Box::new(
        jackdaw_bsn::parse_bsn_text("#Overlay\njackdaw_scene_types::UiSceneRoot\n")
            .expect("the fixture parses"),
    )));
    scenes.tabs[1].content = TabContent::Scene(Some(Box::new(
        jackdaw_bsn::parse_bsn_text("#World\nbevy_transform::components::transform::Transform\n")
            .expect("the fixture parses"),
    )));
    scenes.active = 1;
}

/// The mode stored for a tab, which is what its next activation restores.
fn stored_mode(app: &App, tab: usize) -> Option<ViewportMode> {
    app.world().resource::<Scenes>().tabs[tab]
        .view_state
        .viewport_mode
}

fn switch_mode(app: &mut App, mode: &'static str) {
    app.world_mut()
        .operator("viewport.mode")
        .param("mode", mode)
        .call()
        .expect("viewport.mode dispatches")
        .assert_finished();
    app.update();
}

/// A tab opens in the mode its scene's kind asks for, and a mode the user
/// picked outranks that for the tab it was picked in.
///
/// One contract with two halves, because the second is what makes the first
/// safe to keep applying: the override lives in the tab's view state, so it
/// survives a swap; a tab nobody switched stores none, so it goes on taking
/// its kind's answer instead of being frozen in whatever mode it was left in.
#[test]
fn a_chosen_mode_is_remembered_per_tab_and_an_unchosen_one_is_recomputed() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    two_scene_tabs(&mut app);

    swap_active_tab(app.world_mut(), 0);
    app.update();
    assert_showing(&app, panel, ViewportMode::TwoD);
    assert!(
        !host(&app, panel).mode_chosen,
        "a UI screen opens on the canvas because of what it is, not because \
         anyone asked",
    );

    // The user overrules it for this tab.
    switch_mode(&mut app, "3d");
    assert_showing(&app, panel, ViewportMode::ThreeD);

    swap_active_tab(app.world_mut(), 1);
    app.update();
    assert_showing(&app, panel, ViewportMode::ThreeD);
    assert!(
        !host(&app, panel).mode_chosen,
        "the world scene is in 3D on its own account",
    );
    assert_eq!(
        stored_mode(&app, 0),
        Some(ViewportMode::ThreeD),
        "leaving the tab stores the mode its user chose",
    );

    swap_active_tab(app.world_mut(), 0);
    app.update();
    assert_showing(&app, panel, ViewportMode::ThreeD);
    assert!(
        host(&app, panel).mode_chosen,
        "and coming back restores it over the one the kind asks for",
    );

    swap_active_tab(app.world_mut(), 1);
    app.update();
    swap_active_tab(app.world_mut(), 0);
    app.update();
    assert_eq!(
        stored_mode(&app, 1),
        None,
        "a swap stamps no override on a tab the user never switched",
    );
    assert_showing(&app, panel, ViewportMode::ThreeD);
}

/// The same rule from the other side: a second UI tab that nobody switched
/// still opens on its canvas, however the tab beside it was overruled.
#[test]
fn a_tab_that_was_never_switched_follows_its_scenes_kind() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    two_scene_tabs(&mut app);
    app.world_mut()
        .resource_mut::<Scenes>()
        .tabs
        .push(SceneTab::new_untitled(3));
    app.world_mut().resource_mut::<Scenes>().tabs[2].content = TabContent::Scene(Some(Box::new(
        jackdaw_bsn::parse_bsn_text("#Screen\njackdaw_scene_types::UiSceneRoot\n")
            .expect("the fixture parses"),
    )));

    swap_active_tab(app.world_mut(), 0);
    app.update();
    switch_mode(&mut app, "3d");

    swap_active_tab(app.world_mut(), 2);
    app.update();
    assert_showing(&app, panel, ViewportMode::TwoD);
    assert!(
        !host(&app, panel).mode_chosen,
        "one tab's override is that tab's, and says nothing about another",
    );
}

/// The dock as a saved layout describes it: one leaf holding `windows`,
/// with `fronted`'s tab in front.
fn saved_tree(windows: &[&str], fronted: &str) -> jackdaw_panels::tree::DockTree {
    use jackdaw_panels::{
        area::DockAreaStyle,
        tree::{DockLeaf, DockNode, DockTree},
    };

    let mut tree = DockTree::default();
    let leaf = tree.set_root_leaf(
        DockLeaf::new("center", DockAreaStyle::default())
            .with_windows(windows.iter().copied().map(String::from).collect()),
    );
    let tab = tree
        .get(leaf)
        .and_then(DockNode::as_leaf)
        .and_then(|leaf| {
            leaf.tabs()
                .find_map(|(window, tab)| (window == fronted).then_some(tab))
        })
        .expect("the saved layout holds the window it fronts");
    tree.set_active(leaf, tab);
    tree
}

/// Load `layout` the way opening a project does.
fn load_layout(app: &mut App, layout: serde_json::Value) {
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: std::path::PathBuf::from("/tmp/jackdaw-viewport-mode"),
            config: jackdaw::project::ProjectConfig {
                layout: Some(layout),
                ..default()
            },
        });
    jackdaw::init_layout(app.world_mut());
}

/// The windows the live tree's one leaf holds, and the one in front.
fn loaded_leaf(app: &App) -> (Vec<String>, Option<String>) {
    use jackdaw_panels::tree::{DockNode, DockTree};

    let tree = app.world().resource::<DockTree>();
    let leaf = tree
        .root
        .and_then(|root| tree.get(root))
        .and_then(DockNode::as_leaf)
        .expect("the loaded tree is a single leaf");
    let windows = leaf.tabs().map(|(window, _)| window.to_string()).collect();
    let active = leaf.active.and_then(|active| {
        leaf.tabs()
            .find_map(|(window, tab)| (tab == active).then(|| window.to_string()))
    });
    (windows, active)
}

/// A layout saved while the canvas was a panel of its own docked both of
/// them in one leaf. Reopening it must not show two tabs for the panel
/// they have become, and the tab the user left in front stays in front.
#[test]
fn a_saved_layout_holding_both_viewports_loads_as_one_tab() {
    let mut app = util::editor_test_app();
    let persist = jackdaw_panels::WorkspacesPersist {
        active: Some("authoring".to_string()),
        workspaces: vec![jackdaw_panels::WorkspacePersist {
            id: "authoring".to_string(),
            name: "Authoring".to_string(),
            icon: None,
            accent_color: [0.0, 0.0, 0.0, 1.0],
            tree: saved_tree(
                &["jackdaw.viewport", "jackdaw.viewport_2d"],
                "jackdaw.viewport_2d",
            ),
        }],
    };
    load_layout(
        &mut app,
        serde_json::to_value(&persist).expect("the persist serialises"),
    );

    let (windows, active) = loaded_leaf(&app);
    assert_eq!(windows, vec!["jackdaw.viewport".to_string()]);
    assert_eq!(
        active.as_deref(),
        Some("jackdaw.viewport"),
        "the leaf still fronts a viewport, not a tab that was dropped",
    );
}

/// A layout that docked only the canvas keeps its leaf, showing the panel
/// the canvas is now a mode of.
#[test]
fn a_saved_layout_holding_only_the_canvas_loads_the_viewport() {
    let mut app = util::editor_test_app();
    let tree = saved_tree(&["jackdaw.viewport_2d"], "jackdaw.viewport_2d");
    load_layout(
        &mut app,
        serde_json::to_value(&tree).expect("the tree serialises"),
    );

    let (windows, active) = loaded_leaf(&app);
    assert_eq!(windows, vec!["jackdaw.viewport".to_string()]);
    assert_eq!(active.as_deref(), Some("jackdaw.viewport"));
}

/// With two panels open, the UI scene goes to the one showing the canvas.
/// Routing it into the first panel regardless would leave the user looking
/// at an empty stage while a panel they cannot see holds the scene.
#[test]
fn a_ui_scene_routes_into_the_panel_showing_the_canvas() {
    use bevy::ui::UiTargetCamera;
    use jackdaw::viewport_2d::build_viewport_2d_panel;
    use jackdaw_scene_types::UiSceneRoot;

    let mut app = util::editor_test_app();
    let in_3d = panel(&mut app);
    let in_2d = app
        .world_mut()
        .spawn((jackdaw::EditorEntity, Node::default()))
        .id();
    build_viewport_2d_panel(app.world_mut(), in_2d);
    assert_eq!(host(&app, in_3d).mode, ViewportMode::ThreeD);
    assert_eq!(host(&app, in_2d).mode, ViewportMode::TwoD);

    let root = app
        .world_mut()
        .spawn((
            UiSceneRoot {
                reference_size: UVec2::new(1280, 720),
            },
            Node::default(),
        ))
        .id();
    app.world_mut()
        .run_system_cached(jackdaw::viewport_2d::route_ui_roots_to_cameras)
        .expect("route_ui_roots_to_cameras ran");
    app.update();

    let canvas_camera = app
        .world()
        .get::<Viewport2dPanelHost>(in_2d)
        .expect("host on panel parent")
        .camera;
    assert_eq!(
        app.world()
            .get::<UiTargetCamera>(root)
            .map(UiTargetCamera::entity),
        Some(canvas_camera),
        "the scene is routed into the panel the user can see it in",
    );
}

/// A capture of the viewport is a picture of what the user is looking at,
/// so it follows the mode: the world camera's image in 3D, the canvas
/// camera's in 2D. Aiming at the world camera either way would hand back a
/// picture of an empty world to someone editing a screen.
#[test]
fn the_viewport_capture_aims_at_the_surface_the_mode_is_showing() {
    use bevy::camera::RenderTarget;
    use bevy::render::view::screenshot::Screenshot;

    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let world_camera = app
        .world()
        .get::<ViewportPanelHost>(panel)
        .expect("host on panel parent")
        .camera;
    let canvas_camera = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .camera;

    for (mode, camera) in [
        ("3d", world_camera),
        ("2d", canvas_camera),
        // Back again, so the second aim is a change rather than the state
        // the panel happened to settle in.
        ("3d", world_camera),
    ] {
        switch_mode(&mut app, mode);
        let path = std::env::temp_dir().join(format!("jackdaw-mode-shot-{mode}.png"));
        app.world_mut()
            .operator("viewport.screenshot")
            .param("path", path.to_string_lossy().to_string())
            .call()
            .expect("viewport.screenshot dispatches")
            .assert_finished();
        app.update();

        let expected = match app.world().get::<RenderTarget>(camera) {
            Some(RenderTarget::Image(target)) => target.handle.clone(),
            other => panic!("a presentation camera renders into an image, got {other:?}"),
        };
        let aimed: Vec<(Entity, RenderTarget)> = app
            .world_mut()
            .query::<(Entity, &Screenshot)>()
            .iter(app.world())
            .map(|(entity, shot)| (entity, shot.0.clone()))
            .collect();
        assert_eq!(aimed.len(), 1, "one queued capture per call");
        assert!(
            matches!(&aimed[0].1, RenderTarget::Image(image) if image.handle == expected),
            "in {mode} the capture aims at that presentation's image, got {:?}",
            aimed[0].1,
        );
        app.world_mut().despawn(aimed[0].0);
    }
}
