//! The selected node's tags as chips in the inspector header: one chip per tag
//! that takes it off, and an add field that puts one on through `entity.tag`.

use crate::util;

use bevy::prelude::*;
use jackdaw::commands::CommandHistory;
use jackdaw::tags::{InspectorTagsMount, TagAddField};
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_feathers::text_edit::TextEditCommitEvent;
use jackdaw_scene_types::{PropertyValue, Tags};

fn settle(app: &mut App) {
    for _ in 0..6 {
        app.update();
    }
}

fn mount(app: &mut App) -> Entity {
    app.world_mut()
        .query_filtered::<Entity, With<InspectorTagsMount>>()
        .iter(app.world())
        .min()
        .expect("the inspector header holds the tag chips")
}

/// The tags the chips under the mount take off, in order.
fn chip_tags(app: &mut App) -> Vec<String> {
    let mount = mount(app);
    app.world()
        .get::<Children>(mount)
        .into_iter()
        .flatten()
        .filter_map(|&child| app.world().get::<ButtonOperatorCall>(child))
        .filter(|call| call.id == "entity.tag")
        .filter_map(|call| {
            call.params
                .iter()
                .find(|(key, _)| key == "remove")
                .and_then(|(_, value)| match value {
                    PropertyValue::String(tag) => Some(tag.to_string()),
                    _ => None,
                })
        })
        .collect()
}

fn mount_shown(app: &mut App) -> bool {
    let mount = mount(app);
    app.world()
        .get::<Node>(mount)
        .is_some_and(|node| node.display != Display::None)
}

#[test]
fn tags_show_as_chips_and_the_add_field_tags_the_selection() {
    let mut app = util::editor_test_app();
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    settle(&mut app);
    assert!(!mount_shown(&mut app), "nothing selected, no chips");

    let roof = app
        .world_mut()
        .spawn((
            Name::new("roof"),
            Transform::default(),
            Tags(vec!["roof".into()]),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), roof);
    jackdaw::selection::select_only(app.world_mut(), roof);
    settle(&mut app);
    assert!(mount_shown(&mut app));
    assert_eq!(chip_tags(&mut app), vec!["roof"]);

    let field = app
        .world_mut()
        .query::<(Entity, &TagAddField)>()
        .iter(app.world())
        .find(|(_, field)| field.0 == roof)
        .map(|(entity, _)| entity)
        .expect("an add field for the selected node");
    let before = app.world().resource::<CommandHistory>().undo_stack.len();
    app.world_mut().trigger(TextEditCommitEvent {
        entity: field,
        text: "mossy".to_string(),
    });
    settle(&mut app);
    assert_eq!(
        app.world().get::<Tags>(roof).map(|tags| tags.0.clone()),
        Some(vec!["mossy".to_string(), "roof".to_string()])
    );
    assert_eq!(chip_tags(&mut app), vec!["mossy", "roof"]);
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        before + 1
    );

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    settle(&mut app);
    assert_eq!(chip_tags(&mut app), vec!["roof"], "undo reaches the chips");

    jackdaw::selection::clear_selection_in_world(app.world_mut());
    settle(&mut app);
    assert!(!mount_shown(&mut app), "the chips leave with the selection");
}

#[test]
fn the_chips_follow_an_in_place_edit_and_a_new_primary() {
    let mut app = util::editor_test_app();
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    settle(&mut app);

    let tagged = |app: &mut App, name: &str, tag: &str| {
        let entity = app
            .world_mut()
            .spawn((
                Name::new(name.to_string()),
                Transform::default(),
                Tags(vec![tag.into()]),
            ))
            .id();
        jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
        entity
    };
    let roof = tagged(&mut app, "roof", "roof");
    let wall = tagged(&mut app, "wall", "wall");
    jackdaw::selection::select_only(app.world_mut(), roof);
    settle(&mut app);
    assert_eq!(chip_tags(&mut app), vec!["roof"]);

    app.world_mut()
        .get_mut::<Tags>(roof)
        .expect("the roof is tagged")
        .insert("red");
    settle(&mut app);
    assert_eq!(chip_tags(&mut app), vec!["red", "roof"], "an in-place edit");

    jackdaw::selection::select_many(app.world_mut(), &[roof, wall]);
    settle(&mut app);
    let primary = app
        .world()
        .resource::<jackdaw::selection::Selection>()
        .primary();
    let expected = if primary == Some(wall) {
        vec!["wall"]
    } else {
        vec!["red", "roof"]
    };
    assert_eq!(chip_tags(&mut app), expected, "the primary's tags");

    jackdaw::selection::select_only(app.world_mut(), wall);
    settle(&mut app);
    assert_eq!(chip_tags(&mut app), vec!["wall"], "a new primary");
}
