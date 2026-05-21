use bevy::prelude::*;

mod util;

#[test]
fn prefab_components_register() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(jackdaw::prefab::PrefabPlugin);
    let world = app.world();
    let registry = world.resource::<AppTypeRegistry>().read();
    assert!(
        registry
            .get(std::any::TypeId::of::<jackdaw::prefab::Prefab>())
            .is_some(),
        "Prefab is registered"
    );
    assert!(
        registry
            .get(std::any::TypeId::of::<jackdaw::prefab::PrefabEntityId>())
            .is_some(),
        "PrefabEntityId is registered"
    );
    assert!(
        registry
            .get(std::any::TypeId::of::<jackdaw::prefab::IsA>())
            .is_some(),
        "IsA is registered"
    );
}

#[test]
fn prefab_cache_stores_and_retrieves() {
    use std::path::PathBuf;
    let mut cache = jackdaw::prefab::PrefabAstCache::default();
    let path = PathBuf::from("assets/prefabs/rock.jsn");
    let ast = jackdaw_jsn::SceneJsnAst::default();

    assert!(cache.get(&path).is_none());
    cache.insert(path.clone(), ast.clone());
    assert!(cache.get(&path).is_some());

    cache.invalidate(&path);
    assert!(cache.get(&path).is_none());
}

#[test]
fn override_applier_sets_scalar_field() {
    use serde_json::json;
    let mut base = json!({
        "translation": [0.0, 0.0, 0.0],
        "scale": { "x": 1.0, "y": 1.0, "z": 1.0 }
    });
    let deltas = json!({
        "scale.x": 1.5
    });
    jackdaw::prefab::overrides::apply_deltas(&mut base, &deltas).expect("applier handles dot-path");
    assert_eq!(base["scale"]["x"].as_f64(), Some(1.5));
    assert_eq!(base["scale"]["y"].as_f64(), Some(1.0));
}

#[test]
fn override_applier_sets_nested_struct() {
    use serde_json::json;
    let mut base = json!({
        "translation": { "x": 0.0, "y": 0.0, "z": 0.0 }
    });
    let deltas = json!({
        "translation": { "x": 10.0, "y": 5.0, "z": 0.0 }
    });
    jackdaw::prefab::overrides::apply_deltas(&mut base, &deltas).unwrap();
    assert_eq!(base["translation"]["x"].as_f64(), Some(10.0));
    assert_eq!(base["translation"]["y"].as_f64(), Some(5.0));
}

#[test]
fn cycle_detector_accepts_simple_chain() {
    use std::path::PathBuf;
    let chain = vec![
        PathBuf::from("scene.jsn"),
        PathBuf::from("a.jsn"),
        PathBuf::from("b.jsn"),
    ];
    let next = PathBuf::from("c.jsn");
    assert!(jackdaw::prefab::resolver::would_cycle(&chain, &next).is_none());
}

#[test]
fn cycle_detector_rejects_revisit() {
    use std::path::PathBuf;
    let chain = vec![PathBuf::from("a.jsn"), PathBuf::from("b.jsn")];
    let next = PathBuf::from("a.jsn");
    let err =
        jackdaw::prefab::resolver::would_cycle(&chain, &next).expect("cycle should be reported");
    assert!(err.to_string().contains("a.jsn"));
}

#[test]
fn resolver_materializes_inherited_subtree() {
    let mut prefab_ast = jackdaw_jsn::SceneJsnAst::default();
    let root = prefab_ast.add_root();
    prefab_ast.insert_component(
        root,
        "jackdaw::prefab::components::Prefab",
        serde_json::Value::Null,
    );
    prefab_ast.insert_component(
        root,
        "jackdaw::prefab::components::PrefabEntityId",
        serde_json::json!(0),
    );
    let child = prefab_ast.add_child(root);
    prefab_ast.insert_component(
        child,
        "jackdaw::prefab::components::PrefabEntityId",
        serde_json::json!(1),
    );
    prefab_ast.insert_component(
        child,
        "bevy_transform::components::transform::Transform",
        serde_json::json!({ "translation": [0.0, 1.0, 0.0] }),
    );

    let mut scene_ast = jackdaw_jsn::SceneJsnAst::default();
    let instance_root = scene_ast.add_root();
    scene_ast.insert_component(
        instance_root,
        "jackdaw::prefab::components::IsA",
        serde_json::json!({ "source": "prefab.jsn", "deleted": [] }),
    );

    let mut cache = jackdaw::prefab::PrefabAstCache::default();
    cache.insert(std::path::PathBuf::from("prefab.jsn"), prefab_ast);

    let resolved =
        jackdaw::prefab::resolver::resolve_scene(&scene_ast, &cache).expect("resolution succeeds");

    let kids: Vec<_> = resolved.children_of(instance_root).collect();
    assert_eq!(
        kids.len(),
        1,
        "instance has one inherited child after resolution"
    );
}

#[test]
fn resolver_rejects_isa_cycle() {
    let mut prefab_ast = jackdaw_jsn::SceneJsnAst::default();
    let root = prefab_ast.add_root();
    prefab_ast.insert_component(
        root,
        "jackdaw::prefab::components::Prefab",
        serde_json::Value::Null,
    );
    prefab_ast.insert_component(
        root,
        "jackdaw::prefab::components::PrefabEntityId",
        serde_json::json!(0),
    );
    prefab_ast.insert_component(
        root,
        "jackdaw::prefab::components::IsA",
        serde_json::json!({ "source": "self.jsn", "deleted": [] }),
    );

    let mut scene_ast = jackdaw_jsn::SceneJsnAst::default();
    let inst = scene_ast.add_root();
    scene_ast.insert_component(
        inst,
        "jackdaw::prefab::components::IsA",
        serde_json::json!({ "source": "self.jsn", "deleted": [] }),
    );

    let mut cache = jackdaw::prefab::PrefabAstCache::default();
    cache.insert(std::path::PathBuf::from("self.jsn"), prefab_ast);

    let err = jackdaw::prefab::resolver::resolve_scene(&scene_ast, &cache);
    assert!(err.is_err(), "self-referential IsA must error");
}

fn make_app_for_prefab_tests() -> bevy::prelude::App {
    use bevy::prelude::*;
    use bevy::render::RenderPlugin;
    use bevy::render::settings::{RenderCreation, WgpuSettings};
    use bevy::winit::WinitPlugin;

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(RenderPlugin {
                render_creation: RenderCreation::Automatic(WgpuSettings {
                    backends: None,
                    ..default()
                }),
                ..default()
            })
            .disable::<WinitPlugin>(),
    );
    app.add_plugins(jackdaw_jsn::JsnPlugin::default());
    app.add_plugins(jackdaw::prefab::PrefabPlugin);
    app.init_resource::<jackdaw::commands::CommandHistory>();
    app.init_resource::<jackdaw::scene_io::SceneFilePath>();
    app.init_resource::<jackdaw::scene_io::SceneDirtyState>();
    app.init_resource::<jackdaw_jsn::SceneJsnAst>();
    app.init_resource::<jackdaw::selection::Selection>();
    app
}

#[test]
fn load_resolves_isa_and_caches_prefab() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("rock.jsn");
    let scene_path = tmp.path().join("level.jsn");

    let prefab_jsn = r#"{
        "jsn": { "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" },
        "metadata": { "name": "rock", "created": "", "modified": "" },
        "assets": {},
        "scene": [{
            "components": {
                "jackdaw::prefab::components::Prefab": null,
                "jackdaw::prefab::components::PrefabEntityId": 0,
                "bevy_ecs::name::Name": "rock"
            }
        }]
    }"#;
    std::fs::write(&prefab_path, prefab_jsn).unwrap();

    let scene_jsn = format!(
        r#"{{
            "jsn": {{ "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" }},
            "metadata": {{ "name": "level", "created": "", "modified": "" }},
            "assets": {{}},
            "scene": [{{
                "components": {{
                    "jackdaw::prefab::components::IsA": {{ "source": "{}", "deleted": [] }},
                    "jackdaw::prefab::components::PrefabEntityId": 0
                }}
            }}]
        }}"#,
        prefab_path.to_str().unwrap()
    );
    std::fs::write(&scene_path, scene_jsn).unwrap();

    let mut app = make_app_for_prefab_tests();
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene_path);

    let cache = app.world().resource::<jackdaw::prefab::PrefabAstCache>();
    assert!(
        cache.get(&prefab_path).is_some(),
        "prefab is cached after load (cache keys: {:?})",
        cache.paths().collect::<Vec<_>>()
    );
}

