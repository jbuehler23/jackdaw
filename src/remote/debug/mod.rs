//! Native remote-game debugger: read-only introspection panels that inspect a
//! running Bevy game over BRP, built on the existing `super` connection client.
//!
//! `RemoteDebugPlugin` wires the shared pieces (the sparkline material and the
//! poll helper) plus the Diagnostics view. Later views hang off the same plugin.

pub mod diagnostics;
pub mod poll;
pub mod queries;
pub mod sparkline;

use bevy::asset::embedded_asset;
use bevy::prelude::*;

/// Registers the debugger's shared rendering and the Diagnostics view.
pub struct RemoteDebugPlugin;

impl Plugin for RemoteDebugPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "shaders/sparkline.wgsl");

        app.add_plugins(UiMaterialPlugin::<sparkline::SparklineMaterial>::default());
        app.add_plugins(poll::BrpPollPlugin::<diagnostics::DiagnosticsSample>::new(
            "jackdaw/diagnostics",
            0.25,
        ));

        app.init_resource::<diagnostics::DiagBuffers>();
        app.add_systems(
            Update,
            diagnostics::update_diagnostics_panel.run_if(diagnostics::connected),
        );
    }
}
