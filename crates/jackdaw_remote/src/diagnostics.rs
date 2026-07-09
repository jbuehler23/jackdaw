//! The `jackdaw/diagnostics` BRP method: frame stats for the explorer's
//! stats page and status bar.

use bevy::diagnostic::{Diagnostic, DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use bevy::remote::BrpResult;
use serde_json::{Value, json};

/// Returns fps, frame time, and entity count. Fields are null until the
/// diagnostics have gathered enough frames to smooth.
pub fn jackdaw_diagnostics_handler(In(_params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let store = world.get_resource::<DiagnosticsStore>();
    let fps = store
        .and_then(|s| s.get(&FrameTimeDiagnosticsPlugin::FPS))
        .and_then(Diagnostic::smoothed);
    let frame_time_ms = store
        .and_then(|s| s.get(&FrameTimeDiagnosticsPlugin::FRAME_TIME))
        .and_then(Diagnostic::smoothed);
    let entity_count = world.entities().len();

    Ok(json!({
        "fps": fps,
        "frame_time_ms": frame_time_ms,
        "entity_count": entity_count,
    }))
}
