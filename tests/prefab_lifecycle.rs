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

    // Register both in the AST so they survive the snapshot's
    // "AST-tracked only" filter. In the real editor every user-drawn
    // entity is registered as part of scene_io's spawn path; ECS-only
    // children of brushes (face overlays etc.) are deliberately not
    // registered and therefore get filtered out.
    {
        let mut ast = app.world_mut().resource_mut::<jackdaw_jsn::SceneJsnAst>();
        ast.create_node(parent, None);
        ast.create_node(child, Some(parent));
    }

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
    let instance_entity = {
        let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
        ast.nodes[instance_key]
            .ecs_entity
            .expect("instance ECS entity")
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
        .param("entity", instance_entity)
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
fn save_as_prefab_from_selection_replaces_source_with_instance() {
    // After save, the source scene's AST has exactly one new authored
    // node: the instance. The originally-selected entity is removed
    // from the AST; the resolver materialises it back as an inherited
    // descendant when the next respawn fires.
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
    let isa_keys: Vec<usize> = ast
        .entities_with_component("jackdaw::prefab::components::IsA")
        .collect();
    assert_eq!(isa_keys.len(), 1, "exactly one instance node was added");
    let instance_key = isa_keys[0];
    assert!(
        ast.nodes[instance_key].parent.is_none(),
        "instance is a top-level node"
    );
    assert_eq!(
        ast.children_of(instance_key).count(),
        0,
        "instance has no authored children in the source AST; descendants come from the prefab via the resolver"
    );
}

#[test]
fn unbundle_instance_promotes_inherited_children_to_authored() {
    // Bundle a single entity, then unbundle. The new model puts only
    // the instance node in the source AST after bundling; the inherited
    // child is materialised by the resolver. Unbundle promotes the
    // inherited child to an authored AST node and strips PrefabEntityId.
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

    let instance_key = {
        let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
        ast.entities_with_component("jackdaw::prefab::components::IsA")
            .next()
            .expect("instance exists after bundle")
    };

    jackdaw::prefab::operators::unbundle_instance(app.world_mut(), instance_key);

    let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
    assert!(
        ast.get_component_at(instance_key, "jackdaw::prefab::components::IsA")
            .is_none(),
        "IsA stripped from former instance node"
    );

    // One authored node remains: the promoted child. It has no IsA,
    // no PrefabEntityId, and is a top-level node.
    let promoted: Vec<usize> = ast
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| {
            let has_name = n.components.contains_key("bevy_ecs::name::Name");
            let no_isa = !n
                .components
                .contains_key("jackdaw::prefab::components::IsA");
            let no_peid = !n
                .components
                .contains_key("jackdaw::prefab::components::PrefabEntityId");
            if has_name && no_isa && no_peid && n.parent.is_none() {
                Some(i)
            } else {
                None
            }
        })
        .collect();
    assert!(
        !promoted.is_empty(),
        "at least one authored top-level node with no prefab markers exists after unbundle"
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

    // Centroid is (3, 0, 0). Instance Transform.x in the source AST
    // should equal the centroid. The packaged children live in the
    // prefab file with centroid-relative translations so a fresh
    // instance spawn at world (3, 0, 0) reproduces the originals at
    // (2, 0, 0) and (4, 0, 0).
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

    // Verify the prefab file: children's translations should be
    // (-1, 0, 0) and (1, 0, 0) (centroid-relative).
    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
    let scene = value["scene"].as_array().unwrap();
    let mut child_xs: Vec<f64> = Vec::new();
    for entry in scene.iter().skip(1) {
        let tx = entry["components"]
            .get("bevy_transform::components::transform::Transform")
            .unwrap();
        let x = tx["translation"].as_array().unwrap()[0].as_f64().unwrap();
        child_xs.push(x);
    }
    child_xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(child_xs.len(), 2, "two packaged children in the prefab");
    assert!(
        (child_xs[0] - (-1.0)).abs() < 1e-4,
        "first child Transform.x is -1.0 (centroid-relative); got {child_xs:?}"
    );
    assert!(
        (child_xs[1] - 1.0).abs() < 1e-4,
        "second child Transform.x is 1.0 (centroid-relative); got {child_xs:?}"
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

    // The instance entity in the source AST inherits Visibility from
    // the prefab's synthetic root via the resolver merge; the local
    // node only needs to carry the sparse delta (IsA + placement
    // Transform). Verify the instance exists and the prefab's synthetic
    // root has Visibility (the latter is what the resolver will pull in).
    let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
    assert!(
        ast.entities_with_component("jackdaw::prefab::components::IsA")
            .next()
            .is_some(),
        "instance node with IsA exists in source AST after save"
    );
}

#[test]
fn load_scene_from_jsn_backfills_transform_require_chain() {
    use jackdaw_jsn::format::JsnEntity;
    use std::collections::HashMap;
    use std::path::Path;

    let mut app = make_app_for_prefab_tests();
    let entity = JsnEntity {
        parent: None,
        components: {
            let mut m = HashMap::new();
            m.insert(
                "bevy_transform::components::transform::Transform".to_string(),
                serde_json::json!({
                    "translation": [1.0, 2.0, 3.0],
                    "rotation": [0.0, 0.0, 0.0, 1.0],
                    "scale": [1.0, 1.0, 1.0],
                }),
            );
            m
        },
    };
    let spawned = jackdaw::scene_io::load_scene_from_jsn(
        app.world_mut(),
        &[entity],
        Path::new("."),
        &HashMap::new(),
    );
    assert_eq!(spawned.len(), 1);
    let e = spawned[0];
    assert!(app.world().get::<bevy::prelude::Transform>(e).is_some());
    assert!(
        app.world()
            .get::<bevy::prelude::GlobalTransform>(e)
            .is_some(),
        "spawn path backfills GlobalTransform when Transform is reflected in",
    );
    assert!(
        app.world()
            .get::<bevy::transform::components::TransformTreeChanged>(e)
            .is_some(),
        "spawn path backfills TransformTreeChanged when Transform is reflected in",
    );
}

#[test]
fn load_scene_from_jsn_backfills_visibility_require_chain() {
    use bevy::camera::visibility::{InheritedVisibility, ViewVisibility, Visibility};
    use jackdaw_jsn::format::JsnEntity;
    use std::collections::HashMap;
    use std::path::Path;

    let mut app = make_app_for_prefab_tests();
    let entity = JsnEntity {
        parent: None,
        components: {
            let mut m = HashMap::new();
            m.insert(
                "bevy_camera::visibility::Visibility".to_string(),
                serde_json::Value::String("Inherited".to_string()),
            );
            m
        },
    };
    let spawned = jackdaw::scene_io::load_scene_from_jsn(
        app.world_mut(),
        &[entity],
        Path::new("."),
        &HashMap::new(),
    );
    let e = spawned[0];
    assert!(app.world().get::<Visibility>(e).is_some());
    assert!(
        app.world().get::<InheritedVisibility>(e).is_some(),
        "spawn path backfills InheritedVisibility when Visibility is reflected in",
    );
    assert!(
        app.world().get::<ViewVisibility>(e).is_some(),
        "spawn path backfills ViewVisibility when Visibility is reflected in",
    );
}

#[test]
fn save_as_prefab_skips_ecs_only_descendants() {
    // Brushes have ECS-only children (face overlays, clip previews) that
    // aren't registered in the AST. Bundling them into the prefab would
    // orphan them after respawn because the in-place restructure has no
    // way to re-attach unknown ECS entities to the brush they belong to.
    // The brush spawn pipeline re-derives them from the brush data, so
    // they don't belong in the prefab file at all.
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("derived.jsn");

    let mut app = make_app_for_prefab_tests();
    let brush = app
        .world_mut()
        .spawn(bevy::prelude::Name::new("brush"))
        .id();
    // Child entity exists in ECS as a child of `brush` but never gets
    // registered in the AST - emulating a brush face overlay.
    let _derived = app
        .world_mut()
        .spawn((
            bevy::prelude::Name::new("derived-overlay"),
            bevy::ecs::hierarchy::ChildOf(brush),
        ))
        .id();
    {
        let mut ast = app.world_mut().resource_mut::<jackdaw_jsn::SceneJsnAst>();
        ast.create_node(brush, None);
    }

    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &[brush], &target);

    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
    let scene = value["scene"].as_array().unwrap();
    assert_eq!(
        scene.len(),
        2,
        "synthetic root + brush only; ECS-only derived child must be filtered out"
    );
}

#[test]
fn save_then_respawn_keeps_isa_on_instance_entity() {
    // Reproduces the editor flow: draw a brush, save selection as prefab,
    // let the cache-driven respawn fire, then assert that the new
    // instance entity has IsA on its ECS - which classify_entity needs
    // to assign EntityCategory::Prefab and draw the Package icon.
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("p_test.jsn");

    let mut app = make_app_for_prefab_tests();
    let brush = app
        .world_mut()
        .spawn((
            bevy::prelude::Name::new("Brush"),
            bevy::prelude::Transform::from_xyz(1.0, 0.0, 0.0),
            bevy::camera::visibility::Visibility::Inherited,
        ))
        .id();
    {
        let mut ast = app.world_mut().resource_mut::<jackdaw_jsn::SceneJsnAst>();
        let key = ast.create_node(brush, None);
        ast.insert_component(
            key,
            "bevy_transform::components::transform::Transform",
            serde_json::json!({
                "translation": [1.0, 0.0, 0.0],
                "rotation": [0.0, 0.0, 0.0, 1.0],
                "scale": [1.0, 1.0, 1.0],
            }),
        );
    }
    app.update();

    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &[brush], &target);
    // Force the cache-driven driver to run reload_all_instances.
    jackdaw::prefab::watcher::reload_all_instances(app.world_mut());

    // After respawn, find the instance entity (the one carrying IsA).
    let mut q = app
        .world_mut()
        .query::<(bevy::prelude::Entity, &jackdaw::prefab::IsA)>();
    let instances: Vec<bevy::prelude::Entity> = q.iter(app.world()).map(|(e, _)| e).collect();
    assert_eq!(
        instances.len(),
        1,
        "exactly one instance entity must carry IsA after respawn; got {instances:?}"
    );

    // The instance entity must also carry the visibility / transform
    // require-chain so Bevy's hierarchy propagation doesn't B0004-warn.
    let instance = instances[0];
    let world = app.world();
    assert!(
        world.get::<bevy::prelude::Transform>(instance).is_some(),
        "instance has Transform after respawn"
    );
    assert!(
        world
            .get::<bevy::prelude::GlobalTransform>(instance)
            .is_some(),
        "instance has GlobalTransform after respawn (require-chain backfill)"
    );
    assert!(
        world
            .get::<bevy::camera::visibility::Visibility>(instance)
            .is_some(),
        "instance has Visibility after respawn"
    );
    assert!(
        world
            .get::<bevy::camera::visibility::InheritedVisibility>(instance)
            .is_some(),
        "instance has InheritedVisibility after respawn (require-chain backfill)"
    );
}

