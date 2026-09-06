//! `JACKDAW_RENDER_DIAGNOSTICS`: per-pass GPU timings, on request.
//!
//! Setting the variable adds Bevy's `RenderDiagnosticsPlugin`, which wraps
//! every render-graph pass in a GPU timestamp span, alongside the
//! system-information and frame-time diagnostics, and logs them on a
//! five-second timer. The difference between the frame time and the sum of the
//! passes is the CPU-side cost.
//!
//! ```text
//! JACKDAW_RENDER_DIAGNOSTICS=1 jackdaw
//! ```
//!
//! Off unless asked for: the timestamp queries cost GPU time and reading them
//! back stalls the frame. [`wgpu_settings`] is the other half -- timestamps are
//! a device feature requested at device creation, long before this plugin
//! builds.

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

/// Whether an `ENV_RENDER_DIAGNOSTICS` value asks for the timings. `0`, `false`
/// and an empty value are off, so a permanently exported variable still
/// launches an ordinary editor.
fn enabled(raw: Option<&str>) -> bool {
    raw.is_some_and(|value| !matches!(value.trim(), "" | "0" | "false"))
}

/// The wgpu settings a diagnostics run needs, or `None` for an ordinary launch.
/// Requesting the timestamp features unconditionally would fail on a backend
/// that lacks them.
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

    #[test]
    fn an_unset_environment_is_off() {
        assert!(!enabled(None));
    }

    #[test]
    fn zero_false_and_empty_are_off() {
        assert!(!enabled(Some("0")));
        assert!(!enabled(Some("false")));
        assert!(!enabled(Some("  ")));
    }

    #[test]
    fn anything_else_is_on() {
        assert!(enabled(Some("1")));
        assert!(enabled(Some("true")));
    }
}
