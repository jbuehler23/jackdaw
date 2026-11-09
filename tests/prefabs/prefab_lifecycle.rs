use crate::util;

use bevy::prelude::*;
use path_slash::PathExt as _;

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
    let path = PathBuf::from("assets/prefabs/rock.bsn");
    let ast = jackdaw_bsn::SceneBsnAst::default();

    assert!(cache.get(&path).is_none());
    cache.insert(path.clone(), ast);
    assert!(cache.get(&path).is_some());

    cache.invalidate(&path);
    assert!(cache.get(&path).is_none());
}

#[test]
fn cycle_detector_accepts_simple_chain() {
    use std::path::PathBuf;
    let chain = vec![
        PathBuf::from("scene.bsn"),
        PathBuf::from("a.bsn"),
        PathBuf::from("b.bsn"),
    ];
    let next = PathBuf::from("c.bsn");
    assert!(jackdaw::prefab::resolver_bsn::would_cycle(&chain, &next).is_none());
}

#[test]
fn cycle_detector_rejects_revisit() {
    use std::path::PathBuf;
    let chain = vec![PathBuf::from("a.bsn"), PathBuf::from("b.bsn")];
    let next = PathBuf::from("a.bsn");
    let err = jackdaw::prefab::resolver_bsn::would_cycle(&chain, &next)
        .expect("cycle should be reported");
    assert!(err.to_string().contains("a.bsn"));
}

const PREFAB_TYPE: &str = "jackdaw::prefab::components::Prefab";
const PEID_TYPE: &str = "jackdaw::prefab::components::PrefabEntityId";
const ISA_TYPE: &str = "jackdaw::prefab::components::IsA";
const TRANSFORM_TYPE: &str = "bevy_transform::components::transform::Transform";

fn peid_patch(id: i128) -> jackdaw_bsn::BsnPatch {
    jackdaw_bsn::BsnPatch::TupleStruct(jackdaw_bsn::BsnTupleStructData {
        type_path: PEID_TYPE.to_string(),
        values: vec![jackdaw_bsn::BsnValue::Int(id)],
    })
}

fn isa_patch(source: &str) -> jackdaw_bsn::BsnPatch {
    jackdaw_bsn::BsnPatch::Struct(jackdaw_bsn::BsnStructData {
        type_path: ISA_TYPE.to_string(),
        fields: jackdaw_bsn::BsnStructFields(vec![
            jackdaw_bsn::BsnField {
                name: "source".to_string(),
                value: jackdaw_bsn::BsnValue::String(source.to_string()),
            },
            jackdaw_bsn::BsnField {
                name: "deleted".to_string(),
                value: jackdaw_bsn::BsnValue::List(Vec::new()),
            },
        ]),
    })
}

/// Parse `.bsn` text into a document.
fn bsn_ast(text: &str) -> jackdaw_bsn::SceneBsnAst {
    jackdaw_bsn::parse_bsn_text(text).expect("prefab BSN parses")
}

/// Convert an inline JSON prefab fixture to BSN text and write it to `path`.
/// The conversion needs the app's type registry, so the app must exist first.
fn write_bsn_prefab(app: &mut bevy::prelude::App, path: &std::path::Path, jsn: &str) {
    let converted = jackdaw::jsn_to_bsn::convert_jsn_text(app.world_mut(), jsn)
        .expect("convert prefab fixture to bsn");
    std::fs::write(path, converted.scene_bsn).expect("write .bsn fixture");
}

/// Register an ECS entity as a root node of the live BSN document. Prefab
/// operators package selection data off the document node.
fn register_live_root(
    app: &mut bevy::prelude::App,
    entity: bevy::prelude::Entity,
    patches: Vec<jackdaw_bsn::BsnPatch>,
) -> bevy::prelude::Entity {
    let mut ast = app.world_mut().resource_mut::<jackdaw_bsn::SceneBsnAst>();
    let node = ast.create_entity_node(patches);
    ast.add_to_roots(node);
    ast.link(entity, node);
    node
}

fn vec3_value(x: f64, y: f64, z: f64) -> jackdaw_bsn::BsnValue {
    jackdaw_bsn::BsnValue::Struct(jackdaw_bsn::BsnStructData {
        type_path: "glam::Vec3".to_string(),
        fields: jackdaw_bsn::BsnStructFields(vec![
            jackdaw_bsn::BsnField {
                name: "x".to_string(),
                value: jackdaw_bsn::BsnValue::Float(x),
            },
            jackdaw_bsn::BsnField {
                name: "y".to_string(),
                value: jackdaw_bsn::BsnValue::Float(y),
            },
            jackdaw_bsn::BsnField {
                name: "z".to_string(),
                value: jackdaw_bsn::BsnValue::Float(z),
            },
        ]),
    })
}

/// A full `Transform` patch: given translation, identity rotation, unit scale.
fn transform_patch(x: f64, y: f64, z: f64) -> jackdaw_bsn::BsnPatch {
    jackdaw_bsn::BsnPatch::Struct(jackdaw_bsn::BsnStructData {
        type_path: TRANSFORM_TYPE.to_string(),
        fields: jackdaw_bsn::BsnStructFields(vec![
            jackdaw_bsn::BsnField {
                name: "translation".to_string(),
                value: vec3_value(x, y, z),
            },
            jackdaw_bsn::BsnField {
                name: "rotation".to_string(),
                value: jackdaw_bsn::BsnValue::Struct(jackdaw_bsn::BsnStructData {
                    type_path: "glam::Quat".to_string(),
                    fields: jackdaw_bsn::BsnStructFields(vec![
                        jackdaw_bsn::BsnField {
                            name: "x".to_string(),
                            value: jackdaw_bsn::BsnValue::Float(0.0),
                        },
                        jackdaw_bsn::BsnField {
                            name: "y".to_string(),
                            value: jackdaw_bsn::BsnValue::Float(0.0),
                        },
                        jackdaw_bsn::BsnField {
                            name: "z".to_string(),
                            value: jackdaw_bsn::BsnValue::Float(0.0),
                        },
                        jackdaw_bsn::BsnField {
                            name: "w".to_string(),
                            value: jackdaw_bsn::BsnValue::Float(1.0),
                        },
                    ]),
                }),
            },
            jackdaw_bsn::BsnField {
                name: "scale".to_string(),
                value: vec3_value(1.0, 1.0, 1.0),
            },
        ]),
    })
}

/// The document node for the sole `IsA` instance. Re-fetch after any operator:
/// a reload rebuilds the document with fresh node ids.
fn sole_isa_node(app: &bevy::prelude::App) -> bevy::prelude::Entity {
    app.world()
        .resource::<jackdaw_bsn::SceneBsnAst>()
        .entities_with_component(ISA_TYPE)
        .first()
        .copied()
        .expect("one IsA instance node in the live BSN document")
}

/// The whole-component `BsnValue` on a live-document node, if present.
fn live_component(
    app: &bevy::prelude::App,
    node: bevy::prelude::Entity,
    type_path: &str,
) -> Option<jackdaw_bsn::BsnValue> {
    let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    jackdaw_bsn::get_bsn_field(ast, node, type_path, "")
}

/// A scalar field of a live-document component read as `f64`.
fn live_field_f64(
    app: &bevy::prelude::App,
    node: bevy::prelude::Entity,
    type_path: &str,
    field_path: &str,
) -> Option<f64> {
    let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    match jackdaw_bsn::get_bsn_field(ast, node, type_path, field_path)? {
        jackdaw_bsn::BsnValue::Float(v) => Some(v),
        jackdaw_bsn::BsnValue::Int(v) => Some(v as f64),
        _ => None,
    }
}

#[test]
fn resolver_materializes_inherited_subtree() {
    use jackdaw_bsn::{BsnField, BsnPatch, BsnStructData, BsnStructFields, BsnValue, SceneBsnAst};
    use std::path::{Path, PathBuf};

    let mut prefab = SceneBsnAst::default();
    let root = prefab.create_entity_node(vec![BsnPatch::Type(PREFAB_TYPE.into()), peid_patch(0)]);
    let vec3 = BsnValue::Struct(BsnStructData {
        type_path: "glam::Vec3".to_string(),
        fields: BsnStructFields(vec![
            BsnField {
                name: "x".into(),
                value: BsnValue::Float(0.0),
            },
            BsnField {
                name: "y".into(),
                value: BsnValue::Float(1.0),
            },
            BsnField {
                name: "z".into(),
                value: BsnValue::Float(0.0),
            },
        ]),
    });
    let child = prefab.create_entity_node(vec![
        peid_patch(1),
        BsnPatch::Struct(BsnStructData {
            type_path: TRANSFORM_TYPE.into(),
            fields: BsnStructFields(vec![BsnField {
                name: "translation".into(),
                value: vec3,
            }]),
        }),
    ]);
    prefab.add_to_roots(root);
    prefab.add_child_to_ast(root, child);

    let mut scene = SceneBsnAst::default();
    let instance = scene.create_entity_node(vec![isa_patch("prefab.bsn")]);
    scene.add_to_roots(instance);

    let mut prefabs: std::collections::HashMap<PathBuf, SceneBsnAst> =
        std::collections::HashMap::new();
    prefabs.insert(PathBuf::from("prefab.bsn"), prefab);
    let get = |p: &Path| prefabs.get(p);

    let resolved =
        jackdaw::prefab::resolver_bsn::resolve_scene(&scene, &get).expect("resolution succeeds");
    let resolved_root = resolved.roots[0];
    assert_eq!(
        resolved.get_children_ast(resolved_root).len(),
        1,
        "instance has one inherited child after resolution"
    );
}

#[test]
fn resolver_rejects_isa_cycle() {
    use jackdaw_bsn::{BsnPatch, SceneBsnAst};
    use std::path::{Path, PathBuf};

    let mut prefab = SceneBsnAst::default();
    let root = prefab.create_entity_node(vec![
        BsnPatch::Type(PREFAB_TYPE.into()),
        peid_patch(0),
        isa_patch("self.bsn"),
    ]);
    prefab.add_to_roots(root);

    let mut scene = SceneBsnAst::default();
    let inst = scene.create_entity_node(vec![isa_patch("self.bsn")]);
    scene.add_to_roots(inst);

    let mut prefabs: std::collections::HashMap<PathBuf, SceneBsnAst> =
        std::collections::HashMap::new();
    prefabs.insert(PathBuf::from("self.bsn"), prefab);
    let get = |p: &Path| prefabs.get(p);

    let err = jackdaw::prefab::resolver_bsn::resolve_scene(&scene, &get);
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
                render_creation: RenderCreation::Automatic(Box::new(WgpuSettings {
                    backends: None,
                    ..default()
                })),
                ..default()
            })
            .disable::<WinitPlugin>(),
    );
    app.add_plugins(jackdaw_scene_types::SceneTypesPlugin::default());
    app.add_plugins(jackdaw_bsn::JackdawBsnPlugin);
    app.add_plugins(jackdaw::prefab::PrefabPlugin);
    app.init_resource::<jackdaw::commands::CommandHistory>();
    app.init_resource::<jackdaw::scene_io::SceneFilePath>();
    app.init_resource::<jackdaw::selection::Selection>();
    app
}

#[test]
fn load_resolves_isa_and_caches_prefab() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("rock.bsn");
    let scene_path = tmp.path().join("level.jsn");

    let mut app = make_app_for_prefab_tests();

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
    write_bsn_prefab(&mut app, &prefab_path, prefab_jsn);

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
        prefab_path.to_slash_lossy()
    );
    std::fs::write(&scene_path, scene_jsn).unwrap();

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
    let prefab_path = tmp.path().join("cluster.bsn");
    let scene_path = tmp.path().join("level.jsn");

    let mut app = make_app_for_prefab_tests();

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
    write_bsn_prefab(&mut app, &prefab_path, prefab_jsn);

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
        prefab_path.to_slash_lossy()
    );
    std::fs::write(&scene_path, scene_jsn).unwrap();

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

/// The cache reads `.bsn` only, so opening a legacy scene has to convert its
/// legacy prefab dependency before resolving, or the instance spawns bare.
#[test]
fn a_legacy_scene_resolves_a_legacy_prefab_it_inherits_from() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("cluster.jsn");
    let scene_path = tmp.path().join("level.jsn");

    let mut app = make_app_for_prefab_tests();

    std::fs::write(
        &prefab_path,
        r#"{
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
    }"#,
    )
    .unwrap();

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
        prefab_path.to_slash_lossy()
    );
    std::fs::write(&scene_path, scene_jsn).unwrap();

    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene_path);

    let mut name_q = app.world_mut().query::<&bevy::prelude::Name>();
    let names: Vec<String> = name_q
        .iter(app.world())
        .map(|n| n.as_str().to_string())
        .collect();
    assert!(
        names.iter().any(|n| n == "inherited_rock"),
        "the legacy prefab converted before the resolve, so its tree is here: {names:?}"
    );
}

#[test]
fn save_writes_sparse_deltas_only() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.jsn");
    let scene_path = tmp.path().join("s.jsn");

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
        prefab_path.to_slash_lossy()
    );
    std::fs::write(&scene_path, scene_jsn).unwrap();

    let mut app = make_app_for_prefab_tests();
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene_path);
    let bsn_scene_path = scene_path.with_extension("bsn");
    assert!(bsn_scene_path.exists(), "scene converted to .bsn on open");
    assert!(
        prefab_path.with_extension("bsn").exists(),
        "prefab dependency converted to .bsn on open"
    );
    assert_eq!(
        app.world()
            .resource::<jackdaw::scene_io::SceneFilePath>()
            .path
            .as_deref(),
        Some(bsn_scene_path.to_string_lossy().as_ref()),
        "the open tracks the converted path"
    );

    jackdaw::scene_io::save_scene(app.world_mut());

    // save_scene spawns a task pool job; give it a tick to land.
    std::thread::sleep(std::time::Duration::from_millis(200));

    let written = std::fs::read_to_string(&bsn_scene_path).expect("file exists");
    let doc = bsn_ast(&written);

    let instance = doc
        .entities_with_component(ISA_TYPE)
        .first()
        .copied()
        .expect("instance entity present on disk");
    let transform = "bevy_transform::components::transform::Transform";
    assert!(
        jackdaw_bsn::get_bsn_field(&doc, instance, transform, "translation").is_some(),
        "sparse delta keeps translation"
    );
    assert!(
        jackdaw_bsn::get_bsn_field(&doc, instance, transform, "rotation").is_none(),
        "sparse delta drops rotation"
    );
    assert!(
        jackdaw_bsn::get_bsn_field(&doc, instance, transform, "scale").is_none(),
        "sparse delta drops scale"
    );
}

#[test]
fn save_as_prefab_writes_file_and_converts_in_place() {
    use bevy::prelude::*;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_target = tmp.path().join("brush_prefab.bsn");

    let mut app = make_app_for_prefab_tests();

    let entity = app.world_mut().spawn(Name::new("test_entity")).id();
    register_live_root(
        &mut app,
        entity,
        vec![jackdaw_bsn::BsnPatch::Name("test_entity".to_string())],
    );

    jackdaw::prefab::operators::save_as_prefab_from_selection(
        app.world_mut(),
        &[entity],
        &prefab_target,
    );

    assert!(prefab_target.exists(), "prefab file written");
    let written = bsn_ast(&std::fs::read_to_string(&prefab_target).unwrap());
    let root = written.roots[0];
    assert!(
        written.find_patch_by_type_path(root, PREFAB_TYPE).is_some(),
        "synthetic root has Prefab marker"
    );
    assert!(
        written.find_patch_by_type_path(root, PEID_TYPE).is_some(),
        "synthetic root has PrefabEntityId(0)"
    );

    let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    let has_isa = !ast.entities_with_component(ISA_TYPE).is_empty();
    assert!(has_isa, "new instance node carrying IsA inserted");

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
        prefab.to_slash_lossy()
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
    let prefab_path = tmp.path().join("p.bsn");
    let scene_path = tmp.path().join("s.jsn");

    let mut app = make_app_for_prefab_tests();
    app.add_plugins(jackdaw::prefab::watcher::PrefabWatcherPlugin);
    write_bsn_prefab(&mut app, &prefab_path, &prefab_with_name("v1"));
    std::fs::write(&scene_path, scene_referencing(&prefab_path)).unwrap();

    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene_path);

    let initial = current_names(&mut app);
    assert!(
        initial.iter().any(|n| n == "v1"),
        "initial load sees v1; got {initial:?}"
    );

    write_bsn_prefab(&mut app, &prefab_path, &prefab_with_name("v2"));

    // Filesystem event latency varies by OS; generous deadline to avoid flakes.
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
fn a_prefab_is_not_reloaded_when_its_watch_starts() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");
    let scene_path = tmp.path().join("s.jsn");

    let mut app = make_app_for_prefab_tests();
    app.add_plugins(jackdaw::prefab::watcher::PrefabWatcherPlugin);
    write_bsn_prefab(&mut app, &prefab_path, &prefab_with_name("v1"));
    std::fs::write(&scene_path, scene_referencing(&prefab_path)).unwrap();

    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene_path);
    app.update();

    let entity_named = |app: &mut App| {
        let mut names = app.world_mut().query::<(Entity, &Name)>();
        names
            .iter(app.world())
            .find(|(_, name)| name.as_str() == "v1")
            .map(|(entity, _)| entity)
    };
    let before = entity_named(&mut app).expect("the instance spawned");

    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(800);
    while std::time::Instant::now() < deadline {
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(25));
    }

    assert_eq!(
        entity_named(&mut app),
        Some(before),
        "the watch starting is not a change; the instance keeps its entity"
    );
}

#[test]
fn spawn_instance_caches_and_spawns_inherited_entity() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("rock.bsn");

    let mut app = make_app_for_prefab_tests();
    write_bsn_prefab(
        &mut app,
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
    );

    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        bevy::math::Vec3::new(7.0, 0.0, 0.0),
    );

    let cache = app.world().resource::<jackdaw::prefab::PrefabAstCache>();
    assert!(cache.get(&prefab_path).is_some(), "prefab cached");

    let instance_key = sole_isa_node(&app);
    assert_eq!(
        live_field_f64(&app, instance_key, TRANSFORM_TYPE, "translation.x"),
        Some(7.0),
        "instance carries its placement Transform in the live document"
    );

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
    let prefab_path = tmp.path().join("p.bsn");

    let mut app = make_app_for_prefab_tests();
    write_bsn_prefab(
        &mut app,
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
    );

    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        bevy::math::Vec3::ZERO,
    );

    std::fs::write(&prefab_path, "{}").unwrap();

    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        bevy::math::Vec3::new(2.0, 0.0, 0.0),
    );

    let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    let instance_count = ast.entities_with_component(ISA_TYPE).len();
    assert_eq!(instance_count, 2, "two instances landed in the document");
}

