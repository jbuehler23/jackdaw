//! Asset files: a registered asset type edited as files in the project.
//!
//! A file says what it holds and can sit in any folder; its path is what the
//! index keys it by. Files are created, edited, saved and listed through the
//! same operators whether the edit comes from the inspector or from a caller
//! with no viewport.

use crate::util;

use std::path::PathBuf;

use bevy::asset::{Asset, Assets};
use bevy::prelude::*;
use jackdaw::asset_index::{AssetIndex, AssetValue};
use jackdaw::definition_assets::{MATERIAL_KIND, OpenDefinition};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_commands::CommandHistory;
use jackdaw_scene_types::PropertyValue;

#[derive(Reflect, Clone, Default, PartialEq, Debug)]
#[reflect(Default)]
struct LootRoll {
    item: String,
    weight: u32,
}

#[derive(Reflect, Clone, Default, PartialEq, Debug)]
#[reflect(Default)]
enum Rarity {
    #[default]
    Common,
    Rare,
}

#[derive(Asset, Reflect, Clone, Default)]
#[reflect(Default)]
struct ItemDef {
    stack_size: u32,
    rarity: Rarity,
    loot: Vec<LootRoll>,
}

fn item_type() -> AssetKind {
    AssetKind::extension("item", "Item", ItemDef::type_path())
}

fn items_dir(tmp: &tempfile::TempDir) -> PathBuf {
    let dir = tmp.path().join("assets/content/items");
    std::fs::create_dir_all(&dir).expect("the directory is made");
    dir
}

fn item_path(tmp: &tempfile::TempDir, name: &str) -> PathBuf {
    jackdaw::definition_assets::definition_file_path(&items_dir(tmp), name)
}

/// An editor with a project of its own and one definition type registered.
fn editor_with_items() -> (App, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut app = util::editor_test_app();
    app.init_asset::<ItemDef>();
    app.register_asset_reflect::<ItemDef>();
    app.register_type::<ItemDef>();
    app.register_type::<LootRoll>();
    app.register_type::<Rarity>();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: tmp.path().to_path_buf(),
            config: default(),
        });
    app.world_mut()
        .resource_mut::<AssetKinds>()
        .register(item_type());
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
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

/// Save a live material through the operator that owns it, which is what puts
/// it in the index, the catalog and the panel's list at once.
#[track_caller]
fn save_material(app: &mut App, name: &'static str, handle: &Handle<StandardMaterial>) {
    app.world_mut()
        .resource_mut::<jackdaw::material_assets::MaterialRegistry>()
        .add(name.to_string(), handle.clone());
    app.world_mut()
        .insert_resource(jackdaw::material_preview::MaterialPreviewState {
            active_material: Some(handle.clone()),
            ..default()
        });
    call(app, "material.save", &[("material", name.into())]);
    app.update();
}

fn open_item(app: &App) -> ItemDef {
    let value = jackdaw::definition_assets::open_definition_value(app.world())
        .expect("a definition is open");
    item_of(app, &value)
}

fn item_of(app: &App, value: &AssetValue) -> ItemDef {
    let handle = value.handle().expect("a compiled definition").clone();
    app.world()
        .resource::<Assets<ItemDef>>()
        .get(&handle.typed::<ItemDef>())
        .expect("the definition is in its store")
        .clone()
}

/// The value the index holds for a file under the project's assets.
fn indexed(app: &App, relative: &str) -> Option<AssetValue> {
    app.world()
        .resource::<AssetIndex>()
        .get(std::path::Path::new(relative))
        .map(|entry| entry.value.clone())
}

#[test]
fn a_definition_is_created_edited_and_saved_through_its_operators() {
    let (mut app, tmp) = editor_with_items();

    let path = item_path(&tmp, "torch");
    call(
        &mut app,
        "asset.new",
        &[
            ("type", "item".into()),
            ("name", "torch".into()),
            (
                "path",
                items_dir(&tmp).to_string_lossy().into_owned().into(),
            ),
        ],
    );
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
    assert_eq!(edited.stack_size, 12);
    assert_eq!(edited.rarity, Rarity::Rare);
    assert_eq!(
        edited.loot,
        vec![LootRoll {
            item: "coin".into(),
            weight: 3
        }]
    );

    call(&mut app, "asset.save", &[]);
    let written = std::fs::read_to_string(&path).expect("the file reads");
    assert!(written.contains("stack_size: 12"), "got:\n{written}");
    assert!(written.contains("Rare"), "got:\n{written}");
    assert!(written.contains("coin"), "got:\n{written}");

    let relative = "content/items/torch.bsn";
    app.world_mut()
        .resource_mut::<AssetIndex>()
        .remove(std::path::Path::new(relative));
    let scan = jackdaw::asset_index::rescan_asset_index(app.world_mut());
    assert_eq!(scan.added, vec![std::path::PathBuf::from(relative)]);
    let reloaded = {
        let value = indexed(&app, relative).expect("the walk found it");
        item_of(&app, &value)
    };
    assert_eq!(reloaded.stack_size, 12);
    assert_eq!(reloaded.rarity, Rarity::Rare);
    assert_eq!(reloaded.loot.len(), 1);
}

