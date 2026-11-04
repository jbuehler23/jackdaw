//! The launcher for the Jackdaw scene editor: the first window, with its
//! project list, installed editors, extensions and new project pages, the
//! project select flow that opens a project, and project scaffolding. The
//! SDK setup, module picker and migration dialog stay in the editor core.
//!
//! Add [`LauncherEditorPlugin`] after the editor's own plugins.

pub mod launcher;
pub mod new_project;
pub mod project_select;

pub use jackdaw_editor_core::launcher_mode::{
    EnterProject, LAUNCHER_EXTENSION_ID, LauncherShell, PendingAutoOpen,
};

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ExtensionAppExt as _;

/// The launcher's windows and state, its project select flow, and its
/// extension.
pub struct LauncherEditorPlugin;

impl Plugin for LauncherEditorPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            project_select::ProjectSelectPlugin,
            launcher::LauncherPlugin,
        ))
        .register_extension::<LauncherExtension>();
    }
}

/// The launcher's operators.
#[derive(Default)]
pub struct LauncherExtension;

impl JackdawExtension for LauncherExtension {
    fn id(&self) -> String {
        LAUNCHER_EXTENSION_ID.to_string()
    }

    fn label(&self) -> String {
        "Launcher".to_string()
    }

    fn description(&self) -> String {
        "Projects, editor installs, extensions and new projects.".to_string()
    }

    fn kind(&self) -> ExtensionKind {
        ExtensionKind::Builtin
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        launcher::add_to_extension(ctx);
    }
}
