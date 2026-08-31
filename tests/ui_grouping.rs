//! Grouping a selection into a container, and taking one apart again.
//!
//! What is pinned here:
//!  * the container lands on the selection's bounding rect, flowing along
//!    whichever side of it is longer;
//!  * every member keeps the place it had on the canvas, its authored rect
//!    re-expressed against the container's offset box;
//!  * ungroup puts the children back in the container's own slot, again
//!    without moving them, and takes the empty container away;
//!  * each is one history entry that undoes cleanly;
//!  * both refuse outside a UI scene.

use bevy::{prelude::*, ui::ComputedNode};
use jackdaw::boot_ops::run_op_clause_as_user;
use jackdaw::commands::CommandHistory;
use jackdaw::selection::Selection;
use jackdaw::viewport_2d::{Viewport2dPanelHost, build_viewport_2d_panel};
use jackdaw_api::prelude::*;
use jackdaw_feathers::tokens::TOOLBAR_HEIGHT;
use jackdaw_scene_types::UiSceneRoot;

mod util;

/// The reference resolution the scenes below are authored against: twice the
/// stage the panel lays out, so every conversion factor is an exact 2.
const REFERENCE: UVec2 = UVec2::new(2400, 1200);

/// Run one clause the way a chord runs it.
///
/// `creates_history_entry`, which a scripted call leaves off, is what makes
/// the dispatcher open a snapshot span: an operator that records its own entry
/// and one that leaves the entry to the snapshot are only told apart under a
/// press, and this suite counts entries.
#[track_caller]
fn run_finished(app: &mut App, clause: &str) {
    let result = run_op_clause_as_user(app.world_mut(), clause)
        .unwrap_or_else(|err| panic!("{clause}: dispatch errored: {err}"));
    settle(app);
    assert_eq!(
        result,
        OperatorResult::Finished,
        "{clause} reported {result:?}"
    );
}

fn settle(app: &mut App) {
    for _ in 0..4 {
        app.update();
    }
}

/// A 2D panel framed so the whole authored canvas fits it, which is what
/// gives the authored scene a target to be laid out against.
fn panel(app: &mut App) {
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
}

/// A canvas-filling root with two absolutely placed children, registered in
/// the document the way a load leaves them.
///
/// The root carries a border, so the box the children measure their offsets
/// from does not start at the canvas corner: a container placed at their
/// bounding box has to take that shift back out, and a group that reads the
/// wrong box lands ten pixels off.
fn authored_scene(app: &mut App) -> (Entity, Entity, Entity) {
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
                border: UiRect::all(px(10.0)),
                ..default()
            },
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), root);
    let first = child(app, root, "First", 200.0, 100.0, 400.0, 200.0);
    let second = child(app, root, "Second", 400.0, 200.0, 400.0, 200.0);
    settle(app);
    (root, first, second)
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

fn node_of(app: &App, entity: Entity) -> Node {
    app.world().get::<Node>(entity).cloned().expect("a node")
}

/// The entity's laid-out rect in global authored pixels.
fn rect_of(app: &App, entity: Entity) -> Rect {
    let size = app
        .world()
        .get::<ComputedNode>(entity)
        .expect("a laid-out node")
        .size();
    let centre = app
        .world()
        .get::<bevy::ui::UiGlobalTransform>(entity)
        .expect("a laid-out node")
        .translation;
    Rect::from_corners(centre - size / 2.0, centre + size / 2.0)
}

fn select_both(app: &mut App, entities: [Entity; 2]) {
    app.world_mut().resource_mut::<Selection>().entities = entities.to_vec();
    settle(app);
}

