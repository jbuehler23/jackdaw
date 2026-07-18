//! Shared build state for the status bar.
//!
//! [`BuildStatus`] is the single source of truth for a background
//! project build: the status bar renders progress while
//! [`BuildState::Building`] is active.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use bevy::prelude::*;

use crate::ext_build::BuildProgress;

/// Single source of truth for whether a background project build
/// is in flight, succeeded, or failed. Read by the status bar.
#[derive(Resource, Default)]
pub struct BuildStatus {
    pub state: BuildState,
}

/// The states the status bar's right region renders. The `Idle`
/// variant lets the existing gizmo / edit-mode rendering fall
/// through unchanged.
#[derive(Default)]
pub enum BuildState {
    #[default]
    Idle,
    /// Cargo is running. `progress` is the same `Arc<Mutex<...>>`
    /// the cargo reader threads write into; the status bar reads
    /// `current_crate` + `artifacts_done`/`total` to render the
    /// "Compiling X (12/47)" string.
    Building {
        project: PathBuf,
        started: Instant,
        progress: Arc<Mutex<BuildProgress>>,
    },
    /// The last build finished and the editor loaded its type schema.
    /// Shown briefly, then reverts to `Idle`. `components` is how many
    /// project component types the editor now knows.
    Ready { at: Instant, components: usize },
    /// The last build failed. Shown until it reverts to `Idle`; the
    /// status bar makes it clickable to surface the build log.
    Failed { at: Instant },
}