#[test]
fn undo_takes_back_one_definition_field_edit() {
    let (mut app, _tmp) = editor_with_items();
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
    assert_eq!(open_item(&app).stack_size, 12);

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| {
            history.undo(world);
        });

    assert_eq!(
        open_item(&app).stack_size,
        0,
        "undo restores what the field held before the edit"
    );
}

#[test]
fn undo_reaches_the_definition_after_its_card_is_closed() {
    let (mut app, _tmp) = editor_with_items();
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
    let value = indexed(&app, "torch.bsn").expect("the file is indexed");

    jackdaw::definition_assets::close_open_definition(app.world_mut());
    app.update();
    assert!(app.world().resource::<OpenDefinition>().0.is_none());

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| {
            history.undo(world);
        });

    assert_eq!(
        item_of(&app, &value).stack_size,
        0,
        "the entry is keyed by the file, so it outlives the card"
    );
}

#[test]
fn a_scene_entity_edit_still_lands_while_a_definition_is_open() {
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
        Some(Val::Px(120.0)),
        "the edit reached the entity it named"
    );
    assert_eq!(
        open_item(&app).stack_size,
        0,
        "and left the open definition alone"
    );
}

#[test]
fn a_definition_saves_back_to_the_file_it_was_opened_from() {
    let (mut app, tmp) = editor_with_items();
    let handle = app
        .world_mut()
        .resource_mut::<Assets<ItemDef>>()
        .add(ItemDef::default());
    let flat = jackdaw::definition_assets::write_asset_file(
        app.world(),
        "sword",
        &AssetValue::Handle(handle.untyped()),
        &item_path(&tmp, "sword"),
    )
    .expect("the file is written");
    let nested = tmp.path().join("assets/content/items/weapons/sword.bsn");
    std::fs::create_dir_all(nested.parent().expect("a parent")).expect("the directory is made");
    std::fs::rename(&flat, &nested).expect("the file moves into its subdirectory");

    call(
        &mut app,
        "asset.open",
        &[("path", nested.to_string_lossy().into_owned().into())],
    );
    call(
        &mut app,
        "asset.set",
        &[("field", "stack_size".into()), ("value", "5".into())],
    );
    call(&mut app, "asset.save", &[]);

    let written = std::fs::read_to_string(&nested).expect("the file reads");
    assert!(written.contains("stack_size: 5"), "got:\n{written}");
    assert!(
        !flat.exists(),
        "the save does not leave a second file where the name alone would put it"
    );
}

#[test]
fn deleting_a_definition_closes_its_card_and_drops_the_entry() {
    let (mut app, tmp) = editor_with_items();
    let path = item_path(&tmp, "torch");
    call(
        &mut app,
        "asset.new",
        &[
            ("type", "item".into()),
            ("name", "torch".into()),
            (
                "path",
                items_dir(&tmp).to_string_lossy().into_owned().into(),
            ),
        ],
    );

    call(
        &mut app,
        "asset.delete",
        &[("path", path.to_string_lossy().into_owned().into())],
    );

    assert!(!path.exists(), "the file is gone");
    assert!(app.world().resource::<OpenDefinition>().0.is_none());
    assert!(indexed(&app, "content/items/torch.bsn").is_none());
}

/// A file another tool rewrites is read back into the handle the editor
/// already handed out, so faces, fields and cards keep pointing at it.
#[test]
fn a_file_edited_on_disk_reloads_into_the_handle_it_already_had() {
    let (mut app, tmp) = editor_with_items();
    call(
        &mut app,
        "asset.new",
        &[
            ("type", "item".into()),
            ("name", "torch".into()),
            (
                "path",
                items_dir(&tmp).to_string_lossy().into_owned().into(),
            ),
        ],
    );
    let relative = "content/items/torch.bsn";
    let before = indexed(&app, relative)
        .and_then(|value| value.handle().map(bevy::asset::UntypedHandle::id))
        .expect("the new file is indexed");

    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::write(
        item_path(&tmp, "torch"),
        format!("#torch\n{} {{ stack_size: 9 }}\n", ItemDef::type_path()),
    )
    .expect("the file is rewritten");
    let scan = jackdaw::asset_index::rescan_asset_index(app.world_mut());

    assert_eq!(scan.reloaded, vec![std::path::PathBuf::from(relative)]);
    let after = indexed(&app, relative)
        .and_then(|value| value.handle().map(bevy::asset::UntypedHandle::id))
        .expect("the file is still indexed");
    assert_eq!(before, after, "the handle outlives the edit");
    assert_eq!(
        open_item(&app).stack_size,
        9,
        "and now holds what the file says"
    );
}

