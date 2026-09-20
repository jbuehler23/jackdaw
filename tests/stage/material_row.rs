//! The material a mesh the scene does not author wears.
//!
//! A model's parts are spawned by the loader and the scene holds no node for
//! them, so the inspector used to leave their material out altogether. The
//! row lists it and takes a choice, which lasts as long as the editor holds
//! what put the mesh there.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_bsn::SceneBsnAst;
use jackdaw_scene_types::PropertyValue;

use crate::util;

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

/// An editor with one material file under its assets.
fn editor_with_a_material() -> (App, tempfile::TempDir) {
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

    call(
        &mut app,
        "asset.new",
        &[
            ("type", "material".into()),
            ("name", "slate".into()),
            ("path", "materials".into()),
        ],
    );

    (app, tmp)
}

/// A mesh under an authored root with no document node of its own, which is
/// what a model's parts are, selected and showing.
fn app_with_unauthored_mesh() -> (App, tempfile::TempDir, Entity) {
    let (mut app, tmp) = editor_with_a_material();
    let root = app.world_mut().spawn(Name::new("lamp")).id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), root);
    let part = app
        .world_mut()
        .spawn((
            ChildOf(root),
            Mesh3d::default(),
            MeshMaterial3d(Handle::<StandardMaterial>::default()),
        ))
        .id();
    app.world_mut().resource_mut::<Selection>().entities = vec![part];
    settle(&mut app);
    (app, tmp, part)
}

/// The material a project file holds, as the handle behind it.
fn filed_material(app: &App) -> bevy::asset::UntypedHandle {
    app.world()
        .resource::<jackdaw::asset_index::AssetIndex>()
        .get(Path::new("materials/slate.bsn"))
        .and_then(|entry| entry.value.handle().cloned())
        .expect("the project holds the file the row names")
}

fn all_entities(app: &mut App) -> Vec<Entity> {
    app.world_mut()
        .query::<Entity>()
        .iter(app.world())
        .collect()
}

fn rows_writing(app: &mut App, field_path: &str) -> Vec<Entity> {
    all_entities(app)
        .into_iter()
        .filter(|&entity| {
            jackdaw::inspector::asset_field_shown_by(app.world(), entity)
                .is_some_and(|(_, field)| field == field_path)
        })
        .collect()
}

#[test]
fn a_mesh_the_scene_does_not_author_still_lists_its_material() {
    let (mut app, _tmp, _part) = app_with_unauthored_mesh();

    assert!(
        !rows_writing(&mut app, MATERIAL_FIELD).is_empty(),
        "the mesh's material gets a row of its own, not nothing at all",
    );
}

#[test]
fn picking_a_material_for_such_a_mesh_dresses_it_without_authoring_it() {
    let (mut app, _tmp, part) = app_with_unauthored_mesh();

    call(
        &mut app,
        "asset.pick",
        &[
            ("field", MATERIAL_FIELD.into()),
            ("value", "materials/slate.bsn".into()),
        ],
    );

    let chosen = filed_material(&app);
    let worn = app
        .world()
        .get::<MeshMaterial3d<StandardMaterial>>(part)
        .expect("the mesh still wears a material");
    assert_eq!(
        worn.0.id().untyped(),
        chosen.id(),
        "the mesh wears the material the row chose",
    );
    assert!(
        app.world()
            .resource::<SceneBsnAst>()
            .ast_for(part)
            .is_none(),
        "and the scene still says nothing about a mesh it does not author",
    );
}

