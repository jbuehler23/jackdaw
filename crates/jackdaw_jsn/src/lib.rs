pub mod ast;
mod brush_chunks;
pub mod editor_meta;
pub mod format;
mod loader;
#[cfg(feature = "render")]
pub mod mesh_rebuild;
pub mod types;

use bevy::prelude::*;

// Re-export core types for consumer convenience
pub use editor_meta::{EditorCategory, EditorDescription, EditorHidden, SkipSerialization};
pub use types::{
    Brush, BrushFaceData, BrushGroup, BrushPlane, BrushTopology, CustomProperties, DerivedFaceMesh,
    GltfSource, JsnPrefab, JsnPrefabBaseline, NavmeshRegion, PropertyValue, SceneRootTag, Terrain,
};

// Re-export geometry crate
pub use jackdaw_geometry;

pub use ast::{JsnNodeId, SceneJsnAst, needs_id_migration};
pub use brush_chunks::{MeshChunk, build_brush_chunks};
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
        use jackdaw_geometry::{
            AttributeData, AttributeStack, MeshEdge, MeshLoop, MeshMirror, MeshPoly, MeshVert,
            Modifier, ModifierEntry, ModifierStack,
        };
        app.register_type::<Brush>()
            .register_type::<BrushGroup>()
            .register_type::<SceneRootTag>()
            .register_type::<BrushFaceData>()
            .register_type::<BrushPlane>()
            .register_type::<BrushTopology>()
            .register_type::<MeshVert>()
            .register_type::<MeshEdge>()
            .register_type::<MeshPoly>()
            .register_type::<MeshLoop>()
            .register_type::<AttributeStack>()
            .register_type::<AttributeData>()
            .register_type::<CustomProperties>()
            .register_type::<PropertyValue>()
            .register_type::<ast::JsnNodeId>()
            .register_type::<GltfSource>()
            .register_type::<JsnPrefab>()
            .register_type::<NavmeshRegion>()
            .register_type::<Terrain>()
            .register_type::<MeshMirror>()
            .register_type::<ModifierStack>()
            .register_type::<ModifierEntry>()
            .register_type::<Modifier>()
            .init_asset_loader::<JsnAssetLoader>();

        #[cfg(feature = "render")]
        {
            // With `render`, `BrushFaceData::material` is a `Handle<StandardMaterial>`.
            // A dedicated server builds with `render` for these types but adds no
            // rendering plugins, so material/image asset reflection is never set up and
            // the deserializer (which keys these handles off `ReflectHandle`) drops any
            // brush with an unassigned `material: null` face. Register it only when the
            // render plugins are absent; when they are present (the editor, a windowed
            // client) they own this, and re-registering corrupts the asset storage.
            if !app
                .world()
                .contains_resource::<bevy::asset::Assets<bevy::pbr::StandardMaterial>>()
            {
                use bevy::asset::AssetApp;
                app.init_asset::<bevy::pbr::StandardMaterial>()
                    .register_asset_reflect::<bevy::pbr::StandardMaterial>()
                    .init_asset::<bevy::image::Image>()
                    .register_asset_reflect::<bevy::image::Image>();
            }

            if self.runtime_mesh_rebuild {
                app.add_plugins(mesh_rebuild::MeshRebuildPlugin);
            }
        }
    }
}
