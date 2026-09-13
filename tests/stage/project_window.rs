//! The Project window: one window over the project's files, where a click
//! selects a file into the inspector and the kind filter stands in for the
//! catalog listing that used to be a window of its own.

use std::path::{Path, PathBuf};

use bevy::asset::{Asset, Assets};
use bevy::prelude::*;
use jackdaw::project_window::{KindFilter, ProjectWindowState};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_scene_types::PropertyValue;

use crate::util;

#[derive(Asset, Reflect, Clone, Default)]
#[reflect(Default)]
struct ItemDef {
    stack_size: u32,
}

const SCENE_BODY: &str = "#Town\nbevy_transform::components::transform::Transform\n";

fn item_kind() -> AssetKind {
    AssetKind::extension("item", "Item", ItemDef::type_path())
}

/// An editor over a project holding a material, an item and a scene, with the
/// Project window and the inspector both up.
fn editor_with_a_project() -> (App, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let content = tmp.path().join("assets/content");
    std::fs::create_dir_all(&content).expect("the content folder is made");
    std::fs::write(content.join("town.bsn"), SCENE_BODY).expect("the scene is written");
    let item_body = format!("#torch\n{} {{ stack_size: 4 }}\n", ItemDef::type_path());
    std::fs::write(
        content.join("torch.item.bsn"),
        jackdaw::asset_files::asset_file_text(ItemDef::type_path(), &item_body),
    )
    .expect("the item is written");

    let mut app = util::editor_test_app();
    app.init_asset::<ItemDef>();
    app.register_asset_reflect::<ItemDef>();
    app.register_type::<ItemDef>();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: tmp.path().to_path_buf(),
            config: default(),
        });
    app.world_mut()
        .resource_mut::<AssetKinds>()
        .register(item_kind());
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    let icon_font = app
        .world()
        .resource::<jackdaw_feathers::icons::IconFont>()
        .0
        .clone();
    app.world_mut()
        .spawn(jackdaw::project_window::project_panel_content(icon_font));
    settle(&mut app);
    (app, tmp)
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
        creates_history_entry: false,
    });
    for (key, value) in params {
        call = call.param(*key, value.clone());
    }
    let result = call.call().expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Finished, "{id} did not finish");
    settle(app);
}

fn content_dir(tmp: &tempfile::TempDir) -> PathBuf {
    tmp.path().join("assets/content")
}

/// Every line of text on screen, so a card can be recognised by what it says.
fn lines(app: &mut App) -> Vec<String> {
    let mut texts = app.world_mut().query::<&Text>();
    texts.iter(app.world()).map(|text| text.0.clone()).collect()
}

fn shows(app: &mut App, wanted: &str) -> bool {
    lines(app).iter().any(|line| line.contains(wanted))
}

/// The names the tiles are showing, after the search and the kind filter.
fn tiles(app: &App) -> Vec<String> {
    app.world()
        .resource::<ProjectWindowState>()
        .entries
        .iter()
        .map(|entry| entry.file_name.clone())
        .collect()
}

/// Save a material through the operator that owns it, which is what writes its
/// file and puts it in the index.
fn save_material(app: &mut App, name: &'static str) -> PathBuf {
    let handle = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    app.world_mut()
        .resource_mut::<jackdaw::material_assets::MaterialRegistry>()
        .add(name.to_string(), handle.clone());
    app.world_mut()
        .insert_resource(jackdaw::material_preview::MaterialPreviewState {
            active_material: Some(handle),
            ..default()
        });
    call(app, "material.save", &[("material", name.into())]);
    let path = app
        .world()
        .resource::<jackdaw::asset_index::AssetIndex>()
        .iter()
        .find(|entry| entry.name() == name)
        .map(|entry| entry.path.clone())
        .expect("the material is indexed");
    app.world()
        .resource::<jackdaw::project::ProjectRoot>()
        .assets_dir()
        .join(path)
}

