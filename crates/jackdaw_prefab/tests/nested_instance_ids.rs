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

const NODE_ID: &str = "jackdaw_scene_types::node_id::SceneNodeId";

fn lamp_with_inner_id(inner: &str) -> String {
    format!(
        r#"#Lamp
jackdaw::prefab::components::Prefab
jackdaw::prefab::components::PrefabEntityId(0)
bevy_ecs::hierarchy::Children [
    #Bulb
    bevy_light::point_light::PointLight
    jackdaw::prefab::components::PrefabEntityId(1)
    {inner}
]
"#
    )
}

fn lamp_scene_with_inner_id(id: u64) -> String {
    format!(
        r#"#Level
jackdaw::prefab::components::IsA {{ source: "lamp.bsn", deleted: [] }}
jackdaw::prefab::components::PrefabEntityId(0)
bevy_ecs::hierarchy::Children [
    jackdaw::prefab::components::PrefabEntityId(1)
    {NODE_ID}({id})
]
"#
    )
}

#[test]
fn a_saved_inner_id_does_not_override_the_files() {
    let lamp = lamp_with_inner_id(&format!("{NODE_ID}(222)"));
    let out = resolve(&[("lamp.bsn", &lamp)], &lamp_scene_with_inner_id(111));
    assert!(out.contains(&format!("{NODE_ID}(222)")), "{out}");
    assert!(!out.contains(&format!("{NODE_ID}(111)")), "{out}");
}

#[test]
fn an_id_survives_save_when_the_file_node_has_none() {
    let lamp = lamp_with_inner_id("");
    let prefabs: HashMap<PathBuf, SceneBsnAst> =
        HashMap::from([(PathBuf::from("lamp.bsn"), parse_bsn_text(&lamp).unwrap())]);
    let get = |p: &Path| prefabs.get(p);
    let scene = parse_bsn_text(&lamp_scene_with_inner_id(111)).unwrap();
    let mut resolved = resolve_scene(&scene, &get).unwrap();
    jackdaw_prefab::sparsify_inherited_descendants(&mut resolved, &get, &|_| None);
    let out = emit_scene(&resolved);
    assert!(out.contains(&format!("{NODE_ID}(111)")), "{out}");
}

const SHADE: &str = r#"#Shade
jackdaw::prefab::components::Prefab
jackdaw::prefab::components::PrefabEntityId(0)
jackdaw_scene_types::types::Tint { level: 1.0 }
bevy_ecs::hierarchy::Children [
    #Glass
    jackdaw_scene_types::types::Tint { level: 2.0 }
    jackdaw::prefab::components::PrefabEntityId(1)
]
"#;

fn hall(shade: f64, glass: f64) -> String {
    format!(
        r#"#Hall
jackdaw::prefab::components::Prefab
jackdaw::prefab::components::PrefabEntityId(0)
bevy_ecs::hierarchy::Children [
    jackdaw::prefab::components::IsA {{ source: "shade.bsn", deleted: [] }}
    jackdaw::prefab::components::PrefabEntityId(2)
    jackdaw_scene_types::types::Tint {{ level: {shade:?} }}
    bevy_ecs::hierarchy::Children [
        jackdaw::prefab::components::PrefabEntityId(1)
        jackdaw_scene_types::types::Tint {{ level: {glass:?} }}
    ]
]
"#
    )
}

const HALL_SCENE: &str = r#"#Level
jackdaw::prefab::components::IsA { source: "hall.bsn", deleted: [] }
jackdaw::prefab::components::PrefabEntityId(0)
"#;

/// A nested instance the outer prefab places, and the nodes inside it, take
/// their values from the outer prefab's entry for them: saving keeps none of
/// what the outer prefab authors, and an edit to it shows through.
#[test]
fn a_nested_instance_follows_what_its_outer_prefab_authors() {
    let saved = {
        let hall = hall(3.0, 4.0);
        let prefabs: HashMap<PathBuf, SceneBsnAst> = [("shade.bsn", SHADE), ("hall.bsn", &hall)]
            .iter()
            .map(|(path, text)| (PathBuf::from(path), parse_bsn_text(text).unwrap()))
            .collect();
        let get = |p: &Path| prefabs.get(p);
        let mut resolved = resolve_scene(&parse_bsn_text(HALL_SCENE).unwrap(), &get).unwrap();
        let out = emit_scene(&resolved);
        assert!(
            out.contains("level: 3.0") && out.contains("level: 4.0"),
            "{out}"
        );
        jackdaw_prefab::sparsify_inherited_descendants(&mut resolved, &get, &|_| None);
        emit_scene(&resolved)
    };
    assert!(!saved.contains("Tint"), "{saved}");

    let out = resolve(
        &[("shade.bsn", SHADE), ("hall.bsn", &hall(5.0, 6.0))],
        &saved,
    );
    assert!(
        out.contains("level: 5.0") && out.contains("level: 6.0"),
        "{out}"
    );
    assert!(
        !out.contains("level: 3.0") && !out.contains("level: 4.0"),
        "{out}"
    );
}
