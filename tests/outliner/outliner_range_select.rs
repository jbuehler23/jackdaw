//! Shift-click in the outliner selects the run of rows between the anchor and the
//! row that was clicked.

use crate::util;

use bevy::prelude::*;
use jackdaw::hierarchy::{HierarchyShowAll, HierarchyTreeContainer};
use jackdaw::selection::Selection;
use jackdaw_widgets::tree_view::{TreeNode, TreeNodeExpanded, TreeRowClicked};

fn outliner(app: &mut App) -> Entity {
    app.world_mut().insert_resource(HierarchyShowAll(true));
    let panel = app
        .world_mut()
        .spawn((
            HierarchyTreeContainer,
            Node::default(),
            Visibility::Inherited,
        ))
        .id();
    app.update();
    panel
}

fn authored(app: &mut App, name: &str, parent: Option<Entity>) -> Entity {
    let world = app.world_mut();
    let mut entity = world.spawn((Name::new(name.to_string()), Node::default()));
    if let Some(parent) = parent {
        entity.insert(ChildOf(parent));
    }
    let entity = entity.id();
    jackdaw::scene_io::register_entity_in_ast(world, entity);
    entity
}

/// A root with five named children, the shape a range is stated over.
fn five_rows(app: &mut App) -> (Entity, Vec<Entity>) {
    outliner(app);
    let root = authored(app, "Root", None);
    let children: Vec<Entity> = ["One", "Two", "Three", "Four", "Five"]
        .into_iter()
        .map(|name| authored(app, name, Some(root)))
        .collect();
    app.update();
    app.update();
    (root, children)
}

fn row_of(app: &mut App, source: Entity) -> Entity {
    let mut rows = app.world_mut().query::<(Entity, &TreeNode)>();
    rows.iter(app.world())
        .find(|(_, node)| node.0 == source)
        .map(|(row, _)| row)
        .unwrap_or_else(|| panic!("{source} has no outliner row"))
}

/// Open a row, the state its chevron leaves behind, so its children get rows.
fn expand(app: &mut App, source: Entity) {
    let row = row_of(app, source);
    app.world_mut()
        .entity_mut(row)
        .insert(TreeNodeExpanded(true));
    app.update();
    app.update();
}

fn click(app: &mut App, source: Entity, modifiers: &[KeyCode]) {
    let row = row_of(app, source);
    {
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.reset_all();
        for &key in modifiers {
            keys.press(key);
        }
    }
    app.world_mut().trigger(TreeRowClicked {
        entity: row,
        source_entity: source,
    });
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .reset_all();
    app.update();
}

fn names(app: &App, entities: &[Entity]) -> Vec<String> {
    entities
        .iter()
        .map(|&entity| {
            app.world()
                .get::<Name>(entity)
                .map(|name| name.as_str().to_string())
                .unwrap_or_else(|| format!("{entity}"))
        })
        .collect()
}

fn selected(app: &App) -> Vec<String> {
    names(app, &app.world().resource::<Selection>().entities)
}

#[test]
fn shift_click_below_the_anchor_selects_the_run_down_to_it() {
    let mut app = util::editor_test_app();
    let (root, children) = five_rows(&mut app);
    expand(&mut app, root);

    click(&mut app, children[1], &[]);
    click(&mut app, children[3], &[KeyCode::ShiftLeft]);

    assert_eq!(selected(&app), vec!["Two", "Three", "Four"]);
    assert_eq!(
        app.world().resource::<Selection>().primary(),
        Some(children[3]),
        "the row the user pointed at is what the inspector follows",
    );
}

#[test]
fn shift_click_above_the_anchor_selects_the_run_up_to_it() {
    let mut app = util::editor_test_app();
    let (root, children) = five_rows(&mut app);
    expand(&mut app, root);

    click(&mut app, children[3], &[]);
    click(&mut app, children[1], &[KeyCode::ShiftLeft]);

    let selection = app.world().resource::<Selection>();
    let mut swept = names(&app, &selection.entities);
    assert_eq!(selection.primary(), Some(children[1]));
    swept.sort();
    assert_eq!(swept, vec!["Four", "Three", "Two"]);
}

#[test]
fn the_range_covers_only_the_rows_on_screen() {
    let mut app = util::editor_test_app();
    outliner(&mut app);
    let root = authored(&mut app, "Root", None);
    let first = authored(&mut app, "First", Some(root));
    let buried = authored(&mut app, "Buried", Some(first));
    let last = authored(&mut app, "Last", Some(root));
    app.update();
    app.update();
    expand(&mut app, root);

    click(&mut app, first, &[]);
    click(&mut app, last, &[KeyCode::ShiftLeft]);

    assert_eq!(
        selected(&app),
        vec!["First", "Last"],
        "a collapsed row's child is not on screen to be swept",
    );
    assert!(
        !app.world().resource::<Selection>().is_selected(buried),
        "the closed subtree stays out of the range",
    );
}

#[test]
fn a_second_shift_click_sweeps_from_the_same_anchor() {
    let mut app = util::editor_test_app();
    let (root, children) = five_rows(&mut app);
    expand(&mut app, root);

    click(&mut app, children[0], &[]);
    click(&mut app, children[3], &[KeyCode::ShiftLeft]);
    click(&mut app, children[1], &[KeyCode::ShiftLeft]);

    assert_eq!(
        selected(&app),
        vec!["One", "Two"],
        "the anchor held, so the second sweep restates the range rather than growing it",
    );
}

#[test]
fn ctrl_click_still_toggles_one_row_and_moves_the_anchor() {
    let mut app = util::editor_test_app();
    let (root, children) = five_rows(&mut app);
    expand(&mut app, root);

    click(&mut app, children[0], &[]);
    click(&mut app, children[2], &[KeyCode::ControlLeft]);
    assert_eq!(selected(&app), vec!["One", "Three"]);

    click(&mut app, children[2], &[KeyCode::ControlLeft]);
    assert_eq!(
        selected(&app),
        vec!["One"],
        "Ctrl on a selected row drops it"
    );

    click(&mut app, children[4], &[KeyCode::ControlLeft]);
    click(&mut app, children[3], &[KeyCode::ShiftLeft]);
    assert_eq!(
        selected(&app),
        vec!["Five", "Four"],
        "the Ctrl-click left the anchor on its own row",
    );
}