#[test]
fn field_is_overridden_detects_changed_field() {
    let mut cache = jackdaw::prefab::PrefabAstCache::default();
    cache.insert(
        std::path::PathBuf::from("p.bsn"),
        bsn_ast(
            "jackdaw::prefab::components::Prefab\n\
             jackdaw::prefab::components::PrefabEntityId(0)\n\
             bevy_transform::components::transform::Transform {\n\
                 translation: glam::Vec3 { x: 0.0, y: 0.0, z: 0.0 },\n\
                 rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },\n\
                 scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 },\n\
             }",
        ),
    );

    let scene = bsn_ast(
        "jackdaw::prefab::components::IsA { source: \"p.bsn\" }\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         bevy_transform::components::transform::Transform {\n\
             translation: glam::Vec3 { x: 10.0, y: 0.0, z: 0.0 },\n\
             rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },\n\
             scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 },\n\
         }",
    );
    let instance = scene.roots[0];

    assert!(jackdaw::prefab::overrides_bsn::field_is_overridden(
        &scene,
        &cache,
        instance,
        "bevy_transform::components::transform::Transform",
        None,
        &bevy::reflect::TypeRegistry::default(),
    ));
    assert!(jackdaw::prefab::overrides_bsn::field_is_overridden(
        &scene,
        &cache,
        instance,
        "bevy_transform::components::transform::Transform",
        Some("translation"),
        &bevy::reflect::TypeRegistry::default(),
    ));
    assert!(!jackdaw::prefab::overrides_bsn::field_is_overridden(
        &scene,
        &cache,
        instance,
        "bevy_transform::components::transform::Transform",
        Some("rotation"),
        &bevy::reflect::TypeRegistry::default(),
    ));
}

#[test]
fn field_is_overridden_returns_false_outside_isa_subtree() {
    let cache = jackdaw::prefab::PrefabAstCache::default();
    let scene = bsn_ast(
        "bevy_transform::components::transform::Transform {\n\
             translation: glam::Vec3 { x: 3.0, y: 0.0, z: 0.0 },\n\
         }",
    );
    let entity = scene.roots[0];
    assert!(!jackdaw::prefab::overrides_bsn::field_is_overridden(
        &scene,
        &cache,
        entity,
        "bevy_transform::components::transform::Transform",
        None,
        &bevy::reflect::TypeRegistry::default(),
    ));
}

#[test]
fn revert_field_snaps_value_back_to_prefab() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");
    std::fs::write(
        &prefab_path,
        "jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         bevy_transform::components::transform::Transform {\n\
             translation: glam::Vec3 { x: 0.0, y: 0.0, z: 0.0 },\n\
             rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },\n\
             scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 },\n\
         }",
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();

    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        bevy::math::Vec3::new(7.0, 0.0, 0.0),
    );

    let instance_key = sole_isa_node(&app);
    assert_eq!(
        live_field_f64(
            &app,
            instance_key,
            "bevy_transform::components::transform::Transform",
            "translation.x",
        ),
        Some(7.0),
    );

    jackdaw::prefab::operators::revert_field(
        app.world_mut(),
        instance_key,
        "bevy_transform::components::transform::Transform",
        "translation",
    );

    // The operator rebuilds the document on reload, so re-fetch the node.
    let instance_key = sole_isa_node(&app);
    assert_eq!(
        live_field_f64(
            &app,
            instance_key,
            "bevy_transform::components::transform::Transform",
            "translation.x",
        ),
        Some(0.0),
    );
}

#[test]
fn revert_component_preserves_instance_only_addition() {
    // `revert_component` only reverts to a prefab-provided value: dropping an
    // instance-only addition would erase authored data with no recovery path.
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");
    std::fs::write(
        &prefab_path,
        "jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)",
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();

    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        bevy::math::Vec3::ZERO,
    );

    let instance_key = sole_isa_node(&app);

    {
        let mut ast = app.world_mut().resource_mut::<jackdaw_bsn::SceneBsnAst>();
        let patch = ast
            .world
            .spawn(jackdaw_bsn::BsnPatch::TupleStruct(
                jackdaw_bsn::BsnTupleStructData {
                    type_path: "bevy_ecs::name::Name".to_string(),
                    values: vec![jackdaw_bsn::BsnValue::String("instance_only".to_string())],
                },
            ))
            .id();
        ast.get_patches_mut(instance_key).unwrap().0.push(patch);
    }

    jackdaw::prefab::operators::revert_component(
        app.world_mut(),
        instance_key,
        "bevy_ecs::name::Name",
    );

    let name = live_component(&app, instance_key, "bevy_ecs::name::Name");
    let preserved = matches!(
        name,
        Some(jackdaw_bsn::BsnValue::TupleStruct(ref data))
            if matches!(data.values.first(), Some(jackdaw_bsn::BsnValue::String(s)) if s == "instance_only")
    );
    assert!(
        preserved,
        "instance-only addition is preserved when prefab has no value to revert to"
    );
}

#[test]
fn save_as_variant_writes_prefab_with_isa_and_overrides() {
    let tmp = tempfile::tempdir().unwrap();
    let base_path = tmp.path().join("base.bsn");
    let variant_path = tmp.path().join("variant.jsn");

    let mut app = make_app_for_prefab_tests();
    write_bsn_prefab(
        &mut app,
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
    );

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

    // Prefabs persist as BSN text: a `.jsn` target redirects to its `.bsn`
    // sibling.
    let written_path = variant_path.with_extension("bsn");
    assert!(written_path.exists(), "variant file written as .bsn");
    assert!(
        !variant_path.exists(),
        "no legacy .jsn variant file is written"
    );
    let written = bsn_ast(&std::fs::read_to_string(&written_path).unwrap());
    let root = written.roots[0];
    assert!(
        written.find_patch_by_type_path(root, PREFAB_TYPE).is_some(),
        "variant root has Prefab"
    );
    assert!(
        written.find_patch_by_type_path(root, ISA_TYPE).is_some(),
        "variant root has IsA pointing at base"
    );

    let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    let instance_node = ast
        .entities_with_component(ISA_TYPE)
        .first()
        .copied()
        .expect("instance still in the live document");
    let source = jackdaw_bsn::get_bsn_field(ast, instance_node, ISA_TYPE, "source");
    let rewired = matches!(
        source,
        Some(jackdaw_bsn::BsnValue::String(ref s))
            if s == written_path.to_string_lossy().as_ref()
    );
    assert!(rewired, "instance rewired to variant");
}

#[test]
fn bulk_apply_in_scene_copies_delta_to_all_matching_instances() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");
    std::fs::write(
        &prefab_path,
        "#p\n\
         jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)",
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();

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
        jackdaw_bsn::BsnValue::Struct(jackdaw_bsn::BsnStructData {
            type_path: "glam::Quat".to_string(),
            fields: jackdaw_bsn::BsnStructFields(vec![
                jackdaw_bsn::BsnField {
                    name: "x".to_string(),
                    value: jackdaw_bsn::BsnValue::Float(0.0),
                },
                jackdaw_bsn::BsnField {
                    name: "y".to_string(),
                    value: jackdaw_bsn::BsnValue::Float(std::f64::consts::FRAC_1_SQRT_2),
                },
                jackdaw_bsn::BsnField {
                    name: "z".to_string(),
                    value: jackdaw_bsn::BsnValue::Float(0.0),
                },
                jackdaw_bsn::BsnField {
                    name: "w".to_string(),
                    value: jackdaw_bsn::BsnValue::Float(std::f64::consts::FRAC_1_SQRT_2),
                },
            ]),
        }),
    );

    let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    let hits: usize = ast
        .entities_with_component(ISA_TYPE)
        .into_iter()
        .filter(|&k| jackdaw_bsn::get_bsn_field(ast, k, TRANSFORM_TYPE, "rotation").is_some())
        .count();
    assert_eq!(hits, 3, "all three instances got the rotation override");
}

#[test]
fn apply_to_prefab_source_writes_value_into_prefab_ast() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");
    std::fs::write(
        &prefab_path,
        "jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         bevy_transform::components::transform::Transform {\n\
             translation: glam::Vec3 { x: 0.0, y: 0.0, z: 0.0 },\n\
             rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },\n\
             scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 },\n\
         }",
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        bevy::math::Vec3::ZERO,
    );

    let instance_key = sole_isa_node(&app);

    jackdaw::prefab::operators::apply_to_prefab_source(
        app.world_mut(),
        instance_key,
        0,
        "bevy_transform::components::transform::Transform",
        "scale",
        jackdaw_bsn::BsnValue::Struct(jackdaw_bsn::BsnStructData {
            type_path: "glam::Vec3".to_string(),
            fields: jackdaw_bsn::BsnStructFields(vec![
                jackdaw_bsn::BsnField {
                    name: "x".to_string(),
                    value: jackdaw_bsn::BsnValue::Float(2.0),
                },
                jackdaw_bsn::BsnField {
                    name: "y".to_string(),
                    value: jackdaw_bsn::BsnValue::Float(2.0),
                },
                jackdaw_bsn::BsnField {
                    name: "z".to_string(),
                    value: jackdaw_bsn::BsnValue::Float(2.0),
                },
            ]),
        }),
    );

    let cache = app.world().resource::<jackdaw::prefab::PrefabAstCache>();
    let prefab_ast = cache.get(&prefab_path).expect("prefab cached");
    let root = prefab_ast
        .entities_with_component("jackdaw::prefab::components::Prefab")
        .first()
        .copied()
        .unwrap();
    let scale_x = jackdaw_bsn::get_bsn_field(
        prefab_ast,
        root,
        "bevy_transform::components::transform::Transform",
        "scale.x",
    );
    assert!(
        matches!(scale_x, Some(jackdaw_bsn::BsnValue::Float(v)) if (v - 2.0).abs() < 1e-9),
        "cache reflects applied value"
    );
}

const BRUSH: &str = "jackdaw_scene_types::types::Brush";

/// A prefab whose brush face holds `uv_rotation` as `face` spells it, with one
/// spawned instance.
fn brush_instance_app(
    tmp: &std::path::Path,
    face: &str,
) -> (bevy::prelude::App, std::path::PathBuf) {
    let prefab_path = tmp.join("p.bsn");
    std::fs::write(
        &prefab_path,
        format!(
            "jackdaw::prefab::components::Prefab\n\
             jackdaw::prefab::components::PrefabEntityId(0)\n\
             {BRUSH} {{ faces: [{face}] }}\n"
        ),
    )
    .unwrap();
    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        bevy::math::Vec3::ZERO,
    );
    (app, prefab_path)
}

fn set_uv_rotation(app: &mut bevy::prelude::App, value: f64) {
    let node = sole_isa_node(app);
    let registry = app.world().resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let mut live = app.world_mut().resource_mut::<jackdaw_bsn::SceneBsnAst>();
    jackdaw_bsn::set_bsn_field(
        &mut live,
        node,
        BRUSH,
        "faces[0].uv_rotation",
        jackdaw_bsn::BsnValue::Float(value),
        &registry,
    );
}

fn overridden(app: &bevy::prelude::App) -> bool {
    let node = sole_isa_node(app);
    let live = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    let cache = app.world().resource::<jackdaw::prefab::PrefabAstCache>();
    jackdaw::prefab::overrides_bsn::field_is_overridden(
        live,
        cache,
        node,
        BRUSH,
        Some("faces[0].uv_rotation"),
        &app.world().resource::<AppTypeRegistry>().read(),
    )
}

#[test]
fn revert_field_restores_a_field_the_prefab_list_element_leaves_out() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut app, _) = brush_instance_app(tmp.path(), "jackdaw_geometry::BrushFaceData");
    set_uv_rotation(&mut app, 5.0);
    assert!(overridden(&app));

    let node = sole_isa_node(&app);
    assert!(jackdaw::prefab::operators::revert_field(
        app.world_mut(),
        node,
        BRUSH,
        "faces[0].uv_rotation",
    ));

    assert!(!overridden(&app));
    let node = sole_isa_node(&app);
    let registry = app.world().resource::<AppTypeRegistry>().clone();
    let held = jackdaw_bsn::held_bsn_field(
        app.world().resource::<jackdaw_bsn::SceneBsnAst>(),
        node,
        BRUSH,
        "faces[0].uv_rotation",
        &registry.read(),
    );
    assert_eq!(held, Some(jackdaw_bsn::BsnValue::Float(0.0)));
}

#[test]
fn a_field_applied_to_source_writes_through_a_list_element() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut app, prefab_path) = brush_instance_app(
        tmp.path(),
        "jackdaw_geometry::BrushFaceData { uv_rotation: 5.0 }",
    );
    let node = sole_isa_node(&app);
    jackdaw::prefab::operators::apply_to_prefab_source(
        app.world_mut(),
        node,
        0,
        BRUSH,
        "faces[0].uv_rotation",
        jackdaw_bsn::BsnValue::Float(0.0),
    );
    let cache = app.world().resource::<jackdaw::prefab::PrefabAstCache>();
    let prefab = cache.get(&prefab_path).expect("prefab in cache");
    let root = prefab
        .entities_with_component("jackdaw::prefab::components::Prefab")
        .first()
        .copied()
        .unwrap();
    let registry = app.world().resource::<AppTypeRegistry>().read();
    assert_eq!(
        jackdaw_bsn::held_bsn_field(prefab, root, BRUSH, "faces[0].uv_rotation", &registry),
        Some(jackdaw_bsn::BsnValue::Float(0.0)),
    );
}

#[test]
fn apply_to_source_updates_cache_without_disk_write() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");
    std::fs::write(
        &prefab_path,
        "jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         bevy_transform::components::transform::Transform {\n\
             translation: glam::Vec3 { x: 0.0, y: 0.0, z: 0.0 },\n\
             rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },\n\
             scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 },\n\
         }",
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

    let instance_key = sole_isa_node(&app);
    jackdaw::prefab::operators::apply_to_prefab_source(
        app.world_mut(),
        instance_key,
        0,
        "bevy_transform::components::transform::Transform",
        "translation",
        jackdaw_bsn::BsnValue::Struct(jackdaw_bsn::BsnStructData {
            type_path: "glam::Vec3".to_string(),
            fields: jackdaw_bsn::BsnStructFields(vec![
                jackdaw_bsn::BsnField {
                    name: "x".to_string(),
                    value: jackdaw_bsn::BsnValue::Float(5.0),
                },
                jackdaw_bsn::BsnField {
                    name: "y".to_string(),
                    value: jackdaw_bsn::BsnValue::Float(0.0),
                },
                jackdaw_bsn::BsnField {
                    name: "z".to_string(),
                    value: jackdaw_bsn::BsnValue::Float(0.0),
                },
            ]),
        }),
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
        .first()
        .copied()
        .unwrap();
    let tx_x = jackdaw_bsn::get_bsn_field(
        prefab_ast,
        root,
        "bevy_transform::components::transform::Transform",
        "translation.x",
    );
    assert!(
        matches!(tx_x, Some(jackdaw_bsn::BsnValue::Float(v)) if (v - 5.0).abs() < 1e-9),
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
    let prefab_path = tmp.path().join("p.bsn");
    let scene_path = tmp.path().join("s.jsn");
    std::fs::write(
        &prefab_path,
        "#root\n\
         jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         bevy_transform::components::transform::Transform {\n\
             translation: glam::Vec3 { x: 0.0, y: 0.0, z: 0.0 },\n\
             rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },\n\
             scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 },\n\
         }\n\
         bevy_ecs::hierarchy::Children [\n\
             #rock\n\
             jackdaw::prefab::components::PrefabEntityId(7)\n\
             bevy_transform::components::transform::Transform {\n\
                 translation: glam::Vec3 { x: 3.0, y: 0.0, z: 0.0 },\n\
                 rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },\n\
                 scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 },\n\
             }\n\
         ]",
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
            prefab_path.to_slash_lossy()
        ),
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene_path);

    let (instance_root_key, child_key) = {
        let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
        let instance_root_key = ast
            .entities_with_component(ISA_TYPE)
            .first()
            .copied()
            .unwrap();
        let child_key = ast
            .find_node_by_component_int(PEID_TYPE, 7)
            .expect("inherited child resolved");
        (instance_root_key, child_key)
    };

    let scene_root = {
        let mut ast = app.world_mut().resource_mut::<jackdaw_bsn::SceneBsnAst>();
        let node = ast.create_entity_node(Vec::new());
        ast.add_to_roots(node);
        node
    };

    jackdaw::prefab::operators::unpack_child(app.world_mut(), child_key, scene_root);

    let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    let deleted = jackdaw_bsn::get_bsn_field(ast, instance_root_key, ISA_TYPE, "deleted");
    let has_seven = matches!(
        deleted,
        Some(jackdaw_bsn::BsnValue::List(ref items))
            if items.iter().any(|v| matches!(v, jackdaw_bsn::BsnValue::Int(7)))
    );
    assert!(has_seven, "instance's IsA.deleted contains the unpacked id");

    let kids = ast.get_children_ast(scene_root);
    assert_eq!(kids.len(), 1, "one unpacked child under the drop target");
    let unpacked_x = jackdaw_bsn::get_bsn_field(ast, kids[0], TRANSFORM_TYPE, "translation.x");
    assert!(
        matches!(unpacked_x, Some(jackdaw_bsn::BsnValue::Float(v)) if (v - 3.0).abs() < 1e-6),
        "the unpacked entity keeps its inherited Transform under the drop target"
    );
    assert!(
        ast.find_patch_by_type_path(kids[0], PEID_TYPE).is_none(),
        "the unpacked entity is standalone (no PrefabEntityId)"
    );
}

#[test]
fn save_as_prefab_from_selection_packages_siblings_under_synthetic_root() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("cluster.bsn");

    let mut app = make_app_for_prefab_tests();
    let a = app.world_mut().spawn(bevy::prelude::Name::new("a")).id();
    let b = app.world_mut().spawn(bevy::prelude::Name::new("b")).id();
    register_live_root(
        &mut app,
        a,
        vec![jackdaw_bsn::BsnPatch::Name("a".to_string())],
    );
    register_live_root(
        &mut app,
        b,
        vec![jackdaw_bsn::BsnPatch::Name("b".to_string())],
    );

    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &[a, b], &target);

    let written = bsn_ast(&std::fs::read_to_string(&target).unwrap());
    assert_eq!(written.roots.len(), 1, "one synthetic root");
    let root = written.roots[0];
    assert!(
        written.find_patch_by_type_path(root, PREFAB_TYPE).is_some(),
        "the root is the synthetic prefab root"
    );
    assert_eq!(
        written.get_children_ast(root).len(),
        2,
        "synthetic root + 2 siblings"
    );
}

