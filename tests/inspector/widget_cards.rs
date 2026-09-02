//! The authored widget components in the inspector.
//!
//! `jackdaw_widgets_runtime` holds everything a list-shaped widget carries: a
//! progress bar's value, a dropdown's options, a tab strip's labels. The
//! namespace cull that keeps jackdaw's bookkeeping out of the generic card
//! list swallowed them too, so the widgets could be placed and never authored.
//!
//! The list controls are the other half of the same job: a `Vec<String>` shown
//! but not added to is a list nobody can write.

use crate::util;

use bevy::prelude::*;
use bevy::ui_widgets::ValueChange;
use jackdaw::selection::Selection;
use jackdaw_feathers::button::ButtonClickEvent;
use jackdaw_feathers::tooltip::Tooltip;
use jackdaw_widgets_runtime::{Dropdown, DropdownOption, Progress};

const PROGRESS: &str = "jackdaw_widgets_runtime::Progress";
const DROPDOWN: &str = "jackdaw_widgets_runtime::Dropdown";

/// A selected, document-tracked entity carrying `widget`.
fn app_with(widget: impl Bundle) -> (App, Entity) {
    let mut app = util::editor_test_app();
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    let entity = app
        .world_mut()
        .spawn((Name::new("widget"), Node::default(), widget))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    let world = app.world_mut();
    world.resource_scope(|world, mut selection: Mut<Selection>| {
        let mut commands = world.commands();
        selection.select_single(&mut commands, entity);
    });
    world.flush();
    for _ in 0..4 {
        app.update();
    }
    (app, entity)
}

fn all_entities(app: &mut App) -> Vec<Entity> {
    app.world_mut()
        .query::<Entity>()
        .iter(app.world())
        .collect()
}

/// The control on the inspector that writes `field_path` of `type_path`.
fn field_widget(app: &mut App, type_path: &str, field_path: &str) -> Entity {
    all_entities(app)
        .into_iter()
        .find(|entity| {
            jackdaw::inspector::field_edited_by(app.world(), *entity)
                == Some((type_path, field_path))
        })
        .unwrap_or_else(|| panic!("no control on the inspector writes `{field_path}`"))
}

fn has_field_widget(app: &mut App, type_path: &str, field_path: &str) -> bool {
    all_entities(app).into_iter().any(|entity| {
        jackdaw::inspector::field_edited_by(app.world(), entity) == Some((type_path, field_path))
    })
}

/// The button that adds one item to a list field.
fn add_button(app: &mut App) -> Entity {
    all_entities(app)
        .into_iter()
        .find(|entity| {
            app.world()
                .get::<Tooltip>(*entity)
                .is_some_and(|tip| tip.title == "Add an item to the list")
        })
        .expect("the list field offers an Add")
}

fn option_rows(app: &mut App) -> usize {
    app.world_mut()
        .query::<&DropdownOption>()
        .iter(app.world())
        .count()
}

#[test]
fn a_progress_bar_shows_its_value_and_the_field_commits() {
    let (mut app, entity) = app_with(Progress { value: 0.5 });

    let widget = field_widget(&mut app, PROGRESS, "value");
    app.world_mut().trigger(ValueChange {
        source: widget,
        value: 0.25_f64,
        is_final: true,
    });
    for _ in 0..2 {
        app.update();
    }

    assert_eq!(
        app.world().get::<Progress>(entity).map(|value| value.value),
        Some(0.25),
        "the value field writes the component",
    );
}

#[test]
fn a_dropdown_lists_its_options_and_adding_one_regenerates_the_chrome() {
    let (mut app, entity) = app_with(Dropdown {
        options: vec!["One".to_string(), "Two".to_string()],
        selected: 0,
    });
    assert_eq!(
        option_rows(&mut app),
        2,
        "the widget draws a row per option"
    );
    assert!(
        has_field_widget(&mut app, DROPDOWN, "options[0]"),
        "the card lists the options one editable row each",
    );
    assert!(has_field_widget(&mut app, DROPDOWN, "options[1]"));

    let add = add_button(&mut app);
    app.world_mut().trigger(ButtonClickEvent { entity: add });
    for _ in 0..10 {
        app.update();
    }

    assert_eq!(
        app.world()
            .get::<Dropdown>(entity)
            .map(|dropdown| dropdown.options.len()),
        Some(3),
        "Add put one more option on the list",
    );
    assert_eq!(
        option_rows(&mut app),
        3,
        "and the widget drew the option that was added",
    );
}