#[test]
fn three_instances_keep_independent_positions() {
    // Spawn the same prefab three times at three different positions.
    // Each instance must keep its own Transform; adding a new instance
    // must NOT reset existing instances' positions.
    use bevy::math::Vec3;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");

    // Hand-write a minimal prefab: synthetic root + 1 brush child.
    let prefab_json = serde_json::json!({
        "jsn": { "format_version": [3, 0, 0], "editor_version": "0", "bevy_version": "0.18" },
        "metadata": { "name": "", "description": "", "author": "", "created": "", "modified": "" },
        "assets": {},
        "editor": null,
        "scene": [
            {
                "components": {
                    "jackdaw::prefab::components::Prefab": null,
                    "jackdaw::prefab::components::PrefabEntityId": 0,
                    "bevy_ecs::name::Name": "p",
                    "bevy_transform::components::transform::Transform": {
                        "translation": [0.0, 0.0, 0.0],
                        "rotation": [0.0, 0.0, 0.0, 1.0],
                        "scale": [1.0, 1.0, 1.0]
                    },
                    "bevy_camera::visibility::Visibility": "Inherited"
                }
            },
            {
                "parent": 0,
                "components": {
                    "jackdaw::prefab::components::PrefabEntityId": 1,
                    "bevy_ecs::name::Name": "child",
                    "bevy_transform::components::transform::Transform": {
                        "translation": [0.0, 0.0, 0.0],
                        "rotation": [0.0, 0.0, 0.0, 1.0],
                        "scale": [1.0, 1.0, 1.0]
                    },
                    "bevy_camera::visibility::Visibility": "Inherited"
                }
            }
        ]
    });
    std::fs::write(
        &prefab_path,
        serde_json::to_string_pretty(&prefab_json).unwrap(),
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();

    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        Vec3::new(10.0, 0.0, 0.0),
    );
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        Vec3::new(20.0, 0.0, 0.0),
    );
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        Vec3::new(30.0, 0.0, 0.0),
    );

    // Three instance entities. Each must have a distinct Transform.translation.
    let world = app.world_mut();
    let mut q = world.query::<(
        bevy::prelude::Entity,
        &jackdaw::prefab::IsA,
        &bevy::prelude::Transform,
    )>();
    let mut xs: Vec<f32> = q.iter(world).map(|(_, _, t)| t.translation.x).collect();
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());

    assert_eq!(xs.len(), 3, "exactly three instance entities exist");
    assert!(
        (xs[0] - 10.0).abs() < 1e-3,
        "first instance kept X=10; got {xs:?}"
    );
    assert!(
        (xs[1] - 20.0).abs() < 1e-3,
        "second instance kept X=20; got {xs:?}"
    );
    assert!(
        (xs[2] - 30.0).abs() < 1e-3,
        "third instance kept X=30; got {xs:?}"
    );
}