#[test]
fn load_resolves_isa_spawns_inherited_entities() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("cluster.jsn");
    let scene_path = tmp.path().join("level.jsn");

    let prefab_jsn = r#"{
        "jsn": { "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" },
        "metadata": { "name": "cluster", "created": "", "modified": "" },
        "assets": {},
        "scene": [
            {
                "components": {
                    "jackdaw::prefab::components::Prefab": null,
                    "jackdaw::prefab::components::PrefabEntityId": 0,
                    "bevy_ecs::name::Name": "cluster_root"
                }
            },
            {
                "parent": 0,
                "components": {
                    "jackdaw::prefab::components::PrefabEntityId": 1,
                    "bevy_ecs::name::Name": "inherited_rock"
                }
            }
        ]
    }"#;
    std::fs::write(&prefab_path, prefab_jsn).unwrap();

    let scene_jsn = format!(
        r#"{{
            "jsn": {{ "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" }},
            "metadata": {{ "name": "level", "created": "", "modified": "" }},
            "assets": {{}},
            "scene": [{{
                "components": {{
                    "jackdaw::prefab::components::IsA": {{ "source": "{}", "deleted": [] }},
                    "jackdaw::prefab::components::PrefabEntityId": 0,
                    "bevy_ecs::name::Name": "instance"
                }}
            }}]
        }}"#,
        prefab_path.to_str().unwrap()
    );
    std::fs::write(&scene_path, scene_jsn).unwrap();

    let mut app = make_app_for_prefab_tests();
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene_path);

    let mut name_q = app.world_mut().query::<&bevy::prelude::Name>();
    let names: Vec<String> = name_q
        .iter(app.world())
        .map(|n| n.as_str().to_string())
        .collect();
    assert!(
        names.iter().any(|n| n == "inherited_rock"),
        "inherited entity spawned, names: {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "instance"),
        "instance root spawned, names: {names:?}"
    );
}

#[test]
fn save_writes_sparse_deltas_only() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");
    let scene_path = tmp.path().join("s.jsn");

    // Prefab with a default Transform.
    let prefab_jsn = r#"{
        "jsn": { "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" },
        "metadata": { "name": "p", "created": "", "modified": "" },
        "assets": {},
        "scene": [{
            "components": {
                "jackdaw::prefab::components::Prefab": null,
                "jackdaw::prefab::components::PrefabEntityId": 0,
                "bevy_transform::components::transform::Transform": {
                    "translation": [0.0, 0.0, 0.0],
                    "rotation": [0.0, 0.0, 0.0, 1.0],
                    "scale": [1.0, 1.0, 1.0]
                }
            }
        }]
    }"#;
    std::fs::write(&prefab_path, prefab_jsn).unwrap();

    // Scene with one instance, sparse Transform override (translation only).
    let scene_jsn = format!(
        r#"{{
            "jsn": {{ "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" }},
            "metadata": {{ "name": "s", "created": "", "modified": "" }},
            "assets": {{}},
            "scene": [{{
                "components": {{
                    "jackdaw::prefab::components::IsA": {{ "source": "{}", "deleted": [] }},
                    "jackdaw::prefab::components::PrefabEntityId": 0,
                    "bevy_transform::components::transform::Transform": {{ "translation": [10.0, 0.0, 0.0] }}
                }}
            }}]
        }}"#,
        prefab_path.to_str().unwrap()
    );
    std::fs::write(&scene_path, scene_jsn).unwrap();

    let mut app = make_app_for_prefab_tests();
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene_path);

    // Tell save_scene which file to write back.
    app.world_mut()
        .resource_mut::<jackdaw::scene_io::SceneFilePath>()
        .path = Some(scene_path.to_string_lossy().into_owned());
    jackdaw::scene_io::save_scene(app.world_mut());

    // save_scene spawns a task pool job; give it a tick to land.
    std::thread::sleep(std::time::Duration::from_millis(200));

    let written = std::fs::read_to_string(&scene_path).expect("file exists");
    let value: serde_json::Value = serde_json::from_str(&written).expect("valid json on disk");

    // Find the entry that has the IsA component (its index in scene[]
    // may not be 0 if inherited entities also get serialized).
    let scene_arr = value["scene"].as_array().expect("scene is array");
    let instance = scene_arr
        .iter()
        .find(|e| {
            e["components"]
                .get("jackdaw::prefab::components::IsA")
                .is_some()
        })
        .expect("instance entity present on disk");
    let transform = &instance["components"]["bevy_transform::components::transform::Transform"];
    assert!(
        transform.get("translation").is_some(),
        "sparse delta keeps translation; got {transform:?}"
    );
    assert!(
        transform.get("rotation").is_none(),
        "sparse delta drops rotation; got {transform:?}"
    );
    assert!(
        transform.get("scale").is_none(),
        "sparse delta drops scale; got {transform:?}"
    );
}

#[test]
fn save_as_prefab_writes_file_and_converts_in_place() {
    use bevy::prelude::*;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_target = tmp.path().join("brush_prefab.jsn");

    let mut app = make_app_for_prefab_tests();

    // Spawn a simple entity with a Name so reflect-based serialization
    // has something concrete to write.
    let entity = app.world_mut().spawn(Name::new("test_entity")).id();

    jackdaw::prefab::operators::save_as_prefab_from_selection(
        app.world_mut(),
        &[entity],
        &prefab_target,
    );

    assert!(prefab_target.exists(), "prefab file written");
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&prefab_target).unwrap())
            .expect("prefab file is valid JSON");
    assert!(
        written["scene"][0]["components"]
            .get("jackdaw::prefab::components::Prefab")
            .is_some(),
        "synthetic root has Prefab marker; got {written:?}"
    );
    assert!(
        written["scene"][0]["components"]
            .get("jackdaw::prefab::components::PrefabEntityId")
            .is_some(),
        "synthetic root has PrefabEntityId(0)"
    );

    // After conversion, a new instance node carrying IsA was inserted.
    let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
    let has_isa = ast
        .entities_with_component("jackdaw::prefab::components::IsA")
        .next()
        .is_some();
    assert!(has_isa, "new instance node carrying IsA inserted");

    // The prefab is now in the cache (so re-resolving the scene works).
    let cache = app.world().resource::<jackdaw::prefab::PrefabAstCache>();
    assert!(
        cache.get(&prefab_target).is_some(),
        "newly-written prefab is cached"
    );
}

fn prefab_with_name(n: &str) -> String {
    format!(
        r#"{{
        "jsn": {{ "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" }},
        "metadata": {{ "name": "p", "created": "", "modified": "" }},
        "assets": {{}},
        "scene": [{{
            "components": {{
                "jackdaw::prefab::components::Prefab": null,
                "jackdaw::prefab::components::PrefabEntityId": 0,
                "bevy_ecs::name::Name": "{n}"
            }}
        }}]
    }}"#
    )
}

fn scene_referencing(prefab: &std::path::Path) -> String {
    format!(
        r#"{{
        "jsn": {{ "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" }},
        "metadata": {{ "name": "s", "created": "", "modified": "" }},
        "assets": {{}},
        "scene": [{{
            "components": {{
                "jackdaw::prefab::components::IsA": {{ "source": "{}", "deleted": [] }},
                "jackdaw::prefab::components::PrefabEntityId": 0
            }}
        }}]
    }}"#,
        prefab.to_string_lossy()
    )
}

fn current_names(app: &mut bevy::prelude::App) -> Vec<String> {
    let mut q = app.world_mut().query::<&bevy::prelude::Name>();
    q.iter(app.world())
        .map(|n| n.as_str().to_string())
        .collect()
}

#[test]
fn prefab_file_change_triggers_reload() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");
    let scene_path = tmp.path().join("s.jsn");
    std::fs::write(&prefab_path, prefab_with_name("v1")).unwrap();
    std::fs::write(&scene_path, scene_referencing(&prefab_path)).unwrap();

    let mut app = make_app_for_prefab_tests();
    app.add_plugins(jackdaw::prefab::watcher::PrefabWatcherPlugin);
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene_path);

    let initial = current_names(&mut app);
    assert!(
        initial.iter().any(|n| n == "v1"),
        "initial load sees v1; got {initial:?}"
    );

    // Modify the prefab on disk.
    std::fs::write(&prefab_path, prefab_with_name("v2")).unwrap();

    // Poll the app for up to 3 seconds waiting for the watcher to fire,
    // debounce, and re-resolve. Filesystem event latency varies by OS;
    // generous deadline so this isn't flaky in CI.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        app.update();
        if current_names(&mut app).iter().any(|n| n == "v2") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let final_names = current_names(&mut app);
    assert!(
        final_names.iter().any(|n| n == "v2"),
        "watcher reloaded prefab; v2 should be in world. Got {final_names:?}"
    );
}

#[test]
fn spawn_instance_caches_and_spawns_inherited_entity() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("rock.jsn");
    std::fs::write(
        &prefab_path,
        r#"{
            "jsn": { "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" },
            "metadata": { "name": "rock", "created": "", "modified": "" },
            "assets": {},
            "scene": [{
                "components": {
                    "jackdaw::prefab::components::Prefab": null,
                    "jackdaw::prefab::components::PrefabEntityId": 0,
                    "bevy_ecs::name::Name": "rock_marker"
                }
            }]
        }"#,
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();

    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        bevy::math::Vec3::new(7.0, 0.0, 0.0),
    );

    let cache = app.world().resource::<jackdaw::prefab::PrefabAstCache>();
    assert!(cache.get(&prefab_path).is_some(), "prefab cached");

    let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
    let isa_idx = ast
        .entities_with_component("jackdaw::prefab::components::IsA")
        .next()
        .expect("instance has IsA");
    let tx = ast
        .get_component_at(isa_idx, "bevy_transform::components::transform::Transform")
        .expect("instance has Transform");
    let translation = tx["translation"].as_array().unwrap();
    assert_eq!(translation[0].as_f64(), Some(7.0));

    let mut q = app.world_mut().query::<&bevy::prelude::Name>();
    let names: Vec<String> = q
        .iter(app.world())
        .map(|n| n.as_str().to_string())
        .collect();
    assert!(
        names.iter().any(|n| n == "rock_marker"),
        "inherited entity spawned; names: {names:?}"
    );
}

