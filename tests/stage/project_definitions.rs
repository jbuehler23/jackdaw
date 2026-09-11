//! Asset kinds the open project brings with it.
//!
//! The editor never compiles the project's code, so an item is edited from the
//! schema the last build reported: every type the game registers as a
//! reflected asset is a kind, named after itself, declared nowhere. Its files
//! say which type they hold, live in any folder, and are created, edited,
//! saved and listed through the same operators a compiled kind uses.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw::asset_index::{AssetEntry, AssetIndex};
use jackdaw::definition_assets::OpenDefinition;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::AssetKindSource;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext, OperatorReports};
use jackdaw_commands::CommandHistory;
use jackdaw_scene_types::PropertyValue;

use crate::util;

const ITEM_TYPE: &str = "definition_project::content::ItemDef";

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/definition_project")
}

/// A project of its own carrying the fixture's reported schema.
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
    tmp
}

/// An editor with that project open and its schema picked up, which is what a
/// build leaves behind.
fn editor_on(root: &Path) -> App {
    let mut app = util::editor_test_app();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: root.to_path_buf(),
            config: default(),
        });
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    jackdaw::pie::refresh_project_types(app.world_mut());
    for _ in 0..3 {
        app.update();
    }
    app
}

fn editor_with_items() -> (App, tempfile::TempDir) {
    let tmp = project_copy();
    let app = editor_on(tmp.path());
    (app, tmp)
}

#[track_caller]
fn call(app: &mut App, id: &'static str, params: &[(&'static str, PropertyValue)]) {
    let mut call = app.world_mut().operator(id).settings(CallOperatorSettings {
        execution_context: ExecutionContext::Invoke,
        creates_history_entry: true,
    });
    for (key, value) in params {
        call = call.param(*key, value.clone());
    }
    let result = call.call().expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Finished, "{id} did not finish");
    app.update();
}

fn item_path(tmp: &tempfile::TempDir, name: &str) -> PathBuf {
    tmp.path().join(format!("assets/{name}.bsn"))
}

/// The material type as a game registering it as an asset would report it.
fn material_schema() -> jackdaw_schema::TypeSchema {
    jackdaw_schema::TypeSchema {
        type_path: "bevy_pbr::pbr_material::StandardMaterial".to_string(),
        short_name: "StandardMaterial".to_string(),
        module_path: "bevy_pbr::pbr_material".to_string(),
        category: String::new(),
        description: String::new(),
        editor_description: String::new(),
        hidden: false,
        asset: true,
        preview: String::new(),
        default_constructible: true,
        fields: Vec::new(),
        kind: jackdaw_schema::TypeKind::Struct,
        default: None,
        variants: Vec::new(),
        entity_fields: Vec::new(),
        fills_gaps: true,
    }
}

/// The names the index gives a kind's files, sorted.
fn names_of(app: &App, kind: &str) -> Vec<String> {
    let mut names: Vec<String> = app
        .world()
        .resource::<AssetIndex>()
        .of_kind(kind)
        .map(AssetEntry::name)
        .collect();
    names.sort();
    names
}

/// The open definition as JSON: what its file authors over what its type
/// defaults to.
fn open_item(app: &App) -> serde_json::Value {
    let path = jackdaw::definition_assets::open_definition_path(app.world())
        .expect("a definition is open");
    jackdaw::definition_assets::schema_definition_json(app.world(), "item", &path)
        .expect("the definition reads back")
}

#[test]
fn a_schema_asset_type_registers_a_kind_with_nothing_declared() {
    let (app, _tmp) = editor_with_items();

    let kinds = app.world().resource::<AssetKinds>();
    let item = kinds
        .by_kind("item")
        .expect("the type the game registers as an asset is a kind");
    assert_eq!(item.type_path, ITEM_TYPE);
    assert_eq!(item.label, "Item", "named after its type, not a manifest");
    assert_eq!(item.source, AssetKindSource::Schema);
    assert!(
        item.schema_backed(),
        "the editor has no registration for the type, only the schema"
    );
}

