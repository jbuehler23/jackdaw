//! Materials a placed model's parts wear in place of their own, kept with the scene.

use std::path::{Path, PathBuf};

use bevy::gltf::GltfMaterialName;
use bevy::prelude::*;
use jackdaw::definition_assets::LAYERED_SURFACE_KIND;
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_scene_types::{GltfSource, MaterialOverrides, PropertyValue};
use jackdaw_surface::{LayeredSurfaceMaterial, WornMaterial};

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

/// An editor on a project of its own with a layered surface file and a scene holding one cliff.
fn editor_with_a_cliff() -> (App, tempfile::TempDir, PathBuf, String) {
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

    call(
        &mut app,
        "asset.new",
        &[
            ("type", LAYERED_SURFACE_KIND.into()),
            ("name", "mossy".into()),
            ("path", "materials".into()),
        ],
    );

    let scene = tmp.path().join("assets/valley.bsn");
    std::fs::write(
        &scene,
        "#cliff\nbevy_transform::components::transform::Transform\n",
    )
    .expect("the scene is written");
    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &scene);
    settle(&mut app);
    let cliff = named(&mut app, "cliff").expect("the scene spawned the cliff");
    make_a_model(&mut app, cliff);
    (app, tmp, scene, "materials/mossy.bsn".to_string())
}

/// Give `root` a model source in the document, as a placed model has.
fn make_a_model(app: &mut App, root: Entity) {
    let source = GltfSource {
        path: "models/cliff.glb".to_string(),
        scene_index: 0,
    };
    app.world_mut().entity_mut(root).insert(source.clone());
    jackdaw::commands::sync_component_to_ast(
        app.world_mut(),
        root,
        "jackdaw_scene_types::types::GltfSource",
        &source,
    );
    settle(app);
}

/// A part of the model as the glTF loader leaves one: no document node, a material name and the model's own material.
fn a_part(app: &mut App, root: Entity, material_name: &str) -> (Entity, Handle<StandardMaterial>) {
    let own = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    let part = app
        .world_mut()
        .spawn((
            GltfMaterialName(material_name.to_string()),
            Mesh3d::default(),
            MeshMaterial3d(own.clone()),
            ChildOf(root),
        ))
        .id();
    settle(app);
    (part, own)
}

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

fn wears_a_layered_surface(app: &App, part: Entity) -> bool {
    matches!(
        WornMaterial::of(app.world(), part),
        Some(WornMaterial::Layered(handle)) if handle != Handle::<LayeredSurfaceMaterial>::default()
    )
}

#[test]
fn an_override_on_a_placed_model_saves_reloads_and_dresses_its_part() {
    let (mut app, _tmp, scene, mossy) = editor_with_a_cliff();
    let cliff = named(&mut app, "cliff").expect("the cliff");
    let (rock, _) = a_part(&mut app, cliff, "Rock");
    let (trim, trim_own) = a_part(&mut app, cliff, "Trim");

    call(
        &mut app,
        "material.override",
        &[
            ("entity", cliff.into()),
            ("name", "Rock".into()),
            ("material", mossy.clone().into()),
        ],
    );
    assert!(
        wears_a_layered_surface(&app, rock),
        "the named part is dressed"
    );
    assert_eq!(
        WornMaterial::of(app.world(), trim),
        Some(WornMaterial::Standard(trim_own)),
        "and the other part keeps its own"
    );

    assert!(
        jackdaw::scene_io::save_scene(app.world_mut()),
        "the scene saves"
    );
    let text = std::fs::read_to_string(&scene).expect("the scene is on disk");
    assert!(
        text.contains("MaterialOverrides") && text.contains(&mossy),
        "the document carries the override, got\n{text}"
    );

    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &scene);
    settle(&mut app);
    let cliff = named(&mut app, "cliff").expect("the cliff again");
    assert_eq!(
        app.world()
            .get::<MaterialOverrides>(cliff)
            .and_then(|overrides| overrides.materials.get("Rock").cloned()),
        Some(mossy),
    );
    let (rock, _) = a_part(&mut app, cliff, "Rock");
    assert!(
        wears_a_layered_surface(&app, rock),
        "a part the reloaded model spawns is dressed again"
    );
}

