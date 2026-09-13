//! An authored animation graph compiles onto the skeleton spawned under it,
//! mixes its clips from the parameters the game writes, and moves between its
//! states on conditions and exit times rather than on a state name.

use std::time::Duration;

use bevy::{
    animation::{
        ActiveAnimation, AnimationClip, AnimationTargetId, animated_field,
        animation_curves::{AnimatableCurve, AnimatableKeyframeCurve},
        graph::{AnimationGraph, AnimationNodeType},
    },
    asset::AssetPlugin,
    gltf::Gltf,
    platform::collections::HashMap,
    prelude::*,
    time::TimeUpdateStrategy,
};
use jackdaw_animation_runtime::{
    AnimationBlendPoint, AnimationClipRef, AnimationCondition, AnimationConditionOp,
    AnimationEvent, AnimationGraphAsset, AnimationGraphBound, AnimationGraphDef,
    AnimationGraphPlayback, AnimationGraphRef, AnimationGraphSource, AnimationGraphState,
    AnimationMotion, AnimationParameterDef, AnimationParameterKind, AnimationParams,
    AnimationRuntimePlugin, AnimationSet, AnimationSetBound, AnimationSetSystems, AnimationSources,
    AnimationState, AnimationStateDef, AnimationTransitionDef, ClipEvent,
};

/// Every clip event sent so far.
#[derive(Resource, Default)]
struct Fired(Vec<String>);

fn animation_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::transform::TransformPlugin)
        .add_plugins(bevy::animation::AnimationPlugin)
        .add_plugins(AnimationRuntimePlugin);
    app.init_asset::<Gltf>();
    app.init_resource::<Fired>();
    app.add_systems(Update, collect_fired.after(AnimationSetSystems));
    app
}

fn collect_fired(mut sent: MessageReader<AnimationEvent>, mut out: ResMut<Fired>) {
    out.0.extend(sent.read().map(|event| event.name.clone()));
}

/// Puts a named moment on a clip row under `owner`: a child named for the clip
/// the events belong to, carrying one event.
fn clip_marker(app: &mut App, owner: Entity, clip: &str, time: f32, name: &str) {
    let row = app
        .world_mut()
        .spawn((Name::new(clip.to_string()), ChildOf(owner)))
        .id();
    app.world_mut().spawn((
        ClipEvent {
            time,
            name: name.to_string(),
        },
        ChildOf(row),
    ));
}

/// Runs one frame of a length the test picks rather than the wall clock's.
fn step(app: &mut App, delta: Duration) {
    app.insert_resource(TimeUpdateStrategy::ManualDuration(delta));
    app.update();
}

fn millis(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

/// A clip that slides the bone at `path` one unit along X over a second.
fn sliding_clip(app: &mut App, path: &[&str]) -> Handle<AnimationClip> {
    let names: Vec<Name> = path
        .iter()
        .map(|name| Name::new(name.to_string()))
        .collect();
    let curve = AnimatableKeyframeCurve::new([(0.0, Vec3::ZERO), (1.0, Vec3::X)])
        .expect("two keyframes make a curve");
    let mut clip = AnimationClip::default();
    clip.add_curve_to_target(
        AnimationTargetId::from_names(names.iter()),
        AnimatableCurve::new(animated_field!(Transform::translation), curve),
    );
    app.world_mut()
        .resource_mut::<Assets<AnimationClip>>()
        .add(clip)
}

/// A stand-in for a loaded glTF file that holds nothing but named clips.
fn source_holding(app: &mut App, clips: &[(&str, Handle<AnimationClip>)]) -> Handle<Gltf> {
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
        animations: clips.iter().map(|(_, clip)| clip.clone()).collect(),
        named_animations: clips
            .iter()
            .map(|(name, clip)| ((*name).into(), clip.clone()))
            .collect(),
        source: None,
    };
    app.world_mut().resource_mut::<Assets<Gltf>>().add(gltf)
}

