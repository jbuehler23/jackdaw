//! The animation library, and previewing what it lists.
//!
//! Clips are an index the editor builds by asking each glTF file once, not
//! entities a document accumulates, and previewing one borrows whatever player
//! is already on the target rather than replacing it.

use crate::util;

use bevy::animation::{AnimationPlayer, AnimationTargetId, graph::AnimationGraphHandle};
use bevy::prelude::*;
use jackdaw::animation::AnimationLibrary;
use jackdaw_animation_runtime::{
    AnimationSet, AnimationSetBound, AnimationState, AnimationStateDef,
};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_commands::CommandHistory;
use jackdaw_scene_types::PropertyValue;
use serde_json::{Value, json};

/// How many `AssetEvent::Modified` the glTF assets published, across every
/// tick since the counter was installed.
#[derive(Resource, Default)]
struct GltfReloads(usize);

fn count_gltf_reloads(
    mut events: MessageReader<bevy::asset::AssetEvent<bevy::gltf::Gltf>>,
    mut count: ResMut<GltfReloads>,
) {
    count.0 += events
        .read()
        .filter(|event| matches!(event, bevy::asset::AssetEvent::Modified { .. }))
        .count();
}

const ANIMATED_FILE: &str = "jan/jan.gltf";

/// An editor with this repository's own assets directory open, which is where
/// the animated glTF the library is asked about lives.
fn editor_on_the_test_project() -> App {
    let mut app = util::editor_test_app();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            config: default(),
        });
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app
}

/// Tick until `ready` says so, or fail saying what was still missing.
#[track_caller]
fn settle_until(app: &mut App, what: &str, ready: impl Fn(&App) -> bool) {
    for _ in 0..600 {
        if ready(app) {
            return;
        }
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("{what} never happened");
}

fn call(app: &mut App, id: &'static str, params: &[(&'static str, PropertyValue)]) {
    let mut call = app.world_mut().operator(id).settings(CallOperatorSettings {
        execution_context: ExecutionContext::Invoke,
        creates_history_entry: true,
    });
    for (key, value) in params {
        call = call.param(*key, value.clone());
    }
    let result = call.call().expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Finished, "{id} did not finish");
}

fn place_animated_model(app: &mut App) {
    call(
        app,
        "entity.place_gltf",
        &[
            ("path", ANIMATED_FILE.into()),
            ("pos_x", 0.0f64.into()),
            ("pos_y", 0.0f64.into()),
            ("pos_z", 0.0f64.into()),
        ],
    );
}

fn library_has_the_animated_file(app: &App) -> bool {
    app.world()
        .resource::<AnimationLibrary>()
        .file(ANIMATED_FILE)
        .is_some()
}

#[test]
fn the_library_lists_every_clip_of_a_loaded_file() {
    let mut app = editor_on_the_test_project();
    place_animated_model(&mut app);
    settle_until(&mut app, "the library indexed the file", |app| {
        library_has_the_animated_file(app)
    });

    let library = app.world().resource::<AnimationLibrary>();
    let file = library.file(ANIMATED_FILE).expect("the file is indexed");
    let mut names: Vec<&str> = file.clips.iter().map(|clip| clip.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["dance", "idle", "run", "typing", "walk"]);
    assert!(
        file.clips.iter().all(|clip| clip.duration_secs > 0.0),
        "every clip should carry the length its file gave it: {:?}",
        file.clips
    );
}

#[test]
fn discovery_no_longer_writes_clip_entities_into_the_document() {
    let mut app = editor_on_the_test_project();
    place_animated_model(&mut app);
    settle_until(&mut app, "the library indexed the file", |app| {
        library_has_the_animated_file(app)
    });
    // Long enough that anything spawning clips per frame would have.
    for _ in 0..30 {
        app.update();
    }

    let clips = app
        .world_mut()
        .query::<&jackdaw_animation::Clip>()
        .iter(app.world())
        .count();
    assert_eq!(
        clips, 0,
        "a placed model's clips belong to the library, not to the document"
    );
}

