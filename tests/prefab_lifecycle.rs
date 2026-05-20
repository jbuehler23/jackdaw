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

    jackdaw::prefab::operators::save_as_prefab(app.world_mut(), entity, &prefab_target);

    assert!(prefab_target.exists(), "prefab file written");
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&prefab_target).unwrap())
            .expect("prefab file is valid JSON");
    assert!(
        written["scene"][0]["components"]
            .get("jackdaw::prefab::components::Prefab")
            .is_some(),
        "prefab root has Prefab marker; got {written:?}"
    );
    assert!(
        written["scene"][0]["components"]
            .get("jackdaw::prefab::components::PrefabEntityId")
            .is_some(),
        "prefab root has PrefabEntityId(0)"
    );

    // After conversion, the source entity's AST node has IsA.
    let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
    let has_isa = ast
        .entities_with_component("jackdaw::prefab::components::IsA")
        .next()
        .is_some();
    assert!(has_isa, "source scene entity converted to instance");

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
    // Single-root path: parent is index 0 with Prefab marker, child is index 1.
    assert_eq!(scene.len(), 2, "parent + child only, no synthetic root");
    assert!(
        scene[0]["components"]
            .get("jackdaw::prefab::components::Prefab")
            .is_some(),
        "single-root path tags the actual root with Prefab"
    );
}

#[test]
fn save_as_prefab_from_selection_one_root_matches_single_save() {
    // Regression: selection of size 1 must behave exactly like
    // save_as_prefab.
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("solo.jsn");

    let mut app = make_app_for_prefab_tests();
    let solo = app.world_mut().spawn(bevy::prelude::Name::new("solo")).id();

    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &[solo], &target);

    let ast = app.world().resource::<jackdaw_jsn::SceneJsnAst>();
    // Single-root path mutates the source AST to add IsA.
    assert!(
        ast.entities_with_component("jackdaw::prefab::components::IsA")
            .next()
            .is_some(),
        "single-root flow converted source to IsA instance"
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