#[test]
fn selecting_a_material_file_shows_its_card_in_the_inspector() {
    let (mut app, _tmp) = editor_with_a_project();
    let material = save_material(&mut app, "slate");

    call(
        &mut app,
        "project.select",
        &[("path", material.to_string_lossy().into_owned().into())],
    );

    assert!(
        shows(&mut app, "slate (Material)"),
        "the inspector shows the material's card, got {:?}",
        lines(&mut app)
    );
}

#[test]
fn selecting_a_definition_file_shows_its_card_in_the_inspector() {
    let (mut app, tmp) = editor_with_a_project();
    let item = content_dir(&tmp).join("torch.item.bsn");

    call(
        &mut app,
        "project.select",
        &[("path", item.to_string_lossy().into_owned().into())],
    );

    assert!(
        shows(&mut app, "torch (Item)"),
        "the inspector shows the item's card, got {:?}",
        lines(&mut app)
    );
}

#[test]
fn a_scene_file_card_carries_the_button_that_opens_it() {
    let (mut app, tmp) = editor_with_a_project();
    let scene = content_dir(&tmp).join("town.bsn");
    let path = scene.to_string_lossy().into_owned();

    call(&mut app, "project.select", &[("path", path.clone().into())]);

    assert!(
        shows(&mut app, "Town"),
        "the card names the scene's root, got {:?}",
        lines(&mut app)
    );
    assert!(shows(&mut app, "Open"), "the card carries an Open button");

    let tabs_before = app.world().resource::<jackdaw::scenes::Scenes>().tabs.len();
    call(&mut app, "project.open", &[("path", path.into())]);
    let tabs_after = app.world().resource::<jackdaw::scenes::Scenes>().tabs.len();
    assert!(
        tabs_after > tabs_before,
        "opening the scene from its card puts it in a tab"
    );
}

#[test]
fn filtering_by_kind_shows_only_the_files_of_that_kind() {
    let (mut app, tmp) = editor_with_a_project();
    call(
        &mut app,
        "project.select",
        &[(
            "path",
            content_dir(&tmp).to_string_lossy().into_owned().into(),
        )],
    );
    assert!(tiles(&app).iter().any(|name| name == "town.bsn"));

    call(
        &mut app,
        "project.filter",
        &[("kind", "definitions".into())],
    );

    assert_eq!(
        tiles(&app),
        vec!["torch.item.bsn".to_string()],
        "only the definitions are left"
    );
    assert_eq!(
        app.world().resource::<ProjectWindowState>().kind_filter,
        KindFilter::Definitions
    );
}

#[test]
fn searching_shows_only_the_files_whose_name_holds_the_text() {
    let (mut app, tmp) = editor_with_a_project();
    call(
        &mut app,
        "project.select",
        &[(
            "path",
            content_dir(&tmp).to_string_lossy().into_owned().into(),
        )],
    );

    call(&mut app, "project.search", &[("text", "town".into())]);

    assert_eq!(tiles(&app), vec!["town.bsn".to_string()]);
}

#[test]
fn the_folder_tree_lists_each_folder_once() {
    let (mut app, tmp) = editor_with_a_project();

    let mut rows = app
        .world_mut()
        .query::<&jackdaw::project_window::ProjectFolderNode>();
    let listed: Vec<PathBuf> = rows.iter(app.world()).map(|row| row.0.clone()).collect();

    let content = content_dir(&tmp);
    assert_eq!(
        listed.iter().filter(|path| **path == content).count(),
        1,
        "content is one row, got {listed:?}"
    );
    let mut unique = listed.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), listed.len(), "a folder is listed twice");
}

#[test]
fn a_folder_that_goes_while_it_is_showing_falls_back_to_the_root() {
    let (mut app, tmp) = editor_with_a_project();
    let folder = content_dir(&tmp);
    call(
        &mut app,
        "project.select",
        &[("path", folder.to_string_lossy().into_owned().into())],
    );
    std::fs::remove_dir_all(&folder).expect("the folder goes");

    app.world_mut()
        .resource_mut::<ProjectWindowState>()
        .needs_refresh = true;
    settle(&mut app);

    let state = app.world().resource::<ProjectWindowState>();
    assert_eq!(
        state.current_directory.as_path(),
        Path::new(&state.root_directory),
        "the window falls back to the project's assets"
    );
}
