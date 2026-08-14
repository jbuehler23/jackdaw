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

/// Serializer processor for fixture building: emits `Handle<T>` fields as
/// their asset path (or null for runtime handles) and `Entity` fields as
/// null, the shape the legacy JSON scene writer produced.
struct FixtureSerializerProcessor;

impl bevy::reflect::serde::ReflectSerializerProcessor for FixtureSerializerProcessor {
    fn try_serialize<S>(
        &self,
        value: &dyn bevy::reflect::PartialReflect,
        registry: &bevy::reflect::TypeRegistry,
        serializer: S,
    ) -> Result<Result<S::Ok, S>, S::Error>
    where
        S: serde::Serializer,
    {
        use bevy::asset::ReflectHandle;
        let Some(value) = value.try_as_reflect() else {
            return Ok(Err(serializer));
        };
        let type_id = value.reflect_type_info().type_id();
        if let Some(reflect_handle) = registry.get_type_data::<ReflectHandle>(type_id) {
            let untyped_handle = reflect_handle
                .downcast_handle_untyped(value.as_any())
                .expect("must be a handle");
            if let Some(path) = untyped_handle.path() {
                let path_str = path.path().to_string_lossy().into_owned();
                return Ok(Ok(serde::Serializer::serialize_str(serializer, &path_str)?));
            }
            return Ok(Ok(serde::Serializer::serialize_unit(serializer)?));
        }
        if type_id == std::any::TypeId::of::<Entity>() {
            return Ok(Ok(serde::Serializer::serialize_unit(serializer)?));
        }
        Ok(Err(serializer))
    }
}

/// Build a legacy `JsnScene` fixture from a world, the shape the editor's
/// removed JSON writer used to produce: one entry per `Name`-bearing entity
/// in spawn order, components reflect-serialized, `SceneNodeId` lifted to
/// the structural `id` field.
fn world_to_jsn_scene(world: &mut World) -> jackdaw_jsn::JsnScene {
    use bevy::ecs::reflect::ReflectComponent;
    use bevy::reflect::serde::TypedReflectSerializer;
    use std::any::TypeId;
    use std::collections::HashMap;

    let mut query = world.query_filtered::<Entity, With<Name>>();
    let mut entities: Vec<Entity> = query.iter(world).collect();
    entities.sort_unstable();
    let index_of: HashMap<Entity, usize> =
        entities.iter().enumerate().map(|(i, &e)| (e, i)).collect();

    let registry = world
        .resource::<bevy::ecs::reflect::AppTypeRegistry>()
        .clone();
    let registry = registry.read();
    let skip_ids = [
        TypeId::of::<GlobalTransform>(),
        TypeId::of::<InheritedVisibility>(),
        TypeId::of::<ViewVisibility>(),
        TypeId::of::<ChildOf>(),
        TypeId::of::<Children>(),
        TypeId::of::<SceneNodeId>(),
    ];
    let processor = FixtureSerializerProcessor;

    let scene: Vec<jackdaw_jsn::format::JsnEntity> = entities
        .iter()
        .map(|&entity| {
            let entity_ref = world.entity(entity);
            let parent = entity_ref
                .get::<ChildOf>()
                .and_then(|c| index_of.get(&c.parent()).copied());
            let id = entity_ref.get::<SceneNodeId>().map(|n| n.0);
            let mut components = HashMap::new();
            for registration in registry.iter() {
                if skip_ids.contains(&registration.type_id()) {
                    continue;
                }
                let type_path = registration.type_info().type_path_table().path();
                if jackdaw::scene_io::should_skip_component(type_path) {
                    continue;
                }
                let Some(reflect_component) = registration.data::<ReflectComponent>() else {
                    continue;
                };
                let Some(component) = reflect_component.reflect(entity_ref) else {
                    continue;
                };
                let serializer =
                    TypedReflectSerializer::with_processor(component, &registry, &processor);
                if let Ok(value) = serde_json::to_value(&serializer) {
                    components.insert(type_path.to_string(), value);
                }
            }
            jackdaw_jsn::format::JsnEntity {
                id,
                parent,
                components,
            }
        })
        .collect();

    jackdaw_jsn::JsnScene {
        jsn: jackdaw_jsn::format::JsnHeader::default(),
        metadata: jackdaw_jsn::format::JsnMetadata::default(),
        assets: jackdaw_jsn::format::JsnAssets::default(),
        editor: None,
        scene,
    }
}

