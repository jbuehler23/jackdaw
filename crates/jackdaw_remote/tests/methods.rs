use bevy::prelude::*;
use bevy::remote::BrpResult;
use serde_json::Value;

fn run_method<M, S>(app: &mut App, handler: S, params: Option<Value>) -> BrpResult
where
    S: IntoSystem<In<Option<Value>>, BrpResult, M> + 'static,
{
    let id = app.world_mut().register_system(handler);
    app.world_mut()
        .run_system_with(id, params)
        .expect("system ran")
}

use bevy::diagnostic::{DiagnosticsPlugin, FrameTimeDiagnosticsPlugin};

#[test]
fn diagnostics_reports_entity_count_and_keys() {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        DiagnosticsPlugin,
        FrameTimeDiagnosticsPlugin::default(),
    ));
    app.world_mut().spawn_empty();
    app.world_mut().spawn_empty();
    app.update();
    app.update();

    let result = run_method(
        &mut app,
        jackdaw_remote::diagnostics::jackdaw_diagnostics_handler,
        None,
    )
    .expect("handler ok");

    assert!(result.get("fps").is_some());
    assert!(result.get("frame_time_ms").is_some());
    assert!(result["entity_count"].as_u64().unwrap() >= 2);
}

use serde_json::json;

/// Captures whether virtual time advanced during the last update.
#[derive(Resource, Default)]
struct SawVirtualDelta(bool);

fn record_virtual_delta(time: Res<Time<Virtual>>, mut saw: ResMut<SawVirtualDelta>) {
    saw.0 = !time.delta().is_zero();
}

fn playback_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(jackdaw_remote::playback::PlaybackTestSupport);
    app.init_resource::<SawVirtualDelta>();
    app.add_systems(Update, record_virtual_delta);
    app.update();
    app
}

#[test]
fn pause_freezes_virtual_time_and_resume_unfreezes() {
    let mut app = playback_app();

    let result = run_method(
        &mut app,
        jackdaw_remote::playback::jackdaw_playback_handler,
        Some(json!({"action": "pause"})),
    )
    .expect("pause ok");
    assert_eq!(result["paused"], json!(true));

    std::thread::sleep(std::time::Duration::from_millis(5));
    app.update();
    assert!(
        !app.world().resource::<SawVirtualDelta>().0,
        "paused sim must not advance"
    );

    let result = run_method(
        &mut app,
        jackdaw_remote::playback::jackdaw_playback_handler,
        Some(json!({"action": "resume"})),
    )
    .expect("resume ok");
    assert_eq!(result["paused"], json!(false));

    std::thread::sleep(std::time::Duration::from_millis(5));
    app.update();
    assert!(
        app.world().resource::<SawVirtualDelta>().0,
        "resumed sim must advance"
    );
}

#[test]
fn step_advances_exactly_one_frame_while_paused() {
    let mut app = playback_app();

    run_method(
        &mut app,
        jackdaw_remote::playback::jackdaw_playback_handler,
        Some(json!({"action": "pause"})),
    )
    .expect("pause ok");
    app.update();

    run_method(
        &mut app,
        jackdaw_remote::playback::jackdaw_playback_handler,
        Some(json!({"action": "step"})),
    )
    .expect("step ok");

    std::thread::sleep(std::time::Duration::from_millis(5));
    app.update();
    assert!(
        app.world().resource::<SawVirtualDelta>().0,
        "step frame must advance"
    );

    std::thread::sleep(std::time::Duration::from_millis(5));
    app.update();
    assert!(
        !app.world().resource::<SawVirtualDelta>().0,
        "frame after step must be paused again"
    );
}

#[test]
fn step_while_running_is_an_error() {
    let mut app = playback_app();
    let result = run_method(
        &mut app,
        jackdaw_remote::playback::jackdaw_playback_handler,
        Some(json!({"action": "step"})),
    );
    assert!(result.is_err());
}

#[test]
fn unknown_action_is_invalid_params() {
    let mut app = playback_app();
    let result = run_method(
        &mut app,
        jackdaw_remote::playback::jackdaw_playback_handler,
        Some(json!({"action": "reverse"})),
    );
    assert_eq!(
        result.unwrap_err().code,
        bevy::remote::error_codes::INVALID_PARAMS
    );
}

