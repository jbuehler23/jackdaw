//! The inspector card for a definition kind the project reported.
//!
//! The editor has no registration for the type, only the shape the last build
//! reported, so the rows are built from that: a scalar row, a menu carrying the
//! schema's variants, and list controls that add and remove rows.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy::ui_widgets::{Activate, ValueChange};
use jackdaw::definition_assets::OpenDefinition;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_feathers::button::ButtonClickEvent;
use jackdaw_feathers::tooltip::Tooltip;

use crate::util;

const ITEM_TYPE: &str = "definition_project::content::ItemDef";
const QUEST_TYPE: &str = "definition_project::content::QuestDef";

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/definition_project")
}

/// An editor showing the inspector, with an item definition of a kind the
/// project reported open in it.
fn app_with_open_item() -> (App, tempfile::TempDir) {
    app_with_open("item", "torch")
}

/// The same editor with a quest open, whose objectives are an enum whose
/// variants carry fields.
fn app_with_open_quest() -> (App, tempfile::TempDir) {
    app_with_open("quest", "errand")
}

fn app_with_open(kind: &'static str, name: &'static str) -> (App, tempfile::TempDir) {
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
    app.update();

    let result = app
        .world_mut()
        .operator("asset.new")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        })
        .param("type", kind)
        .param("name", name)
        .call()
        .expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Finished);
    for _ in 0..6 {
        app.update();
    }
    (app, tmp)
}

fn open_item(app: &App) -> serde_json::Value {
    open_definition(app, "item")
}

fn open_quest(app: &App) -> serde_json::Value {
    open_definition(app, "quest")
}

fn open_definition(app: &App, kind: &str) -> serde_json::Value {
    let path = jackdaw::definition_assets::open_definition_path(app.world())
        .expect("a definition is open");
    jackdaw::definition_assets::schema_definition_json(app.world(), kind, &path)
        .expect("the definition reads back")
}

fn all_entities(app: &mut App) -> Vec<Entity> {
    app.world_mut()
        .query::<Entity>()
        .iter(app.world())
        .collect()
}

fn field_widget(app: &mut App, field_path: &str) -> Entity {
    widget_writing(app, ITEM_TYPE, field_path)
}

fn widget_writing(app: &mut App, type_path: &'static str, field_path: &str) -> Entity {
    let found = all_entities(app).into_iter().find(|entity| {
        jackdaw::inspector::field_edited_by(app.world(), *entity) == Some((type_path, field_path))
    });
    found.unwrap_or_else(|| {
        let paths: Vec<String> = all_entities(app)
            .into_iter()
            .filter_map(|entity| {
                jackdaw::inspector::field_edited_by(app.world(), entity)
                    .map(|(known, field)| format!("{known}.{field}"))
            })
            .collect();
        panic!("no control on the inspector writes `{field_path}`; it writes {paths:?}")
    })
}

/// Every menu item caption on the card, with the entity carrying it.
fn menu_items(app: &mut App) -> Vec<(Entity, String)> {
    let mut items = Vec::new();
    for entity in all_entities(app) {
        if app
            .world()
            .get::<bevy::ui_widgets::MenuItem>(entity)
            .is_none()
        {
            continue;
        }
        let caption = descendant_text(app, entity);
        items.push((entity, caption));
    }
    items
}

fn descendant_text(app: &mut App, root: Entity) -> String {
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        if let Some(text) = app.world().get::<Text>(entity) {
            return text.0.clone();
        }
        if let Some(children) = app.world().get::<Children>(entity) {
            stack.extend(children.iter());
        }
    }
    String::new()
}

#[test]
fn a_scalar_row_on_a_schema_definition_writes_the_value_its_file_holds() {
    let (mut app, _tmp) = app_with_open_item();

    let widget = field_widget(&mut app, "stack_size");
    app.world_mut().trigger(ValueChange {
        source: widget,
        value: 40.0_f64,
        is_final: true,
    });
    for _ in 0..4 {
        app.update();
    }

    assert_eq!(open_item(&app)["stack_size"], 40, "the row wrote the value");
    let open = app.world().resource::<OpenDefinition>().0.expect("open");
    assert!(
        app.world()
            .get::<jackdaw::definition_assets::DefinitionAssetEdit>(open)
            .is_some_and(|edit| edit.dirty),
        "and the card knows it has something to save",
    );
}