#[test]
fn three_instances_each_carry_isa_on_ecs() {
    // Every instance entity must carry IsA on its ECS state so the
    // outliner classifies it as `Prefab` (Package icon). After
    // multiple spawn_instance calls + reload_all_instances passes, no
    // instance can be missing IsA.
    use bevy::math::Vec3;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");

    let prefab_json = serde_json::json!({
        "jsn": { "format_version": [3, 0, 0], "editor_version": "0", "bevy_version": "0.18" },
        "metadata": { "name": "", "description": "", "author": "", "created": "", "modified": "" },
        "assets": {},
        "editor": null,
        "scene": [
            {
                "components": {
                    "jackdaw::prefab::components::Prefab": null,
                    "jackdaw::prefab::components::PrefabEntityId": 0,
                    "bevy_ecs::name::Name": "p",
                    "bevy_transform::components::transform::Transform": {
                        "translation": [0.0, 0.0, 0.0],
                        "rotation": [0.0, 0.0, 0.0, 1.0],
                        "scale": [1.0, 1.0, 1.0]
                    },
                    "bevy_camera::visibility::Visibility": "Inherited"
                }
            }
        ]
    });
    std::fs::write(
        &prefab_path,
        serde_json::to_string_pretty(&prefab_json).unwrap(),
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();
    for x in [10.0_f32, 20.0, 30.0] {
        jackdaw::prefab::operators::spawn_instance(
            app.world_mut(),
            &prefab_path,
            Vec3::new(x, 0.0, 0.0),
        );
    }

    let world = app.world_mut();
    let mut q = world.query::<&jackdaw::prefab::IsA>();
    let isa_count = q.iter(world).count();
    assert_eq!(
        isa_count, 3,
        "three instance entities, each carrying IsA on ECS",
    );
}

#[test]
fn save_then_drag_spawn_twice_keeps_distinct_positions() {
    // Mirrors the editor flow that's been showing position clustering:
    // 1. Draw a brush at position A, save selection as prefab.
    //    -> instance #1 ends up at A.
    // 2. spawn_instance at position B (drag from asset browser).
    //    -> instance #2 at B.
    // 3. spawn_instance at position C.
    //    -> instance #3 at C.
    // After all three, each instance's Transform must reflect its
    // original placement. Adding instance #3 must NOT reset #1 or #2.
    use bevy::math::Vec3;

    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("p.jsn");

    let mut app = make_app_for_prefab_tests();
    let source = app
        .world_mut()
        .spawn((
            bevy::prelude::Name::new("source"),
            bevy::prelude::Transform::from_xyz(5.0, 0.0, 0.0),
            bevy::camera::visibility::Visibility::Inherited,
        ))
        .id();
    {
        let mut ast = app.world_mut().resource_mut::<jackdaw_jsn::SceneJsnAst>();
        let key = ast.create_node(source, None);
        ast.insert_component(
            key,
            "bevy_ecs::name::Name",
            serde_json::Value::String("source".to_string()),
        );
        ast.insert_component(
            key,
            "bevy_transform::components::transform::Transform",
            serde_json::json!({
                "translation": [5.0, 0.0, 0.0],
                "rotation": [0.0, 0.0, 0.0, 1.0],
                "scale": [1.0, 1.0, 1.0],
            }),
        );
    }
    app.update();

    // Step 1: save selection as prefab. Source despawn + spawn_instance
    // at centroid (5, 0, 0).
    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &[source], &target);

    // Step 2: drag-spawn at (20, 0, 0).
    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &target, Vec3::new(20.0, 0.0, 0.0));

    // Step 3: drag-spawn at (30, 0, 0). This is the move that the
    // user reports resetting the earlier two instances' positions.
    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &target, Vec3::new(30.0, 0.0, 0.0));

    let world = app.world_mut();
    let mut q = world.query::<(
        bevy::prelude::Entity,
        &jackdaw::prefab::IsA,
        &bevy::prelude::Transform,
    )>();
    let mut xs: Vec<f32> = q.iter(world).map(|(_, _, t)| t.translation.x).collect();
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());

    assert_eq!(xs.len(), 3, "three instances exist");
    assert!(
        (xs[0] - 5.0).abs() < 1e-3,
        "instance from save_as_prefab kept X=5; got {xs:?}"
    );
    assert!(
        (xs[1] - 20.0).abs() < 1e-3,
        "second drag-spawn kept X=20; got {xs:?}"
    );
    assert!(
        (xs[2] - 30.0).abs() < 1e-3,
        "third drag-spawn kept X=30; got {xs:?}"
    );
}

#[test]
fn set_transform_sync_after_external_execute_writes_to_ast() {
    // Reproduces the gizmo-drag-then-reload regression: a "live drag"
    // path mutates the ECS Transform directly and pushes a
    // `SetTransform` via `push_executed` (no execute). Previously this
    // left the AST holding the pre-drag value, and a subsequent reload
    // (triggered e.g. by a prefab spawn) snapped the entity back.
    //
    // `SetTransform::sync_after_external_execute` is the hook that
    // brings the AST up to date.
    use bevy::math::Vec3;
    use jackdaw_commands::EditorCommand;

    let mut app = make_app_for_prefab_tests();
    let entity = app
        .world_mut()
        .spawn((
            bevy::prelude::Name::new("e"),
            bevy::prelude::Transform::from_xyz(0.0, 0.0, 0.0),
        ))
        .id();
    {
        let mut ast = app.world_mut().resource_mut::<jackdaw_jsn::SceneJsnAst>();
        let key = ast.create_node(entity, None);
        ast.insert_component(
            key,
            "bevy_transform::components::transform::Transform",
            serde_json::json!({
                "translation": [0.0, 0.0, 0.0],
                "rotation": [0.0, 0.0, 0.0, 1.0],
                "scale": [1.0, 1.0, 1.0],
            }),
        );
    }

    let dragged_to = Vec3::new(7.0, 0.0, -3.0);
    if let Some(mut t) = app.world_mut().get_mut::<bevy::prelude::Transform>(entity) {
        t.translation = dragged_to;
    }

    let cmd = jackdaw::commands::SetTransform {
        entity,
        old_transform: bevy::prelude::Transform::from_xyz(0.0, 0.0, 0.0),
        new_transform: bevy::prelude::Transform::from_xyz(dragged_to.x, dragged_to.y, dragged_to.z),
    };
    cmd.sync_after_external_execute(app.world_mut());

    let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
    let key = ast.key_for_entity(entity).unwrap();
    let tx = ast
        .get_component_at(key, "bevy_transform::components::transform::Transform")
        .unwrap();
    let translation = tx["translation"].as_array().unwrap();
    let x = translation[0].as_f64().unwrap() as f32;
    let z = translation[2].as_f64().unwrap() as f32;
    assert!(
        (x - 7.0).abs() < 1e-4,
        "AST Transform.x reflects the dragged value (7.0); got {x}"
    );
    assert!(
        (z - (-3.0)).abs() < 1e-4,
        "AST Transform.z reflects the dragged value (-3.0); got {z}"
    );
}

