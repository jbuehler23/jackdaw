//! Full-document save/load/save fixpoint: a scene holding an entity
//! hierarchy, a runtime material referenced by a brush face, and a prefab
//! instance emits to text, respawns from that text, and emits the identical
//! text again, with the reloaded world semantically matching the original.

use bevy::prelude::*;

fn make_app() -> App {
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
    app
}

/// A two-entity prefab: a root and one inherited child named `part`.
const PREFAB_BSN: &str = "\
#tree
jackdaw::prefab::components::Prefab
jackdaw::prefab::components::PrefabEntityId(0)
bevy_transform::components::transform::Transform { translation: glam::Vec3 { x: 0.0, y: 0.0, z: 0.0 }, rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 }, scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 } }
bevy_camera::visibility::Visibility::Inherited
bevy_ecs::hierarchy::Children [
    #part
    jackdaw::prefab::components::PrefabEntityId(1)
    bevy_transform::components::transform::Transform { translation: glam::Vec3 { x: 0.0, y: 1.0, z: 0.0 }, rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 }, scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 } }
]
";

#[test]
fn scene_with_assets_and_prefab_instance_round_trips_to_a_fixpoint() {
    use bevy::pbr::StandardMaterial;
    use jackdaw_scene_types::Brush;

    let tmp = tempfile::tempdir().unwrap();
    let prefab_path = tmp.path().join("tree.bsn");
    std::fs::write(&prefab_path, PREFAB_BSN).unwrap();

    let mut app = make_app();

    // A runtime material (no filesystem path) with a non-default color,
    // referenced by the first face of a brush.
    let color = Color::srgb(0.2, 0.7, 0.3);
    let handle = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: color,
            ..Default::default()
        });
    let mut brush = Brush::cuboid(1.0, 1.0, 1.0);
    brush.faces[0].material = handle.clone();

    // A two-level authored hierarchy registered into the live document.
    let parent = app
        .world_mut()
        .spawn((
            Name::new("Parent"),
            Transform::from_xyz(1.0, 2.0, 3.0),
            Visibility::Inherited,
            brush,
        ))
        .id();
    let kid = app
        .world_mut()
        .spawn((
            Name::new("Kid"),
            Transform::from_xyz(0.0, 1.0, 0.0),
            Visibility::Inherited,
            ChildOf(parent),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), parent);
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), kid);

    // A prefab instance alongside the authored entities.
    jackdaw::prefab::operators::spawn_instance(
        app.world_mut(),
        &prefab_path,
        Vec3::new(5.0, 0.0, 0.0),
    );

    let count_before = {
        let world = app.world_mut();
        let mut q = world.query_filtered::<Entity, With<jackdaw_bsn::AstNodeRef>>();
        q.iter(world).count()
    };

    // Save -> load -> save: the emitted text must be a fixpoint.
    let text1 = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        std::path::Path::new(""),
    );
    assert!(
        text1.contains("StandardMaterial"),
        "the runtime material must embed into the emitted scene:\n{text1}"
    );

    jackdaw::prefab::watcher::respawn_from_sparse_text(app.world_mut(), &text1);

    let text2 = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        std::path::Path::new(""),
    );
    assert_eq!(
        text1, text2,
        "emit -> respawn -> emit must reproduce the identical scene text"
    );

    // The reloaded world matches semantically: same document entity count.
    let count_after = {
        let world = app.world_mut();
        let mut q = world.query_filtered::<Entity, With<jackdaw_bsn::AstNodeRef>>();
        q.iter(world).count()
    };
    assert_eq!(count_after, count_before, "document entity count survives");

    // The authored hierarchy survives with its names and parentage.
    let world = app.world_mut();
    let find = |world: &mut World, wanted: &str| -> Entity {
        let mut q = world.query::<(Entity, &Name)>();
        q.iter(world)
            .find(|(_, n)| n.as_str() == wanted)
            .map(|(e, _)| e)
            .unwrap_or_else(|| panic!("entity named {wanted} after reload"))
    };
    let new_parent = find(world, "Parent");
    let new_kid = find(world, "Kid");
    assert_eq!(
        world.get::<ChildOf>(new_kid).map(ChildOf::parent),
        Some(new_parent),
        "Kid stays a child of Parent"
    );

    // The brush face still resolves its runtime material with the color.
    let face_handle = world
        .get::<Brush>(new_parent)
        .expect("Parent keeps its Brush")
        .faces[0]
        .material
        .clone();
    let material = world
        .resource::<Assets<StandardMaterial>>()
        .get(&face_handle)
        .expect("face material asset survived the round trip");
    let want = color.to_linear();
    let got = material.base_color.to_linear();
    assert!(
        (want.red - got.red).abs() < 1e-4
            && (want.green - got.green).abs() < 1e-4
            && (want.blue - got.blue).abs() < 1e-4,
        "face material color must survive: want {want:?}, got {got:?}"
    );

    // The prefab instance is resolved: one IsA root with the inherited child.
    let mut isa_q = world.query::<(Entity, &jackdaw::prefab::IsA)>();
    let instances: Vec<Entity> = isa_q.iter(world).map(|(e, _)| e).collect();
    assert_eq!(instances.len(), 1, "exactly one prefab instance");
    let mut part_q = world.query::<(&jackdaw::prefab::PrefabEntityId, &Name, &ChildOf)>();
    let parts: Vec<Entity> = part_q
        .iter(world)
        .filter(|(id, name, child_of)| {
            id.0 == 1 && name.as_str() == "part" && child_of.parent() == instances[0]
        })
        .map(|(_, _, child_of)| child_of.parent())
        .collect();
    assert_eq!(
        parts.len(),
        1,
        "the instance resolved its inherited descendant"
    );
}