#[test]
fn a_kind_the_editor_compiled_in_wins_over_the_schemas() {
    let (mut app, _tmp) = editor_with_items();
    let native = jackdaw::project_types::native_type_paths(
        &app.world().resource::<AppTypeRegistry>().read(),
    );
    app.world_mut()
        .resource_mut::<jackdaw::project_types::ProjectTypes>()
        .update(
            &jackdaw_schema::ProjectSchema {
                assets: vec![material_schema()],
                ..default()
            },
            &native,
        );
    jackdaw::project_definitions::register_project_definitions(app.world_mut());
    app.update();

    let kinds = app.world().resource::<AssetKinds>();
    let material = kinds
        .by_kind(jackdaw::definition_assets::MATERIAL_KIND)
        .expect("the editor's own material kind is still registered");
    assert_eq!(material.source, AssetKindSource::Compiled);
    assert!(
        !material.schema_backed(),
        "a material is still edited through the store the editor loads it into"
    );
}

#[test]
fn a_compiled_kind_survives_the_project_that_reported_its_type() {
    let (mut app, _tmp) = editor_with_items();
    let native = jackdaw::project_types::native_type_paths(
        &app.world().resource::<AppTypeRegistry>().read(),
    );
    app.world_mut()
        .resource_mut::<jackdaw::project_types::ProjectTypes>()
        .update(
            &jackdaw_schema::ProjectSchema {
                assets: vec![material_schema()],
                ..default()
            },
            &native,
        );
    jackdaw::project_definitions::register_project_definitions(app.world_mut());

    jackdaw::project_definitions::forget_project_definitions(app.world_mut());

    assert!(
        app.world()
            .resource::<AssetKinds>()
            .by_kind(jackdaw::definition_assets::MATERIAL_KIND)
            .is_some(),
        "closing a project does not take the editor's own kind with it"
    );
}

