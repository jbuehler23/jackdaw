//! A scrolling outliner builds the rows near its view and keeps the rest as
//! frames one row high. Selecting, walking with the keys, renaming, dropping and
//! filtering all still reach rows that were frames when they started.

use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::{
    prelude::*,
    ui::{ComputedNode, ScrollPosition, UiGlobalTransform, UiScale},
    window::{PrimaryWindow, WindowResolution},
};
use jackdaw::hierarchy::{HierarchyShowAll, HierarchyTreeContainer};
use jackdaw::layout::HierarchyFilter;
use jackdaw::test_input::SyntheticInput;
use jackdaw_widgets::tree_view::{
    TreeFocused, TreeIndex, TreeNode, TreeNodeExpanded, TreeRowContent, TreeRowDropped,
    TreeRowInlineRename, TreeRowVisibilityToggle, TreeRowVisibilityToggled,
};

const ROOTS: usize = 400;

fn settle(app: &mut App) {
    for _ in 0..8 {
        app.update();
    }
}

fn run(app: &mut App, clause: &str) {
    jackdaw::boot_ops::run_op_clause(app.world_mut(), clause)
        .expect("the clause dispatches")
        .assert_finished();
    for _ in 0..600 {
        app.update();
        if app.world().resource::<SyntheticInput>().is_idle() {
            break;
        }
    }
    settle(app);
}

/// An outliner panel that scrolls, over `ROOTS` named top-level nodes, the
/// last of which holds a child. Returns the app, the panel and the roots in
/// the order the outliner lists them.
fn long_outliner() -> (App, Entity, Vec<Entity>) {
    let mut app = util::editor_test_app();
    {
        let mut windows = app
            .world_mut()
            .query_filtered::<&mut Window, With<PrimaryWindow>>();
        let mut window = windows
            .single_mut(app.world_mut())
            .expect("headless apps still have a primary window");
        window.resolution = WindowResolution::new(1600, 1000);
    }
    app.add_systems(
        Update,
        jackdaw_feathers::tree_view::tree_keyboard_navigation,
    );
    app.world_mut()
        .resource_mut::<bevy::input_focus::InputFocus>()
        .clear();
    app.world_mut().insert_resource(HierarchyShowAll(true));
    let panel = app
        .world_mut()
        .spawn((
            HierarchyTreeContainer,
            Node {
                width: px(320),
                height: px(600),
                flex_direction: FlexDirection::Column,
                overflow: Overflow::scroll_y(),
                ..default()
            },
            ScrollPosition::default(),
            BackgroundColor(Color::NONE),
        ))
        .id();
    let roots = spawn_roots(&mut app);
    settle(&mut app);
    (app, panel, roots)
}

/// `ROOTS` named top-level nodes, the last of which holds a child, in the
/// order the outliner lists them.
fn spawn_roots(app: &mut App) -> Vec<Entity> {
    let roots: Vec<Entity> = (0..ROOTS)
        .map(|index| {
            let entity = app
                .world_mut()
                .spawn((Name::new(format!("Node {index:04}")), Node::default()))
                .id();
            jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
            entity
        })
        .collect();
    let child = app
        .world_mut()
        .spawn((
            Name::new("Deep Child"),
            Node::default(),
            ChildOf(roots[ROOTS - 1]),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), child);
    roots
}

fn row(app: &App, panel: Entity, source: Entity) -> Option<Entity> {
    app.world().resource::<TreeIndex>().get(panel, source)
}

fn built(app: &App, panel: Entity, source: Entity) -> bool {
    row(app, panel, source).is_some_and(|row| {
        app.world().get::<Children>(row).is_some_and(|children| {
            children
                .iter()
                .any(|child| app.world().get::<TreeRowContent>(child).is_some())
        })
    })
}

fn built_count(app: &mut App) -> usize {
    app.world_mut()
        .query_filtered::<(), With<TreeRowContent>>()
        .iter(app.world())
        .count()
}

