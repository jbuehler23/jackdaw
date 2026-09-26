//! Saving a scene keeps an instance sparse when its inherited content names an
//! asset file.
//!
//! The live document holds the asset-blind placeholder (`""`) the ECS mirror
//! stores for a `Handle<T>`. A save has to name the asset again before it
//! compares an instance with its prefab, or an unchanged brush whose faces use
//! a material file reads as an override and is written out whole.

use bevy::prelude::*;
use jackdaw::project::{ProjectConfig, ProjectRoot};
use jackdaw_bsn::BsnPatch;
use jackdaw_scene_types::types::Brush;

const BRUSH_TYPE: &str = "jackdaw_scene_types::types::Brush";
const PREFAB_TYPE: &str = "jackdaw::prefab::components::Prefab";
const PEID_TYPE: &str = "jackdaw::prefab::components::PrefabEntityId";

fn make_app(root: &std::path::Path) -> App {
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
    app.init_resource::<jackdaw::scene_io::SceneDirtyState>();
    app.init_resource::<jackdaw::selection::Selection>();
    app.init_resource::<jackdaw::scenes::Scenes>();
    app.insert_resource(ProjectRoot::new(
        root.to_path_buf(),
        ProjectConfig::default(),
    ));
    app
}

fn entity_named(app: &mut App, name: &str) -> Option<Entity> {
    let mut query = app.world_mut().query::<(Entity, &Name)>();
    query
        .iter(app.world())
        .find(|(_, found)| found.as_str() == name)
        .map(|(entity, _)| entity)
}

fn peid_patch(id: i128) -> BsnPatch {
    BsnPatch::TupleStruct(jackdaw_bsn::BsnTupleStructData {
        type_path: PEID_TYPE.to_string(),
        values: vec![jackdaw_bsn::BsnValue::Int(id)],
    })
}

/// A material the project holds as the file at `path`: loaded into the
/// store (no asset-server path, the way the editor's asset index loads one)
/// and published under that path for documents to resolve and emit.
fn material_file(app: &mut App, path: &str, color: Color) -> Handle<StandardMaterial> {
    let handle = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: color,
            ..default()
        });
    let untyped = handle.clone().untyped();
    let world = app.world_mut();
    world
        .get_resource_or_init::<jackdaw_bsn::BsnProjectAssets>()
        .0
        .insert(path.to_string(), untyped.clone());
    world
        .get_resource_or_init::<jackdaw_bsn::BsnSceneAssets>()
        .0
        .insert(path.to_string(), untyped.clone());
    world
        .get_resource_or_init::<jackdaw_bsn::BsnAssetPaths>()
        .0
        .insert(untyped.id(), path.to_string());
    handle
}

/// A prefab of one cube brush whose every face uses `material`, spelled the
/// way a save spells it.
fn write_brush_prefab(app: &App, path: &std::path::Path, material: &Handle<StandardMaterial>) {
    let mut brush = Brush::cuboid(0.5, 0.5, 0.5);
    for face in &mut brush.faces {
        face.material = material.clone();
    }
    let world = app.world();
    let server = world.resource::<AssetServer>().clone();
    let names = world.resource::<jackdaw_bsn::BsnAssetPaths>().0.clone();
    let registry = world.resource::<AppTypeRegistry>().read();
    let parent = path.parent().expect("the prefab has a folder");
    let ctx = jackdaw_bsn::BsnAssetContext {
        asset_server: &server,
        parent_path: parent,
        asset_names: Some(&names),
    };
    let brush_patch = jackdaw_bsn::component_to_bsn_patch_with_assets(&brush, &registry, &ctx);

    let mut doc = jackdaw_bsn::SceneBsnAst::default();
    let root = doc.create_entity_node(vec![
        BsnPatch::Type(PREFAB_TYPE.to_string()),
        peid_patch(0),
        BsnPatch::Name("Crate".to_string()),
    ]);
    doc.add_to_roots(root);
    let body = doc.create_entity_node(vec![
        peid_patch(1),
        BsnPatch::Name("CrateBody".to_string()),
        brush_patch,
    ]);
    doc.add_child_to_ast(root, body);
    std::fs::write(path, jackdaw_bsn::emit_scene(&doc)).expect("write the prefab");
}

/// Mirror the brush into the live document the way the editor does for every
/// brush it spawns or changes (`Changed<Brush>`), handles and all.
fn mirror(app: &mut App, entity: Entity) {
    let brush = app
        .world()
        .get::<Brush>(entity)
        .expect("the brush is live")
        .clone();
    jackdaw::brush::sync_brush_to_ast(app.world_mut(), entity, &brush);
}

#[test]
fn an_unchanged_prefab_brush_with_a_material_file_saves_sparse() {
    let tmp = tempfile::tempdir().unwrap();
    let assets = tmp.path().join("assets");
    std::fs::create_dir_all(&assets).unwrap();

    let mut app = make_app(tmp.path());
    let red = material_file(&mut app, "materials/red.bsn", Color::srgb(0.8, 0.1, 0.1));
    let blue = material_file(&mut app, "materials/blue.bsn", Color::srgb(0.1, 0.2, 0.9));
    let prefab = assets.join("crate.bsn");
    write_brush_prefab(&app, &prefab, &red);

    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &prefab, Vec3::ZERO);
    app.update();
    let body = entity_named(&mut app, "CrateBody").expect("the inherited brush spawned");
    assert!(
        app.world()
            .get::<Brush>(body)
            .is_some_and(|brush| brush.faces.iter().all(|face| face.material == red)),
        "the instance's faces resolved the prefab's material file"
    );
    mirror(&mut app, body);

    let saved = jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), &assets)
        .expect("the scene emits");
    assert!(
        !saved.contains(BRUSH_TYPE),
        "an unchanged inherited brush is not written as an override:\n{saved}"
    );

    // A real change still saves as an override, naming the new file.
    app.world_mut()
        .get_mut::<Brush>(body)
        .expect("the brush is live")
        .faces
        .iter_mut()
        .for_each(|face| face.material = blue.clone());
    mirror(&mut app, body);

    let saved = jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), &assets)
        .expect("the scene emits");
    assert!(
        saved.contains(BRUSH_TYPE) && saved.contains("\"materials/blue.bsn\""),
        "a changed material is kept as an override that names its file:\n{saved}"
    );
}