#[test]
fn the_enum_row_offers_the_variants_the_schema_reports() {
    let (mut app, _tmp) = app_with_open_item();

    let items = menu_items(&mut app);
    let captions: Vec<&str> = items.iter().map(|(_, caption)| caption.as_str()).collect();
    assert!(
        captions.contains(&"Common") && captions.contains(&"Rare") && captions.contains(&"Epic"),
        "the menu offers the reported variants, got {captions:?}"
    );

    let rare = items
        .iter()
        .find(|(_, caption)| caption == "Rare")
        .map(|(entity, _)| *entity)
        .expect("the menu offers Rare");
    app.world_mut().trigger(Activate { entity: rare });
    for _ in 0..4 {
        app.update();
    }

    assert_eq!(open_item(&app)["rarity"], "Rare", "picking one writes it");
}

fn control_titled(app: &mut App, title: &str) -> Entity {
    all_entities(app)
        .into_iter()
        .find(|entity| {
            app.world()
                .get::<Tooltip>(*entity)
                .is_some_and(|tip| tip.title == title)
        })
        .unwrap_or_else(|| panic!("the card offers a '{title}' control"))
}

#[test]
fn the_list_controls_take_a_row_back_off_the_list() {
    let (mut app, _tmp) = app_with_open_item();
    let add = control_titled(&mut app, "Add an item to the list");
    app.world_mut().trigger(ButtonClickEvent { entity: add });
    for _ in 0..10 {
        app.update();
    }
    assert_eq!(open_item(&app)["loot"].as_array().map(Vec::len), Some(1));

    let remove = control_titled(&mut app, "Remove");
    app.world_mut().trigger(ButtonClickEvent { entity: remove });
    for _ in 0..10 {
        app.update();
    }

    assert_eq!(
        open_item(&app)["loot"].as_array().map(Vec::len),
        Some(0),
        "Remove took the row it stood beside off the list"
    );
}

#[test]
fn the_list_controls_add_a_row_to_a_schema_definitions_list() {
    let (mut app, _tmp) = app_with_open_item();
    assert_eq!(open_item(&app)["loot"].as_array().map(Vec::len), Some(0));

    let add = control_titled(&mut app, "Add an item to the list");
    app.world_mut().trigger(ButtonClickEvent { entity: add });
    for _ in 0..10 {
        app.update();
    }

    assert_eq!(
        open_item(&app)["loot"].as_array().map(Vec::len),
        Some(1),
        "Add put one entry on the definition's list"
    );
    assert_eq!(
        open_item(&app)["loot"][0],
        serde_json::json!({ "item": "", "weight": 1 }),
        "the new row holds what the schema says its type defaults to"
    );
    let row = field_widget(&mut app, "loot[0].weight");
    assert!(
        jackdaw::inspector::field_edited_by(app.world(), row).is_some(),
        "and the card rebuilt with a row for it",
    );
}

#[test]
fn removing_one_row_leaves_the_rest_holding_what_they_held() {
    let (mut app, _tmp) = app_with_open_item();
    let coin = serde_json::json!({ "item": "coin", "weight": 3 });
    let gem = serde_json::json!({ "item": "gem", "weight": 7 });
    let result = app
        .world_mut()
        .operator("asset.set")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        })
        .param("field", "loot")
        .param("value", serde_json::json!([coin, gem]).to_string())
        .call()
        .expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Finished);
    for _ in 0..10 {
        app.update();
    }
    assert_eq!(open_item(&app)["loot"].as_array().map(Vec::len), Some(2));

    let remove = control_titled(&mut app, "Remove");
    app.world_mut().trigger(ButtonClickEvent { entity: remove });
    for _ in 0..10 {
        app.update();
    }

    let loot = open_item(&app)["loot"]
        .as_array()
        .cloned()
        .expect("the field is still a list");
    assert_eq!(loot.len(), 1, "one row went, got {loot:?}");
    assert!(
        loot[0] == coin || loot[0] == gem,
        "the row that stayed is the one it was, got {loot:?}"
    );
}

/// The menu item on the card carrying this caption.
fn menu_item(app: &mut App, caption: &str) -> Entity {
    let items = menu_items(app);
    items
        .iter()
        .find(|(_, shown)| shown == caption)
        .map(|(entity, _)| *entity)
        .unwrap_or_else(|| {
            let shown: Vec<&String> = items.iter().map(|(_, caption)| caption).collect();
            panic!("no menu item says '{caption}'; the card offers {shown:?}")
        })
}

fn settle(app: &mut App) {
    for _ in 0..10 {
        app.update();
    }
}