/// The glyph on the eye toggle of `source`'s row.
fn eye_glyph(app: &App, panel: Entity, source: Entity) -> String {
    let row = row(app, panel, source).expect("the node has a row");
    let child_with = |parent: Entity, is_wanted: &dyn Fn(Entity) -> bool| {
        app.world()
            .get::<Children>(parent)
            .and_then(|children| children.iter().find(|&child| is_wanted(child)))
    };
    let line = child_with(row, &|part| {
        app.world().get::<TreeRowContent>(part).is_some()
    })
    .expect("the row is built");
    let toggle = child_with(line, &|part| {
        app.world().get::<TreeRowVisibilityToggle>(part).is_some()
    })
    .expect("the row has an eye");
    let glyph = child_with(toggle, &|_| true).expect("the eye has a glyph");
    app.world()
        .get::<Text>(glyph)
        .map(|text| text.0.clone())
        .expect("the glyph is text")
}

/// Where `entity` is drawn, in the window logical pixels `input.pointer`
/// takes.
fn centre_of(app: &App, entity: Entity) -> Vec2 {
    let transform = app
        .world()
        .get::<UiGlobalTransform>(entity)
        .expect("the node is placed");
    let computed = app
        .world()
        .get::<ComputedNode>(entity)
        .expect("the node is laid out");
    transform.translation * computed.inverse_scale_factor() * app.world().resource::<UiScale>().0
}

fn scroll(app: &App, panel: Entity) -> f32 {
    app.world()
        .get::<ScrollPosition>(panel)
        .map_or(0.0, |scroll| scroll.y)
}

#[test]
fn only_the_rows_near_the_view_are_built() {
    let (mut app, panel, roots) = long_outliner();

    for &root in &roots {
        assert!(row(&app, panel, root).is_some(), "every node has a row");
    }
    assert!(built(&app, panel, roots[0]), "the top of the list is built");
    assert!(
        !built(&app, panel, roots[ROOTS - 1]),
        "the bottom of the list is still a frame"
    );
    let count = built_count(&mut app);
    assert!(
        count < ROOTS / 4,
        "{count} of {ROOTS} rows were built for a view of about 30"
    );
}

#[test]
fn a_row_out_of_view_takes_the_room_of_a_built_row() {
    let (app, panel, roots) = long_outliner();
    let first = row(&app, panel, roots[0]).expect("the first node has a row");
    let line = app
        .world()
        .get::<Children>(first)
        .and_then(|parts| {
            parts
                .iter()
                .find(|&part| app.world().get::<TreeRowContent>(part).is_some())
        })
        .expect("the first row is built");
    let line_height = app
        .world()
        .get::<ComputedNode>(line)
        .map(|computed| computed.size.y * computed.inverse_scale_factor())
        .expect("the label line is laid out");
    let first_height = app
        .world()
        .get::<ComputedNode>(first)
        .map(|computed| computed.size.y * computed.inverse_scale_factor())
        .expect("the first row is laid out");
    assert_eq!(first_height, line_height, "a built row fits its label line");

    let far = row(&app, panel, roots[ROOTS - 2]).expect("every node has a row");
    assert!(!built(&app, panel, roots[ROOTS - 2]));
    assert!(
        app.world().get::<Node>(far).is_none(),
        "a row far out of view is not laid out"
    );
    let content = app
        .world()
        .get::<ComputedNode>(panel)
        .map(|computed| computed.content_size.y * computed.inverse_scale_factor())
        .expect("the panel is laid out");
    let rows = app
        .world()
        .get::<Children>(panel)
        .map(|children| {
            children
                .iter()
                .filter(|&child| app.world().get::<TreeNode>(child).is_some())
                .count()
        })
        .expect("the panel holds rows");
    let every_row = rows as f32 * line_height;
    assert!(
        (content - every_row).abs() < 1.0,
        "the list is {content} tall where {rows} rows take {every_row}"
    );
}

#[test]
fn scrolling_builds_the_rows_that_come_into_view() {
    let (mut app, panel, roots) = long_outliner();

    app.world_mut()
        .get_mut::<ScrollPosition>(panel)
        .expect("the panel scrolls")
        .y = 200.0 * jackdaw_feathers::tree_view::tree_row_height();
    settle(&mut app);

    assert!(
        built(&app, panel, roots[205]),
        "a row scrolled into view is built"
    );
    assert!(
        !built(&app, panel, roots[0]),
        "a row scrolled far out of view goes back to a frame"
    );
}