#[test]
fn a_definition_is_created_edited_and_saved_through_its_operators() {
    let (mut app, tmp) = editor_with_items();

    call(
        &mut app,
        "asset.new",
        &[("type", "item".into()), ("name", "torch".into())],
    );
    let path = item_path(&tmp, "torch");
    assert!(path.is_file(), "asset.new writes the file at {path:?}");

    call(
        &mut app,
        "asset.set",
        &[("field", "stack_size".into()), ("value", "12".into())],
    );
    call(
        &mut app,
        "asset.set",
        &[("field", "rarity".into()), ("value", "Rare".into())],
    );
    call(
        &mut app,
        "asset.set",
        &[
            ("field", "loot".into()),
            ("value", r#"[{"item":"coin","weight":3}]"#.into()),
        ],
    );

    let edited = open_item(&app);
    assert_eq!(edited["stack_size"], 12);
    assert_eq!(edited["rarity"], "Rare");
    assert_eq!(edited["loot"][0]["item"], "coin");
    assert_eq!(edited["loot"][0]["weight"], 3);

    call(&mut app, "asset.save", &[]);
    let written = std::fs::read_to_string(&path).expect("the file reads");
    assert!(written.contains("stack_size: 12"), "got:\n{written}");
    assert!(written.contains("Rarity::Rare"), "got:\n{written}");
    assert!(written.contains("coin"), "got:\n{written}");
    assert!(
        written.contains(ITEM_TYPE),
        "the file names the type it holds, got:\n{written}"
    );
}

#[test]
fn a_saved_definition_carries_its_type_in_its_header() {
    let (mut app, tmp) = editor_with_items();
    call(
        &mut app,
        "asset.new",
        &[("type", "item".into()), ("name", "torch".into())],
    );

    let written = std::fs::read_to_string(item_path(&tmp, "torch")).expect("the file reads");
    assert_eq!(
        jackdaw_bsn::read_asset_header(&written).as_deref(),
        Some(ITEM_TYPE),
        "the file says what it holds, got:\n{written}"
    );
}

#[test]
fn a_new_definition_lands_in_the_folder_it_was_asked_for_and_opens_from_there() {
    let (mut app, tmp) = editor_with_items();
    let folder = tmp.path().join("assets/content/gear");
    std::fs::create_dir_all(&folder).expect("the directory is made");

    call(
        &mut app,
        "asset.new",
        &[
            ("type", "item".into()),
            ("name", "torch".into()),
            ("path", folder.to_string_lossy().into_owned().into()),
        ],
    );

    let path = folder.join("torch.bsn");
    assert!(path.is_file(), "asset.new writes the file at {path:?}");
    call(
        &mut app,
        "asset.set",
        &[("field", "stack_size".into()), ("value", "12".into())],
    );
    call(&mut app, "asset.save", &[]);

    jackdaw::definition_assets::close_open_definition(app.world_mut());
    app.update();
    call(
        &mut app,
        "asset.open",
        &[("path", path.to_string_lossy().into_owned().into())],
    );

    assert_eq!(
        open_item(&app)["stack_size"],
        12,
        "a file in any folder opens by its path"
    );
}

#[test]
fn a_new_definition_asked_for_by_file_path_takes_that_name_and_folder() {
    let (mut app, tmp) = editor_with_items();

    call(
        &mut app,
        "asset.new",
        &[
            ("type", "item".into()),
            ("path", "content/lantern.bsn".into()),
        ],
    );

    let path = tmp.path().join("assets/content/lantern.bsn");
    assert!(path.is_file(), "asset.new writes the file at {path:?}");
    assert_eq!(
        names_of(&app, "item"),
        vec!["lantern".to_string()],
        "the file the caller named is the name it goes by"
    );
}

#[test]
fn creating_a_definition_over_a_file_that_is_already_there_is_refused() {
    let (mut app, tmp) = editor_with_items();
    let path = tmp.path().join("assets/content/lantern.bsn");
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("the directory is made");
    std::fs::write(&path, "already here").expect("the file is written");

    call(
        &mut app,
        "asset.new",
        &[
            ("type", "item".into()),
            ("path", "content/lantern.bsn".into()),
        ],
    );

    assert_eq!(
        std::fs::read_to_string(&path).expect("the file reads"),
        "already here",
        "asset.new does not write over a file that is already there"
    );
    assert!(app.world().resource::<OpenDefinition>().0.is_none());
}

#[test]
fn an_asset_file_in_another_folder_is_found_by_the_scan() {
    let (app, tmp) = editor_with_items();
    let folder = tmp.path().join("assets/somewhere/deep");
    std::fs::create_dir_all(&folder).expect("the directory is made");
    std::fs::write(
        folder.join("lantern.bsn"),
        format!("#lantern {ITEM_TYPE} {{ stack_size: 3 }}\n"),
    )
    .expect("the file is written");
    drop(app);

    let reopened = editor_on(tmp.path());

    assert_eq!(
        names_of(&reopened, "item"),
        vec!["lantern".to_string()],
        "a file says what it is wherever it sits"
    );
}

#[test]
fn a_saved_definition_is_listed_and_read_back_when_the_project_is_opened_again() {
    let (mut app, tmp) = editor_with_items();
    call(
        &mut app,
        "asset.new",
        &[("type", "item".into()), ("name", "torch".into())],
    );
    call(
        &mut app,
        "asset.set",
        &[("field", "stack_size".into()), ("value", "12".into())],
    );
    call(
        &mut app,
        "asset.set",
        &[("field", "rarity".into()), ("value", "Rare".into())],
    );
    call(&mut app, "asset.save", &[]);
    drop(app);

    let reopened = editor_on(tmp.path());
    assert_eq!(
        names_of(&reopened, "item"),
        vec!["torch".to_string()],
        "the scan lists what the project holds"
    );
    let read_back = jackdaw::definition_assets::schema_definition_json(
        reopened.world(),
        "item",
        std::path::Path::new("torch.bsn"),
    )
    .expect("the definition reads back");
    assert_eq!(read_back["stack_size"], 12);
    assert_eq!(read_back["rarity"], "Rare");
}

#[test]
fn a_field_set_back_to_its_default_stops_being_written_at_all() {
    let (mut app, tmp) = editor_with_items();
    call(
        &mut app,
        "asset.new",
        &[("type", "item".into()), ("name", "torch".into())],
    );
    call(
        &mut app,
        "asset.set",
        &[("field", "stack_size".into()), ("value", "12".into())],
    );
    call(
        &mut app,
        "asset.set",
        &[("field", "stack_size".into()), ("value", "1".into())],
    );
    call(&mut app, "asset.save", &[]);

    let written = std::fs::read_to_string(item_path(&tmp, "torch")).expect("the file reads");
    assert!(
        !written.contains("stack_size"),
        "a field holding what the type defaults to is not a change, got:\n{written}"
    );
}

#[test]
fn undo_takes_back_one_edit_to_a_definition() {
    let (mut app, _tmp) = editor_with_items();
    call(
        &mut app,
        "asset.new",
        &[("type", "item".into()), ("name", "torch".into())],
    );
    call(
        &mut app,
        "asset.set",
        &[("field", "rarity".into()), ("value", "Epic".into())],
    );
    assert_eq!(open_item(&app)["rarity"], "Epic");

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| {
            history.undo(world);
        });

    assert_eq!(
        open_item(&app)["rarity"],
        "Common",
        "undo restores what the field held before the edit"
    );
}