/// Spawns a rig playing `def`, with the graph asset and its glTF file already
/// in hand so nothing waits on the disk.
fn spawn_graphed_rig(app: &mut App, def: AnimationGraphDef, source: Handle<Gltf>) -> Entity {
    let asset = app
        .world_mut()
        .resource_mut::<Assets<AnimationGraphAsset>>()
        .add(AnimationGraphAsset { def });
    let root = app
        .world_mut()
        .spawn((
            AnimationGraphRef {
                path: "animation/rig.animgraph.bsn".to_string(),
                ..AnimationGraphRef::default()
            },
            AnimationGraphSource(asset),
            AnimationSources(vec![source]),
            AnimationParams::default(),
        ))
        .id();
    let body = app
        .world_mut()
        .spawn((Name::new("Body"), ChildOf(root)))
        .id();
    let armature = app
        .world_mut()
        .spawn((Name::new("Armature"), Transform::default(), ChildOf(body)))
        .id();
    app.world_mut()
        .spawn((Name::new("Hips"), Transform::default(), ChildOf(armature)));
    root
}

/// A state that plays one clip out of the rig's only source file.
fn clip_state(name: &str, clip: &str, looped: bool) -> AnimationGraphState {
    AnimationGraphState {
        name: name.to_string(),
        motion: AnimationMotion::Clip(AnimationClipRef {
            source: "rig.glb".to_string(),
            clip: clip.to_string(),
        }),
        looped,
        ..AnimationGraphState::default()
    }
}

fn blend_point(threshold: f32, clip: &str) -> AnimationBlendPoint {
    AnimationBlendPoint {
        threshold,
        clip: AnimationClipRef {
            source: "rig.glb".to_string(),
            clip: clip.to_string(),
        },
    }
}

fn triggered_by(parameter: &str) -> Vec<AnimationCondition> {
    vec![AnimationCondition {
        parameter: parameter.to_string(),
        ..AnimationCondition::default()
    }]
}

fn params(app: &mut App, root: Entity) -> Mut<'_, AnimationParams> {
    app.world_mut()
        .get_mut::<AnimationParams>(root)
        .expect("the rig carries its parameters")
}

fn playing_state(app: &App, root: Entity) -> String {
    app.world()
        .get::<AnimationGraphPlayback>(root)
        .expect("the rig reports what it is playing")
        .state
        .clone()
}

/// What a clip is worth on the player right now, found by the handle rather
/// than by a node index the test would have to know.
fn clip_weight(app: &App, root: Entity, clip: &Handle<AnimationClip>) -> f32 {
    let bound = app
        .world()
        .get::<AnimationGraphBound>(root)
        .expect("the graph is bound");
    let graphs = app.world().resource::<Assets<AnimationGraph>>();
    let graph = graphs.get(&bound.graph).expect("the compiled graph");
    let player = app
        .world()
        .get::<AnimationPlayer>(bound.player)
        .expect("the skeleton carries a player");
    graph
        .nodes()
        .find(|&node| match &graph[node].node_type {
            AnimationNodeType::Clip(held) => held.id() == clip.id(),
            _ => false,
        })
        .and_then(|node| player.animation(node))
        .map_or(0.0, ActiveAnimation::weight)
}

/// The share of the pose a blended state is currently taking.
fn state_weight(app: &App, root: Entity, state: &str) -> f32 {
    let bound = app
        .world()
        .get::<AnimationGraphBound>(root)
        .expect("the graph is bound");
    let graphs = app.world().resource::<Assets<AnimationGraph>>();
    let graph = graphs.get(&bound.graph).expect("the compiled graph");
    graph[bound.node(state).expect("the state compiled")].weight
}

/// The clip a compiled state plays.
fn clip_of(app: &App, root: Entity, state: &str) -> Handle<AnimationClip> {
    let bound = app
        .world()
        .get::<AnimationGraphBound>(root)
        .expect("the graph is bound");
    let graphs = app.world().resource::<Assets<AnimationGraph>>();
    let graph = graphs.get(&bound.graph).expect("the compiled graph");
    match &graph[bound.node(state).expect("the state compiled")].node_type {
        AnimationNodeType::Clip(handle) => handle.clone(),
        other => panic!("state `{state}` is a {other:?}, not a clip"),
    }
}

