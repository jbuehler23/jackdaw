//! The official Jackdaw editor. The implementation lives in
//! `jackdaw_editor_core` and the feature crates beside it; this crate
//! re-exports the core so `jackdaw::...` paths keep resolving, and adds the
//! feature crates to the editor.
pub use jackdaw_editor_core::*;
pub use jackdaw_editor_terrain;
pub use jackdaw_editor_tile;

use bevy::app::{App, PluginGroup, PluginGroupBuilder};

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
    }
}

/// Adds the feature crates to an app that already has the core's plugins.
pub fn add_feature_plugins(app: &mut App) {
    app.add_plugins((
        jackdaw_editor_tile::TileEditorPlugin,
        jackdaw_editor_terrain::TerrainEditorPlugin,
    ));
}

pub mod prelude {
    pub use super::JackdawEditorPlugins;
    pub use jackdaw_editor_core::prelude::*;
}