#[test]
fn save_as_prefab_from_selection_filters_descendants_of_selected_ancestors() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("nested.bsn");

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

    // Only document-tracked entities survive the bundle filter; ECS-only
    // children of brushes are deliberately left unregistered.
    {
        let mut ast = app.world_mut().resource_mut::<jackdaw_bsn::SceneBsnAst>();
        let parent_node =
            ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::Name("parent".to_string())]);
        ast.add_to_roots(parent_node);
        ast.link(parent, parent_node);
        let child_node =
            ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::Name("child".to_string())]);
        ast.add_child_to_ast(parent_node, child_node);
        ast.link(child, child_node);
    }

    jackdaw::prefab::operators::save_as_prefab_from_selection(
        app.world_mut(),
        &[parent, child],
        &target,
    );

    let written = bsn_ast(&std::fs::read_to_string(&target).unwrap());
    let root = written.roots[0];
    assert_eq!(
        1 + written.descendants_of(root).len(),
        3,
        "synthetic root + parent + child, no duplicate child"
    );
    assert!(
        written.find_patch_by_type_path(root, PREFAB_TYPE).is_some(),
        "synthetic root carries the Prefab marker"
    );
}

#[test]
fn save_as_prefab_from_selection_one_root_inserts_instance() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("solo.bsn");

    let mut app = make_app_for_prefab_tests();
    let solo = app.world_mut().spawn(bevy::prelude::Name::new("solo")).id();
    register_live_root(
        &mut app,
        solo,
        vec![jackdaw_bsn::BsnPatch::Name("solo".to_string())],
    );

    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &[solo], &target);

    let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    assert!(
        !ast.entities_with_component(ISA_TYPE).is_empty(),
        "single-root flow inserts a new instance node carrying IsA"
    );
}

#[test]
fn save_round_trip_preserves_prefab_markers() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");

    let mut app = make_app_for_prefab_tests();
    write_bsn_prefab(
        &mut app,
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
    );

    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        bevy::math::Vec3::ZERO,
    );

    // Capture as save_scene does: this is where `should_skip_component` runs.
    let text = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        std::path::Path::new(""),
    );
    assert!(
        text.contains("jackdaw::prefab::components::IsA"),
        "saved scene must preserve IsA on the instance; got {text}"
    );
}

#[test]
fn cache_canonicalizes_path_inputs() {
    let tmp = tempfile::tempdir().unwrap();
    let abs = tmp.path().join("p.bsn");
    std::fs::write(&abs, "{}").unwrap();

    let mut cache = jackdaw::prefab::PrefabAstCache::default();
    cache.insert(&abs, jackdaw_bsn::SceneBsnAst::default());

    let weird = abs.parent().unwrap().join(".").join("p.bsn");
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
        std::path::PathBuf::from("/tmp/jackdaw_cache_test_a.bsn"),
        jackdaw_bsn::SceneBsnAst::default(),
    );
    let after_insert = cache.epoch();
    assert!(after_insert > start, "insert bumps epoch");
    cache.invalidate(&std::path::PathBuf::from("/tmp/jackdaw_cache_test_a.bsn"));
    assert!(cache.epoch() > after_insert, "invalidate bumps epoch");
}

#[test]
fn editor_save_records_fingerprint() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");

    let mut app = make_app_for_prefab_tests();
    write_bsn_prefab(
        &mut app,
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
    );

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
    let prefab_path = tmp.path().join("p.bsn");
    std::fs::write(&prefab_path, "{}").unwrap();
    let fp_a = jackdaw::prefab::cache::compute_file_fingerprint(&prefab_path).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(&prefab_path, r#"{"changed":true}"#).unwrap();
    let fp_b = jackdaw::prefab::cache::compute_file_fingerprint(&prefab_path).unwrap();
    assert_ne!(fp_a, fp_b, "fingerprint changes when content changes");
}

#[test]
fn revert_component_operator_runs_through_dispatch() {
    use jackdaw_api::prelude::*;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");
    std::fs::write(
        &prefab_path,
        "#prefab_root\n\
         jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         bevy_transform::components::transform::Transform {\n\
             translation: glam::Vec3 { x: 0.0, y: 0.0, z: 0.0 },\n\
             rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },\n\
             scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 },\n\
         }",
    )
    .unwrap();

    let mut app = util::editor_test_app();

    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        bevy::math::Vec3::new(5.0, 0.0, 0.0),
    );
    let instance_key = sole_isa_node(&app);
    assert_eq!(
        live_field_f64(&app, instance_key, TRANSFORM_TYPE, "translation.x"),
        Some(5.0),
        "placement override present before the revert"
    );
    let instance_entity = app
        .world()
        .resource::<jackdaw_bsn::SceneBsnAst>()
        .ecs_for_ast(instance_key)
        .expect("instance ECS entity");

    let _ = app
        .world_mut()
        .operator("prefab.revert_component")
        .param("entity", instance_entity)
        .param("type_path", TRANSFORM_TYPE.to_string())
        .call()
        .expect("operator dispatch resolves");
    // The dispatcher queues commands through the world; flush them.
    app.update();

    let instance_key = sole_isa_node(&app);
    assert_eq!(
        live_field_f64(&app, instance_key, TRANSFORM_TYPE, "translation.x"),
        Some(0.0),
        "operator-driven revert restored the inherited prefab value",
    );
}

#[test]
fn save_as_prefab_strips_inherited_prefab_markers() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("fresh.bsn");

    let mut app = make_app_for_prefab_tests();
    let entity = app
        .world_mut()
        .spawn(bevy::prelude::Name::new("source"))
        .id();
    register_live_root(
        &mut app,
        entity,
        vec![
            jackdaw_bsn::BsnPatch::Name("source".to_string()),
            isa_patch("/tmp/some_other_prefab.bsn"),
        ],
    );

    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &[entity], &target);

    let written = bsn_ast(&std::fs::read_to_string(&target).unwrap());
    let root = written.roots[0];
    assert!(
        written.find_patch_by_type_path(root, PREFAB_TYPE).is_some(),
        "synthetic root has fresh Prefab marker"
    );
    let mut nodes = written.roots.clone();
    for &r in &written.roots {
        nodes.extend(written.descendants_of(r));
    }
    for node in nodes {
        assert!(
            written.find_patch_by_type_path(node, ISA_TYPE).is_none(),
            "no packaged entity may carry inherited IsA"
        );
    }
}

#[test]
fn save_as_prefab_does_not_bake_self_isa_into_file() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("box.bsn");

    let mut app = make_app_for_prefab_tests();
    let entity = app
        .world_mut()
        .spawn(bevy::prelude::Name::new("source"))
        .id();
    register_live_root(
        &mut app,
        entity,
        vec![
            jackdaw_bsn::BsnPatch::Name("source".to_string()),
            isa_patch(&target.to_string_lossy()),
        ],
    );

    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &[entity], &target);

    assert!(target.exists(), "always-wrap path writes the file");
    let written = bsn_ast(&std::fs::read_to_string(&target).unwrap());
    let mut nodes = written.roots.clone();
    for &r in &written.roots {
        nodes.extend(written.descendants_of(r));
    }
    for node in nodes {
        assert!(
            written.find_patch_by_type_path(node, ISA_TYPE).is_none(),
            "no entry in the written prefab carries a self-IsA"
        );
    }
}

#[test]
fn repair_self_cycles_strips_self_isa_from_cached_prefab() {
    let tmp = tempfile::tempdir().unwrap();
    // The cache entry is keyed at a legacy `.jsn` path; the repair's disk
    // write redirects to the `.bsn` sibling and backs up the old file.
    let path = tmp.path().join("poisoned.jsn");
    let poisoned = format!(
        "jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         jackdaw::prefab::components::IsA {{ source: \"{}\", deleted: [] }}\n\
         bevy_ecs::name::Name(\"poisoned\")",
        path.to_string_lossy()
    );
    std::fs::write(&path, "{}").unwrap();

    let mut app = make_app_for_prefab_tests();
    app.world_mut()
        .resource_mut::<jackdaw::prefab::PrefabAstCache>()
        .insert(&path, bsn_ast(&poisoned));

    jackdaw::prefab::operators::repair_self_cycles_system(app.world_mut());

    let cache = app.world().resource::<jackdaw::prefab::PrefabAstCache>();
    let repaired = cache.get(&path).expect("still cached");
    let root = repaired.roots[0];
    assert!(
        repaired.find_patch_by_type_path(root, ISA_TYPE).is_none(),
        "self-IsA was stripped"
    );
    assert!(
        repaired
            .find_patch_by_type_path(root, PREFAB_TYPE)
            .is_some(),
        "Prefab marker preserved"
    );

    let written_path = path.with_extension("bsn");
    let written = bsn_ast(&std::fs::read_to_string(&written_path).unwrap());
    let written_root = written.roots[0];
    assert!(
        written
            .find_patch_by_type_path(written_root, ISA_TYPE)
            .is_none(),
        "disk file also has IsA stripped"
    );
    assert!(
        path.with_extension("jsn.bak").exists(),
        "legacy .jsn file backed up as .jsn.bak"
    );
    assert!(
        !path.exists(),
        "legacy .jsn file no longer shadows the .bsn"
    );
}

#[test]
fn prefab_edit_propagates_to_instance_in_other_tab_on_swap() {
    use bevy::prelude::*;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("box.bsn");

    let mut app = make_app_for_prefab_tests();
    app.init_resource::<jackdaw::scenes::Scenes>();

    app.world_mut()
        .resource_mut::<jackdaw::prefab::PrefabAstCache>()
        .insert(
            &prefab_path,
            bsn_ast(
                "#initial_name\n\
                 jackdaw::prefab::components::Prefab\n\
                 jackdaw::prefab::components::PrefabEntityId(0)\n\
                 bevy_transform::components::transform::Transform {\n\
                     translation: glam::Vec3 { x: 0.0, y: 0.0, z: 0.0 },\n\
                     rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },\n\
                     scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 },\n\
                 }",
            ),
        );

    {
        let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
        let mut tab_a = jackdaw::scenes::SceneTab::new_untitled(1);
        let mut scene_ast = jackdaw_bsn::SceneBsnAst::default();
        let instance = scene_ast.create_entity_node(vec![
            isa_patch(&prefab_path.to_string_lossy()),
            peid_patch(0),
        ]);
        scene_ast.add_to_roots(instance);
        tab_a.content = jackdaw::scenes::TabContent::Scene(Some(Box::new(scene_ast)));
        scenes.tabs.push(tab_a);
        scenes.active = 0;

        let canonical = jackdaw::prefab::canonical_prefab_path(&prefab_path);
        let mut tab_b = jackdaw::scenes::SceneTab::new_untitled(2);
        tab_b.path = Some(prefab_path.clone());
        tab_b.kind = jackdaw::scenes::TabKind::Prefab;
        tab_b.content = jackdaw::scenes::TabContent::Prefab(canonical);
        scenes.tabs.push(tab_b);
    }

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

    jackdaw::scenes::swap::swap_active_tab(app.world_mut(), 1);

    {
        let mut cache = app
            .world_mut()
            .resource_mut::<jackdaw::prefab::PrefabAstCache>();
        cache.mutate(&prefab_path, |ast| {
            let root_key = ast
                .entities_with_component(PREFAB_TYPE)
                .first()
                .copied()
                .expect("prefab root exists");
            // The Name lives as a `#name` reference patch, not a component.
            let patches = ast
                .get_patches(root_key)
                .map(|p| p.0.clone())
                .unwrap_or_default();
            for pe in patches {
                if matches!(ast.get_patch(pe), Some(jackdaw_bsn::BsnPatch::Name(_))) {
                    ast.set_patch(
                        pe,
                        jackdaw_bsn::BsnPatch::Name("renamed_in_prefab".to_string()),
                    );
                }
            }
        });
        // Capture-on-swap flushes the live document into the cache, so mutate
        // both or the swap clobbers this rename.
        let mut live = app.world_mut().resource_mut::<jackdaw_bsn::SceneBsnAst>();
        let root_key = live.entities_with_component(PREFAB_TYPE).first().copied();
        if let Some(root_key) = root_key {
            let patches = live
                .get_patches(root_key)
                .map(|p| p.0.clone())
                .unwrap_or_default();
            for pe in patches {
                if matches!(live.get_patch(pe), Some(jackdaw_bsn::BsnPatch::Name(_))) {
                    live.set_patch(
                        pe,
                        jackdaw_bsn::BsnPatch::Name("renamed_in_prefab".to_string()),
                    );
                }
            }
        }
    }

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
    use bevy::prelude::*;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");

    let mut app = make_app_for_prefab_tests();
    app.init_resource::<jackdaw::scenes::Scenes>();

    app.world_mut()
        .resource_mut::<jackdaw::prefab::PrefabAstCache>()
        .insert(
            &prefab_path,
            bsn_ast(
                "#p\n\
                 jackdaw::prefab::components::Prefab\n\
                 jackdaw::prefab::components::PrefabEntityId(0)",
            ),
        );

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

    // An edit, so the status bar would otherwise show unsaved.
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
    }
    assert!(jackdaw::scene_io::is_scene_dirty(app.world()));

    jackdaw::scene_io::save_scene(app.world_mut());

    assert!(
        !app.world().resource::<jackdaw::scenes::Scenes>().tabs[0].dirty,
        "prefab tab.dirty must be cleared after save"
    );
    assert!(
        !jackdaw::scene_io::is_scene_dirty(app.world()),
        "the scene must report clean after save"
    );

    let _ = app
        .world_mut()
        .run_system_cached(jackdaw::scenes::track_active_dirty_from_history);
    assert!(
        !app.world().resource::<jackdaw::scenes::Scenes>().tabs[0].dirty,
        "track_active_dirty_from_history must not re-dirty the tab post-save"
    );

    // The cache-epoch driver clears the command history, which must not leave the
    // status bar stuck on unsaved.
    jackdaw::prefab::sync::drive_respawn_on_prefab_cache_change(app.world_mut());
    assert!(
        !jackdaw::scene_io::is_scene_dirty(app.world()),
        "after the respawn-on-cache-change driver fires, the scene must still report clean"
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
    app.init_resource::<jackdaw::selection::Selection>();

    {
        let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
        scenes.tabs.push(jackdaw::scenes::SceneTab::new_untitled(1));
        scenes.active = 0;
    }
    {
        let entity = app.world_mut().spawn(Name::new("source")).id();
        register_live_root(
            &mut app,
            entity,
            vec![jackdaw_bsn::BsnPatch::Name("source".to_string())],
        );
    }

    jackdaw::prefab::operators::save_scene_as_prefab(app.world_mut(), &target);

    // Prefabs persist as BSN text: the `.jsn` target redirects to its `.bsn`
    // sibling, and the tab tracks the written path.
    let written_path = target.with_extension("bsn");
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
    assert_eq!(tab.path.as_deref(), Some(written_path.as_path()));
    assert!(!tab.dirty, "tab cleared dirty flag after save");
    assert_eq!(tab.display_name, "box");

    assert!(written_path.exists(), "prefab file written as .bsn");
    assert!(!target.exists(), "no legacy .jsn prefab file is written");
    let written = bsn_ast(&std::fs::read_to_string(&written_path).unwrap());
    let root = written.roots[0];
    assert!(
        written.find_patch_by_type_path(root, PREFAB_TYPE).is_some(),
        "root has Prefab marker"
    );
    assert!(
        written.find_patch_by_type_path(root, PEID_TYPE).is_some(),
        "root has PrefabEntityId(0)"
    );
    assert!(
        written.find_patch_by_type_path(root, ISA_TYPE).is_none(),
        "root has NO IsA (this is the prefab definition, not an instance)"
    );

    let cache = app.world().resource::<jackdaw::prefab::PrefabAstCache>();
    assert!(cache.get(&written_path).is_some(), "new prefab cached");
}

#[test]
fn save_scene_as_prefab_with_multiple_roots_uses_synthetic_root() {
    use bevy::prelude::*;

    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("multi.bsn");

    let mut app = make_app_for_prefab_tests();
    app.init_resource::<jackdaw::scenes::Scenes>();
    app.init_resource::<jackdaw::commands::CommandHistory>();
    app.init_resource::<jackdaw::scene_io::SceneFilePath>();
    app.init_resource::<jackdaw::selection::Selection>();

    {
        let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
        scenes.tabs.push(jackdaw::scenes::SceneTab::new_untitled(1));
        scenes.active = 0;
    }
    {
        let a = app.world_mut().spawn(Name::new("a")).id();
        let b = app.world_mut().spawn(Name::new("b")).id();
        register_live_root(
            &mut app,
            a,
            vec![jackdaw_bsn::BsnPatch::Name("a".to_string())],
        );
        register_live_root(
            &mut app,
            b,
            vec![jackdaw_bsn::BsnPatch::Name("b".to_string())],
        );
    }

    jackdaw::prefab::operators::save_scene_as_prefab(app.world_mut(), &target);

    let written = bsn_ast(&std::fs::read_to_string(&target).unwrap());
    assert_eq!(written.roots.len(), 1, "one synthetic root wraps the scene");
    let root = written.roots[0];
    assert!(
        written.find_patch_by_type_path(root, PREFAB_TYPE).is_some(),
        "the root entry is the synthetic Prefab root"
    );
    assert_eq!(
        written.descendants_of(root).len(),
        2,
        "synthetic root carries the 2 former top-level roots"
    );
}

#[test]
fn save_as_prefab_from_selection_always_wraps_in_prefab_root() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("solo.bsn");

    let mut app = make_app_for_prefab_tests();
    let solo = app.world_mut().spawn(bevy::prelude::Name::new("solo")).id();
    register_live_root(
        &mut app,
        solo,
        vec![jackdaw_bsn::BsnPatch::Name("solo".to_string())],
    );

    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &[solo], &target);

    let written = bsn_ast(&std::fs::read_to_string(&target).unwrap());
    let root = written.roots[0];
    assert!(
        written.find_patch_by_type_path(root, PREFAB_TYPE).is_some(),
        "synthetic root carries the Prefab marker"
    );
    assert_eq!(
        written.get_name(root),
        Some("solo"),
        "synthetic root is named after the target file stem (solo.bsn -> 'solo')"
    );
    let children = written.get_children_ast(root);
    assert_eq!(children.len(), 1, "synthetic root + 1 child");
    assert_eq!(
        written.find_node_by_component_int(PEID_TYPE, 1),
        Some(children[0]),
        "child parented under synthetic root with PrefabEntityId(1)"
    );
}