fn headless_app() -> App {
    // Minimal plugin set: the converter needs the type registry, an asset
    // server, and the jackdaw scene plugins. bevy_render is deliberately
    // absent (its component sync hooks require a live render world).
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()));
    app.add_plugins(jackdaw_scene_types::SceneTypesPlugin {
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
    let text = include_str!("fixtures/jsn_to_bsn/real_scene.jsn.bak");

    // World A: the JSN load path (with canonicalized legacy type paths).
    let mut app_a = headless_app();
    let mut scene: jackdaw_jsn::JsnScene = serde_json::from_str(text).expect("fixture parses");
    jackdaw_jsn::format::canonicalize_scene(&mut scene);
    let local_assets = load_inline_assets(app_a.world_mut(), &scene.assets, Path::new(""));
    let spawned_a = load_scene_from_jsn(
        app_a.world_mut(),
        &scene.scene,
        Path::new(""),
        &local_assets,
    );
    assert_eq!(spawned_a.len(), 2, "fixture has two entities");

    // Convert (from raw text, exercising the parse + canonicalize boundary).
    let mut app_c = headless_app();
    let converted = convert_jsn_text(app_c.world_mut(), text).expect("conversion succeeds");
    assert_eq!(converted.report.entity_count, 2);
    assert!(
        converted
            .scene_bsn
            .contains("jackdaw_scene_types::types::Brush"),
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
    let lum_a = app_a
        .world()
        .get::<DirectionalLight>(sun_a)
        .expect("light A")
        .illuminance;
    let lum_b = app_b
        .world()
        .get::<DirectionalLight>(sun_b)
        .expect("light B")
        .illuminance;
    assert!(
        (lum_a - lum_b).abs() < 1e-3,
        "illuminance {lum_a} vs {lum_b}"
    );
    let t_a = app_a
        .world()
        .get::<Transform>(sun_a)
        .expect("transform A")
        .translation;
    let t_b = app_b
        .world()
        .get::<Transform>(sun_b)
        .expect("transform B")
        .translation;
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

    let scene = world_to_jsn_scene(app_a.world_mut());
    assert!(scene.scene.len() >= 2, "both entities serialize");

    let mut app_c = headless_app();
    let converted =
        convert_jsn_scene_to_bsn(app_c.world_mut(), &scene).expect("conversion succeeds");

    let mut app_b = headless_app();
    spawn_bsn(&mut app_b, &converted.scene_bsn);

    // Match by stable node id; compare values.
    let parent_b = find_by_node_id(app_b.world_mut(), parent_id).expect("parent by node id");
    let child_b = find_by_node_id(app_b.world_mut(), child_id).expect("child by node id");

    let pt = app_b
        .world()
        .get::<Transform>(parent_b)
        .expect("parent transform")
        .translation;
    assert!(
        (pt - Vec3::new(1.0, 2.0, 3.0)).length() < 1e-5,
        "parent translation {pt}"
    );
    let ct = app_b
        .world()
        .get::<Transform>(child_b)
        .expect("child transform")
        .translation;
    assert!(
        (ct - Vec3::new(5.0, 0.0, 0.0)).length() < 1e-5,
        "child translation {ct}"
    );

    assert_eq!(
        app_b
            .world()
            .get::<ChildOf>(child_b)
            .map(bevy::prelude::ChildOf::parent),
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
    let t = app_b
        .world()
        .get::<Transform>(old)
        .expect("transform")
        .translation;
    assert!(
        (t - Vec3::new(1.0, 2.0, 3.0)).length() < 1e-5,
        "translation {t}"
    );
}

#[test]
fn conversion_is_deterministic() {
    let text = include_str!("fixtures/jsn_to_bsn/real_scene.jsn.bak");
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

#[test]
fn inline_material_reference_and_terrain_survive_conversion() {
    use jackdaw_scene_types::Terrain;

    // Author a scene with a Terrain component, then hand it an inline
    // material asset and a brush face that references it by name, the way the
    // editor's save path stores catalog-less materials.
    //
    // The terrain here is deliberately a *legacy* one: heights inline on
    // the component and no `data_path`, which is what every scene written
    // before the binary sidecar looks like. Conversion has to carry those
    // heights through untouched -- the editor drains them into the sidecar
    // store on load, and it can only do that if they survived the trip.
    let mut app_a = headless_app();
    let node_id = SceneNodeId::next();
    app_a.world_mut().spawn((
        Name::new("Ground"),
        Transform::default(),
        node_id,
        jackdaw_scene_types::Brush::cuboid(1.0, 1.0, 1.0),
        Terrain {
            resolution: 3,
            size: Vec2::new(8.0, 8.0),
            max_height: 2.5,
            channels: Vec::new(),
            data_path: String::new(),
            heights: vec![0.0, 0.5, 1.0, 0.0, 0.25, 0.75, 0.1, 0.2, 0.3],
            // A scene this old predates quantization, so it carries the
            // default. Spelled out rather than `..default()` because the
            // point of this literal is to enumerate what a legacy scene
            // holds.
            quantization: Default::default(),
        },
        jackdaw_scene_types::SceneRootTag,
    ));
    let mut scene = world_to_jsn_scene(app_a.world_mut());

    // Inline material asset entry, reflect-serialized like the editor writes.
    let material_json = {
        let registry = app_a
            .world()
            .resource::<bevy::ecs::reflect::AppTypeRegistry>()
            .clone();
        let reg = registry.read();
        let material = StandardMaterial {
            base_color: Color::srgb(0.8, 0.1, 0.1),
            ..Default::default()
        };
        let serializer =
            bevy::reflect::serde::TypedReflectSerializer::new(material.as_partial_reflect(), &reg);
        serde_json::to_value(serializer).expect("material serializes")
    };
    scene.assets.0.insert(
        "bevy_pbr::pbr_material::StandardMaterial".to_string(),
        std::collections::HashMap::from([("#RedMat".to_string(), material_json)]),
    );

    // Point the first brush face's material at the inline asset by name.
    let entity = scene
        .scene
        .iter_mut()
        .find(|e| {
            e.components
                .contains_key("jackdaw_scene_types::types::Brush")
        })
        .expect("brush entity serialized");
    let brush = entity
        .components
        .get_mut("jackdaw_scene_types::types::Brush")
        .unwrap();
    brush["faces"][0]["material"] = serde_json::Value::String("#RedMat".to_string());

    let mut app_c = headless_app();
    let converted =
        convert_jsn_scene_to_bsn(app_c.world_mut(), &scene).expect("conversion succeeds");

    // The scene text carries the reference name (not an empty string) on the
    // face, and embeds the material definition as a named asset root.
    assert!(
        converted.scene_bsn.contains("\"#RedMat\""),
        "face material must emit its inline reference name:\n{}",
        converted.scene_bsn
    );
    assert!(
        converted.scene_bsn.contains("#RedMat\n")
            && converted.scene_bsn.contains("StandardMaterial"),
        "scene must embed the inline material definition:\n{}",
        converted.scene_bsn
    );
    assert_eq!(converted.report.asset_count, 1);

    // Terrain round-trips semantically into the BSN-loaded world: the
    // small descriptive fields, the (empty) sidecar path, and the legacy
    // inline heights the editor migrates on next load.
    let mut app_b = headless_app();
    spawn_bsn(&mut app_b, &converted.scene_bsn);
    let ground = find_by_node_id(app_b.world_mut(), node_id).expect("ground by node id");
    let terrain = app_b
        .world()
        .get::<Terrain>(ground)
        .expect("terrain survives");
    assert_eq!(terrain.resolution, 3);
    assert!((terrain.max_height - 2.5).abs() < 1e-6);
    assert!((terrain.size - Vec2::new(8.0, 8.0)).length() < 1e-6);
    assert_eq!(
        terrain.data_path, "",
        "a legacy terrain names no sidecar; the editor mints one on load"
    );
    assert_eq!(terrain.heights.len(), 9);
    assert!((terrain.heights[2] - 1.0).abs() < 1e-6);
    assert!(
        !terrain.quantization.enabled,
        "a scene that never mentioned quantization comes back with it off"
    );
}

/// A terrain's quantization is a small reflected descriptor, so it has to
/// survive the scene text the way `resolution` does. Without this the
/// setting would look like it worked and silently reset on next load.
#[test]
fn terrain_quantization_survives_conversion() {
    use jackdaw_scene_types::{Terrain, TerrainQuantization};

    let mut app_a = headless_app();
    let node_id = SceneNodeId::next();
    app_a.world_mut().spawn((
        Name::new("Ground"),
        Transform::default(),
        node_id,
        Terrain {
            resolution: 5,
            quantization: TerrainQuantization {
                enabled: true,
                cell_size: 1.5,
                height_step: 0.25,
            },
            ..Default::default()
        },
        jackdaw_scene_types::SceneRootTag,
    ));
    let scene = world_to_jsn_scene(app_a.world_mut());

    let mut app_c = headless_app();
    let converted =
        convert_jsn_scene_to_bsn(app_c.world_mut(), &scene).expect("conversion succeeds");

    let mut app_b = headless_app();
    spawn_bsn(&mut app_b, &converted.scene_bsn);
    let ground = find_by_node_id(app_b.world_mut(), node_id).expect("ground by node id");
    let quantization = app_b
        .world()
        .get::<Terrain>(ground)
        .expect("terrain survives")
        .quantization
        .clone();

    assert!(quantization.enabled);
    assert!((quantization.cell_size - 1.5).abs() < 1e-6);
    assert!((quantization.height_step - 0.25).abs() < 1e-6);
}

#[test]
fn convert_project_walks_scenes_prefabs_and_catalog() {
    use jackdaw::jsn_to_bsn::convert_project;

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("assets/scenes")).unwrap();
    std::fs::create_dir_all(root.join("assets/prefabs")).unwrap();
    std::fs::create_dir_all(root.join(".jsn")).unwrap();

    // Two small scenes authored via the editor's own serializer.
    let mut app = headless_app();
    app.world_mut().spawn((
        Name::new("LevelRoot"),
        Transform::from_xyz(1.0, 0.0, 0.0),
        SceneNodeId::next(),
        jackdaw_scene_types::SceneRootTag,
    ));
    let scene = world_to_jsn_scene(app.world_mut());
    let scene_json = serde_json::to_string(&scene).unwrap();
    std::fs::write(root.join("assets/scenes/level.jsn"), &scene_json).unwrap();
    std::fs::write(root.join("assets/prefabs/thing.jsn"), &scene_json).unwrap();

    // A catalog with one project-wide material.
    let material_json = {
        let registry = app
            .world()
            .resource::<bevy::ecs::reflect::AppTypeRegistry>()
            .clone();
        let reg = registry.read();
        let material = StandardMaterial {
            base_color: Color::srgb(0.2, 0.9, 0.2),
            ..Default::default()
        };
        let serializer =
            bevy::reflect::serde::TypedReflectSerializer::new(material.as_partial_reflect(), &reg);
        serde_json::to_value(serializer).unwrap()
    };
    let catalog = serde_json::json!({
        "jsn": {"format_version": [3, 0, 0], "editor_version": "0.5.0", "bevy_version": "0.19"},
        "assets": {
            "bevy_pbr::pbr_material::StandardMaterial": {"@GreenMat": material_json}
        }
    });
    std::fs::write(
        root.join(".jsn/catalog.jsn"),
        serde_json::to_string(&catalog).unwrap(),
    )
    .unwrap();

    // Config and backups that must be left alone.
    std::fs::write(root.join(".jsn/project.jsn"), "{\"name\": \"t\"}").unwrap();
    std::fs::write(root.join("assets/old.jsn.bak"), "old backup").unwrap();

    let mut convert_app = headless_app();
    let report = convert_project(convert_app.world_mut(), root);

    assert_eq!(report.failures.len(), 0, "failures: {:?}", report.failures);
    assert_eq!(report.scenes.len(), 2, "level + prefab converted");
    assert_eq!(report.catalogs.len(), 1);

    // Converted outputs exist; sources renamed to .jsn.bak.
    assert!(root.join("assets/scenes/level.bsn").is_file());
    assert!(root.join("assets/scenes/level.jsn.bak").is_file());
    assert!(!root.join("assets/scenes/level.jsn").exists());
    assert!(root.join("assets/prefabs/thing.bsn").is_file());
    assert!(root.join(".jsn/catalog.bsn").is_file());
    assert!(root.join(".jsn/catalog.jsn.bak").is_file());

    // Config untouched; pre-existing backup untouched.
    assert!(root.join(".jsn/project.jsn").is_file());
    assert_eq!(
        std::fs::read_to_string(root.join("assets/old.jsn.bak")).unwrap(),
        "old backup"
    );

    // The converted catalog holds the material under its reference name.
    let catalog_bsn = std::fs::read_to_string(root.join(".jsn/catalog.bsn")).unwrap();
    assert!(catalog_bsn.contains("GreenMat"), "{catalog_bsn}");
    assert!(catalog_bsn.contains("StandardMaterial"), "{catalog_bsn}");

    // The converted scene parses and spawns.
    let level_bsn = std::fs::read_to_string(root.join("assets/scenes/level.bsn")).unwrap();
    let mut app_b = headless_app();
    let spawned = spawn_bsn(&mut app_b, &level_bsn);
    assert_eq!(spawned.len(), 1);
    assert!(find_by_name(app_b.world_mut(), "LevelRoot").is_some());

    // A second run finds nothing left to convert.
    let mut app_c = headless_app();
    let second = convert_project(app_c.world_mut(), root);
    assert_eq!(second.scenes.len(), 0);
    assert_eq!(second.catalogs.len(), 0);
    assert_eq!(second.failures.len(), 0);
}

#[test]
fn editor_opens_and_saves_bsn_scenes() {
    use jackdaw::scene_io::{SceneFilePath, load_scene_from_file, save_scene};

    // Author + convert a scene to .bsn on disk.
    let dir = tempfile::tempdir().expect("tempdir");
    let bsn_path = dir.path().join("level.bsn");
    {
        let mut app = headless_app();
        let id = SceneNodeId::next();
        app.world_mut().spawn((
            Name::new("Hero"),
            Transform::from_xyz(4.0, 0.0, 0.0),
            id,
            jackdaw_scene_types::SceneRootTag,
        ));
        let scene = world_to_jsn_scene(app.world_mut());
        let converted = convert_jsn_scene_to_bsn(app.world_mut(), &scene).expect("converts");
        std::fs::write(&bsn_path, &converted.scene_bsn).unwrap();
    }

    fn editor_app() -> App {
        let mut app = headless_app();
        app.init_resource::<jackdaw::scene_io::SceneFilePath>();
        app.init_resource::<jackdaw::scene_io::SceneDirtyState>();
        app.init_resource::<jackdaw::selection::Selection>();
        app.init_resource::<jackdaw_commands::CommandHistory>();
        app
    }

    // Open the .bsn in the editor load path.
    let mut app = editor_app();
    load_scene_from_file(app.world_mut(), &bsn_path);
    let hero = find_by_name(app.world_mut(), "Hero").expect("hero loaded from .bsn");
    let x = app.world().get::<Transform>(hero).unwrap().translation.x;
    assert!((x - 4.0).abs() < 1e-6);
    assert!(
        !app.world()
            .resource::<jackdaw_bsn::SceneBsnAst>()
            .roots
            .is_empty(),
        "opening .bsn fills the live BSN document so tabs/undo/save keep working"
    );
    assert_eq!(
        app.world().resource::<SceneFilePath>().path.as_deref(),
        Some(bsn_path.to_string_lossy().as_ref())
    );

    // Save back to the same .bsn (async IO pool write; poll for it). The
    // save path comes from the active tab.
    {
        let mut scenes = jackdaw::scenes::Scenes::default();
        let mut tab = jackdaw::scenes::SceneTab::new_untitled(1);
        tab.path = Some(bsn_path.clone());
        scenes.push_tab(tab);
        app.world_mut().insert_resource(scenes);
    }
    let before = std::fs::read_to_string(&bsn_path).unwrap();
    std::fs::remove_file(&bsn_path).unwrap();
    save_scene(app.world_mut());
    let mut waited = 0;
    while !bsn_path.exists() && waited < 200 {
        std::thread::sleep(std::time::Duration::from_millis(10));
        waited += 1;
    }
    let after = std::fs::read_to_string(&bsn_path).expect("save wrote the .bsn");
    assert!(
        after.contains("#Hero"),
        "saved text is BSN, not JSON:\n{after}"
    );
    // Saving prepends a `// jackdaw <ver> | bevy <ver>` provenance stamp at the
    // disk boundary; strip it before comparing the round-tripped content.
    let after_body: String = after
        .lines()
        .skip_while(|l| l.starts_with("// jackdaw"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        before.trim(),
        after_body.trim(),
        "load then save round-trips"
    );

    // Reload the saved file into a fresh editor world.
    let mut app2 = editor_app();
    load_scene_from_file(app2.world_mut(), &bsn_path);
    assert!(find_by_name(app2.world_mut(), "Hero").is_some());
}

#[test]
fn catalog_round_trips_as_bsn() {
    use jackdaw::asset_catalog::{AssetCatalog, load_catalog, save_catalog};
    use jackdaw::project::ProjectRoot;

    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join(".jsn")).unwrap();

    fn project_app(root: &std::path::Path) -> App {
        let mut app = headless_app();
        app.init_resource::<AssetCatalog>();
        app.world_mut().insert_resource(ProjectRoot::new(
            root.to_path_buf(),
            jackdaw::project::ProjectConfig::default(),
        ));
        app
    }

    // Populate a catalog with a live material and save it.
    let mut app = project_app(dir.path());
    let handle = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            metallic: 0.75,
            ..Default::default()
        })
        .untyped();
    {
        let mut catalog = app.world_mut().resource_mut::<AssetCatalog>();
        catalog.insert("@Steel".to_string(), handle);
        catalog.dirty = true;
    }
    save_catalog(app.world_mut());

    let catalog_path = dir.path().join("assets/catalog.bsn");
    let text = std::fs::read_to_string(&catalog_path).expect("catalog.bsn written");
    assert!(text.contains("#Steel"), "{text}");
    assert!(text.contains("metallic"), "{text}");

    // A fresh app loads it back and exposes the @Name handle.
    let mut app2 = project_app(dir.path());
    load_catalog(app2.world_mut());
    let catalog = app2.world().resource::<AssetCatalog>();
    let loaded = catalog.handles.get("@Steel").expect("catalog entry loaded");
    let material = app2
        .world()
        .resource::<Assets<StandardMaterial>>()
        .get(&loaded.clone().typed::<StandardMaterial>())
        .expect("material in store");
    assert!((material.metallic - 0.75).abs() < 1e-6);
}

#[test]
fn prefab_cache_reads_bsn_prefabs_with_stale_extension_references() {
    use jackdaw::prefab::PrefabAstCache;
    use jackdaw::prefab::save_load::populate_cache_for_scene_bsn;

    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("prefabs")).unwrap();

    // A .bsn prefab baseline on disk.
    let mut app = headless_app();
    app.world_mut().spawn((
        Name::new("Crate"),
        Transform::from_xyz(0.0, 0.5, 0.0),
        SceneNodeId::next(),
        jackdaw_scene_types::SceneRootTag,
    ));
    let scene = world_to_jsn_scene(app.world_mut());
    let mut app_c = headless_app();
    let bsn = convert_jsn_scene_to_bsn(app_c.world_mut(), &scene)
        .expect("converts")
        .scene_bsn;
    std::fs::write(dir.path().join("prefabs/crate.bsn"), &bsn).unwrap();

    // A scene document whose IsA still references the pre-conversion .jsn
    // path.
    let ast =
        parse_bsn_text("jackdaw::prefab::components::IsA { source: \"prefabs/crate.jsn\" }\n")
            .expect("scene document parses");

    let mut cache = PrefabAstCache::default();
    populate_cache_for_scene_bsn(&ast, &mut cache, dir.path());

    let cached = cache
        .get(&dir.path().join("prefabs/crate.bsn"))
        .or_else(|| cache.get(&dir.path().join("prefabs/crate.jsn")))
        .expect("stale .jsn reference resolves to the .bsn sibling");
    assert_eq!(cached.roots.len(), 1);
    assert!(
        cached
            .find_patch_by_type_path(
                cached.roots[0],
                "bevy_transform::components::transform::Transform"
            )
            .is_some(),
        "prefab baseline components survive the bridge"
    );
}