#[test]
fn snapshot_captures_inherited_descendant_edit_as_override() {
    // After editing a component on an inherited brush child (ECS-only,
    // materialised by the resolver), the snapshot AST must encode the
    // change as an override entry under the instance, not as a top-level
    // authored entity. Without this, the snapshot loses the prefab
    // relationship and a subsequent reload spawns a duplicate inherited
    // child alongside the edited one.
    use bevy::math::Vec3;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");

    // Prefab with one synthetic root + one child carrying Transform.
    let prefab_json = serde_json::json!({
        "jsn": { "format_version": [3, 0, 0], "editor_version": "0", "bevy_version": "0.18" },
        "metadata": { "name": "", "description": "", "author": "", "created": "", "modified": "" },
        "assets": {},
        "editor": null,
        "scene": [
            {
                "components": {
                    "jackdaw::prefab::components::Prefab": null,
                    "jackdaw::prefab::components::PrefabEntityId": 0,
                    "bevy_ecs::name::Name": "p",
                    "bevy_transform::components::transform::Transform": {
                        "translation": [0.0, 0.0, 0.0],
                        "rotation": [0.0, 0.0, 0.0, 1.0],
                        "scale": [1.0, 1.0, 1.0]
                    },
                    "bevy_camera::visibility::Visibility": "Inherited"
                }
            },
            {
                "parent": 0,
                "components": {
                    "jackdaw::prefab::components::PrefabEntityId": 1,
                    "bevy_ecs::name::Name": "child",
                    "bevy_transform::components::transform::Transform": {
                        "translation": [1.0, 0.0, 0.0],
                        "rotation": [0.0, 0.0, 0.0, 1.0],
                        "scale": [1.0, 1.0, 1.0]
                    },
                    "bevy_camera::visibility::Visibility": "Inherited"
                }
            }
        ]
    });
    std::fs::write(
        &prefab_path,
        serde_json::to_string_pretty(&prefab_json).unwrap(),
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        Vec3::new(5.0, 0.0, 0.0),
    );

    // The resolver materialises a child entity (PrefabEntityId=1).
    // Find it and mutate its Transform to simulate an inherited-child
    // edit.
    let child_entity = {
        let world = app.world_mut();
        let mut q = world.query::<(bevy::prelude::Entity, &jackdaw::prefab::PrefabEntityId)>();
        q.iter(world)
            .find(|(_, id)| id.0 == 1)
            .map(|(e, _)| e)
            .expect("inherited child with PrefabEntityId 1 exists")
    };
    if let Some(mut t) = app
        .world_mut()
        .get_mut::<bevy::prelude::Transform>(child_entity)
    {
        t.translation = Vec3::new(99.0, 0.0, 0.0);
    }

    // Capture a snapshot. The result must encode the edit as an override.
    let snapshot = jackdaw::scene_io::build_snapshot_ast(app.world_mut());

    // The instance entity (with IsA) should still have a child node
    // carrying PrefabEntityId(1). The child node should NOT have a Name
    // (matches prefab, omitted), but SHOULD have a Transform (differs
    // from prefab).
    let isa_type = "jackdaw::prefab::components::IsA";
    let prefab_entity_id_type = "jackdaw::prefab::components::PrefabEntityId";
    let instance_idx = snapshot
        .nodes
        .iter()
        .position(|n| n.components.contains_key(isa_type))
        .expect("instance node in snapshot");
    let override_node = snapshot
        .nodes
        .iter()
        .find(|n| {
            n.parent == Some(instance_idx)
                && n.components
                    .get(prefab_entity_id_type)
                    .and_then(serde_json::Value::as_u64)
                    == Some(1)
        })
        .expect("override node for PrefabEntityId 1 under the instance");

    assert!(
        !override_node
            .components
            .contains_key("bevy_ecs::name::Name"),
        "name matched the prefab baseline; should be omitted from the override. got components: {:?}",
        override_node.components.keys().collect::<Vec<_>>()
    );
    let tx = override_node
        .components
        .get("bevy_transform::components::transform::Transform")
        .expect("Transform diverged from prefab and should appear in the override");
    let x = tx["translation"].as_array().unwrap()[0].as_f64().unwrap();
    assert!(
        (x - 99.0).abs() < 1e-4,
        "override carries the edited translation X (99.0); got {x}"
    );
}

#[test]
fn snapshot_install_plus_reload_keeps_inherited_child_visible() {
    // Reproduces the regression: capturing a snapshot then dragging in
    // another instance (which triggers `reload_all_instances`) must NOT
    // erase the existing instance's children. Earlier the resolver was
    // skipping materialisation of inherited descendants whose id matched
    // an override entry, but the override entry only carried
    // `PrefabEntityId`. After reload, spawned entities had no Name /
    // Transform / Brush data.
    use bevy::math::Vec3;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");

    let prefab_json = serde_json::json!({
        "jsn": { "format_version": [3, 0, 0], "editor_version": "0", "bevy_version": "0.18" },
        "metadata": { "name": "", "description": "", "author": "", "created": "", "modified": "" },
        "assets": {},
        "editor": null,
        "scene": [
            {
                "components": {
                    "jackdaw::prefab::components::Prefab": null,
                    "jackdaw::prefab::components::PrefabEntityId": 0,
                    "bevy_ecs::name::Name": "p",
                    "bevy_transform::components::transform::Transform": {
                        "translation": [0.0, 0.0, 0.0],
                        "rotation": [0.0, 0.0, 0.0, 1.0],
                        "scale": [1.0, 1.0, 1.0]
                    },
                    "bevy_camera::visibility::Visibility": "Inherited"
                }
            },
            {
                "parent": 0,
                "components": {
                    "jackdaw::prefab::components::PrefabEntityId": 1,
                    "bevy_ecs::name::Name": "child",
                    "bevy_transform::components::transform::Transform": {
                        "translation": [2.0, 0.0, 0.0],
                        "rotation": [0.0, 0.0, 0.0, 1.0],
                        "scale": [1.0, 1.0, 1.0]
                    },
                    "bevy_camera::visibility::Visibility": "Inherited"
                }
            }
        ]
    });
    std::fs::write(
        &prefab_path,
        serde_json::to_string_pretty(&prefab_json).unwrap(),
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        Vec3::new(10.0, 0.0, 0.0),
    );

    // Capture the snapshot (the install side effect updates live AST).
    let _ = jackdaw::scene_io::build_snapshot_ast(app.world_mut());

    // Drag in a second instance, which triggers reload_all_instances.
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        Vec3::new(20.0, 0.0, 0.0),
    );

    // Both instances should have a visible child entity carrying the
    // inherited Name and Transform.
    let world = app.world_mut();
    let mut q = world.query::<(
        &jackdaw::prefab::PrefabEntityId,
        &bevy::prelude::Name,
        &bevy::prelude::Transform,
    )>();
    let children: Vec<_> = q.iter(world).filter(|(id, _, _)| id.0 == 1).collect();
    assert_eq!(
        children.len(),
        2,
        "two instances => two inherited child entities with PrefabEntityId 1"
    );
    for (_, name, _) in &children {
        assert_eq!(name.as_str(), "child", "inherited Name carries through");
    }
}

