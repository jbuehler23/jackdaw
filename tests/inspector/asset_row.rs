//! The inspector row beside a field that names an asset.
//!
//! A field holding a handle is shown as the path of the file behind it, with
//! Pick, Clear and New beside it and a drop target on the row, so an outfit's
//! material is chosen the way a material's texture is.

use std::path::{Path, PathBuf};

use bevy::camera::{NormalizedRenderTarget, RenderTarget};
use bevy::picking::backend::HitData;
use bevy::picking::events::{DragDrop, Pointer};
use bevy::picking::pointer::{Location, PointerButton, PointerId};
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, WindowRef};
use jackdaw::asset_browser::ActiveAssetDrag;
use jackdaw::commands::CommandHistory;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_feathers::button::{ButtonClickEvent, EditorButton};
use jackdaw_feathers::icons::Icon;
use jackdaw_feathers::picker::PickerItems;
use jackdaw_feathers::tooltip::Tooltip;
use jackdaw_scene_types::PropertyValue;

use crate::util;

const MATERIAL_TYPE: &str = "bevy_pbr::pbr_material::StandardMaterial";

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/definition_project")
}

fn settle(app: &mut App) {
    for _ in 0..6 {
        app.update();
    }
}

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

/// An editor with the fixture project open, two material files and an item
/// file under its assets, and an outfit open in the inspector.
fn app_with_open_outfit() -> (App, tempfile::TempDir) {
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
    std::fs::create_dir_all(tmp.path().join("assets/materials")).expect("a materials folder");
    std::fs::create_dir_all(tmp.path().join("assets/content")).expect("a content folder");

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

    for name in ["slate", "moss"] {
        call(
            &mut app,
            "asset.new",
            &[
                ("type", "material".into()),
                ("name", name.into()),
                ("path", "materials".into()),
            ],
        );
    }
    call(
        &mut app,
        "asset.new",
        &[
            ("type", "item".into()),
            ("name", "torch".into()),
            ("path", "content".into()),
        ],
    );
    call(
        &mut app,
        "asset.new",
        &[
            ("type", "outfit".into()),
            ("name", "ranger".into()),
            ("path", "content".into()),
        ],
    );
    (app, tmp)
}

fn open_outfit(app: &App) -> serde_json::Value {
    let path =
        jackdaw::definition_assets::open_definition_path(app.world()).expect("an asset is open");
    jackdaw::definition_assets::schema_definition_json(app.world(), "outfit", &path)
        .expect("the asset reads back")
}

fn all_entities(app: &mut App) -> Vec<Entity> {
    app.world_mut()
        .query::<Entity>()
        .iter(app.world())
        .collect()
}

/// Every asset row the inspector is showing, as the field it writes.
fn asset_rows(app: &mut App) -> Vec<(Entity, String, String)> {
    all_entities(app)
        .into_iter()
        .filter_map(|entity| {
            jackdaw::inspector::asset_field_shown_by(app.world(), entity)
                .map(|(asset, field)| (entity, asset.to_string(), field.to_string()))
        })
        .collect()
}

fn asset_row(app: &mut App, field_path: &str) -> Entity {
    let rows = asset_rows(app);
    rows.iter()
        .find(|(_, _, field)| field == field_path)
        .map(|(entity, _, _)| *entity)
        .unwrap_or_else(|| {
            let shown: Vec<&String> = rows.iter().map(|(_, _, field)| field).collect();
            let bound: Vec<String> = all_entities(app)
                .into_iter()
                .filter_map(|entity| {
                    jackdaw::inspector::field_edited_by(app.world(), entity)
                        .map(|(known, field)| format!("{known}.{field}"))
                })
                .collect();
            panic!("no asset row writes `{field_path}`; the inspector shows {shown:?} and binds {bound:?}")
        })
}

/// Every line of text under a row, for a row that draws its path.
fn row_text(app: &mut App, row: Entity) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![row];
    while let Some(entity) = stack.pop() {
        if let Some(text) = app.world().get::<Text>(entity) {
            found.push(text.0.clone());
        }
        if let Some(children) = app.world().get::<Children>(entity) {
            stack.extend(children.iter());
        }
    }
    found
}

/// The button under a row whose tooltip names this action.
fn row_button(app: &mut App, row: Entity, label: &str) -> Entity {
    let mut stack = vec![row];
    while let Some(entity) = stack.pop() {
        if app.world().get::<EditorButton>(entity).is_some()
            && app
                .world()
                .get::<Tooltip>(entity)
                .is_some_and(|tip| tip.title == label)
        {
            return entity;
        }
        if let Some(children) = app.world().get::<Children>(entity) {
            stack.extend(children.iter());
        }
    }
    panic!("no '{label}' button sits beside the row")
}

