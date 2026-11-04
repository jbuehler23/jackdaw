//! Launcher state the core reads without the launcher: the project to open
//! once the launcher is reached, the marker on the launcher's body, and the
//! request to open a project that the launcher carries out.

use std::path::PathBuf;

use bevy::prelude::*;

/// Catalog name of the launcher extension, which owns the `launcher.`
/// operators.
pub const LAUNCHER_EXTENSION_ID: &str = "jackdaw.launcher";

/// When set, the project selector will skip UI and auto-open the given project.
#[derive(Resource)]
pub struct PendingAutoOpen {
    pub path: PathBuf,
    /// `true` when we got here via a post-restart auto-open ;
    /// the parent process already built + installed the dylib,
    /// so we skip that step (preventing an infinite
    /// build->restart->auto-open->build loop).
    pub skip_build: bool,
}

/// The launcher's body; while it exists, the launcher's pages are kept up
/// to date.
#[derive(Component, Default, Clone)]
pub struct LauncherShell;

/// Open the project at `root`, building it first if it needs a build.
#[derive(Event, Clone, Debug)]
pub struct EnterProject {
    pub root: PathBuf,
}
