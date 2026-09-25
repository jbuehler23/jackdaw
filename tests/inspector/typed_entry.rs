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
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
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

/// The text entry inside a field row.
fn text_entry(app: &mut App, row: Entity) -> Entity {
    let mut stack = vec![row];
    while let Some(entity) = stack.pop() {
        if app
            .world()
            .get::<bevy::text::EditableText>(entity)
            .is_some()
        {
            return entity;
        }
        if let Some(children) = app.world().get::<Children>(entity) {
            stack.extend(children.iter());
        }
    }
    panic!("the row has a text entry")
}

/// Focus the row's text entry and replace its text with `text`.
fn type_into(app: &mut App, row: Entity, text: &str) -> Entity {
    let entry = text_entry(app, row);
    app.world_mut()
        .resource_mut::<bevy::input_focus::InputFocus>()
        .set(entry, bevy::input_focus::FocusCause::Pressed);
    app.update();
    let mut editable = app
        .world_mut()
        .get_mut::<bevy::text::EditableText>(entry)
        .expect("editable");
    editable.queue_edit(bevy::text::TextEdit::SelectAll);
    editable.queue_edit(bevy::text::TextEdit::Insert(text.to_string().into()));
    app.update();
    app.update();
    entry
}

/// Press and release one key on the focused field.
fn press(app: &mut App, key: KeyCode, logical: bevy::input::keyboard::Key) {
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<bevy::window::PrimaryWindow>>()
        .iter(app.world())
        .next()
        .unwrap_or(Entity::PLACEHOLDER);
    for state in [
        bevy::input::ButtonState::Pressed,
        bevy::input::ButtonState::Released,
    ] {
        app.world_mut()
            .write_message(bevy::input::keyboard::KeyboardInput {
                key_code: key,
                logical_key: logical.clone(),
                state,
                text: None,
                repeat: false,
                window,
            });
        app.update();
    }
    app.update();
}

fn undo_depth(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

#[test]
fn enter_commits_a_typed_number_once_and_lets_go_of_the_field() {
    let (mut app, entity) = app_with_a_block();
    let row = field(&mut app, "translation.y");
    let before = undo_depth(&app);

    let entry = type_into(&mut app, row, "4");
    press(&mut app, KeyCode::Enter, bevy::input::keyboard::Key::Enter);

    assert_eq!(height(&app, entity), 4.0);
    assert_ne!(
        app.world()
            .resource::<bevy::input_focus::InputFocus>()
            .get(),
        Some(entry),
        "the field let go of focus"
    );
    assert_eq!(undo_depth(&app), before + 1, "one Enter, one entry");

    let undone = run_op_clause(app.world_mut(), "history.undo").expect("undo dispatches");
    assert_eq!(undone, OperatorResult::Finished);
    app.update();
    assert_eq!(height(&app, entity), 1.0, "one undo takes it back");
}

#[test]
fn escape_puts_back_what_the_number_field_showed_and_commits_nothing() {
    let (mut app, entity) = app_with_a_block();
    let row = field(&mut app, "translation.y");
    let before = undo_depth(&app);

    let entry = type_into(&mut app, row, "7");
    press(
        &mut app,
        KeyCode::Escape,
        bevy::input::keyboard::Key::Escape,
    );

    assert_eq!(height(&app, entity), 1.0);
    assert_eq!(undo_depth(&app), before, "nothing was committed");
    assert_ne!(
        app.world()
            .resource::<bevy::input_focus::InputFocus>()
            .get(),
        Some(entry),
        "the field let go of focus"
    );
    let shown = app
        .world()
        .get::<bevy::text::EditableText>(entry)
        .expect("editable")
        .value()
        .to_string();
    assert!(
        shown.starts_with('1'),
        "the field shows 1 again, not {shown}"
    );
}

/// A sign's wording.
#[derive(Component, Reflect, Default, Clone)]
#[reflect(Component, Default)]
struct Sign {
    wording: String,
}

const SIGN: &str = "inspector::typed_entry::Sign";

fn app_with_a_sign() -> (App, Entity) {
    let mut app = util::editor_test_app();
    app.register_type::<Sign>();
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    let entity = app
        .world_mut()
        .spawn((
            Name::new("sign"),
            Sign {
                wording: "Ashdene".to_string(),
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

fn wording_row(app: &mut App) -> Entity {
    let mut all = app.world_mut().query::<Entity>();
    let entities: Vec<Entity> = all.iter(app.world()).collect();
    entities
        .into_iter()
        .find(|entity| {
            jackdaw::inspector::field_edited_by(app.world(), *entity) == Some((SIGN, "wording"))
        })
        .expect("the wording has a row")
}

fn wording(app: &App, entity: Entity) -> String {
    app.world()
        .get::<Sign>(entity)
        .expect("sign")
        .wording
        .clone()
}

#[test]
fn enter_commits_typed_text_once_and_one_undo_takes_it_back() {
    let (mut app, entity) = app_with_a_sign();
    let row = wording_row(&mut app);
    let before = undo_depth(&app);

    type_into(&mut app, row, "Thornback Fells");
    press(&mut app, KeyCode::Enter, bevy::input::keyboard::Key::Enter);
    assert_eq!(wording(&app, entity), "Thornback Fells");
    assert_eq!(undo_depth(&app), before + 1, "one Enter, one entry");

    let undone = run_op_clause(app.world_mut(), "history.undo").expect("undo dispatches");
    assert_eq!(undone, OperatorResult::Finished);
    app.update();
    app.update();
    assert_eq!(wording(&app, entity), "Ashdene");
    for _ in 0..4 {
        app.update();
    }

    let row = wording_row(&mut app);
    let entry = text_entry(&mut app, row);
    assert_eq!(
        app.world()
            .get::<bevy::text::EditableText>(entry)
            .expect("editable")
            .value()
            .to_string(),
        "Ashdene",
        "the row shows what undo put back"
    );
}

#[test]
fn escape_puts_back_the_text_and_commits_nothing() {
    let (mut app, entity) = app_with_a_sign();
    let row = wording_row(&mut app);
    let before = undo_depth(&app);

    let entry = type_into(&mut app, row, "Elsewhere");
    press(
        &mut app,
        KeyCode::Escape,
        bevy::input::keyboard::Key::Escape,
    );
    assert_eq!(wording(&app, entity), "Ashdene");
    assert_eq!(undo_depth(&app), before, "nothing was committed");
    assert_eq!(
        app.world()
            .get::<bevy::text::EditableText>(entry)
            .expect("editable")
            .value()
            .to_string(),
        "Ashdene"
    );
}
