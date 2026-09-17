//! The inspector card for a definition opened from a file on disk.
//!
//! The project reports the type only as schema, and the file names itself with
//! a kind segment its kind id never spells, so the card is built from the
//! schema and the patch the file holds rather than from any registration.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw::definition_assets::OpenDefinition;
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};

use crate::util;

const MOB_TYPE: &str = "mob_project::content::MobArchetypeDef";

const NODE: &str = "bevy_ui::ui_node::Node";

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mob_project")
}

/// A hand-authored mob file, in a folder of its own and under a name carrying
/// a kind segment its kind id never spells.
const AUTHORED_RAT: &str = "\
#giant_rat
mob_project::content::MobArchetypeDef {
    archetype_id: \"giant_rat\",
    name: \"Giant Rat\",
    max_health: 40,
    move_speed: 4.0,
    aggro_radius: 8.0,
    leash_radius: 20.0,
    temper: mob_project::content::Temper::Fierce,
    drops: [
        mob_project::content::LootDrop {
            family: \"chest\",
            weight: 6,
        },
        mob_project::content::LootDrop {
            family: \"legs\",
            weight: 2,
        },
    ],
}
";

/// A copy of the fixture project, with its schema where a build leaves it and
/// the rat's file under its assets.
fn project_copy() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let fixture = fixture_dir();
    std::fs::copy(
        fixture.join("jackdaw.toml"),
        tmp.path().join("jackdaw.toml"),
    )
    .expect("the manifest copies");
    let jackdaw_dir = tmp.path().join(".jackdaw");
    std::fs::create_dir_all(&jackdaw_dir).expect("the jackdaw directory is made");
    std::fs::copy(fixture.join("schema.json"), jackdaw_dir.join("schema.json"))
        .expect("the schema copies");
    let mobs = tmp.path().join("assets/content/mobs");
    std::fs::create_dir_all(&mobs).expect("the directory is made");
    std::fs::write(
        mobs.join("giant_rat.mob.bsn"),
        jackdaw::asset_files::asset_file_text(MOB_TYPE, AUTHORED_RAT),
    )
    .expect("the file is written");
    tmp
}

fn editor_with_the_rat_open() -> (App, tempfile::TempDir) {
    let tmp = project_copy();
    let mut app = util::editor_test_app();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: tmp.path().to_path_buf(),
            config: default(),
        });
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    jackdaw::pie::refresh_project_types(app.world_mut());
    for _ in 0..4 {
        app.update();
    }

    let result = app
        .world_mut()
        .operator("asset.open")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: false,
        })
        .param("path", "content/mobs/giant_rat.mob.bsn")
        .call()
        .expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Finished);
    for _ in 0..8 {
        app.update();
    }
    (app, tmp)
}

/// Whether a row is on screen: neither it nor anything it sits inside is
/// switched off.
fn on_screen(app: &App, entity: Entity) -> bool {
    let mut current = entity;
    loop {
        if app
            .world()
            .get::<Node>(current)
            .is_some_and(|node| node.display == Display::None)
        {
            return false;
        }
        match app.world().get::<ChildOf>(current) {
            Some(parent) => current = parent.parent(),
            None => return true,
        }
    }
}

/// Every control on the panel that writes a field, with the type it writes to.
fn all_rows(app: &mut App) -> Vec<(Entity, String, String)> {
    let entities: Vec<Entity> = app
        .world_mut()
        .query::<Entity>()
        .iter(app.world())
        .collect();
    entities
        .into_iter()
        .filter_map(|entity| {
            jackdaw::inspector::field_edited_by(app.world(), entity)
                .map(|(type_path, field)| (entity, type_path.to_string(), field.to_string()))
        })
        .collect()
}

/// Every row on the panel writing a field of `type_path`.
fn rows_of(app: &mut App, type_path: &str) -> Vec<Entity> {
    all_rows(app)
        .into_iter()
        .filter(|(_, known, _)| known == type_path)
        .map(|(entity, _, _)| entity)
        .collect()
}

fn field_rows(app: &mut App) -> Vec<(Entity, String)> {
    all_rows(app)
        .into_iter()
        .filter(|(_, type_path, _)| type_path == MOB_TYPE)
        .map(|(entity, _, field)| (entity, field))
        .collect()
}

/// Tick until the definition's rows have gone, so a slow frame budget cannot
/// read as a card that stayed up.
fn settle_until_the_field_rows_are_gone(app: &mut App) {
    for _ in 0..120 {
        if field_rows(app).is_empty() {
            return;
        }
        app.update();
    }
}

