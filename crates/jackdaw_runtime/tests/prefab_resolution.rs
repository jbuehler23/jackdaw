//! A world scene that references a prefab resolves that reference when a game
//! loads it, the same way the editor resolves it on open: the instance root
//! carries the inherited components and the referenced tree spawns beneath it.

use std::path::PathBuf;

use bevy::prelude::*;
use jackdaw_runtime::{
    JackdawCatalogPath, JackdawPlugin, JackdawScene, JackdawSceneMember, JackdawSceneRoot,
};
use jackdaw_scene_types::UiSceneRoot;

/// A UI prefab: a `UiSceneRoot` panel with one named child.
const PANEL_BSN: &str = "\
#Panel
jackdaw::prefab::components::Prefab
jackdaw::prefab::components::PrefabEntityId(0)
jackdaw_scene_types::UiSceneRoot
bevy_ecs::hierarchy::Children [
    #Label
    jackdaw::prefab::components::PrefabEntityId(1)
    bevy_transform::components::transform::Transform
]
";

#[test]
fn a_world_scene_resolves_the_prefab_it_references() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("panel.bsn"), PANEL_BSN).unwrap();

    let mut app = runtime_app(dir.path());
    let scene = add_scene(
        &mut app,
        "jackdaw::prefab::components::IsA { source: \"panel.bsn\", deleted: [] }\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n",
    );
    let scene_root = app.world_mut().spawn(JackdawSceneRoot(scene)).id();

    app.update();
    app.update();

    let panel = named_entity(app.world_mut(), "Panel").expect("the prefab root materialized");
    assert!(
        app.world().get::<UiSceneRoot>(panel).is_some(),
        "the instance inherited the prefab root's UiSceneRoot"
    );
    assert!(
        app.world().get::<ChildOf>(panel).is_none(),
        "a resolved UI root is an ECS root, exactly as an inline one is"
    );
    assert_eq!(
        app.world()
            .get::<JackdawSceneMember>(panel)
            .map(|member| member.root),
        Some(scene_root),
        "and the scene owns it, so an unparented root still goes when the scene goes"
    );
    assert!(
        named_entity(app.world_mut(), "Label").is_some(),
        "the inherited child spawned"
    );
}

#[test]
fn an_authored_override_wins_over_the_inherited_value() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("panel.bsn"), PANEL_BSN).unwrap();

    let mut app = runtime_app(dir.path());
    let scene = add_scene(
        &mut app,
        "jackdaw::prefab::components::IsA { source: \"panel.bsn\", deleted: [] }\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         bevy_ecs::hierarchy::Children [\n\
         jackdaw::prefab::components::PrefabEntityId(1)\n\
         bevy_transform::components::transform::Transform { translation: glam::Vec3 { x: 7.0, y: 0.0, z: 0.0 } }\n\
         ]\n",
    );
    app.world_mut().spawn(JackdawSceneRoot(scene));

    app.update();
    app.update();

    let label = named_entity(app.world_mut(), "Label").expect("the inherited child spawned");
    assert_eq!(
        app.world().get::<Transform>(label).map(|t| t.translation.x),
        Some(7.0),
        "the scene's own value overrides the prefab baseline"
    );
}

#[test]
fn a_cyclic_reference_is_refused_rather_than_followed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("a.bsn"),
        "#A\n\
         jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         bevy_ecs::hierarchy::Children [\n\
         #InnerB\n\
         jackdaw::prefab::components::PrefabEntityId(1)\n\
         jackdaw::prefab::components::IsA { source: \"b.bsn\", deleted: [] }\n\
         ]\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("b.bsn"),
        "#B\n\
         jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         bevy_ecs::hierarchy::Children [\n\
         #InnerA\n\
         jackdaw::prefab::components::PrefabEntityId(1)\n\
         jackdaw::prefab::components::IsA { source: \"a.bsn\", deleted: [] }\n\
         ]\n",
    )
    .unwrap();

    let mut app = runtime_app(dir.path());
    let scene = add_scene(
        &mut app,
        "#Instance\n\
         jackdaw::prefab::components::IsA { source: \"a.bsn\", deleted: [] }\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n",
    );
    app.world_mut().spawn(JackdawSceneRoot(scene));

    app.update();
    app.update();

    // Fail-soft: the refusal costs the inherited content, not the scene.
    assert!(
        named_entity(app.world_mut(), "Instance").is_some(),
        "the authored instance node still spawns"
    );
    assert!(
        named_entity(app.world_mut(), "InnerA").is_none(),
        "nothing from the cycle materialized"
    );
}

#[test]
fn a_reference_deeper_than_the_cap_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let depth = jackdaw_prefab::MAX_PREFAB_DEPTH + 2;
    std::fs::write(
        dir.path().join(format!("level{depth}.bsn")),
        "#Leaf\n\
         jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n",
    )
    .unwrap();
    for level in (0..depth).rev() {
        std::fs::write(
            dir.path().join(format!("level{level}.bsn")),
            format!(
                "#Level{level}\n\
                 jackdaw::prefab::components::Prefab\n\
                 jackdaw::prefab::components::PrefabEntityId(0)\n\
                 bevy_ecs::hierarchy::Children [\n\
                 jackdaw::prefab::components::PrefabEntityId(1)\n\
                 jackdaw::prefab::components::IsA {{ source: \"level{next}.bsn\", deleted: [] }}\n\
                 ]\n",
                next = level + 1
            ),
        )
        .unwrap();
    }

    let mut app = runtime_app(dir.path());
    let scene = add_scene(
        &mut app,
        "#Instance\n\
         jackdaw::prefab::components::IsA { source: \"level0.bsn\", deleted: [] }\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n",
    );
    app.world_mut().spawn(JackdawSceneRoot(scene));

    app.update();
    app.update();

    assert!(
        named_entity(app.world_mut(), "Instance").is_some(),
        "the authored instance node still spawns"
    );
    assert!(
        named_entity(app.world_mut(), "Leaf").is_none(),
        "the chain past the cap never materialized"
    );
}