#[test]
fn save_as_prefab_from_selection_replaces_source_with_instance() {
    // The selected entity is removed; the resolver materialises it back as an
    // inherited descendant on the next respawn.
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("box.bsn");

    let mut app = make_app_for_prefab_tests();
    let source = app
        .world_mut()
        .spawn(bevy::prelude::Name::new("source"))
        .id();
    register_live_root(
        &mut app,
        source,
        vec![jackdaw_bsn::BsnPatch::Name("source".to_string())],
    );

    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &[source], &target);

    let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    let isa_nodes = ast.entities_with_component(ISA_TYPE);
    assert_eq!(isa_nodes.len(), 1, "exactly one instance node was added");
    let instance_key = isa_nodes[0];
    assert!(
        ast.roots.contains(&instance_key),
        "instance is a top-level node"
    );
    for child in ast.get_children_ast(instance_key) {
        assert!(
            ast.find_patch_by_type_path(child, PEID_TYPE).is_some(),
            "children under the instance are inherited materialisations from the prefab, not authored nodes"
        );
    }
}

#[test]
fn unbundle_instance_promotes_inherited_children_to_authored() {
    let tmp = tempfile::tempdir().unwrap();
    // A `.bsn` target so the bundle write and the follow-up spawn_instance both
    // round-trip through the BSN prefab reader.
    let target = tmp.path().join("u.bsn");

    let mut app = make_app_for_prefab_tests();
    let source = app
        .world_mut()
        .spawn(bevy::prelude::Name::new("source"))
        .id();
    {
        let mut ast = app.world_mut().resource_mut::<jackdaw_bsn::SceneBsnAst>();
        let node = ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::Name("source".to_string())]);
        ast.add_to_roots(node);
        ast.link(source, node);
    }
    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &[source], &target);

    let instance_key = sole_isa_node(&app);

    jackdaw::prefab::operators::unbundle_instance(app.world_mut(), instance_key);

    let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    assert!(
        ast.entities_with_component(ISA_TYPE).is_empty(),
        "IsA stripped from former instance node"
    );

    let promoted = ast
        .roots
        .iter()
        .copied()
        .filter(|&n| {
            let has_name = ast
                .get_patches(n)
                .map(|p| {
                    p.0.iter().any(|&pe| {
                        matches!(ast.get_patch(pe), Some(jackdaw_bsn::BsnPatch::Name(_)))
                    })
                })
                .unwrap_or(false);
            has_name
                && ast.find_patch_by_type_path(n, ISA_TYPE).is_none()
                && ast.find_patch_by_type_path(n, PEID_TYPE).is_none()
        })
        .count();
    assert!(
        promoted >= 1,
        "at least one authored top-level node with no prefab markers exists after unbundle"
    );
}

#[test]
fn save_as_prefab_preserves_world_positions_of_selection() {
    // Saving a selection as a prefab must not move it: the instance sits at the
    // centroid and each child's local Transform is shifted by `-centroid`.
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("p.bsn");

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

    // Force GlobalTransform population so the centroid read takes the
    // GlobalTransform path rather than the Transform fallback.
    app.update();

    register_live_root(&mut app, e1, vec![transform_patch(2.0, 0.0, 0.0)]);
    register_live_root(&mut app, e2, vec![transform_patch(4.0, 0.0, 0.0)]);

    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &[e1, e2], &target);

    // Centroid of (2, 0, 0) and (4, 0, 0).
    let instance_key = sole_isa_node(&app);
    let instance_x = live_field_f64(&app, instance_key, TRANSFORM_TYPE, "translation.x")
        .expect("instance has a Transform translation");
    assert!(
        (instance_x - 3.0).abs() < 1e-4,
        "instance Transform.x is the centroid (3.0); got {instance_x}"
    );

    // Children are stored centroid-relative.
    let written = bsn_ast(&std::fs::read_to_string(&target).unwrap());
    let root = written.roots[0];
    let mut child_xs: Vec<f64> = written
        .get_children_ast(root)
        .into_iter()
        .map(|child| {
            match jackdaw_bsn::get_bsn_field(&written, child, TRANSFORM_TYPE, "translation.x") {
                Some(jackdaw_bsn::BsnValue::Float(v)) => v,
                _ => panic!("packaged child is missing Transform.translation.x"),
            }
        })
        .collect();
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
    // Bevy's hierarchy propagation needs Visibility on every entity in a render
    // parent chain, or the parent's GlobalTransform stays at identity.
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("v.bsn");

    let mut app = make_app_for_prefab_tests();
    let e = app.world_mut().spawn(bevy::prelude::Name::new("a")).id();
    register_live_root(
        &mut app,
        e,
        vec![jackdaw_bsn::BsnPatch::Name("a".to_string())],
    );

    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &[e], &target);

    let written = bsn_ast(&std::fs::read_to_string(&target).unwrap());
    let root = written.roots[0];
    assert!(
        written
            .find_patch_by_type_path(root, "bevy_camera::visibility::Visibility")
            .is_some(),
        "synthetic PrefabRoot carries Visibility for hierarchy propagation"
    );

    // The instance inherits Visibility from the prefab's synthetic root, so the
    // local node only carries the sparse delta (IsA + placement Transform).
    let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    assert!(
        !ast.entities_with_component(ISA_TYPE).is_empty(),
        "instance node with IsA exists in the live document after save"
    );
}

#[test]
fn load_scene_from_jsn_backfills_transform_require_chain() {
    use jackdaw_jsn::format::JsnEntity;
    use std::collections::HashMap;
    use std::path::Path;

    let mut app = make_app_for_prefab_tests();
    let entity = JsnEntity {
        id: None,
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
        id: None,
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
    // Brushes have ECS-only children (face overlays, clip previews) the brush
    // spawn pipeline re-derives; bundling them would orphan them after respawn.
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("derived.bsn");

    let mut app = make_app_for_prefab_tests();
    let brush = app
        .world_mut()
        .spawn(bevy::prelude::Name::new("brush"))
        .id();
    // Unregistered ECS child, emulating a brush face overlay.
    let _derived = app
        .world_mut()
        .spawn((
            bevy::prelude::Name::new("derived-overlay"),
            bevy::ecs::hierarchy::ChildOf(brush),
        ))
        .id();
    register_live_root(
        &mut app,
        brush,
        vec![jackdaw_bsn::BsnPatch::Name("brush".to_string())],
    );

    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &[brush], &target);

    let written = bsn_ast(&std::fs::read_to_string(&target).unwrap());
    let root = written.roots[0];
    assert_eq!(
        1 + written.descendants_of(root).len(),
        2,
        "synthetic root + brush only; ECS-only derived child must be filtered out"
    );
}

#[test]
fn save_then_respawn_keeps_isa_on_instance_entity() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("p_test.bsn");

    let mut app = make_app_for_prefab_tests();
    let brush = app
        .world_mut()
        .spawn((
            bevy::prelude::Name::new("Brush"),
            bevy::prelude::Transform::from_xyz(1.0, 0.0, 0.0),
            bevy::camera::visibility::Visibility::Inherited,
        ))
        .id();
    register_live_root(&mut app, brush, vec![transform_patch(1.0, 0.0, 0.0)]);
    app.update();

    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &[brush], &target);
    jackdaw::prefab::watcher::reload_all_instances(app.world_mut());

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
    use bevy::math::Vec3;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");

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

    let mut app = make_app_for_prefab_tests();
    write_bsn_prefab(
        &mut app,
        &prefab_path,
        &serde_json::to_string(&prefab_json).unwrap(),
    );

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
    use bevy::math::Vec3;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");

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

    let mut app = make_app_for_prefab_tests();
    write_bsn_prefab(
        &mut app,
        &prefab_path,
        &serde_json::to_string(&prefab_json).unwrap(),
    );
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
    use bevy::math::Vec3;

    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("p.bsn");

    let mut app = make_app_for_prefab_tests();
    let source = app
        .world_mut()
        .spawn((
            bevy::prelude::Name::new("source"),
            bevy::prelude::Transform::from_xyz(5.0, 0.0, 0.0),
            bevy::camera::visibility::Visibility::Inherited,
        ))
        .id();
    register_live_root(
        &mut app,
        source,
        vec![
            jackdaw_bsn::BsnPatch::Name("source".to_string()),
            transform_patch(5.0, 0.0, 0.0),
        ],
    );
    app.update();

    // Saving the selection despawns the source and spawns an instance at the
    // centroid (5, 0, 0).
    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &[source], &target);

    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &target, Vec3::new(20.0, 0.0, 0.0));

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
    // A live gizmo drag mutates the ECS Transform and pushes `SetTransform` via
    // `push_executed`; `sync_after_external_execute` brings the AST up to date.
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
    register_live_root(
        &mut app,
        entity,
        vec![
            jackdaw_bsn::BsnPatch::Name("e".to_string()),
            transform_patch(0.0, 0.0, 0.0),
        ],
    );

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

    let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    let node = ast.ast_for(entity).expect("entity in the live document");
    let x = jackdaw_bsn::get_bsn_field(
        ast,
        node,
        "bevy_transform::components::transform::Transform",
        "translation.x",
    );
    let z = jackdaw_bsn::get_bsn_field(
        ast,
        node,
        "bevy_transform::components::transform::Transform",
        "translation.z",
    );
    assert!(
        matches!(x, Some(jackdaw_bsn::BsnValue::Float(v)) if (v - 7.0).abs() < 1e-4),
        "document Transform.x reflects the dragged value (7.0)"
    );
    assert!(
        matches!(z, Some(jackdaw_bsn::BsnValue::Float(v)) if (v - (-3.0)).abs() < 1e-4),
        "document Transform.z reflects the dragged value (-3.0)"
    );
}

#[test]
fn snapshot_captures_inherited_descendant_edit_as_override() {
    // An edit to a resolver-materialised child must snapshot as an override
    // under the instance, or a reload spawns a duplicate alongside it.
    use bevy::math::Vec3;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");

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
    let mut app = make_app_for_prefab_tests();
    write_bsn_prefab(
        &mut app,
        &prefab_path,
        &serde_json::to_string(&prefab_json).unwrap(),
    );
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        Vec3::new(5.0, 0.0, 0.0),
    );

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
    jackdaw_bsn::sync_to_ast(
        app.world_mut(),
        child_entity,
        std::any::TypeId::of::<bevy::prelude::Transform>(),
    );

    let text = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        std::path::Path::new(""),
    );
    let snapshot = bsn_ast(&text);

    // The Transform differs from the prefab baseline so it stays; the matching
    // Name is omitted.
    let isa_type = "jackdaw::prefab::components::IsA";
    let instance = snapshot
        .entities_with_component(isa_type)
        .first()
        .copied()
        .expect("instance node in snapshot");
    let override_node = snapshot
        .find_node_by_component_int(PEID_TYPE, 1)
        .expect("override node for PrefabEntityId 1 in snapshot");
    assert_eq!(
        snapshot.ast_parent_of(override_node),
        Some(instance),
        "override node sits under the instance"
    );

    assert!(
        snapshot.get_name(override_node).is_none(),
        "name matched the prefab baseline; should be omitted from the override. got components: {:?}",
        snapshot.component_type_paths(override_node)
    );
    let x = jackdaw_bsn::get_bsn_field(
        &snapshot,
        override_node,
        "bevy_transform::components::transform::Transform",
        "translation.x",
    )
    .expect("Transform diverged from prefab and should appear in the override");
    assert!(
        matches!(x, jackdaw_bsn::BsnValue::Float(v) if (v - 99.0).abs() < 1e-4),
        "override carries the edited translation X (99.0)"
    );
}

