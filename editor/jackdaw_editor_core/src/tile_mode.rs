//! Tile edit state the core reads without the tile editor: whether tile edit
//! mode is on and with which tool, the status line's hover readout, and the
//! names other areas refer to the tile editor's windows and files by.

use bevy::prelude::*;

/// Catalog name of the tile editor extension.
pub const TILE_EDITOR_EXTENSION_ID: &str = "jackdaw.tiles";

pub const TILE_PALETTE_WINDOW_ID: &str = "jackdaw.tile_palette";

/// The file name ending of a tileset document.
pub const TILESET_SUFFIX: &str = ".tileset.bsn";

/// The operator that imports an image as a tileset.
pub const TILESET_IMPORT_OP: &str = "tileset.import";

/// The operator that adds a tilemap to the 2D scene.
pub const TILEMAP_CREATE_OP: &str = "tilemap.create";

/// The tool a press on the map uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TileTool {
    #[default]
    Brush,
    Eraser,
    Rectangle,
    Line,
    Bucket,
    Picker,
    Select,
}

impl TileTool {
    pub fn label(self) -> &'static str {
        match self {
            Self::Brush => "Brush",
            Self::Eraser => "Eraser",
            Self::Rectangle => "Rectangle",
            Self::Line => "Line",
            Self::Bucket => "Bucket",
            Self::Picker => "Picker",
            Self::Select => "Select",
        }
    }

    /// Whether the tool writes one shape between the press and release cells.
    pub fn is_shape(self) -> bool {
        matches!(self, Self::Rectangle | Self::Line)
    }
}

/// Tile edit mode: on while it names a map.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TileEditMode {
    pub map: Option<Entity>,
    /// The index of the layer strokes paint.
    pub layer: usize,
    pub tool: TileTool,
}

/// The hovered cell as the status bar reads it out.
#[derive(Resource, Default, Clone, PartialEq, Eq)]
pub struct TileHoverReadout(pub Option<String>);

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<TileEditMode>()
        .init_resource::<TileHoverReadout>();
}