fn undo(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| {
            history.undo(world);
        });
    settle(app);
}

#[test]
fn a_handle_field_shows_the_path_it_names_rather_than_an_opaque_marker() {
    let (mut app, _tmp) = app_with_open_outfit();

    let row = asset_row(&mut app, "material");
    assert_eq!(
        jackdaw::inspector::asset_field_shown_by(app.world(), row).map(|(asset, _)| asset),
        Some(MATERIAL_TYPE),
        "the row knows which files the field can name",
    );
    assert!(
        row_text(&mut app, row).iter().any(|line| line == "None"),
        "a field naming nothing says so, got {:?}",
        row_text(&mut app, row),
    );

    call(
        &mut app,
        "asset.pick",
        &[
            ("field", "material".into()),
            ("value", "materials/slate.bsn".into()),
        ],
    );

    let row = asset_row(&mut app, "material");
    assert!(
        row_text(&mut app, row)
            .iter()
            .any(|line| line == "slate.bsn"),
        "the row shows the file it now names, got {:?}",
        row_text(&mut app, row),
    );
    assert_eq!(
        row_tooltips(&mut app, row)
            .into_iter()
            .find(|tip| tip.contains('/')),
        Some("materials/slate.bsn".to_string()),
        "and the whole path is there to hover",
    );
    assert!(
        !row_text(&mut app, row)
            .iter()
            .any(|line| line.contains("<opaque>")),
        "and nothing is left showing the handle as opaque",
    );
}

/// Every tooltip title under a row.
fn row_tooltips(app: &mut App, row: Entity) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![row];
    while let Some(entity) = stack.pop() {
        if let Some(tip) = app.world().get::<Tooltip>(entity) {
            found.push(tip.title.clone());
        }
        if let Some(children) = app.world().get::<Children>(entity) {
            stack.extend(children.iter());
        }
    }
    found
}

#[test]
fn the_three_actions_are_icons_that_say_what_they_do() {
    let (mut app, _tmp) = app_with_open_outfit();
    let row = asset_row(&mut app, "material");

    for (label, icon) in [
        ("Pick", Icon::FolderOpen),
        ("Clear", Icon::X),
        ("New", Icon::Plus),
    ] {
        let button = row_button(&mut app, row, label);
        let glyph = String::from(icon.unicode());
        assert!(
            row_text(&mut app, button).contains(&glyph),
            "{label} draws its own glyph rather than a caption, got {:?}",
            row_text(&mut app, button),
        );
        assert!(
            app.world()
                .get::<Tooltip>(button)
                .is_some_and(|tip| !tip.description.is_empty()),
            "{label} says what it does on hover",
        );
    }
}

#[test]
fn clicking_the_file_a_row_shows_opens_the_list_to_choose_from() {
    let (mut app, _tmp) = app_with_open_outfit();
    let row = asset_row(&mut app, "material");
    let value = jackdaw::inspector::asset_field_value_of(app.world(), row)
        .expect("the row draws the file it names");

    let target = primary_target(&mut app);
    app.world_mut().trigger(Pointer::new(
        PointerId::Mouse,
        Location {
            target,
            position: Vec2::ZERO,
        },
        bevy::picking::events::Click {
            button: PointerButton::Primary,
            hit: HitData::new(value, 0.0, None, None),
            duration: std::time::Duration::ZERO,
            count: 1,
        },
        value,
    ));
    settle(&mut app);

    assert!(
        app.world_mut()
            .query_filtered::<Entity, With<PickerItems<String>>>()
            .iter(app.world())
            .next()
            .is_some(),
        "the file itself opens the list, the way a resource field does",
    );
}

#[test]
fn the_picker_lists_only_the_files_the_field_can_name() {
    let (mut app, _tmp) = app_with_open_outfit();

    call(&mut app, "asset.pick", &[("field", "material".into())]);

    let listed: Vec<String> = all_entities(&mut app)
        .into_iter()
        .filter_map(|entity| app.world().get::<PickerItems<String>>(entity))
        .flat_map(|items| items.items().to_vec())
        .collect();
    assert_eq!(
        listed,
        vec![
            "materials/moss.bsn".to_string(),
            "materials/slate.bsn".to_string()
        ],
        "the picker offers the project's materials and nothing else",
    );
}

#[test]
fn picking_a_file_assigns_it_and_undo_puts_back_what_was_there() {
    let (mut app, _tmp) = app_with_open_outfit();

    call(
        &mut app,
        "asset.pick",
        &[
            ("field", "material".into()),
            ("value", "materials/slate.bsn".into()),
        ],
    );
    assert_eq!(open_outfit(&app)["material"], "materials/slate.bsn");

    call(
        &mut app,
        "asset.pick",
        &[
            ("field", "material".into()),
            ("value", "materials/moss.bsn".into()),
        ],
    );
    assert_eq!(open_outfit(&app)["material"], "materials/moss.bsn");

    undo(&mut app);

    assert_eq!(
        open_outfit(&app)["material"],
        "materials/slate.bsn",
        "undo puts back the file the field named before",
    );
}

