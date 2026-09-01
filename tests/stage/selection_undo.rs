//! What Ctrl+Z does to the selection.
//!
//! An undo that respawns the scene re-mints every entity, so a selection
//! held as entity ids would come back empty and leave the inspector, the
//! canvas outline and every gesture pointing at nothing. The snapshot
//! records the selection by document node instead.
//!
//! What is pinned here:
//!  * undoing a delete puts the deleted node back and selects it again,
//!    with the marker the outliner paints off;
//!  * undoing an edit that changed no entity leaves the selection alone;
//!  * a selection change on its own is not an edit, so it records nothing;
//!  * group and ungroup still undo back to what was selected when they
//!    were asked for.

use crate::util;

use bevy::prelude::*;
use jackdaw::boot_ops::run_op_clause_as_user;
use jackdaw::commands::CommandHistory;
use jackdaw::selection::{Selected, Selection};
use jackdaw_scene_types::UiSceneRoot;

use crate::util::OperatorResultExt as _;

const REFERENCE: UVec2 = UVec2::new(2400, 1200);

fn settle(app: &mut App) {
    for _ in 0..4 {
        app.update();
    }
}

#[track_caller]
fn run_finished(app: &mut App, clause: &str) {
    run_op_clause_as_user(app.world_mut(), clause)
        .unwrap_or_else(|err| panic!("{clause}: dispatch errored: {err}"))
        .assert_finished();
    settle(app);
}

fn scene(app: &mut App) -> (Entity, Vec<Entity>) {
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
    let children: Vec<Entity> = ["First", "Second"]
        .into_iter()
        .enumerate()
        .map(|(index, name)| {
            let entity = app
                .world_mut()
                .spawn((
                    Name::new(name),
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(100.0 + 300.0 * index as f32),
                        top: px(100.0),
                        width: px(200.0),
                        height: px(100.0),
                        ..default()
                    },
                    ChildOf(root),
                ))
                .id();
            jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
            entity
        })
        .collect();
    settle(app);
    (root, children)
}

/// The names of the selected entities, which survive a respawn where the
/// entity ids do not.
fn selected_names(app: &App) -> Vec<String> {
    app.world()
        .resource::<Selection>()
        .entities
        .iter()
        .filter_map(|&entity| app.world().get::<Name>(entity))
        .map(|name| name.as_str().to_string())
        .collect()
}

fn undo(app: &mut App) {
    run_finished(app, "history.undo");
}

#[test]
fn undoing_a_delete_selects_the_node_it_puts_back() {
    let mut app = util::editor_test_app();
    let (_root, children) = scene(&mut app);
    jackdaw::selection::select_only(app.world_mut(), children[1]);
    settle(&mut app);

    run_finished(&mut app, "entity.delete");
    assert!(
        selected_names(&app).is_empty(),
        "the deleted node is not selected, it is gone",
    );

    undo(&mut app);

    assert_eq!(
        selected_names(&app),
        vec!["Second"],
        "undo puts the node back and puts the selection back on it",
    );
    let restored = app.world().resource::<Selection>().entities[0];
    assert!(
        app.world().get::<Selected>(restored).is_some(),
        "the marker the outliner paints its row off comes back too",
    );
}

#[test]
fn undoing_a_drag_keeps_the_selection_it_was_dragged_with() {
    let mut app = util::editor_test_app();
    let (_root, children) = scene(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = children.clone();
    settle(&mut app);

    run_finished(&mut app, "ui.align_left");
    assert_eq!(selected_names(&app), vec!["First", "Second"]);

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    settle(&mut app);

    assert_eq!(
        selected_names(&app),
        vec!["First", "Second"],
        "an edit that moved no entity leaves the selection exactly as it was",
    );
}

#[test]
fn selecting_something_is_not_an_edit() {
    let mut app = util::editor_test_app();
    let (_root, children) = scene(&mut app);
    jackdaw::selection::select_only(app.world_mut(), children[0]);
    settle(&mut app);
    let depth = app.world().resource::<CommandHistory>().undo_stack.len();

    run_finished(&mut app, "entity.duplicate");
    jackdaw::selection::select_only(app.world_mut(), children[1]);
    settle(&mut app);
    run_finished(&mut app, "selection.clear");

    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len() - depth,
        1,
        "the duplicate is the one entry; the two selection changes are not edits",
    );
}

#[test]
fn undoing_a_group_selects_what_was_grouped() {
    let mut app = util::editor_test_app();
    let (_root, children) = scene(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = children.clone();
    settle(&mut app);

    run_finished(&mut app, "ui.group_into");
    assert_eq!(
        selected_names(&app),
        vec!["Group"],
        "the group selects the container it made",
    );

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    settle(&mut app);

    let mut names = selected_names(&app);
    names.sort();
    assert_eq!(
        names,
        vec!["First", "Second"],
        "undo goes back to the selection the group was asked for",
    );
}