#[test]
fn applying_a_material_to_a_models_part_overrides_it_on_the_model_and_undo_takes_it_back() {
    let (mut app, _tmp, _scene, mossy) = editor_with_a_cliff();
    let cliff = named(&mut app, "cliff").expect("the cliff");
    let (rock, own) = a_part(&mut app, cliff, "Rock");
    app.world_mut().resource_mut::<Selection>().entities = vec![rock];
    settle(&mut app);

    call(
        &mut app,
        "material.apply",
        &[("material", mossy.clone().into())],
    );

    assert_eq!(
        app.world()
            .get::<MaterialOverrides>(cliff)
            .map(|overrides| overrides.materials.clone()),
        Some([("Rock".to_string(), mossy)].into_iter().collect()),
        "the override lands on the placed model, where the scene keeps it"
    );
    assert!(wears_a_layered_surface(&app, rock));

    call(&mut app, "history.undo", &[]);

    assert!(app.world().get::<MaterialOverrides>(cliff).is_none());
    assert_eq!(
        WornMaterial::of(app.world(), rock),
        Some(WornMaterial::Standard(own)),
        "the part wears its own material again"
    );
}

#[test]
fn an_override_naming_no_material_is_refused() {
    let (mut app, _tmp, _scene, _) = editor_with_a_cliff();
    let cliff = named(&mut app, "cliff").expect("the cliff");
    a_part(&mut app, cliff, "Rock");

    let result = app
        .world_mut()
        .operator("material.override")
        .param("entity", cliff)
        .param("material", "materials/none.bsn")
        .call()
        .expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Cancelled);
    assert!(app.world().get::<MaterialOverrides>(cliff).is_none());
}

/// The instances of the prefab at `prefab` the scene holds.
fn instances_of(app: &mut App, prefab: &Path) -> Vec<Entity> {
    let mut query = app.world_mut().query::<(Entity, &jackdaw::prefab::IsA)>();
    query
        .iter(app.world())
        .filter(|(_, is_a)| {
            is_a.source
                .ends_with(prefab.file_name().expect("a prefab file"))
        })
        .map(|(entity, _)| entity)
        .collect()
}

/// The model a prefab instance wraps.
fn model_under(app: &App, instance: Entity) -> Entity {
    jackdaw::material_overrides::model_root(app.world(), instance)
        .expect("the instance wraps one model")
}

fn a_second_layered_surface(app: &mut App) -> String {
    call(
        app,
        "asset.new",
        &[
            ("type", LAYERED_SURFACE_KIND.into()),
            ("name", "wet".into()),
            ("path", "materials".into()),
        ],
    );
    "materials/wet.bsn".to_string()
}

fn layered_handle(app: &App, part: Entity) -> Option<Handle<LayeredSurfaceMaterial>> {
    match WornMaterial::of(app.world(), part) {
        Some(WornMaterial::Layered(handle)) => Some(handle),
        _ => None,
    }
}

#[test]
fn a_models_overrides_survive_packing_into_a_prefab_and_dress_every_instance() {
    let (mut app, tmp, _scene, mossy) = editor_with_a_cliff();
    let cliff = named(&mut app, "cliff").expect("the cliff");
    call(
        &mut app,
        "material.override",
        &[
            ("entity", cliff.into()),
            ("name", "Rock".into()),
            ("material", mossy.clone().into()),
        ],
    );

    call(
        &mut app,
        "prefab.pack",
        &[
            ("entity", cliff.into()),
            ("path", "prefabs/cliff.bsn".into()),
        ],
    );
    let prefab = tmp.path().join("assets/prefabs/cliff.bsn");
    let text = std::fs::read_to_string(&prefab).expect("the prefab is written");
    assert!(
        text.contains("MaterialOverrides") && text.contains(&mossy),
        "the prefab carries the override, got\n{text}"
    );

    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &prefab, Vec3::new(20.0, 0.0, 0.0));
    settle(&mut app);
    let instances = instances_of(&mut app, &prefab);
    assert_eq!(instances.len(), 2, "the packed cliff and a new instance");
    for instance in instances {
        let model = model_under(&app, instance);
        let (rock, _) = a_part(&mut app, model, "Rock");
        assert!(
            wears_a_layered_surface(&app, rock),
            "every instance of the prefab dresses its part"
        );
    }
}