#[test]
fn rows_far_from_the_view_leave_layout_and_return_when_scrolled_to() {
    let (mut app, panel, roots) = long_outliner();
    let laid_out = |app: &App, source: Entity| {
        let row = row(app, panel, source).expect("every node has a row");
        app.world().get::<Node>(row).is_some()
    };
    let scroll_to = |app: &mut App, y: f32| {
        app.world_mut()
            .get_mut::<ScrollPosition>(panel)
            .expect("the panel scrolls")
            .y = y;
        settle(app);
    };
    let row_height = jackdaw_feathers::tree_view::tree_row_height();
    assert!(
        !laid_out(&app, roots[300]),
        "a row far below is out of layout"
    );

    scroll_to(&mut app, 300.0 * row_height);
    assert!(
        built(&app, panel, roots[305]),
        "the rows scrolled to are built"
    );
    assert!(
        !laid_out(&app, roots[5]),
        "a row far above is out of layout"
    );
    let scrolled = app
        .world()
        .get::<ComputedNode>(panel)
        .map(|computed| computed.scroll_position.y * computed.inverse_scale_factor())
        .expect("the panel is laid out");
    assert!(
        (scrolled - 300.0 * row_height).abs() < 1.0,
        "the list scrolled to {scrolled} where every row laid out would let it reach {}",
        300.0 * row_height
    );

    scroll_to(&mut app, 0.0);
    assert!(
        built(&app, panel, roots[5]),
        "the rows scrolled back to are built"
    );
    assert!(!laid_out(&app, roots[300]));
}

#[test]
fn opening_a_long_branch_builds_only_the_child_rows_near_the_view() {
    let (mut app, panel, roots) = long_outliner();
    let branch = roots[0];
    let leaves: Vec<Entity> = (0..ROOTS)
        .map(|index| {
            let entity = app
                .world_mut()
                .spawn((
                    Name::new(format!("Leaf {index:04}")),
                    Node::default(),
                    ChildOf(branch),
                ))
                .id();
            jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
            entity
        })
        .collect();
    settle(&mut app);
    let branch_row = row(&app, panel, branch).expect("the branch has a row");

    app.world_mut()
        .entity_mut(branch_row)
        .insert(TreeNodeExpanded(true));
    settle(&mut app);

    for &leaf in &leaves {
        assert!(row(&app, panel, leaf).is_some(), "every child has a row");
    }
    assert!(built(&app, panel, leaves[0]), "the first child is built");
    assert!(
        !built(&app, panel, leaves[ROOTS - 1]),
        "the last child is still a frame"
    );
}

#[test]
fn a_hidden_node_keeps_its_closed_eye_when_its_row_is_built_again() {
    let (mut app, panel, roots) = long_outliner();
    let first = row(&app, panel, roots[0]).expect("the first node has a row");
    app.world_mut().trigger(TreeRowVisibilityToggled {
        entity: first,
        source_entity: roots[0],
    });
    settle(&mut app);
    let closed_eye = eye_glyph(&app, panel, roots[0]);

    for rows_down in [300.0, 0.0] {
        app.world_mut()
            .get_mut::<ScrollPosition>(panel)
            .expect("the panel scrolls")
            .y = rows_down * jackdaw_feathers::tree_view::tree_row_height();
        settle(&mut app);
    }

    assert_eq!(
        app.world().get::<Visibility>(roots[0]),
        Some(&Visibility::Hidden)
    );
    assert_eq!(eye_glyph(&app, panel, roots[0]), closed_eye);
    assert_ne!(eye_glyph(&app, panel, roots[1]), closed_eye);
}

