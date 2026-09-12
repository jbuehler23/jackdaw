//! Creating an asset from wherever the user is: `asset.new` with a folder of
//! its own, the New Asset list a folder puts up, and the Add menu's Assets
//! group. The kind of file follows from what was picked, never from a dialog
//! asking what flavour of file to write.

use crate::util;

use std::path::{Path, PathBuf};

use bevy::asset::Asset;
use bevy::prelude::*;
use jackdaw::asset_index::AssetIndex;
use jackdaw::definition_assets::{MATERIAL_KIND, OpenDefinition};
use jackdaw::material_assets::MaterialRegistry;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_feathers::picker::{PickerItems, PickerSelect};
use jackdaw_scene_types::PropertyValue;

const OUTFIT_TYPE: &str = "my_game::content::OutfitDef";

#[derive(Asset, Reflect, Clone, Default)]
#[reflect(Default)]
struct ItemDef {
    stack_size: u32,
}

/// An editor with a project of its own, one type it has registered and one the
/// project's schema reports.
fn editor_with_kinds() -> (App, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("assets")).expect("an assets folder");
    let mut app = util::editor_test_app();
    app.init_asset::<ItemDef>();
    app.register_asset_reflect::<ItemDef>();
    app.register_type::<ItemDef>();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: tmp.path().to_path_buf(),
            config: default(),
        });
    {
        let mut kinds = app.world_mut().resource_mut::<AssetKinds>();
        kinds.register(AssetKind::extension("item", "Item", ItemDef::type_path()));
        kinds.register(AssetKind::from_schema(OUTFIT_TYPE));
    }
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    settle(&mut app);
    (app, tmp)
}

fn settle(app: &mut App) {
    for _ in 0..6 {
        app.update();
    }
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
    settle(app);
}

/// A folder under the project's assets named after nothing in particular.
fn folder(tmp: &tempfile::TempDir, relative: &str) -> PathBuf {
    let dir = tmp.path().join("assets").join(relative);
    std::fs::create_dir_all(&dir).expect("the directory is made");
    dir
}

fn indexed(app: &App, relative: &str) -> bool {
    app.world()
        .resource::<AssetIndex>()
        .get(Path::new(relative))
        .is_some()
}

/// The lines the open list offers, one per kind.
fn listed(app: &mut App) -> Vec<String> {
    let entities: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<PickerItems<String>>>()
        .iter(app.world())
        .collect();
    entities
        .into_iter()
        .filter_map(|entity| app.world().get::<PickerItems<String>>(entity))
        .flat_map(|items| items.items().to_vec())
        .collect()
}

/// Choose the line the list shows for a kind, the way a click on the row does.
#[track_caller]
fn choose(app: &mut App, label: &str) {
    let index = listed(app)
        .iter()
        .position(|line| line.starts_with(label))
        .unwrap_or_else(|| panic!("the list offers {label}, got {:?}", listed(app)));
    let picker = app
        .world_mut()
        .query_filtered::<Entity, With<PickerItems<String>>>()
        .iter(app.world())
        .next()
        .expect("a list is open");
    app.world_mut().trigger(PickerSelect {
        entity: picker,
        index,
    });
    settle(app);
}

#[test]
fn an_asset_is_written_into_whatever_folder_it_is_asked_for() {
    let (mut app, tmp) = editor_with_kinds();
    let dir = folder(&tmp, "content/odds_and_ends");

    call(
        &mut app,
        "asset.new",
        &[
            ("type", "item".into()),
            ("name", "torch".into()),
            ("path", dir.to_string_lossy().into_owned().into()),
        ],
    );

    let written = std::fs::read_to_string(dir.join("torch.bsn")).expect("the file reads");
    assert!(
        written.contains(&format!("jackdaw asset {}", ItemDef::type_path())),
        "the file says what it holds: {written}"
    );
    assert!(
        indexed(&app, "content/odds_and_ends/torch.bsn"),
        "the index keys it by the path it landed on"
    );
}