#[test]
fn a_document_holding_old_clip_children_loads_clean_and_saves_without_them() {
    let mut app = editor_on_the_test_project();
    place_animated_model(&mut app);
    app.update();

    let model = app
        .world_mut()
        .query_filtered::<Entity, With<jackdaw_scene_types::GltfSource>>()
        .single(app.world())
        .expect("one placed model");

    // What a document written before the library looked like: one clip entity
    // per animation, registered in the document under the model.
    let stale = app
        .world_mut()
        .spawn((
            jackdaw_animation::Clip::default(),
            jackdaw_animation::GltfClipRef {
                gltf_path: ANIMATED_FILE.to_string(),
                clip_name: "run".to_string(),
            },
            Name::new("run"),
            ChildOf(model),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), stale);
    assert!(
        app.world()
            .resource::<jackdaw_bsn::SceneBsnAst>()
            .ast_for(stale)
            .is_some(),
        "the stale clip has to be in the document for the test to mean anything"
    );

    app.update();
    app.update();

    assert!(
        app.world().get_entity(stale).is_err(),
        "loading a document that still carries imported clips should drop them"
    );
    let bsn = app
        .world_mut()
        .run_system_cached_with(jackdaw::remote::server::scene_bsn_handler, None)
        .expect("the handler ran")
        .expect("the document emitted")["bsn"]
        .as_str()
        .expect("BSN text")
        .to_string();
    assert!(
        !bsn.contains("GltfClipRef"),
        "a resave must not write the imported clips back: {bsn}"
    );
}

/// A skeleton the runtime can bind an authored set to: a named root the set
/// points at, with a bone under it.
fn spawn_bound_rig(app: &mut App) -> Entity {
    let rig = app
        .world_mut()
        .spawn((
            Name::new("Rig"),
            AnimationSet {
                sources: vec![ANIMATED_FILE.to_string()],
                states: vec![AnimationStateDef {
                    name: "Idle".to_string(),
                    source: 0,
                    clip: "idle".to_string(),
                    ..default()
                }],
                default_state: "Idle".to_string(),
                skeleton_root: "Armature".to_string(),
            },
            Transform::default(),
        ))
        .id();
    let armature = app
        .world_mut()
        .spawn((Name::new("Armature"), Transform::default(), ChildOf(rig)))
        .id();
    app.world_mut()
        .spawn((Name::new("Bone"), Transform::default(), ChildOf(armature)));
    rig
}

#[test]
fn previewing_a_clip_on_a_bound_set_restores_the_set_state_on_stop() {
    let mut app = editor_on_the_test_project();
    let rig = spawn_bound_rig(&mut app);
    settle_until(&mut app, "the set bound to its skeleton", |app| {
        app.world().get::<AnimationSetBound>(rig).is_some()
    });

    let bound = app.world().get::<AnimationSetBound>(rig).expect("bound");
    let player = bound.player;
    let own_graph = bound.graph.id();
    let idle_node = *bound.nodes.get("Idle").expect("the set plays Idle");

    call(
        &mut app,
        "animation.preview",
        &[
            ("clip", format!("{ANIMATED_FILE}#run").into()),
            ("entity", rig.into()),
        ],
    );
    settle_until(&mut app, "the preview took the player", |app| {
        app.world()
            .get::<AnimationGraphHandle>(player)
            .is_some_and(|handle| handle.0.id() != own_graph)
    });

    call(&mut app, "animation.preview.stop", &[]);
    for _ in 0..5 {
        app.update();
    }

    assert_eq!(
        app.world()
            .get::<AnimationGraphHandle>(player)
            .map(|handle| handle.0.id()),
        Some(own_graph),
        "stopping must hand the set's own graph back"
    );
    assert_eq!(
        app.world().get::<AnimationState>(rig),
        Some(&AnimationState("Idle".to_string()))
    );
    assert!(
        app.world()
            .get::<AnimationPlayer>(player)
            .is_some_and(|player| player.animation(idle_node).is_some()),
        "the set should be playing the state it was asked for again"
    );
}

#[test]
fn previewing_on_a_bare_skeleton_leaves_no_player_behind() {
    let mut app = editor_on_the_test_project();
    let rig = app
        .world_mut()
        .spawn((Name::new("Rig"), Transform::default()))
        .id();
    let bone = app
        .world_mut()
        .spawn((Name::new("Bone"), Transform::default(), ChildOf(rig)))
        .id();

    call(
        &mut app,
        "animation.preview",
        &[
            ("clip", format!("{ANIMATED_FILE}#run").into()),
            ("entity", rig.into()),
        ],
    );
    settle_until(&mut app, "the preview installed a player", |app| {
        app.world().get::<AnimationPlayer>(rig).is_some()
    });

    call(&mut app, "animation.preview.stop", &[]);
    app.update();

    assert!(
        app.world().get::<AnimationPlayer>(rig).is_none(),
        "the player the preview installed has to go with it"
    );
    assert!(app.world().get::<AnimationGraphHandle>(rig).is_none());
    for entity in [rig, bone] {
        assert!(
            app.world().get::<AnimationTargetId>(entity).is_none(),
            "a target id the preview wrote has to go with it too"
        );
    }
}