#[test]
fn a_float_sweep_hands_the_blend_from_idle_to_run() {
    let mut app = animation_app();
    let idle = sliding_clip(&mut app, &["Armature", "Hips"]);
    let walk = sliding_clip(&mut app, &["Armature", "Hips"]);
    let run = sliding_clip(&mut app, &["Armature", "Hips"]);
    let source = source_holding(
        &mut app,
        &[
            ("Idle", idle.clone()),
            ("Walk", walk.clone()),
            ("Run", run.clone()),
        ],
    );
    let def = AnimationGraphDef {
        parameters: vec![AnimationParameterDef {
            name: "speed".to_string(),
            kind: AnimationParameterKind::Float,
            default: 0.0,
        }],
        states: vec![AnimationGraphState {
            name: "locomotion".to_string(),
            motion: AnimationMotion::Blend1d {
                parameter: "speed".to_string(),
                points: vec![
                    blend_point(0.0, "Idle"),
                    blend_point(1.5, "Walk"),
                    blend_point(6.0, "Run"),
                ],
            },
            ..AnimationGraphState::default()
        }],
        transitions: Vec::new(),
        entry: "locomotion".to_string(),
    };
    let root = spawn_graphed_rig(&mut app, def, source);

    step(&mut app, millis(16));
    step(&mut app, millis(16));

    assert_eq!(clip_weight(&app, root, &idle), 1.0, "speed 0 stands still");
    assert_eq!(clip_weight(&app, root, &walk), 0.0);
    assert_eq!(
        state_weight(&app, root, "locomotion"),
        1.0,
        "and the blend is the whole of the pose"
    );

    params(&mut app, root).set_float("speed", 1.5);
    step(&mut app, millis(16));
    assert_eq!(clip_weight(&app, root, &idle), 0.0);
    assert_eq!(clip_weight(&app, root, &walk), 1.0, "1.5 is a walk exactly");

    params(&mut app, root).set_float("speed", 3.75);
    step(&mut app, millis(16));
    let (walking, running) = (
        clip_weight(&app, root, &walk),
        clip_weight(&app, root, &run),
    );
    assert!(
        (walking - 0.5).abs() < 1.0e-4 && (running - 0.5).abs() < 1.0e-4,
        "halfway between the thresholds the two clips share the pose: {walking} and {running}"
    );

    params(&mut app, root).set_float("speed", 9.0);
    step(&mut app, millis(16));
    assert_eq!(
        clip_weight(&app, root, &run),
        1.0,
        "past the last threshold the fastest clip carries it alone"
    );
}

#[test]
fn a_trigger_runs_a_one_shot_state_and_returns_to_the_one_before_it() {
    let mut app = animation_app();
    let clip = sliding_clip(&mut app, &["Armature", "Hips"]);
    let source = source_holding(
        &mut app,
        &[("Idle", clip.clone()), ("Attack", clip.clone())],
    );
    let def = AnimationGraphDef {
        parameters: vec![AnimationParameterDef {
            name: "attack".to_string(),
            kind: AnimationParameterKind::Trigger,
            default: 0.0,
        }],
        states: vec![
            clip_state("idle", "Idle", true),
            clip_state("attack", "Attack", false),
        ],
        transitions: vec![
            AnimationTransitionDef {
                to: "attack".to_string(),
                conditions: triggered_by("attack"),
                crossfade_secs: 0.0,
                ..AnimationTransitionDef::default()
            },
            AnimationTransitionDef {
                from: "attack".to_string(),
                to: "idle".to_string(),
                exit_time: Some(1.0),
                crossfade_secs: 0.0,
                ..AnimationTransitionDef::default()
            },
        ],
        entry: "idle".to_string(),
    };
    let root = spawn_graphed_rig(&mut app, def, source);
    step(&mut app, millis(16));
    assert_eq!(playing_state(&app, root), "idle");

    params(&mut app, root).trigger("attack");
    step(&mut app, millis(16));

    assert_eq!(playing_state(&app, root), "attack");
    assert!(
        !params(&mut app, root).triggered("attack"),
        "the transition that took the trigger cleared it"
    );

    for _ in 0..10 {
        step(&mut app, millis(200));
    }

    assert_eq!(
        playing_state(&app, root),
        "idle",
        "a one-shot state hands back once its clip has run out"
    );
}

#[test]
fn a_transition_waits_for_its_exit_time_before_it_is_taken() {
    let mut app = animation_app();
    let clip = sliding_clip(&mut app, &["Armature", "Hips"]);
    let source = source_holding(&mut app, &[("Idle", clip.clone()), ("Walk", clip.clone())]);
    let def = AnimationGraphDef {
        parameters: Vec::new(),
        states: vec![
            clip_state("idle", "Idle", true),
            clip_state("walk", "Walk", true),
        ],
        transitions: vec![AnimationTransitionDef {
            from: "idle".to_string(),
            to: "walk".to_string(),
            exit_time: Some(0.5),
            crossfade_secs: 0.0,
            ..AnimationTransitionDef::default()
        }],
        entry: "idle".to_string(),
    };
    let root = spawn_graphed_rig(&mut app, def, source);

    step(&mut app, millis(16));
    step(&mut app, millis(100));
    step(&mut app, millis(100));
    assert_eq!(
        playing_state(&app, root),
        "idle",
        "a fifth of the way through the clip the exit time has not come"
    );

    for _ in 0..5 {
        step(&mut app, millis(100));
    }

    assert_eq!(
        playing_state(&app, root),
        "walk",
        "and past the halfway mark of the clip it is taken"
    );
}

