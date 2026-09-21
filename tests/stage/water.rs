//! The water material: a material asset of its own that a mesh wears in place
//! of its standard one.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw::asset_index::{AssetIndex, AssetValue};
use jackdaw::definition_assets::WATER_KIND;
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_scene_types::PropertyValue;
use jackdaw_surface::WaterMaterial;

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

/// Create a water file and return the path the index holds it at.
fn a_water_material(app: &mut App, name: &'static str) -> String {
    call(
        app,
        "asset.new",
        &[
            ("type", WATER_KIND.into()),
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
fn the_new_asset_list_offers_water() {
    let (app, _tmp) = editor();
    let kinds = app.world().resource::<AssetKinds>();
    let kind = kinds
        .by_kind(WATER_KIND)
        .expect("the editor offers a water material");
    assert_eq!(kind.label, "Water");
    assert_eq!(kind.type_path, <WaterMaterial as TypePath>::type_path());
}

#[test]
fn a_water_file_round_trips_through_save_and_load() {
    let (mut app, tmp) = editor();
    let relative = a_water_material(&mut app, "lake");
    let handle = handle_at(&app, &relative)
        .try_typed::<WaterMaterial>()
        .expect("the file holds a water material");

    {
        let mut materials = app.world_mut().resource_mut::<Assets<WaterMaterial>>();
        let mut material = materials.get_mut(&handle).expect("the asset is stored");
        material.extension.wave_height = 0.42;
        material.extension.depth_distance = 7.5;
    }

    let file = tmp.path().join("assets").join(&relative);
    let world = app.world();
    jackdaw::definition_assets::write_asset_file(
        world,
        "lake",
        &AssetValue::Handle(handle.clone().untyped()),
        &file,
    )
    .expect("the file writes");

    let text = std::fs::read_to_string(&file).expect("the file reads back");
    assert!(
        text.contains(<WaterMaterial as TypePath>::type_path()),
        "the file says what it holds, got {text}",
    );

    let reloaded = jackdaw::material_assets::load_surface_file(
        app.world_mut(),
        &file,
        <WaterMaterial as TypePath>::type_path(),
    )
    .expect("the file loads back")
    .try_typed::<WaterMaterial>()
    .expect("as a water material");
    let materials = app.world().resource::<Assets<WaterMaterial>>();
    let material = materials.get(&reloaded).expect("the reload is stored");
    assert_eq!(material.extension.wave_height, 0.42);
    assert_eq!(material.extension.depth_distance, 7.5);
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
            Name::new("lake surface"),
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
fn applying_water_replaces_the_meshs_material_component() {
    let (mut app, _tmp) = editor();
    let relative = a_water_material(&mut app, "lake");
    let mesh = a_mesh_wearing_a_standard_material(&mut app);

    call(
        &mut app,
        "material.apply",
        &[("material", relative.clone().into())],
    );

    let chosen = handle_at(&app, &relative);
    let worn = app
        .world()
        .get::<MeshMaterial3d<WaterMaterial>>(mesh)
        .expect("the mesh wears the water material");
    assert_eq!(worn.0.id().untyped(), chosen.id());
    assert!(
        app.world()
            .get::<MeshMaterial3d<StandardMaterial>>(mesh)
            .is_none(),
        "and no longer wears a standard material as well",
    );
}

#[test]
fn the_cameras_take_a_depth_prepass_once_a_scene_holds_water() {
    let (mut app, _tmp) = editor();
    let camera = app
        .world_mut()
        .spawn((Camera3d::default(), Camera::default()))
        .id();
    settle(&mut app);
    assert!(
        app.world()
            .get::<bevy::core_pipeline::prepass::DepthPrepass>(camera)
            .is_none(),
        "a scene with no water leaves the camera as it is",
    );

    a_water_material(&mut app, "lake");

    assert!(
        app.world()
            .get::<bevy::core_pipeline::prepass::DepthPrepass>(camera)
            .is_some(),
        "and water gives it the depth the surface reads behind itself",
    );
}