#[test]
fn spawn_instance_reuses_cached_prefab() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");
    std::fs::write(
        &prefab_path,
        r#"{
            "jsn": { "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" },
            "metadata": { "name": "p", "created": "", "modified": "" },
            "assets": {},
            "scene": [{
                "components": {
                    "jackdaw::prefab::components::Prefab": null,
                    "jackdaw::prefab::components::PrefabEntityId": 0,
                    "bevy_ecs::name::Name": "p_root"
                }
            }]
        }"#,
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();

    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        bevy::math::Vec3::ZERO,
    );

    // Mutate the on-disk file. The second spawn should not pick up the
    // change, because the cache already has the original. Pins the
    // "caches if missing, otherwise reuses" semantics.
    std::fs::write(&prefab_path, "{}").unwrap();

    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        bevy::math::Vec3::new(2.0, 0.0, 0.0),
    );

    let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
    let instance_count = ast
        .entities_with_component("jackdaw::prefab::components::IsA")
        .count();
    assert_eq!(instance_count, 2, "two instances landed in the AST");
}

#[test]
fn field_is_overridden_detects_changed_field() {
    let mut prefab_ast = jackdaw_jsn::SceneJsnAst::default();
    let root = prefab_ast.add_root();
    prefab_ast.insert_component(
        root,
        "jackdaw::prefab::components::Prefab",
        serde_json::Value::Null,
    );
    prefab_ast.insert_component(
        root,
        "jackdaw::prefab::components::PrefabEntityId",
        serde_json::json!(0),
    );
    prefab_ast.insert_component(
        root,
        "bevy_transform::components::transform::Transform",
        serde_json::json!({
            "translation": [0.0, 0.0, 0.0],
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "scale": [1.0, 1.0, 1.0]
        }),
    );

    let mut cache = jackdaw::prefab::PrefabAstCache::default();
    cache.insert(std::path::PathBuf::from("p.jsn"), prefab_ast);

    let mut scene_ast = jackdaw_jsn::SceneJsnAst::default();
    let instance = scene_ast.add_root();
    scene_ast.insert_component(
        instance,
        "jackdaw::prefab::components::IsA",
        serde_json::json!({ "source": "p.jsn", "deleted": [] }),
    );
    scene_ast.insert_component(
        instance,
        "jackdaw::prefab::components::PrefabEntityId",
        serde_json::json!(0),
    );
    scene_ast.insert_component(
        instance,
        "bevy_transform::components::transform::Transform",
        serde_json::json!({
            "translation": [10.0, 0.0, 0.0],
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "scale": [1.0, 1.0, 1.0]
        }),
    );

    assert!(jackdaw::prefab::overrides::field_is_overridden(
        &scene_ast,
        &cache,
        instance,
        "bevy_transform::components::transform::Transform",
        None,
    ));
    assert!(jackdaw::prefab::overrides::field_is_overridden(
        &scene_ast,
        &cache,
        instance,
        "bevy_transform::components::transform::Transform",
        Some("translation"),
    ));
    assert!(!jackdaw::prefab::overrides::field_is_overridden(
        &scene_ast,
        &cache,
        instance,
        "bevy_transform::components::transform::Transform",
        Some("rotation"),
    ));
}

#[test]
fn field_is_overridden_returns_false_outside_isa_subtree() {
    let cache = jackdaw::prefab::PrefabAstCache::default();
    let mut scene_ast = jackdaw_jsn::SceneJsnAst::default();
    let entity = scene_ast.add_root();
    scene_ast.insert_component(
        entity,
        "bevy_transform::components::transform::Transform",
        serde_json::json!({ "translation": [3.0, 0.0, 0.0] }),
    );
    assert!(!jackdaw::prefab::overrides::field_is_overridden(
        &scene_ast,
        &cache,
        entity,
        "bevy_transform::components::transform::Transform",
        None,
    ));
}

#[test]
fn revert_field_snaps_value_back_to_prefab() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");
    std::fs::write(
        &prefab_path,
        r#"{
            "jsn": { "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" },
            "metadata": { "name": "p", "created": "", "modified": "" },
            "assets": {},
            "scene": [{
                "components": {
                    "jackdaw::prefab::components::Prefab": null,
                    "jackdaw::prefab::components::PrefabEntityId": 0,
                    "bevy_transform::components::transform::Transform": {
                        "translation": [0.0, 0.0, 0.0],
                        "rotation": [0.0, 0.0, 0.0, 1.0],
                        "scale": [1.0, 1.0, 1.0]
                    }
                }
            }]
        }"#,
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();

    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        bevy::math::Vec3::new(7.0, 0.0, 0.0),
    );

    let instance_key = {
        let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
        ast.entities_with_component("jackdaw::prefab::components::IsA")
            .next()
            .unwrap()
    };

    {
        let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
        let tx = ast
            .get_component_at(
                instance_key,
                "bevy_transform::components::transform::Transform",
            )
            .unwrap();
        assert_eq!(tx["translation"][0].as_f64(), Some(7.0));
    }

    jackdaw::prefab::operators::revert_field(
        app.world_mut(),
        instance_key,
        "bevy_transform::components::transform::Transform",
        "translation",
    );

    let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
    let tx = ast
        .get_component_at(
            instance_key,
            "bevy_transform::components::transform::Transform",
        )
        .unwrap();
    assert_eq!(tx["translation"][0].as_f64(), Some(0.0));
}

#[test]
fn revert_component_preserves_instance_only_addition() {
    // `revert_component` only reverts to a prefab-provided value. When
    // the prefab doesn't have the component (instance-only addition),
    // the operator must refuse to drop it; removing in that case
    // erases authored data with no recovery path. Re-enable the "drop
    // instance-only addition" behaviour later behind explicit gating
    // (an IsA-ancestor check).
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");
    std::fs::write(
        &prefab_path,
        r#"{
            "jsn": { "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" },
            "metadata": { "name": "p", "created": "", "modified": "" },
            "assets": {},
            "scene": [{
                "components": {
                    "jackdaw::prefab::components::Prefab": null,
                    "jackdaw::prefab::components::PrefabEntityId": 0
                }
            }]
        }"#,
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();

    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        bevy::math::Vec3::ZERO,
    );

    let instance_key = {
        let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
        ast.entities_with_component("jackdaw::prefab::components::IsA")
            .next()
            .unwrap()
    };

    {
        let mut ast = app.world_mut().resource_mut::<jackdaw_jsn::SceneJsnAst>();
        ast.insert_component(
            instance_key,
            "bevy_ecs::name::Name",
            serde_json::Value::String("instance_only".to_string()),
        );
    }

    jackdaw::prefab::operators::revert_component(
        app.world_mut(),
        instance_key,
        "bevy_ecs::name::Name",
    );

    let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
    assert_eq!(
        ast.get_component_at(instance_key, "bevy_ecs::name::Name"),
        Some(&serde_json::Value::String("instance_only".to_string())),
        "instance-only addition is preserved when prefab has no value to revert to"
    );
}

#[test]
fn save_as_variant_writes_prefab_with_isa_and_overrides() {
    let tmp = tempfile::tempdir().unwrap();
    let base_path = tmp.path().join("base.jsn");
    let variant_path = tmp.path().join("variant.jsn");

    std::fs::write(
        &base_path,
        r#"{
            "jsn": { "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" },
            "metadata": { "name": "base", "created": "", "modified": "" },
            "assets": {},
            "scene": [{
                "components": {
                    "jackdaw::prefab::components::Prefab": null,
                    "jackdaw::prefab::components::PrefabEntityId": 0,
                    "bevy_ecs::name::Name": "base"
                }
            }]
        }"#,
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();

    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &base_path,
        bevy::math::Vec3::new(5.0, 0.0, 0.0),
    );

    let instance_entity = app
        .world_mut()
        .query_filtered::<bevy::prelude::Entity, bevy::prelude::With<jackdaw::prefab::IsA>>()
        .iter(app.world())
        .next()
        .expect("instance entity present");

    jackdaw::prefab::operators::save_as_variant(app.world_mut(), instance_entity, &variant_path);

    assert!(variant_path.exists(), "variant file written");
    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&variant_path).unwrap()).unwrap();
    let root_components = &value["scene"][0]["components"];
    assert!(
        root_components
            .get("jackdaw::prefab::components::Prefab")
            .is_some(),
        "variant root has Prefab"
    );
    assert!(
        root_components
            .get("jackdaw::prefab::components::IsA")
            .is_some(),
        "variant root has IsA pointing at base"
    );

    // Source scene's instance now points at the variant.
    let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
    let instance_isa = ast
        .entities_with_component("jackdaw::prefab::components::IsA")
        .next()
        .and_then(|k| {
            ast.get_component_at(k, "jackdaw::prefab::components::IsA")
                .cloned()
        })
        .expect("instance still in scene AST");
    assert_eq!(
        instance_isa["source"].as_str(),
        Some(variant_path.to_string_lossy().as_ref()),
        "instance rewired to variant"
    );
}

