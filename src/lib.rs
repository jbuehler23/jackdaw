//! The official Jackdaw editor. The implementation lives in
//! `jackdaw_editor_core` and the feature crates beside it; this crate
//! re-exports the core so `jackdaw::...` paths keep resolving, and adds the
//! feature crates to the editor.
pub use jackdaw_editor_animation;
pub use jackdaw_editor_animation as animation;
pub use jackdaw_editor_core::*;
pub use jackdaw_editor_modelling;
pub use jackdaw_editor_remote;
pub use jackdaw_editor_remote as remote;
pub use jackdaw_editor_terrain;
pub use jackdaw_editor_tile;

use bevy::app::{App, AppExit, PluginGroup, PluginGroupBuilder};

/// The editor plugin group: the core's [`jackdaw_editor_core::JackdawEditorPlugins`]
/// and the feature crates.
#[derive(Default)]
pub struct JackdawEditorPlugins {
    core: jackdaw_editor_core::JackdawEditorPlugins,
}

impl PluginGroup for JackdawEditorPlugins {
    fn build(self) -> PluginGroupBuilder {
        self.core
            .build()
            .add(jackdaw_editor_tile::TileEditorPlugin)
            .add(jackdaw_editor_terrain::TerrainEditorPlugin)
            .add(jackdaw_editor_modelling::ModellingEditorPlugin)
            .add(jackdaw_editor_remote::RemoteEditorPlugin)
            .add(jackdaw_editor_animation::AnimationEditorPlugin)
    }
}

/// Adds the feature crates to an app that already has the core's plugins.
pub fn add_feature_plugins(app: &mut App) {
    app.add_plugins((
        jackdaw_editor_tile::TileEditorPlugin,
        jackdaw_editor_terrain::TerrainEditorPlugin,
        jackdaw_editor_modelling::ModellingEditorPlugin,
        jackdaw_editor_remote::RemoteEditorPlugin,
        jackdaw_editor_animation::AnimationEditorPlugin,
    ));
}

/// The core's launch entry points, with the editor's remote-control server
/// added to the process. The server is not in [`JackdawEditorPlugins`]: a
/// headless test app builds the same group, and two of them would fight over
/// the port. It refuses to start when the project turns `remote.enabled` off.
pub mod launch {
    pub use jackdaw_editor_core::launch::*;

    use super::{App, AppExit};

    /// Run the editor as the `jackdaw` binary does, with every feature crate
    /// and its subcommands, calling `extend` on the app before it runs.
    pub fn run(extend: impl FnOnce(&mut App)) -> AppExit {
        run_with(
            LaunchCommands {
                export_terrain: Some(jackdaw_editor_terrain::export::run_export_terrain_cli),
            },
            extend,
        )
    }

    /// [`run`], with the given subcommands.
    pub fn run_with(commands: LaunchCommands, extend: impl FnOnce(&mut App)) -> AppExit {
        jackdaw_editor_core::launch::run_with(commands, |app| {
            super::add_feature_plugins(app);
            app.add_plugins(jackdaw_editor_remote::server::JackdawEditorRemotePlugin::default());
            extend(app);
        })
    }
}

pub mod prelude {
    pub use super::JackdawEditorPlugins;
    pub use jackdaw_editor_core::prelude::*;
}