#[test]
fn an_any_state_transition_fires_from_every_state() {
    let mut app = animation_app();
    let clip = sliding_clip(&mut app, &["Armature", "Hips"]);
    let source = source_holding(
        &mut app,
        &[
            ("Idle", clip.clone()),
            ("Walk", clip.clone()),
            ("Hit", clip.clone()),
        ],
    );
    let def = AnimationGraphDef {
        parameters: vec![
            AnimationParameterDef {
                name: "speed".to_string(),
                kind: AnimationParameterKind::Float,
                default: 0.0,
            },
            AnimationParameterDef {
                name: "hit".to_string(),
                kind: AnimationParameterKind::Trigger,
                default: 0.0,
            },
        ],
        states: vec![
            clip_state("idle", "Idle", true),
            clip_state("walk", "Walk", true),
            clip_state("hit", "Hit", false),
        ],
        transitions: vec![
            AnimationTransitionDef {
                to: "hit".to_string(),
                conditions: triggered_by("hit"),
                crossfade_secs: 0.0,
                ..AnimationTransitionDef::default()
            },
            AnimationTransitionDef {
                from: "hit".to_string(),
                to: "idle".to_string(),
                exit_time: Some(1.0),
                crossfade_secs: 0.0,
                ..AnimationTransitionDef::default()
            },
            AnimationTransitionDef {
                from: "idle".to_string(),
                to: "walk".to_string(),
                conditions: vec![AnimationCondition {
                    parameter: "speed".to_string(),
                    op: AnimationConditionOp::Greater,
                    value: 1.0,
                }],
                crossfade_secs: 0.0,
                ..AnimationTransitionDef::default()
            },
        ],
        entry: "idle".to_string(),
    };
    let root = spawn_graphed_rig(&mut app, def, source);
    step(&mut app, millis(16));

    params(&mut app, root).trigger("hit");
    step(&mut app, millis(16));
    assert_eq!(
        playing_state(&app, root),
        "hit",
        "the any-state move is taken from the entry state"
    );

    for _ in 0..8 {
        step(&mut app, millis(200));
    }
    params(&mut app, root).set_float("speed", 4.0);
    step(&mut app, millis(16));
    step(&mut app, millis(16));
    assert_eq!(playing_state(&app, root), "walk");

    params(&mut app, root).trigger("hit");
    step(&mut app, millis(16));
    assert_eq!(
        playing_state(&app, root),
        "hit",
        "and from a state reached later, without a wire of its own"
    );
}

#[test]
fn a_graph_file_saved_in_the_binary_form_reads_back_through_the_loader() {
    let dir = tempfile::tempdir().expect("a directory to save into");
    let text = "\
jackdaw_animation_runtime::graph::AnimationGraphDef {
    entry: \"idle\",
    states: [
        jackdaw_animation_runtime::graph::AnimationGraphState {
            name: \"idle\",
            motion: jackdaw_animation_runtime::graph::AnimationMotion::Clip(
                jackdaw_animation_runtime::graph::AnimationClipRef {
                    source: \"rig.glb\",
                    clip: \"Idle\",
                },
            ),
        },
    ],
}
";
    jackdaw_bsn::write_document_text(&dir.path().join("rig.animgraph.bsb"), text)
        .expect("the file is written");

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin {
            file_path: dir.path().to_string_lossy().into_owned(),
            ..AssetPlugin::default()
        })
        .add_plugins(bevy::transform::TransformPlugin)
        .add_plugins(bevy::animation::AnimationPlugin)
        .add_plugins(AnimationRuntimePlugin);

    let handle: Handle<AnimationGraphAsset> = app
        .world()
        .resource::<AssetServer>()
        .load("rig.animgraph.bsb");
    let mut loaded = None;
    for _ in 0..200 {
        app.update();
        if let Some(asset) = app
            .world()
            .resource::<Assets<AnimationGraphAsset>>()
            .get(&handle)
        {
            loaded = Some(asset.def.clone());
            break;
        }
        std::thread::sleep(millis(5));
    }

    assert_eq!(
        loaded.expect("the binary graph file loads").entry,
        "idle",
        "the loader that owns .animgraph.bsn owns its binary twin too"
    );
}

