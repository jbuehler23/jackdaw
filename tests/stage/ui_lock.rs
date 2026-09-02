//! Locking a node so the canvas stops picking it up.
//!
//! What is pinned here:
//!  * a press over a locked node reaches whatever is under it, down to the
//!    scene root, rather than being swallowed;
//!  * the lock is on the node the author locked, not on its children;
//!  * the outliner still selects a locked node, which is how it is reached
//!    to be unlocked again;
//!  * the lock is document data, so it survives a save and a reload;
//!  * the row's padlock says which way the lock is set.

use crate::util;

use crate::util::OperatorResultExt as _;
use bevy::{
    camera::{NormalizedRenderTarget, RenderTarget},
    picking::{
        backend::HitData,
        events::{Pointer, Press},
        pointer::{Location, PointerButton, PointerId},
    },
    prelude::*,
    ui::ComputedNode,
    window::{PrimaryWindow, WindowRef},
};
use jackdaw::boot_ops::run_op_clause_as_user;
use jackdaw::hierarchy::{HierarchyShowAll, HierarchyTreeContainer, set_locked};
use jackdaw::selection::Selection;
use jackdaw::viewport_2d::{Viewport2dPanelHost, build_viewport_2d_panel};
use jackdaw_feathers::tokens::TOOLBAR_HEIGHT;
use jackdaw_scene_types::{Locked, UiSceneRoot};
use jackdaw_widgets::tree_view::{TreeNode, TreeRowClicked, TreeRowContent, TreeRowLockToggle};

const REFERENCE: UVec2 = UVec2::new(2400, 1200);

fn settle(app: &mut App) {
    for _ in 0..4 {
        app.update();
    }
}

fn panel(app: &mut App) -> Entity {
    let parent = app
        .world_mut()
        .spawn((
            jackdaw::EditorEntity,
            Node {
                width: px(1200.0 + jackdaw::viewport_2d::RULER_SIZE),
                height: px(600.0 + jackdaw::viewport_2d::RULER_SIZE + TOOLBAR_HEIGHT),
                ..default()
            },
        ))
        .id();
    build_viewport_2d_panel(app.world_mut(), parent);
    let mut host = app
        .world_mut()
        .get_mut::<Viewport2dPanelHost>(parent)
        .expect("host on panel parent");
    host.view.zoom = 0.5;
    host.fit_pending = false;
    parent
}

fn root(app: &mut App) -> Entity {
    let root = app
        .world_mut()
        .spawn((
            Name::new("UiRoot"),
            UiSceneRoot {
                reference_size: REFERENCE,
            },
            Node {
                width: percent(100),
                height: percent(100),
                ..default()
            },
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), root);
    root
}

fn child(
    app: &mut App,
    parent: Entity,
    name: &str,
    left: f32,
    top: f32,
    width: f32,
    height: f32,
) -> Entity {
    let entity = app
        .world_mut()
        .spawn((
            Name::new(name.to_string()),
            Node {
                position_type: PositionType::Absolute,
                left: px(left),
                top: px(top),
                width: px(width),
                height: px(height),
                ..default()
            },
            ChildOf(parent),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    entity
}

/// Where on screen the panel is showing authored point `authored`.
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
    let area_centre_logical = centre * computed.inverse_scale_factor();
    let logical = area_centre_logical + (authored - focus) * view.zoom;
    logical * app.world().resource::<UiScale>().0
}

/// Press the primary button over the point on screen showing `authored`.
fn click_authored(app: &mut App, panel: Entity, authored: Vec2) {
    let (stage, camera) = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .map(|host| (host.stage, host.camera))
        .expect("host on panel parent");
    let position = screen_position_of(app, panel, authored);
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(app.world())
        .expect("headless apps still have a primary window");
    let render_target: NormalizedRenderTarget = RenderTarget::Window(WindowRef::Primary)
        .normalize(Some(window))
        .expect("the primary window normalizes");
    app.world_mut().trigger(Pointer::new(
        PointerId::Mouse,
        Location {
            target: render_target,
            position,
        },
        Press {
            button: PointerButton::Primary,
            hit: HitData::new(camera, 0.0, None, None),
            count: 1,
        },
        stage,
    ));
    settle(app);
}

fn selection(app: &App) -> Vec<Entity> {
    app.world().resource::<Selection>().entities.clone()
}

#[test]
fn a_press_over_a_locked_node_reaches_what_is_under_it() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let root = root(&mut app);
    let under = child(&mut app, root, "Under", 100.0, 100.0, 400.0, 400.0);
    let over = child(&mut app, root, "Over", 100.0, 100.0, 400.0, 400.0);
    settle(&mut app);

    click_authored(&mut app, panel, Vec2::new(300.0, 300.0));
    assert_eq!(selection(&app), vec![over], "the later sibling is on top");

    set_locked(app.world_mut(), over, true);
    settle(&mut app);
    click_authored(&mut app, panel, Vec2::new(300.0, 300.0));

    assert_eq!(
        selection(&app),
        vec![under],
        "the lock takes the top node out of the pick, it does not swallow the press",
    );
}