#[test]
fn add_as_state_appends_to_the_selected_set_and_undoes() {
    let mut app = editor_on_the_test_project();
    let rig = app
        .world_mut()
        .spawn((
            Name::new("Rig"),
            AnimationSet::default(),
            Transform::default(),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), rig);
    jackdaw::selection::select_only(app.world_mut(), rig);
    settle_until(&mut app, "the library indexed the file", |app| {
        library_has_the_animated_file(app)
    });

    {
        let mut panel = app
            .world_mut()
            .resource_mut::<jackdaw::animation::AnimationPanelState>();
        panel.file = Some(ANIMATED_FILE.to_string());
        panel.clip = Some("run".to_string());
    }
    call(&mut app, "animation.library.add_state", &[]);
    app.update();

    let set = app.world().get::<AnimationSet>(rig).expect("a set").clone();
    assert_eq!(set.sources, vec![ANIMATED_FILE.to_string()]);
    assert_eq!(set.states.len(), 1, "{set:?}");
    assert_eq!(set.states[0].name, "run");
    assert_eq!(set.states[0].clip, "run");
    assert_eq!(set.states[0].source, 0);

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    let set = app.world().get::<AnimationSet>(rig).expect("a set");
    assert!(
        set.states.is_empty() && set.sources.is_empty(),
        "one undo has to take back both the state and the source it needed: {set:?}"
    );
}

#[test]
fn the_assets_method_reports_clip_names_with_details() {
    let mut app = editor_on_the_test_project();
    place_animated_model(&mut app);
    settle_until(&mut app, "the library indexed the file", |app| {
        library_has_the_animated_file(app)
    });

    let answer = poll_assets(&mut app, json!({ "details": true, "request": "details" }));
    let listed = answer["assets"].as_array().expect("an array");
    let entry = listed
        .iter()
        .find(|entry| entry["path"] == json!(ANIMATED_FILE))
        .unwrap_or_else(|| panic!("the animated file was not listed: {answer}"));
    assert_eq!(entry["kind"], json!("model"), "{entry}");
    let clips: Vec<&str> = entry["clips"]
        .as_array()
        .expect("an array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(clips.contains(&"run"), "{entry}");

    let plain = poll_assets(&mut app, json!({ "request": "plain" }));
    assert!(
        plain["assets"]
            .as_array()
            .expect("an array")
            .iter()
            .any(|path| path == &json!(ANIMATED_FILE)),
        "without details the listing stays a list of paths: {plain}"
    );
}

/// Poll the watching assets method until it answers.
#[track_caller]
fn poll_assets(app: &mut App, params: Value) -> Value {
    for _ in 0..600 {
        let answer = app
            .world_mut()
            .run_system_cached_with(
                jackdaw::remote::server::assets_handler,
                Some(params.clone()),
            )
            .expect("the handler ran")
            .unwrap_or_else(|err| panic!("the handler refused: {}", err.message));
        if let Some(answer) = answer {
            return answer;
        }
        app.update();
    }
    panic!("the assets method never answered");
}

/// The library parks the handle of the file it is asking about rather than
/// dropping it and asking again next frame, which republishes
/// `AssetEvent::Modified` and respawns every instance of the model.
#[test]
fn indexing_settles_without_reloading_the_gltf() {
    let mut app = editor_on_the_test_project();
    app.init_resource::<GltfReloads>();
    app.add_systems(Last, count_gltf_reloads);

    place_animated_model(&mut app);
    call(
        &mut app,
        "entity.place_gltf",
        &[
            ("path", "models/dungeon.glb".into()),
            ("pos_x", 4.0f64.into()),
            ("pos_y", 0.0f64.into()),
            ("pos_z", 0.0f64.into()),
        ],
    );
    settle_until(&mut app, "the library indexed the file", |app| {
        library_has_the_animated_file(app)
    });
    // Long enough for the file with no clips to be asked about too.
    for _ in 0..120 {
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }

    app.world_mut().resource_mut::<GltfReloads>().0 = 0;
    for _ in 0..60 {
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(
        app.world().resource::<GltfReloads>().0,
        0,
        "a settled project must not reload its glTFs"
    );
}