#[test]
fn clearing_a_field_leaves_it_naming_nothing_and_undo_puts_the_path_back() {
    let (mut app, _tmp) = app_with_open_outfit();
    call(
        &mut app,
        "asset.pick",
        &[
            ("field", "material".into()),
            ("value", "materials/slate.bsn".into()),
        ],
    );

    call(&mut app, "asset.clear", &[("field", "material".into())]);

    assert_eq!(
        open_outfit(&app)["material"],
        "",
        "Clear leaves the field naming no file",
    );
    let row = asset_row(&mut app, "material");
    assert!(
        row_text(&mut app, row).iter().any(|line| line == "None"),
        "and the row says so, got {:?}",
        row_text(&mut app, row),
    );

    undo(&mut app);

    assert_eq!(
        open_outfit(&app)["material"],
        "materials/slate.bsn",
        "undo puts the path back",
    );
}

#[test]
fn new_writes_a_file_beside_the_open_asset_and_assigns_it() {
    let (mut app, tmp) = app_with_open_outfit();
    let row = asset_row(&mut app, "material");

    let new = row_button(&mut app, row, "New");
    app.world_mut().trigger(ButtonClickEvent { entity: new });
    settle(&mut app);

    let written = open_outfit(&app)["material"]
        .as_str()
        .expect("the field names a file")
        .to_string();
    assert!(
        written.starts_with("content/"),
        "New wrote beside the open asset, got {written}",
    );
    assert!(
        tmp.path().join("assets").join(&written).is_file(),
        "and the file it named is on disk",
    );
}

#[test]
fn a_drop_of_a_file_of_another_type_leaves_the_field_as_it_was() {
    let (mut app, tmp) = app_with_open_outfit();
    call(
        &mut app,
        "asset.pick",
        &[
            ("field", "material".into()),
            ("value", "materials/slate.bsn".into()),
        ],
    );
    let row = asset_row(&mut app, "material");

    drop_file_on(&mut app, row, tmp.path().join("assets/content/torch.bsn"));

    assert_eq!(
        open_outfit(&app)["material"],
        "materials/slate.bsn",
        "a file holding something else is refused",
    );
    assert!(
        !app.world()
            .resource::<jackdaw::status_bar::StatusNotice>()
            .text()
            .is_empty(),
        "and the refusal is said out loud",
    );
}

#[test]
fn a_drop_of_a_file_the_field_can_name_assigns_it() {
    let (mut app, tmp) = app_with_open_outfit();
    let row = asset_row(&mut app, "material");

    drop_file_on(&mut app, row, tmp.path().join("assets/materials/moss.bsn"));

    assert_eq!(
        open_outfit(&app)["material"],
        "materials/moss.bsn",
        "the dropped file is what the field now names",
    );
}

/// The window a synthetic pointer event is aimed at.
fn primary_target(app: &mut App) -> NormalizedRenderTarget {
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(app.world())
        .expect("headless apps still have a primary window");
    RenderTarget::Window(WindowRef::Primary)
        .normalize(Some(window))
        .expect("the primary window normalizes")
}

/// Drag a file out of the asset browser and drop it on a row.
fn drop_file_on(app: &mut App, row: Entity, file: PathBuf) {
    app.world_mut().resource_mut::<ActiveAssetDrag>().path = Some(file);
    let target = primary_target(app);
    let dropped = app.world_mut().spawn_empty().id();
    app.world_mut().trigger(Pointer::new(
        PointerId::Mouse,
        Location {
            target,
            position: Vec2::ZERO,
        },
        DragDrop {
            button: PointerButton::Primary,
            dropped,
            hit: HitData::new(row, 0.0, None, None),
        },
        row,
    ));
    settle(app);
}

