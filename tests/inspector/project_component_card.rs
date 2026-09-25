//! The entity card for a component only the open project knows.
//!
//! A project component is no ECS component in the editor: it lives in the
//! scene document and is drawn from the extracted schema. The card has to
//! stand whether the schema arrived before the scene or after it.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy::ui_widgets::{Activate, MenuItem};
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_bsn::BsnValue;
use jackdaw_scene_types::PropertyValue;

use crate::util;

const REGION: &str = "definition_project::markers::SpawnRegion";

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/definition_project")
}

fn settle(app: &mut App) {
    for _ in 0..8 {
        app.update();
    }
}

#[track_caller]
fn call(app: &mut App, id: &'static str, params: &[(&'static str, PropertyValue)]) {
    let mut call = app.world_mut().operator(id).settings(CallOperatorSettings {
        execution_context: ExecutionContext::Invoke,
        creates_history_entry: true,
    });
    for (name, value) in params {
        call = call.param(*name, value.clone());
    }
    let result = call.call().expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Finished, "{id} ran");
    settle(app);
}

/// An editor with the fixture project open, whose schema reports one
/// component of the project's own.
fn app_with_project_schema() -> (App, tempfile::TempDir) {
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
    std::fs::create_dir_all(tmp.path().join("assets")).expect("an assets folder");

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
    settle(&mut app);
    (app, tmp)
}

/// A scene holding one group that carries the project component, saved and
/// closed, as an author leaves it.
fn scene_with_a_region(app: &mut App, tmp: &tempfile::TempDir) -> PathBuf {
    let scene = tmp.path().join("assets/camp.bsn");
    let path = scene.display().to_string();
    call(
        app,
        "scene.new",
        &[
            ("kind", "3d".into()),
            ("path", PropertyValue::from(path.clone())),
        ],
    );
    call(app, "entity.add.group", &[("name", "Camp".into())]);
    let group = named_entity(app, "Camp");
    call(
        app,
        "component.add",
        &[
            ("entity", PropertyValue::Entity(group)),
            ("type_path", REGION.into()),
        ],
    );
    call(
        app,
        "component.set",
        &[
            ("entity", PropertyValue::Entity(group)),
            ("type_path", REGION.into()),
            ("field", "radius".into()),
            ("value", "4.0".into()),
        ],
    );
    call(app, "scene.save", &[]);
    scene
}

/// The entity this scene calls `wanted`.
#[track_caller]
fn named_entity(app: &mut App, wanted: &str) -> Entity {
    let mut query = app.world_mut().query::<(Entity, &Name)>();
    query
        .iter(app.world())
        .find(|(_, name)| name.as_str() == wanted)
        .map(|(entity, _)| entity)
        .unwrap_or_else(|| panic!("the scene holds `{wanted}`"))
}

fn select_the_group(app: &mut App) -> Entity {
    let group = named_entity(app, "Camp");
    let world = app.world_mut();
    world.resource_scope(|world, mut selection: Mut<Selection>| {
        let mut commands = world.commands();
        selection.select_single(&mut commands, group);
    });
    world.flush();
    settle(app);
    group
}

#[test]
fn a_project_component_the_document_holds_is_on_the_card_after_a_reopen() {
    let (mut app, tmp) = app_with_project_schema();
    let scene = scene_with_a_region(&mut app, &tmp);

    let path = scene.display().to_string();
    call(&mut app, "scene.close", &[]);
    call(
        &mut app,
        "scene.open",
        &[
            ("path", PropertyValue::from(path.clone())),
            ("reload", PropertyValue::Bool(true)),
        ],
    );
    select_the_group(&mut app);

    let showing = jackdaw::inspector::component_cards_showing(app.world_mut());
    assert!(
        showing.iter().any(|type_path| type_path == REGION),
        "the card lists the component the document holds, got {showing:?}",
    );
}

#[test]
fn a_project_component_card_shows_the_fields_its_schema_reports() {
    let (mut app, tmp) = app_with_project_schema();
    let scene = scene_with_a_region(&mut app, &tmp);

    let path = scene.display().to_string();
    call(&mut app, "scene.close", &[]);
    call(
        &mut app,
        "scene.open",
        &[
            ("path", PropertyValue::from(path.clone())),
            ("reload", PropertyValue::Bool(true)),
        ],
    );
    let group = select_the_group(&mut app);

    let lines = jackdaw::inspector::component_card_text(app.world_mut(), REGION);
    assert!(
        lines.iter().any(|line| line.contains("radius")),
        "the card draws the field the schema reports, got {lines:?}",
    );
    assert!(
        app.world().get::<Name>(group).is_some(),
        "the group is still the entity the card stands for",
    );
}

