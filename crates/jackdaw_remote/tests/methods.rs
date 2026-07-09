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