#[test]
fn a_saved_graph_file_reads_back_through_the_loader() {
    let dir = tempfile::tempdir().expect("a directory to save into");
    let text = r#"
jackdaw_animation_runtime::graph::AnimationGraphDef {
    entry: "locomotion",
    parameters: [
        jackdaw_animation_runtime::graph::AnimationParameterDef {
            name: "speed",
        },
        jackdaw_animation_runtime::graph::AnimationParameterDef {
            name: "attack",
            kind: jackdaw_animation_runtime::graph::AnimationParameterKind::Trigger,
        },
    ],
    states: [
        jackdaw_animation_runtime::graph::AnimationGraphState {
            name: "locomotion",
            motion: jackdaw_animation_runtime::graph::AnimationMotion::Blend1d {
                parameter: "speed",
                points: [
                    jackdaw_animation_runtime::graph::AnimationBlendPoint {
                        threshold: 0.0,
                        clip: jackdaw_animation_runtime::graph::AnimationClipRef {
                            source: "rig.glb",
                            clip: "Idle",
                        },
                    },
                    jackdaw_animation_runtime::graph::AnimationBlendPoint {
                        threshold: 6.0,
                        clip: jackdaw_animation_runtime::graph::AnimationClipRef {
                            source: "rig.glb",
                            clip: "Run",
                        },
                    },
                ],
            },
        },
        jackdaw_animation_runtime::graph::AnimationGraphState {
            name: "attack",
            motion: jackdaw_animation_runtime::graph::AnimationMotion::Clip(
                jackdaw_animation_runtime::graph::AnimationClipRef {
                    source: "rig.glb",
                    clip: "Attack",
                },
            ),
            looped: false,
        },
    ],
    transitions: [
        jackdaw_animation_runtime::graph::AnimationTransitionDef {
            to: "attack",
            conditions: [
                jackdaw_animation_runtime::graph::AnimationCondition {
                    parameter: "attack",
                },
            ],
            crossfade_secs: 0.05,
        },
        jackdaw_animation_runtime::graph::AnimationTransitionDef {
            from: "attack",
            to: "locomotion",
            exit_time: Some(1.0),
        },
    ],
}
"#;
    std::fs::write(dir.path().join("rig.animgraph.bsn"), text).expect("the file is written");

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin {
            file_path: dir.path().to_string_lossy().into_owned(),
            ..AssetPlugin::default()
        })
        .add_plugins(bevy::transform::TransformPlugin)
        .add_plugins(bevy::animation::AnimationPlugin)
        .add_plugins(AnimationRuntimePlugin);

    let handle: Handle<AnimationGraphAsset> = app
        .world()
        .resource::<AssetServer>()
        .load("rig.animgraph.bsn");
    let mut loaded = None;
    for _ in 0..200 {
        app.update();
        if let Some(asset) = app
            .world()
            .resource::<Assets<AnimationGraphAsset>>()
            .get(&handle)
        {
            loaded = Some(asset.def.clone());
            break;
        }
        std::thread::sleep(millis(5));
    }
    let def = loaded.expect("the graph file loads");

    assert_eq!(
        def,
        AnimationGraphDef {
            parameters: vec![
                AnimationParameterDef {
                    name: "speed".to_string(),
                    kind: AnimationParameterKind::Float,
                    default: 0.0,
                },
                AnimationParameterDef {
                    name: "attack".to_string(),
                    kind: AnimationParameterKind::Trigger,
                    default: 0.0,
                },
            ],
            states: vec![
                AnimationGraphState {
                    name: "locomotion".to_string(),
                    motion: AnimationMotion::Blend1d {
                        parameter: "speed".to_string(),
                        points: vec![blend_point(0.0, "Idle"), blend_point(6.0, "Run")],
                    },
                    ..AnimationGraphState::default()
                },
                clip_state("attack", "Attack", false),
            ],
            transitions: vec![
                AnimationTransitionDef {
                    to: "attack".to_string(),
                    conditions: triggered_by("attack"),
                    crossfade_secs: 0.05,
                    ..AnimationTransitionDef::default()
                },
                AnimationTransitionDef {
                    from: "attack".to_string(),
                    to: "locomotion".to_string(),
                    exit_time: Some(1.0),
                    ..AnimationTransitionDef::default()
                },
            ],
            entry: "locomotion".to_string(),
        },
        "an elided field keeps what the type's own default gives it"
    );
}