#[test]
fn a_definition_edit_and_a_scene_edit_undo_in_the_order_they_were_made() {
    let (mut app, _tmp) = editor_with_items();
    let node = app
        .world_mut()
        .spawn((Name::new("Target"), Node::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), node);
    app.update();

    call(
        &mut app,
        "asset.new",
        &[("type", "item".into()), ("name", "torch".into())],
    );
    call(
        &mut app,
        "asset.set",
        &[("field", "stack_size".into()), ("value", "12".into())],
    );
    let result = app
        .world_mut()
        .operator("field.set")
        .param("entity", node)
        .param("type_path", Node::type_path().to_string())
        .param("field", "width".to_string())
        .param("value", "{\"Px\":120.0}".to_string())
        .call()
        .expect("field.set dispatches");
    assert_eq!(result, OperatorResult::Finished);
    app.update();
    assert_eq!(
        app.world().get::<Node>(node).map(|node| node.width),
        Some(Val::Px(120.0))
    );

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| {
            history.undo(world);
        });
    assert_eq!(
        app.world().get::<Node>(node).map(|node| node.width),
        Some(Val::Auto),
        "the scene edit goes back first"
    );
    assert_eq!(open_item(&app)["stack_size"], 12, "and takes nothing else");

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| {
            history.undo(world);
        });
    assert_eq!(
        open_item(&app)["stack_size"],
        1,
        "then the definition edit goes back"
    );
}

#[test]
fn a_variant_the_type_does_not_have_is_refused() {
    let (mut app, _tmp) = editor_with_items();
    call(
        &mut app,
        "asset.new",
        &[("type", "item".into()), ("name", "torch".into())],
    );
    call(
        &mut app,
        "asset.set",
        &[("field", "rarity".into()), ("value", "Mythic".into())],
    );

    assert_eq!(
        open_item(&app)["rarity"],
        "Common",
        "the schema's variant list is what the field accepts"
    );
    assert!(
        app.world()
            .resource::<CommandHistory>()
            .undo_stack
            .is_empty(),
        "a refused value leaves nothing for undo to walk back over"
    );
}