#[test]
fn a_brush_takes_the_material_its_row_names_onto_its_faces() {
    let (mut app, _tmp) = editor_with_a_material();
    let brush = app
        .world_mut()
        .spawn((
            Name::new("wall"),
            jackdaw_scene_types::Brush::cuboid(0.5, 0.5, 0.5),
            Transform::default(),
            Visibility::default(),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), brush);
    app.world_mut().resource_mut::<Selection>().entities = vec![brush];
    settle(&mut app);

    assert!(
        !rows_writing(&mut app, MATERIAL_FIELD).is_empty(),
        "a brush face names its material on a row like any other asset field",
    );

    call(
        &mut app,
        "asset.pick",
        &[
            ("field", MATERIAL_FIELD.into()),
            ("value", "materials/slate.bsn".into()),
        ],
    );

    let chosen = filed_material(&app);
    let faces = app
        .world()
        .get::<jackdaw_scene_types::Brush>(brush)
        .expect("the brush is still there");
    assert!(
        faces
            .faces
            .iter()
            .all(|face| face.material.id().untyped() == chosen.id()),
        "every face wears the material the row chose",
    );
}

/// The operator a caller outside the editor reaches for takes the material's
/// path and puts it on the selected brush's faces.
#[test]
fn a_brush_takes_the_material_the_apply_operator_names_by_path() {
    let (mut app, _tmp) = editor_with_a_material();
    let brush = app
        .world_mut()
        .spawn((
            Name::new("wall"),
            jackdaw_scene_types::Brush::cuboid(0.5, 0.5, 0.5),
            Transform::default(),
            Visibility::default(),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), brush);
    app.world_mut().resource_mut::<Selection>().entities = vec![brush];
    settle(&mut app);

    call(
        &mut app,
        "material.apply",
        &[("material", "materials/slate.bsn".into())],
    );

    let chosen = filed_material(&app);
    let faces = app
        .world()
        .get::<jackdaw_scene_types::Brush>(brush)
        .expect("the brush is still there");
    assert!(
        faces
            .faces
            .iter()
            .all(|face| face.material.id().untyped() == chosen.id()),
        "every face wears the material the path named",
    );
}

/// A mesh the scene authors keeps the material picked on it, which the
/// document records on the component the mesh wears it on.
#[test]
fn a_material_picked_on_an_authored_mesh_survives_a_save_and_a_reopen() {
    let (mut app, tmp) = editor_with_a_material();
    let scene = tmp.path().join("assets/hall.bsn");
    std::fs::write(
        &scene,
        "#lamp\nbevy_transform::components::transform::Transform\n",
    )
    .expect("the scene is written");
    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &scene);
    settle(&mut app);

    let mesh = last_named(&mut app, "lamp").expect("the scene spawned the mesh");
    let empty = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    app.world_mut()
        .entity_mut(mesh)
        .insert((Mesh3d::default(), MeshMaterial3d(empty)));
    app.world_mut().resource_mut::<Selection>().entities = vec![mesh];
    settle(&mut app);

    call(
        &mut app,
        "asset.pick",
        &[
            ("field", MATERIAL_FIELD.into()),
            ("value", "materials/slate.bsn".into()),
        ],
    );
    assert!(
        jackdaw::scene_io::save_scene(app.world_mut()),
        "the scene saves",
    );

    let text = std::fs::read_to_string(&scene).expect("the scene is on disk");
    assert!(
        text.contains("materials/slate.bsn"),
        "the document names the material the mesh wears, got\n{text}",
    );

    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &scene);
    settle(&mut app);
    let reopened = last_named(&mut app, "lamp").expect("the scene spawned the mesh again");
    let chosen = filed_material(&app);
    assert_eq!(
        app.world()
            .get::<MeshMaterial3d<StandardMaterial>>(reopened)
            .map(|worn| worn.0.id().untyped()),
        Some(chosen.id()),
        "and it comes back wearing it",
    );
}

/// The most recently spawned entity under a name, so a reopen is read rather
/// than the run before it.
fn last_named(app: &mut App, name: &str) -> Option<Entity> {
    let found: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &Name)>()
        .iter(app.world())
        .filter(|(_, spawned)| spawned.as_str() == name)
        .map(|(entity, _)| entity)
        .collect();
    found.into_iter().next_back()
}
