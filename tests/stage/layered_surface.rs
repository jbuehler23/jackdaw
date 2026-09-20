//! The layered surface: a material asset of its own that a mesh wears in place
//! of its standard one.

use std::path::{Path, PathBuf};

use bevy::pbr::EntitiesNeedingSpecialization;
use bevy::prelude::*;
use jackdaw::asset_index::{AssetIndex, AssetValue};
use jackdaw::definition_assets::LAYERED_SURFACE_KIND;
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_scene_types::PropertyValue;
use jackdaw_surface::LayeredSurfaceMaterial;

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

/// Create a layered surface file and return the path the index holds it at.
fn a_layered_surface(app: &mut App, name: &'static str) -> String {
    call(
        app,
        "asset.new",
        &[
            ("type", LAYERED_SURFACE_KIND.into()),
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
fn the_new_asset_list_offers_a_layered_surface() {
    let (app, _tmp) = editor();
    let kinds = app.world().resource::<AssetKinds>();
    let kind = kinds
        .by_kind(LAYERED_SURFACE_KIND)
        .expect("the editor offers a layered surface");
    assert_eq!(kind.label, "Layered Surface");
    assert_eq!(
        kind.type_path,
        <LayeredSurfaceMaterial as TypePath>::type_path(),
    );
}

#[test]
fn a_layered_surface_file_round_trips_through_save_and_load() {
    let (mut app, tmp) = editor();
    let relative = a_layered_surface(&mut app, "mossy");
    let handle = handle_at(&app, &relative)
        .try_typed::<LayeredSurfaceMaterial>()
        .expect("the file holds a layered surface");

    {
        let mut materials = app
            .world_mut()
            .resource_mut::<Assets<LayeredSurfaceMaterial>>();
        let mut material = materials.get_mut(&handle).expect("the asset is stored");
        material.extension.blend.amount = 0.75;
        material.extension.layer_uv_scale = 0.03;
    }

    let file = tmp.path().join("assets").join(&relative);
    let world = app.world();
    jackdaw::definition_assets::write_asset_file(
        world,
        "mossy",
        &AssetValue::Handle(handle.clone().untyped()),
        &file,
    )
    .expect("the file writes");

    let text = std::fs::read_to_string(&file).expect("the file reads back");
    assert!(
        text.contains(<LayeredSurfaceMaterial as TypePath>::type_path()),
        "the file says what it holds, got {text}",
    );

    let reloaded = jackdaw::material_assets::load_surface_file(
        app.world_mut(),
        &file,
        <LayeredSurfaceMaterial as TypePath>::type_path(),
    )
    .expect("the file loads back")
    .try_typed::<LayeredSurfaceMaterial>()
    .expect("as a layered surface");
    let materials = app.world().resource::<Assets<LayeredSurfaceMaterial>>();
    let material = materials.get(&reloaded).expect("the reload is stored");
    assert_eq!(material.extension.blend.amount, 0.75);
    assert_eq!(material.extension.layer_uv_scale, 0.03);
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
            Name::new("cliff"),
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
fn applying_a_layered_surface_replaces_the_meshs_material_component() {
    let (mut app, _tmp) = editor();
    let relative = a_layered_surface(&mut app, "mossy");
    let mesh = a_mesh_wearing_a_standard_material(&mut app);

    call(
        &mut app,
        "material.apply",
        &[("material", relative.clone().into())],
    );

    let chosen = handle_at(&app, &relative);
    let worn = app
        .world()
        .get::<MeshMaterial3d<LayeredSurfaceMaterial>>(mesh)
        .expect("the mesh wears the layered surface");
    assert_eq!(worn.0.id().untyped(), chosen.id());
    assert!(
        app.world()
            .get::<MeshMaterial3d<StandardMaterial>>(mesh)
            .is_none(),
        "and no longer wears a standard material as well",
    );
}

#[test]
fn undoing_the_apply_puts_the_standard_material_back() {
    let (mut app, _tmp) = editor();
    let relative = a_layered_surface(&mut app, "mossy");
    let mesh = a_mesh_wearing_a_standard_material(&mut app);
    let before = app
        .world()
        .get::<MeshMaterial3d<StandardMaterial>>(mesh)
        .map(|worn| worn.0.id())
        .expect("the mesh starts in a standard material");

    call(&mut app, "material.apply", &[("material", relative.into())]);
    call(&mut app, "history.undo", &[]);

    assert!(
        app.world()
            .get::<MeshMaterial3d<LayeredSurfaceMaterial>>(mesh)
            .is_none(),
        "the layered surface comes off",
    );
    assert_eq!(
        app.world()
            .get::<MeshMaterial3d<StandardMaterial>>(mesh)
            .map(|worn| worn.0.id()),
        Some(before),
        "and the material the mesh wore is back",
    );
}

#[test]
fn a_scene_naming_a_layered_surface_reloads_with_it_on_the_mesh() {
    let (mut app, tmp) = editor();
    let relative = a_layered_surface(&mut app, "mossy");
    let scene = tmp.path().join("assets/cliffs.bsn");
    std::fs::write(
        &scene,
        "#cliff\nbevy_transform::components::transform::Transform\n",
    )
    .expect("the scene is written");
    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &scene);
    settle(&mut app);

    let mesh = named(&mut app, "cliff").expect("the scene spawned the mesh");
    let standard = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    app.world_mut()
        .entity_mut(mesh)
        .insert((Mesh3d::default(), MeshMaterial3d(standard)));
    app.world_mut().resource_mut::<Selection>().entities = vec![mesh];
    settle(&mut app);

    call(
        &mut app,
        "material.apply",
        &[("material", relative.clone().into())],
    );
    assert!(
        jackdaw::scene_io::save_scene(app.world_mut()),
        "the scene saves",
    );
    let text = std::fs::read_to_string(&scene).expect("the scene is on disk");
    assert!(
        text.contains(&relative),
        "the document names the file the mesh wears, got\n{text}",
    );

    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &scene);
    settle(&mut app);
    let reopened = named(&mut app, "cliff").expect("the scene spawned the mesh again");
    assert!(
        app.world()
            .get::<MeshMaterial3d<LayeredSurfaceMaterial>>(reopened)
            .is_some(),
        "and it comes back wearing the layered surface",
    );
}

/// The one entity the scene named, if it spawned one.
fn named(app: &mut App, name: &str) -> Option<Entity> {
    let found: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &Name)>()
        .iter(app.world())
        .filter(|(_, spawned)| spawned.as_str() == name)
        .map(|(entity, _)| entity)
        .collect();
    found.into_iter().next_back()
}

/// Dispatch an operator and apply its commands without letting a frame run,
/// which is what the renderer sees when an operator arrives late in the frame.
#[track_caller]
fn call_without_a_frame(app: &mut App, id: &'static str, params: &[(&'static str, PropertyValue)]) {
    let mut call = app.world_mut().operator(id).settings(CallOperatorSettings {
        execution_context: ExecutionContext::Invoke,
        creates_history_entry: true,
    });
    for (name, value) in params {
        call = call.param(*name, value.clone());
    }
    assert_eq!(
        call.call().expect("the operator dispatched"),
        OperatorResult::Finished,
        "{id} ran",
    );
    app.world_mut().flush();
}