#[test]
fn migration_prompt_detects_converts_and_respects_decline() {
    use jackdaw::migrate_dialog::{PendingMigration, count_legacy_files, resolve_migration};
    use jackdaw::project::ProjectRoot;

    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("assets/scenes")).unwrap();
    std::fs::create_dir_all(dir.path().join(".jsn")).unwrap();

    let mut app = headless_app();
    app.world_mut().spawn((
        Name::new("Thing"),
        Transform::default(),
        SceneNodeId::next(),
        jackdaw_scene_types::SceneRootTag,
    ));
    let scene = world_to_jsn_scene(app.world_mut());
    let scene_json = serde_json::to_string(&scene).unwrap();
    std::fs::write(dir.path().join("assets/scenes/level.jsn"), &scene_json).unwrap();
    // Config must not count as a legacy scene file.
    std::fs::write(dir.path().join(".jsn/project.jsn"), "{}").unwrap();

    assert_eq!(count_legacy_files(dir.path()), 1);

    let mut editor = headless_app();
    editor.init_resource::<PendingMigration>();
    editor.world_mut().insert_resource(ProjectRoot::new(
        dir.path().to_path_buf(),
        jackdaw::project::ProjectConfig::default(),
    ));

    // Decline: files stay, prompt state clears, detection still fires.
    editor
        .world_mut()
        .resource_mut::<PendingMigration>()
        .file_count = Some(1);
    resolve_migration(editor.world_mut(), false);
    assert!(dir.path().join("assets/scenes/level.jsn").exists());
    assert!(
        editor
            .world()
            .resource::<PendingMigration>()
            .file_count
            .is_none()
    );
    assert_eq!(count_legacy_files(dir.path()), 1, "still prompts next open");

    // Accept: project converts, backups kept, nothing left to prompt about.
    editor
        .world_mut()
        .resource_mut::<PendingMigration>()
        .file_count = Some(1);
    resolve_migration(editor.world_mut(), true);
    assert!(dir.path().join("assets/scenes/level.bsn").exists());
    assert!(dir.path().join("assets/scenes/level.jsn.bak").exists());
    assert!(!dir.path().join("assets/scenes/level.jsn").exists());
    assert!(
        dir.path().join(".jsn/project.jsn").exists(),
        "config untouched"
    );
    assert_eq!(count_legacy_files(dir.path()), 0);
}