#[test]
fn bulk_apply_in_scene_copies_delta_to_all_matching_instances() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");
    std::fs::write(
        &prefab_path,
        r#"{
            "jsn": { "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" },
            "metadata": { "name": "p", "created": "", "modified": "" },
            "assets": {},
            "scene": [{
                "components": {
                    "jackdaw::prefab::components::Prefab": null,
                    "jackdaw::prefab::components::PrefabEntityId": 0,
                    "bevy_ecs::name::Name": "p"
                }
            }]
        }"#,
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();

    // Spawn three instances of the same prefab.
    for x in [0.0, 2.0, 4.0] {
        jackdaw::prefab::operators::spawn_instance(
            app.world_mut(),
            &prefab_path,
            bevy::math::Vec3::new(x, 0.0, 0.0),
        );
    }

    jackdaw::prefab::operators::bulk_apply_in_scene(
        app.world_mut(),
        &prefab_path,
        "bevy_transform::components::transform::Transform",
        "rotation",
        serde_json::json!([
            0.0,
            std::f64::consts::FRAC_1_SQRT_2,
            0.0,
            std::f64::consts::FRAC_1_SQRT_2,
        ]),
    );

    let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
    let hits: usize = ast
        .entities_with_component("jackdaw::prefab::components::IsA")
        .filter(|k| {
            ast.get_component_at(*k, "bevy_transform::components::transform::Transform")
                .and_then(|v| v.get("rotation"))
                .is_some()
        })
        .count();
    assert_eq!(hits, 3, "all three instances got the rotation override");
}

#[test]
fn apply_to_prefab_source_writes_value_into_prefab_ast() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");
    std::fs::write(
        &prefab_path,
        r#"{
            "jsn": { "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" },
            "metadata": { "name": "p", "created": "", "modified": "" },
            "assets": {},
            "scene": [{
                "components": {
                    "jackdaw::prefab::components::Prefab": null,
                    "jackdaw::prefab::components::PrefabEntityId": 0,
                    "bevy_transform::components::transform::Transform": {
                        "translation": [0,0,0],
                        "rotation": [0,0,0,1],
                        "scale": [1,1,1]
                    }
                }
            }]
        }"#,
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        bevy::math::Vec3::ZERO,
    );

    let instance_key = {
        let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
        ast.entities_with_component("jackdaw::prefab::components::IsA")
            .next()
            .expect("instance")
    };

    jackdaw::prefab::operators::apply_to_prefab_source(
        app.world_mut(),
        instance_key,
        0,
        "bevy_transform::components::transform::Transform",
        "scale",
        serde_json::json!([2.0, 2.0, 2.0]),
    );

    let cache = app.world().resource::<jackdaw::prefab::PrefabAstCache>();
    let prefab_ast = cache.get(&prefab_path).expect("prefab cached");
    let root = prefab_ast
        .entities_with_component("jackdaw::prefab::components::Prefab")
        .next()
        .unwrap();
    let transform = prefab_ast
        .get_component_at(root, "bevy_transform::components::transform::Transform")
        .unwrap();
    assert_eq!(
        transform["scale"].as_array().unwrap()[0].as_f64(),
        Some(2.0),
        "cache reflects applied value"
    );
}

#[test]
fn apply_to_source_updates_cache_without_disk_write() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");
    std::fs::write(
        &prefab_path,
        r#"{
            "jsn": { "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" },
            "metadata": { "name": "p", "created": "", "modified": "" },
            "assets": {},
            "scene": [{
                "components": {
                    "jackdaw::prefab::components::Prefab": null,
                    "jackdaw::prefab::components::PrefabEntityId": 0,
                    "bevy_transform::components::transform::Transform": {
                        "translation": [0.0, 0.0, 0.0],
                        "rotation": [0.0, 0.0, 0.0, 1.0],
                        "scale": [1.0, 1.0, 1.0]
                    }
                }
            }]
        }"#,
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        bevy::math::Vec3::ZERO,
    );

    let mtime_before = std::fs::metadata(&prefab_path).unwrap().modified().unwrap();
    let cache_epoch_before = app
        .world()
        .resource::<jackdaw::prefab::PrefabAstCache>()
        .epoch();

    let instance_key = {
        let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
        ast.entities_with_component("jackdaw::prefab::components::IsA")
            .next()
            .unwrap()
    };
    jackdaw::prefab::operators::apply_to_prefab_source(
        app.world_mut(),
        instance_key,
        0,
        "bevy_transform::components::transform::Transform",
        "translation",
        serde_json::json!([5.0, 0.0, 0.0]),
    );

    let cache_epoch_after = app
        .world()
        .resource::<jackdaw::prefab::PrefabAstCache>()
        .epoch();
    assert!(cache_epoch_after > cache_epoch_before, "cache epoch bumped");
    let cache = app.world().resource::<jackdaw::prefab::PrefabAstCache>();
    let prefab_ast = cache.get(&prefab_path).expect("prefab in cache");
    let root = prefab_ast
        .entities_with_component("jackdaw::prefab::components::Prefab")
        .next()
        .unwrap();
    let tx = prefab_ast
        .get_component_at(root, "bevy_transform::components::transform::Transform")
        .unwrap();
    assert_eq!(
        tx["translation"].as_array().unwrap()[0].as_f64(),
        Some(5.0),
        "cache reflects applied value"
    );

    let mtime_after = std::fs::metadata(&prefab_path).unwrap().modified().unwrap();
    assert_eq!(
        mtime_before, mtime_after,
        "apply_to_source must not write disk; disk-write is deferred to an explicit save"
    );
}

#[test]
fn unpack_child_adds_to_deleted_and_creates_standalone_node() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");
    let scene_path = tmp.path().join("s.jsn");
    std::fs::write(
        &prefab_path,
        r#"{
            "jsn": { "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" },
            "metadata": { "name":"p","created":"","modified":"" },
            "assets": {},
            "scene": [
                { "components": { "jackdaw::prefab::components::Prefab": null, "jackdaw::prefab::components::PrefabEntityId": 0, "bevy_ecs::name::Name": "root" } },
                { "parent": 0, "components": { "jackdaw::prefab::components::PrefabEntityId": 7, "bevy_ecs::name::Name": "rock" } }
            ]
        }"#,
    )
    .unwrap();
    std::fs::write(
        &scene_path,
        format!(
            r#"{{
                "jsn": {{ "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" }},
                "metadata": {{ "name":"s","created":"","modified":"" }},
                "assets": {{}},
                "scene": [
                    {{
                        "components": {{
                            "jackdaw::prefab::components::IsA": {{ "source": "{}", "deleted": [] }},
                            "jackdaw::prefab::components::PrefabEntityId": 0
                        }}
                    }},
                    {{
                        "parent": 0,
                        "components": {{
                            "jackdaw::prefab::components::PrefabEntityId": 7,
                            "bevy_ecs::name::Name": "rock"
                        }}
                    }}
                ]
            }}"#,
            prefab_path.to_string_lossy()
        ),
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene_path);

    let (instance_root_key, child_key) = {
        let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
        let instance_root_key = ast
            .entities_with_component("jackdaw::prefab::components::IsA")
            .next()
            .unwrap();
        let child_key = ast
            .descendants_of(instance_root_key)
            .into_iter()
            .find(|k| {
                ast.get_component_at(*k, "jackdaw::prefab::components::PrefabEntityId")
                    .and_then(serde_json::Value::as_u64)
                    == Some(7)
            })
            .expect("inherited child resolved");
        (instance_root_key, child_key)
    };

    let scene_root = {
        let mut ast = app.world_mut().resource_mut::<jackdaw_jsn::SceneJsnAst>();
        ast.add_root()
    };

    jackdaw::prefab::operators::unpack_child(app.world_mut(), child_key, scene_root);

    let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
    let isa = ast
        .get_component_at(instance_root_key, "jackdaw::prefab::components::IsA")
        .unwrap();
    let deleted = isa["deleted"].as_array().unwrap();
    assert!(
        deleted.iter().any(|v| v.as_u64() == Some(7)),
        "instance's IsA.deleted contains the unpacked id, got {deleted:?}"
    );

    let unpacked_count = ast
        .descendants_of(scene_root)
        .into_iter()
        .filter(|k| {
            ast.get_component_at(*k, "bevy_ecs::name::Name")
                .and_then(|v| v.as_str())
                == Some("rock")
        })
        .count();
    assert_eq!(
        unpacked_count, 1,
        "the unpacked entity sits under the drop target"
    );
}