#[test]
fn resume_after_step_keeps_running() {
    let mut app = playback_app();

    run_method(
        &mut app,
        jackdaw_remote::playback::jackdaw_playback_handler,
        Some(json!({"action": "pause"})),
    )
    .expect("pause ok");
    app.update();

    run_method(
        &mut app,
        jackdaw_remote::playback::jackdaw_playback_handler,
        Some(json!({"action": "step"})),
    )
    .expect("step ok");
    app.update();

    // The trailing re-pause for the step frame is still pending here; resume
    // must cancel it, not be swallowed by it.
    run_method(
        &mut app,
        jackdaw_remote::playback::jackdaw_playback_handler,
        Some(json!({"action": "resume"})),
    )
    .expect("resume ok");

    std::thread::sleep(std::time::Duration::from_millis(5));
    app.update();
    assert!(
        app.world().resource::<SawVirtualDelta>().0,
        "resume after step must keep the sim running"
    );
}

const APPLY_BSN_SOURCE: &str = r#"
#Spawned
bevy_transform::components::transform::Transform {
    translation: glam::Vec3 { x: 1.0, y: 2.0, z: 3.0 },
}
bevy_ecs::hierarchy::Children [
    #SpawnedChild
    bevy_transform::components::transform::Transform
]
"#;

fn bsn_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.register_type::<Transform>();
    app.register_type::<Name>();
    app
}

#[test]
fn apply_bsn_spawns_entities_with_components() {
    let mut app = bsn_app();
    let result = run_method(
        &mut app,
        jackdaw_remote::bsn_methods::jackdaw_apply_bsn_handler,
        Some(json!({"source": APPLY_BSN_SOURCE})),
    )
    .expect("apply ok");

    let spawned = result["entities"].as_array().expect("entities array");
    assert_eq!(spawned.len(), 2, "root and child");

    let world = app.world_mut();
    let root = world
        .query::<(&Name, &Transform)>()
        .iter(world)
        .find(|(name, _)| name.as_str() == "Spawned")
        .expect("root spawned with Name");
    assert_eq!(root.1.translation, Vec3::new(1.0, 2.0, 3.0));

    let children_count = world
        .query::<(&Name, &Children)>()
        .iter(world)
        .find(|(name, _)| name.as_str() == "Spawned")
        .map(|(_, children)| children.len())
        .expect("root has Children");
    assert_eq!(children_count, 1);
}

#[test]
fn apply_bsn_parse_error_is_reported() {
    let mut app = bsn_app();
    let result = run_method(
        &mut app,
        jackdaw_remote::bsn_methods::jackdaw_apply_bsn_handler,
        Some(json!({"source": "bevy_transform::components::transform::Transform {"})),
    );
    let err = result.unwrap_err();
    assert_eq!(err.code, bevy::remote::error_codes::INVALID_PARAMS);
    assert!(!err.message.is_empty());
}

#[test]
fn apply_bsn_without_source_is_invalid_params() {
    let mut app = bsn_app();
    let result = run_method(
        &mut app,
        jackdaw_remote::bsn_methods::jackdaw_apply_bsn_handler,
        Some(json!({})),
    );
    assert_eq!(
        result.unwrap_err().code,
        bevy::remote::error_codes::INVALID_PARAMS
    );
}

#[test]
fn apply_bsn_preserves_host_scene_ast() {
    use jackdaw_bsn::SceneBsnAst;

    let mut app = bsn_app();
    let mut host_ast = SceneBsnAst::default();
    let node = host_ast.create_entity_node(Vec::new());
    host_ast.add_to_roots(node);
    app.world_mut().insert_resource(host_ast);

    run_method(
        &mut app,
        jackdaw_remote::bsn_methods::jackdaw_apply_bsn_handler,
        Some(json!({"source": APPLY_BSN_SOURCE})),
    )
    .expect("apply ok");

    let restored = app
        .world()
        .get_resource::<SceneBsnAst>()
        .expect("host SceneBsnAst restored after apply");
    assert_eq!(restored.roots.len(), 1, "host AST roots preserved");
}

#[test]
fn entity_bsn_emits_components_and_children() {
    let mut app = bsn_app();
    let child = app
        .world_mut()
        .spawn(Transform::from_xyz(0.0, 1.0, 0.0))
        .id();
    let root = app
        .world_mut()
        .spawn((Name::new("Exported"), Transform::from_xyz(4.0, 5.0, 6.0)))
        .add_child(child)
        .id();

    let result = run_method(
        &mut app,
        jackdaw_remote::bsn_methods::jackdaw_entity_bsn_handler,
        Some(json!({"entity": root.to_bits()})),
    )
    .expect("emit ok");

    let bsn = result["bsn"].as_str().expect("bsn text");
    assert!(bsn.contains("#Exported"), "got: {bsn}");
    assert!(
        bsn.contains("bevy_transform::components::transform::Transform"),
        "got: {bsn}"
    );
    assert!(bsn.contains("Children ["), "got: {bsn}");

    // Round trip: the emitted text must be valid BSN.
    jackdaw_bsn::parse_bsn_text(bsn).expect("emitted BSN parses");
}