#[test]
fn a_set_converted_to_a_graph_plays_the_same_clip_for_the_same_state_name() {
    let mut app = animation_app();
    let idle = sliding_clip(&mut app, &["Armature", "Hips"]);
    let attack = sliding_clip(&mut app, &["Armature", "Hips"]);
    let source = source_holding(
        &mut app,
        &[("Idle", idle.clone()), ("Attack", attack.clone())],
    );
    let set = AnimationSet {
        sources: vec!["rig.glb".to_string()],
        states: vec![
            AnimationStateDef {
                name: "idle".to_string(),
                clip: "Idle".to_string(),
                ..AnimationStateDef::default()
            },
            AnimationStateDef {
                name: "attack".to_string(),
                clip: "Attack".to_string(),
                looped: false,
                then: Some("idle".to_string()),
                ..AnimationStateDef::default()
            },
        ],
        default_state: "attack".to_string(),
        ..AnimationSet::default()
    };

    let def = set.to_graph();
    assert_eq!(def.entry, "attack", "the default state becomes the entry");
    assert_eq!(
        def.transitions.len(),
        1,
        "and the only state naming what follows it becomes the only transition"
    );
    assert_eq!(def.transitions[0].exit_time, Some(1.0));

    let root = spawn_graphed_rig(&mut app, def, source);
    step(&mut app, millis(16));

    assert_eq!(clip_of(&app, root, "idle").id(), idle.id());
    assert_eq!(clip_of(&app, root, "attack").id(), attack.id());
    assert_eq!(playing_state(&app, root), "attack");

    for _ in 0..10 {
        step(&mut app, millis(200));
    }

    assert_eq!(
        playing_state(&app, root),
        "idle",
        "and the state the set said should follow is where it lands"
    );
}

#[test]
fn a_blend_entered_without_a_fade_carries_the_pose_from_its_first_frame() {
    let mut app = animation_app();
    let idle = sliding_clip(&mut app, &["Armature", "Hips"]);
    let run = sliding_clip(&mut app, &["Armature", "Hips"]);
    let source = source_holding(&mut app, &[("Idle", idle.clone()), ("Run", run.clone())]);
    let def = AnimationGraphDef {
        parameters: Vec::new(),
        states: vec![AnimationGraphState {
            name: "locomotion".to_string(),
            motion: AnimationMotion::Blend1d {
                parameter: "speed".to_string(),
                points: vec![blend_point(0.0, "Idle"), blend_point(6.0, "Run")],
            },
            ..AnimationGraphState::default()
        }],
        transitions: Vec::new(),
        entry: "locomotion".to_string(),
    };
    let root = spawn_graphed_rig(&mut app, def, source);

    step(&mut app, millis(16));

    assert_eq!(
        state_weight(&app, root, "locomotion"),
        1.0,
        "the entry state has no predecessor to fade out, so nothing holds it back"
    );
    assert_eq!(clip_weight(&app, root, &idle), 1.0);
}

#[test]
fn a_crossfade_hands_the_pose_over_and_leaves_no_weight_on_the_state_it_left() {
    let mut app = animation_app();
    let idle = sliding_clip(&mut app, &["Armature", "Hips"]);
    let walk = sliding_clip(&mut app, &["Armature", "Hips"]);
    let source = source_holding(&mut app, &[("Idle", idle.clone()), ("Walk", walk.clone())]);
    let def = AnimationGraphDef {
        parameters: vec![AnimationParameterDef {
            name: "speed".to_string(),
            kind: AnimationParameterKind::Float,
            default: 0.0,
        }],
        states: vec![
            clip_state("idle", "Idle", true),
            clip_state("walk", "Walk", true),
        ],
        transitions: vec![AnimationTransitionDef {
            from: "idle".to_string(),
            to: "walk".to_string(),
            conditions: vec![AnimationCondition {
                parameter: "speed".to_string(),
                op: AnimationConditionOp::Greater,
                value: 1.0,
            }],
            crossfade_secs: 0.2,
            ..AnimationTransitionDef::default()
        }],
        entry: "idle".to_string(),
    };
    let root = spawn_graphed_rig(&mut app, def, source);
    step(&mut app, millis(50));

    params(&mut app, root).set_float("speed", 4.0);
    step(&mut app, millis(50));
    step(&mut app, millis(50));

    let (leaving, arriving) = (
        clip_weight(&app, root, &idle),
        clip_weight(&app, root, &walk),
    );
    assert!(
        leaving > 0.0 && arriving > 0.0 && (leaving + arriving - 1.0).abs() < 1.0e-4,
        "mid-fade the two clips share the pose between them: {leaving} and {arriving}"
    );

    for _ in 0..6 {
        step(&mut app, millis(50));
    }

    assert_eq!(
        clip_weight(&app, root, &idle),
        0.0,
        "and once the fade has run out the state it left is worth nothing"
    );
    assert_eq!(clip_weight(&app, root, &walk), 1.0);
}