#[test]
fn save_as_prefab_from_selection_packages_siblings_under_synthetic_root() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("cluster.jsn");

    let mut app = make_app_for_prefab_tests();
    let a = app.world_mut().spawn(bevy::prelude::Name::new("a")).id();
    let b = app.world_mut().spawn(bevy::prelude::Name::new("b")).id();

    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &[a, b], &target);

    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
    let scene = value["scene"].as_array().unwrap();
    assert_eq!(scene.len(), 3, "synthetic root + 2 siblings");
    assert!(
        scene[0]["components"]
            .get("jackdaw::prefab::components::Prefab")
            .is_some(),
        "first entry is the synthetic prefab root"
    );
    // Both authored entities are children of index 0.
    assert_eq!(scene[1]["parent"].as_u64(), Some(0));
    assert_eq!(scene[2]["parent"].as_u64(), Some(0));
}

#[test]
fn save_as_prefab_from_selection_filters_descendants_of_selected_ancestors() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("nested.jsn");

    let mut app = make_app_for_prefab_tests();
    let parent = app
        .world_mut()
        .spawn(bevy::prelude::Name::new("parent"))
        .id();
    let child = app
        .world_mut()
        .spawn((
            bevy::prelude::Name::new("child"),
            bevy::ecs::hierarchy::ChildOf(parent),
        ))
        .id();

    // Select both - normalization should drop the child (its parent
    // already covers it), leaving a single top root.
    jackdaw::prefab::operators::save_as_prefab_from_selection(
        app.world_mut(),
        &[parent, child],
        &target,
    );

    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
    let scene = value["scene"].as_array().unwrap();
    // Always-wrap shape: synthetic PrefabRoot + parent + child.
    assert_eq!(
        scene.len(),
        3,
        "synthetic root + parent + child, no duplicate child"
    );
    assert!(
        scene[0]["components"]
            .get("jackdaw::prefab::components::Prefab")
            .is_some(),
        "synthetic root carries the Prefab marker"
    );
}

#[test]
fn save_as_prefab_from_selection_one_root_inserts_instance() {
    // Selection of size 1 still mutates the source AST to add a new
    // instance node carrying IsA + PrefabEntityId(0).
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("solo.jsn");

    let mut app = make_app_for_prefab_tests();
    let solo = app.world_mut().spawn(bevy::prelude::Name::new("solo")).id();

    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &[solo], &target);

    let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
    assert!(
        ast.entities_with_component("jackdaw::prefab::components::IsA")
            .next()
            .is_some(),
        "single-root flow inserts a new instance node carrying IsA"
    );
}

#[test]
fn save_round_trip_preserves_prefab_markers() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");
    std::fs::write(
        &prefab_path,
        r#"{
            "jsn": { "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" },
            "metadata": { "name": "p", "created": "", "modified": "" },
            "assets": {},
            "scene": [{
                "components": {
                    "jackdaw::prefab::components::Prefab": null,
                    "jackdaw::prefab::components::PrefabEntityId": 0,
                    "bevy_ecs::name::Name": "p"
                }
            }]
        }"#,
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();

    // Drop an instance into the scene.
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        bevy::math::Vec3::ZERO,
    );

    // Serialize the live world the same way save_scene does. This is
    // the boundary where `should_skip_component` runs, so it's the
    // exact path that previously dropped the prefab marker components.
    let jsn = jackdaw::scene_io::serialize_world_to_jsn_scene(app.world_mut());
    let value = serde_json::to_value(&jsn).expect("serializes to json");

    let has_isa = value["scene"].as_array().unwrap().iter().any(|e| {
        e["components"]
            .get("jackdaw::prefab::components::IsA")
            .is_some()
    });
    assert!(
        has_isa,
        "saved scene must preserve IsA on the instance; got {value:#}"
    );
}

#[test]
fn cache_canonicalizes_path_inputs() {
    let tmp = tempfile::tempdir().unwrap();
    let abs = tmp.path().join("p.jsn");
    std::fs::write(&abs, "{}").unwrap();

    let mut cache = jackdaw::prefab::PrefabAstCache::default();
    cache.insert(&abs, jackdaw_jsn::SceneJsnAst::default());

    let weird = abs.parent().unwrap().join(".").join("p.jsn");
    assert!(
        cache.get(&weird).is_some(),
        "lookup tolerates non-canonical inputs"
    );
}

#[test]
fn cache_bumps_epoch_on_every_mutation() {
    let mut cache = jackdaw::prefab::PrefabAstCache::default();
    let start = cache.epoch();
    cache.insert(
        std::path::PathBuf::from("/tmp/jackdaw_cache_test_a.jsn"),
        jackdaw_jsn::SceneJsnAst::default(),
    );
    let after_insert = cache.epoch();
    assert!(after_insert > start, "insert bumps epoch");
    cache.invalidate(&std::path::PathBuf::from("/tmp/jackdaw_cache_test_a.jsn"));
    assert!(cache.epoch() > after_insert, "invalidate bumps epoch");
}

#[test]
fn editor_save_records_fingerprint() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");
    std::fs::write(
        &prefab_path,
        r#"{
            "jsn": { "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" },
            "metadata": { "name": "p", "created": "", "modified": "" },
            "assets": {},
            "scene": [{
                "components": {
                    "jackdaw::prefab::components::Prefab": null,
                    "jackdaw::prefab::components::PrefabEntityId": 0,
                    "bevy_ecs::name::Name": "p"
                }
            }]
        }"#,
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        bevy::math::Vec3::ZERO,
    );
    jackdaw::prefab::operators::save_prefab_to_disk(app.world_mut(), &prefab_path)
        .expect("save ok");
    let cache = app.world().resource::<jackdaw::prefab::PrefabAstCache>();
    let recorded = cache.last_saved_fingerprint(&prefab_path).cloned();
    assert!(recorded.is_some(), "fingerprint recorded after save");
    let on_disk = jackdaw::prefab::cache::compute_file_fingerprint(&prefab_path).unwrap();
    assert_eq!(
        recorded.unwrap(),
        on_disk,
        "recorded fingerprint matches disk"
    );
}

#[test]
fn external_edit_changes_fingerprint() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");
    std::fs::write(&prefab_path, "{}").unwrap();
    let fp_a = jackdaw::prefab::cache::compute_file_fingerprint(&prefab_path).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(&prefab_path, r#"{"changed":true}"#).unwrap();
    let fp_b = jackdaw::prefab::cache::compute_file_fingerprint(&prefab_path).unwrap();
    assert_ne!(fp_a, fp_b, "fingerprint changes when content changes");
}

/// Smoke test: dispatch `prefab.revert_component` through the operator
/// framework end-to-end. Verifies the wrapper decodes parameters and
/// calls the underlying `revert_component` helper.
#[test]
fn revert_component_operator_runs_through_dispatch() {
    use jackdaw_api::prelude::*;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");
    std::fs::write(
        &prefab_path,
        r#"{
            "jsn": { "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" },
            "metadata": { "name": "p", "created": "", "modified": "" },
            "assets": {},
            "scene": [{
                "components": {
                    "jackdaw::prefab::components::Prefab": null,
                    "jackdaw::prefab::components::PrefabEntityId": 0,
                    "bevy_ecs::name::Name": "prefab_root"
                }
            }]
        }"#,
    )
    .unwrap();

    let mut app = util::editor_test_app();

    // Spawn a prefab instance and add a Name override to it.
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        bevy::math::Vec3::ZERO,
    );
    let instance_key = {
        let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
        ast.entities_with_component("jackdaw::prefab::components::IsA")
            .next()
            .expect("instance key")
    };
    {
        let mut ast = app.world_mut().resource_mut::<jackdaw_jsn::SceneJsnAst>();
        ast.insert_component(
            instance_key,
            "bevy_ecs::name::Name",
            serde_json::Value::String("override".to_string()),
        );
    }

    // Dispatch through the operator framework.
    let _ = app
        .world_mut()
        .operator("prefab.revert_component")
        .param("entity_key", instance_key as i64)
        .param("type_path", "bevy_ecs::name::Name".to_string())
        .call()
        .expect("operator dispatch resolves");
    // The dispatcher queues commands through the world; flush them.
    app.update();

    let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
    let name = ast.get_component_at(instance_key, "bevy_ecs::name::Name");
    assert_eq!(
        name,
        Some(&serde_json::Value::String("prefab_root".to_string())),
        "operator-driven revert restored the inherited prefab value",
    );
}

#[test]
fn save_as_prefab_strips_inherited_prefab_markers() {
    // An entity whose AST node already carries an `IsA` (because the
    // user previously converted it to an instance) must not bake that
    // marker into the freshly-authored prefab file. After saving,
    // neither the synthetic root nor any packaged child carries the
    // inherited IsA.
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("fresh.jsn");

    let mut app = make_app_for_prefab_tests();
    let entity = app
        .world_mut()
        .spawn(bevy::prelude::Name::new("source"))
        .id();
    {
        let mut ast = app.world_mut().resource_mut::<jackdaw_jsn::SceneJsnAst>();
        let key = ast.create_node(entity, None);
        ast.insert_component(
            key,
            "jackdaw::prefab::components::IsA",
            serde_json::json!({ "source": "/tmp/some_other_prefab.jsn", "deleted": [] }),
        );
    }

    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &[entity], &target);

    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
    let scene = value["scene"].as_array().unwrap();
    assert!(
        scene[0]["components"]
            .get("jackdaw::prefab::components::Prefab")
            .is_some(),
        "synthetic root has fresh Prefab marker"
    );
    for entry in scene {
        assert!(
            entry["components"]
                .get("jackdaw::prefab::components::IsA")
                .is_none(),
            "no packaged entity may carry inherited IsA: entry={entry:?}"
        );
    }
}