fn undo_depth(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

fn named(app: &mut App, wanted: &str) -> Option<Entity> {
    app.world_mut()
        .query::<(Entity, &Name)>()
        .iter(app.world())
        .find(|(_, name)| name.as_str() == wanted)
        .map(|(entity, _)| entity)
}

#[test]
fn a_group_wraps_the_selection_at_its_bounding_rect() {
    let mut app = util::editor_test_app();
    panel(&mut app);
    let (root, first, second) = authored_scene(&mut app);
    let before = [rect_of(&app, first), rect_of(&app, second)];
    select_both(&mut app, [first, second]);

    run_finished(&mut app, "ui.group_into");

    let container = named(&mut app, "Group").expect("the group container");
    assert_eq!(
        app.world().get::<ChildOf>(container).map(ChildOf::parent),
        Some(root),
        "the container took the members' own parent"
    );
    let node = node_of(&app, container);
    assert_eq!(node.position_type, PositionType::Absolute);
    assert_eq!((node.left, node.top), (px(200.0), px(100.0)));
    assert_eq!((node.width, node.height), (px(600.0), px(300.0)));
    assert_eq!(
        node.flex_direction,
        FlexDirection::Row,
        "a box wider than it is tall flows as a row"
    );

    let children: Vec<Entity> = app
        .world()
        .get::<Children>(container)
        .map(|children| children.iter().collect())
        .unwrap_or_default();
    assert_eq!(children, vec![first, second], "in their visual order");

    assert_eq!(
        (node_of(&app, first).left, node_of(&app, first).top),
        (px(0.0), px(0.0))
    );
    assert_eq!(
        (node_of(&app, second).left, node_of(&app, second).top),
        (px(200.0), px(100.0))
    );

    settle(&mut app);
    assert_eq!(
        [rect_of(&app, first), rect_of(&app, second)],
        before,
        "grouping moved a member on the canvas"
    );

    assert!(
        app.world()
            .resource::<Selection>()
            .entities
            .contains(&container),
        "the container is what is selected now"
    );
}

#[test]
fn a_group_is_one_entry_and_undo_puts_the_members_back() {
    let mut app = util::editor_test_app();
    panel(&mut app);
    let (root, first, second) = authored_scene(&mut app);
    let before = [node_of(&app, first), node_of(&app, second)];
    select_both(&mut app, [first, second]);
    let depth = undo_depth(&app);

    run_finished(&mut app, "ui.group_into");
    assert_eq!(
        undo_depth(&app) - depth,
        1,
        "one group is one history entry"
    );

    run_finished(&mut app, "history.undo");
    assert!(named(&mut app, "Group").is_none(), "the container is gone");
    assert_eq!(
        app.world().get::<ChildOf>(first).map(ChildOf::parent),
        Some(root)
    );
    assert_eq!(
        [node_of(&app, first), node_of(&app, second)],
        before,
        "undo put the authored rects back"
    );
}

#[test]
fn ungroup_lifts_the_children_into_the_containers_place() {
    let mut app = util::editor_test_app();
    panel(&mut app);
    let (root, first, second) = authored_scene(&mut app);
    let before = [node_of(&app, first), node_of(&app, second)];
    let rects = [rect_of(&app, first), rect_of(&app, second)];
    select_both(&mut app, [first, second]);
    run_finished(&mut app, "ui.group_into");

    let container = named(&mut app, "Group").expect("the group container");
    let depth = undo_depth(&app);
    run_finished(&mut app, "ui.ungroup");

    assert!(
        app.world().get_entity(container).is_err(),
        "the emptied container went away"
    );
    assert_eq!(
        app.world().get::<ChildOf>(first).map(ChildOf::parent),
        Some(root),
        "the children came out into the container's own parent"
    );
    assert_eq!(
        [node_of(&app, first), node_of(&app, second)],
        before,
        "the authored rects came back to what they were outside the group"
    );
    settle(&mut app);
    assert_eq!(
        [rect_of(&app, first), rect_of(&app, second)],
        rects,
        "ungrouping moved a child on the canvas"
    );
    assert_eq!(
        undo_depth(&app) - depth,
        1,
        "one ungroup is one history entry"
    );

    run_finished(&mut app, "history.undo");
    let restored = named(&mut app, "Group").expect("undo put the container back");
    assert_eq!(
        app.world().get::<ChildOf>(first).map(ChildOf::parent),
        Some(restored),
        "the children went back inside it"
    );
}

#[test]
fn grouping_refuses_outside_a_ui_scene() {
    let mut app = util::editor_test_app();
    let entity = app
        .world_mut()
        .spawn((Name::new("Loose"), Node::default()))
        .id();
    app.world_mut().resource_mut::<Selection>().entities = vec![entity];
    app.update();

    for id in ["ui.group_into", "ui.ungroup"] {
        assert!(
            !app.world_mut()
                .operator(id)
                .is_available()
                .unwrap_or_else(|err| panic!("{id}: is_available errored: {err}")),
            "{id} should refuse with no UI scene open"
        );
    }
}

/// A container holds whatever a widget definition put inside it, not only
/// laid-out nodes. Ungroup used to move the nodes and leave the rest to be
/// despawned with the container, which lost them, and with a childless
/// snapshot undo had nothing to put back.
#[test]
fn ungroup_carries_out_a_child_that_is_not_a_node() {
    let mut app = util::editor_test_app();
    panel(&mut app);
    let (root, first, second) = authored_scene(&mut app);
    select_both(&mut app, [first, second]);
    run_finished(&mut app, "ui.group_into");
    let container = named(&mut app, "Group").expect("the group container");

    // A plain marker-component child, of the kind a widget's own parts are.
    let marker = app
        .world_mut()
        .spawn((Name::new("Marker"), ChildOf(container)))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), marker);
    settle(&mut app);

    jackdaw::selection::select_only(app.world_mut(), container);
    settle(&mut app);
    run_finished(&mut app, "ui.ungroup");

    assert!(
        app.world().get_entity(marker).is_ok(),
        "the child with no Node was despawned with the container"
    );
    assert_eq!(
        app.world().get::<ChildOf>(marker).map(ChildOf::parent),
        Some(root),
        "it came out into the container's own parent, like the nodes did"
    );
    assert_eq!(
        app.world()
            .get::<Name>(first)
            .map(|n| n.as_str().to_owned()),
        Some("First".to_string()),
        "the laid-out children came out too"
    );

    run_finished(&mut app, "history.undo");
    let restored = named(&mut app, "Group").expect("undo put the container back");
    assert_eq!(
        app.world().get::<ChildOf>(marker).map(ChildOf::parent),
        Some(restored),
        "undo put the non-node child back inside the container"
    );
    let inside: Vec<Entity> = app
        .world()
        .get::<Children>(restored)
        .map(|children| children.iter().collect())
        .unwrap_or_default();
    assert_eq!(
        inside.len(),
        3,
        "undo restored exactly what was inside, with nothing doubled"
    );
}