#[test]
fn an_edited_graph_takes_the_moves_and_the_states_the_edit_added() {
    let mut app = animation_app();
    let idle = sliding_clip(&mut app, &["Armature", "Hips"]);
    let walk = sliding_clip(&mut app, &["Armature", "Hips"]);
    let source = source_holding(&mut app, &[("Idle", idle.clone()), ("Walk", walk.clone())]);
    let def = AnimationGraphDef {
        parameters: Vec::new(),
        states: vec![clip_state("idle", "Idle", true)],
        transitions: Vec::new(),
        entry: "idle".to_string(),
    };
    let root = spawn_graphed_rig(&mut app, def, source);
    step(&mut app, millis(16));
    assert_eq!(playing_state(&app, root), "idle");

    let handle = app
        .world()
        .get::<AnimationGraphSource>(root)
        .expect("the rig holds the file it plays")
        .0
        .clone();
    {
        let mut assets = app
            .world_mut()
            .resource_mut::<Assets<AnimationGraphAsset>>();
        let mut asset = assets.get_mut(&handle).expect("the graph is in hand");
        asset.def.states.push(clip_state("walk", "Walk", true));
        asset.def.transitions.push(AnimationTransitionDef {
            from: "idle".to_string(),
            to: "walk".to_string(),
            crossfade_secs: 0.0,
            ..AnimationTransitionDef::default()
        });
    }

    step(&mut app, millis(16));
    step(&mut app, millis(16));

    assert_eq!(
        playing_state(&app, root),
        "walk",
        "a move to a state the edit added is one the graph can take"
    );
    assert_eq!(clip_of(&app, root, "walk").id(), walk.id());
}

#[test]
fn a_graph_reference_naming_no_file_leaves_the_set_beside_it_to_play() {
    let mut app = animation_app();
    let idle = sliding_clip(&mut app, &["Armature", "Hips"]);
    let source = source_holding(&mut app, &[("Idle", idle.clone())]);
    let root = app
        .world_mut()
        .spawn((
            AnimationSet {
                sources: vec!["rig.glb".to_string()],
                states: vec![AnimationStateDef {
                    name: "idle".to_string(),
                    clip: "Idle".to_string(),
                    ..AnimationStateDef::default()
                }],
                default_state: "idle".to_string(),
                ..AnimationSet::default()
            },
            AnimationGraphRef::default(),
            AnimationSources(vec![source]),
        ))
        .id();
    let armature = app
        .world_mut()
        .spawn((Name::new("Armature"), Transform::default(), ChildOf(root)))
        .id();
    app.world_mut()
        .spawn((Name::new("Hips"), Transform::default(), ChildOf(armature)));

    step(&mut app, millis(16));
    step(&mut app, millis(16));

    let bound = app
        .world()
        .get::<AnimationSetBound>(root)
        .expect("a graph reference with no file behind it holds nothing back");
    assert_eq!(
        app.world().get::<AnimationState>(root),
        Some(&AnimationState("idle".to_string()))
    );
    let node = bound.nodes["idle"];
    assert!(
        app.world()
            .get::<AnimationPlayer>(bound.player)
            .expect("the skeleton carries a player")
            .animation(node)
            .is_some(),
        "and its default state is playing on the skeleton"
    );
}