#[test]
fn snapshot_round_trip_undoes_spawn_instance() {
    // What `Ctrl+Z` does on a prefab-spawn operator boils down to:
    // capture a snapshot, run the spawn, then apply the captured
    // snapshot back. This test exercises that round-trip directly,
    // bypassing the operator framework so the harness stays minimal.
    use bevy::math::Vec3;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");
    let prefab_json = serde_json::json!({
        "jsn": { "format_version": [3, 0, 0], "editor_version": "0", "bevy_version": "0.18" },
        "metadata": { "name": "", "description": "", "author": "", "created": "", "modified": "" },
        "assets": {},
        "editor": null,
        "scene": [{
            "components": {
                "jackdaw::prefab::components::Prefab": null,
                "jackdaw::prefab::components::PrefabEntityId": 0,
                "bevy_ecs::name::Name": "p",
                "bevy_transform::components::transform::Transform": {
                    "translation": [0.0, 0.0, 0.0],
                    "rotation": [0.0, 0.0, 0.0, 1.0],
                    "scale": [1.0, 1.0, 1.0]
                },
                "bevy_camera::visibility::Visibility": "Inherited"
            }
        }]
    });
    std::fs::write(
        &prefab_path,
        serde_json::to_string_pretty(&prefab_json).unwrap(),
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();

    // Capture the "before" snapshot AST.
    let before_ast = jackdaw::scene_io::build_snapshot_ast(app.world_mut());
    {
        let world = app.world_mut();
        let mut q = world.query::<&jackdaw::prefab::IsA>();
        assert_eq!(q.iter(world).count(), 0, "no instances before spawn");
    }

    // Run the spawn.
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        Vec3::new(7.0, 0.0, 0.0),
    );
    {
        let world = app.world_mut();
        let mut q = world.query::<&jackdaw::prefab::IsA>();
        assert_eq!(q.iter(world).count(), 1, "instance spawned");
    }

    // Apply the before-snapshot — what SnapshotDiff::undo does.
    jackdaw::scene_io::apply_ast_to_world(app.world_mut(), &before_ast);
    {
        let world = app.world_mut();
        let mut q = world.query::<&jackdaw::prefab::IsA>();
        assert_eq!(
            q.iter(world).count(),
            0,
            "instance removed after applying the before-snapshot (undo)"
        );
    }
}

#[test]
fn save_3_brushes_as_prefab_produces_3_inherited_children() {
    // Reproduces user-reported bug: selecting 3 brushes and saving as a
    // prefab should produce ONE instance entity with THREE inherited
    // children, not one inherited child + three top-level brushes.

    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("p_boxes.jsn");

    let mut app = make_app_for_prefab_tests();
    let mut entities = Vec::new();
    for i in 0..3 {
        let entity = app
            .world_mut()
            .spawn((
                bevy::prelude::Name::new("Brush"),
                bevy::prelude::Transform::from_xyz(i as f32 * 2.0, 0.0, 0.0),
                bevy::camera::visibility::Visibility::Inherited,
            ))
            .id();
        {
            let mut ast = app.world_mut().resource_mut::<jackdaw_jsn::SceneJsnAst>();
            let key = ast.create_node(entity, None);
            ast.insert_component(
                key,
                "bevy_ecs::name::Name",
                serde_json::Value::String("Brush".to_string()),
            );
            ast.insert_component(
                key,
                "bevy_transform::components::transform::Transform",
                serde_json::json!({
                    "translation": [i as f32 * 2.0, 0.0, 0.0],
                    "rotation": [0.0, 0.0, 0.0, 1.0],
                    "scale": [1.0, 1.0, 1.0],
                }),
            );
        }
        entities.push(entity);
    }
    app.update();

    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &entities, &target);

    // The prefab file on disk should contain the synthetic root +
    // three child entries.
    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
    let scene = value["scene"].as_array().unwrap();
    assert_eq!(
        scene.len(),
        4,
        "prefab file has synthetic root + 3 children; got {} entries",
        scene.len()
    );

    // After the save, the live world should have:
    //  - one IsA-bearing instance entity
    //  - three inherited Brush children (PrefabEntityId set, ChildOf the instance)
    //  - zero authored top-level Brushes left over
    let world = app.world_mut();
    let mut isa_q = world.query::<&jackdaw::prefab::IsA>();
    let isa_count = isa_q.iter(world).count();
    assert_eq!(isa_count, 1, "exactly one instance after save");

    let mut child_q = world.query::<(
        &jackdaw::prefab::PrefabEntityId,
        Option<&bevy::ecs::hierarchy::ChildOf>,
        &bevy::prelude::Name,
    )>();
    let mut inherited_under_instance = 0;
    let mut top_level_brushes = 0;
    for (_peid, child_of, name) in child_q.iter(world) {
        if name.as_str() != "Brush" {
            continue;
        }
        if child_of.is_some() {
            inherited_under_instance += 1;
        } else {
            top_level_brushes += 1;
        }
    }
    assert_eq!(
        inherited_under_instance, 3,
        "three inherited brush children under the instance; got {inherited_under_instance}"
    );
    assert_eq!(
        top_level_brushes, 0,
        "zero authored top-level brushes left over; got {top_level_brushes}"
    );
}

#[test]
fn save_3_brushes_survives_snapshot_capture_and_install() {
    // The operator framework calls `build_snapshot_ast` twice (before
    // and after) and installs each as the live AST. The prefabify pass
    // reduces inherited descendants to override entries. After all of
    // this, the world must still have the same 3 inherited children
    // under the instance — not lose any to the prefabify+install
    // round-trip.

    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("p_boxes.jsn");

    let mut app = make_app_for_prefab_tests();
    let mut entities = Vec::new();
    for i in 0..3 {
        let entity = app
            .world_mut()
            .spawn((
                bevy::prelude::Name::new("Brush"),
                bevy::prelude::Transform::from_xyz(i as f32 * 2.0, 0.0, 0.0),
                bevy::camera::visibility::Visibility::Inherited,
            ))
            .id();
        {
            let mut ast = app.world_mut().resource_mut::<jackdaw_jsn::SceneJsnAst>();
            let key = ast.create_node(entity, None);
            ast.insert_component(
                key,
                "bevy_ecs::name::Name",
                serde_json::Value::String("Brush".to_string()),
            );
            ast.insert_component(
                key,
                "bevy_transform::components::transform::Transform",
                serde_json::json!({
                    "translation": [i as f32 * 2.0, 0.0, 0.0],
                    "rotation": [0.0, 0.0, 0.0, 1.0],
                    "scale": [1.0, 1.0, 1.0],
                }),
            );
        }
        entities.push(entity);
    }
    app.update();

    // Simulate the operator framework: before-snapshot capture (which
    // installs as live AST), then the actual save, then after-snapshot
    // (also installs as live), then a reload triggered by cache change.
    let _before = jackdaw::scene_io::build_snapshot_ast(app.world_mut());

    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &entities, &target);

    let _after = jackdaw::scene_io::build_snapshot_ast(app.world_mut());

    // The cache-driven driver respawn (next-frame side effect of cache
    // mutation in save_as_prefab_from_selection):
    jackdaw::prefab::watcher::reload_all_instances(app.world_mut());

    // Assertions: one instance, three inherited children, zero top-level brushes.
    let world = app.world_mut();
    let mut isa_q = world.query::<&jackdaw::prefab::IsA>();
    assert_eq!(isa_q.iter(world).count(), 1, "exactly one instance");

    let mut child_q = world.query::<(
        &jackdaw::prefab::PrefabEntityId,
        Option<&bevy::ecs::hierarchy::ChildOf>,
        &bevy::prelude::Name,
    )>();
    let mut inherited_under_instance = 0;
    let mut top_level_brushes = 0;
    for (_peid, child_of, name) in child_q.iter(world) {
        if name.as_str() != "Brush" {
            continue;
        }
        if child_of.is_some() {
            inherited_under_instance += 1;
        } else {
            top_level_brushes += 1;
        }
    }
    assert_eq!(
        inherited_under_instance, 3,
        "three inherited brushes under instance after snapshot+respawn; got {inherited_under_instance}"
    );
    assert_eq!(
        top_level_brushes, 0,
        "zero top-level brushes after snapshot+respawn; got {top_level_brushes}"
    );
}