#[test]
fn a_kind_whose_type_leaves_the_schema_takes_its_entries_and_its_card_with_it() {
    let (mut app, _tmp) = editor_with_items();
    call(
        &mut app,
        "asset.new",
        &[("type", "item".into()), ("name", "torch".into())],
    );
    assert!(app.world().resource::<OpenDefinition>().0.is_some());

    let native = jackdaw::project_types::native_type_paths(
        &app.world().resource::<AppTypeRegistry>().read(),
    );
    app.world_mut()
        .resource_mut::<jackdaw::project_types::ProjectTypes>()
        .update(&jackdaw_schema::ProjectSchema::default(), &native);
    jackdaw::project_definitions::register_project_definitions(app.world_mut());
    for _ in 0..3 {
        app.update();
    }

    assert!(
        app.world()
            .resource::<AssetKinds>()
            .by_kind("item")
            .is_none(),
        "a type the schema no longer carries is not a kind the editor can edit"
    );
    assert!(app.world().resource::<OpenDefinition>().0.is_none());
    assert!(
        names_of(&app, "item").is_empty(),
        "and the index lets go of what its files held"
    );
}

#[test]
fn a_file_whose_fields_the_schema_no_longer_declares_keeps_the_ones_it_does() {
    let (mut app, tmp) = editor_with_items();
    let path = item_path(&tmp, "torch");
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("the directory is made");
    std::fs::write(
        &path,
        format!("#torch {ITEM_TYPE} {{ stack_size: 9, charges: 4 }}\n"),
    )
    .expect("the file is written");
    for _ in 0..3 {
        app.update();
    }

    call(
        &mut app,
        "asset.open",
        &[("path", path.to_string_lossy().into_owned().into())],
    );

    let opened = open_item(&app);
    assert_eq!(
        opened["stack_size"], 9,
        "the fields the schema declares still read"
    );
    assert!(
        opened.get("charges").is_none(),
        "and one it does not is not offered as a field"
    );
}

#[test]
fn a_field_the_schema_no_longer_declares_survives_an_edit_and_a_save() {
    let (mut app, tmp) = editor_with_items();
    let path = item_path(&tmp, "torch");
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("the directory is made");
    std::fs::write(
        &path,
        format!("#torch {ITEM_TYPE} {{ stack_size: 9, charges: 4 }}\n"),
    )
    .expect("the file is written");
    for _ in 0..3 {
        app.update();
    }

    call(
        &mut app,
        "asset.open",
        &[("path", path.to_string_lossy().into_owned().into())],
    );
    call(
        &mut app,
        "asset.set",
        &[("field", "stack_size".into()), ("value", "12".into())],
    );
    call(&mut app, "asset.save", &[]);

    let written = std::fs::read_to_string(&path).expect("the file reads");
    assert!(written.contains("stack_size: 12"), "got:\n{written}");
    assert!(
        written.contains("charges: 4"),
        "a field the editor cannot show is written back as it was, got:\n{written}"
    );
}

#[test]
fn a_kind_reports_what_the_project_holds() {
    let (mut app, _tmp) = editor_with_items();
    call(
        &mut app,
        "asset.new",
        &[("type", "item".into()), ("name", "torch".into())],
    );
    call(
        &mut app,
        "asset.new",
        &[("type", "item".into()), ("name", "lantern".into())],
    );

    assert_eq!(
        names_of(&app, "item"),
        vec!["lantern".to_string(), "torch".to_string()]
    );

    app.world_mut()
        .get_resource_or_init::<OperatorReports>()
        .0
        .clear();
    call(&mut app, "asset.list", &[("type", "item".into())]);
    assert_eq!(
        app.world().resource::<OperatorReports>().0,
        vec!["item: lantern.bsn, torch.bsn".to_string()],
        "asset.list answers for a kind the schema brought, by path"
    );
}