fn add_scene(app: &mut App, bsn: &str) -> Handle<JackdawScene> {
    app.world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(bsn.to_owned(), PathBuf::new()))
}

fn runtime_app(assets_root: &std::path::Path) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(AssetPlugin::default());
    app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    // The runtime reads prefab sources from the asset root, which the catalog
    // names.
    app.insert_resource(JackdawCatalogPath(assets_root.join("catalog.bsn")));
    app.add_plugins(JackdawPlugin);
    app
}

fn named_entity(world: &mut World, target: &str) -> Option<Entity> {
    let mut names = world.query::<(Entity, &Name)>();
    names
        .iter(world)
        .find_map(|(entity, name)| (name.as_str() == target).then_some(entity))
}

/// A game follows references only inside its own asset root: a reference is an
/// instruction to open whatever it names, and a scene file is content a player
/// can edit. The editor is laxer, which `jackdaw`'s `prefab_source_paths`
/// tests cover.
#[test]
fn a_source_outside_the_asset_root_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let assets = dir.path().join("assets");
    std::fs::create_dir_all(&assets).unwrap();
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("panel.bsn"), PANEL_BSN).unwrap();

    let mut app = runtime_app(&assets);
    let scene = add_scene(
        &mut app,
        "#Instance\n\
         jackdaw::prefab::components::IsA { source: \"../outside/panel.bsn\", deleted: [] }\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n",
    );
    app.world_mut().spawn(JackdawSceneRoot(scene));

    app.update();
    app.update();

    assert!(
        named_entity(app.world_mut(), "Instance").is_some(),
        "the authored instance node still spawns"
    );
    // The inherited child, not the root: an instance keeps its own name either
    // way.
    assert!(
        named_entity(app.world_mut(), "Label").is_none(),
        "nothing outside the asset root was read"
    );
}

/// Containment is checked as each source comes off the queue, so a prefab inside
/// the asset root cannot be used as a doorway to one outside it.
#[test]
fn a_source_reached_through_another_prefab_is_refused_outside_the_asset_root() {
    let dir = tempfile::tempdir().unwrap();
    let assets = dir.path().join("assets");
    std::fs::create_dir_all(assets.join("zones")).unwrap();
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("panel.bsn"), PANEL_BSN).unwrap();
    std::fs::write(
        assets.join("zones/doorway.bsn"),
        "#Doorway\n\
         jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         jackdaw::prefab::components::IsA { source: \"../../outside/panel.bsn\", deleted: [] }\n",
    )
    .unwrap();

    let mut app = runtime_app(&assets);
    let scene = add_scene(
        &mut app,
        "#Instance\n\
         jackdaw::prefab::components::IsA { source: \"zones/doorway.bsn\", deleted: [] }\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n",
    );
    app.world_mut().spawn(JackdawSceneRoot(scene));

    app.update();
    app.update();

    assert!(
        named_entity(app.world_mut(), "Instance").is_some(),
        "the authored instance node still spawns"
    );
    assert!(
        named_entity(app.world_mut(), "Label").is_none(),
        "nothing outside the asset root was read"
    );
    assert!(
        named_entity(app.world_mut(), "Doorway").is_none(),
        "and the refusal ends the whole gather, so the local half resolves no further"
    );
}

/// A variant inherits from the base it was cut from, so resolving one is two
/// levels of reference, the inner spelled relative to the variant file.
#[test]
fn a_variant_resolves_through_the_base_it_was_cut_from() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("base")).unwrap();
    std::fs::create_dir_all(dir.path().join("zones")).unwrap();
    std::fs::write(dir.path().join("base/panel.bsn"), PANEL_BSN).unwrap();
    std::fs::write(
        dir.path().join("zones/hud_variant.bsn"),
        "#Variant\n\
         jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         jackdaw::prefab::components::IsA { source: \"../base/panel.bsn\", deleted: [] }\n",
    )
    .unwrap();

    let mut app = runtime_app(dir.path());
    let scene = add_scene(
        &mut app,
        "jackdaw::prefab::components::IsA { source: \"zones/hud_variant.bsn\", deleted: [] }\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n",
    );
    app.world_mut().spawn(JackdawSceneRoot(scene));

    app.update();
    app.update();

    let variant = named_entity(app.world_mut(), "Variant").expect("the variant root materialized");
    assert!(
        app.world().get::<UiSceneRoot>(variant).is_some(),
        "the variant inherited the base's UiSceneRoot through two references"
    );
    assert!(
        named_entity(app.world_mut(), "Label").is_some(),
        "the base's child came through the variant"
    );
}
