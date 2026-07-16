pub mod ast;
pub mod bsn_bridge;
pub mod format;
mod loader;

use bevy::prelude::*;

pub use ast::{JsnEntityNode, SceneJsnAst, needs_id_migration};
pub use format::{JsnProject, JsnProjectConfig, JsnScene};
pub use loader::JsnAssetLoader;

pub struct JsnPlugin {
    /// Whether to run the built-in runtime mesh rebuild for brushes.
    /// Defaults to `true`. Set to `false` if your app has its own mesh rebuild
    /// (e.g. the editor's per-face material palette system).
    pub runtime_mesh_rebuild: bool,
}

impl Default for JsnPlugin {
    fn default() -> Self {
        Self {
            runtime_mesh_rebuild: true,
        }
    }
}

impl Plugin for JsnPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(jackdaw_scene_types::SceneTypesPlugin {
            runtime_mesh_rebuild: self.runtime_mesh_rebuild,
        });
        app.init_asset_loader::<JsnAssetLoader>();
    }
}