/// A file that leaves the project takes its card with it; whatever already
/// holds the handle keeps rendering.
#[test]
fn a_file_deleted_on_disk_closes_its_card() {
    let (mut app, tmp) = editor_with_items();
    call(
        &mut app,
        "asset.new",
        &[
            ("type", "item".into()),
            ("name", "torch".into()),
            (
                "path",
                items_dir(&tmp).to_string_lossy().into_owned().into(),
            ),
        ],
    );
    assert!(app.world().resource::<OpenDefinition>().0.is_some());

    std::fs::remove_file(item_path(&tmp, "torch")).expect("the file is removed");
    let scan = jackdaw::asset_index::rescan_asset_index(app.world_mut());

    assert_eq!(
        scan.removed,
        vec![std::path::PathBuf::from("content/items/torch.bsn")]
    );
    assert!(
        app.world().resource::<OpenDefinition>().0.is_none(),
        "there is nothing left to save the card back to"
    );
}

/// Edits the user has not saved yet are the point of the card, so a file that
/// goes out from under a dirty one leaves both standing and its save writes
/// the file again.
#[test]
fn a_file_deleted_under_a_dirty_card_leaves_the_card_open() {
    let (mut app, tmp) = editor_with_items();
    call(
        &mut app,
        "asset.new",
        &[
            ("type", "item".into()),
            ("name", "torch".into()),
            (
                "path",
                items_dir(&tmp).to_string_lossy().into_owned().into(),
            ),
        ],
    );
    call(
        &mut app,
        "asset.set",
        &[("field", "stack_size".into()), ("value", "12".into())],
    );

    std::fs::remove_file(item_path(&tmp, "torch")).expect("the file is removed");
    let scan = jackdaw::asset_index::rescan_asset_index(app.world_mut());

    assert!(scan.removed.is_empty(), "got {:?}", scan.removed);
    assert!(app.world().resource::<OpenDefinition>().0.is_some());
    assert_eq!(open_item(&app).stack_size, 12, "the edit is still there");

    call(&mut app, "asset.save", &[]);
    let written = std::fs::read_to_string(item_path(&tmp, "torch")).expect("the file reads");
    assert!(written.contains("stack_size: 12"), "got:\n{written}");
}

/// A file rewritten to hold a type nothing registers is no longer an asset of
/// this project, whatever the index held for it before.
#[test]
fn a_file_that_stops_naming_a_known_type_leaves_the_index() {
    let (mut app, tmp) = editor_with_items();
    call(
        &mut app,
        "asset.new",
        &[
            ("type", "item".into()),
            ("name", "torch".into()),
            (
                "path",
                items_dir(&tmp).to_string_lossy().into_owned().into(),
            ),
        ],
    );
    let relative = "content/items/torch.bsn";
    assert!(indexed(&app, relative).is_some());

    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::write(
        item_path(&tmp, "torch"),
        "#torch\nmy_game::content::LanternDef { }\n",
    )
    .expect("the file is rewritten");
    let scan = jackdaw::asset_index::rescan_asset_index(app.world_mut());

    assert_eq!(scan.removed, vec![std::path::PathBuf::from(relative)]);
    assert!(indexed(&app, relative).is_none());
    assert!(
        item_path(&tmp, "torch").is_file(),
        "the file itself is the user's to keep"
    );
}

/// A file's folder is no part of what it is; the index keys it by the path it
/// sits at.
#[test]
fn an_asset_file_in_any_folder_is_indexed_by_its_path() {
    let (mut app, tmp) = editor_with_items();
    let handle = app
        .world_mut()
        .resource_mut::<Assets<ItemDef>>()
        .add(ItemDef {
            stack_size: 7,
            ..Default::default()
        });
    let deep = tmp.path().join("assets/somewhere/else");
    std::fs::create_dir_all(&deep).expect("the directory is made");
    jackdaw::definition_assets::write_asset_file(
        app.world(),
        "torch",
        &AssetValue::Handle(handle.untyped()),
        &deep.join("torch.bsn"),
    )
    .expect("the file is written");

    let scan = jackdaw::asset_index::rescan_asset_index(app.world_mut());

    assert_eq!(
        scan.added,
        vec![std::path::PathBuf::from("somewhere/else/torch.bsn")]
    );
    let value = indexed(&app, "somewhere/else/torch.bsn").expect("the walk found it");
    assert_eq!(item_of(&app, &value).stack_size, 7);
}

