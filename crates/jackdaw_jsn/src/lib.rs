pub mod ast;
pub mod editor_meta;
pub mod format;
mod loader;
pub mod mesh_rebuild;
pub mod types;

use bevy::prelude::*;

// Re-export core types for consumer convenience
pub use editor_meta::{EditorCategory, EditorDescription};
pub use types::{
    Brush, BrushFaceData, BrushGroup, BrushPlane, CustomProperties, GltfSource, JsnPrefab,
    JsnPrefabBaseline, NavmeshRegion, PropertyValue, Terrain,
};

// Re-export geometry crate
pub use jackdaw_geometry;

pub use ast::SceneJsnAst;
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
        app.register_type::<Brush>()
            .register_type::<BrushGroup>()
            .register_type::<BrushFaceData>()
            .register_type::<BrushPlane>()
            .register_type::<CustomProperties>()
            .register_type::<PropertyValue>()
            .register_type::<GltfSource>()
            .register_type::<JsnPrefab>()
            .register_type::<NavmeshRegion>()
            .register_type::<Terrain>()
            .init_asset_loader::<JsnAssetLoader>();
        if self.runtime_mesh_rebuild {
            app.add_plugins(mesh_rebuild::MeshRebuildPlugin);
        }
    }
}