/// Take the schema away, as a project whose build stopped reporting the type
/// leaves the editor.
fn forget_the_schema(app: &mut App) {
    let native = jackdaw::project_types::native_type_paths(
        &app.world().resource::<AppTypeRegistry>().read(),
    );
    app.world_mut()
        .resource_mut::<jackdaw::project_types::ProjectTypes>()
        .update(&jackdaw_schema::ProjectSchema::default(), &native);
}

#[test]
fn a_component_the_schema_no_longer_describes_still_shows_what_it_holds() {
    let (mut app, tmp) = app_with_project_schema();
    let scene = scene_with_a_region(&mut app, &tmp);
    forget_the_schema(&mut app);

    let path = scene.display().to_string();
    call(&mut app, "scene.close", &[]);
    call(
        &mut app,
        "scene.open",
        &[
            ("path", PropertyValue::from(path.clone())),
            ("reload", PropertyValue::Bool(true)),
        ],
    );
    select_the_group(&mut app);

    let showing = jackdaw::inspector::component_cards_showing(app.world_mut());
    assert!(
        showing.iter().any(|type_path| type_path == REGION),
        "a type nothing describes is still on the card, got {showing:?}",
    );
    let lines = jackdaw::inspector::component_card_text(app.world_mut(), REGION);
    assert!(
        lines.iter().any(|line| line.contains("radius")),
        "and it says what the document holds, got {lines:?}",
    );
}

/// The schema of a project that reports one enum component beside the region.
fn schema_with_an_enum() -> jackdaw_schema::ProjectSchema {
    serde_json::from_value(serde_json::json!({
        "components": [{
            "type_path": "definition_project::markers::Stance",
            "short_name": "Stance",
            "module_path": "definition_project::markers",
            "category": "",
            "description": "",
            "editor_description": "",
            "hidden": false,
            "preview": "",
            "default_constructible": true,
            "fields": [],
            "kind": "Enum",
            "default": null,
            "variants": [{ "name": "Guard", "fields": [] }],
            "entity_fields": [],
            "fills_gaps": true,
            "asset": false
        }],
        "resources": [],
        "events": [],
        "functions": [],
        "assets": []
    }))
    .expect("the schema reads")
}

/// A scene whose group carries one authored enum variant, as a document the
/// game's own types spell it.
fn scene_with_a_stance(app: &mut App, tmp: &tempfile::TempDir) -> PathBuf {
    let scene = tmp.path().join("assets/watch.bsn");
    let path = scene.display().to_string();
    call(
        app,
        "scene.new",
        &[
            ("kind", "3d".into()),
            ("path", PropertyValue::from(path.clone())),
        ],
    );
    call(app, "entity.add.group", &[("name", "Camp".into())]);
    call(app, "scene.save", &[]);
    let written = std::fs::read_to_string(&scene).expect("the scene is on disk");
    let authored = written.replace(
        "#Camp\n",
        "#Camp\n    definition_project::markers::Stance::Guard\n",
    );
    assert_ne!(authored, written, "the variant went into the document");
    std::fs::write(&scene, authored).expect("the scene is written back");
    scene
}

#[test]
fn an_authored_variant_of_a_reported_enum_is_named_by_its_type() {
    let (mut app, tmp) = app_with_project_schema();
    let native = jackdaw::project_types::native_type_paths(
        &app.world().resource::<AppTypeRegistry>().read(),
    );
    app.world_mut()
        .resource_mut::<jackdaw::project_types::ProjectTypes>()
        .update(&schema_with_an_enum(), &native);
    jackdaw::project_types::publish_document_only_types(app.world_mut());
    let scene = scene_with_a_stance(&mut app, &tmp);

    call(&mut app, "scene.close", &[]);
    call(
        &mut app,
        "scene.open",
        &[
            ("path", PropertyValue::from(scene.display().to_string())),
            ("reload", PropertyValue::Bool(true)),
        ],
    );
    select_the_group(&mut app);

    let variant = "definition_project::markers::Stance::Guard";
    let showing = jackdaw::inspector::component_cards_showing(app.world_mut());
    assert!(
        showing.iter().any(|type_path| type_path == variant),
        "the variant the document holds is on the card, got {showing:?}",
    );
    let lines = jackdaw::inspector::component_card_text(app.world_mut(), variant);
    assert!(
        lines.iter().any(|line| line.contains("Stance::Guard")),
        "headed by the type it belongs to, got {lines:?}",
    );
    assert!(
        !lines
            .iter()
            .any(|line| line.contains("does not describe this type")),
        "and not called a type nothing describes, got {lines:?}",
    );
}

const TRIGGER: &str = "definition_project::markers::ActionTrigger";
const WHEN: &str = "definition_project::markers::ActionWhen";

