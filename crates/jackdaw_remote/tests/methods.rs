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
