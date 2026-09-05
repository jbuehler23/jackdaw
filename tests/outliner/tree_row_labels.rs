//! What a row shows of a name too long for it, and which rows are cut at all.
//! The cut is made in the tree crate, over labels laid out to fill their row;
//! other trees carry the same marker on labels laid out to their own text, where
//! a cut narrows the label, which lowers the budget, which cuts it again.

use crate::util;

use bevy::prelude::*;
use jackdaw::hierarchy::HierarchyTreeContainer;
use jackdaw_feathers::tree_view::TreeRowLabelEllipsis;
use jackdaw_scene_types::UiSceneRoot;
use jackdaw_widgets::tree_view::{TreeNode, TreeRowContent, TreeRowLabel};

/// An open UI scene, named `name`, shown in an outliner `width` logical
/// pixels wide.
fn outliner_app(width: f32, name: &str) -> (App, Entity) {
    let mut app = util::editor_test_app();
    let world = app.world_mut();
    let root = world
        .spawn((
            Name::new(name.to_string()),
            UiSceneRoot::default(),
            Node::default(),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(world, root);
    world.spawn((
        HierarchyTreeContainer,
        Node {
            width: px(width),
            ..default()
        },
        Visibility::Inherited,
    ));
    for _ in 0..8 {
        app.update();
    }
    (app, root)
}

/// The label entity of `source`'s row.
fn row_label(app: &App, source: Entity) -> Entity {
    let world = app.world();
    let row = world
        .iter_entities()
        .find(|entity| {
            entity
                .get::<TreeNode>()
                .is_some_and(|node| node.0 == source)
        })
        .expect("the entity has an outliner row")
        .id();
    let content = world
        .get::<Children>(row)
        .expect("a row has children")
        .iter()
        .find(|&child| world.get::<TreeRowContent>(child).is_some())
        .expect("a row has content");
    world
        .get::<Children>(content)
        .expect("the content has children")
        .iter()
        .find(|&child| world.get::<TreeRowLabel>(child).is_some())
        .expect("the content has a label")
}

fn shown(app: &App, label: Entity) -> String {
    app.world()
        .get::<Text>(label)
        .expect("the label carries text")
        .0
        .clone()
}

/// A name the row has room for is shown whole. "Separator" in a 145 pixel panel
/// came out as "Separ...": the label is laid out to a 64 pixel floor there, and a
/// character guessed at 55% of the font size makes that eight characters of room
/// for a name drawn in 58 pixels.
#[test]
fn a_name_the_row_has_room_for_is_shown_whole() {
    let (app, root) = outliner_app(145.0, "Separator");

    let label = row_label(&app, root);
    assert_eq!(
        shown(&app, label),
        "Separator",
        "a name that fits the row is not cut",
    );
    assert!(
        app.world()
            .get::<jackdaw_feathers::tooltip::Tooltip>(label)
            .is_none(),
        "and carries no tooltip repeating what is on screen",
    );
}

/// A name the row has no room for is cut, and the whole of it is on the
/// tooltip.
#[test]
fn a_name_too_long_for_the_row_is_cut_and_kept_on_a_tooltip() {
    let name = "MainMenuBackgroundPanelContainer";
    let (app, root) = outliner_app(145.0, name);

    let label = row_label(&app, root);
    let shown = shown(&app, label);
    assert!(
        shown.ends_with("...") && shown.len() < name.len(),
        "a name with no room is cut: {shown:?}",
    );
    assert!(
        app.world()
            .get::<jackdaw_feathers::tooltip::Tooltip>(label)
            .is_some(),
        "and the whole of it is on a tooltip",
    );
}

/// A tree whose labels are laid out to their own text is left alone. This is the
/// shape `project_files.rs` spawns, where cutting spirals: the cut narrows the
/// label, the narrower label lowers the budget, and "assets" came to read "a".
#[test]
fn a_label_laid_out_to_its_own_text_is_never_cut() {
    let (mut app, _root) = outliner_app(145.0, "UiRoot");
    let panel = app
        .world_mut()
        .spawn(Node {
            width: px(120.0),
            ..default()
        })
        .id();
    let label = app
        .world_mut()
        .spawn((
            TreeRowLabel,
            Text::new("assets"),
            TextFont {
                font_size: jackdaw_feathers::tokens::TEXT_SIZE,
                ..default()
            },
            ChildOf(panel),
        ))
        .id();
    for _ in 0..8 {
        app.update();
    }

    assert!(
        app.world().get::<TreeRowLabelEllipsis>(label).is_none(),
        "the label never opted in to being cut",
    );
    assert_eq!(
        shown(&app, label),
        "assets",
        "so its name is shown whole however narrow it is laid out",
    );
}
