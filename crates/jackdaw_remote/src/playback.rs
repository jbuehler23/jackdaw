//! The `jackdaw/playback` BRP method: pause, resume, and single-step the
//! game's virtual clock. Rendering and BRP keep running while paused, so
//! the frozen world stays fully inspectable.

use bevy::prelude::*;
use bevy::remote::{BrpError, BrpResult, error_codes};
use serde_json::{Value, json};

/// Pending single-step requests. `stepping` marks that the current frame is
/// a step frame so the following frame re-pauses.
#[derive(Resource, Default)]
pub struct PlaybackStepState {
    pub pending: u32,
    stepping: bool,
}

pub fn jackdaw_playback_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let action = params
        .as_ref()
        .and_then(|p| p.get("action"))
        .and_then(|a| a.as_str())
        .ok_or_else(|| {
            invalid_params("expected {\"action\": \"pause\" | \"resume\" | \"step\"}")
        })?;

    match action {
        "pause" => world.resource_mut::<Time<Virtual>>().pause(),
        "resume" => {
            world.resource_mut::<Time<Virtual>>().unpause();
            let mut state = world.resource_mut::<PlaybackStepState>();
            state.pending = 0;
            state.stepping = false;
        }
        "step" => {
            if !world.resource::<Time<Virtual>>().is_paused() {
                return Err(invalid_params("step requires the sim to be paused"));
            }
            world.resource_mut::<PlaybackStepState>().pending += 1;
        }
        other => return Err(invalid_params(&format!("unknown action \"{other}\""))),
    }

    Ok(json!({ "paused": world.resource::<Time<Virtual>>().is_paused() }))
}

fn invalid_params(message: &str) -> BrpError {
    BrpError {
        code: error_codes::INVALID_PARAMS,
        message: message.to_string(),
        data: None,
    }
}

/// Runs before the clock updates each frame: re-pauses after a step frame,
/// then consumes one pending step by unpausing for this frame.
pub fn playback_step_system(mut state: ResMut<PlaybackStepState>, mut time: ResMut<Time<Virtual>>) {
    if state.stepping {
        time.pause();
        state.stepping = false;
    }
    if state.pending > 0 && time.is_paused() {
        time.unpause();
        state.pending -= 1;
        state.stepping = true;
    }
}

/// Test-only plugin wiring the step state and system exactly like
/// `JackdawRemotePlugin` does, without BRP or HTTP.
pub struct PlaybackTestPlugin;

impl Plugin for PlaybackTestPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PlaybackStepState>();
        app.add_systems(First, playback_step_system.before(bevy::time::TimeSystems));
    }
}