/// The control with this tooltip that acts on the named list.
fn list_control(app: &mut App, title: &str, field_path: &str) -> Entity {
    all_entities(app)
        .into_iter()
        .find(|entity| {
            app.world()
                .get::<Tooltip>(*entity)
                .is_some_and(|tip| tip.title == title)
                && jackdaw::inspector::list_edited_by(app.world(), *entity)
                    .is_some_and(|(_, field)| field == field_path)
        })
        .unwrap_or_else(|| panic!("no '{title}' control acts on `{field_path}`"))
}

#[test]
fn an_objective_added_to_the_list_offers_the_variants_its_type_declares() {
    let (mut app, _tmp) = app_with_open_quest();
    let add = list_control(&mut app, "Add an item to the list", "objectives");
    app.world_mut().trigger(ButtonClickEvent { entity: add });
    settle(&mut app);

    assert_eq!(
        open_quest(&app)["objectives"][0],
        serde_json::json!("Explore"),
        "the new row holds what the enum defaults to"
    );
    let captions: Vec<String> = menu_items(&mut app)
        .into_iter()
        .map(|(_, caption)| caption)
        .collect();
    for variant in ["Explore", "Kill", "Reach"] {
        assert!(
            captions.contains(&variant.to_string()),
            "the menu offers {variant}, got {captions:?}"
        );
    }
}

#[test]
fn choosing_a_variant_that_carries_fields_gives_the_row_its_fields() {
    let (mut app, _tmp) = app_with_open_quest();
    let add = list_control(&mut app, "Add an item to the list", "objectives");
    app.world_mut().trigger(ButtonClickEvent { entity: add });
    settle(&mut app);

    let kill = menu_item(&mut app, "Kill");
    app.world_mut().trigger(Activate { entity: kill });
    settle(&mut app);

    assert_eq!(
        open_quest(&app)["objectives"][0],
        serde_json::json!({ "Kill": { "mob": "", "count": 0 } }),
        "the variant is written with what its fields default to"
    );
    let count = widget_writing(&mut app, QUEST_TYPE, "objectives[0].Kill.count");
    app.world_mut().trigger(ValueChange {
        source: count,
        value: 3.0_f64,
        is_final: true,
    });
    settle(&mut app);

    assert_eq!(
        open_quest(&app)["objectives"][0]["Kill"]["count"],
        3,
        "and the row it grew writes the field it stands for"
    );
}

#[test]
fn an_objective_taken_off_the_list_leaves_the_rest_holding_what_they_held() {
    let (mut app, _tmp) = app_with_open_quest();
    let result = app
        .world_mut()
        .operator("asset.set")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        })
        .param("field", "objectives")
        .param(
            "value",
            r#"[{"Kill":{"mob":"rat","count":3}},"Explore"]"#.to_string(),
        )
        .call()
        .expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Finished);
    settle(&mut app);
    assert_eq!(
        open_quest(&app)["objectives"].as_array().map(Vec::len),
        Some(2)
    );

    let remove = list_control(&mut app, "Remove", "objectives");
    app.world_mut().trigger(ButtonClickEvent { entity: remove });
    settle(&mut app);

    let objectives = open_quest(&app)["objectives"]
        .as_array()
        .cloned()
        .expect("the field is still a list");
    assert_eq!(objectives.len(), 1, "one row went, got {objectives:?}");
    assert!(
        objectives[0] == serde_json::json!({ "Kill": { "mob": "rat", "count": 3 } })
            || objectives[0] == serde_json::json!("Explore"),
        "the row that stayed is the one it was, got {objectives:?}"
    );
}

#[test]
fn a_variant_field_the_file_leaves_out_is_still_shown_and_written() {
    let (mut app, _tmp) = app_with_open_quest();
    let result = app
        .world_mut()
        .operator("asset.set")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        })
        .param("field", "objectives")
        .param("value", r#"[{"Kill":{"mob":"rat"}}]"#.to_string())
        .call()
        .expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Finished);
    settle(&mut app);

    let count = widget_writing(&mut app, QUEST_TYPE, "objectives[0].Kill.count");
    app.world_mut().trigger(ValueChange {
        source: count,
        value: 2.0_f64,
        is_final: true,
    });
    settle(&mut app);

    assert_eq!(
        open_quest(&app)["objectives"][0]["Kill"]["count"],
        2,
        "the row a spelled-out field never got writes the field it stands for"
    );
}
