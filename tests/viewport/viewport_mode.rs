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

use crate::util;

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

use crate::util::OperatorResultExt as _;

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

/// The switch is a radio group: the bar is the group, each segment is a
/// radio button, and the segment the panel is in carries `Checked`. No
/// segment is a hand-rolled `Interaction` control.
#[test]
fn the_switch_is_a_radio_group_and_checks_the_current_mode() {
    use bevy::ui::Checked;
    use bevy::ui_widgets::{RadioButton, RadioGroup};

    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    app.update();

    let segments: Vec<(Entity, ViewportModeSegment)> = app
        .world_mut()
        .query::<(Entity, &ViewportModeSegment)>()
        .iter(app.world())
        .filter(|(_, segment)| segment.host == panel)
        .map(|(entity, segment)| (entity, *segment))
        .collect();
    assert_eq!(segments.len(), 4, "two modes in each presentation's bar");

    for (entity, segment) in segments {
        assert!(
            app.world().get::<RadioButton>(entity).is_some(),
            "a segment is a radio button",
        );
        assert!(
            app.world().get::<Interaction>(entity).is_none(),
            "and not a hand-rolled interaction control",
        );
        let bar = app
            .world()
            .get::<ChildOf>(entity)
            .expect("a segment sits in a bar")
            .parent();
        assert!(
            app.world().get::<RadioGroup>(bar).is_some(),
            "the bar the segments share is the radio group",
        );
        assert_eq!(
            app.world().get::<Checked>(entity).is_some(),
            segment.mode == ViewportMode::ThreeD,
            "the segment the panel is in is the checked one",
        );
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

/// A panel is built in whatever mode the tab is already in. The dock
/// reconciler rebuilds a leaf whenever its tabs change, it is split, or the
/// workspace switches, so a panel that always started in the 3D world would
/// drop a screen's canvas on any of them.
#[test]
fn a_panel_built_while_the_canvas_is_showing_opens_on_the_canvas() {
    let mut app = util::editor_test_app();
    app.world_mut().insert_resource(ViewportModeIntent {
        mode: ViewportMode::TwoD,
        chosen: true,
    });

    let panel = panel(&mut app);
    app.update();

    assert_showing(&app, panel, ViewportMode::TwoD);
    assert!(
        host(&app, panel).mode_chosen,
        "and it opens with the choice the tab carries, not as its kind's default",
    );
}

/// The same rule through the machinery that actually rebuilds panels: adding a
/// tab to the viewport's leaf makes the reconciler tear the panel down and
/// build it again, and the mode has to come back with it.
#[test]
fn a_rebuilt_leaf_brings_the_panel_back_on_the_canvas() {
    use jackdaw_api::prelude::JackdawExtension as _;
    use jackdaw_panels::tree::{DockLeaf, DockNode, DockTree};

    let mut app = util::editor_test_app();
    jackdaw_api_internal::lifecycle::enable_extension(
        app.world_mut(),
        &jackdaw::builtin_extensions::ViewportExtension.id(),
    );
    app.update();

    app.world_mut().spawn((
        jackdaw_panels::reconcile::DockTreeHost::default(),
        Node::default(),
    ));
    let leaf = app.world_mut().resource_mut::<DockTree>().set_root_leaf(
        DockLeaf::new("center", jackdaw_panels::DockAreaStyle::TabBar)
            .with_windows(vec![jackdaw::viewport::VIEWPORT_WINDOW_ID.to_string()]),
    );
    app.update();
    let before = only_panel(&mut app);

    switch_mode(&mut app, "2d");
    assert_showing(&app, before, ViewportMode::TwoD);

    app.world_mut()
        .resource_mut::<DockTree>()
        .add_tab(leaf, "jackdaw.outliner")
        .expect("the viewport's leaf takes a second tab");
    app.update();

    assert_eq!(
        app.world()
            .resource::<DockTree>()
            .get(leaf)
            .and_then(DockNode::as_leaf)
            .map(|leaf| leaf.windows.len()),
        Some(2),
        "the leaf really did change, or the rebuild never happened",
    );
    let rebuilt = only_panel(&mut app);
    assert_ne!(rebuilt, before, "the reconciler built a fresh panel");
    assert_showing(&app, rebuilt, ViewportMode::TwoD);
}

/// The one panel in the world, which a rebuild replaces with a new entity.
fn only_panel(app: &mut App) -> Entity {
    let panels: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<ViewportHost>>()
        .iter(app.world())
        .collect();
    assert_eq!(panels.len(), 1, "one viewport leaf, one panel");
    panels[0]
}

/// Asking for a mode with no panel open is not a refusal: the request is what
/// the next panel opens in, so a run that sets the mode before the dock has a
/// viewport leaf still lands.
#[test]
fn the_mode_operator_records_a_mode_with_no_panel_to_move() {
    let mut app = util::editor_test_app();

    switch_mode(&mut app, "2d");
    assert_eq!(
        *app.world().resource::<ViewportModeIntent>(),
        ViewportModeIntent {
            mode: ViewportMode::TwoD,
            chosen: true,
        },
    );

    let panel = panel(&mut app);
    app.update();
    assert_showing(&app, panel, ViewportMode::TwoD);
}

/// A world scene's imported overlay belongs to the world, so it is routed into
/// a panel showing the world. Sending it to the panel answering for the canvas
/// would aim it at a camera that panel's mode has switched off, and the overlay
/// would vanish from the panel still showing the world.
#[test]
fn an_imported_overlay_routes_into_the_panel_showing_the_world() {
    use bevy::ui::UiTargetCamera;
    use jackdaw::viewport::ViewportPanelHost;
    use jackdaw_scene_types::UiSceneRoot;

    let mut app = util::editor_test_app();
    app.world_mut().insert_resource(ViewportModeIntent {
        mode: ViewportMode::TwoD,
        chosen: true,
    });
    let in_2d = panel(&mut app);
    app.world_mut().insert_resource(ViewportModeIntent {
        mode: ViewportMode::ThreeD,
        chosen: true,
    });
    let in_3d = panel(&mut app);
    assert_eq!(host(&app, in_2d).mode, ViewportMode::TwoD);
    assert_eq!(host(&app, in_3d).mode, ViewportMode::ThreeD);

    // Imported rather than authored: an `IsA` instance root with no `Prefab`
    // of its own, which is what tells the routing it is an overlay a world
    // scene pulled in rather than the document being edited.
    let root = app
        .world_mut()
        .spawn((
            UiSceneRoot {
                reference_size: UVec2::new(1280, 720),
            },
            Node::default(),
            jackdaw::prefab::IsA {
                source: std::path::PathBuf::from("hud.bsn"),
                deleted: Vec::new(),
            },
        ))
        .id();
    app.world_mut()
        .run_system_cached(jackdaw::viewport_2d::route_ui_roots_to_cameras)
        .expect("route_ui_roots_to_cameras ran");
    app.update();

    let world_camera = app
        .world()
        .get::<ViewportPanelHost>(in_3d)
        .expect("host on panel parent")
        .camera;
    assert_eq!(
        app.world()
            .get::<UiTargetCamera>(root)
            .map(UiTargetCamera::entity),
        Some(world_camera),
        "the overlay is routed into a panel that is showing the world",
    );
}

/// Every segment of the switch that names `panel` and `mode`. There is one in
/// each presentation's bar, because whichever bar is showing has to carry the
/// way back out.
fn segments_for(app: &mut App, panel: Entity, mode: ViewportMode) -> Vec<Entity> {
    let found: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &ViewportModeSegment)>()
        .iter(app.world())
        .filter(|(_, segment)| segment.host == panel && segment.mode == mode)
        .map(|(entity, _)| entity)
        .collect();
    assert_eq!(found.len(), 2, "one segment per mode per presentation bar");
    found
}

/// Click a segment the way a user does: the `Pointer<Click>` its inline
/// observer is watching for.
fn click(app: &mut App, segment: Entity) {
    use bevy::camera::{NormalizedRenderTarget, RenderTarget};
    use bevy::picking::{
        backend::HitData,
        events::{Click, Pointer},
        pointer::{Location, PointerButton, PointerId},
    };
    use bevy::window::WindowRef;

    let window = app
        .world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(app.world())
        .expect("headless apps still have a primary window");
    let target: NormalizedRenderTarget = RenderTarget::Window(WindowRef::Primary)
        .normalize(Some(window))
        .expect("the primary window normalizes");
    let camera = app
        .world()
        .get::<ViewportPanelHost>(
            app.world()
                .get::<ViewportModeSegment>(segment)
                .expect("a segment names its panel")
                .host,
        )
        .expect("the 3D presentation's state")
        .camera;

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

/// Clicking a segment moves the panel it names and records the mode as one the
/// user asked for. The panel is carried on the segment rather than looked up,
/// so a switch in one panel's bar leaves the panel beside it alone.
#[test]
fn clicking_a_segment_moves_only_the_panel_it_names() {
    let mut app = util::editor_test_app();
    let first = panel(&mut app);
    let second = panel(&mut app);
    app.update();

    let segment = segments_for(&mut app, first, ViewportMode::TwoD)[0];
    click(&mut app, segment);
    app.update();

    assert_showing(&app, first, ViewportMode::TwoD);
    assert!(
        host(&app, first).mode_chosen,
        "the switch is the user asking, so the mode is chosen",
    );
    assert_showing(&app, second, ViewportMode::ThreeD);
    assert!(
        !host(&app, second).mode_chosen,
        "and the panel beside it is untouched",
    );
    assert_eq!(
        *app.world().resource::<ViewportModeIntent>(),
        ViewportModeIntent {
            mode: ViewportMode::TwoD,
            chosen: true,
        },
        "the tab's intent records the choice, so a swap can restore it",
    );
}

/// A disabled segment is not a switch. The observer reads the flag itself,
/// because a `Pointer<Click>` reaches an entity whatever its interaction state.
#[test]
fn a_disabled_segment_does_not_switch_the_mode() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    app.update();

    let segment = segments_for(&mut app, panel, ViewportMode::TwoD)[0];
    app.world_mut()
        .entity_mut(segment)
        .insert(bevy::ui::InteractionDisabled);
    click(&mut app, segment);
    app.update();

    assert_showing(&app, panel, ViewportMode::ThreeD);
    assert_eq!(
        *app.world().resource::<ViewportModeIntent>(),
        ViewportModeIntent::default(),
        "and nothing was recorded for the tab either",
    );
}

/// Everything the `Update` schedule orders after `system`.
///
/// An ordering may be stated against the system, against a set holding it, or
/// against a set holding that: `.after(a_system)` names the anonymous set of
/// everything with that system's type. So the walk starts from the system and
/// every set that contains it, follows the dependency edges out, and expands
/// each set it lands on into its members.
fn ordered_after(app: &App, system: &str) -> std::collections::HashSet<String> {
    use bevy::ecs::schedule::{NodeId, Schedules};
    use std::collections::HashSet;

    let schedule = app
        .world()
        .resource::<Schedules>()
        .get(Update)
        .expect("the app has an Update schedule");
    let names: Vec<(NodeId, String)> = schedule
        .systems()
        .expect("the schedule has run, so its systems are built")
        .map(|(key, system)| (NodeId::System(key), system.name().as_string()))
        .collect();
    let start = names
        .iter()
        .find(|(_, name)| name.ends_with(system))
        .map(|(node, _)| *node)
        .unwrap_or_else(|| panic!("{system} is scheduled in Update"));

    let graph = schedule.graph();
    let hierarchy: Vec<(NodeId, NodeId)> = graph.hierarchy().graph().all_edges().collect();
    let dependency: Vec<(NodeId, NodeId)> = graph.dependency().graph().all_edges().collect();

    let mut sources = vec![start];
    let mut seen_sources: HashSet<NodeId> = [start].into_iter().collect();
    let mut next = 0;
    while next < sources.len() {
        let node = sources[next];
        next += 1;
        for (parent, child) in &hierarchy {
            if *child == node && seen_sources.insert(*parent) {
                sources.push(*parent);
            }
        }
    }

    let mut after: HashSet<NodeId> = HashSet::new();
    let mut frontier: Vec<NodeId> = Vec::new();
    for (from, to) in &dependency {
        if seen_sources.contains(from) && after.insert(*to) {
            frontier.push(*to);
        }
    }
    while let Some(node) = frontier.pop() {
        for (from, to) in &dependency {
            if *from == node && after.insert(*to) {
                frontier.push(*to);
            }
        }
        for (parent, child) in &hierarchy {
            if *parent == node && after.insert(*child) {
                frontier.push(*child);
            }
        }
    }

    names
        .into_iter()
        .filter(|(node, _)| after.contains(node))
        .map(|(_, name)| name)
        .collect()
}

/// The grid-size chord reads the hovered panel's mode, so it has to read this
/// frame's answer. Both systems sit in one set with no edge of their own, so
/// without the ordering the executor is free to run the chord first, and the
/// first scroll after the cursor moves onto a canvas retunes the world's grid.
#[test]
fn the_grid_size_chord_runs_after_the_hover_pass() {
    let app = util::editor_test_app();
    let after = ordered_after(&app, "update_active_viewport");
    assert!(
        after
            .iter()
            .any(|name| name.ends_with("handle_grid_size_scroll")),
        "the grid-size chord must be ordered after the hover pass",
    );
}