#[test]
fn creating_a_definition_refuses_a_name_whose_file_is_already_there() {
    let (mut app, tmp) = editor_with_items();
    let path = item_path(&tmp, "torch");
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("the directory is made");
    std::fs::write(&path, "#torch ItemDef(stack_size: 7)\n").expect("the file is written");

    call(
        &mut app,
        "asset.new",
        &[("type", "item".into()), ("name", "torch".into())],
    );

    assert_eq!(
        std::fs::read_to_string(&path).expect("the file reads"),
        "#torch ItemDef(stack_size: 7)\n",
        "the file that was already there is left alone"
    );
}

#[test]
fn unregistering_a_definition_type_drops_its_entries_and_closes_the_card() {
    let (mut app, _tmp) = editor_with_items();
    call(
        &mut app,
        "asset.new",
        &[("type", "item".into()), ("name", "torch".into())],
    );
    assert!(app.world().resource::<OpenDefinition>().0.is_some());

    app.world_mut()
        .resource_mut::<AssetKinds>()
        .unregister("item");
    app.update();

    assert!(
        app.world().resource::<OpenDefinition>().0.is_none(),
        "the card goes with the type that registered it"
    );
    assert_eq!(
        app.world().resource::<AssetIndex>().of_kind("item").count(),
        0
    );
}

#[test]
fn a_material_file_is_not_deleted_through_the_definition_operator() {
    let (mut app, tmp) = editor_with_items();
    let handle = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    jackdaw::material_assets::write_material_file(app.world(), "slate", &handle)
        .expect("the material file is written");
    let path = tmp.path().join("assets/materials/slate.bsn");

    call(
        &mut app,
        "asset.delete",
        &[("path", path.to_string_lossy().into_owned().into())],
    );

    assert!(
        path.is_file(),
        "materials are removed by the tool that keeps their registry in step"
    );
}

#[test]
fn a_definition_file_reports_the_kind_its_type_belongs_to() {
    let (mut app, tmp) = editor_with_items();
    let handle = app
        .world_mut()
        .resource_mut::<Assets<ItemDef>>()
        .add(ItemDef::default());
    let path = item_path(&tmp, "torch");
    jackdaw::definition_assets::write_asset_file(
        app.world(),
        "torch",
        &AssetValue::Handle(handle.untyped()),
        &path,
    )
    .expect("the file is written");
    let scene = tmp.path().join("assets/scenes/level.bsn");
    std::fs::create_dir_all(scene.parent().expect("a parent")).expect("the directory is made");
    std::fs::write(&scene, "#Cube\njackdaw_scene_types::types::Brush { }\n")
        .expect("the scene is written");

    assert_eq!(
        jackdaw::definition_assets::kind_of_file(app.world(), &path).map(|kind| kind.kind),
        Some("item".to_string())
    );
    assert!(
        jackdaw::definition_assets::kind_of_file(app.world(), &scene).is_none(),
        "a plain scene is still a scene"
    );
}

/// Materials are loaded with their textures like any other asset file, and the
/// index lists them alongside every other kind.
#[test]
fn a_material_file_is_indexed_alongside_every_other_kind() {
    let (mut app, tmp) = editor_with_items();
    let handle = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            perceptual_roughness: 0.31,
            ..default()
        });
    jackdaw::material_assets::write_material_file(app.world(), "slate", &handle)
        .expect("the material file is written");
    assert!(tmp.path().join("assets/materials/slate.bsn").is_file());

    let scan = jackdaw::asset_index::rescan_asset_index(app.world_mut());
    assert_eq!(
        scan.added,
        vec![std::path::PathBuf::from("materials/slate.bsn")]
    );

    let entry_path = app
        .world()
        .resource::<AssetIndex>()
        .of_kind(MATERIAL_KIND)
        .map(|entry| entry.path.clone())
        .next()
        .expect("the material lists like any other asset");
    assert_eq!(entry_path, std::path::PathBuf::from("materials/slate.bsn"));
}