#[test]
fn an_asset_with_no_name_takes_the_next_free_one() {
    let (mut app, tmp) = editor_with_kinds();
    let dir = folder(&tmp, "content/items");
    let path: PropertyValue = dir.to_string_lossy().into_owned().into();

    call(
        &mut app,
        "asset.new",
        &[("type", "item".into()), ("path", path.clone())],
    );
    call(
        &mut app,
        "asset.new",
        &[("type", "item".into()), ("path", path)],
    );

    assert!(dir.join("item_1.bsn").is_file(), "the first takes item_1");
    assert!(
        dir.join("item_2.bsn").is_file(),
        "a second creation in the same folder does not fight the first for its name"
    );
    assert!(indexed(&app, "content/items/item_2.bsn"));
}

#[test]
fn the_new_asset_list_offers_every_kind_and_no_scene() {
    let (mut app, tmp) = editor_with_kinds();
    let dir = folder(&tmp, "content");

    call(
        &mut app,
        "asset.new_picker",
        &[("path", dir.to_string_lossy().into_owned().into())],
    );

    let lines = listed(&mut app);
    assert!(
        lines
            .iter()
            .any(|line| *line == format!("Item  {}", ItemDef::type_path())),
        "the type the editor holds is offered under its own type: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| *line == format!("Outfit  {OUTFIT_TYPE}")),
        "the kind the schema reports is offered under its type: {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.starts_with("Material  ")),
        "a material is an asset file like any other: {lines:?}"
    );
    assert!(
        !lines
            .iter()
            .any(|line| line.to_lowercase().contains("scene")),
        "a scene is made by File > New, not by picking a type: {lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line.starts_with("Prefab")),
        "a prefab is packed from a selection, and has no blank value to write: {lines:?}"
    );
}

#[test]
fn a_material_picked_from_the_list_joins_the_materials_panel() {
    let (mut app, tmp) = editor_with_kinds();
    let dir = folder(&tmp, "content/looks");

    call(
        &mut app,
        "asset.new_picker",
        &[("path", dir.to_string_lossy().into_owned().into())],
    );
    choose(&mut app, "Material");

    let created = format!("{MATERIAL_KIND}_1");
    assert!(
        dir.join(format!("{created}.bsn")).is_file(),
        "the material lands in the folder the list was opened on"
    );
    assert!(
        app.world()
            .resource::<MaterialRegistry>()
            .get_by_name(&created)
            .is_some(),
        "the panel lists it with the rest of the project's materials"
    );
}

#[test]
fn the_add_menu_offers_every_kind_and_creates_where_the_browser_is() {
    let (mut app, tmp) = editor_with_kinds();
    let dir = folder(&tmp, "content/things");
    app.world_mut()
        .resource_mut::<jackdaw::asset_browser::AssetBrowserState>()
        .current_directory = dir.clone();

    let items = jackdaw::add_entity_picker::collect_add_menu_items(app.world_mut());
    let assets: Vec<(String, String)> = items
        .iter()
        .filter(|item| item.category.name.as_deref() == Some("Assets"))
        .map(|item| (item.action.clone(), item.label.clone()))
        .collect();
    for expected in [("asset:item", "Item"), ("asset:outfit", "Outfit")] {
        assert!(
            assets
                .iter()
                .any(|(action, label)| (action.as_str(), label.as_str()) == expected),
            "the Assets group offers {expected:?}, got {assets:?}"
        );
    }

    app.world_mut()
        .trigger(jackdaw_widgets::menu_bar::MenuAction {
            action: String::from("asset:item"),
        });
    settle(&mut app);

    assert!(
        dir.join("item_1.bsn").is_file(),
        "the menu writes into the folder the browser is showing"
    );
    assert!(indexed(&app, "content/things/item_1.bsn"));
}

#[test]
fn creating_where_nothing_can_be_written_leaves_no_entry_behind() {
    let (mut app, tmp) = editor_with_kinds();
    let dir = folder(&tmp, "content/locked");
    let mut locked = std::fs::metadata(&dir)
        .expect("the folder is there")
        .permissions();
    locked.set_readonly(true);
    std::fs::set_permissions(&dir, locked).expect("the folder locks");

    call(
        &mut app,
        "asset.new",
        &[
            ("type", "item".into()),
            ("path", dir.to_string_lossy().into_owned().into()),
        ],
    );

    let entries = app.world().resource::<AssetIndex>().iter().count();
    assert!(
        !dir.join("item_1.bsn").exists(),
        "nothing was written into a folder that refuses writes"
    );
    assert_eq!(
        entries, 0,
        "and the index holds no entry for what is not there"
    );
    assert!(
        app.world().resource::<OpenDefinition>().0.is_none(),
        "and no card stands for it"
    );

    let mut open = std::fs::metadata(&dir)
        .expect("the folder is there")
        .permissions();
    #[expect(
        clippy::permissions_set_readonly_false,
        reason = "the folder is torn down next"
    )]
    open.set_readonly(false);
    std::fs::set_permissions(&dir, open).expect("the folder unlocks");
}

