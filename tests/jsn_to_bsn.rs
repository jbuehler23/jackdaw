//! Converter tests: a legacy `.jsn` scene converts to `.bsn` text that loads
//! into an ECS world semantically equal to what the JSN load path produces.

use std::collections::BTreeMap;
use std::path::Path;

use bevy::asset::AssetPlugin;
use bevy::prelude::*;

use jackdaw::jsn_to_bsn::{convert_jsn_scene_to_bsn, convert_jsn_text};
use jackdaw::scene_io::{load_inline_assets, load_scene_from_jsn};
use jackdaw_bsn::{apply_dirty_ast_patches, parse_bsn_text, spawn_from_ast};
use jackdaw_scene_types::{CustomProperties, PropertyValue, SceneNodeId};

fn headless_app() -> App {
    // Minimal plugin set: the converter needs the type registry, an asset
    // server, and the jackdaw scene plugins. bevy_render is deliberately
    // absent (its component sync hooks require a live render world).
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()));
    app.add_plugins(jackdaw_jsn::JsnPlugin {
        runtime_mesh_rebuild: false,
    });
    app.add_plugins(jackdaw_bsn::JackdawBsnPlugin);
    app
}

/// Spawn the converted `.bsn` text into a world and return the entities.
fn spawn_bsn(app: &mut App, bsn_text: &str) -> Vec<Entity> {
    let ast = parse_bsn_text(bsn_text).expect("converted .bsn parses");
    app.world_mut().insert_resource(ast);
    let spawned = spawn_from_ast(app.world_mut());
    apply_dirty_ast_patches(app.world_mut());
    spawned
}

fn find_by_name(world: &mut World, name: &str) -> Option<Entity> {
    let mut query = world.query::<(Entity, &Name)>();
    query
        .iter(world)
        .find(|(_, n)| n.as_str() == name)
        .map(|(e, _)| e)
}

fn find_by_node_id(world: &mut World, id: SceneNodeId) -> Option<Entity> {
    let mut query = world.query::<(Entity, &SceneNodeId)>();
    query.iter(world).find(|(_, n)| **n == id).map(|(e, _)| e)
}

#[test]
fn real_scene_with_legacy_type_paths_converts_semantically() {
    let text = include_str!("fixtures/jsn_to_bsn/real_scene.jsn");

    // World A: the JSN load path (with canonicalized legacy type paths).
    let mut app_a = headless_app();
    let mut scene: jackdaw_jsn::JsnScene = serde_json::from_str(text).expect("fixture parses");
    jackdaw_jsn::format::canonicalize_scene(&mut scene);
    let local_assets = load_inline_assets(app_a.world_mut(), &scene.assets, Path::new(""));
    let spawned_a = load_scene_from_jsn(app_a.world_mut(), &scene.scene, Path::new(""), &local_assets);
    assert_eq!(spawned_a.len(), 2, "fixture has two entities");

    // Convert (from raw text, exercising the parse + canonicalize boundary).
    let mut app_c = headless_app();
    let converted = convert_jsn_text(app_c.world_mut(), text).expect("conversion succeeds");
    assert_eq!(converted.report.entity_count, 2);
    assert!(
        converted.scene_bsn.contains("jackdaw_scene_types::types::Brush"),
        "legacy Brush type path must convert to its current path:\n{}",
        converted.scene_bsn
    );

    // World B: the converted text through the BSN load path.
    let mut app_b = headless_app();
    let spawned_b = spawn_bsn(&mut app_b, &converted.scene_bsn);
    assert_eq!(spawned_b.len(), 2, "converted scene spawns both entities");

    // The lit entity: light parameters and transform survive.
    let sun_a = find_by_name(app_a.world_mut(), "Sun").expect("light in world A");
    let sun_b = find_by_name(app_b.world_mut(), "Sun").expect("light in world B");
    let lum_a = app_a.world().get::<DirectionalLight>(sun_a).expect("light A").illuminance;
    let lum_b = app_b.world().get::<DirectionalLight>(sun_b).expect("light B").illuminance;
    assert!((lum_a - lum_b).abs() < 1e-3, "illuminance {lum_a} vs {lum_b}");
    let t_a = app_a.world().get::<Transform>(sun_a).expect("transform A").translation;
    let t_b = app_b.world().get::<Transform>(sun_b).expect("transform B").translation;
    assert!((t_a - t_b).length() < 1e-5, "translation {t_a} vs {t_b}");

    // The brush entity: the Brush component (legacy type path) survives with
    // its geometry intact.
    let brush_a = find_by_name(app_a.world_mut(), "Brush").expect("brush in world A");
    let brush_b = find_by_name(app_b.world_mut(), "Brush").expect("brush in world B");
    let faces_a = app_a
        .world()
        .get::<jackdaw_scene_types::Brush>(brush_a)
        .expect("brush component A")
        .faces
        .len();
    let faces_b = app_b
        .world()
        .get::<jackdaw_scene_types::Brush>(brush_b)
        .expect("brush component B")
        .faces
        .len();
    assert_eq!(faces_a, faces_b, "face count survives conversion");
    assert!(faces_a > 0, "fixture brush has faces");
}