#[test]
fn snapshot_install_plus_reload_keeps_inherited_child_visible() {
    // A snapshot reduces inherited descendants to sparse override entries; the
    // resolver must still materialise them fully on the next reload.
    use bevy::math::Vec3;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");

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
    let mut app = make_app_for_prefab_tests();
    write_bsn_prefab(
        &mut app,
        &prefab_path,
        &serde_json::to_string(&prefab_json).unwrap(),
    );
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        Vec3::new(10.0, 0.0, 0.0),
    );

    let _ = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        std::path::Path::new(""),
    );

    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        Vec3::new(20.0, 0.0, 0.0),
    );

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
    // The snapshot round-trip `Ctrl+Z` performs, driven directly rather than
    // through the operator framework.
    use bevy::math::Vec3;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");
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

    let mut app = make_app_for_prefab_tests();
    write_bsn_prefab(
        &mut app,
        &prefab_path,
        &serde_json::to_string(&prefab_json).unwrap(),
    );

    let before_text = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        std::path::Path::new(""),
    );
    {
        let world = app.world_mut();
        let mut q = world.query::<&jackdaw::prefab::IsA>();
        assert_eq!(q.iter(world).count(), 0, "no instances before spawn");
    }

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

    jackdaw::prefab::watcher::respawn_from_sparse_text(app.world_mut(), &before_text);
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
    // Three selected brushes must become one instance with three inherited
    // children, not one child plus three leftover top-level brushes.

    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("p_boxes.bsn");

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
        register_live_root(
            &mut app,
            entity,
            vec![
                jackdaw_bsn::BsnPatch::Name("Brush".to_string()),
                transform_patch(f64::from(i) * 2.0, 0.0, 0.0),
            ],
        );
        entities.push(entity);
    }
    app.update();

    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &entities, &target);

    let written = bsn_ast(&std::fs::read_to_string(&target).unwrap());
    let root = written.roots[0];
    assert_eq!(
        1 + written.descendants_of(root).len(),
        4,
        "prefab file has synthetic root + 3 children"
    );

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
    // The framework installs a before- and after-snapshot as the live AST; the
    // prefabify+install round-trip must not lose inherited children.

    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("p_boxes.bsn");

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
        register_live_root(
            &mut app,
            entity,
            vec![
                jackdaw_bsn::BsnPatch::Name("Brush".to_string()),
                transform_patch(f64::from(i) * 2.0, 0.0, 0.0),
            ],
        );
        entities.push(entity);
    }
    app.update();

    let _before = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        std::path::Path::new(""),
    );

    jackdaw::prefab::operators::save_as_prefab_from_selection(app.world_mut(), &entities, &target);

    let _after = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        std::path::Path::new(""),
    );

    jackdaw::prefab::watcher::reload_all_instances(app.world_mut());

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
    use bevy::math::Vec3;
    use bevy::prelude::Mut;
    use jackdaw_api_internal::snapshot::ActiveSnapshotter;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");
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
    let mut app = make_app_for_prefab_tests();
    write_bsn_prefab(
        &mut app,
        &prefab_path,
        &serde_json::to_string(&prefab_json).unwrap(),
    );
    // The snapshotter reads editor-state resources; initialize them.
    app.init_resource::<jackdaw::brush::EditMode>();
    app.init_resource::<jackdaw::active_tool::ActiveTool>();
    app.init_resource::<jackdaw::gizmos::GizmoSpace>();
    app.init_resource::<jackdaw::snapping::SnapSettings>();
    app.init_resource::<jackdaw::view_modes::ViewModeSettings>();
    app.init_resource::<jackdaw::viewport_overlays::OverlaySettings>();

    app.world_mut().insert_resource(ActiveSnapshotter(Box::new(
        jackdaw::undo_snapshot::BsnDocumentSnapshotter,
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
    // `reload_all_instances` must not truncate the undo stack: it runs the frame
    // after spawn_instance and would wipe the just-pushed SnapshotDiff.
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
    // The before-snapshot capture reshuffles node indices, so a key fetched
    // before dispatch is stale; the operator resolves it from the ECS Entity.
    use bevy::math::Vec3;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");
    std::fs::write(
        &prefab_path,
        "#p\n\
         jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         bevy_transform::components::transform::Transform {\n\
             translation: glam::Vec3 { x: 0.0, y: 0.0, z: 0.0 },\n\
             rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },\n\
             scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 },\n\
         }\n\
         bevy_camera::visibility::Visibility::Inherited",
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        Vec3::new(0.0, 0.0, 0.0),
    );

    // The instance root's ECS entity, via the live document's node link.
    let instance_entity = {
        let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
        let isa_node = ast
            .entities_with_component(ISA_TYPE)
            .first()
            .copied()
            .expect("instance node exists after spawn");
        ast.ecs_for_ast(isa_node)
            .expect("instance node links to a spawned entity")
    };

    let _ = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        std::path::Path::new(""),
    );

    // Resolve the document node fresh from the Entity, as dispatch does.
    let key = app
        .world()
        .resource::<jackdaw_bsn::SceneBsnAst>()
        .ast_for(instance_entity);
    let Some(key) = key else {
        panic!("entity {instance_entity:?} not in the live BSN document");
    };
    let has_isa = jackdaw_bsn::get_bsn_field(
        app.world().resource::<jackdaw_bsn::SceneBsnAst>(),
        key,
        ISA_TYPE,
        "",
    )
    .is_some();
    assert!(has_isa, "node resolved from entity points at an IsA node");

    jackdaw::prefab::operators::unbundle_instance(app.world_mut(), key);
    let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    assert!(
        ast.entities_with_component(ISA_TYPE).is_empty(),
        "instance removed by unbundle"
    );
}

#[test]
fn apply_ast_with_override_entries_resolves_inherited_components() {
    // Applying a snapshot whose inherited descendants are sparse override
    // entries must refill the prefab baseline, not spawn bare `PrefabEntityId`.
    use bevy::math::Vec3;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");
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
    let mut app = make_app_for_prefab_tests();
    write_bsn_prefab(
        &mut app,
        &prefab_path,
        &serde_json::to_string(&prefab_json).unwrap(),
    );
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        Vec3::new(0.0, 0.0, 0.0),
    );

    let snapshot_text = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        std::path::Path::new(""),
    );

    // The test is meaningful only if the sparsify ran.
    let snapshot_ast = bsn_ast(&snapshot_text);
    let override_node = snapshot_ast
        .find_node_by_component_int(PEID_TYPE, 1)
        .expect("snapshot contains a sparse override entry for the inherited child");
    assert!(
        snapshot_ast
            .find_patch_by_type_path(override_node, ISA_TYPE)
            .is_none(),
        "the override entry is a descendant, not the instance root"
    );
    assert!(
        snapshot_ast.get_name(override_node).is_none(),
        "sparse override omits the Name (matches prefab baseline)"
    );

    jackdaw::prefab::watcher::respawn_from_sparse_text(app.world_mut(), &snapshot_text);

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
    use bevy::math::Vec3;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");
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
    let mut app = make_app_for_prefab_tests();
    write_bsn_prefab(
        &mut app,
        &prefab_path,
        &serde_json::to_string(&prefab_json).unwrap(),
    );

    let before = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        std::path::Path::new(""),
    );

    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        Vec3::new(0.0, 0.0, 0.0),
    );

    let after = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        std::path::Path::new(""),
    );

    jackdaw::prefab::watcher::respawn_from_sparse_text(app.world_mut(), &before);
    {
        let world = app.world_mut();
        let mut q = world.query::<&jackdaw::prefab::IsA>();
        assert_eq!(q.iter(world).count(), 0, "instance removed after undo");
    }

    jackdaw::prefab::watcher::respawn_from_sparse_text(app.world_mut(), &after);
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
    let prefab_path = tmp.path().join("p.bsn");
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

    let mut app = make_app_for_prefab_tests();
    write_bsn_prefab(
        &mut app,
        &prefab_path,
        &serde_json::to_string(&prefab_json).unwrap(),
    );
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));

    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    app.world_mut()
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(Counter(counter.clone())));

    app.init_resource::<jackdaw::brush::EditMode>();
    app.init_resource::<jackdaw::active_tool::ActiveTool>();
    app.init_resource::<jackdaw::gizmos::GizmoSpace>();
    app.init_resource::<jackdaw::snapping::SnapSettings>();
    app.init_resource::<jackdaw::view_modes::ViewModeSettings>();
    app.init_resource::<jackdaw::viewport_overlays::OverlaySettings>();
    app.world_mut()
        .insert_resource(jackdaw_api_internal::snapshot::ActiveSnapshotter(Box::new(
            jackdaw::undo_snapshot::BsnDocumentSnapshotter,
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

#[test]
fn scene_save_reopen_round_trip_preserves_instance_and_override() {
    use bevy::math::Vec3;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");
    std::fs::write(
        &prefab_path,
        "#p\n\
         jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         bevy_transform::components::transform::Transform {\n\
             translation: glam::Vec3 { x: 0.0, y: 0.0, z: 0.0 },\n\
             rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },\n\
             scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 },\n\
         }\n\
         bevy_camera::visibility::Visibility::Inherited\n\
         bevy_ecs::hierarchy::Children [\n\
             #child\n\
             jackdaw::prefab::components::PrefabEntityId(1)\n\
             bevy_transform::components::transform::Transform {\n\
                 translation: glam::Vec3 { x: 1.0, y: 0.0, z: 0.0 },\n\
                 rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },\n\
                 scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 },\n\
             }\n\
             bevy_camera::visibility::Visibility::Inherited\n\
         ]",
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        Vec3::new(10.0, 0.0, 0.0),
    );

    let child_entity = {
        let world = app.world_mut();
        let mut q = world.query::<(bevy::prelude::Entity, &jackdaw::prefab::PrefabEntityId)>();
        q.iter(world)
            .find(|(_, id)| id.0 == 1)
            .map(|(e, _)| e)
            .expect("inherited child exists")
    };
    if let Some(mut t) = app
        .world_mut()
        .get_mut::<bevy::prelude::Transform>(child_entity)
    {
        t.translation.x = 99.0;
    }
    jackdaw_bsn::sync_to_ast(
        app.world_mut(),
        child_entity,
        std::any::TypeId::of::<bevy::prelude::Transform>(),
    );

    let snapshot_text = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        std::path::Path::new(""),
    );

    let snapshot_ast = bsn_ast(&snapshot_text);
    let override_entry = snapshot_ast
        .find_node_by_component_int(PEID_TYPE, 1)
        .expect("override entry on disk for the edited child");
    assert!(
        snapshot_ast
            .find_patch_by_type_path(override_entry, ISA_TYPE)
            .is_none(),
        "the override entry is a descendant, not the instance root"
    );
    let tx = jackdaw_bsn::get_bsn_field(
        &snapshot_ast,
        override_entry,
        "bevy_transform::components::transform::Transform",
        "translation.x",
    )
    .expect("override carries the Transform diff");
    assert!(
        matches!(tx, jackdaw_bsn::BsnValue::Float(v) if (v - 99.0).abs() < 1e-4),
        "on-disk override stores the edited translation X (99.0)"
    );

    let mut app2 = make_app_for_prefab_tests();
    {
        let scene_text = std::fs::read_to_string(&prefab_path).unwrap();
        app2.world_mut()
            .resource_mut::<jackdaw::prefab::PrefabAstCache>()
            .insert(&prefab_path, bsn_ast(&scene_text));
    }
    jackdaw::prefab::watcher::respawn_from_sparse_text(app2.world_mut(), &snapshot_text);

    let world = app2.world_mut();
    let mut isa_q = world.query::<&jackdaw::prefab::IsA>();
    assert_eq!(isa_q.iter(world).count(), 1, "one instance restored");

    let mut q = world.query::<(
        &jackdaw::prefab::PrefabEntityId,
        &bevy::prelude::Name,
        &bevy::prelude::Transform,
    )>();
    let (_, name, transform) = q
        .iter(world)
        .find(|(id, _, _)| id.0 == 1)
        .expect("inherited child restored on reopen");
    assert_eq!(name.as_str(), "child", "Name inherited from prefab");
    assert!(
        (transform.translation.x - 99.0).abs() < 1e-4,
        "edited Transform.x preserved across save/reopen (99.0); got {}",
        transform.translation.x
    );
}

/// A prefab that itself inherits hands the instance ids absent from its own
/// file, so baselines must be read from the resolved source, not the raw one.
#[test]
fn revert_all_reverts_field_inherited_through_a_nested_prefab() {
    let tmp = tempfile::tempdir().unwrap();
    let base_path = tmp.path().join("base.bsn");
    let mid_path = tmp.path().join("mid.bsn");
    std::fs::write(
        &base_path,
        "#base\n\
         jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         bevy_transform::components::transform::Transform {\n\
             translation: glam::Vec3 { x: 0.0, y: 0.0, z: 0.0 },\n\
             rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },\n\
             scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 },\n\
         }\n\
         bevy_ecs::hierarchy::Children [\n\
             #knob\n\
             jackdaw::prefab::components::PrefabEntityId(7)\n\
             bevy_transform::components::transform::Transform {\n\
                 translation: glam::Vec3 { x: 3.0, y: 0.0, z: 0.0 },\n\
                 rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },\n\
                 scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 },\n\
             }\n\
         ]",
    )
    .unwrap();
    std::fs::write(
        &mid_path,
        format!(
            "#mid\n\
             jackdaw::prefab::components::Prefab\n\
             jackdaw::prefab::components::PrefabEntityId(0)\n\
             jackdaw::prefab::components::IsA {{ source: \"{}\", deleted: [] }}",
            base_path.to_slash_lossy()
        ),
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &mid_path, bevy::math::Vec3::ZERO);

    let knob = |app: &bevy::prelude::App| {
        app.world()
            .resource::<jackdaw_bsn::SceneBsnAst>()
            .find_node_by_component_int(PEID_TYPE, 7)
            .expect("id inherited through the middle prefab materializes")
    };

    {
        let registry = app
            .world()
            .resource::<bevy::ecs::reflect::AppTypeRegistry>()
            .clone();
        let reg = registry.read();
        let node = knob(&app);
        let mut ast = app.world_mut().resource_mut::<jackdaw_bsn::SceneBsnAst>();
        jackdaw_bsn::set_bsn_field(
            &mut ast,
            node,
            TRANSFORM_TYPE,
            "translation.x",
            jackdaw_bsn::BsnValue::Float(99.0),
            &reg,
        );
    }
    assert_eq!(
        live_field_f64(&app, knob(&app), TRANSFORM_TYPE, "translation.x"),
        Some(99.0),
        "override lands before the revert"
    );

    let instance_key = sole_isa_node(&app);
    let report = jackdaw::prefab::operators::revert_all(app.world_mut(), instance_key);

    assert_eq!(
        report.unresolved, 0,
        "every instance entity resolves against the expanded source"
    );
    assert_eq!(
        live_field_f64(&app, knob(&app), TRANSFORM_TYPE, "translation.x"),
        Some(3.0),
        "inherited field reverts to the base prefab's value"
    );
}

/// Text of an inner prefab whose root scales by `scale` and whose child (id 7)
/// sits at `knob_x`.
fn inner_prefab_text(scale: f64, knob_x: f64) -> String {
    format!(
        "#inner\n\
         jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         bevy_transform::components::transform::Transform {{\n\
             translation: glam::Vec3 {{ x: 0.0, y: 0.0, z: 0.0 }},\n\
             rotation: glam::Quat {{ x: 0.0, y: 0.0, z: 0.0, w: 1.0 }},\n\
             scale: glam::Vec3 {{ x: {scale:?}, y: {scale:?}, z: {scale:?} }},\n\
         }}\n\
         bevy_camera::visibility::Visibility::Inherited\n\
         bevy_ecs::hierarchy::Children [\n\
             #knob\n\
             jackdaw::prefab::components::PrefabEntityId(7)\n\
             bevy_transform::components::transform::Transform {{\n\
                 translation: glam::Vec3 {{ x: {knob_x:?}, y: 0.0, z: 0.0 }},\n\
                 rotation: glam::Quat {{ x: 0.0, y: 0.0, z: 0.0, w: 1.0 }},\n\
                 scale: glam::Vec3 {{ x: 1.0, y: 1.0, z: 1.0 }},\n\
             }}\n\
             bevy_camera::visibility::Visibility::Inherited\n\
         ]"
    )
}

/// Write an inner prefab and an outer prefab whose root is an `IsA` of it.
/// Returns `(inner, outer)`.
fn write_root_inheriting_prefabs(
    dir: &std::path::Path,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let inner = dir.join("inner.bsn");
    let outer = dir.join("outer.bsn");
    std::fs::write(&inner, inner_prefab_text(2.0, 3.0)).unwrap();
    std::fs::write(
        &outer,
        format!(
            "#outer\n\
             jackdaw::prefab::components::Prefab\n\
             jackdaw::prefab::components::PrefabEntityId(0)\n\
             jackdaw::prefab::components::IsA {{ source: \"{}\", deleted: [] }}",
            inner.to_slash_lossy()
        ),
    )
    .unwrap();
    (inner, outer)
}

/// Rewrite the inner prefab and reload the scene the way the file watcher does:
/// capture the sparse scene against the old baseline, then respawn it against
/// the new one.
fn edit_inner_prefab(app: &mut App, inner: &std::path::Path, scale: f64, knob_x: f64) {
    let sparse = jackdaw::prefab::watcher::capture_sparse_scene_text(app.world_mut())
        .expect("a live document to capture");
    let text = inner_prefab_text(scale, knob_x);
    std::fs::write(inner, &text).unwrap();
    app.world_mut()
        .resource_mut::<jackdaw::prefab::PrefabAstCache>()
        .insert(inner, bsn_ast(&text));
    jackdaw::prefab::watcher::respawn_from_sparse_text(app.world_mut(), &sparse);
}

fn instance_transform(app: &mut App) -> Transform {
    let mut q = app
        .world_mut()
        .query_filtered::<&Transform, With<jackdaw::prefab::IsA>>();
    let transforms: Vec<Transform> = q.iter(app.world()).copied().collect();
    assert_eq!(transforms.len(), 1, "one instance entity");
    transforms[0]
}

fn knob_transform(app: &mut App) -> Transform {
    let mut q = app
        .world_mut()
        .query::<(&jackdaw::prefab::PrefabEntityId, &Transform)>();
    q.iter(app.world())
        .find(|(id, _)| id.0 == 7)
        .map(|(_, t)| *t)
        .expect("the inherited knob exists")
}

fn saved_scene_text(app: &mut App) -> String {
    jackdaw::scene_io::emit_bsn_scene_with_inline_assets(app.world_mut(), std::path::Path::new(""))
}

fn saved_instance_field(text: &str, field_path: &str) -> Option<jackdaw_bsn::BsnValue> {
    let ast = bsn_ast(text);
    let node = ast
        .entities_with_component(ISA_TYPE)
        .first()
        .copied()
        .expect("the saved scene holds the instance");
    jackdaw_bsn::get_bsn_field(&ast, node, TRANSFORM_TYPE, field_path)
}

fn saved_knob_transform(text: &str) -> Option<jackdaw_bsn::BsnValue> {
    let ast = bsn_ast(text);
    let node = ast.find_node_by_component_int(PEID_TYPE, 7)?;
    jackdaw_bsn::get_bsn_field(&ast, node, TRANSFORM_TYPE, "")
}

#[test]
fn an_instance_of_a_prefab_whose_root_inherits_saves_no_inherited_values() {
    let tmp = tempfile::tempdir().unwrap();
    let (_, outer) = write_root_inheriting_prefabs(tmp.path());

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &outer,
        bevy::math::Vec3::new(10.0, 0.0, 0.0),
    );
    assert!(
        (instance_transform(&mut app).scale.x - 2.0).abs() < 1e-4,
        "the instance inherits the inner root's scale"
    );

    let text = saved_scene_text(&mut app);
    assert!(
        saved_instance_field(&text, "scale").is_none(),
        "the inherited scale is not written as an override; got {text}"
    );
    assert!(
        saved_instance_field(&text, "rotation").is_none(),
        "the inherited rotation is not written as an override; got {text}"
    );
    assert!(
        matches!(
            saved_instance_field(&text, "translation.x"),
            Some(jackdaw_bsn::BsnValue::Float(x)) if (x - 10.0).abs() < 1e-4
        ),
        "the placement is written; got {text}"
    );
    assert!(
        saved_knob_transform(&text).is_none(),
        "the knob inherited through the outer prefab carries no override; got {text}"
    );
}

#[test]
fn an_edit_to_an_inner_prefab_shows_through_a_root_inheriting_instance() {
    let tmp = tempfile::tempdir().unwrap();
    let (inner, outer) = write_root_inheriting_prefabs(tmp.path());

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &outer,
        bevy::math::Vec3::new(10.0, 0.0, 0.0),
    );
    edit_inner_prefab(&mut app, &inner, 3.0, 5.0);

    let transform = instance_transform(&mut app);
    assert!(
        (transform.scale.x - 3.0).abs() < 1e-4,
        "the instance shows the inner prefab's new scale; got {}",
        transform.scale.x
    );
    assert!(
        (transform.translation.x - 10.0).abs() < 1e-4,
        "the placement survives the reload; got {}",
        transform.translation.x
    );
    assert!(
        (knob_transform(&mut app).translation.x - 5.0).abs() < 1e-4,
        "the knob shows the inner prefab's new position"
    );

    let text = saved_scene_text(&mut app);
    assert!(
        saved_instance_field(&text, "scale").is_none(),
        "no override appears after the reload; got {text}"
    );

    let mut reopened = make_app_for_prefab_tests();
    {
        let mut cache = reopened
            .world_mut()
            .resource_mut::<jackdaw::prefab::PrefabAstCache>();
        cache.insert(&inner, bsn_ast(&inner_prefab_text(4.0, 6.0)));
        cache.insert(&outer, bsn_ast(&std::fs::read_to_string(&outer).unwrap()));
    }
    jackdaw::prefab::watcher::respawn_from_sparse_text(reopened.world_mut(), &text);
    let transform = instance_transform(&mut reopened);
    assert!(
        (transform.scale.x - 4.0).abs() < 1e-4,
        "the reopened instance shows the inner prefab's current scale; got {}",
        transform.scale.x
    );
    assert!(
        (transform.translation.x - 10.0).abs() < 1e-4,
        "the reopened instance keeps its placement"
    );
    assert!(
        (knob_transform(&mut reopened).translation.x - 6.0).abs() < 1e-4,
        "the reopened knob shows the inner prefab's current position"
    );
}

#[test]
fn the_watcher_reloads_an_instance_whose_prefab_root_inherits_the_changed_file() {
    let tmp = tempfile::tempdir().unwrap();
    let (inner, outer) = write_root_inheriting_prefabs(tmp.path());

    let mut app = make_app_for_prefab_tests();
    app.add_plugins(jackdaw::prefab::watcher::PrefabWatcherPlugin);
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &inner,
        bevy::math::Vec3::new(-10.0, 0.0, 0.0),
    );
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &outer,
        bevy::math::Vec3::new(10.0, 0.0, 0.0),
    );
    for _ in 0..6 {
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let scale_of = |app: &mut App, file: &str| {
        let mut q = app
            .world_mut()
            .query::<(&Transform, &jackdaw::prefab::IsA)>();
        q.iter(app.world())
            .find(|(_, isa)| isa.source.ends_with(file))
            .map(|(t, _)| t.scale.x)
            .expect("the instance exists")
    };

    std::fs::write(&inner, inner_prefab_text(3.0, 5.0)).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        app.update();
        if (scale_of(&mut app, "inner.bsn") - 3.0).abs() < 1e-4 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        (scale_of(&mut app, "inner.bsn") - 3.0).abs() < 1e-4,
        "the watcher reloaded the direct instance"
    );
    assert!(
        (scale_of(&mut app, "outer.bsn") - 3.0).abs() < 1e-4,
        "the outer instance shows the inner prefab's new scale; got {}",
        scale_of(&mut app, "outer.bsn")
    );

    let text = saved_scene_text(&mut app);
    let ast = bsn_ast(&text);
    for node in ast.entities_with_component(ISA_TYPE) {
        assert!(
            jackdaw_bsn::get_bsn_field(&ast, node, TRANSFORM_TYPE, "scale").is_none(),
            "no instance saves the inherited scale; got {text}"
        );
    }
}

#[test]
fn the_override_dots_of_a_root_inheriting_instance_mark_only_real_overrides() {
    let tmp = tempfile::tempdir().unwrap();
    let (_, outer) = write_root_inheriting_prefabs(tmp.path());

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &outer, bevy::math::Vec3::ZERO);

    let overridden = |app: &App, node: Entity, field: &str| {
        let registry = app.world().resource::<AppTypeRegistry>().read();
        jackdaw::prefab::overrides_bsn::field_is_overridden(
            app.world().resource::<jackdaw_bsn::SceneBsnAst>(),
            app.world().resource::<jackdaw::prefab::PrefabAstCache>(),
            node,
            TRANSFORM_TYPE,
            Some(field),
            &registry,
        )
    };
    let knob = |app: &App| {
        app.world()
            .resource::<jackdaw_bsn::SceneBsnAst>()
            .find_node_by_component_int(PEID_TYPE, 7)
            .expect("the knob materializes")
    };

    let root = sole_isa_node(&app);
    assert!(
        !overridden(&app, root, "scale"),
        "the inherited scale is not marked"
    );
    assert!(
        !overridden(&app, knob(&app), "translation.x"),
        "the inherited knob position is not marked"
    );

    {
        let registry = app.world().resource::<AppTypeRegistry>().clone();
        let reg = registry.read();
        let node = knob(&app);
        let mut ast = app.world_mut().resource_mut::<jackdaw_bsn::SceneBsnAst>();
        jackdaw_bsn::set_bsn_field(
            &mut ast,
            node,
            TRANSFORM_TYPE,
            "translation.x",
            jackdaw_bsn::BsnValue::Float(99.0),
            &reg,
        );
    }
    assert!(
        overridden(&app, knob(&app), "translation.x"),
        "an edit on the inherited knob is marked"
    );
}

