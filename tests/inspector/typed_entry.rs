//! A number typed into an inspector field commits when Enter is pressed and
//! again when the field loses focus with the text still as typed. The second
//! commit changes nothing, and one undo has to take the typed value back.

use crate::util;

use bevy::prelude::*;
use bevy::ui_widgets::ValueChange;
use jackdaw::boot_ops::run_op_clause;
use jackdaw::commands::CommandHistory;
use jackdaw::selection::Selection;
use jackdaw_api::prelude::OperatorResult;

const TRANSFORM: &str = "bevy_transform::components::transform::Transform";

fn app_with_a_block() -> (App, Entity) {
    let mut app = util::editor_test_app();
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    let entity = app
        .world_mut()
        .spawn((Name::new("block"), Transform::from_xyz(0.0, 1.0, 0.0)))
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

fn field(app: &mut App, path: &str) -> Entity {
    let mut all = app.world_mut().query::<Entity>();
    let entities: Vec<Entity> = all.iter(app.world()).collect();
    entities
        .into_iter()
        .find(|entity| {
            jackdaw::inspector::field_edited_by(app.world(), *entity) == Some((TRANSFORM, path))
        })
        .unwrap_or_else(|| panic!("no row writes {path}"))
}

fn height(app: &App, entity: Entity) -> f32 {
    app.world()
        .get::<Transform>(entity)
        .expect("transform")
        .translation
        .y
}

#[test]
fn a_typed_number_committed_by_enter_and_again_by_blur_undoes_in_one_step() {
    let (mut app, entity) = app_with_a_block();
    let row = field(&mut app, "translation.y");
    let before = app.world().resource::<CommandHistory>().undo_stack.len();

    for _ in 0..2 {
        app.world_mut().trigger(ValueChange {
            source: row,
            value: 4.0_f64,
            is_final: true,
        });
        app.update();
    }
    assert_eq!(height(&app, entity), 4.0);
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        before + 1,
        "the commit that changed nothing left no entry"
    );

    let undone = run_op_clause(app.world_mut(), "history.undo").expect("undo dispatches");
    assert_eq!(undone, OperatorResult::Finished);
    app.update();
    assert_eq!(height(&app, entity), 1.0);
}
