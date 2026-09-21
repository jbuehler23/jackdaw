//! The foliage material: an asset of its own that a mesh wears in place of its
//! standard one, and that blows with the scene's wind.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw::asset_index::{AssetIndex, AssetValue};
use jackdaw::definition_assets::FOLIAGE_KIND;
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_scene_types::{PropertyValue, Wind};
use jackdaw_surface::FoliageMaterial;

use crate::util;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/definition_project")
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
    for (name, value) in params {
        call = call.param(*name, value.clone());
    }
    let result = call.call().expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Finished, "{id} ran");
    settle(app);
}

/// An editor with a project of its own and a materials folder to write into.
fn editor() -> (App, tempfile::TempDir) {
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
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    settle(&mut app);
    (app, tmp)
}

/// Create a foliage material file and return the path the index holds it at.
fn a_foliage_material(app: &mut App, name: &'static str) -> String {
    call(
        app,
        "asset.new",
        &[
            ("type", FOLIAGE_KIND.into()),
            ("name", name.into()),
            ("path", "materials".into()),
        ],
    );
    format!("materials/{name}.bsn")
}

fn handle_at(app: &App, relative: &str) -> bevy::asset::UntypedHandle {
    app.world()
        .resource::<AssetIndex>()
        .get(Path::new(relative))
        .and_then(|entry| entry.value.handle().cloned())
        .unwrap_or_else(|| panic!("the project holds {relative}"))
}

#[test]
fn the_new_asset_list_offers_a_foliage_material() {
    let (app, _tmp) = editor();
    let kinds = app.world().resource::<AssetKinds>();
    let kind = kinds
        .by_kind(FOLIAGE_KIND)
        .expect("the editor offers a foliage material");
    assert_eq!(kind.label, "Foliage");
    assert_eq!(kind.type_path, <FoliageMaterial as TypePath>::type_path());
}

#[test]
fn a_foliage_material_file_round_trips_through_save_and_load() {
    let (mut app, tmp) = editor();
    let relative = a_foliage_material(&mut app, "pine_leaves");
    let handle = handle_at(&app, &relative)
        .try_typed::<FoliageMaterial>()
        .expect("the file holds a foliage material");

    {
        let mut materials = app.world_mut().resource_mut::<Assets<FoliageMaterial>>();
        let mut material = materials.get_mut(&handle).expect("the asset is stored");
        material.extension.wind_response = 1.75;
        material.extension.translucency_strength = 0.6;
        material.extension.bend_position = 8.0;
    }

    let file = tmp.path().join("assets").join(&relative);
    jackdaw::definition_assets::write_asset_file(
        app.world(),
        "pine_leaves",
        &AssetValue::Handle(handle.clone().untyped()),
        &file,
    )
    .expect("the file writes");

    let text = std::fs::read_to_string(&file).expect("the file reads back");
    assert!(
        text.contains(<FoliageMaterial as TypePath>::type_path()),
        "the file says what it holds, got {text}",
    );
    assert!(
        !text.contains("wind:"),
        "and the wind it was blowing by is not authored, got {text}",
    );

    let reloaded = jackdaw::material_assets::load_surface_file(
        app.world_mut(),
        &file,
        <FoliageMaterial as TypePath>::type_path(),
    )
    .expect("the file loads back")
    .try_typed::<FoliageMaterial>()
    .expect("as a foliage material");
    let materials = app.world().resource::<Assets<FoliageMaterial>>();
    let material = materials.get(&reloaded).expect("the reload is stored");
    assert_eq!(material.extension.wind_response, 1.75);
    assert_eq!(material.extension.translucency_strength, 0.6);
    assert_eq!(material.extension.bend_position, 8.0);
}

/// A mesh the scene authors, wearing the material the editor gave it.
fn a_mesh_wearing_a_standard_material(app: &mut App) -> Entity {
    let standard = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    let mesh = app
        .world_mut()
        .spawn((
            Name::new("pine"),
            Mesh3d::default(),
            MeshMaterial3d(standard),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), mesh);
    app.world_mut().resource_mut::<Selection>().entities = vec![mesh];
    settle(app);
    mesh
}

#[test]
fn applying_a_foliage_material_replaces_the_meshs_material_component() {
    let (mut app, _tmp) = editor();
    let relative = a_foliage_material(&mut app, "pine_leaves");
    let mesh = a_mesh_wearing_a_standard_material(&mut app);

    call(
        &mut app,
        "material.apply",
        &[("material", relative.clone().into())],
    );

    let chosen = handle_at(&app, &relative);
    let worn = app
        .world()
        .get::<MeshMaterial3d<FoliageMaterial>>(mesh)
        .expect("the mesh wears the foliage material");
    assert_eq!(worn.0.id().untyped(), chosen.id());
    assert!(
        app.world()
            .get::<MeshMaterial3d<StandardMaterial>>(mesh)
            .is_none(),
        "and no longer wears a standard material as well",
    );

    call(&mut app, "history.undo", &[]);

    assert!(
        app.world()
            .get::<MeshMaterial3d<StandardMaterial>>(mesh)
            .is_some(),
        "and the undo puts the standard material back",
    );
}

#[test]
fn a_foliage_material_takes_the_wind_the_scene_is_blowing_by() {
    let (mut app, _tmp) = editor();
    let relative = a_foliage_material(&mut app, "pine_leaves");
    let handle = handle_at(&app, &relative)
        .try_typed::<FoliageMaterial>()
        .expect("the file holds a foliage material");

    let blowing = Wind {
        strength: 2.0,
        ..Wind::default()
    };
    app.world_mut().spawn(blowing);
    settle(&mut app);

    let materials = app.world().resource::<Assets<FoliageMaterial>>();
    let material = materials.get(&handle).expect("the asset is stored");
    assert_eq!(
        material.extension.wind, blowing,
        "a wind authored once reaches every material without the file naming it",
    );
}
