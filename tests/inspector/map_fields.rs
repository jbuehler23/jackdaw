//! A map field on the inspector: each entry's value is a row, and what is
//! typed into it lands in the component and comes back out with one undo.

use std::collections::BTreeMap;

use crate::util;

use bevy::prelude::*;
use bevy::ui_widgets::ValueChange;
use jackdaw::boot_ops::run_op_clause;
use jackdaw::selection::Selection;

/// A model's material overrides, by the model's material name.
#[derive(Component, Reflect, Default, Clone)]
#[reflect(Component, Default)]
struct Worn {
    materials: BTreeMap<String, String>,
}

const WORN: &str = "inspector::map_fields::Worn";

fn app_with_worn() -> (App, Entity) {
    let mut app = util::editor_test_app();
    app.register_type::<Worn>();
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    let entity = app
        .world_mut()
        .spawn((
            Name::new("pine"),
            Worn {
                materials: BTreeMap::from([
                    ("Bark".to_string(), "materials/bark.bsn".to_string()),
                    ("Leaves".to_string(), "materials/leaves.bsn".to_string()),
                ]),
            },
        ))
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

/// The row that edits the `Leaves` entry.
fn leaves_row(app: &mut App) -> Entity {
    let mut all = app.world_mut().query::<Entity>();
    let entities: Vec<Entity> = all.iter(app.world()).collect();
    entities
        .into_iter()
        .find(|entity| {
            jackdaw::inspector::field_edited_by(app.world(), *entity).is_some_and(
                |(type_path, field)| {
                    type_path == WORN && field.starts_with("materials[") && field.contains("Leaves")
                },
            )
        })
        .expect("the Leaves entry has a row")
}

fn leaves(app: &App, entity: Entity) -> String {
    app.world().get::<Worn>(entity).expect("worn").materials["Leaves"].clone()
}

#[test]
fn text_committed_in_a_map_entry_row_lands_and_one_undo_takes_it_back() {
    let (mut app, entity) = app_with_worn();
    let row = leaves_row(&mut app);

    app.world_mut().trigger(ValueChange {
        source: row,
        value: "materials/pine.bsn".to_string(),
        is_final: true,
    });
    for _ in 0..2 {
        app.update();
    }
    assert_eq!(leaves(&app, entity), "materials/pine.bsn");
    assert_eq!(
        app.world().get::<Worn>(entity).expect("worn").materials["Bark"],
        "materials/bark.bsn",
        "the other entry is untouched"
    );

    let undone = run_op_clause(app.world_mut(), "history.undo").expect("undo dispatches");
    assert_eq!(undone, jackdaw_api::prelude::OperatorResult::Finished);
    for _ in 0..2 {
        app.update();
    }
    assert_eq!(leaves(&app, entity), "materials/leaves.bsn");
}
