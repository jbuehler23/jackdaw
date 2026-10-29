use bevy::prelude::*;

/// A grid of tiles drawn in ordered layers.
///
/// Only the shape and the layer list are scene text. The cells live in a
/// binary `.jdtiles` sidecar named by [`Tilemap::data_path`], whose layers
/// follow [`Tilemap::layers`] in order.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, @crate::EditorCategory::new("2D"))]
#[cfg_attr(feature = "render", require(Transform, Visibility))]
#[cfg_attr(not(feature = "render"), require(Transform))]
pub struct Tilemap {
    /// Cell size in world units.
    pub cell_size: UVec2,
    /// Scene-relative path of the cell sidecar. The editor keeps the decoded
    /// cells in a resource keyed by it, since a tab swap round-trips the
    /// scene through text.
    pub data_path: String,
    pub layers: Vec<TilemapLayer>,
    /// Authored extent in cells, min inclusive and max exclusive. Without
    /// it the map reaches as far as its painted cells.
    pub bounds: Option<IRect>,
}

/// A persisted contract: BSN elides fields equal to these on write.
impl Default for Tilemap {
    fn default() -> Self {
        Self {
            cell_size: UVec2::splat(16),
            data_path: String::new(),
            layers: Vec::new(),
            bounds: None,
        }
    }
}

#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Default)]
pub struct TilemapLayer {
    pub name: String,
    /// Asset path of the layer's tileset.
    pub tileset: String,
    pub kind: TilemapLayerKind,
    /// The match mode a terrain layer resolves under.
    pub terrain_match: LayerTerrainMatch,
}

#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Default)]
pub enum TilemapLayerKind {
    /// Cells hold terrains; tiles come from the solver.
    #[default]
    Terrain,
    /// Cells hold tiles directly.
    Tile,
}

#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[reflect(Default)]
pub enum LayerTerrainMatch {
    /// The tileset's only match mode.
    #[default]
    Auto,
    Corners,
    Sides,
    CornersAndSides,
}