/// A colour on a project component is written the way a person says one. The
/// editor knows `Color` even when it does not know the component holding it.
#[test]
fn a_colour_field_on_a_project_component_takes_channels() {
    let mut app = util::editor_test_app();
    let day_cycle = "definition_project::world::DayCycle";
    let schema = jackdaw_schema::ProjectSchema {
        components: vec![jackdaw_schema::TypeSchema {
            type_path: day_cycle.to_string(),
            short_name: "DayCycle".to_string(),
            module_path: "definition_project::world".to_string(),
            category: String::new(),
            description: String::new(),
            editor_description: String::new(),
            hidden: false,
            asset: false,
            preview: String::new(),
            default_constructible: true,
            fields: vec![jackdaw_schema::FieldSchema {
                name: "night_color".to_string(),
                type_path: "bevy_color::color::Color".to_string(),
                item_type_path: String::new(),
            }],
            kind: jackdaw_schema::TypeKind::Struct,
            default: None,
            variants: Vec::new(),
            entity_fields: Vec::new(),
            fills_gaps: true,
        }],
        ..default()
    };
    let native = jackdaw::project_types::native_type_paths(
        &app.world().resource::<AppTypeRegistry>().read(),
    );
    app.world_mut()
        .resource_mut::<jackdaw::project_types::ProjectTypes>()
        .update(&schema, &native);
    jackdaw::project_types::publish_document_only_types(app.world_mut());

    let entity = app
        .world_mut()
        .spawn((Name::new("Sky"), Node::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    app.update();

    let result = app
        .world_mut()
        .operator("component.set")
        .param("entity", entity)
        .param("type_path", day_cycle.to_string())
        .param("field", "night_color".to_string())
        .param("value", "0.1,0.2,0.4".to_string())
        .call()
        .expect("component.set dispatches");
    assert_eq!(result, OperatorResult::Finished);
    app.update();

    let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    let node = ast.ast_for(entity).expect("the entity is in the document");
    let authored = jackdaw_bsn::get_bsn_field(ast, node, day_cycle, "night_color")
        .expect("the document authors the field");
    let spelled = format!("{authored:?}");
    assert!(
        spelled.contains("Srgba") && spelled.contains("0.1"),
        "the colour is authored as a colour, not as the text it was typed as: {spelled}"
    );
}

/// A hand-authored item file, spelled the way the emitter writes one so a save
/// that changes nothing changes no line.
const AUTHORED_TORCH: &str = "\
#torch
definition_project::content::ItemDef {
    stack_size: 12,
    rarity: definition_project::content::Rarity::Rare,
    loot: [
        definition_project::content::LootRoll {
            item: \"coin\",
            weight: 3,
        },
        definition_project::content::LootRoll {
            item: \"gem\",
            weight: 7,
        },
    ],
}
";

fn authored_torch() -> String {
    jackdaw::asset_files::asset_file_text(ITEM_TYPE, AUTHORED_TORCH)
}

/// A project holding that file under a name carrying a kind segment.
fn editor_on_an_authored_torch() -> (App, tempfile::TempDir, PathBuf) {
    let tmp = project_copy();
    let path = tmp.path().join("assets/content/torch.item.bsn");
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("the directory is made");
    std::fs::write(&path, authored_torch()).expect("the file is written");
    let app = editor_on(tmp.path());
    (app, tmp, path)
}

#[test]
fn a_re_saved_definition_keeps_the_root_name_its_file_carries() {
    let (mut app, _tmp, path) = editor_on_an_authored_torch();
    call(
        &mut app,
        "asset.open",
        &[("path", path.to_string_lossy().into_owned().into())],
    );

    call(
        &mut app,
        "asset.set",
        &[("field", "stack_size".into()), ("value", "20".into())],
    );
    call(&mut app, "asset.save", &[]);

    let written = std::fs::read_to_string(&path).expect("the file reads");
    assert!(
        written.contains("#torch\n"),
        "the root is still the one the file named, got:\n{written}"
    );
}

#[test]
fn a_new_definition_goes_by_the_file_it_was_asked_for_rather_than_the_name() {
    let (mut app, tmp) = editor_with_items();

    call(
        &mut app,
        "asset.new",
        &[
            ("type", "item".into()),
            ("name", "torch".into()),
            ("path", "content/lantern.item.bsn".into()),
        ],
    );

    assert!(
        tmp.path().join("assets/content/lantern.item.bsn").is_file(),
        "the file the caller named is the file that is written"
    );
    assert_eq!(
        names_of(&app, "item"),
        vec!["lantern".to_string()],
        "and the name is the one a scan would read back from it"
    );
}

#[test]
fn a_re_saved_definition_keeps_a_quoted_root_name_quoted() {
    let tmp = project_copy();
    let path = tmp.path().join("assets/content/torch.item.bsn");
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("the directory is made");
    let quoted = AUTHORED_TORCH.replace("#torch", "#\"torch.item\"");
    std::fs::write(
        &path,
        jackdaw::asset_files::asset_file_text(ITEM_TYPE, &quoted),
    )
    .expect("the file is written");
    let mut app = editor_on(tmp.path());
    call(
        &mut app,
        "asset.open",
        &[("path", path.to_string_lossy().into_owned().into())],
    );

    call(
        &mut app,
        "asset.set",
        &[("field", "stack_size".into()), ("value", "20".into())],
    );
    call(&mut app, "asset.save", &[]);

    let written = std::fs::read_to_string(&path).expect("the file reads");
    assert!(
        written.contains("#\"torch.item\"\n"),
        "the name the file spells survives the save, got:\n{written}"
    );
}

#[test]
fn a_definition_is_named_by_the_stem_before_the_first_dot_of_its_file() {
    let (app, _tmp, _path) = editor_on_an_authored_torch();

    assert_eq!(
        names_of(&app, "item"),
        vec!["torch".to_string()],
        "the kind segment the file carries is no part of the name"
    );
}

#[test]
fn a_new_definition_asked_for_by_a_file_path_drops_its_kind_segment() {
    let (mut app, tmp) = editor_with_items();

    call(
        &mut app,
        "asset.new",
        &[
            ("type", "item".into()),
            ("path", "content/lantern.item.bsn".into()),
        ],
    );

    let path = tmp.path().join("assets/content/lantern.item.bsn");
    assert!(path.is_file(), "asset.new writes the file at {path:?}");
    assert_eq!(names_of(&app, "item"), vec!["lantern".to_string()],);
    let written = std::fs::read_to_string(&path).expect("the file reads");
    assert!(
        written.contains("#lantern\n"),
        "and its root is that name, got:\n{written}"
    );
}

#[test]
fn editing_one_field_of_a_loaded_file_rewrites_one_line() {
    let (mut app, _tmp, path) = editor_on_an_authored_torch();
    call(
        &mut app,
        "asset.open",
        &[("path", path.to_string_lossy().into_owned().into())],
    );
    let before = authored_torch();

    call(
        &mut app,
        "asset.set",
        &[("field", "stack_size".into()), ("value", "20".into())],
    );
    call(&mut app, "asset.save", &[]);

    let after = std::fs::read_to_string(&path).expect("the file reads");
    let before_lines: Vec<&str> = before.lines().collect();
    let after_lines: Vec<&str> = after.lines().collect();
    assert_eq!(
        before_lines.len(),
        after_lines.len(),
        "the file keeps its shape, got:\n{after}"
    );
    let changed: Vec<(&str, &str)> = before_lines
        .iter()
        .zip(&after_lines)
        .filter(|(before, after)| before != after)
        .map(|(before, after)| (*before, *after))
        .collect();
    assert_eq!(
        changed,
        vec![("    stack_size: 12,", "    stack_size: 20,")],
        "only the field that was edited moved, got:\n{after}"
    );
}