#[test]
fn an_instances_own_override_wins_over_its_prefabs() {
    let (mut app, tmp, scene, mossy) = editor_with_a_cliff();
    let wet = a_second_layered_surface(&mut app);
    let cliff = named(&mut app, "cliff").expect("the cliff");
    call(
        &mut app,
        "material.override",
        &[
            ("entity", cliff.into()),
            ("name", "Rock".into()),
            ("material", mossy.into()),
        ],
    );
    call(
        &mut app,
        "prefab.pack",
        &[
            ("entity", cliff.into()),
            ("path", "prefabs/cliff.bsn".into()),
        ],
    );
    let prefab = tmp.path().join("assets/prefabs/cliff.bsn");
    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &prefab, Vec3::new(20.0, 0.0, 0.0));
    settle(&mut app);
    let instances = instances_of(&mut app, &prefab);
    let (first, second) = (instances[0], instances[1]);
    let (first_model, second_model) = (model_under(&app, first), model_under(&app, second));
    let (first_rock, _) = a_part(&mut app, first_model, "Rock");
    let (second_rock, _) = a_part(&mut app, second_model, "Rock");

    call(
        &mut app,
        "material.override",
        &[
            ("entity", second.into()),
            ("name", "Rock".into()),
            ("material", wet.clone().into()),
        ],
    );

    assert!(
        jackdaw::scene_io::save_scene(app.world_mut()),
        "the scene saves"
    );
    let text = std::fs::read_to_string(&scene).expect("the scene is on disk");
    assert!(
        text.contains(&wet),
        "the instance's own override is kept with the scene, got\n{text}"
    );

    let first_wears = layered_handle(&app, first_rock).expect("the first instance is dressed");
    let second_wears = layered_handle(&app, second_rock).expect("the second instance is dressed");
    assert_ne!(
        first_wears, second_wears,
        "the second instance wears its own material and the first keeps the prefab's"
    );
}

/// The layered surface the project holds at `path`.
fn layered_at(app: &App, path: &str) -> Handle<LayeredSurfaceMaterial> {
    app.world()
        .resource::<jackdaw::asset_index::AssetIndex>()
        .get(Path::new(path))
        .and_then(|entry| entry.value.handle().cloned())
        .and_then(|handle| handle.try_typed::<LayeredSurfaceMaterial>().ok())
        .unwrap_or_else(|| panic!("the project holds {path}"))
}

/// Write new contents for a prefab, reload its instances, and let the file watcher's own reload of the same write pass.
fn rewrite_prefab(app: &mut App, path: &Path, text: &str) {
    let sparse = jackdaw::prefab::watcher::capture_sparse_scene_text(app.world_mut())
        .expect("the scene has a live document");
    std::fs::write(path, text).expect("the prefab is rewritten");
    let assets_root = jackdaw::prefab::save_load::source_root_of(app.world(), path);
    let ast = jackdaw::prefab::save_load::read_prefab_ast(path, &assets_root)
        .expect("the new prefab parses");
    app.world_mut()
        .resource_mut::<jackdaw::prefab::PrefabAstCache>()
        .insert(path, ast);
    jackdaw::prefab::watcher::reload_instances_of(app.world_mut(), &sparse, path);
    std::thread::sleep(std::time::Duration::from_millis(400));
    settle(app);
}

#[test]
fn an_instance_keeps_following_the_prefab_entries_it_did_not_override() {
    let (mut app, tmp, _scene, mossy) = editor_with_a_cliff();
    let wet = a_second_layered_surface(&mut app);
    let cliff = named(&mut app, "cliff").expect("the cliff");
    for name in ["Rock", "Trim"] {
        call(
            &mut app,
            "material.override",
            &[
                ("entity", cliff.into()),
                ("name", name.into()),
                ("material", mossy.clone().into()),
            ],
        );
    }
    call(
        &mut app,
        "prefab.pack",
        &[
            ("entity", cliff.into()),
            ("path", "prefabs/cliff.bsn".into()),
        ],
    );
    let prefab = tmp.path().join("assets/prefabs/cliff.bsn");
    let instance = instances_of(&mut app, &prefab)[0];

    call(
        &mut app,
        "material.override",
        &[
            ("entity", instance.into()),
            ("name", "Rock".into()),
            ("material", wet.clone().into()),
        ],
    );
    assert_eq!(
        app.world()
            .get::<jackdaw_scene_types::InstanceMaterialOverrides>(instance)
            .map(|overrides| overrides.materials.clone()),
        Some([("Rock".to_string(), wet.clone())].into_iter().collect()),
        "the instance records only the entry it changed"
    );

    let text = std::fs::read_to_string(&prefab).expect("the prefab is on disk");
    let trim_at = text
        .rfind(&mossy)
        .expect("the prefab names the trim's material");
    let edited = format!(
        "{}{}{}",
        &text[..trim_at],
        wet,
        &text[trim_at + mossy.len()..]
    );
    rewrite_prefab(&mut app, &prefab, &edited);

    let instance = instances_of(&mut app, &prefab)[0];
    let model = model_under(&app, instance);
    let (rock, _) = a_part(&mut app, model, "Rock");
    let (trim, _) = a_part(&mut app, model, "Trim");
    let wet_handle = layered_at(&app, &wet);
    assert_eq!(
        layered_handle(&app, rock),
        Some(wet_handle.clone()),
        "the instance's own entry"
    );
    assert_eq!(
        layered_handle(&app, trim),
        Some(wet_handle),
        "and the prefab's edited entry, which the instance never overrode"
    );
}
