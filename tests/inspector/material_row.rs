//! The asset row above the material cards.
//!
//! A mesh entity wears a material asset, and the row names it: the file it
//! points at, the list to choose another from, and a Clear that takes the
//! choice back out of the scene again.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw::commands::CommandHistory;
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_bsn::{BsnValue, SceneBsnAst};
use jackdaw_scene_types::PropertyValue;

use crate::util;

/// The component a mesh wears its material on, the field the document carries
/// it under, and the name the row answers to.
const MESH_MATERIAL: &str =
    "bevy_pbr::mesh_material::MeshMaterial3d<bevy_pbr::pbr_material::StandardMaterial>";
const HANDLE_FIELD: &str = "0";
const MATERIAL_FIELD: &str = "material";

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

/// An editor holding two material files, showing a mesh entity the document
/// knows about.
fn app_with_mesh() -> (App, tempfile::TempDir, Entity) {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::copy(
        fixture_dir().join("jackdaw.toml"),
        tmp.path().join("jackdaw.toml"),
    )
    .expect("the manifest copies");
    std::fs::create_dir_all(tmp.path().join("assets/materials")).expect("a materials folder");

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

    let entity = app
        .world_mut()
        .spawn((
            Name::new("crate"),
            Mesh3d::default(),
            MeshMaterial3d(Handle::<StandardMaterial>::default()),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    app.world_mut().resource_mut::<Selection>().entities = vec![entity];
    settle(&mut app);
    (app, tmp, entity)
}

fn all_entities(app: &mut App) -> Vec<Entity> {
    app.world_mut()
        .query::<Entity>()
        .iter(app.world())
        .collect()
}

/// A row writing a field, addressed the way the operators address it.
fn row_for(app: &mut App, field_path: &str) -> Entity {
    all_entities(app)
        .into_iter()
        .find(|&entity| {
            jackdaw::inspector::asset_field_shown_by(app.world(), entity)
                .is_some_and(|(_, field)| field == field_path)
        })
        .unwrap_or_else(|| {
            let shown: Vec<String> = all_entities(app)
                .into_iter()
                .filter_map(|entity| {
                    jackdaw::inspector::asset_field_shown_by(app.world(), entity)
                        .map(|(_, field)| field.to_string())
                })
                .collect();
            panic!("no row writes `{field_path}`; the inspector shows {shown:?}")
        })
}

/// Every line of text under a row.
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

/// What the document says the entity's material is.
fn authored_material(app: &App, entity: Entity) -> Option<BsnValue> {
    let ast = app.world().resource::<SceneBsnAst>();
    let node = ast.ast_for(entity)?;
    jackdaw_bsn::get_bsn_field(ast, node, MESH_MATERIAL, HANDLE_FIELD)
}

fn undo(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| {
            history.undo(world);
        });
    settle(app);
}

#[test]
fn a_mesh_entity_shows_the_material_it_wears_and_a_pick_assigns_it() {
    let (mut app, _tmp, entity) = app_with_mesh();

    let row = row_for(&mut app, MATERIAL_FIELD);
    assert!(
        row_text(&mut app, row).iter().any(|line| line == "None"),
        "a mesh wearing no material file says so, got {:?}",
        row_text(&mut app, row),
    );

    call(
        &mut app,
        "asset.pick",
        &[
            ("field", MATERIAL_FIELD.into()),
            ("value", "materials/slate.bsn".into()),
        ],
    );

    let row = row_for(&mut app, MATERIAL_FIELD);
    assert!(
        row_text(&mut app, row)
            .iter()
            .any(|line| line == "slate.bsn"),
        "the row names the file it now points at, got {:?}",
        row_text(&mut app, row),
    );
    assert_eq!(
        authored_material(&app, entity),
        Some(BsnValue::String("materials/slate.bsn".to_string())),
        "and the scene carries the path",
    );

    undo(&mut app);
    assert_ne!(
        authored_material(&app, entity),
        Some(BsnValue::String("materials/slate.bsn".to_string())),
        "undo puts the scene back",
    );
}

#[test]
fn the_cards_under_the_row_edit_the_asset_the_row_names() {
    let (mut app, _tmp, _entity) = app_with_mesh();

    call(
        &mut app,
        "asset.pick",
        &[
            ("field", MATERIAL_FIELD.into()),
            ("value", "materials/moss.bsn".into()),
        ],
    );

    let named = app
        .world()
        .resource::<jackdaw::asset_index::AssetIndex>()
        .get(Path::new("materials/moss.bsn"))
        .and_then(|entry| entry.value.handle().cloned())
        .expect("the project holds the file the row names");
    let edited = app
        .world()
        .resource::<jackdaw::material_preview::MaterialPreviewState>()
        .active_material
        .clone()
        .expect("the cards are editing a material");

    assert_eq!(
        edited.id().untyped(),
        named.id(),
        "the cards below the row follow the pick onto the asset it chose",
    );
}

#[test]
fn clearing_a_mesh_material_takes_the_override_out_of_the_scene() {
    let (mut app, _tmp, entity) = app_with_mesh();

    call(
        &mut app,
        "asset.pick",
        &[
            ("field", MATERIAL_FIELD.into()),
            ("value", "materials/slate.bsn".into()),
        ],
    );
    assert!(authored_material(&app, entity).is_some(), "the pick landed");

    call(&mut app, "asset.clear", &[("field", MATERIAL_FIELD.into())]);

    assert_eq!(
        authored_material(&app, entity),
        None,
        "Clear leaves the scene saying nothing about the entity's material",
    );

    undo(&mut app);
    assert_eq!(
        authored_material(&app, entity),
        Some(BsnValue::String("materials/slate.bsn".to_string())),
        "and undo puts the override back",
    );
}

/// The asset a project file holds, as the id behind it.
fn filed_material(app: &App, path: &str) -> bevy::asset::UntypedAssetId {
    app.world()
        .resource::<jackdaw::asset_index::AssetIndex>()
        .get(Path::new(path))
        .and_then(|entry| entry.value.handle().cloned())
        .expect("the project holds the file")
        .id()
}

/// The material the cards under the row are editing.
fn material_the_cards_edit(app: &App) -> Option<bevy::asset::UntypedAssetId> {
    app.world()
        .resource::<jackdaw::material_preview::MaterialPreviewState>()
        .active_material
        .as_ref()
        .map(|handle| handle.id().untyped())
}

/// The material the entity is wearing.
fn worn_material(app: &App, entity: Entity) -> Option<bevy::asset::UntypedAssetId> {
    app.world()
        .get::<MeshMaterial3d<StandardMaterial>>(entity)
        .map(|material| material.0.id().untyped())
}

/// The material a project file holds, as the handle for it.
fn material_handle(app: &App, path: &str) -> Handle<StandardMaterial> {
    app.world()
        .resource::<jackdaw::asset_index::AssetIndex>()
        .get(Path::new(path))
        .and_then(|entry| entry.value.handle().cloned())
        .and_then(|handle| handle.try_typed::<StandardMaterial>().ok())
        .expect("the project holds the material file")
}

#[test]
fn undoing_a_pick_puts_the_material_the_entity_wore_back_on_it() {
    let (mut app, _tmp, entity) = app_with_mesh();
    let moss = material_handle(&app, "materials/moss.bsn");
    app.world_mut()
        .entity_mut(entity)
        .insert(MeshMaterial3d(moss.clone()));
    settle(&mut app);

    call(
        &mut app,
        "asset.pick",
        &[
            ("field", MATERIAL_FIELD.into()),
            ("value", "materials/slate.bsn".into()),
        ],
    );
    assert_eq!(
        worn_material(&app, entity),
        Some(filed_material(&app, "materials/slate.bsn")),
        "the pick dressed the mesh",
    );

    undo(&mut app);
    assert_eq!(
        worn_material(&app, entity),
        Some(moss.id().untyped()),
        "undo puts the material it wore back on",
    );
}

#[test]
fn undoing_a_pick_takes_the_cards_off_the_material_it_chose() {
    let (mut app, _tmp, _entity) = app_with_mesh();

    call(
        &mut app,
        "asset.pick",
        &[
            ("field", MATERIAL_FIELD.into()),
            ("value", "materials/slate.bsn".into()),
        ],
    );
    let chosen = filed_material(&app, "materials/slate.bsn");
    assert_eq!(
        material_the_cards_edit(&app),
        Some(chosen),
        "the cards followed the pick onto the asset it chose",
    );

    undo(&mut app);
    assert_ne!(
        material_the_cards_edit(&app),
        Some(chosen),
        "undo takes the cards off the material the pick chose",
    );
}