#[test]
fn a_list_of_material_paths_shows_one_row_per_element() {
    let (mut app, _tmp) = app_with_open_outfit();
    call(
        &mut app,
        "asset.set",
        &[
            ("field", "materials".into()),
            (
                "value",
                serde_json::json!(["materials/slate.bsn", "materials/moss.bsn"])
                    .to_string()
                    .into(),
            ),
        ],
    );

    let mut fields: Vec<String> = asset_rows(&mut app)
        .into_iter()
        .map(|(_, _, field)| field)
        .filter(|field| field.starts_with("materials["))
        .collect();
    fields.sort();
    fields.dedup();
    assert_eq!(
        fields,
        vec!["materials[0]".to_string(), "materials[1]".to_string()],
        "each element of the list has a row of its own",
    );

    call(
        &mut app,
        "asset.pick",
        &[
            ("field", "materials[1]".into()),
            ("value", "materials/slate.bsn".into()),
        ],
    );
    assert_eq!(
        open_outfit(&app)["materials"],
        serde_json::json!(["materials/slate.bsn", "materials/slate.bsn"]),
        "Pick on one element writes only that element",
    );

    call(&mut app, "asset.clear", &[("field", "materials[0]".into())]);
    assert_eq!(
        open_outfit(&app)["materials"],
        serde_json::json!(["", "materials/slate.bsn"]),
        "and Clear empties only the element it stands beside",
    );
}

/// Build the card again, the way reopening the file does, so a row whose kind
/// follows the value is decided against what the value now holds.
fn reopen_outfit(app: &mut App) {
    let path =
        jackdaw::definition_assets::open_definition_path(app.world()).expect("an asset is open");
    let path = path.to_string_lossy().into_owned();
    call(app, "asset.open", &[("path", path.into())]);
}

#[test]
fn a_material_a_string_field_names_gets_the_row_a_handle_gets() {
    let (mut app, _tmp) = app_with_open_outfit();
    call(
        &mut app,
        "asset.set",
        &[
            ("field", "skin".into()),
            ("value", "materials/slate.bsn".into()),
        ],
    );
    reopen_outfit(&mut app);

    let row = asset_row(&mut app, "skin");
    assert!(
        row_text(&mut app, row)
            .iter()
            .any(|line| line == "slate.bsn"),
        "the row shows the file the string names",
    );

    call(
        &mut app,
        "asset.pick",
        &[
            ("field", "skin".into()),
            ("value", "materials/moss.bsn".into()),
        ],
    );
    assert_eq!(
        open_outfit(&app)["skin"],
        serde_json::json!("materials/moss.bsn"),
        "and picking writes the path the field spells",
    );
}

#[test]
fn a_string_naming_no_file_stays_a_plain_text_row() {
    let (mut app, _tmp) = app_with_open_outfit();
    call(
        &mut app,
        "asset.set",
        &[("field", "skin".into()), ("value", "ranger".into())],
    );
    reopen_outfit(&mut app);

    assert!(
        !asset_rows(&mut app)
            .iter()
            .any(|(_, _, field)| field == "skin"),
        "a string that names nothing the project holds is no asset field",
    );
}

#[test]
fn an_empty_element_beside_a_path_still_offers_the_picker() {
    let (mut app, _tmp) = app_with_open_outfit();
    call(
        &mut app,
        "asset.set",
        &[
            ("field", "skins".into()),
            (
                "value",
                serde_json::json!(["materials/slate.bsn", ""])
                    .to_string()
                    .into(),
            ),
        ],
    );
    reopen_outfit(&mut app);

    let mut fields: Vec<String> = asset_rows(&mut app)
        .into_iter()
        .map(|(_, _, field)| field)
        .filter(|field| field.starts_with("skins["))
        .collect();
    fields.sort();
    fields.dedup();
    assert_eq!(
        fields,
        vec!["skins[0]".to_string(), "skins[1]".to_string()],
        "the element holding nothing takes its kind from the list it sits in",
    );

    call(
        &mut app,
        "asset.pick",
        &[
            ("field", "skins[1]".into()),
            ("value", "materials/moss.bsn".into()),
        ],
    );
    assert_eq!(
        open_outfit(&app)["skins"],
        serde_json::json!(["materials/slate.bsn", "materials/moss.bsn"]),
        "and the empty element takes the file picked for it",
    );
}

#[test]
fn a_second_pick_replaces_the_list_the_first_put_up() {
    let (mut app, _tmp) = app_with_open_outfit();
    call(&mut app, "asset.pick", &[("field", "material".into())]);
    call(&mut app, "asset.pick", &[("field", "material".into())]);

    let open = app
        .world_mut()
        .query_filtered::<Entity, With<PickerItems<String>>>()
        .iter(app.world())
        .count();
    assert_eq!(open, 1, "one list is open, not one per click");
}

#[test]
fn clearing_a_field_no_row_shows_is_refused_rather_than_reported_as_done() {
    let (mut app, _tmp) = app_with_open_outfit();

    let result = app
        .world_mut()
        .operator("asset.clear")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        })
        .param("field", PropertyValue::from("nothing_shows_this"))
        .call()
        .expect("the operator dispatched");
    assert_eq!(
        result,
        OperatorResult::Cancelled,
        "a caller naming a field the inspector is not showing is told so",
    );
}