#[test]
fn a_locked_node_does_not_swallow_the_press_it_declines() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let root = root(&mut app);
    let backdrop = child(&mut app, root, "Backdrop", 0.0, 0.0, 600.0, 600.0);
    let other = child(&mut app, root, "Other", 900.0, 100.0, 100.0, 100.0);
    settle(&mut app);
    jackdaw::selection::select_only(app.world_mut(), other);
    settle(&mut app);

    set_locked(app.world_mut(), backdrop, true);
    settle(&mut app);
    click_authored(&mut app, panel, Vec2::new(300.0, 300.0));

    assert_eq!(
        selection(&app),
        vec![root],
        "the press carries on down past the lock to the root behind it",
    );
    let _ = other;
}

#[test]
fn locking_a_container_leaves_its_children_pickable() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let root = root(&mut app);
    let container = child(&mut app, root, "Container", 0.0, 0.0, 600.0, 600.0);
    let inside = child(&mut app, container, "Inside", 100.0, 100.0, 200.0, 200.0);
    settle(&mut app);

    set_locked(app.world_mut(), container, true);
    settle(&mut app);
    click_authored(&mut app, panel, Vec2::new(200.0, 200.0));

    assert_eq!(
        selection(&app),
        vec![inside],
        "the lock is on the node it was set on, not on everything inside it",
    );
}

#[test]
fn the_outliner_still_selects_a_locked_node() {
    let mut app = util::editor_test_app();
    app.world_mut().insert_resource(HierarchyShowAll(true));
    app.world_mut().spawn((
        HierarchyTreeContainer,
        Node::default(),
        Visibility::Inherited,
    ));
    panel(&mut app);
    let root = root(&mut app);
    settle(&mut app);
    set_locked(app.world_mut(), root, true);
    settle(&mut app);

    let mut rows = app.world_mut().query::<(Entity, &TreeNode)>();
    let row = rows
        .iter(app.world())
        .find(|(_, node)| node.0 == root)
        .map(|(row, _)| row)
        .expect("the locked node still has a row");
    app.world_mut().trigger(TreeRowClicked {
        entity: row,
        source_entity: root,
    });
    settle(&mut app);

    assert_eq!(
        selection(&app),
        vec![root],
        "the outliner is how a locked node is reached to be unlocked again",
    );
}

#[test]
fn the_row_shows_a_closed_padlock_once_the_node_is_locked() {
    let mut app = util::editor_test_app();
    app.world_mut().insert_resource(HierarchyShowAll(true));
    app.world_mut().spawn((
        HierarchyTreeContainer,
        Node::default(),
        Visibility::Inherited,
    ));
    panel(&mut app);
    let root = root(&mut app);
    settle(&mut app);

    let open = lock_glyph(&mut app, root);
    set_locked(app.world_mut(), root, true);
    settle(&mut app);
    let closed = lock_glyph(&mut app, root);

    assert_eq!(
        open,
        String::from(jackdaw_feathers::icons::Icon::LockOpen.unicode()),
        "an unlocked row carries the open padlock",
    );
    assert_eq!(
        closed,
        String::from(jackdaw_feathers::icons::Icon::Lock.unicode()),
        "the glyph follows the lock",
    );
}