#[test]
fn selecting_a_node_far_down_builds_its_row_and_scrolls_to_it() {
    let (mut app, panel, roots) = long_outliner();
    let child = app
        .world()
        .get::<Children>(roots[ROOTS - 1])
        .and_then(|children| children.first().copied())
        .expect("the last root holds a child");

    jackdaw::selection::select_only(app.world_mut(), child);
    for _ in 0..24 {
        app.update();
    }

    assert!(
        built(&app, panel, child),
        "the selected node's row is built"
    );
    assert!(
        scroll(&app, panel) > 100.0 * jackdaw_feathers::tree_view::tree_row_height(),
        "the list scrolled down to it, and sits at {}",
        scroll(&app, panel)
    );
}

#[test]
fn the_arrow_keys_walk_past_the_rows_that_were_built() {
    let (mut app, _panel, roots) = long_outliner();

    run(&mut app, "input.key key=ArrowDown");
    for _ in 0..59 {
        run(&mut app, "input.key key=ArrowDown");
    }

    let walking_on = app
        .world()
        .resource::<TreeFocused>()
        .0
        .and_then(|row| app.world().get::<TreeNode>(row).map(|node| node.0));
    assert_eq!(
        walking_on,
        Some(roots[59]),
        "sixty presses land on the sixtieth row, past the first view"
    );
}

#[test]
fn f2_renames_a_selection_far_down_the_list() {
    let (mut app, _panel, roots) = long_outliner();
    jackdaw::selection::select_only(app.world_mut(), roots[ROOTS - 2]);
    for _ in 0..24 {
        app.update();
    }

    run(&mut app, "input.key key=F2");

    assert!(
        app.world_mut()
            .query_filtered::<(), With<TreeRowInlineRename>>()
            .iter(app.world())
            .next()
            .is_some(),
        "F2 opens the entry on a row that started as a frame"
    );
}

#[test]
fn a_row_dropped_on_another_far_down_moves_under_it() {
    let (mut app, panel, roots) = long_outliner();
    let (dragged, target) = (roots[300], roots[310]);
    app.world_mut()
        .get_mut::<ScrollPosition>(panel)
        .expect("the panel scrolls")
        .y = 295.0 * jackdaw_feathers::tree_view::tree_row_height();
    settle(&mut app);

    jackdaw::selection::select_only(app.world_mut(), dragged);
    settle(&mut app);
    app.world_mut().trigger(TreeRowDropped {
        entity: dragged,
        dragged_source: dragged,
        target_source: target,
    });
    settle(&mut app);

    assert_eq!(
        app.world().get::<ChildOf>(dragged).map(ChildOf::parent),
        Some(target)
    );
    let moved = row(&app, panel, dragged).expect("the moved node keeps a row");
    let parent_row = app
        .world()
        .get::<ChildOf>(moved)
        .and_then(|children| app.world().get::<ChildOf>(children.parent()))
        .map(ChildOf::parent);
    assert_eq!(parent_row, row(&app, panel, target));
}

#[test]
fn the_filter_finds_a_node_far_down_and_builds_its_row() {
    let mut app = util::editor_test_app();
    app.world_mut().insert_resource(HierarchyShowAll(true));
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    settle(&mut app);
    let roots = spawn_roots(&mut app);
    settle(&mut app);
    let panel = app
        .world_mut()
        .query_filtered::<Entity, With<HierarchyTreeContainer>>()
        .single(app.world())
        .expect("the editor has one outliner");
    assert!(
        !built(&app, panel, roots[390]),
        "the node starts far below the view"
    );

    let filter = app
        .world_mut()
        .query_filtered::<Entity, With<HierarchyFilter>>()
        .single(app.world())
        .expect("the outliner has a filter");
    let at = centre_of(&app, filter);
    run(
        &mut app,
        &format!("input.pointer x={} y={} action=click", at.x, at.y),
    );
    run(&mut app, "input.text text=node%200390");

    assert!(
        built(&app, panel, roots[390]),
        "the match is shown and built"
    );
    let shown_rows = app
        .world_mut()
        .query::<(&TreeNode, &Node)>()
        .iter(app.world())
        .filter(|(_, node)| node.display != Display::None)
        .count();
    assert_eq!(shown_rows, 1, "the filter shows only the match");
}