#[test]
fn applying_all_keeps_an_override_on_an_id_the_prefab_only_inherits() {
    let tmp = tempfile::tempdir().unwrap();
    let (_, outer) = write_root_inheriting_prefabs(tmp.path());
    let outer_text = std::fs::read_to_string(&outer).unwrap();

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &outer, bevy::math::Vec3::ZERO);

    let light = <bevy::prelude::PointLight as bevy::reflect::TypePath>::type_path();
    let knob = |app: &App| {
        app.world()
            .resource::<jackdaw_bsn::SceneBsnAst>()
            .find_node_by_component_int(PEID_TYPE, 7)
            .expect("the knob materializes")
    };
    {
        let registry = app.world().resource::<AppTypeRegistry>().clone();
        let reg = registry.read();
        let node = knob(&app);
        let mut ast = app.world_mut().resource_mut::<jackdaw_bsn::SceneBsnAst>();
        jackdaw_bsn::set_bsn_field(
            &mut ast,
            node,
            light,
            "intensity",
            jackdaw_bsn::BsnValue::Float(500.0),
            &reg,
        );
    }
    assert_eq!(
        live_field_f64(&app, knob(&app), light, "intensity"),
        Some(500.0),
        "the override lands before the apply"
    );

    let instance = sole_isa_node(&app);
    jackdaw::prefab::operators::apply_all_overrides_to_source(app.world_mut(), instance);

    assert_eq!(
        live_field_f64(&app, knob(&app), light, "intensity"),
        Some(500.0),
        "an override the prefab could not take stays on the instance"
    );
    let cached = app
        .world()
        .resource::<jackdaw::prefab::PrefabAstCache>()
        .get(&outer)
        .map(jackdaw_bsn::emit_scene)
        .expect("the outer prefab stays cached");
    assert_eq!(
        cached,
        jackdaw_bsn::emit_scene(&bsn_ast(&outer_text)),
        "the outer prefab is unchanged"
    );
}

#[test]
fn an_override_on_a_root_inheriting_instance_is_saved_and_kept() {
    let tmp = tempfile::tempdir().unwrap();
    let (inner, outer) = write_root_inheriting_prefabs(tmp.path());

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &outer,
        bevy::math::Vec3::new(10.0, 0.0, 0.0),
    );
    let instance = {
        let mut q = app
            .world_mut()
            .query_filtered::<Entity, With<jackdaw::prefab::IsA>>();
        q.single(app.world()).expect("one instance entity")
    };
    if let Some(mut t) = app.world_mut().get_mut::<Transform>(instance) {
        t.scale = bevy::math::Vec3::splat(7.0);
    }
    jackdaw_bsn::sync_to_ast(
        app.world_mut(),
        instance,
        std::any::TypeId::of::<Transform>(),
    );

    let text = saved_scene_text(&mut app);
    assert!(
        matches!(
            saved_instance_field(&text, "scale.x"),
            Some(jackdaw_bsn::BsnValue::Float(x)) if (x - 7.0).abs() < 1e-4
        ),
        "the authored scale is written; got {text}"
    );

    edit_inner_prefab(&mut app, &inner, 3.0, 5.0);
    assert!(
        (instance_transform(&mut app).scale.x - 7.0).abs() < 1e-4,
        "the authored scale wins over the inner prefab's edit"
    );
}

/// Text of an outer prefab that places the inner prefab as its child (id 5) at
/// `child_x` and moves the inner prefab's knob to `knob_x`.
fn nesting_prefab_text(inner: &std::path::Path, child_x: f64, knob_x: f64) -> String {
    placing_prefab_text(inner, "", child_x, Some(knob_x), &[])
}

/// Text of an outer prefab whose root carries `root_isa` and which places the
/// inner prefab as its child (id 5) at `child_x`, deleting the inner ids in
/// `deleted` and moving the knob to `knob_x` when given.
fn placing_prefab_text(
    inner: &std::path::Path,
    root_isa: &str,
    child_x: f64,
    knob_x: Option<f64>,
    deleted: &[u32],
) -> String {
    let knob = knob_x.map_or(String::new(), |x| {
        format!(
            "bevy_ecs::hierarchy::Children [\n\
                 jackdaw::prefab::components::PrefabEntityId(7)\n\
                 bevy_transform::components::transform::Transform {{ translation: glam::Vec3 {{ x: {x:?}, y: 0.0, z: 0.0 }} }}\n\
             ]\n"
        )
    });
    let deleted = deleted
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "#outer\n\
         jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         {root_isa}\n\
         bevy_transform::components::transform::Transform {{\n\
             translation: glam::Vec3 {{ x: 0.0, y: 0.0, z: 0.0 }},\n\
             rotation: glam::Quat {{ x: 0.0, y: 0.0, z: 0.0, w: 1.0 }},\n\
             scale: glam::Vec3 {{ x: 1.0, y: 1.0, z: 1.0 }},\n\
         }}\n\
         bevy_ecs::hierarchy::Children [\n\
             jackdaw::prefab::components::IsA {{ source: \"{}\", deleted: [{deleted}] }}\n\
             jackdaw::prefab::components::PrefabEntityId(5)\n\
             bevy_transform::components::transform::Transform {{ translation: glam::Vec3 {{ x: {child_x:?}, y: 0.0, z: 0.0 }} }}\n\
             {knob}\
         ]",
        inner.to_slash_lossy()
    )
}

/// Write an inner prefab and an outer prefab that places it as a child at x 4
/// with its knob moved to x 8. Returns `(inner, outer)`.
fn write_nesting_prefabs(dir: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let inner = dir.join("inner.bsn");
    let outer = dir.join("outer.bsn");
    std::fs::write(&inner, inner_prefab_text(2.0, 3.0)).unwrap();
    std::fs::write(&outer, nesting_prefab_text(&inner, 4.0, 8.0)).unwrap();
    (inner, outer)
}

/// Rewrite the outer prefab and reload the scene the way the file watcher does.
fn edit_nesting_prefab(
    app: &mut App,
    inner: &std::path::Path,
    outer: &std::path::Path,
    child_x: f64,
    knob_x: f64,
) {
    let sparse = jackdaw::prefab::watcher::capture_sparse_scene_text(app.world_mut())
        .expect("a live document to capture");
    let text = nesting_prefab_text(inner, child_x, knob_x);
    std::fs::write(outer, &text).unwrap();
    app.world_mut()
        .resource_mut::<jackdaw::prefab::PrefabAstCache>()
        .insert(outer, bsn_ast(&text));
    jackdaw::prefab::watcher::respawn_from_sparse_text(app.world_mut(), &sparse);
}

fn live_node(app: &App, id: u64) -> Entity {
    app.world()
        .resource::<jackdaw_bsn::SceneBsnAst>()
        .find_node_by_component_int(PEID_TYPE, id)
        .expect("the node materializes")
}

fn live_x(app: &App, id: u64, field_path: &str) -> f64 {
    live_field_f64(app, live_node(app, id), TRANSFORM_TYPE, field_path)
        .expect("the node holds the field")
}

fn set_live_field(app: &mut App, id: u64, field_path: &str, value: f64) {
    let registry = app.world().resource::<AppTypeRegistry>().clone();
    let reg = registry.read();
    let node = live_node(app, id);
    let mut ast = app.world_mut().resource_mut::<jackdaw_bsn::SceneBsnAst>();
    jackdaw_bsn::set_bsn_field(
        &mut ast,
        node,
        TRANSFORM_TYPE,
        field_path,
        jackdaw_bsn::BsnValue::Float(value),
        &reg,
    );
}

fn saved_field(text: &str, id: u64, field_path: &str) -> Option<jackdaw_bsn::BsnValue> {
    let ast = bsn_ast(text);
    let node = ast
        .find_node_by_component_int(PEID_TYPE, id)
        .expect("the saved scene holds the node");
    jackdaw_bsn::get_bsn_field(&ast, node, TRANSFORM_TYPE, field_path)
}

fn is_float(value: Option<jackdaw_bsn::BsnValue>, expected: f64) -> bool {
    matches!(value, Some(jackdaw_bsn::BsnValue::Float(v)) if (v - expected).abs() < 1e-4)
}

fn reopen_with_nesting_prefab(
    text: &str,
    inner: &std::path::Path,
    outer: &std::path::Path,
    child_x: f64,
    knob_x: f64,
) -> App {
    let mut reopened = make_app_for_prefab_tests();
    {
        let mut cache = reopened
            .world_mut()
            .resource_mut::<jackdaw::prefab::PrefabAstCache>();
        cache.insert(inner, bsn_ast(&inner_prefab_text(2.0, 3.0)));
        cache.insert(outer, bsn_ast(&nesting_prefab_text(inner, child_x, knob_x)));
    }
    jackdaw::prefab::watcher::respawn_from_sparse_text(reopened.world_mut(), text);
    reopened
}

#[test]
fn an_instance_of_a_prefab_nesting_another_saves_none_of_the_values_it_authors() {
    let tmp = tempfile::tempdir().unwrap();
    let (_, outer) = write_nesting_prefabs(tmp.path());

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &outer,
        bevy::math::Vec3::new(10.0, 0.0, 0.0),
    );
    assert!((live_x(&app, 5, "translation.x") - 4.0).abs() < 1e-4);
    assert!((live_x(&app, 5, "scale.x") - 2.0).abs() < 1e-4);
    assert!((live_x(&app, 7, "translation.x") - 8.0).abs() < 1e-4);

    let text = saved_scene_text(&mut app);
    assert!(
        saved_field(&text, 5, "").is_none(),
        "the nested child's placement is not written as an override; got {text}"
    );
    assert!(
        saved_field(&text, 7, "").is_none(),
        "the knob position the outer prefab sets is not written as an override; got {text}"
    );
}

#[test]
fn an_edit_to_an_outer_prefab_shows_through_its_nested_child_instance() {
    let tmp = tempfile::tempdir().unwrap();
    let (inner, outer) = write_nesting_prefabs(tmp.path());

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &outer,
        bevy::math::Vec3::new(10.0, 0.0, 0.0),
    );
    edit_nesting_prefab(&mut app, &inner, &outer, 6.0, 9.0);
    assert!(
        (live_x(&app, 5, "translation.x") - 6.0).abs() < 1e-4,
        "the nested child moves with the outer prefab"
    );
    assert!(
        (live_x(&app, 7, "translation.x") - 9.0).abs() < 1e-4,
        "the knob moves with the outer prefab"
    );
    assert!(
        (live_x(&app, 5, "scale.x") - 2.0).abs() < 1e-4,
        "the nested child keeps the inner prefab's scale"
    );

    let text = saved_scene_text(&mut app);
    assert!(saved_field(&text, 5, "").is_none(), "got {text}");
    assert!(saved_field(&text, 7, "").is_none(), "got {text}");

    let reopened = reopen_with_nesting_prefab(&text, &inner, &outer, 1.0, 2.0);
    assert!(
        (live_x(&reopened, 5, "translation.x") - 1.0).abs() < 1e-4,
        "the reopened nested child shows the outer prefab's current placement"
    );
    assert!(
        (live_x(&reopened, 7, "translation.x") - 2.0).abs() < 1e-4,
        "the reopened knob shows the outer prefab's current position"
    );
}

#[test]
fn an_override_on_a_nested_child_instance_is_saved_and_wins() {
    let tmp = tempfile::tempdir().unwrap();
    let (inner, outer) = write_nesting_prefabs(tmp.path());

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &outer, bevy::math::Vec3::ZERO);
    set_live_field(&mut app, 5, "translation.x", 50.0);
    set_live_field(&mut app, 7, "translation.x", 77.0);

    let text = saved_scene_text(&mut app);
    assert!(
        is_float(saved_field(&text, 5, "translation.x"), 50.0),
        "got {text}"
    );
    assert!(
        is_float(saved_field(&text, 7, "translation.x"), 77.0),
        "got {text}"
    );

    edit_nesting_prefab(&mut app, &inner, &outer, 6.0, 9.0);
    assert!((live_x(&app, 5, "translation.x") - 50.0).abs() < 1e-4);
    assert!((live_x(&app, 7, "translation.x") - 77.0).abs() < 1e-4);

    let reopened = reopen_with_nesting_prefab(&text, &inner, &outer, 1.0, 2.0);
    assert!((live_x(&reopened, 5, "translation.x") - 50.0).abs() < 1e-4);
    assert!((live_x(&reopened, 7, "translation.x") - 77.0).abs() < 1e-4);
}

#[test]
fn the_override_dots_of_a_nested_child_instance_mark_only_real_overrides() {
    let tmp = tempfile::tempdir().unwrap();
    let (_, outer) = write_nesting_prefabs(tmp.path());

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &outer, bevy::math::Vec3::ZERO);

    let overridden = |app: &App, id: u64, field: &str| {
        let registry = app.world().resource::<AppTypeRegistry>().read();
        jackdaw::prefab::overrides_bsn::field_is_overridden(
            app.world().resource::<jackdaw_bsn::SceneBsnAst>(),
            app.world().resource::<jackdaw::prefab::PrefabAstCache>(),
            live_node(app, id),
            TRANSFORM_TYPE,
            Some(field),
            &registry,
        )
    };
    assert!(!overridden(&app, 5, "translation.x"));
    assert!(!overridden(&app, 5, "scale.x"));
    assert!(!overridden(&app, 7, "translation.x"));

    set_live_field(&mut app, 5, "translation.y", 3.0);
    set_live_field(&mut app, 7, "translation.x", 77.0);
    assert!(overridden(&app, 5, "translation.y"));
    assert!(!overridden(&app, 5, "translation.x"));
    assert!(overridden(&app, 7, "translation.x"));
}

#[test]
fn reverting_a_nested_child_instance_restores_the_outer_prefabs_values() {
    let tmp = tempfile::tempdir().unwrap();
    let (_, outer) = write_nesting_prefabs(tmp.path());

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &outer, bevy::math::Vec3::ZERO);
    set_live_field(&mut app, 5, "translation.x", 50.0);
    set_live_field(&mut app, 7, "translation.x", 77.0);

    let child = live_node(&app, 5);
    assert!(jackdaw::prefab::operators::revert_field(
        app.world_mut(),
        child,
        TRANSFORM_TYPE,
        "translation.x",
    ));
    assert!((live_x(&app, 5, "translation.x") - 4.0).abs() < 1e-4);
    let knob = live_node(&app, 7);
    assert!(jackdaw::prefab::operators::revert_field(
        app.world_mut(),
        knob,
        TRANSFORM_TYPE,
        "translation.x",
    ));
    assert!((live_x(&app, 7, "translation.x") - 8.0).abs() < 1e-4);

    set_live_field(&mut app, 5, "translation.x", 50.0);
    set_live_field(&mut app, 7, "translation.x", 77.0);
    let root = sole_isa_node(&app);
    let report = jackdaw::prefab::operators::revert_all(app.world_mut(), root);
    assert_eq!(report.unresolved, 0);
    assert!((live_x(&app, 5, "translation.x") - 4.0).abs() < 1e-4);
    assert!((live_x(&app, 5, "scale.x") - 2.0).abs() < 1e-4);
    assert!((live_x(&app, 7, "translation.x") - 8.0).abs() < 1e-4);
}

#[test]
fn applying_a_nested_child_override_writes_it_into_the_outer_prefab() {
    let tmp = tempfile::tempdir().unwrap();
    let (inner, outer) = write_nesting_prefabs(tmp.path());

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &outer, bevy::math::Vec3::ZERO);
    set_live_field(&mut app, 5, "translation.y", 3.0);
    set_live_field(&mut app, 7, "translation.x", 12.0);

    let child_owner = {
        let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
        jackdaw::prefab::overrides_bsn::id_owner(ast, live_node(&app, 5))
            .expect("the nested child is numbered by the outer instance")
    };
    assert_eq!(child_owner, sole_isa_node(&app));
    jackdaw::prefab::operators::apply_to_prefab_source(
        app.world_mut(),
        child_owner,
        5,
        TRANSFORM_TYPE,
        "translation.y",
        jackdaw_bsn::BsnValue::Float(3.0),
    );
    let knob_owner = {
        let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
        jackdaw::prefab::overrides_bsn::id_owner(ast, live_node(&app, 7))
            .expect("the knob is numbered by the nested child")
    };
    assert_eq!(knob_owner, live_node(&app, 5));
    jackdaw::prefab::operators::apply_to_prefab_source(
        app.world_mut(),
        knob_owner,
        7,
        TRANSFORM_TYPE,
        "translation.x",
        jackdaw_bsn::BsnValue::Float(12.0),
    );

    {
        let cache = app.world().resource::<jackdaw::prefab::PrefabAstCache>();
        let raw = cache.get(&outer).expect("the outer prefab stays cached");
        let entry = raw
            .find_node_by_component_int(PEID_TYPE, 5)
            .expect("the outer prefab holds its child");
        let field = |node, path| jackdaw_bsn::get_bsn_field(raw, node, TRANSFORM_TYPE, path);
        assert!(is_float(field(entry, "translation.y"), 3.0));
        assert!(is_float(field(entry, "translation.x"), 4.0));
        assert!(
            field(entry, "scale").is_none(),
            "the outer prefab does not take over the inner prefab's scale"
        );
        let knob = raw
            .find_node_by_component_int(PEID_TYPE, 7)
            .expect("the outer prefab holds the knob tweak");
        assert!(is_float(field(knob, "translation.x"), 12.0));
        let inner_raw = cache.get(&inner).expect("the inner prefab stays cached");
        assert_eq!(
            jackdaw_bsn::emit_scene(inner_raw),
            jackdaw_bsn::emit_scene(&bsn_ast(&inner_prefab_text(2.0, 3.0))),
            "the inner prefab is unchanged"
        );
    }

    for _ in 0..3 {
        app.update();
    }
    assert!((live_x(&app, 5, "translation.y") - 3.0).abs() < 1e-4);
    assert!((live_x(&app, 7, "translation.x") - 12.0).abs() < 1e-4);
    let text = saved_scene_text(&mut app);
    assert!(saved_field(&text, 5, "").is_none(), "got {text}");
    assert!(saved_field(&text, 7, "").is_none(), "got {text}");
}

