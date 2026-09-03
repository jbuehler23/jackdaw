//! `JACKDAW_RENDER_DIAGNOSTICS`: per-pass GPU timings, on request.
//!
//! The FPS overlay answers "is it slow"; it cannot answer "slow where".
//! A frame that takes 140ms because the terrain's fragment shader is
//! expensive and one that takes 140ms because an editor system walks
//! every entity look identical from the readout, and the fix for each is
//! the opposite of the fix for the other.
//!
//! Setting the variable adds Bevy's [`RenderDiagnosticsPlugin`], which
//! wraps every render-graph pass in a GPU timestamp span (shadows, main
//! opaque, transparent, UI, ...), alongside the system-information and
//! frame-time diagnostics, and logs the lot on a five-second timer. The
//! difference between the frame time and the sum of the passes is the
//! CPU-side cost.
//!
//! ```text
//! JACKDAW_RENDER_DIAGNOSTICS=1 jackdaw
//! ```
//!
//! Off unless asked for: the timestamp queries cost a little GPU time
//! themselves, and reading them back stalls the frame slightly, so a
//! measurement run should be the only thing paying for them.
//!
//! [`wgpu_settings`] is the other half. Timestamps need
//! `TIMESTAMP_QUERY` on the device, which is requested at device
//! creation -- long before this plugin builds -- so `main` asks
//! [`wgpu_settings`] what to hand [`RenderPlugin`](bevy::render::RenderPlugin)
//! and gets `None` on an ordinary launch.

use core::time::Duration;

use bevy::diagnostic::{LogDiagnosticsPlugin, SystemInformationDiagnosticsPlugin};
use bevy::prelude::*;
use bevy::render::diagnostic::RenderDiagnosticsPlugin;
use bevy::render::settings::{WgpuFeatures, WgpuSettings};

/// Turns the per-pass timings on when set to anything but `0`.
pub const ENV_RENDER_DIAGNOSTICS: &str = "JACKDAW_RENDER_DIAGNOSTICS";

/// How long a reported average covers.
const REPORT_INTERVAL: Duration = Duration::from_secs(5);

/// Whether this process was asked for render diagnostics.
pub fn requested() -> bool {
    enabled(
        std::env::var_os(ENV_RENDER_DIAGNOSTICS)
            .and_then(|value| value.into_string().ok())
            .as_deref(),
    )
}

/// Whether a [`ENV_RENDER_DIAGNOSTICS`] value asks for the timings.
///
/// A shell that exports the variable permanently still has to be able to
/// launch an ordinary editor, so `0`, `false` and an empty value are
/// off.
fn enabled(raw: Option<&str>) -> bool {
    raw.is_some_and(|value| !matches!(value.trim(), "" | "0" | "false"))
}

/// The wgpu settings a diagnostics run needs, or `None` for an ordinary
/// launch.
///
/// The GPU timestamps the pass spans are built from are a device
/// feature, and a device's features are fixed when it is created. Asking
/// for them unconditionally would fail on a backend that lacks them
/// (Metal, WebGPU, WebGL2), so the request is tied to the same variable
/// that adds the plugin.
pub fn wgpu_settings() -> Option<WgpuSettings> {
    requested().then(|| WgpuSettings {
        features: WgpuFeatures::TIMESTAMP_QUERY
            | WgpuFeatures::TIMESTAMP_QUERY_INSIDE_ENCODERS
            | WgpuFeatures::TIMESTAMP_QUERY_INSIDE_PASSES,
        ..default()
    })
}

pub(crate) fn plugin(app: &mut App) {
    if !requested() {
        return;
    }
    // `FrameTimeDiagnosticsPlugin` is already in from `fps_overlay`, and
    // adding it twice is a panic.
    app.add_plugins((
        RenderDiagnosticsPlugin,
        SystemInformationDiagnosticsPlugin,
        LogDiagnosticsPlugin {
            debug: false,
            wait_duration: REPORT_INTERVAL,
            filter: None,
        },
    ));
    info!("{ENV_RENDER_DIAGNOSTICS}: logging per-pass GPU timings every {REPORT_INTERVAL:?}");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An ordinary launch pays nothing: the plugin is not added and no
    /// timestamp feature is requested, so the device is created exactly
    /// as it always was.
    #[test]
    fn an_unset_environment_is_off() {
        assert!(!enabled(None));
    }

    /// A shell that exports the variable permanently can still launch a
    /// normal editor.
    #[test]
    fn zero_false_and_empty_are_off() {
        assert!(!enabled(Some("0")));
        assert!(!enabled(Some("false")));
        assert!(!enabled(Some("  ")));
    }

    /// Anything else asks for the timings; `=1` is what the docs say.
    #[test]
    fn anything_else_is_on() {
        assert!(enabled(Some("1")));
        assert!(enabled(Some("true")));
    }
}
