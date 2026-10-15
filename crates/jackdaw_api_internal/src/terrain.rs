//! Terrain as an extension sees it: the data the editor holds for each
//! terrain, and tools of its own in the terrain tool palette.
//!
//! [`TerrainDataStore`] holds the open scene's terrain heights, materials,
//! paint and scatter, keyed by `Terrain::data_path`. Read it; change terrain
//! through the editor's `terrain.*` operators so the change reaches undo and
//! the terrain's sidecar file.
//!
//! A [`TerrainTool`] adds a button to the palette that calls the tool's
//! operator, which picks the tool by setting [`TerrainEditMode::Extension`].
//! While it is picked, the options bar above the viewport holds a
//! [`TerrainToolOptions`] node for the extension to fill with the tool's
//! fields.

use bevy::prelude::*;

pub use jackdaw_terrain::store::TerrainDataStore;

/// The terrain tool in use. `None` is no tool: clicks in the viewport select
/// and the gizmo moves things.
#[derive(Resource, Default, PartialEq, Eq, Clone, Debug)]
pub enum TerrainEditMode {
    #[default]
    None,
    /// One of the sculpt brushes.
    Sculpt(jackdaw_terrain::SculptTool),
    /// Stamping palette values into the active paint channel.
    Paint,
    /// The grid-quantization options. Claims no viewport input.
    Quantize,
    /// Choosing which region is active and how the rest of them draw. A
    /// click picks a region; the terrain is not edited.
    Regions,
    /// A tool an extension added, named by the operator that picks it.
    /// Claims no viewport input.
    Extension(&'static str),
}

impl TerrainEditMode {
    /// Whether the brush (sculpt or paint) is the active tool and owns viewport
    /// input: entity click-select, gizmo drag, and Shift+Scroll grid resize all
    /// defer to it.
    pub fn brush_active(&self) -> bool {
        matches!(self, Self::Sculpt(_) | Self::Paint)
    }

    /// Pick `mode`, or put it away when it is already the one in use.
    pub fn toggle(&mut self, mode: Self) {
        *self = if *self == mode { Self::None } else { mode };
    }
}

/// A tool an extension adds to the terrain tool palette.
///
/// Spawned by [`crate::ExtensionContext::register_terrain_tool`] as a child
/// of the extension, so the button leaves the palette when the extension
/// unloads. The button is highlighted while [`TerrainEditMode`] is
/// `Extension(operator_id)`.
#[derive(Component, Clone, Debug)]
pub struct TerrainTool {
    /// The operator the button calls.
    pub operator_id: &'static str,
    /// The glyph the button shows.
    pub icon: lucide_icons::Icon,
}

/// The node on the options bar an extension's terrain tool fills while it is
/// in use.
///
/// The editor spawns it, empty, when the tool is picked, and despawns it with
/// everything under it when the tool is put away or the selection leaves the
/// terrain. Observe `On<Add, TerrainToolOptions>` to fill it.
#[derive(Component, Clone, Debug)]
pub struct TerrainToolOptions {
    /// The operator id of the tool the node belongs to.
    pub tool: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brush_active_covers_both_sculpt_and_paint() {
        assert!(TerrainEditMode::Sculpt(jackdaw_terrain::SculptTool::Raise).brush_active());
        assert!(TerrainEditMode::Paint.brush_active());
    }

    #[test]
    fn brush_active_excludes_tools_without_a_brush() {
        assert!(!TerrainEditMode::None.brush_active());
        assert!(!TerrainEditMode::Quantize.brush_active());
        assert!(!TerrainEditMode::Extension("example.tool").brush_active());
    }

    #[test]
    fn toggling_the_tool_in_use_puts_it_away() {
        let mut mode = TerrainEditMode::Quantize;
        mode.toggle(TerrainEditMode::Extension("example.tool"));
        assert_eq!(mode, TerrainEditMode::Extension("example.tool"));
        mode.toggle(TerrainEditMode::Extension("example.tool"));
        assert_eq!(mode, TerrainEditMode::None);
    }
}