/// The text the lock control on `source`'s row is drawing.
fn lock_glyph(app: &mut App, source: Entity) -> String {
    let mut rows = app.world_mut().query::<(Entity, &TreeNode)>();
    let row = rows
        .iter(app.world())
        .find(|(_, node)| node.0 == source)
        .map(|(row, _)| row)
        .expect("the source has a row");
    let world = app.world();
    world
        .get::<Children>(row)
        .into_iter()
        .flatten()
        .filter(|&&child| world.get::<TreeRowContent>(child).is_some())
        .flat_map(|&content| world.get::<Children>(content).into_iter().flatten())
        .filter(|&&child| world.get::<TreeRowLockToggle>(child).is_some())
        .flat_map(|&toggle| world.get::<Children>(toggle).into_iter().flatten())
        .find_map(|&child| world.get::<Text>(child).map(|text| text.0.clone()))
        .expect("the lock control draws a glyph")
}

#[test]
fn a_lock_survives_a_save_and_a_reload() {
    let mut app = util::editor_test_app();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("locked.bsn");
    run_op_clause_as_user(
        app.world_mut(),
        &format!("scene.new ui=true path={}", path.display()),
    )
    .expect("scene.new dispatches")
    .assert_finished();
    settle(&mut app);

    let scene_root = app
        .world_mut()
        .query_filtered::<Entity, With<UiSceneRoot>>()
        .iter(app.world())
        .next()
        .expect("the new scene has a root");
    let node = child(&mut app, scene_root, "Backdrop", 0.0, 0.0, 400.0, 400.0);
    settle(&mut app);
    set_locked(app.world_mut(), node, true);
    settle(&mut app);

    run_op_clause_as_user(app.world_mut(), "scene.save")
        .expect("scene.save dispatches")
        .assert_finished();
    settle(&mut app);
    let saved = std::fs::read_to_string(&path).expect("the scene saved");
    assert!(
        saved.contains("Locked"),
        "the saved document states the lock:\n{saved}",
    );
    run_op_clause_as_user(
        app.world_mut(),
        &format!("scene.open path={}", path.display()),
    )
    .expect("scene.open dispatches")
    .assert_finished();
    settle(&mut app);

    let reloaded = app
        .world_mut()
        .query::<(Entity, &Name)>()
        .iter(app.world())
        .find(|(_, name)| name.as_str() == "Backdrop")
        .map(|(entity, _)| entity)
        .expect("the reloaded scene holds the node");
    assert!(
        app.world().get::<Locked>(reloaded).is_some(),
        "the lock is document data, so it comes back with the document",
    );
}

/// The keyboard is the canvas too: a locked node the pointer refuses to pick
/// up must not be moved by the arrow keys either.
#[test]
fn a_nudge_leaves_a_locked_node_where_it_is() {
    let mut app = util::editor_test_app();
    let _panel = panel(&mut app);
    let root = root(&mut app);
    let node = child(&mut app, root, "Backdrop", 100.0, 100.0, 400.0, 400.0);
    settle(&mut app);
    set_locked(app.world_mut(), node, true);
    jackdaw::selection::select_only(app.world_mut(), node);
    settle(&mut app);

    run_op_clause_as_user(app.world_mut(), "transform.nudge_x_pos")
        .expect("the nudge dispatches")
        .assert_finished();
    settle(&mut app);

    assert_eq!(
        app.world().get::<Node>(node).expect("a node").left,
        px(100.0),
        "the locked node stayed where it was",
    );

    set_locked(app.world_mut(), node, false);
    settle(&mut app);
    run_op_clause_as_user(app.world_mut(), "transform.nudge_x_pos")
        .expect("the nudge dispatches")
        .assert_finished();
    settle(&mut app);
    assert_eq!(
        app.world().get::<Node>(node).expect("a node").left,
        px(101.0),
        "and the same press moves it once the lock is off",
    );
}