#[test]
fn a_material_saved_by_its_own_operator_carries_what_asset_set_wrote() {
    let (mut app, tmp) = editor_with_items();
    let handle = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    save_material(&mut app, "slate", &handle);

    call(
        &mut app,
        "asset.open",
        &[("path", "assets/materials/slate.bsn".into())],
    );
    call(
        &mut app,
        "asset.set",
        &[("field", "metallic".into()), ("value", "0.75".into())],
    );
    call(&mut app, "material.save", &[("material", "slate".into())]);
    app.update();

    let written = std::fs::read_to_string(tmp.path().join("assets/materials/slate.bsn"))
        .expect("the file reads");
    assert!(written.contains("metallic: 0.75"), "got:\n{written}");
}

/// A material's fields are filled in through the definition operators, with
/// no panel in the way.
#[test]
fn a_material_is_opened_and_its_fields_set_through_the_definition_operators() {
    let (mut app, tmp) = editor_with_items();
    let handle = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    save_material(&mut app, "slate", &handle);

    call(
        &mut app,
        "asset.open",
        &[("path", "assets/materials/slate.bsn".into())],
    );
    call(
        &mut app,
        "asset.set",
        &[
            ("field", "perceptual_roughness".into()),
            ("value", "0.25".into()),
        ],
    );
    call(&mut app, "asset.save", &[]);

    let roughness = app
        .world()
        .resource::<Assets<StandardMaterial>>()
        .get(&handle)
        .expect("the material the scene is using")
        .perceptual_roughness;
    assert!(
        (roughness - 0.25).abs() < f32::EPSILON,
        "the edit lands on the loaded material, not on a copy"
    );
    let written = std::fs::read_to_string(tmp.path().join("assets/materials/slate.bsn"))
        .expect("the file reads");
    assert!(
        written.contains("perceptual_roughness: 0.25"),
        "got:\n{written}"
    );
}

/// A texture slot names its image by path, which is the one spelling a caller
/// with no handle in hand can give it.
#[test]
fn asset_set_fills_a_texture_slot_from_a_path_and_the_save_keeps_it() {
    let (mut app, tmp) = editor_with_items();
    let handle = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    save_material(&mut app, "slate", &handle);

    call(
        &mut app,
        "asset.open",
        &[("path", "assets/materials/slate.bsn".into())],
    );
    call(
        &mut app,
        "asset.set",
        &[
            ("field", "base_color_texture".into()),
            ("value", "textures/slate_base.png".into()),
        ],
    );
    call(&mut app, "material.save", &[("material", "slate".into())]);
    app.update();

    let texture = app
        .world()
        .resource::<Assets<StandardMaterial>>()
        .get(&handle)
        .expect("the material the scene is using")
        .base_color_texture
        .clone()
        .expect("the slot holds an image");
    assert_eq!(
        app.world()
            .resource::<AssetServer>()
            .get_path(texture.id())
            .map(|path| path.to_string()),
        Some("textures/slate_base.png".to_string()),
        "the slot names the image the caller asked for, whether or not it is there yet"
    );
    let written = std::fs::read_to_string(tmp.path().join("assets/materials/slate.bsn"))
        .expect("the file reads");
    assert!(
        written.contains("textures/slate_base.png"),
        "got:\n{written}"
    );
}

/// A colour is written the way a person says one, and undo puts back what the
/// field held.
#[test]
fn asset_set_takes_a_colour_as_channels_and_undo_puts_the_old_one_back() {
    let (mut app, _tmp) = editor_with_items();
    let handle = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    save_material(&mut app, "slate", &handle);

    call(
        &mut app,
        "asset.open",
        &[("path", "assets/materials/slate.bsn".into())],
    );
    call(
        &mut app,
        "asset.set",
        &[
            ("field", "base_color".into()),
            ("value", "0.25,0.5,1".into()),
        ],
    );

    let base_color = |app: &App| {
        app.world()
            .resource::<Assets<StandardMaterial>>()
            .get(&handle)
            .expect("the material the scene is using")
            .base_color
            .to_linear()
    };
    let set = base_color(&app);
    let asked = Color::srgb(0.25, 0.5, 1.0).to_linear();
    assert!(
        (set.red - asked.red).abs() < 1e-5
            && (set.green - asked.green).abs() < 1e-5
            && (set.blue - asked.blue).abs() < 1e-5,
        "got {set:?}"
    );

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| {
            history.undo(world);
        });
    let back = base_color(&app);
    assert!(
        (back.red - 1.0).abs() < 1e-5 && (back.green - 1.0).abs() < 1e-5,
        "undo puts the colour it had back, got {back:?}"
    );
}