/// A struct component holding a unit enum, the shape an action timer trigger
/// uses: a string plus when/action enums the editor has no registration for.
fn schema_with_a_trigger() -> jackdaw_schema::ProjectSchema {
    serde_json::from_value(serde_json::json!({
        "components": [{
            "type_path": TRIGGER,
            "short_name": "ActionTrigger",
            "module_path": "definition_project::markers",
            "category": "",
            "description": "",
            "editor_description": "",
            "hidden": false,
            "preview": "",
            "default_constructible": true,
            "fields": [
                { "name": "timer_key", "type_path": "alloc::string::String" },
                { "name": "when", "type_path": WHEN }
            ],
            "kind": "Struct",
            "default": {
                "definition_project::markers::ActionTrigger": {
                    "timer_key": "",
                    "when": "Interact"
                }
            },
            "variants": [],
            "entity_fields": [],
            "fills_gaps": true,
            "asset": false
        }],
        "field_types": [{
            "type_path": WHEN,
            "short_name": "ActionWhen",
            "module_path": "definition_project::markers",
            "category": "",
            "description": "",
            "editor_description": "",
            "hidden": false,
            "preview": "",
            "default_constructible": true,
            "fields": [],
            "kind": "Enum",
            "default": {
                "definition_project::markers::ActionWhen": "Interact"
            },
            "variants": [
                { "name": "Interact", "fields": [] },
                { "name": "PlayerEnter", "fields": [] },
                { "name": "PlayerLeave", "fields": [] }
            ],
            "entity_fields": [],
            "fills_gaps": true,
            "asset": false
        }],
        "resources": [],
        "events": [],
        "functions": [],
        "assets": []
    }))
    .expect("the schema reads")
}

fn scene_with_a_trigger(app: &mut App, tmp: &tempfile::TempDir) -> Entity {
    let scene = tmp.path().join("assets/timed.bsn");
    let path = scene.display().to_string();
    call(
        app,
        "scene.new",
        &[("kind", "3d".into()), ("path", PropertyValue::from(path))],
    );
    call(app, "entity.add.group", &[("name", "Gate".into())]);
    let group = named_entity(app, "Gate");
    call(
        app,
        "component.add",
        &[
            ("entity", PropertyValue::Entity(group)),
            ("type_path", TRIGGER.into()),
        ],
    );
    group
}

fn menu_items(app: &mut App) -> Vec<(Entity, String)> {
    let mut items = Vec::new();
    for entity in app
        .world_mut()
        .query::<Entity>()
        .iter(app.world())
        .collect::<Vec<_>>()
    {
        if app.world().get::<MenuItem>(entity).is_none() {
            continue;
        }
        let mut stack = vec![entity];
        let mut caption = String::new();
        while let Some(next) = stack.pop() {
            if let Some(text) = app.world().get::<Text>(next) {
                caption = text.0.clone();
                break;
            }
            if let Some(children) = app.world().get::<Children>(next) {
                stack.extend(children.iter());
            }
        }
        items.push((entity, caption));
    }
    items
}

fn authored_when(app: &App, entity: Entity) -> Option<String> {
    let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    let node = ast.ast_for(entity)?;
    match jackdaw_bsn::get_bsn_field(ast, node, TRIGGER, "when")? {
        BsnValue::Type(path) => Some(path),
        BsnValue::String(name) => Some(name),
        _ => None,
    }
}

#[test]
fn an_enum_field_on_a_project_component_is_editable() {
    let (mut app, tmp) = app_with_project_schema();
    let native = jackdaw::project_types::native_type_paths(
        &app.world().resource::<AppTypeRegistry>().read(),
    );
    app.world_mut()
        .resource_mut::<jackdaw::project_types::ProjectTypes>()
        .update(&schema_with_a_trigger(), &native);
    jackdaw::project_types::publish_document_only_types(app.world_mut());
    let group = scene_with_a_trigger(&mut app, &tmp);
    app.world_mut()
        .resource_scope(|world, mut selection: Mut<Selection>| {
            let mut commands = world.commands();
            selection.select_single(&mut commands, group);
        });
    app.world_mut().flush();
    settle(&mut app);

    let items = menu_items(&mut app);
    let captions: Vec<&str> = items.iter().map(|(_, caption)| caption.as_str()).collect();
    assert!(
        captions.contains(&"Interact")
            && captions.contains(&"PlayerEnter")
            && captions.contains(&"PlayerLeave"),
        "the menu offers the reported variants, got {captions:?}"
    );

    let enter = items
        .iter()
        .find(|(_, caption)| caption == "PlayerEnter")
        .map(|(entity, _)| *entity)
        .expect("the menu offers PlayerEnter");
    app.world_mut().trigger(Activate { entity: enter });
    settle(&mut app);

    assert_eq!(
        authored_when(&app, group).as_deref(),
        Some("definition_project::markers::ActionWhen::PlayerEnter"),
        "picking a variant authors it on the document"
    );
}