#[track_caller]
fn call(
    app: &mut App,
    id: &'static str,
    params: &[(&'static str, jackdaw_scene_types::PropertyValue)],
) {
    let mut call = app.world_mut().operator(id).settings(CallOperatorSettings {
        execution_context: ExecutionContext::Invoke,
        creates_history_entry: false,
    });
    for (key, value) in params {
        call = call.param(*key, value.clone());
    }
    let result = call.call().expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Finished, "{id} did not finish");
    for _ in 0..4 {
        app.update();
    }
}

/// The name the card carries, as the header spells it and as the operators
/// that open and close a card ask for it.
const CARD_NAME: &str = "giant_rat (Mob Archetype)";

fn definition_entity(app: &App) -> Entity {
    app.world()
        .resource::<OpenDefinition>()
        .0
        .expect("a definition is open")
}

/// Ask the inspector to build its cards again, saying nothing about the card
/// the definition put up: the name is one no card carries.
fn rebuild(app: &mut App) {
    call(
        app,
        "inspector.card",
        &[
            ("name", "no card of this name".into()),
            ("open", true.into()),
        ],
    );
}

#[test]
fn opening_a_definition_file_fills_the_card_with_a_row_for_every_field() {
    let (mut app, _tmp) = editor_with_the_rat_open();

    let fields: Vec<String> = field_rows(&mut app)
        .into_iter()
        .map(|(_, field)| field)
        .collect();
    for expected in [
        "archetype_id",
        "name",
        "max_health",
        "move_speed",
        "aggro_radius",
        "leash_radius",
        "drops[0].family",
        "drops[1].weight",
    ] {
        assert!(
            fields.iter().any(|field| field == expected),
            "the card has a row for {expected}, got {fields:?}"
        );
    }
}

#[test]
fn the_rows_of_an_opened_definition_are_on_screen() {
    let (mut app, _tmp) = editor_with_the_rat_open();

    let rows = field_rows(&mut app);
    assert!(!rows.is_empty(), "the card built rows to show");
    let hidden: Vec<&String> = rows
        .iter()
        .filter(|(entity, _)| !on_screen(&app, *entity))
        .map(|(_, field)| field)
        .collect();

    assert!(
        hidden.is_empty(),
        "every row the card built is shown, hidden: {hidden:?}"
    );
}

#[test]
fn a_rebuild_replaces_the_card_rather_than_stacking_one_on_it() {
    let (mut app, _tmp) = editor_with_the_rat_open();
    let open = definition_entity(&app);
    let before = field_rows(&mut app).len();
    assert!(before > 0, "the card built rows to count");

    app.world_mut().resource_mut::<Selection>().entities = vec![open];
    rebuild(&mut app);

    assert_eq!(
        field_rows(&mut app).len(),
        before,
        "the rebuild left the same rows, not a second set of them"
    );
}

#[test]
fn the_card_opens_expanded_and_keeps_a_collapse_across_a_rebuild() {
    let (mut app, _tmp) = editor_with_the_rat_open();
    assert!(
        field_rows(&mut app)
            .iter()
            .all(|(entity, _)| on_screen(&app, *entity)),
        "the card opens expanded"
    );

    call(
        &mut app,
        "inspector.card",
        &[("name", CARD_NAME.into()), ("open", false.into())],
    );
    let rows = field_rows(&mut app);
    assert!(!rows.is_empty(), "the card still holds its rows");
    assert!(
        rows.iter().all(|(entity, _)| !on_screen(&app, *entity)),
        "closing the card puts its rows away"
    );

    rebuild(&mut app);

    let rows = field_rows(&mut app);
    assert!(!rows.is_empty(), "the rebuilt card still holds its rows");
    assert!(
        rows.iter().all(|(entity, _)| !on_screen(&app, *entity)),
        "and the rebuild leaves it closed"
    );
}

#[test]
fn no_category_tab_hides_the_card_and_an_entity_selected_after_it_gets_its_own() {
    let (mut app, _tmp) = editor_with_the_rat_open();
    call(
        &mut app,
        "inspector.category",
        &[("category", "components".into())],
    );

    let rows = field_rows(&mut app);
    assert!(!rows.is_empty(), "the card is still up");
    assert!(
        rows.iter().all(|(entity, _)| on_screen(&app, *entity)),
        "a tab the definition has no part in does not hide its rows"
    );

    let entity = app
        .world_mut()
        .spawn((Name::new("lamp"), Node::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    app.world_mut().resource_mut::<Selection>().entities = vec![entity];
    settle_until_the_field_rows_are_gone(&mut app);

    assert!(
        !rows_of(&mut app, NODE).is_empty(),
        "the entity's own cards took the panel over"
    );
    assert!(
        field_rows(&mut app).is_empty(),
        "and the definition's card went with the definition"
    );
}

#[test]
fn clearing_the_selection_takes_the_definition_card_down() {
    let (mut app, _tmp) = editor_with_the_rat_open();
    assert!(
        app.world().resource::<OpenDefinition>().0.is_some(),
        "the card is up to begin with"
    );

    jackdaw::selection::clear_selection_in_world(app.world_mut());
    settle_until_the_field_rows_are_gone(&mut app);

    assert!(
        app.world().resource::<OpenDefinition>().0.is_none(),
        "an empty selection closes the definition"
    );
    assert!(
        field_rows(&mut app).is_empty(),
        "and the card's rows go with it"
    );
}
