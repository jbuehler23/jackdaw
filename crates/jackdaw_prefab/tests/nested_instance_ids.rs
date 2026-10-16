//! `PrefabEntityId`s are unique only within one prefab file, so a nested
//! instance's entities must never be matched against the outer prefab's ids.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use jackdaw_bsn::{SceneBsnAst, emit_scene, parse_bsn_text};
use jackdaw_prefab::resolve_scene;

const TEMPLATE: &str = r#"#Room
jackdaw::prefab::components::Prefab
jackdaw::prefab::components::PrefabEntityId(0)
bevy_ecs::hierarchy::Children [
    #Camera
    bevy_camera::components::Camera3d
    jackdaw::prefab::components::PrefabEntityId(1)
]
"#;

fn player(id: u32) -> String {
    format!(
        r#"#Player
jackdaw::prefab::components::Prefab
jackdaw::prefab::components::PrefabEntityId(0)
bevy_ecs::hierarchy::Children [
    #PlayerModel
    jackdaw_scene_types::types::GltfSource {{ path: "models/player.glb", scene_index: 0 }}
    jackdaw::prefab::components::PrefabEntityId({id})
]
"#
    )
}

/// The scene as the editor saves it, with an entry for every inherited child.
fn scene(id: u32) -> String {
    format!(
        r#"#Level
jackdaw::prefab::components::IsA {{ source: "template.bsn", deleted: [] }}
jackdaw::prefab::components::PrefabEntityId(0)
bevy_ecs::hierarchy::Children [
    jackdaw::prefab::components::IsA {{ source: "player.bsn", deleted: [] }}
    jackdaw::prefab::components::PrefabEntityId(0)
    bevy_ecs::hierarchy::Children [
        jackdaw::prefab::components::PrefabEntityId({id})
    ]
    --
    jackdaw::prefab::components::PrefabEntityId(1)
]
"#
    )
}

fn resolve(prefabs: &[(&str, &str)], scene: &str) -> String {
    let prefabs: HashMap<PathBuf, SceneBsnAst> = prefabs
        .iter()
        .map(|(path, text)| (PathBuf::from(path), parse_bsn_text(text).unwrap()))
        .collect();
    let get = |p: &Path| prefabs.get(p);
    let scene = parse_bsn_text(scene).unwrap();
    emit_scene(&resolve_scene(&scene, &get).unwrap())
}

#[test]
fn nested_instance_ids_are_scoped_to_their_own_prefab() {
    for id in [1, 101] {
        let player = player(id);
        let out = resolve(
            &[("template.bsn", TEMPLATE), ("player.bsn", &player)],
            &scene(id),
        );
        assert_eq!(out.matches("Camera3d").count(), 1, "id {id}:\n{out}");
        assert_eq!(out.matches("#Camera").count(), 1, "id {id}:\n{out}");
        assert_eq!(out.matches("#PlayerModel").count(), 1, "id {id}:\n{out}");
        assert_eq!(out.matches("#Player\n").count(), 1, "id {id}:\n{out}");
    }
}

/// A prefab that nests another one carries the nested prefab's children,
/// numbered in that prefab's id space. They still materialize when the outer
/// instance already has an entry with the same id.
#[test]
fn nested_children_in_a_prefab_materialize_despite_matching_outer_ids() {
    const LAMP: &str = r#"#Lamp
jackdaw::prefab::components::Prefab
jackdaw::prefab::components::PrefabEntityId(0)
bevy_ecs::hierarchy::Children [
    #Bulb
    bevy_light::point_light::PointLight
    jackdaw::prefab::components::PrefabEntityId(1)
]
"#;
    const ROOM: &str = r#"#Room
jackdaw::prefab::components::Prefab
jackdaw::prefab::components::PrefabEntityId(0)
bevy_ecs::hierarchy::Children [
    #Camera
    bevy_camera::components::Camera3d
    jackdaw::prefab::components::PrefabEntityId(1)
    --
    jackdaw::prefab::components::IsA { source: "lamp.bsn", deleted: [] }
    jackdaw::prefab::components::PrefabEntityId(2)
]
"#;
    const SCENE: &str = r#"#Level
jackdaw::prefab::components::IsA { source: "room.bsn", deleted: [] }
jackdaw::prefab::components::PrefabEntityId(0)
bevy_ecs::hierarchy::Children [
    jackdaw::prefab::components::PrefabEntityId(1)
]
"#;
    let out = resolve(&[("lamp.bsn", LAMP), ("room.bsn", ROOM)], SCENE);
    assert_eq!(out.matches("#Bulb").count(), 1, "{out}");
    assert_eq!(out.matches("PointLight").count(), 1, "{out}");
    assert_eq!(out.matches("Camera3d").count(), 1, "{out}");
}