#[test]
fn a_blend_state_fires_the_markers_of_the_clip_it_leads_with() {
    let mut app = animation_app();
    let idle = sliding_clip(&mut app, &["Armature", "Hips"]);
    let walk = sliding_clip(&mut app, &["Armature", "Hips"]);
    let source = source_holding(&mut app, &[("Idle", idle), ("Walk", walk)]);
    let def = AnimationGraphDef {
        parameters: vec![AnimationParameterDef {
            name: "speed".to_string(),
            kind: AnimationParameterKind::Float,
            default: 0.0,
        }],
        states: vec![AnimationGraphState {
            name: "locomotion".to_string(),
            motion: AnimationMotion::Blend1d {
                parameter: "speed".to_string(),
                points: vec![blend_point(0.0, "Idle"), blend_point(1.5, "Walk")],
            },
            ..AnimationGraphState::default()
        }],
        transitions: Vec::new(),
        entry: "locomotion".to_string(),
    };
    let root = spawn_graphed_rig(&mut app, def, source);
    clip_marker(&mut app, root, "Idle", 0.4, "breath");
    clip_marker(&mut app, root, "Walk", 0.4, "footfall");

    step(&mut app, Duration::ZERO);
    step(&mut app, Duration::ZERO);
    params(&mut app, root).set_float("speed", 1.5);
    for _ in 0..8 {
        step(&mut app, millis(100));
    }

    assert_eq!(
        app.world().resource::<Fired>().0,
        vec!["footfall".to_string()],
        "the clip carrying the blend sends its markers, and the one at no \
         weight sends none"
    );
}

#[test]
fn a_clip_that_takes_the_blend_partway_through_sends_no_marker_behind_it() {
    let mut app = animation_app();
    let idle = sliding_clip(&mut app, &["Armature", "Hips"]);
    let walk = sliding_clip(&mut app, &["Armature", "Hips"]);
    let source = source_holding(&mut app, &[("Idle", idle), ("Walk", walk)]);
    let def = AnimationGraphDef {
        parameters: vec![AnimationParameterDef {
            name: "speed".to_string(),
            kind: AnimationParameterKind::Float,
            default: 0.0,
        }],
        states: vec![AnimationGraphState {
            name: "locomotion".to_string(),
            motion: AnimationMotion::Blend1d {
                parameter: "speed".to_string(),
                points: vec![blend_point(0.0, "Idle"), blend_point(1.5, "Walk")],
            },
            ..AnimationGraphState::default()
        }],
        transitions: Vec::new(),
        entry: "locomotion".to_string(),
    };
    let root = spawn_graphed_rig(&mut app, def, source);
    clip_marker(&mut app, root, "Walk", 0.2, "footfall");

    step(&mut app, Duration::ZERO);
    step(&mut app, Duration::ZERO);
    for _ in 0..6 {
        step(&mut app, millis(100));
    }
    params(&mut app, root).set_float("speed", 1.5);
    for _ in 0..3 {
        step(&mut app, millis(100));
    }

    assert!(
        app.world().resource::<Fired>().0.is_empty(),
        "a clip taking the lead at 0.6s starts from where it stands, rather \
         than sending every marker below it at once: {:?}",
        app.world().resource::<Fired>().0
    );

    for _ in 0..6 {
        step(&mut app, millis(100));
    }

    assert_eq!(
        app.world().resource::<Fired>().0,
        vec!["footfall".to_string()],
        "and it sends the marker on the pass that reaches it"
    );
}

/// A graph file says what it is, so one under any name in any folder loads
/// like one written as `animation/<name>.animgraph.bsn`.
#[test]
fn a_graph_file_in_another_folder_loads_under_a_plain_bsn_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("rigs")).expect("the folder is made");
    let text = "// jackdaw asset jackdaw_animation_runtime::graph::AnimationGraphDef\n\
                #hero\n\
                jackdaw_animation_runtime::graph::AnimationGraphDef {\n\
                    entry: \"idle\",\n\
                }\n";
    std::fs::write(dir.path().join("rigs/hero.bsn"), text).expect("the file is written");

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin {
            file_path: dir.path().to_string_lossy().into_owned(),
            ..AssetPlugin::default()
        })
        .add_plugins(bevy::transform::TransformPlugin)
        .add_plugins(bevy::animation::AnimationPlugin)
        .add_plugins(AnimationRuntimePlugin);

    let handle: Handle<AnimationGraphAsset> =
        app.world().resource::<AssetServer>().load("rigs/hero.bsn");
    let mut loaded = None;
    for _ in 0..200 {
        app.update();
        if let Some(asset) = app
            .world()
            .resource::<Assets<AnimationGraphAsset>>()
            .get(&handle)
        {
            loaded = Some(asset.def.clone());
            break;
        }
        std::thread::sleep(millis(5));
    }

    assert_eq!(loaded.expect("the graph file loads").entry, "idle");
}