#[test]
fn spawn_instance_undo_via_framework_snapshot_round_trip_removes_instance() {
    // Reproduce the "drag prefab in, undo, instance still there" bug by
    // simulating the operator framework's snapshot path:
    //   1) capture before-snapshot via the snapshotter
    //   2) run spawn_instance
    //   3) capture after-snapshot
    //   4) apply before-snapshot (what SnapshotDiff::undo does)
    //   5) assert no instance entities remain
    use bevy::math::Vec3;
    use bevy::prelude::Mut;
    use jackdaw_api_internal::snapshot::ActiveSnapshotter;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");
    let prefab_json = serde_json::json!({
        "jsn": { "format_version": [3, 0, 0], "editor_version": "0", "bevy_version": "0.18" },
        "metadata": { "name": "", "description": "", "author": "", "created": "", "modified": "" },
        "assets": {},
        "editor": null,
        "scene": [
            {
                "components": {
                    "jackdaw::prefab::components::Prefab": null,
                    "jackdaw::prefab::components::PrefabEntityId": 0,
                    "bevy_ecs::name::Name": "p",
                    "bevy_transform::components::transform::Transform": {
                        "translation": [0.0, 0.0, 0.0],
                        "rotation": [0.0, 0.0, 0.0, 1.0],
                        "scale": [1.0, 1.0, 1.0]
                    },
                    "bevy_camera::visibility::Visibility": "Inherited"
                }
            },
            {
                "parent": 0,
                "components": {
                    "jackdaw::prefab::components::PrefabEntityId": 1,
                    "bevy_ecs::name::Name": "child",
                    "bevy_transform::components::transform::Transform": {
                        "translation": [0.0, 0.0, 0.0],
                        "rotation": [0.0, 0.0, 0.0, 1.0],
                        "scale": [1.0, 1.0, 1.0]
                    },
                    "bevy_camera::visibility::Visibility": "Inherited"
                }
            }
        ]
    });
    std::fs::write(
        &prefab_path,
        serde_json::to_string_pretty(&prefab_json).unwrap(),
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();
    // The JsnAstSnapshotter captures editor state alongside the AST;
    // initialize the resources it reads so its capture/apply don't panic.
    app.init_resource::<jackdaw::brush::EditMode>();
    app.init_resource::<jackdaw::gizmos::GizmoMode>();
    app.init_resource::<jackdaw::gizmos::GizmoSpace>();
    app.init_resource::<jackdaw::snapping::SnapSettings>();
    app.init_resource::<jackdaw::view_modes::ViewModeSettings>();
    app.init_resource::<jackdaw::viewport_overlays::OverlaySettings>();
    app.init_resource::<jackdaw_avian_integration::PhysicsOverlayConfig>();
    app.init_resource::<jackdaw::viewport_select::GroupEditState>();

    app.world_mut().insert_resource(ActiveSnapshotter(Box::new(
        jackdaw::undo_snapshot::JsnAstSnapshotter,
    )));

    let before = app
        .world_mut()
        .resource_scope(|world, snapshotter: Mut<ActiveSnapshotter>| snapshotter.0.capture(world));

    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        Vec3::new(7.0, 0.0, 0.0),
    );
    {
        let world = app.world_mut();
        let mut q = world.query::<&jackdaw::prefab::IsA>();
        assert_eq!(q.iter(world).count(), 1, "instance present after spawn");
    }

    let after = app
        .world_mut()
        .resource_scope(|world, snapshotter: Mut<ActiveSnapshotter>| snapshotter.0.capture(world));
    assert!(
        !before.equals(&*after),
        "before and after snapshots should differ"
    );

    before.apply(app.world_mut());

    let world = app.world_mut();
    let mut q = world.query::<&jackdaw::prefab::IsA>();
    let count = q.iter(world).count();
    assert_eq!(
        count, 0,
        "undo (snapshot apply) removed the instance; {count} still present"
    );
}

