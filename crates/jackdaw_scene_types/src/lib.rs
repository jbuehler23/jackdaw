//! Editor display metadata as Bevy reflect custom attributes.
//! The picker reads them via
//! `type_info.custom_attributes().get::<T>()` and falls back to
//! the type's reflected doc comment (workspace bevy
//! `reflect_documentation` feature).
//!
//! ```ignore
//! use bevy::prelude::*;
//! use jackdaw_scene_types::EditorCategory;
//!
//! /// Spawns the player entity.
//! #[derive(Component, Reflect, Default)]
//! #[reflect(Component, Default, @EditorCategory("Actor"))]
//! pub struct PlayerSpawn;
//! ```
//!
//! `jackdaw_runtime` and `jackdaw` re-export both newtypes
//! through their preludes.

pub mod brush_chunks;
#[cfg(feature = "render")]
pub mod mesh_rebuild;
pub mod node_id;
pub mod types;

pub use brush_chunks::{MeshChunk, build_brush_chunks};
#[cfg(feature = "render")]
pub use mesh_rebuild::evaluate_brush_geometry;
pub use node_id::{SCENE_NODE_ID_TYPE_PATH, SPARSE_MIN, SceneNodeId};
pub use types::{
    Brush, BrushFaceData, BrushGroup, BrushPlane, BrushTopology, CustomProperties, DerivedFaceMesh,
    GltfSource, NavmeshRegion, PrefabBaseline, PropertyValue, SceneRootTag, Terrain,
    TerrainChannel, TerrainChannelElement, TerrainPaletteEntry, TerrainQuantization,
};

use bevy::prelude::*;
use std::borrow::Cow;

/// Registers every scene component type for reflection. With the `render`
/// feature this also covers the asset reflection a headless app needs to
/// deserialize material handles, plus the optional brush mesh rebuild.
pub struct SceneTypesPlugin {
    /// Whether to run the built-in runtime mesh rebuild for brushes.
    /// Defaults to `true`. Set to `false` if your app has its own mesh rebuild
    /// (e.g. the editor's per-face material palette system).
    pub runtime_mesh_rebuild: bool,
}

impl Default for SceneTypesPlugin {
    fn default() -> Self {
        Self {
            runtime_mesh_rebuild: true,
        }
    }
}

impl Plugin for SceneTypesPlugin {
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
            .register_type::<SceneNodeId>()
            .register_type::<GltfSource>()
            .register_type::<NavmeshRegion>()
            .register_type::<Terrain>()
            .register_type::<TerrainChannel>()
            .register_type::<TerrainChannelElement>()
            .register_type::<TerrainPaletteEntry>()
            .register_type::<TerrainQuantization>()
            .register_type::<MeshMirror>()
            .register_type::<ModifierStack>()
            .register_type::<ModifierEntry>()
            .register_type::<Modifier>();

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

/// Picker grouping for a component. Attach via
/// `#[reflect(@EditorCategory("Your Group"))]`.
#[derive(Reflect, Clone, Debug, PartialEq, Eq)]
pub struct EditorCategory(pub Cow<'static, str>);

impl EditorCategory {
    pub const fn new(name: &'static str) -> Self {
        EditorCategory(Cow::Borrowed(name))
    }
}

impl From<&'static str> for EditorCategory {
    fn from(value: &'static str) -> Self {
        EditorCategory(Cow::Borrowed(value))
    }
}

impl From<String> for EditorCategory {
    fn from(value: String) -> Self {
        EditorCategory(Cow::Owned(value))
    }
}

/// Picker tooltip override. Falls back to the reflected doc
/// comment when absent.
#[derive(Reflect, Clone, Debug, PartialEq, Eq)]
pub struct EditorDescription(pub Cow<'static, str>);

impl EditorDescription {
    pub const fn new(text: &'static str) -> Self {
        EditorDescription(Cow::Borrowed(text))
    }
}

impl From<&'static str> for EditorDescription {
    fn from(value: &'static str) -> Self {
        EditorDescription(Cow::Borrowed(value))
    }
}

impl From<String> for EditorDescription {
    fn from(value: String) -> Self {
        EditorDescription(Cow::Owned(value))
    }
}

/// Hides things from editor-facing surfaces. Used in two ways:
///
/// - As a Bevy `Component` on an entity: hides that entity from
///   the hierarchy panel.
/// - As a `#[reflect(@EditorHidden)]` attribute on a Component
///   type: hides the type from the Add Component picker. Used by
///   jackdaw's own scene types (brushes, navmesh, terrain, node
///   graph, animation graph) and available to extension and game
///   crates with helper Components.
///
/// ```ignore
/// #[derive(Component, Reflect, Default)]
/// #[reflect(Component, Default, @EditorHidden)]
/// pub struct InternalRig;
/// ```
#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component, Default)]
pub struct EditorHidden;

/// Marker for entities that exist as editor-time visual
/// indicators. The save filter skips this entity (and its
/// subtree) so the helper never lands in `.jsn`; the editor
/// viewport still renders it.
///
/// Pattern: under your scene-authored marker (e.g. `PlayerSpawn`),
/// spawn a child carrying `SkipSerialization` plus a `Mesh3d` +
/// `MeshMaterial3d`. The editor renders the helper; the saved
/// scene never includes it.
#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component, Default)]
pub struct SkipSerialization;