#[test]
fn applying_a_list_element_to_a_root_inheriting_prefab_keeps_the_inner_list() {
    let tmp = tempfile::tempdir().unwrap();
    let inner = tmp.path().join("inner.bsn");
    let outer = tmp.path().join("outer.bsn");
    std::fs::write(
        &inner,
        format!(
            "#inner\n\
             jackdaw::prefab::components::Prefab\n\
             jackdaw::prefab::components::PrefabEntityId(0)\n\
             {BRUSH} {{ faces: [jackdaw_geometry::BrushFaceData {{ uv_rotation: 5.0 }}, \
             jackdaw_geometry::BrushFaceData {{ uv_rotation: 7.0 }}] }}\n"
        ),
    )
    .unwrap();
    std::fs::write(
        &outer,
        format!(
            "#outer\n\
             jackdaw::prefab::components::Prefab\n\
             jackdaw::prefab::components::PrefabEntityId(0)\n\
             jackdaw::prefab::components::IsA {{ source: \"{}\", deleted: [] }}",
            inner.to_slash_lossy()
        ),
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &outer, bevy::math::Vec3::ZERO);
    let entity = {
        let mut q = app
            .world_mut()
            .query_filtered::<Entity, With<jackdaw::prefab::IsA>>();
        q.single(app.world()).expect("one instance entity")
    };
    if let Some(mut brush) = app
        .world_mut()
        .get_mut::<jackdaw_scene_types::types::Brush>(entity)
    {
        brush.faces[0].uv_rotation = 9.0;
    }
    jackdaw_bsn::sync_to_ast(
        app.world_mut(),
        entity,
        std::any::TypeId::of::<jackdaw_scene_types::types::Brush>(),
    );
    let instance = sole_isa_node(&app);
    jackdaw::prefab::operators::apply_to_prefab_source(
        app.world_mut(),
        instance,
        0,
        BRUSH,
        "faces[0].uv_rotation",
        jackdaw_bsn::BsnValue::Float(9.0),
    );
    for _ in 0..3 {
        app.update();
    }

    let held = |app: &App, path: &str| {
        let registry = app.world().resource::<AppTypeRegistry>().read();
        jackdaw_bsn::held_bsn_field(
            app.world().resource::<jackdaw_bsn::SceneBsnAst>(),
            sole_isa_node(app),
            BRUSH,
            path,
            &registry,
        )
    };
    assert_eq!(
        held(&app, "faces[0].uv_rotation"),
        Some(jackdaw_bsn::BsnValue::Float(9.0))
    );
    assert_eq!(
        held(&app, "faces[1].uv_rotation"),
        Some(jackdaw_bsn::BsnValue::Float(7.0)),
        "the inner prefab's other face survives the apply"
    );
    let text = saved_scene_text(&mut app);
    let saved = bsn_ast(&text);
    let node = saved
        .entities_with_component(ISA_TYPE)
        .first()
        .copied()
        .expect("the saved scene holds the instance");
    assert!(
        jackdaw_bsn::get_bsn_field(&saved, node, BRUSH, "").is_none(),
        "the applied face is no longer an override; got {text}"
    );
}

/// An unmatched `PrefabEntityId` must be reported, never silently dropped from
/// a pass that then claims to have reverted everything.
#[test]
fn revert_all_reports_targets_with_no_baseline() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");
    std::fs::write(
        &prefab_path,
        "#p\n\
         jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         bevy_transform::components::transform::Transform {\n\
             translation: glam::Vec3 { x: 0.0, y: 0.0, z: 0.0 },\n\
             rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },\n\
             scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 },\n\
         }",
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        bevy::math::Vec3::ZERO,
    );

    let instance_key = sole_isa_node(&app);
    {
        let mut ast = app.world_mut().resource_mut::<jackdaw_bsn::SceneBsnAst>();
        let stray = ast.create_entity_node(vec![peid_patch(42)]);
        ast.add_child_to_ast(instance_key, stray);
    }

    let report = jackdaw::prefab::operators::revert_all(app.world_mut(), instance_key);

    assert_eq!(
        report.unresolved, 1,
        "the id absent from the source is reported, not silently skipped"
    );
    assert_eq!(report.reverted, 1, "the instance root itself still reverts");
}

/// Write a prefab the way most editors save: a new file renamed over the old.
fn write_bsn_prefab_by_rename(app: &mut App, path: &std::path::Path, jsn: &str) {
    let staged = path.with_extension("bsn.tmp");
    write_bsn_prefab(app, &staged, jsn);
    std::fs::rename(&staged, path).expect("rename over the prefab");
}

fn wait_for_name(app: &mut App, name: &str) -> Vec<String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        app.update();
        if current_names(app).iter().any(|n| n == name) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    current_names(app)
}

/// A save by rename replaces the file the watch was placed on; the edits
/// after it are still picked up.
#[test]
fn a_prefab_saved_by_rename_stays_watched() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");
    let scene_path = tmp.path().join("s.jsn");

    let mut app = make_app_for_prefab_tests();
    app.add_plugins(jackdaw::prefab::watcher::PrefabWatcherPlugin);
    write_bsn_prefab(&mut app, &prefab_path, &prefab_with_name("v1"));
    std::fs::write(&scene_path, scene_referencing(&prefab_path)).unwrap();
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene_path);
    app.update();

    for name in ["v2", "v3"] {
        write_bsn_prefab_by_rename(&mut app, &prefab_path, &prefab_with_name(name));
        let names = wait_for_name(&mut app, name);
        assert!(
            names.iter().any(|n| n == name),
            "{name} is picked up after a save by rename; got {names:?}"
        );
    }
}

/// Putting a prefab back as it was, modification time included, reloads the
/// first version rather than reading as a file the editor already holds.
#[test]
fn a_prefab_restored_to_its_first_contents_reloads_them() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");
    let scene_path = tmp.path().join("s.jsn");
    let backup = tmp.path().join("p.bsn.orig");

    let mut app = make_app_for_prefab_tests();
    app.add_plugins(jackdaw::prefab::watcher::PrefabWatcherPlugin);
    write_bsn_prefab(&mut app, &prefab_path, &prefab_with_name("v1"));
    std::fs::copy(&prefab_path, &backup).unwrap();
    let first_written = std::fs::metadata(&prefab_path).unwrap().modified().unwrap();
    std::fs::File::options()
        .write(true)
        .open(&backup)
        .unwrap()
        .set_modified(first_written)
        .unwrap();
    std::fs::write(&scene_path, scene_referencing(&prefab_path)).unwrap();
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene_path);
    app.update();

    std::fs::write(&prefab_path, {
        let staged = tmp.path().join("v2.bsn");
        write_bsn_prefab(&mut app, &staged, &prefab_with_name("v2"));
        std::fs::read(&staged).unwrap()
    })
    .unwrap();
    let names = wait_for_name(&mut app, "v2");
    assert!(
        names.iter().any(|n| n == "v2"),
        "the edit lands; got {names:?}"
    );

    std::fs::rename(&backup, &prefab_path).unwrap();
    let names = wait_for_name(&mut app, "v1");
    assert!(
        names.iter().any(|n| n == "v1"),
        "the restored first version is reloaded; got {names:?}"
    );
}

/// A prefab that stops parsing stays watched and keeps its last good cached
/// copy, so the repairing edit is picked up like any other.
#[test]
fn a_prefab_that_fails_to_parse_stays_watched_and_recovers() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");
    let scene_path = tmp.path().join("s.jsn");

    let mut app = make_app_for_prefab_tests();
    app.add_plugins(jackdaw::prefab::watcher::PrefabWatcherPlugin);
    write_bsn_prefab(&mut app, &prefab_path, &prefab_with_name("v1"));
    std::fs::write(&scene_path, scene_referencing(&prefab_path)).unwrap();
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene_path);
    assert!(
        current_names(&mut app).iter().any(|n| n == "v1"),
        "initial load sees v1"
    );

    std::fs::write(&prefab_path, "not a bsn document {{{").unwrap();
    let settle = std::time::Instant::now() + std::time::Duration::from_millis(1200);
    while std::time::Instant::now() < settle {
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        current_names(&mut app).iter().any(|n| n == "v1"),
        "a file that stopped parsing leaves the scene on its last good copy"
    );

    write_bsn_prefab(&mut app, &prefab_path, &prefab_with_name("v2"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        app.update();
        if current_names(&mut app).iter().any(|n| n == "v2") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let names = current_names(&mut app);
    assert!(
        names.iter().any(|n| n == "v2"),
        "the repaired prefab is picked up; got {names:?}"
    );
}

#[test]
fn saving_a_prefab_whose_text_does_not_parse_back_leaves_the_file_and_stays_dirty() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");
    let original = "#p\n\
                    jackdaw::prefab::components::Prefab\n\
                    jackdaw::prefab::components::PrefabEntityId(0)\n";
    std::fs::write(&prefab_path, original).unwrap();

    let mut app = make_app_for_prefab_tests();
    app.init_resource::<jackdaw::scenes::Scenes>();
    app.init_resource::<jackdaw::status_bar::StatusNotice>();
    app.world_mut()
        .resource_mut::<jackdaw::prefab::PrefabAstCache>()
        .insert(&prefab_path, bsn_ast(original));
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
    jackdaw::scenes::swap::activate_tab(app.world_mut(), 0);

    {
        let mut ast = app.world_mut().resource_mut::<jackdaw_bsn::SceneBsnAst>();
        let odd = ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::Struct(
            jackdaw_bsn::BsnStructData {
                type_path: String::new(),
                fields: jackdaw_bsn::BsnStructFields(vec![jackdaw_bsn::BsnField {
                    name: "0".to_string(),
                    value: jackdaw_bsn::BsnValue::Int(1),
                }]),
            },
        )]);
        ast.add_to_roots(odd);
    }
    app.world_mut()
        .resource_mut::<jackdaw::scenes::Scenes>()
        .tabs[0]
        .dirty = true;

    assert!(!jackdaw::scene_io::save_scene(app.world_mut()));

    assert_eq!(
        std::fs::read_to_string(&prefab_path).unwrap(),
        original,
        "the prefab file was overwritten"
    );
    assert!(
        app.world().resource::<jackdaw::scenes::Scenes>().tabs[0].dirty,
        "a prefab that was not written stays dirty"
    );
    let cached_roots = app
        .world()
        .resource::<jackdaw::prefab::PrefabAstCache>()
        .get(&prefab_path)
        .map(|ast| ast.roots.len());
    assert_eq!(
        cached_roots,
        Some(1),
        "the cache keeps the prefab's last good document"
    );
    let notice = app
        .world()
        .resource::<jackdaw::status_bar::StatusNotice>()
        .text()
        .to_string();
    assert!(
        notice.contains("Not saved"),
        "the refusal is shown: {notice:?}"
    );
}

#[test]
fn saving_a_prefab_tab_writes_its_terrain_and_tile_sidecars() {
    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("p.bsn");

    let mut app = make_app_for_prefab_tests();
    app.init_resource::<jackdaw::scenes::Scenes>();
    app.init_resource::<jackdaw::terrain_mode::TerrainDataStore>();
    jackdaw_editor_tile::tilemap::plugin(&mut app);
    jackdaw_editor_terrain::sidecar_io::plugin(&mut app);
    app.world_mut()
        .resource_mut::<jackdaw::prefab::PrefabAstCache>()
        .insert(
            &prefab_path,
            bsn_ast(
                "#p\n\
                 jackdaw::prefab::components::Prefab\n\
                 jackdaw::prefab::components::PrefabEntityId(0)\n\
                 jackdaw_scene_types::types::Terrain { resolution: 2, data_path: \"p.terrain-0.jdterrain\" }\n\
                 jackdaw_scene_types::tilemap::Tilemap { data_path: \"p.tilemap-0.jdtiles\" }",
            ),
        );
    {
        let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
        let mut tab = jackdaw::scenes::SceneTab::new_untitled(1);
        tab.path = Some(prefab_path.clone());
        tab.kind = jackdaw::scenes::TabKind::Prefab;
        tab.content = jackdaw::scenes::TabContent::Prefab(jackdaw::prefab::canonical_prefab_path(
            &prefab_path,
        ));
        scenes.tabs.push(tab);
    }
    app.world_mut()
        .resource_mut::<jackdaw::scene_io::SceneFilePath>()
        .path = Some(prefab_path.to_string_lossy().into_owned());
    jackdaw::scenes::swap::activate_tab(app.world_mut(), 0);

    app.world_mut()
        .resource_mut::<jackdaw::terrain_mode::TerrainDataStore>()
        .insert(
            "p.terrain-0.jdterrain".to_string(),
            jackdaw_terrain::RegionTerrainData::from_legacy_v1(&jackdaw_terrain::TerrainData {
                resolution: 2,
                heights: vec![0.5; 4],
                channels: vec![],
            })
            .expect("a power-of-two resolution migrates"),
        );
    let map = jackdaw_tilemap::TilemapData {
        layers: vec![jackdaw_tilemap::LayerData {
            kind: jackdaw_tilemap::LayerKind::Tile,
            placement: Default::default(),
            tiles: jackdaw_tilemap::ChunkedLayer::new(),
            authoring: None,
        }],
    };
    let key = tilemap_key(&mut app);
    app.world_mut()
        .resource_mut::<jackdaw_editor_tile::tilemap::TilemapDataStore>()
        .insert(key, map.clone());

    jackdaw::scene_io::save_scene(app.world_mut());

    assert!(tmp.path().join("p.terrain-0.jdterrain").exists());
    assert_eq!(
        std::fs::read(tmp.path().join("p.tilemap-0.jdtiles")).expect("tile sidecar written"),
        jackdaw_tilemap::encode(&map)
    );
}

#[test]
fn a_tilemap_inherited_from_a_prefab_keeps_its_sidecar_beside_the_prefab() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("levels")).unwrap();
    std::fs::create_dir_all(tmp.path().join("prefabs")).unwrap();
    let prefab_sidecar = tmp.path().join("prefabs/p.tilemap-0.jdtiles");
    let scene_path = tmp.path().join("levels/zone.bsn");
    std::fs::write(
        tmp.path().join("prefabs/p.bsn"),
        "#p\n\
         jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         jackdaw_scene_types::tilemap::Tilemap { data_path: \"p.tilemap-0.jdtiles\" }\n",
    )
    .unwrap();
    let map = jackdaw_tilemap::TilemapData {
        layers: vec![jackdaw_tilemap::LayerData {
            kind: jackdaw_tilemap::LayerKind::Tile,
            placement: Default::default(),
            tiles: jackdaw_tilemap::ChunkedLayer::new(),
            authoring: None,
        }],
    };
    let bytes = jackdaw_tilemap::encode(&map);
    std::fs::write(&prefab_sidecar, &bytes).unwrap();
    std::fs::write(
        &scene_path,
        "#instance\n\
         jackdaw::prefab::components::IsA { source: \"../prefabs/p.bsn\", deleted: [] }\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n",
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();
    app.init_resource::<jackdaw::scenes::Scenes>();
    app.init_resource::<jackdaw::terrain_mode::TerrainDataStore>();
    jackdaw_editor_tile::tilemap::plugin(&mut app);
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene_path);
    let key = tilemap_key(&mut app);
    assert_eq!(
        app.world()
            .resource::<jackdaw_editor_tile::tilemap::TilemapDataStore>()
            .get(&key),
        Some(&map),
        "the instance reads the prefab's sidecar"
    );
    {
        let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
        let mut tab = jackdaw::scenes::SceneTab::new_untitled(1);
        tab.path = Some(scene_path.clone());
        tab.dirty = true;
        scenes.tabs = vec![tab];
    }
    app.world_mut()
        .resource_mut::<jackdaw_editor_tile::tilemap::TilemapDataStore>()
        .get_mut(&key)
        .expect("loaded")
        .layers
        .clear();
    assert!(jackdaw::scenes::operators::scene_save_all_system(
        app.world_mut()
    ));

    assert!(
        !tmp.path().join("levels/p.tilemap-0.jdtiles").exists(),
        "nothing is written beside the scene"
    );
    assert_eq!(
        std::fs::read(&prefab_sidecar).unwrap(),
        bytes,
        "the prefab's file is written by the prefab's own save, not the scene's"
    );
}

/// The store key of the one tilemap in the world.
fn tilemap_key(app: &mut bevy::prelude::App) -> String {
    tilemap_keys(app).pop().expect("one tilemap")
}

fn tilemap_keys(app: &mut bevy::prelude::App) -> Vec<String> {
    let entities: Vec<_> = app
        .world_mut()
        .query_filtered::<bevy::prelude::Entity, bevy::prelude::With<jackdaw_scene_types::Tilemap>>(
        )
        .iter(app.world())
        .collect();
    let mut keys: Vec<String> = entities
        .into_iter()
        .filter_map(|e| jackdaw_editor_tile::tilemap::store_key_of(app.world_mut(), e))
        .collect();
    keys.sort();
    keys
}

fn tile_map(tile: u16) -> jackdaw_tilemap::TilemapData {
    let mut tiles = jackdaw_tilemap::ChunkedLayer::new();
    tiles.set(
        jackdaw_tilemap::CellCoord::new(0, 0),
        jackdaw_tilemap::TileRef::new(tile),
    );
    jackdaw_tilemap::TilemapData {
        layers: vec![jackdaw_tilemap::LayerData {
            kind: jackdaw_tilemap::LayerKind::Tile,
            placement: Default::default(),
            tiles,
            authoring: None,
        }],
    }
}

