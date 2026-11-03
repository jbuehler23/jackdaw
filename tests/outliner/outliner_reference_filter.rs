//! The outliner's filter reads an entity reference the way an operator does:
//! `#house` shows the tagged nodes, `!` takes some away, and a plain word
//! still searches names.

use crate::util;

use bevy::prelude::*;
use jackdaw::layout::HierarchyFilter;
use jackdaw_widgets::tree_view::TreeNode;
use serde_json::json;

fn settle(app: &mut App) {
    for _ in 0..8 {
        app.update();
    }
}

fn scene_node(app: &mut App, name: &str, parent: Option<Entity>) -> Entity {
    let mut spawned = app.world_mut().spawn((
        Name::new(name.to_string()),
        Transform::default(),
        Visibility::default(),
    ));
    if let Some(parent) = parent {
        spawned.insert(ChildOf(parent));
    }
    let entity = spawned.id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    entity
}

/// Type `text` into the outliner's filter field.
fn filter_by(app: &mut App, text: &str) {
    let filter = app
        .world_mut()
        .query_filtered::<Entity, With<HierarchyFilter>>()
        .single(app.world())
        .expect("the outliner has a filter");
    let mut stack = vec![filter];
    while let Some(entity) = stack.pop() {
        if let Some(mut editable) = app.world_mut().get_mut::<bevy::text::EditableText>(entity) {
            jackdaw_feathers::text_edit::set_text_input_value(&mut editable, text.to_string());
            break;
        }
        stack.extend(
            app.world()
                .get::<Children>(entity)
                .into_iter()
                .flatten()
                .copied(),
        );
    }
    settle(app);
}

/// The scene nodes whose rows are showing.
fn shown(app: &mut App) -> Vec<Entity> {
    let mut sources: Vec<Entity> = app
        .world_mut()
        .query::<(&TreeNode, &Node)>()
        .iter(app.world())
        .filter(|(_, node)| node.display != Display::None)
        .map(|(row, _)| row.0)
        .collect();
    sources.sort();
    sources
}

fn tag(app: &mut App, entities: &[Entity], add: &str) {
    let ids: Vec<u64> = entities.iter().map(|entity| entity.to_bits()).collect();
    app.world_mut()
        .run_system_cached_with(
            jackdaw::remote::server::call_operator_handler,
            Some(json!({ "id": "entity.tag", "params": { "entities": ids, "add": add } })),
        )
        .expect("the handler ran")
        .expect("entity.tag answered");
    settle(app);
}

#[test]
fn the_outliner_filter_takes_tag_queries() {
    let mut app = util::editor_test_app();
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    settle(&mut app);

    let inn = scene_node(&mut app, "inn", None);
    let mill = scene_node(&mut app, "mill", None);
    let well = scene_node(&mut app, "well", None);
    settle(&mut app);
    tag(&mut app, &[inn, mill], "house");
    tag(&mut app, &[mill], "old");
    let mut all = vec![inn, mill, well];
    all.sort();
    assert_eq!(shown(&mut app), all, "every row shows before a filter");

    filter_by(&mut app, "#house");
    let mut houses = vec![inn, mill];
    houses.sort();
    assert_eq!(shown(&mut app), houses);

    filter_by(&mut app, "#house&!#old");
    assert_eq!(shown(&mut app), vec![inn]);

    filter_by(&mut app, "#nothing");
    assert!(
        shown(&mut app).is_empty(),
        "a reference matching nothing hides every row"
    );

    let numbered = scene_node(&mut app, "#1 tower", None);
    settle(&mut app);
    for partial in ["#", "#1 t"] {
        filter_by(&mut app, partial);
        assert_eq!(
            shown(&mut app),
            vec![numbered],
            "`{partial}` is no complete query yet, so it searches names"
        );
    }
    all.push(numbered);
    all.sort();

    filter_by(&mut app, "WEL");
    assert_eq!(
        shown(&mut app),
        vec![well],
        "a plain word still searches names"
    );

    filter_by(&mut app, "");
    assert_eq!(shown(&mut app), all);
}
