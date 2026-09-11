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

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/definition_project")
}

/// An editor showing the inspector, with an item definition of a kind the
/// project reported open in it.
fn app_with_open_item() -> (App, tempfile::TempDir) {
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
        .param("type", "item")
        .param("name", "torch")
        .call()
        .expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Finished);
    for _ in 0..6 {
        app.update();
    }
    (app, tmp)
}

fn open_item(app: &App) -> serde_json::Value {
    let path = jackdaw::definition_assets::open_definition_path(app.world())
        .expect("a definition is open");
    jackdaw::definition_assets::schema_definition_json(app.world(), "item", &path)
        .expect("the definition reads back")
}

fn all_entities(app: &mut App) -> Vec<Entity> {
    app.world_mut()
        .query::<Entity>()
        .iter(app.world())
        .collect()
}

fn field_widget(app: &mut App, field_path: &str) -> Entity {
    let found = all_entities(app).into_iter().find(|entity| {
        jackdaw::inspector::field_edited_by(app.world(), *entity) == Some((ITEM_TYPE, field_path))
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
