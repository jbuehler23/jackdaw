//! The entity clipboard: `entity.copy`, `entity.cut` and `entity.paste`.
//!
//! What is pinned here:
//!  * a paste lands as the sibling straight after the primary selection, with
//!    a name no other entity holds, and becomes the selection;
//!  * nothing selected pastes under the open UI scene's root;
//!  * the clipboard is scene text, so a copy survives a reload and pastes
//!    into a different scene in a different tab;
//!  * a cut is one history entry and undo puts the subtree back in place;
//!  * a paste is one history entry that undoes and redoes to the same place;
//!  * the timeline keeps `Ctrl+C` / `Ctrl+V` while it is the focused window.
//!
//! The tests take the OS clipboard out of the app: it is one object shared by
//! every process on the machine (and by every test thread), so leaving it in
//! would make what one test copies visible to another. What is left is the
//! editor's own [`jackdaw::entity_ops::EntityClipboard`], the same fallback a
//! run with no clipboard at all uses.

use bevy::prelude::*;
use jackdaw::boot_ops::run_op_clause;
use jackdaw::commands::CommandHistory;
use jackdaw::entity_ops::{EntityClipboard, SystemClipboard};
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_bsn::SceneBsnAst;

mod util;

fn clipboard_app() -> App {
    let mut app = util::editor_test_app();
    app.world_mut().remove_resource::<SystemClipboard>();
    app
}

#[track_caller]
fn run_finished(app: &mut App, clause: &str) {
    let result = run_op_clause(app.world_mut(), clause)
        .unwrap_or_else(|err| panic!("{clause}: dispatch errored: {err}"));
    app.update();
    assert_eq!(
        result,
        OperatorResult::Finished,
        "{clause} reported {result:?}"
    );
}

