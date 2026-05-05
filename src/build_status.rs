//! Static-editor build state machine.
//!
//! For **static** game projects the launcher's editor (built into
//! the `jackdaw` binary itself) doesn't carry the user's
//! `MyGamePlugin` types, so the inspector's Add Component picker
//! and PIE Play wouldn't see them. The user runs a separate
//! `<project>/target/debug/editor` binary that statically links
//! their plugin alongside jackdaw's editor stack. The launcher's
//! job for static projects is therefore: build that editor
//! binary, then hand off to it.
//!
//! [`BuildStatus`] tracks where in that lifecycle we are. The
//! launcher's modal (the same one used by the dylib install path)
//! displays progress while [`BuildState::Building`] is active;
//! once [`BuildState::Ready`] fires, the driver in
//! `project_select::drive_static_editor_build` closes the modal,
//! spawns the user's editor binary, and exits the launcher.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use bevy::prelude::*;

use crate::ext_build::BuildProgress;

/// Single source of truth for whether a static-editor background
/// build is in flight, succeeded, or failed. Read by the status
/// bar, written by the build driver in `project_select`.
#[derive(Resource, Default)]
pub struct BuildStatus {
    pub state: BuildState,
}

/// The four states the status bar's right region renders. The
/// `Idle` variant lets the existing gizmo / edit-mode rendering
/// fall through unchanged.
#[derive(Default)]
pub enum BuildState {
    #[default]
    Idle,
    /// Cargo is running. `progress` is the same `Arc<Mutex<…>>`
    /// the cargo reader threads write into; the status bar reads
    /// `current_crate` + `artifacts_done`/`total` to render the
    /// "Compiling X (12/47)" string.
    Building {
        project: PathBuf,
        started: Instant,
        progress: Arc<Mutex<BuildProgress>>,
    },
    /// Build finished successfully. `bin` points at the static
    /// editor binary on disk. The driver fires the handoff
    /// automatically iff `auto_reload` is `true`; otherwise the
    /// user reloads by clicking the footer.
    Ready {
        project: PathBuf,
        bin: PathBuf,
        auto_reload: bool,
    },
    /// Build failed. `log_tail` is the last few lines of cargo's
    /// stderr, surfaced via the click handler so the user can
    /// figure out what to fix.
    Failed { project: PathBuf, log_tail: String },
}