#[test]
fn hierarchy_node_ids_and_custom_properties_round_trip() {
    // Author a scene in ECS, serialize it to JSN (the editor's own writer),
    // convert that, and compare the two loaded worlds.
    let mut app_a = headless_app();

    let parent_id = SceneNodeId::next();
    let child_id = SceneNodeId::next();

    let mut props = BTreeMap::new();
    props.insert("hp".to_string(), PropertyValue::Int(7));
    props.insert("title".to_string(), PropertyValue::String("Boss".into()));

    let parent = app_a
        .world_mut()
        .spawn((
            Name::new("Parent"),
            Transform::from_xyz(1.0, 2.0, 3.0),
            Visibility::default(),
            parent_id,
            CustomProperties { properties: props },
            jackdaw_scene_types::SceneRootTag,
        ))
        .id();
    app_a.world_mut().spawn((
        Name::new("Child"),
        Transform::from_xyz(5.0, 0.0, 0.0),
        Visibility::default(),
        child_id,
        ChildOf(parent),
    ));

    let scene = jackdaw::scene_io::serialize_world_to_jsn_scene(app_a.world_mut());
    assert!(scene.scene.len() >= 2, "both entities serialize");

    let mut app_c = headless_app();
    let converted =
        convert_jsn_scene_to_bsn(app_c.world_mut(), &scene).expect("conversion succeeds");

    let mut app_b = headless_app();
    spawn_bsn(&mut app_b, &converted.scene_bsn);

    // Match by stable node id; compare values.
    let parent_b = find_by_node_id(app_b.world_mut(), parent_id).expect("parent by node id");
    let child_b = find_by_node_id(app_b.world_mut(), child_id).expect("child by node id");

    let pt = app_b.world().get::<Transform>(parent_b).expect("parent transform").translation;
    assert!((pt - Vec3::new(1.0, 2.0, 3.0)).length() < 1e-5, "parent translation {pt}");
    let ct = app_b.world().get::<Transform>(child_b).expect("child transform").translation;
    assert!((ct - Vec3::new(5.0, 0.0, 0.0)).length() < 1e-5, "child translation {ct}");

    assert_eq!(
        app_b.world().get::<ChildOf>(child_b).map(|c| c.parent()),
        Some(parent_b),
        "hierarchy survives conversion"
    );

    let props_b = app_b
        .world()
        .get::<CustomProperties>(parent_b)
        .expect("custom properties survive");
    assert_eq!(props_b.properties.get("hp"), Some(&PropertyValue::Int(7)));
    assert_eq!(
        props_b.properties.get("title"),
        Some(&PropertyValue::String("Boss".into()))
    );
}

#[test]
fn legacy_v2_scene_converts() {
    // Minimal v2 layout: structural name/transform fields instead of the v3
    // components map.
    let v2 = r#"{
        "jsn": {"format_version": [2, 0, 0], "editor_version": "0.4.0", "bevy_version": "0.18"},
        "metadata": {"name": "old"},
        "assets": {},
        "editor": null,
        "scene": [{
            "name": "Old",
            "transform": {
                "translation": [1.0, 2.0, 3.0],
                "rotation": [0.0, 0.0, 0.0, 1.0],
                "scale": [1.0, 1.0, 1.0]
            },
            "components": {}
        }]
    }"#;

    let mut app_c = headless_app();
    let converted = convert_jsn_text(app_c.world_mut(), v2).expect("v2 converts");
    assert_eq!(converted.report.entity_count, 1);

    let mut app_b = headless_app();
    spawn_bsn(&mut app_b, &converted.scene_bsn);
    let old = find_by_name(app_b.world_mut(), "Old").expect("v2 entity present");
    let t = app_b.world().get::<Transform>(old).expect("transform").translation;
    assert!((t - Vec3::new(1.0, 2.0, 3.0)).length() < 1e-5, "translation {t}");
}

#[test]
fn conversion_is_deterministic() {
    let text = include_str!("fixtures/jsn_to_bsn/real_scene.jsn");
    let mut app = headless_app();
    let first = convert_jsn_text(app.world_mut(), text).expect("first conversion");
    let mut app2 = headless_app();
    let second = convert_jsn_text(app2.world_mut(), text).expect("second conversion");
    // Node ids are minted fresh per conversion (the fixture has none), so
    // strip the id patches before comparing the remaining text.
    let strip = |s: &str| {
        s.lines()
            .filter(|line| !line.contains("SceneNodeId"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(strip(&first.scene_bsn), strip(&second.scene_bsn));
}