#[test]
fn nothing_is_written_outside_the_project_assets() {
    let (mut app, tmp) = editor_with_kinds();
    let outside = tmp.path().join("notes");
    std::fs::create_dir_all(&outside).expect("a folder beside the assets");

    call(
        &mut app,
        "asset.new",
        &[
            ("type", "item".into()),
            ("path", outside.to_string_lossy().into_owned().into()),
        ],
    );

    assert!(
        !outside.join("item_1.bsn").exists(),
        "a folder the index cannot key is no place for an asset"
    );
    assert_eq!(app.world().resource::<AssetIndex>().iter().count(), 0);
}

#[test]
fn a_name_that_carries_its_own_extension_writes_one_file() {
    let (mut app, tmp) = editor_with_kinds();
    let dir = folder(&tmp, "content/items");

    call(
        &mut app,
        "asset.new",
        &[
            ("type", "item".into()),
            ("name", "torch.bsn".into()),
            ("path", dir.to_string_lossy().into_owned().into()),
        ],
    );

    assert!(
        dir.join("torch.bsn").is_file(),
        "the name already said what file to write"
    );
    assert!(
        !dir.join("torch.bsn.bsn").exists(),
        "and it was not written again on the end"
    );
    assert!(indexed(&app, "content/items/torch.bsn"));
}

#[test]
fn a_name_with_a_flavour_keeps_the_flavour_and_is_known_by_its_stem() {
    let (mut app, tmp) = editor_with_kinds();
    let dir = folder(&tmp, "content/items");

    call(
        &mut app,
        "asset.new",
        &[
            ("type", "item".into()),
            ("name", "lamp.item.bsn".into()),
            ("path", dir.to_string_lossy().into_owned().into()),
        ],
    );

    let written = std::fs::read_to_string(dir.join("lamp.item.bsn")).expect("the file reads");
    assert!(
        written.contains("lamp"),
        "the file names what it holds by the stem before its first dot: {written}"
    );
    assert!(indexed(&app, "content/items/lamp.item.bsn"));
}

#[test]
fn a_creation_never_writes_over_a_file_already_there() {
    let (mut app, tmp) = editor_with_kinds();
    let dir = folder(&tmp, "content/items");
    let path = dir.join("torch.bsn");
    std::fs::write(&path, "hand written").expect("a file is there first");

    call(
        &mut app,
        "asset.new",
        &[
            ("type", "item".into()),
            ("name", "torch".into()),
            ("path", dir.to_string_lossy().into_owned().into()),
        ],
    );

    assert_eq!(
        std::fs::read_to_string(&path).expect("the file still reads"),
        "hand written",
        "what was there is what is still there"
    );
    assert!(
        app.world().resource::<OpenDefinition>().0.is_none(),
        "and no card stands for a file that was not written"
    );
}

#[test]
fn the_same_name_in_two_folders_is_two_assets() {
    let (mut app, tmp) = editor_with_kinds();
    let items = folder(&tmp, "content/items");
    let props = folder(&tmp, "content/props");

    for dir in [&items, &props] {
        call(
            &mut app,
            "asset.new",
            &[
                ("type", "item".into()),
                ("name", "torch".into()),
                ("path", dir.to_string_lossy().into_owned().into()),
            ],
        );
    }

    assert!(items.join("torch.bsn").is_file());
    assert!(props.join("torch.bsn").is_file());
    assert!(indexed(&app, "content/items/torch.bsn"));
    assert!(
        indexed(&app, "content/props/torch.bsn"),
        "a name is the file's, not the whole project's"
    );
}