#[test]
fn save_as_prefab_does_not_bake_self_isa_into_file() {
    // The source entity already has an `IsA` pointing at `target`.
    // The always-wrap save path writes a synthetic PrefabRoot wrapping
    // the source entity; the source's pre-existing IsA must be stripped
    // from the written file so the prefab does not reference itself.
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("box.jsn");

    let mut app = make_app_for_prefab_tests();
    let entity = app
        .world_mut()
        .spawn(bevy::prelude::Name::new("source"))
        .id();
    {
        let mut ast = app.world_mut().resource_mut::<jackdaw_jsn::SceneJsnAst>();
        let key = ast.create_node(entity, None);
        ast.insert_component(
            key,
            "jackdaw::prefab::components::IsA",
            serde_json::json!({ "source": target.to_string_lossy(), "deleted": [] }),
        );
    }

    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &[entity], &target);

    assert!(target.exists(), "always-wrap path writes the file");
    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
    for entry in value["scene"].as_array().unwrap() {
        assert!(
            entry["components"]
                .get("jackdaw::prefab::components::IsA")
                .is_none(),
            "no entry in the written prefab carries a self-IsA: entry={entry:?}"
        );
    }
}

#[test]
fn repair_self_cycles_strips_self_isa_from_cached_prefab() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("poisoned.jsn");
    let poisoned = format!(
        r#"{{
            "jsn": {{ "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" }},
            "metadata": {{ "name": "poisoned", "created": "", "modified": "" }},
            "assets": {{}},
            "scene": [{{
                "components": {{
                    "jackdaw::prefab::components::Prefab": null,
                    "jackdaw::prefab::components::PrefabEntityId": 0,
                    "jackdaw::prefab::components::IsA": {{ "source": "{}", "deleted": [] }},
                    "bevy_ecs::name::Name": "poisoned"
                }}
            }}]
        }}"#,
        path.to_string_lossy()
    );
    std::fs::write(&path, poisoned).unwrap();

    let mut app = make_app_for_prefab_tests();
    let scene: jackdaw_jsn::format::JsnScene =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let ast = jackdaw_jsn::SceneJsnAst::from_jsn_scene(&scene, &[]);
    app.world_mut()
        .resource_mut::<jackdaw::prefab::PrefabAstCache>()
        .insert(&path, ast);

    jackdaw::prefab::operators::repair_self_cycles_system(app.world_mut());

    let cache = app.world().resource::<jackdaw::prefab::PrefabAstCache>();
    let repaired = cache.get(&path).expect("still cached");
    let root = &repaired.nodes[0];
    assert!(
        !root
            .components
            .contains_key("jackdaw::prefab::components::IsA"),
        "self-IsA was stripped"
    );
    assert!(
        root.components
            .contains_key("jackdaw::prefab::components::Prefab"),
        "Prefab marker preserved"
    );

    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(
        written["scene"][0]["components"]
            .get("jackdaw::prefab::components::IsA")
            .is_none(),
        "disk file also has IsA stripped"
    );
}

#[test]
fn prefab_edit_propagates_to_instance_in_other_tab_on_swap() {
    // Simulates: user has tab A with an instance of box.jsn + tab B
    // editing box.jsn directly. Edit the prefab via the cache (as
    // `scene.save`'s prefab branch does), then swap back to tab A and
    // assert the instance's spawned entity reflects the updated prefab.
    use bevy::prelude::*;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("box.jsn");

    let mut app = make_app_for_prefab_tests();
    app.init_resource::<jackdaw::scenes::Scenes>();

    // 1. Seed a prefab with a Name component and a Transform.
    {
        let mut prefab_ast = jackdaw_jsn::SceneJsnAst::default();
        let root = prefab_ast.add_root();
        prefab_ast.insert_component(
            root,
            "jackdaw::prefab::components::Prefab",
            serde_json::Value::Null,
        );
        prefab_ast.insert_component(
            root,
            "jackdaw::prefab::components::PrefabEntityId",
            serde_json::json!(0),
        );
        prefab_ast.insert_component(
            root,
            "bevy_ecs::name::Name",
            serde_json::Value::String("initial_name".to_string()),
        );
        prefab_ast.insert_component(
            root,
            "bevy_transform::components::transform::Transform",
            serde_json::json!({
                "translation": [0.0, 0.0, 0.0],
                "rotation": [0.0, 0.0, 0.0, 1.0],
                "scale": [1.0, 1.0, 1.0]
            }),
        );
        app.world_mut()
            .resource_mut::<jackdaw::prefab::PrefabAstCache>()
            .insert(&prefab_path, prefab_ast);
    }

    // 2. Build tab A: a scene with one instance of the prefab. Push a
    //    second tab (tab B) pointing at the prefab via its cache entry.
    {
        let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
        let mut tab_a = jackdaw::scenes::SceneTab::new_untitled(1);
        let mut scene_ast = jackdaw_jsn::SceneJsnAst::default();
        let instance = scene_ast.add_root();
        scene_ast.insert_component(
            instance,
            "jackdaw::prefab::components::IsA",
            serde_json::json!({ "source": prefab_path.to_string_lossy(), "deleted": [] }),
        );
        scene_ast.insert_component(
            instance,
            "jackdaw::prefab::components::PrefabEntityId",
            serde_json::json!(0),
        );
        tab_a.content = jackdaw::scenes::TabContent::Scene(Some(scene_ast));
        scenes.tabs.push(tab_a);
        scenes.active = 0;

        let canonical = jackdaw::prefab::canonical_prefab_path(&prefab_path);
        let mut tab_b = jackdaw::scenes::SceneTab::new_untitled(2);
        tab_b.path = Some(prefab_path.clone());
        tab_b.kind = jackdaw::scenes::TabKind::Prefab;
        tab_b.content = jackdaw::scenes::TabContent::Prefab(canonical);
        scenes.tabs.push(tab_b);
    }

    // 3. Activate tab A: resolver should spawn the instance with the
    //    initial name + transform inherited from the prefab.
    jackdaw::scenes::swap::activate_tab(app.world_mut(), 0);

    let initial_names: Vec<String> = {
        let world = app.world_mut();
        let mut q = world.query::<&bevy::prelude::Name>();
        q.iter(world).map(|n| n.as_str().to_string()).collect()
    };
    assert!(
        initial_names.iter().any(|n| n == "initial_name"),
        "instance should spawn with the inherited prefab Name; got {initial_names:?}"
    );

    // 4. Swap to tab B (the prefab tab). This is what happens when the
    //    user clicks the prefab tab in the strip. capture_active_tab
    //    flushes tab A's instance AST into tab.content; activate_tab
    //    reads the cache, resolves, and spawns the prefab into the
    //    live world. Now the live SceneJsnAst is the prefab AST.
    jackdaw::scenes::swap::swap_active_tab(app.world_mut(), 1);

    // 5. Mutate the cache entry: rename the prefab. This is what
    //    `scene.save`'s prefab branch does after the user hits Ctrl+S
    //    on a prefab tab: clone the live AST and insert it into the
    //    cache under the prefab path.
    {
        let mut cache = app
            .world_mut()
            .resource_mut::<jackdaw::prefab::PrefabAstCache>();
        cache.mutate(&prefab_path, |ast| {
            let root_key = ast
                .entities_with_component("jackdaw::prefab::components::Prefab")
                .next()
                .expect("prefab root exists");
            ast.replace_component(
                root_key,
                "bevy_ecs::name::Name",
                serde_json::Value::String("renamed_in_prefab".to_string()),
            );
        });
        // Also update the live AST so the upcoming capture-on-swap
        // doesn't clobber our mutation. In the real editor, scene.save
        // mutates the cache from the live AST, so they stay in sync.
        let mut live = app.world_mut().resource_mut::<jackdaw_jsn::SceneJsnAst>();
        let root_key = live
            .entities_with_component("jackdaw::prefab::components::Prefab")
            .next();
        if let Some(root_key) = root_key {
            live.replace_component(
                root_key,
                "bevy_ecs::name::Name",
                serde_json::Value::String("renamed_in_prefab".to_string()),
            );
        }
    }

    // 6. Swap back to tab A. The resolver should re-read the cache and
    //    respawn the instance with the new Name.
    jackdaw::scenes::swap::swap_active_tab(app.world_mut(), 0);

    let final_names: Vec<String> = {
        let world = app.world_mut();
        let mut q = world.query::<&bevy::prelude::Name>();
        q.iter(world).map(|n| n.as_str().to_string()).collect()
    };
    assert!(
        final_names.iter().any(|n| n == "renamed_in_prefab"),
        "after the swap-back, the instance should reflect the renamed prefab; got {final_names:?}"
    );
    assert!(
        !final_names.iter().any(|n| n == "initial_name"),
        "the stale initial_name must NOT still be present; got {final_names:?}"
    );
}