#[test]
fn entity_bsn_unknown_entity_is_invalid_params() {
    let mut app = bsn_app();
    // Despawning frees the entity; its stale bits no longer resolve to a
    // live entity, unlike an arbitrary raw u64. Under bevy 0.19's NonMaxU32
    // index encoding, u32::MAX bits decode to a live entity, so a spawned-
    // then-despawned entity's stale bits are used instead.
    let despawned = app.world_mut().spawn_empty().id();
    app.world_mut().despawn(despawned);

    let result = run_method(
        &mut app,
        jackdaw_remote::bsn_methods::jackdaw_entity_bsn_handler,
        Some(json!({"entity": despawned.to_bits()})),
    );
    assert_eq!(
        result.unwrap_err().code,
        bevy::remote::error_codes::INVALID_PARAMS
    );
}

#[test]
fn entity_bsn_preserves_host_scene_ast() {
    use jackdaw_bsn::SceneBsnAst;

    let mut app = bsn_app();
    let target = app.world_mut().spawn(Transform::default()).id();

    let mut host_ast = SceneBsnAst::default();
    let node = host_ast.create_entity_node(Vec::new());
    host_ast.add_to_roots(node);
    app.world_mut().insert_resource(host_ast);

    run_method(
        &mut app,
        jackdaw_remote::bsn_methods::jackdaw_entity_bsn_handler,
        Some(json!({"entity": target.to_bits()})),
    )
    .expect("emit ok");

    let restored = app
        .world()
        .get_resource::<SceneBsnAst>()
        .expect("host SceneBsnAst restored after entity_bsn");
    assert_eq!(restored.roots.len(), 1, "host AST roots preserved");
}

#[test]
fn archetypes_counts_cover_spawned_entities() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.world_mut().spawn(Transform::default());
    app.world_mut().spawn(Transform::default());
    app.world_mut()
        .spawn((Transform::default(), Name::new("n")));
    app.update();

    let result = run_method(
        &mut app,
        jackdaw_remote::ecs_methods::jackdaw_archetypes_handler,
        None,
    )
    .expect("ok");

    let archetypes = result["archetypes"].as_array().expect("array");
    assert!(!archetypes.is_empty());

    // Transform requires GlobalTransform and TransformTreeChanged, so the
    // "Transform-only" archetype (the two entities spawned without a Name)
    // carries all three; distinguish it from the Name-bearing archetype by
    // the absence of Name rather than by component count.
    let transform_only = archetypes
        .iter()
        .find(|a| {
            let comps = a["components"].as_array().unwrap();
            let strs: Vec<&str> = comps.iter().map(|c| c.as_str().unwrap()).collect();
            strs.iter().any(|c| c.contains("::transform::Transform"))
                && !strs.iter().any(|c| c.contains("::name::Name"))
        })
        .expect("Transform-only archetype listed");
    assert_eq!(transform_only["entity_count"], json!(2));

    // Sorted by count descending.
    let counts: Vec<u64> = archetypes
        .iter()
        .map(|a| a["entity_count"].as_u64().unwrap())
        .collect();
    let mut sorted = counts.clone();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(counts, sorted);
}

fn named_probe_system() {}

#[test]
fn schedules_lists_systems_in_run_order() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_systems(Update, named_probe_system);
    app.update();

    let result = run_method(
        &mut app,
        jackdaw_remote::ecs_methods::jackdaw_schedules_handler,
        None,
    )
    .expect("ok");

    let schedules = result["schedules"].as_array().expect("array");
    // Exact match: several schedule labels (PreUpdate, PostUpdate,
    // FixedUpdate, ...) contain "Update" as a substring.
    let update = schedules
        .iter()
        .find(|s| s["schedule"].as_str().unwrap() == "Update")
        .expect("Update schedule listed");
    assert_eq!(update["initialized"], json!(true));
    let systems: Vec<&str> = update["systems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap())
        .collect();
    assert!(
        systems.iter().any(|s| s.contains("named_probe_system")),
        "got: {systems:?}"
    );
}