#[test]
fn a_scene_map_and_a_prefab_map_of_one_file_name_stay_apart() {
    let tmp = tempfile::tempdir().unwrap();
    for dir in ["levels", "prefabs"] {
        std::fs::create_dir_all(tmp.path().join(dir)).unwrap();
    }
    std::fs::write(
        tmp.path().join("prefabs/forest.bsn"),
        "#forest\n\
         jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         jackdaw_scene_types::tilemap::Tilemap { data_path: \"forest.tilemap-0.jdtiles\" }\n",
    )
    .unwrap();
    let scene_path = tmp.path().join("levels/forest.bsn");
    std::fs::write(
        &scene_path,
        "bevy_ecs::hierarchy::Children [\n\
         #instance\n\
         jackdaw::prefab::components::IsA { source: \"../prefabs/forest.bsn\", deleted: [] }\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         --\n\
         #ground\n\
         jackdaw_scene_types::tilemap::Tilemap { data_path: \"forest.tilemap-0.jdtiles\" }\n\
         ]\n",
    )
    .unwrap();
    let prefab_file = tmp.path().join("prefabs/forest.tilemap-0.jdtiles");
    let scene_file = tmp.path().join("levels/forest.tilemap-0.jdtiles");
    std::fs::write(&prefab_file, jackdaw_tilemap::encode(&tile_map(1))).unwrap();
    std::fs::write(&scene_file, jackdaw_tilemap::encode(&tile_map(2))).unwrap();

    let mut app = make_app_for_prefab_tests();
    app.init_resource::<jackdaw::scenes::Scenes>();
    app.init_resource::<jackdaw::terrain_mode::TerrainDataStore>();
    jackdaw_editor_tile::tilemap::plugin(&mut app);
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene_path);

    let keys = tilemap_keys(&mut app);
    assert_eq!(keys.len(), 2, "two entries: {keys:?}");
    let store = app
        .world()
        .resource::<jackdaw_editor_tile::tilemap::TilemapDataStore>();
    let cells: Vec<_> = keys.iter().map(|k| store.get(k).cloned()).collect();
    assert!(cells.contains(&Some(tile_map(1))) && cells.contains(&Some(tile_map(2))));

    {
        let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
        let mut tab = jackdaw::scenes::SceneTab::new_untitled(1);
        tab.path = Some(scene_path.clone());
        tab.dirty = true;
        scenes.tabs = vec![tab];
    }
    for key in keys {
        app.world_mut()
            .resource_mut::<jackdaw_editor_tile::tilemap::TilemapDataStore>()
            .get_mut(&key);
    }
    assert!(jackdaw::scenes::operators::scene_save_all_system(
        app.world_mut()
    ));
    assert_eq!(
        std::fs::read(&prefab_file).unwrap(),
        jackdaw_tilemap::encode(&tile_map(1))
    );
    assert_eq!(
        std::fs::read(&scene_file).unwrap(),
        jackdaw_tilemap::encode(&tile_map(2))
    );
}

#[test]
fn a_variant_of_a_prefab_holding_a_tilemap_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let base_path = tmp.path().join("base.bsn");
    let variant_path = tmp.path().join("variant.bsn");
    let base_sidecar = tmp.path().join("base.tilemap-0.jdtiles");
    std::fs::write(
        &base_path,
        "#base\n\
         jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         jackdaw_scene_types::tilemap::Tilemap { data_path: \"base.tilemap-0.jdtiles\" }\n",
    )
    .unwrap();
    let bytes = jackdaw_tilemap::encode(&tile_map(1));
    std::fs::write(&base_sidecar, &bytes).unwrap();

    let mut app = make_app_for_prefab_tests();
    app.init_resource::<jackdaw::status_bar::StatusNotice>();
    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &base_path, bevy::math::Vec3::ZERO);
    let instance = app
        .world_mut()
        .query_filtered::<bevy::prelude::Entity, bevy::prelude::With<jackdaw::prefab::IsA>>()
        .iter(app.world())
        .next()
        .expect("instance entity present");
    jackdaw::prefab::operators::save_as_variant(app.world_mut(), instance, &variant_path);

    assert!(!variant_path.exists(), "no variant is written");
    assert_eq!(std::fs::read(&base_sidecar).unwrap(), bytes);
    let notice = app.world().resource::<jackdaw::status_bar::StatusNotice>();
    assert!(
        notice.text().contains("not supported yet"),
        "{}",
        notice.text()
    );
}

#[test]
fn a_prefab_tab_and_a_scene_instancing_it_share_the_prefabs_cells() {
    let tmp = tempfile::tempdir().unwrap();
    for dir in ["levels", "prefabs"] {
        std::fs::create_dir_all(tmp.path().join(dir)).unwrap();
    }
    let prefab_path = tmp.path().join("prefabs/p.bsn");
    std::fs::write(
        &prefab_path,
        "#p\n\
         jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         jackdaw_scene_types::tilemap::Tilemap { data_path: \"p.tilemap-0.jdtiles\" }\n",
    )
    .unwrap();
    let prefab_file = tmp.path().join("prefabs/p.tilemap-0.jdtiles");
    std::fs::write(&prefab_file, jackdaw_tilemap::encode(&tile_map(1))).unwrap();
    let scene_path = tmp.path().join("levels/zone.bsn");
    std::fs::write(
        &scene_path,
        "#instance\n\
         jackdaw::prefab::components::IsA { source: \"../prefabs/p.bsn\", deleted: [] }\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n",
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();
    app.init_resource::<jackdaw::scenes::Scenes>();
    app.init_resource::<jackdaw::terrain_mode::TerrainDataStore>();
    jackdaw_editor_tile::tilemap::plugin(&mut app);
    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &scene_path);
    let in_scene = tilemap_key(&mut app);
    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &prefab_path);
    assert_eq!(tilemap_key(&mut app), in_scene, "one entry for both tabs");

    for (tab, cells) in [(0, tile_map(2)), (1, tile_map(3))] {
        jackdaw::scenes::swap::swap_active_tab(app.world_mut(), tab);
        *app.world_mut()
            .resource_mut::<jackdaw_editor_tile::tilemap::TilemapDataStore>()
            .get_mut(&in_scene)
            .expect("loaded") = cells.clone();
        {
            let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
            for tab in &mut scenes.tabs {
                tab.dirty = true;
            }
        }
        assert!(
            jackdaw::scenes::operators::scene_save_all_system(app.world_mut()),
            "neither tab's save is refused"
        );
        assert_eq!(
            std::fs::read(&prefab_file).unwrap(),
            jackdaw_tilemap::encode(&cells),
            "an edit from tab {tab} lands in the prefab's file"
        );
    }
    assert!(!tmp.path().join("levels/p.tilemap-0.jdtiles").exists());
}

/// Two instances of one prefab share its sidecar. Their terrain resolves with
/// the bounds of the first instance by entity; when the other's bounds differ
/// the key is noted, and a warning logged.
#[test]
fn instances_sharing_a_sidecar_with_different_bounds_are_noted() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("levels")).unwrap();
    std::fs::create_dir_all(tmp.path().join("prefabs")).unwrap();
    std::fs::write(
        tmp.path().join("prefabs/p.bsn"),
        "#p\n\
         jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         jackdaw_scene_types::tilemap::Tilemap { data_path: \"p.tilemap-0.jdtiles\" }\n",
    )
    .unwrap();
    let map = jackdaw_tilemap::TilemapData::default();
    std::fs::write(
        tmp.path().join("prefabs/p.tilemap-0.jdtiles"),
        jackdaw_tilemap::encode(&map),
    )
    .unwrap();
    let scene_path = tmp.path().join("levels/zone.bsn");
    std::fs::write(
        &scene_path,
        "bevy_ecs::hierarchy::Children [\n\
         #a\n\
         jackdaw::prefab::components::IsA { source: \"../prefabs/p.bsn\", deleted: [] }\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         --\n\
         #b\n\
         jackdaw::prefab::components::IsA { source: \"../prefabs/p.bsn\", deleted: [] }\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         ]\n",
    )
    .unwrap();

    let mut app = make_app_for_prefab_tests();
    app.init_resource::<jackdaw::scenes::Scenes>();
    app.init_resource::<jackdaw::terrain_mode::TerrainDataStore>();
    app.init_resource::<jackdaw_editor_tile::tilemap_resolve::TilemapResolveStats>();
    jackdaw_editor_tile::tilemap::plugin(&mut app);
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene_path);
    let keys = tilemap_keys(&mut app);
    assert_eq!(keys.len(), 2, "both instances read one sidecar");
    assert_eq!(keys[0], keys[1]);
    let key = keys[0].clone();
    jackdaw_editor_tile::tilemap_resolve::refresh_terrain(app.world_mut());
    let store = app
        .world()
        .resource::<jackdaw_editor_tile::tilemap::TilemapDataStore>();
    assert!(!store.bounds_diverge(&key));

    let mut maps: Vec<bevy::prelude::Entity> = app
        .world_mut()
        .query_filtered::<bevy::prelude::Entity, bevy::prelude::With<jackdaw_scene_types::Tilemap>>(
        )
        .iter(app.world())
        .collect();
    maps.sort();
    app.world_mut()
        .get_mut::<jackdaw_scene_types::Tilemap>(maps[1])
        .unwrap()
        .bounds = Some(bevy::math::IRect::new(0, 0, 4, 4));
    jackdaw_editor_tile::tilemap_resolve::refresh_terrain(app.world_mut());
    let store = app
        .world()
        .resource::<jackdaw_editor_tile::tilemap::TilemapDataStore>();
    assert!(store.bounds_diverge(&key));
}

/// Respawn a saved scene in a fresh app whose cache holds the inner prefab and
/// `outer_text` for the outer one.
fn reopen_with_outer_text(
    text: &str,
    inner: &std::path::Path,
    outer: &std::path::Path,
    outer_text: &str,
) -> App {
    let mut reopened = make_app_for_prefab_tests();
    {
        let mut cache = reopened
            .world_mut()
            .resource_mut::<jackdaw::prefab::PrefabAstCache>();
        cache.insert(inner, bsn_ast(&inner_prefab_text(2.0, 3.0)));
        cache.insert(outer, bsn_ast(outer_text));
    }
    jackdaw::prefab::watcher::respawn_from_sparse_text(reopened.world_mut(), text);
    reopened
}

fn holds_node(app: &App, id: u64) -> bool {
    app.world()
        .resource::<jackdaw_bsn::SceneBsnAst>()
        .find_node_by_component_int(PEID_TYPE, id)
        .is_some()
}

#[test]
fn applying_a_knob_override_seeds_a_sparse_entry_in_the_outer_prefab() {
    let tmp = tempfile::tempdir().unwrap();
    let inner = tmp.path().join("inner.bsn");
    let outer = tmp.path().join("outer.bsn");
    std::fs::write(&inner, inner_prefab_text(2.0, 3.0)).unwrap();
    std::fs::write(&outer, placing_prefab_text(&inner, "", 4.0, None, &[])).unwrap();

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &outer, bevy::math::Vec3::ZERO);
    assert!((live_x(&app, 7, "translation.x") - 3.0).abs() < 1e-4);
    set_live_field(&mut app, 7, "translation.x", 12.0);

    let owner = {
        let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
        jackdaw::prefab::overrides_bsn::id_owner(ast, live_node(&app, 7))
            .expect("the knob is numbered by the nested child")
    };
    jackdaw::prefab::operators::apply_to_prefab_source(
        app.world_mut(),
        owner,
        7,
        TRANSFORM_TYPE,
        "translation.x",
        jackdaw_bsn::BsnValue::Float(12.0),
    );

    {
        let cache = app.world().resource::<jackdaw::prefab::PrefabAstCache>();
        let raw = cache.get(&outer).expect("the outer prefab stays cached");
        let child = raw
            .find_node_by_component_int(PEID_TYPE, 5)
            .expect("the outer prefab holds its child");
        let knob = raw
            .get_children_ast(child)
            .into_iter()
            .find(|&node| {
                matches!(
                    jackdaw_bsn::get_bsn_field(raw, node, PEID_TYPE, ""),
                    Some(jackdaw_bsn::BsnValue::TupleStruct(_))
                )
            })
            .expect("the outer prefab gains a knob entry under its child");
        let field = |path| jackdaw_bsn::get_bsn_field(raw, knob, TRANSFORM_TYPE, path);
        assert!(is_float(field("translation.x"), 12.0));
        assert!(field("scale").is_none(), "the knob entry stays sparse");
        let inner_raw = cache.get(&inner).expect("the inner prefab stays cached");
        assert_eq!(
            jackdaw_bsn::emit_scene(inner_raw),
            jackdaw_bsn::emit_scene(&bsn_ast(&inner_prefab_text(2.0, 3.0))),
            "the inner prefab is unchanged"
        );
    }

    for _ in 0..3 {
        app.update();
    }
    assert!((live_x(&app, 7, "translation.x") - 12.0).abs() < 1e-4);
    let text = saved_scene_text(&mut app);
    assert!(saved_field(&text, 7, "").is_none(), "got {text}");
}

#[test]
fn a_child_the_outer_prefab_deletes_later_stays_gone() {
    let tmp = tempfile::tempdir().unwrap();
    let (inner, outer) = write_nesting_prefabs(tmp.path());

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &outer, bevy::math::Vec3::ZERO);
    let text = saved_scene_text(&mut app);

    let deleting = placing_prefab_text(&inner, "", 4.0, None, &[7]);
    let mut reopened = reopen_with_outer_text(&text, &inner, &outer, &deleting);
    assert!(
        !holds_node(&reopened, 7),
        "the knob the outer prefab now deletes does not come back"
    );
    let resaved = saved_scene_text(&mut reopened);
    let ast = bsn_ast(&resaved);
    assert!(
        ast.find_node_by_component_int(PEID_TYPE, 7).is_none(),
        "the deleted knob is not saved; got {resaved}"
    );
}

#[test]
fn a_child_the_outer_prefab_restores_later_comes_back() {
    let tmp = tempfile::tempdir().unwrap();
    let inner = tmp.path().join("inner.bsn");
    let outer = tmp.path().join("outer.bsn");
    std::fs::write(&inner, inner_prefab_text(2.0, 3.0)).unwrap();
    std::fs::write(&outer, placing_prefab_text(&inner, "", 4.0, None, &[7])).unwrap();

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &outer, bevy::math::Vec3::ZERO);
    assert!(!holds_node(&app, 7), "the outer prefab deletes the knob");
    let text = saved_scene_text(&mut app);

    let reopened = reopen_with_outer_text(
        &text,
        &inner,
        &outer,
        &nesting_prefab_text(&inner, 4.0, 8.0),
    );
    assert!(
        (live_x(&reopened, 7, "translation.x") - 8.0).abs() < 1e-4,
        "the restored knob comes back where the outer prefab puts it"
    );
}

#[test]
fn a_nested_child_in_a_root_inheriting_outer_prefab_follows_it() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("base.bsn");
    let inner = tmp.path().join("inner.bsn");
    let outer = tmp.path().join("outer.bsn");
    std::fs::write(
        &base,
        "#base\n\
         jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         bevy_transform::components::transform::Transform {\n\
             translation: glam::Vec3 { x: 0.0, y: 0.0, z: 0.0 },\n\
             rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },\n\
             scale: glam::Vec3 { x: 3.0, y: 3.0, z: 3.0 },\n\
         }",
    )
    .unwrap();
    std::fs::write(&inner, inner_prefab_text(2.0, 3.0)).unwrap();
    let root_isa = format!(
        "jackdaw::prefab::components::IsA {{ source: \"{}\", deleted: [] }}",
        base.to_slash_lossy()
    );
    let outer_text = |child_x, knob_x| {
        placing_prefab_text(&inner, &root_isa, child_x, Some(knob_x), &[])
            .replace("scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 },\n", "")
    };
    std::fs::write(&outer, outer_text(4.0, 8.0)).unwrap();

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &outer, bevy::math::Vec3::ZERO);
    assert!((live_x(&app, 5, "translation.x") - 4.0).abs() < 1e-4);
    assert!((live_x(&app, 7, "translation.x") - 8.0).abs() < 1e-4);

    let text = saved_scene_text(&mut app);
    assert!(saved_field(&text, 5, "").is_none(), "got {text}");
    assert!(saved_field(&text, 7, "").is_none(), "got {text}");
    assert!(saved_instance_field(&text, "scale").is_none(), "got {text}");

    let reopened = reopen_with_outer_text(&text, &inner, &outer, &outer_text(6.0, 9.0));
    assert!((live_x(&reopened, 5, "translation.x") - 6.0).abs() < 1e-4);
    assert!((live_x(&reopened, 7, "translation.x") - 9.0).abs() < 1e-4);
}

/// Text of a top prefab that places the outer prefab as its child (id 3) at
/// `child_x` and moves the knob two prefabs down to `knob_x`.
fn top_prefab_text(outer: &std::path::Path, child_x: f64, knob_x: f64) -> String {
    format!(
        "#top\n\
         jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         bevy_ecs::hierarchy::Children [\n\
             jackdaw::prefab::components::IsA {{ source: \"{}\", deleted: [] }}\n\
             jackdaw::prefab::components::PrefabEntityId(3)\n\
             bevy_transform::components::transform::Transform {{ translation: glam::Vec3 {{ x: {child_x:?}, y: 0.0, z: 0.0 }} }}\n\
             bevy_ecs::hierarchy::Children [\n\
                 jackdaw::prefab::components::PrefabEntityId(5)\n\
                 bevy_ecs::hierarchy::Children [\n\
                     jackdaw::prefab::components::PrefabEntityId(7)\n\
                     bevy_transform::components::transform::Transform {{ translation: glam::Vec3 {{ x: {knob_x:?}, y: 0.0, z: 0.0 }} }}\n\
                 ]\n\
             ]\n\
         ]",
        outer.to_slash_lossy()
    )
}

#[test]
fn three_levels_of_nested_prefabs_save_none_of_their_values_and_follow_edits() {
    let tmp = tempfile::tempdir().unwrap();
    let (inner, outer) = write_nesting_prefabs(tmp.path());
    let top = tmp.path().join("top.bsn");
    std::fs::write(&top, top_prefab_text(&outer, 1.0, 20.0)).unwrap();

    let mut app = make_app_for_prefab_tests();
    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &top, bevy::math::Vec3::ZERO);
    assert!((live_x(&app, 3, "translation.x") - 1.0).abs() < 1e-4);
    assert!((live_x(&app, 5, "translation.x") - 4.0).abs() < 1e-4);
    assert!((live_x(&app, 5, "scale.x") - 2.0).abs() < 1e-4);
    assert!((live_x(&app, 7, "translation.x") - 20.0).abs() < 1e-4);

    let text = saved_scene_text(&mut app);
    for id in [3, 5, 7] {
        assert!(saved_field(&text, id, "").is_none(), "id {id}: got {text}");
    }

    let edit = |app: &mut App, path: &std::path::Path, body: String| {
        let sparse = jackdaw::prefab::watcher::capture_sparse_scene_text(app.world_mut())
            .expect("a live document to capture");
        app.world_mut()
            .resource_mut::<jackdaw::prefab::PrefabAstCache>()
            .insert(path, bsn_ast(&body));
        jackdaw::prefab::watcher::respawn_from_sparse_text(app.world_mut(), &sparse);
    };
    edit(&mut app, &outer, nesting_prefab_text(&inner, 6.0, 8.0));
    assert!((live_x(&app, 5, "translation.x") - 6.0).abs() < 1e-4);
    assert!((live_x(&app, 7, "translation.x") - 20.0).abs() < 1e-4);
    edit(&mut app, &top, top_prefab_text(&outer, 1.0, 30.0));
    assert!((live_x(&app, 7, "translation.x") - 30.0).abs() < 1e-4);
    assert!((live_x(&app, 5, "translation.x") - 6.0).abs() < 1e-4);

    let text = saved_scene_text(&mut app);
    for id in [3, 5, 7] {
        assert!(saved_field(&text, id, "").is_none(), "id {id}: got {text}");
    }
}