fn names_under(world: &World, parent: Entity) -> Vec<String> {
    world
        .get::<Children>(parent)
        .map(|children| {
            children
                .iter()
                .filter_map(|child| world.get::<Name>(child))
                .map(|name| name.as_str().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn scene_names(app: &mut App) -> Vec<String> {
    let mut names: Vec<String> = app
        .world_mut()
        .query_filtered::<&Name, With<Node>>()
        .iter(app.world())
        .map(|name| name.as_str().to_string())
        .collect();
    names.sort();
    names
}

fn select(app: &mut App, entity: Entity) {
    jackdaw::selection::select_only(app.world_mut(), entity);
    app.update();
}

fn undo_depth(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

/// A UI scene whose root holds two named children.
fn scene_with_two_children(app: &mut App) -> (Entity, Vec<Entity>) {
    let world = app.world_mut();
    let root = world
        .spawn((
            Name::new("UiRoot"),
            jackdaw_scene_types::UiSceneRoot::default(),
            Node::default(),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(world, root);
    let children: Vec<Entity> = ["Alpha", "Omega"]
        .into_iter()
        .map(|name| {
            let child = world
                .spawn((Name::new(name), Node::default(), ChildOf(root)))
                .id();
            jackdaw::scene_io::register_entity_in_ast(world, child);
            child
        })
        .collect();
    app.update();
    (root, children)
}

#[test]
fn a_paste_lands_beside_the_selection_under_a_free_name() {
    let mut app = clipboard_app();
    let (root, children) = scene_with_two_children(&mut app);
    select(&mut app, children[0]);

    run_finished(&mut app, "entity.copy");
    assert!(
        !app.world().resource::<EntityClipboard>().text.is_empty(),
        "the copy put scene text on the clipboard"
    );

    let before = undo_depth(&app);
    run_finished(&mut app, "entity.paste");

    assert_eq!(
        names_under(app.world(), root),
        vec!["Alpha", "Alpha 2", "Omega"],
        "the paste is the sibling straight after the one that was copied"
    );
    assert_eq!(
        undo_depth(&app) - before,
        1,
        "one paste is one history entry"
    );

    let selected = app.world().resource::<Selection>().entities.clone();
    assert_eq!(selected.len(), 1, "the paste is what is selected now");
    assert_eq!(
        app.world()
            .get::<Name>(selected[0])
            .map(|name| name.as_str().to_string()),
        Some("Alpha 2".to_string())
    );
    assert!(
        app.world()
            .resource::<SceneBsnAst>()
            .ast_for(selected[0])
            .is_some(),
        "the paste is in the document, so a save keeps it"
    );
}

#[test]
fn a_paste_with_nothing_selected_lands_under_the_scene_root() {
    let mut app = clipboard_app();
    let (root, children) = scene_with_two_children(&mut app);
    select(&mut app, children[1]);
    run_finished(&mut app, "entity.copy");

    jackdaw::selection::clear_selection_in_world(app.world_mut());
    app.update();
    run_finished(&mut app, "entity.paste");

    assert_eq!(
        names_under(app.world(), root),
        vec!["Alpha", "Omega", "Omega 2"],
        "with nothing selected the paste goes to the end of the scene root"
    );
}

#[test]
fn a_paste_undoes_and_redoes_in_the_same_place() {
    let mut app = clipboard_app();
    let (root, children) = scene_with_two_children(&mut app);
    select(&mut app, children[0]);
    run_finished(&mut app, "entity.copy");
    run_finished(&mut app, "entity.paste");

    run_finished(&mut app, "history.undo");
    assert_eq!(
        names_under(app.world(), root),
        vec!["Alpha", "Omega"],
        "undo took the paste back out"
    );

    run_finished(&mut app, "history.redo");
    assert_eq!(
        names_under(app.world(), root),
        vec!["Alpha", "Alpha 2", "Omega"],
        "redo put it back beside the selection, not at the end"
    );
}

#[test]
fn a_cut_is_one_entry_and_undo_puts_the_subtree_back_in_place() {
    let mut app = clipboard_app();
    let (root, children) = scene_with_two_children(&mut app);
    select(&mut app, children[0]);
    let before = undo_depth(&app);

    run_finished(&mut app, "entity.cut");

    assert_eq!(
        names_under(app.world(), root),
        vec!["Omega"],
        "the cut took the subtree out of the scene"
    );
    assert_eq!(undo_depth(&app) - before, 1, "one cut is one history entry");
    assert!(
        app.world()
            .resource::<EntityClipboard>()
            .text
            .contains("Alpha"),
        "the cut left the subtree on the clipboard"
    );

    run_finished(&mut app, "history.undo");
    assert_eq!(
        names_under(app.world(), root),
        vec!["Alpha", "Omega"],
        "undo of a cut puts the subtree back where it was"
    );
}

#[test]
fn a_copy_survives_a_reload_and_pastes_into_the_reopened_scene() {
    let mut app = clipboard_app();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("clipboard.bsn");
    run_finished(
        &mut app,
        &format!("scene.new ui=true path={}", path.display()),
    );

    let target = app
        .world_mut()
        .spawn((Name::new("Kept"), Node::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), target);
    app.update();
    select(&mut app, target);
    run_finished(&mut app, "entity.copy");
    run_finished(&mut app, "scene.save");
    run_finished(&mut app, &format!("scene.open path={}", path.display()));

    run_finished(&mut app, "entity.paste");

    let names = scene_names(&mut app);
    assert!(
        names.iter().any(|name| name == "Kept 2"),
        "the clipboard outlived the reload; names are {names:?}"
    );
}

#[test]
fn a_subtree_copied_in_one_tab_pastes_into_another() {
    let mut app = clipboard_app();
    run_finished(&mut app, "scene.new ui=true");
    let source = app
        .world_mut()
        .spawn((Name::new("Travelled"), Node::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), source);
    app.update();
    select(&mut app, source);
    run_finished(&mut app, "entity.copy");

    // A second tab, with its own document.
    run_finished(&mut app, "scene.new ui=true");
    assert!(
        !scene_names(&mut app).iter().any(|name| name == "Travelled"),
        "the new tab starts without the copied entity"
    );

    run_finished(&mut app, "entity.paste");

    let names = scene_names(&mut app);
    assert!(
        names.iter().any(|name| name == "Travelled"),
        "the copy crossed into the other tab's scene; names are {names:?}"
    );
    let pasted = app
        .world_mut()
        .query::<(Entity, &Name)>()
        .iter(app.world())
        .find(|(_, name)| name.as_str() == "Travelled")
        .map(|(entity, _)| entity)
        .expect("the pasted entity");
    assert!(
        app.world()
            .resource::<SceneBsnAst>()
            .ast_for(pasted)
            .is_some(),
        "the paste is in the second tab's document"
    );
}

#[test]
fn the_timeline_keeps_the_chord_while_it_is_the_focused_window() {
    let mut app = clipboard_app();
    let (_, children) = scene_with_two_children(&mut app);
    select(&mut app, children[0]);

    for id in ["entity.copy", "entity.cut", "entity.paste"] {
        assert!(
            available(&mut app, id),
            "{id} answers the chord with no timeline open"
        );
    }

    let mut tree = jackdaw_panels::tree::DockTree::new();
    let leaf = tree.insert(jackdaw_panels::tree::DockNode::Leaf(
        jackdaw_panels::tree::DockLeaf::new("root", jackdaw_panels::area::DockAreaStyle::TabBar)
            .with_windows(vec!["jackdaw.timeline".into()]),
    ));
    tree.root = Some(leaf);
    *app.world_mut()
        .resource_mut::<jackdaw_panels::tree::DockTree>() = tree;
    app.update();

    for id in ["entity.copy", "entity.cut", "entity.paste"] {
        assert!(
            !available(&mut app, id),
            "{id} should stand down while the timeline is the focused window"
        );
    }
}

fn available(app: &mut App, id: &'static str) -> bool {
    app.world_mut()
        .operator(id)
        .is_available()
        .unwrap_or_else(|err| panic!("{id}: is_available errored: {err}"))
}

#[test]
fn the_paste_gets_a_row_in_the_outliner() {
    let mut app = clipboard_app();
    let panel = app
        .world_mut()
        .spawn((
            jackdaw::hierarchy::HierarchyTreeContainer,
            Node::default(),
            Visibility::Inherited,
        ))
        .id();
    app.update();

    let (root, children) = scene_with_two_children(&mut app);
    let row = app
        .world()
        .resource::<jackdaw_widgets::tree_view::TreeIndex>()
        .get(panel, root)
        .expect("the root has a row");
    app.world_mut()
        .entity_mut(row)
        .insert(jackdaw_widgets::tree_view::TreeNodeExpanded(true));
    app.update();
    app.update();

    // A widget is a subtree, not one entity: the button's caption is
    // authored too, and a paste has to give the root a row and not the part.
    let caption = app
        .world_mut()
        .spawn((Name::new("Caption"), Node::default(), ChildOf(children[0])))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), caption);
    app.update();

    select(&mut app, children[0]);
    run_finished(&mut app, "entity.copy");
    run_finished(&mut app, "entity.paste");
    app.update();
    app.update();

    let pasted = app.world().resource::<Selection>().entities[0];
    assert!(
        app.world()
            .resource::<jackdaw_widgets::tree_view::TreeIndex>()
            .get(panel, pasted)
            .is_some(),
        "the pasted entity has no Outliner row"
    );
}