#[test]
fn reload_all_instances_preserves_command_history() {
    // Regression for the "drag prefab in, undo doesn't remove it"
    // bug. `reload_all_instances` used to call `clear_scene_entities`,
    // which truncates the undo stack. That ran on the next frame
    // after spawn_instance (cache-driven driver), wiping the
    // SnapshotDiff the operator framework had just pushed.
    use bevy::math::Vec3;
    use jackdaw_commands::{CommandHistory, EditorCommand};

    struct NoopCommand;
    impl EditorCommand for NoopCommand {
        fn execute(&mut self, _world: &mut bevy::prelude::World) {}
        fn undo(&mut self, _world: &mut bevy::prelude::World) {}
        fn description(&self) -> &str {
            "noop"
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");
    let prefab_json = serde_json::json!({
        "jsn": { "format_version": [3, 0, 0], "editor_version": "0", "bevy_version": "0.18" },
        "metadata": { "name": "", "description": "", "author": "", "created": "", "modified": "" },
        "assets": {},
        "editor": null,
        "scene": [{
            "components": {
                "jackdaw::prefab::components::Prefab": null,
                "jackdaw::prefab::components::PrefabEntityId": 0,
                "bevy_ecs::name::Name": "p",
                "bevy_transform::components::transform::Transform": {
                    "translation": [0.0, 0.0, 0.0],
                    "rotation": [0.0, 0.0, 0.0, 1.0],
                    "scale": [1.0, 1.0, 1.0]
                },
                "bevy_camera::visibility::Visibility": "Inherited"
            }
        }]
    });
    std::fs::write(
        &prefab_path,
        serde_json::to_string_pretty(&prefab_json).unwrap(),
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();

    // Push a canary command before any prefab work.
    app.world_mut()
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(NoopCommand));
    let before = app.world().resource::<CommandHistory>().undo_stack.len();
    assert_eq!(before, 1, "canary present before reload");

    // spawn_instance internally calls reload_all_instances.
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        Vec3::new(0.0, 0.0, 0.0),
    );

    let after = app.world().resource::<CommandHistory>().undo_stack.len();
    assert_eq!(
        after, 1,
        "undo stack preserved across reload_all_instances; got {after}"
    );
}

#[test]
fn unbundle_resolves_key_from_entity_after_snapshot_install() {
    // The framework's before-snapshot capture during operator dispatch
    // rewrites the live AST (build_snapshot_ast installs the captured
    // snapshot, with prefabify_inherited_descendants reshuffling node
    // indices). A key fetched before the operator runs would therefore
    // be stale by the time the operator's body reads the live AST.
    // The fix is to pass the ECS Entity and look the key up inside
    // the operator. This test exercises that flow.
    use bevy::math::Vec3;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");
    let prefab_json = serde_json::json!({
        "jsn": { "format_version": [3, 0, 0], "editor_version": "0", "bevy_version": "0.18" },
        "metadata": { "name": "", "description": "", "author": "", "created": "", "modified": "" },
        "assets": {},
        "editor": null,
        "scene": [{
            "components": {
                "jackdaw::prefab::components::Prefab": null,
                "jackdaw::prefab::components::PrefabEntityId": 0,
                "bevy_ecs::name::Name": "p",
                "bevy_transform::components::transform::Transform": {
                    "translation": [0.0, 0.0, 0.0],
                    "rotation": [0.0, 0.0, 0.0, 1.0],
                    "scale": [1.0, 1.0, 1.0]
                },
                "bevy_camera::visibility::Visibility": "Inherited"
            }
        }]
    });
    std::fs::write(
        &prefab_path,
        serde_json::to_string_pretty(&prefab_json).unwrap(),
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        Vec3::new(0.0, 0.0, 0.0),
    );

    // Find the instance entity.
    let instance_entity = {
        let world = app.world_mut();
        let mut q = world.query::<(bevy::prelude::Entity, &jackdaw::prefab::IsA)>();
        q.iter(world)
            .next()
            .map(|(e, _)| e)
            .expect("instance exists after spawn")
    };

    // Simulate the framework's before-snapshot install (reshuffles the
    // live AST). After this, the key the dispatch site would have
    // looked up may no longer point at the instance node.
    let _ = jackdaw::scene_io::build_snapshot_ast(app.world_mut());

    // Now look up the key fresh, using the Entity. This is what the
    // operator's body does internally. It must resolve to the instance.
    let key = app
        .world()
        .resource::<jackdaw_jsn::SceneJsnAst>()
        .key_for_entity(instance_entity);
    let Some(key) = key else {
        panic!("entity {instance_entity:?} not in post-install AST");
    };
    let has_isa = app
        .world()
        .resource::<jackdaw_jsn::SceneJsnAst>()
        .get_component_at(key, "jackdaw::prefab::components::IsA")
        .is_some();
    assert!(
        has_isa,
        "key resolved from entity post-install points at an IsA node"
    );

    // And the underlying unbundle works.
    jackdaw::prefab::operators::unbundle_instance(app.world_mut(), key);
    let world = app.world_mut();
    let mut isa_q = world.query::<&jackdaw::prefab::IsA>();
    assert_eq!(isa_q.iter(world).count(), 0, "instance removed by unbundle");
}

#[test]
fn apply_ast_with_override_entries_resolves_inherited_components() {
    // Snapshots captured via `build_snapshot_ast` reduce inherited
    // descendants to sparse override entries (just PrefabEntityId).
    // When such a snapshot is applied (e.g. on undo of unbundle), the
    // resolver must fill in the prefab baseline so the spawned ECS
    // entities have their Name / Transform / Brush data, not just an
    // empty `PrefabEntityId`.
    use bevy::math::Vec3;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");
    let prefab_json = serde_json::json!({
        "jsn": { "format_version": [3, 0, 0], "editor_version": "0", "bevy_version": "0.18" },
        "metadata": { "name": "", "description": "", "author": "", "created": "", "modified": "" },
        "assets": {},
        "editor": null,
        "scene": [
            {
                "components": {
                    "jackdaw::prefab::components::Prefab": null,
                    "jackdaw::prefab::components::PrefabEntityId": 0,
                    "bevy_ecs::name::Name": "p",
                    "bevy_transform::components::transform::Transform": {
                        "translation": [0.0, 0.0, 0.0],
                        "rotation": [0.0, 0.0, 0.0, 1.0],
                        "scale": [1.0, 1.0, 1.0]
                    },
                    "bevy_camera::visibility::Visibility": "Inherited"
                }
            },
            {
                "parent": 0,
                "components": {
                    "jackdaw::prefab::components::PrefabEntityId": 1,
                    "bevy_ecs::name::Name": "child",
                    "bevy_transform::components::transform::Transform": {
                        "translation": [5.0, 0.0, 0.0],
                        "rotation": [0.0, 0.0, 0.0, 1.0],
                        "scale": [1.0, 1.0, 1.0]
                    },
                    "bevy_camera::visibility::Visibility": "Inherited"
                }
            }
        ]
    });
    std::fs::write(
        &prefab_path,
        serde_json::to_string_pretty(&prefab_json).unwrap(),
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        Vec3::new(0.0, 0.0, 0.0),
    );

    // Capture a snapshot — this prefabifies the inherited brush into
    // an override entry with just PrefabEntityId.
    let snapshot_ast = jackdaw::scene_io::build_snapshot_ast(app.world_mut());

    // Confirm the snapshot does carry a sparse override (the test
    // is meaningful only if prefabify ran).
    let override_node = snapshot_ast.nodes.iter().find(|n| {
        n.components
            .contains_key("jackdaw::prefab::components::PrefabEntityId")
            && !n
                .components
                .contains_key("jackdaw::prefab::components::IsA")
    });
    assert!(
        override_node.is_some(),
        "snapshot contains a sparse override entry for the inherited child"
    );
    assert!(
        !override_node
            .unwrap()
            .components
            .contains_key("bevy_ecs::name::Name"),
        "sparse override omits the Name (matches prefab baseline)"
    );

    // Despawn everything, then re-apply the snapshot. This is the
    // path SnapshotDiff::undo takes.
    jackdaw::scene_io::apply_ast_to_world(app.world_mut(), &snapshot_ast);

    // Verify the inherited child is back in the world WITH its
    // inherited Name + Transform (resolved from the prefab cache).
    let world = app.world_mut();
    let mut q = world.query::<(
        &jackdaw::prefab::PrefabEntityId,
        &bevy::prelude::Name,
        &bevy::prelude::Transform,
    )>();
    let inherited: Vec<_> = q.iter(world).filter(|(id, _, _)| id.0 == 1).collect();
    assert_eq!(
        inherited.len(),
        1,
        "exactly one inherited descendant for PrefabEntityId(1)"
    );
    let (_, name, transform) = inherited[0];
    assert_eq!(
        name.as_str(),
        "child",
        "inherited Name resolved from prefab; got {name}"
    );
    assert!(
        (transform.translation.x - 5.0).abs() < 1e-4,
        "inherited Transform.x resolved from prefab (5.0); got {}",
        transform.translation.x
    );
}

#[test]
fn snapshot_round_trip_redo_brings_back_instance() {
    // Verify the redo path: capture before, spawn, capture after,
    // apply before (undo), apply after (redo). Redo must restore the
    // instance + inherited descendants with full components.
    use bevy::math::Vec3;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");
    let prefab_json = serde_json::json!({
        "jsn": { "format_version": [3, 0, 0], "editor_version": "0", "bevy_version": "0.18" },
        "metadata": { "name": "", "description": "", "author": "", "created": "", "modified": "" },
        "assets": {},
        "editor": null,
        "scene": [
            {
                "components": {
                    "jackdaw::prefab::components::Prefab": null,
                    "jackdaw::prefab::components::PrefabEntityId": 0,
                    "bevy_ecs::name::Name": "p",
                    "bevy_transform::components::transform::Transform": {
                        "translation": [0.0, 0.0, 0.0],
                        "rotation": [0.0, 0.0, 0.0, 1.0],
                        "scale": [1.0, 1.0, 1.0]
                    },
                    "bevy_camera::visibility::Visibility": "Inherited"
                }
            },
            {
                "parent": 0,
                "components": {
                    "jackdaw::prefab::components::PrefabEntityId": 1,
                    "bevy_ecs::name::Name": "child",
                    "bevy_transform::components::transform::Transform": {
                        "translation": [7.0, 0.0, 0.0],
                        "rotation": [0.0, 0.0, 0.0, 1.0],
                        "scale": [1.0, 1.0, 1.0]
                    },
                    "bevy_camera::visibility::Visibility": "Inherited"
                }
            }
        ]
    });
    std::fs::write(
        &prefab_path,
        serde_json::to_string_pretty(&prefab_json).unwrap(),
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();

    let before = jackdaw::scene_io::build_snapshot_ast(app.world_mut());

    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        Vec3::new(0.0, 0.0, 0.0),
    );

    let after = jackdaw::scene_io::build_snapshot_ast(app.world_mut());

    // Undo
    jackdaw::scene_io::apply_ast_to_world(app.world_mut(), &before);
    {
        let world = app.world_mut();
        let mut q = world.query::<&jackdaw::prefab::IsA>();
        assert_eq!(q.iter(world).count(), 0, "instance removed after undo");
    }

    // Redo
    jackdaw::scene_io::apply_ast_to_world(app.world_mut(), &after);
    {
        let world = app.world_mut();
        let mut q = world.query::<&jackdaw::prefab::IsA>();
        assert_eq!(q.iter(world).count(), 1, "instance restored after redo");
    }

    let world = app.world_mut();
    let mut q = world.query::<(
        &jackdaw::prefab::PrefabEntityId,
        &bevy::prelude::Name,
        &bevy::prelude::Transform,
    )>();
    let inherited: Vec<_> = q.iter(world).filter(|(id, _, _)| id.0 == 1).collect();
    assert_eq!(inherited.len(), 1, "inherited child restored after redo");
    let (_, name, transform) = inherited[0];
    assert_eq!(name.as_str(), "child");
    assert!((transform.translation.x - 7.0).abs() < 1e-4);
}

#[test]
fn typed_command_and_snapshot_diff_interleave_cleanly_on_undo() {
    // Verify that a typed EditorCommand (manual push_executed) and a
    // SnapshotDiff (framework-pushed) coexist on the same undo stack
    // and each Ctrl+Z peels off the right one.
    use bevy::math::Vec3;
    use jackdaw_commands::{CommandHistory, EditorCommand};

    struct Counter(std::sync::Arc<std::sync::atomic::AtomicU32>);
    impl EditorCommand for Counter {
        fn execute(&mut self, _world: &mut bevy::prelude::World) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        fn undo(&mut self, _world: &mut bevy::prelude::World) {
            self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
        fn description(&self) -> &str {
            "counter"
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");
    let prefab_json = serde_json::json!({
        "jsn": { "format_version": [3, 0, 0], "editor_version": "0", "bevy_version": "0.18" },
        "metadata": { "name": "", "description": "", "author": "", "created": "", "modified": "" },
        "assets": {},
        "editor": null,
        "scene": [{
            "components": {
                "jackdaw::prefab::components::Prefab": null,
                "jackdaw::prefab::components::PrefabEntityId": 0,
                "bevy_ecs::name::Name": "p",
                "bevy_transform::components::transform::Transform": {
                    "translation": [0.0, 0.0, 0.0],
                    "rotation": [0.0, 0.0, 0.0, 1.0],
                    "scale": [1.0, 1.0, 1.0]
                },
                "bevy_camera::visibility::Visibility": "Inherited"
            }
        }]
    });
    std::fs::write(
        &prefab_path,
        serde_json::to_string_pretty(&prefab_json).unwrap(),
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));

    // Sequence:
    //   1) Counter to 1 (typed push_executed-only)
    //   2) Capture before-snapshot
    //   3) Spawn instance
    //   4) Capture after-snapshot, push SnapshotDiff
    // Stack: [Counter, SnapshotDiff]
    //
    // Then:
    //   Ctrl+Z → pop SnapshotDiff → instance removed, counter still 1
    //   Ctrl+Z → pop Counter → counter back to 0

    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    app.world_mut()
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(Counter(counter.clone())));

    // Initialize editor-state resources the snapshotter expects.
    app.init_resource::<jackdaw::brush::EditMode>();
    app.init_resource::<jackdaw::gizmos::GizmoMode>();
    app.init_resource::<jackdaw::gizmos::GizmoSpace>();
    app.init_resource::<jackdaw::snapping::SnapSettings>();
    app.init_resource::<jackdaw::view_modes::ViewModeSettings>();
    app.init_resource::<jackdaw::viewport_overlays::OverlaySettings>();
    app.init_resource::<jackdaw_avian_integration::PhysicsOverlayConfig>();
    app.init_resource::<jackdaw::viewport_select::GroupEditState>();
    app.world_mut()
        .insert_resource(jackdaw_api_internal::snapshot::ActiveSnapshotter(Box::new(
            jackdaw::undo_snapshot::JsnAstSnapshotter,
        )));

    use bevy::prelude::Mut;
    use jackdaw_api_internal::snapshot::{ActiveSnapshotter, SceneSnapshot};

    let before: Box<dyn SceneSnapshot> = app
        .world_mut()
        .resource_scope(|world, snapshotter: Mut<ActiveSnapshotter>| snapshotter.0.capture(world));
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        Vec3::new(0.0, 0.0, 0.0),
    );
    let after: Box<dyn SceneSnapshot> = app
        .world_mut()
        .resource_scope(|world, snapshotter: Mut<ActiveSnapshotter>| snapshotter.0.capture(world));

    struct SnapshotDiffTest {
        before: Box<dyn SceneSnapshot>,
        after: Box<dyn SceneSnapshot>,
    }
    impl EditorCommand for SnapshotDiffTest {
        fn execute(&mut self, world: &mut bevy::prelude::World) {
            self.after.apply(world);
        }
        fn undo(&mut self, world: &mut bevy::prelude::World) {
            self.before.apply(world);
        }
        fn description(&self) -> &str {
            "snapshot"
        }
    }
    app.world_mut()
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(SnapshotDiffTest { before, after }));

    assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
    {
        let world = app.world_mut();
        let mut q = world.query::<&jackdaw::prefab::IsA>();
        assert_eq!(q.iter(world).count(), 1, "instance present pre-undo");
    }

    // First undo: pops SnapshotDiff → instance removed, counter unchanged
    app.world_mut()
        .resource_scope(|world, mut history: bevy::prelude::Mut<CommandHistory>| {
            history.undo(world);
        });
    assert_eq!(
        counter.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "counter unchanged after first undo (typed cmd not yet popped)"
    );
    {
        let world = app.world_mut();
        let mut q = world.query::<&jackdaw::prefab::IsA>();
        assert_eq!(q.iter(world).count(), 0, "instance removed by first undo");
    }

    // Second undo: pops Counter → counter back to 0
    app.world_mut()
        .resource_scope(|world, mut history: bevy::prelude::Mut<CommandHistory>| {
            history.undo(world);
        });
    assert_eq!(
        counter.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "counter reverted by second undo"
    );
}