#[test]
fn migration_prompt_detects_and_converts_legacy_projects() {
    use jackdaw::migrate_dialog::{PendingMigration, count_legacy_files, resolve_migration};
    use jackdaw::project::ProjectRoot;

    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("assets/scenes")).unwrap();

    let mut app = headless_app();
    app.init_resource::<PendingMigration>();
    app.world_mut().spawn((
        Name::new("Thing"),
        Transform::default(),
        SceneNodeId::next(),
        jackdaw_scene_types::SceneRootTag,
    ));
    let scene = world_to_jsn_scene(app.world_mut());
    std::fs::write(
        dir.path().join("assets/scenes/level.jsn"),
        serde_json::to_string(&scene).unwrap(),
    )
    .unwrap();
    // Backups and config never count as legacy files.
    std::fs::write(dir.path().join("assets/old.jsn.bak"), "x").unwrap();
    std::fs::create_dir_all(dir.path().join(".jsn")).unwrap();
    std::fs::write(dir.path().join(".jsn/project.jsn"), "{}").unwrap();

    assert_eq!(count_legacy_files(dir.path()), 1);

    app.world_mut().insert_resource(ProjectRoot::new(
        dir.path().to_path_buf(),
        jackdaw::project::ProjectConfig::default(),
    ));

    // Declining leaves the project untouched and clears the pending state.
    app.world_mut()
        .resource_mut::<PendingMigration>()
        .file_count = Some(1);
    resolve_migration(app.world_mut(), false);
    assert!(dir.path().join("assets/scenes/level.jsn").is_file());
    assert!(
        app.world()
            .resource::<PendingMigration>()
            .file_count
            .is_none()
    );

    // Accepting converts and backs up.
    app.world_mut()
        .resource_mut::<PendingMigration>()
        .file_count = Some(1);
    resolve_migration(app.world_mut(), true);
    assert!(dir.path().join("assets/scenes/level.bsn").is_file());
    assert!(dir.path().join("assets/scenes/level.jsn.bak").is_file());
    assert!(!dir.path().join("assets/scenes/level.jsn").exists());
    assert_eq!(count_legacy_files(dir.path()), 0, "nothing left to convert");
}