#[test]
fn scene_save_on_prefab_tab_clears_dirty_state() {
    // After a Ctrl+S on a prefab tab, neither the per-tab dirty flag
    // nor the global `SceneDirtyState` should report unsaved work.
    use bevy::prelude::*;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");

    let mut app = make_app_for_prefab_tests();
    app.init_resource::<jackdaw::scenes::Scenes>();

    // Seed the cache with a prefab.
    {
        let mut prefab_ast = jackdaw_jsn::SceneJsnAst::default();
        let root = prefab_ast.add_root();
        prefab_ast.insert_component(
            root,
            "jackdaw::prefab::components::Prefab",
            serde_json::Value::Null,
        );
        prefab_ast.insert_component(
            root,
            "jackdaw::prefab::components::PrefabEntityId",
            serde_json::json!(0),
        );
        prefab_ast.insert_component(
            root,
            "bevy_ecs::name::Name",
            serde_json::Value::String("p".to_string()),
        );
        app.world_mut()
            .resource_mut::<jackdaw::prefab::PrefabAstCache>()
            .insert(&prefab_path, prefab_ast);
    }

    // Set up one prefab tab as the active tab.
    {
        let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
        let canonical = jackdaw::prefab::canonical_prefab_path(&prefab_path);
        let mut tab = jackdaw::scenes::SceneTab::new_untitled(1);
        tab.path = Some(prefab_path.clone());
        tab.kind = jackdaw::scenes::TabKind::Prefab;
        tab.content = jackdaw::scenes::TabContent::Prefab(canonical);
        scenes.tabs.push(tab);
        scenes.active = 0;
    }
    // Sync the global file path so save_scene routes to save_scene_inner.
    {
        let mut sp = app
            .world_mut()
            .resource_mut::<jackdaw::scene_io::SceneFilePath>();
        sp.path = Some(prefab_path.to_string_lossy().into_owned());
    }
    jackdaw::scenes::swap::activate_tab(app.world_mut(), 0);

    // Simulate a user edit: push something onto the command history and
    // flip the tab dirty flag. Also drift `undo_len_at_save` so the
    // global status bar would otherwise still show `*Unsaved`.
    struct NoOpCommand;
    impl jackdaw_commands::EditorCommand for NoOpCommand {
        fn execute(&mut self, _world: &mut bevy::prelude::World) {}
        fn undo(&mut self, _world: &mut bevy::prelude::World) {}
        fn description(&self) -> &str {
            "noop"
        }
    }
    app.world_mut()
        .resource_mut::<jackdaw_commands::CommandHistory>()
        .push_executed(Box::new(NoOpCommand));
    {
        let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
        scenes.tabs[0].dirty = true;
        scenes.tabs[0].history_depth_at_last_check = 1;
    }
    assert!(jackdaw::scene_io::is_scene_dirty(app.world()));

    // Save: this routes through the prefab branch in save_scene_inner
    // because the active tab is a prefab.
    jackdaw::scene_io::save_scene(app.world_mut());

    assert!(
        !app.world().resource::<jackdaw::scenes::Scenes>().tabs[0].dirty,
        "prefab tab.dirty must be cleared after save"
    );
    assert!(
        !jackdaw::scene_io::is_scene_dirty(app.world()),
        "global SceneDirtyState must report clean after save"
    );

    // Pump the dirty-tracker system once; it must not flip dirty back on.
    let _ = app
        .world_mut()
        .run_system_cached(jackdaw::scenes::mark_active_dirty_on_history_growth);
    assert!(
        !app.world().resource::<jackdaw::scenes::Scenes>().tabs[0].dirty,
        "mark_active_dirty_on_history_growth must not re-dirty the tab post-save"
    );

    // Pump the cache-epoch-change driver: this is what fires on the next
    // frame after the save inserts into the cache. It calls
    // `reload_all_instances`, which calls `clear_scene_entities`, which
    // *clears the command history*. If `SceneDirtyState.undo_len_at_save`
    // is not also reset to 0, the status bar will keep showing `*Unsaved`
    // because `undo_stack.len() (0) != undo_len_at_save (>0)`.
    jackdaw::prefab::sync::drive_respawn_on_prefab_cache_change(app.world_mut());
    assert!(
        !jackdaw::scene_io::is_scene_dirty(app.world()),
        "after the respawn-on-cache-change driver fires, the scene must still report clean; \
         got undo_stack.len()={} undo_len_at_save={}",
        app.world()
            .resource::<jackdaw_commands::CommandHistory>()
            .undo_stack
            .len(),
        app.world()
            .resource::<jackdaw::scene_io::SceneDirtyState>()
            .undo_len_at_save,
    );
    assert!(
        !app.world().resource::<jackdaw::scenes::Scenes>().tabs[0].dirty,
        "after the respawn-on-cache-change driver fires, tab.dirty must still be false"
    );
}

#[test]
fn save_scene_as_prefab_converts_tab_to_prefab() {
    use bevy::prelude::*;

    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("box.jsn");

    let mut app = make_app_for_prefab_tests();
    app.init_resource::<jackdaw::scenes::Scenes>();
    app.init_resource::<jackdaw::commands::CommandHistory>();
    app.init_resource::<jackdaw::scene_io::SceneFilePath>();
    app.init_resource::<jackdaw::scene_io::SceneDirtyState>();
    app.init_resource::<jackdaw::selection::Selection>();

    {
        let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
        scenes.tabs.push(jackdaw::scenes::SceneTab::new_untitled(1));
        scenes.active = 0;
    }
    {
        let entity = app.world_mut().spawn(Name::new("source")).id();
        let mut ast = app.world_mut().resource_mut::<jackdaw_jsn::SceneJsnAst>();
        let key = ast.create_node(entity, None);
        ast.insert_component(
            key,
            "bevy_ecs::name::Name",
            serde_json::Value::String("source".to_string()),
        );
    }

    jackdaw::prefab::operators::save_scene_as_prefab(app.world_mut(), &target);

    let scenes = app.world().resource::<jackdaw::scenes::Scenes>();
    let tab = &scenes.tabs[0];
    assert!(
        matches!(tab.kind, jackdaw::scenes::TabKind::Prefab),
        "tab kind transitioned to Prefab"
    );
    assert!(
        matches!(&tab.content, jackdaw::scenes::TabContent::Prefab(_)),
        "tab content references the prefab cache, not a Scene AST"
    );
    assert_eq!(tab.path.as_deref(), Some(target.as_path()));
    assert!(!tab.dirty, "tab cleared dirty flag after save");
    assert_eq!(tab.display_name, "box");

    assert!(target.exists(), "prefab file written");
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
    let components = &written["scene"][0]["components"];
    assert!(
        components
            .get("jackdaw::prefab::components::Prefab")
            .is_some(),
        "root has Prefab marker"
    );
    assert!(
        components
            .get("jackdaw::prefab::components::PrefabEntityId")
            .is_some(),
        "root has PrefabEntityId(0)"
    );
    assert!(
        components.get("jackdaw::prefab::components::IsA").is_none(),
        "root has NO IsA (this is the prefab definition, not an instance)"
    );

    let cache = app.world().resource::<jackdaw::prefab::PrefabAstCache>();
    assert!(cache.get(&target).is_some(), "new prefab cached");
}

#[test]
fn save_scene_as_prefab_with_multiple_roots_uses_synthetic_root() {
    use bevy::prelude::*;

    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("multi.jsn");

    let mut app = make_app_for_prefab_tests();
    app.init_resource::<jackdaw::scenes::Scenes>();
    app.init_resource::<jackdaw::commands::CommandHistory>();
    app.init_resource::<jackdaw::scene_io::SceneFilePath>();
    app.init_resource::<jackdaw::scene_io::SceneDirtyState>();
    app.init_resource::<jackdaw::selection::Selection>();

    {
        let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
        scenes.tabs.push(jackdaw::scenes::SceneTab::new_untitled(1));
        scenes.active = 0;
    }
    {
        let a = app.world_mut().spawn(Name::new("a")).id();
        let b = app.world_mut().spawn(Name::new("b")).id();
        let mut ast = app.world_mut().resource_mut::<jackdaw_jsn::SceneJsnAst>();
        let ka = ast.create_node(a, None);
        let kb = ast.create_node(b, None);
        ast.insert_component(
            ka,
            "bevy_ecs::name::Name",
            serde_json::Value::String("a".into()),
        );
        ast.insert_component(
            kb,
            "bevy_ecs::name::Name",
            serde_json::Value::String("b".into()),
        );
    }

    jackdaw::prefab::operators::save_scene_as_prefab(app.world_mut(), &target);

    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
    let scene = written["scene"].as_array().unwrap();
    assert_eq!(scene.len(), 3, "synthetic root + 2 children = 3 entries");
    assert!(
        scene[0]["components"]
            .get("jackdaw::prefab::components::Prefab")
            .is_some(),
        "first entry is the synthetic Prefab root"
    );
}