/// The scene's own root is what the canvas draws, what a paste falls back to
/// and what the outliner hangs the scene off. Ctrl+Shift+G on it used to
/// delete it and leave the editor with a scene it could not draw.
#[test]
fn neither_operator_touches_the_scene_root() {
    let mut app = util::editor_test_app();
    panel(&mut app);
    let (root, _, _) = authored_scene(&mut app);
    jackdaw::selection::select_only(app.world_mut(), root);
    settle(&mut app);

    for id in ["ui.group_into", "ui.ungroup"] {
        assert!(
            !app.world_mut()
                .operator(id)
                .is_available()
                .unwrap_or_else(|err| panic!("{id}: is_available errored: {err}")),
            "{id} must refuse while the scene root is the selection"
        );
    }

    // And the world functions refuse too, so a caller reaching past the
    // availability gate gets a refusal rather than a broken document.
    let depth = undo_depth(&app);
    jackdaw::ui_grouping::ungroup_selection(app.world_mut());
    settle(&mut app);
    assert!(
        app.world().get_entity(root).is_ok(),
        "ungroup deleted the scene root"
    );
    jackdaw::ui_grouping::group_selection(app.world_mut());
    settle(&mut app);
    assert_eq!(
        app.world().get::<ChildOf>(root).map(ChildOf::parent),
        None,
        "group buried the scene root under a container"
    );
    assert_eq!(undo_depth(&app), depth, "neither refusal recorded an entry");
    assert!(
        app.world()
            .resource::<jackdaw::status_bar::StatusNotice>()
            .is_active(),
        "a refusal says so in the status bar"
    );
}
