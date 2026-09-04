//! The arrow keys walk the outliner.
//!
//! The walk reads the keyboard itself rather than going through the
//! keymap, and it used to look for its rows under a marker nothing
//! carried: the outliner's list is a `TreeRoot`, and the walk asked for a
//! `TreeView`. It found no rows, so every arrow key did nothing in the
//! real panel while the unit tests, which spawned the marker the walk
//! asked for, passed.
//!
//! So the keys are pressed through the window's own stream, and the rows
//! are the outliner's own.

use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::{
    prelude::*,
    window::{PrimaryWindow, WindowResolution},
};
use jackdaw::hierarchy::{HierarchyShowAll, HierarchyTreeContainer};
use jackdaw::test_input::SyntheticInput;
use jackdaw_widgets::tree_view::{TreeFocused, TreeIndex, TreeNode, TreeNodeExpanded};

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
    assert!(
        app.world().resource::<SyntheticInput>().is_idle(),
        "the gesture drained",
    );
    settle(app);
}

/// An editor showing an outliner over two roots, the first with a child.
///
/// The walk is added to `Update` here because the editor registers it
/// behind `AppState::Editor`, and entering that state stands the whole
/// editor up around the panel under test. The container, its rows and the
/// key stream are the real ones either way, which is what the walk was
/// getting wrong.
fn outliner_app() -> (App, Entity, Vec<Entity>, Entity) {
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
    // The walk stands down while a text field has focus, and the project
    // screen this app starts on leaves one focused. Nothing is being typed
    // into here.
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
                ..default()
            },
            BackgroundColor(Color::NONE),
        ))
        .id();
    let roots: Vec<Entity> = ["Alpha", "Beta"]
        .into_iter()
        .map(|name| {
            let entity = app
                .world_mut()
                .spawn((Name::new(name), Node::default()))
                .id();
            jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
            entity
        })
        .collect();
    let child = app
        .world_mut()
        .spawn((Name::new("Leaf"), Node::default(), ChildOf(roots[0])))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), child);
    settle(&mut app);
    (app, panel, roots, child)
}

/// The source entity of the row the walk is standing on.
fn walking_on(app: &App) -> Option<Entity> {
    app.world()
        .resource::<TreeFocused>()
        .0
        .and_then(|row| app.world().get::<TreeNode>(row).map(|node| node.0))
}

/// Whether `source`'s row is open.
fn open(app: &App, panel: Entity, source: Entity) -> bool {
    let row = app
        .world()
        .resource::<TreeIndex>()
        .get(panel, source)
        .expect("the entity has a row in this panel");
    app.world()
        .get::<TreeNodeExpanded>(row)
        .is_some_and(|expanded| expanded.0)
}

#[test]
fn down_and_up_walk_the_rows() {
    let (mut app, _panel, roots, _child) = outliner_app();

    run(&mut app, "input.key key=ArrowDown");
    assert_eq!(
        walking_on(&app),
        Some(roots[0]),
        "the first press lands on the first row"
    );

    run(&mut app, "input.key key=ArrowDown");
    assert_eq!(walking_on(&app), Some(roots[1]));

    run(&mut app, "input.key key=ArrowUp");
    assert_eq!(walking_on(&app), Some(roots[0]));
}

#[test]
fn right_opens_a_branch_and_left_closes_it() {
    let (mut app, panel, roots, child) = outliner_app();

    run(&mut app, "input.key key=ArrowDown");
    assert_eq!(walking_on(&app), Some(roots[0]));
    assert!(!open(&app, panel, roots[0]), "a branch starts closed");

    run(&mut app, "input.key key=ArrowRight");
    assert!(open(&app, panel, roots[0]), "Right opens the branch");

    run(&mut app, "input.key key=ArrowDown");
    assert_eq!(
        walking_on(&app),
        Some(child),
        "the walk goes on into the branch it opened"
    );

    run(&mut app, "input.key key=ArrowLeft");
    assert_eq!(
        walking_on(&app),
        Some(roots[0]),
        "Left from inside a branch goes out to its parent"
    );

    run(&mut app, "input.key key=ArrowLeft");
    assert!(!open(&app, panel, roots[0]), "Left on the parent closes it");
}

/// Focus is not the same as being typed into. A button, a toolbar
/// control, whatever the pointer last landed on: the editor almost always
/// has focus somewhere, and the walk standing down for any of it is the
/// other half of why the arrow keys did nothing.
#[test]
fn focus_on_something_that_is_not_a_field_leaves_the_walk_alone() {
    use bevy::input_focus::{FocusCause, InputFocus};

    let (mut app, _panel, roots, _child) = outliner_app();
    let button = app.world_mut().spawn(Node::default()).id();
    app.world_mut()
        .resource_mut::<InputFocus>()
        .set(button, FocusCause::Pressed);
    settle(&mut app);

    run(&mut app, "input.key key=ArrowDown");
    assert_eq!(walking_on(&app), Some(roots[0]));
}

/// A field being typed into does take them, which is what the guard was
/// for: an arrow moves the caret, not the tree.
#[test]
fn a_field_being_typed_into_keeps_the_arrow_keys() {
    use bevy::input_focus::{FocusCause, InputFocus};
    use jackdaw_feathers::text_edit::EditorTextEdit;

    let (mut app, _panel, _roots, _child) = outliner_app();
    let field = app
        .world_mut()
        .spawn((Node::default(), EditorTextEdit))
        .id();
    app.world_mut()
        .resource_mut::<InputFocus>()
        .set(field, FocusCause::Pressed);
    settle(&mut app);

    run(&mut app, "input.key key=ArrowDown");
    assert_eq!(walking_on(&app), None, "the caret moved, not the tree");
}