#[test]
fn save_as_prefab_from_selection_always_wraps_in_prefab_root() {
    // Single-entity selection produces the same shape as multi-entity:
    // synthetic PrefabRoot + child(ren) with PrefabEntityId(1..).
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("solo.jsn");

    let mut app = make_app_for_prefab_tests();
    let solo = app.world_mut().spawn(bevy::prelude::Name::new("solo")).id();

    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &[solo], &target);

    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
    let scene = value["scene"].as_array().unwrap();
    assert_eq!(scene.len(), 2, "synthetic root + 1 child");
    let root_components = &scene[0]["components"];
    assert!(
        root_components
            .get("jackdaw::prefab::components::Prefab")
            .is_some()
    );
    assert_eq!(
        root_components
            .get("bevy_ecs::name::Name")
            .and_then(|v| v.as_str()),
        Some("solo"),
        "synthetic root is named after the target file stem (solo.jsn -> 'solo')"
    );
    assert_eq!(
        scene[1]["parent"].as_u64(),
        Some(0),
        "child parented under synthetic root"
    );
    assert_eq!(
        scene[1]["components"]["jackdaw::prefab::components::PrefabEntityId"].as_u64(),
        Some(1)
    );
}

#[test]
fn save_as_prefab_from_selection_restructures_source_scene() {
    // After save, the source scene's AST has a new instance node with
    // IsA + PrefabEntityId(0), and the selected entity is reparented
    // under it carrying PrefabEntityId(1).
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("box.jsn");

    let mut app = make_app_for_prefab_tests();
    let source = app
        .world_mut()
        .spawn(bevy::prelude::Name::new("source"))
        .id();
    {
        let mut ast = app.world_mut().resource_mut::<jackdaw_jsn::SceneJsnAst>();
        let key = ast.create_node(source, None);
        ast.insert_component(
            key,
            "bevy_ecs::name::Name",
            serde_json::Value::String("source".to_string()),
        );
    }

    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &[source], &target);

    let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
    let instance_key = ast
        .entities_with_component("jackdaw::prefab::components::IsA")
        .next()
        .expect("instance node added to source scene");
    assert!(
        ast.nodes[instance_key].parent.is_none(),
        "instance is a top-level node"
    );
    let source_key = ast.key_for_entity(source).expect("source still in AST");
    assert_eq!(
        ast.nodes[source_key].parent,
        Some(instance_key),
        "source entity reparented under the new instance"
    );
    assert_eq!(
        ast.get_component_at(source_key, "jackdaw::prefab::components::PrefabEntityId")
            .and_then(serde_json::Value::as_u64),
        Some(1),
        "source entity tagged with PrefabEntityId(1)"
    );
}

#[test]
fn unbundle_instance_promotes_children_and_strips_markers() {
    // Bundle a single entity, then unbundle. After unbundle, the
    // promoted child node should be a top-level node with no
    // PrefabEntityId, and the former instance node should be inert.
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("u.jsn");

    let mut app = make_app_for_prefab_tests();
    let source = app
        .world_mut()
        .spawn(bevy::prelude::Name::new("source"))
        .id();
    {
        let mut ast = app.world_mut().resource_mut::<jackdaw_jsn::SceneJsnAst>();
        let key = ast.create_node(source, None);
        ast.insert_component(
            key,
            "bevy_ecs::name::Name",
            serde_json::Value::String("source".to_string()),
        );
    }
    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &[source], &target);

    // Capture the source AST key BEFORE unbundle, because the reload
    // pass inside `unbundle_instance` despawns and respawns ECS entities
    // (so `key_for_entity(source)` no longer resolves afterwards).
    let (instance_key, source_key) = {
        let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
        let instance_key = ast
            .entities_with_component("jackdaw::prefab::components::IsA")
            .next()
            .unwrap();
        let source_key = ast
            .nodes
            .iter()
            .enumerate()
            .find_map(|(i, n)| {
                let same_name = n
                    .components
                    .get("bevy_ecs::name::Name")
                    .and_then(|v| v.as_str())
                    == Some("source");
                if same_name { Some(i) } else { None }
            })
            .expect("source node exists in AST");
        (instance_key, source_key)
    };

    jackdaw::prefab::operators::unbundle_instance(app.world_mut(), instance_key);

    let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
    assert!(
        ast.get_component_at(instance_key, "jackdaw::prefab::components::IsA")
            .is_none(),
        "IsA stripped from former instance node"
    );
    assert!(
        ast.nodes[source_key].parent.is_none(),
        "source promoted to top-level"
    );
    assert!(
        ast.get_component_at(source_key, "jackdaw::prefab::components::PrefabEntityId")
            .is_none(),
        "PrefabEntityId stripped from promoted child"
    );
}

#[test]
fn save_as_prefab_preserves_world_positions_of_selection() {
    // After Save Selection as Prefab, the visual positions of the
    // selected entities in the source scene must NOT change. The
    // instance entity sits at the selection centroid and each child's
    // local Transform is shifted by `-centroid` to compensate.
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("p.jsn");

    let mut app = make_app_for_prefab_tests();
    let e1 = app
        .world_mut()
        .spawn((
            bevy::prelude::Name::new("a"),
            bevy::prelude::Transform::from_xyz(2.0, 0.0, 0.0),
        ))
        .id();
    let e2 = app
        .world_mut()
        .spawn((
            bevy::prelude::Name::new("b"),
            bevy::prelude::Transform::from_xyz(4.0, 0.0, 0.0),
        ))
        .id();

    // Force GlobalTransform population so the centroid read uses the
    // production GlobalTransform path. The Transform fallback would
    // give the same answer for these top-level entities, but exercising
    // the GlobalTransform branch is the goal here.
    app.update();

    {
        let mut ast = app.world_mut().resource_mut::<jackdaw_jsn::SceneJsnAst>();
        let k1 = ast.create_node(e1, None);
        let k2 = ast.create_node(e2, None);
        ast.insert_component(
            k1,
            "bevy_transform::components::transform::Transform",
            serde_json::json!({
                "translation": [2.0, 0.0, 0.0],
                "rotation": [0.0, 0.0, 0.0, 1.0],
                "scale": [1.0, 1.0, 1.0],
            }),
        );
        ast.insert_component(
            k2,
            "bevy_transform::components::transform::Transform",
            serde_json::json!({
                "translation": [4.0, 0.0, 0.0],
                "rotation": [0.0, 0.0, 0.0, 1.0],
                "scale": [1.0, 1.0, 1.0],
            }),
        );
    }

    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &[e1, e2], &target);

    // Centroid is (3, 0, 0). Instance Transform.x should equal the
    // centroid; each child's local Transform.x should be its original
    // world X minus 3.0 so the world position survives reparenting.
    let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
    let instance_key = ast
        .entities_with_component("jackdaw::prefab::components::IsA")
        .next()
        .unwrap();
    let instance_tx = ast
        .get_component_at(
            instance_key,
            "bevy_transform::components::transform::Transform",
        )
        .unwrap();
    let instance_translation = instance_tx["translation"].as_array().unwrap();
    assert!(
        (instance_translation[0].as_f64().unwrap() - 3.0).abs() < 1e-4,
        "instance Transform.x is the centroid (3.0); got {instance_translation:?}"
    );

    let e1_key = ast.key_for_entity(e1).unwrap();
    let e1_tx = ast
        .get_component_at(e1_key, "bevy_transform::components::transform::Transform")
        .unwrap();
    let e1_translation = e1_tx["translation"].as_array().unwrap();
    assert!(
        (e1_translation[0].as_f64().unwrap() - (-1.0)).abs() < 1e-4,
        "e1 local Transform.x is centroid-relative (-1.0); got {e1_translation:?}"
    );

    let e2_key = ast.key_for_entity(e2).unwrap();
    let e2_tx = ast
        .get_component_at(e2_key, "bevy_transform::components::transform::Transform")
        .unwrap();
    let e2_translation = e2_tx["translation"].as_array().unwrap();
    assert!(
        (e2_translation[0].as_f64().unwrap() - 1.0).abs() < 1e-4,
        "e2 local Transform.x is centroid-relative (1.0); got {e2_translation:?}"
    );
}

#[test]
fn save_as_prefab_synthetic_root_has_visibility() {
    // Bevy's hierarchy propagation requires Visibility on every entity
    // in a render parent chain. Without it, children log B0004 warnings
    // and render at the wrong world position because the parent's
    // GlobalTransform stays at identity.
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("v.jsn");

    let mut app = make_app_for_prefab_tests();
    let e = app.world_mut().spawn(bevy::prelude::Name::new("a")).id();

    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &[e], &target);

    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
    let root_components = &value["scene"][0]["components"];
    assert!(
        root_components
            .get("bevy_camera::visibility::Visibility")
            .is_some(),
        "synthetic PrefabRoot carries Visibility for hierarchy propagation; got {root_components:?}"
    );

    // The in-place instance entity in the source AST also needs
    // Visibility for the same reason.
    let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
    let instance_key = ast
        .entities_with_component("jackdaw::prefab::components::IsA")
        .next()
        .unwrap();
    assert!(
        ast.get_component_at(instance_key, "bevy_camera::visibility::Visibility")
            .is_some(),
        "in-place instance entity carries Visibility for hierarchy propagation"
    );
}
