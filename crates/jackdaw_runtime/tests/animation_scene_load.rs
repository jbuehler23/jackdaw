#![cfg(feature = "animation")]
//! A game runtime plays the animation set its scenes were authored with.
//!
//! The set names its clips by file and its skeleton by bone name, so the only
//! thing the document carries is text; everything else is resolved after the
//! scene has spawned.

use bevy::{
    animation::{
        AnimationClip, AnimationTargetId, animated_field,
        animation_curves::{AnimatableCurve, AnimatableKeyframeCurve},
        transition::AnimationTransitions,
    },
    gltf::Gltf,
    platform::collections::HashMap,
    prelude::*,
};
use jackdaw_animation_runtime::{AnimationSet, AnimationSetBound};
use jackdaw_runtime::{JackdawPlugin, JackdawScene, JackdawSceneRoot};

/// Holds the stand-in source, which is dropped from the asset store the moment
/// nothing points at it.
#[derive(Resource)]
struct SourceHandle(
    #[expect(dead_code, reason = "held only to keep the asset alive")] Handle<Gltf>,
);

const SCENE: &str = r#"
bevy_ecs::hierarchy::Children [
    #Rig
    jackdaw_animation_runtime::AnimationSet {
        sources: ["rig.glb"],
        states: [
            jackdaw_animation_runtime::AnimationStateDef {
                name: "idle",
                source: 0,
                clip: "Idle",
                looped: true,
                transition_secs: 0.15,
                speed: 1.0,
                then: None,
            },
        ],
        default_state: "idle",
        skeleton_root: "Armature",
    }
    bevy_ecs::hierarchy::Children [
        #Armature
        bevy_transform::components::transform::Transform
    ]
]
"#;

#[test]
fn a_loaded_scene_plays_its_default_state() {
    let mut app = runtime_app();
    stand_in_for_source(&mut app, "rig.glb", "Idle");
    let scene = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(SCENE.into(), ".".into()));
    app.world_mut().spawn(JackdawSceneRoot(scene));

    for _ in 0..4 {
        app.update();
    }

    let rig = named_entity(app.world_mut(), "Rig");
    let armature = named_entity(app.world_mut(), "Armature");
    assert!(
        app.world().get::<AnimationSet>(rig).is_some(),
        "the authored set loaded"
    );
    let bound = app
        .world()
        .get::<AnimationSetBound>(rig)
        .expect("the set found the skeleton the scene spawned under it");
    assert_eq!(bound.player, armature);
    assert_eq!(
        app.world()
            .get::<AnimationTransitions>(armature)
            .and_then(AnimationTransitions::get_main_animation),
        Some(bound.nodes["idle"]),
        "a game plays the set's default state with nothing else asking it to"
    );
}

/// Puts a file holding one named clip where the asset server would find it,
/// so the test needs no glTF on disk.
fn stand_in_for_source(app: &mut App, path: &'static str, clip_name: &str) {
    let curve = AnimatableKeyframeCurve::new([(0.0, Vec3::ZERO), (1.0, Vec3::X)])
        .expect("two keyframes make a curve");
    let mut clip = AnimationClip::default();
    let names = [Name::new("Armature")];
    clip.add_curve_to_target(
        AnimationTargetId::from_names(names.iter()),
        AnimatableCurve::new(animated_field!(Transform::translation), curve),
    );
    let clip = app
        .world_mut()
        .resource_mut::<Assets<AnimationClip>>()
        .add(clip);
    let gltf = Gltf {
        scenes: Vec::new(),
        named_scenes: HashMap::default(),
        meshes: Vec::new(),
        named_meshes: HashMap::default(),
        materials: Vec::new(),
        named_materials: HashMap::default(),
        nodes: Vec::new(),
        named_nodes: HashMap::default(),
        skins: Vec::new(),
        named_skins: HashMap::default(),
        default_scene: None,
        animations: vec![clip.clone()],
        named_animations: [(clip_name.into(), clip)].into_iter().collect(),
        source: None,
    };
    let handle: Handle<Gltf> = app.world().resource::<AssetServer>().load(path);
    app.world_mut()
        .resource_mut::<Assets<Gltf>>()
        .insert(handle.id(), gltf)
        .expect("the stand-in source is the only thing written at that id");
    app.insert_resource(SourceHandle(handle));
}

fn runtime_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(AssetPlugin::default());
    app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    app.add_plugins(bevy::animation::AnimationPlugin);
    app.add_plugins(JackdawPlugin);
    app.init_asset::<Gltf>();
    app
}

fn named_entity(world: &mut World, target: &str) -> Entity {
    let mut names = world.query::<(Entity, &Name)>();
    names
        .iter(world)
        .find_map(|(entity, name)| (name.as_str() == target).then_some(entity))
        .unwrap_or_else(|| panic!("expected entity named {target}"))
}