/// The meshes whose material changed, as the renderer will read them when it
/// extracts this frame. One list covers every material type.
fn listed_for_respecialization(app: &App) -> &[Entity] {
    &app.world()
        .resource::<EntitiesNeedingSpecialization<StandardMaterial>>()
        .changed
}

/// Bevy collects the meshes whose material changed in `PostUpdate` and reads
/// the material itself when it extracts the frame. A swap between the two,
/// which is where every operator a remote session dispatches runs, has to say
/// so itself, or the mesh is drawn for a frame against the pipeline its old
/// material was specialized to. It holds in both directions: taking a layered
/// surface off again is the same swap backwards.
#[test]
fn swapping_a_meshs_material_lists_it_for_respecialization_either_way() {
    let (mut app, _tmp) = editor();
    let relative = a_layered_surface(&mut app, "mossy");
    let mesh = a_mesh_wearing_a_standard_material(&mut app);

    call_without_a_frame(&mut app, "material.apply", &[("material", relative.into())]);

    assert!(
        listed_for_respecialization(&app).contains(&mesh),
        "the apply has to list the mesh before the frame is extracted",
    );
    settle(&mut app);

    call_without_a_frame(&mut app, "history.undo", &[]);

    assert!(
        app.world()
            .get::<MeshMaterial3d<StandardMaterial>>(mesh)
            .is_some(),
        "the undo puts the standard material back",
    );
    assert!(
        listed_for_respecialization(&app).contains(&mesh),
        "and lists the mesh again, or the reverse swap draws against the layered pipeline",
    );
}
