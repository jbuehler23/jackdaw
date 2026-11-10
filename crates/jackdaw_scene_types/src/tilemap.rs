use bevy::prelude::*;

/// A grid of tiles drawn in ordered layers.
///
/// Only the shape and the layer list are scene text. The cells live in a
/// binary `.jdtiles` sidecar named by [`Tilemap::data_path`], keyed by each
/// layer's [`TilemapLayer::id`]; the order of [`Tilemap::layers`] is the
/// draw order.
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
    /// Bottom to top.
    pub layers: Vec<TilemapLayer>,
    /// Authored extent in cells, min inclusive and max exclusive. Without
    /// it the map reaches as far as its painted cells.
    pub bounds: Option<IRect>,
    /// The id the next new layer gets. Ids are never given twice, so a
    /// removed layer's id stays unused.
    pub next_layer_id: u32,
}

/// A persisted contract: BSN elides fields equal to these on write.
impl Default for Tilemap {
    fn default() -> Self {
        Self {
            cell_size: UVec2::splat(16),
            data_path: String::new(),
            layers: Vec::new(),
            bounds: None,
            next_layer_id: 0,
        }
    }
}

impl Tilemap {
    /// The position in [`Self::layers`] of the layer with id `id`.
    pub fn layer_index(&self, id: u32) -> Option<usize> {
        self.layers.iter().position(|layer| layer.id == id)
    }

    /// A layer id no layer of the map holds or held, advancing the counter.
    /// Ids past the counter, as a hand-edited map may hold, are skipped too.
    pub fn mint_layer_id(&mut self) -> u32 {
        let past_held = (self.layers.iter()).map(|layer| layer.id.saturating_add(1));
        let id = past_held.fold(self.next_layer_id, u32::max);
        self.next_layer_id = id.saturating_add(1);
        id
    }

    /// Gives a fresh id to every layer whose id an earlier layer holds.
    /// Returns whether any layer changed.
    pub fn dedup_layer_ids(&mut self) -> bool {
        let mut seen = std::collections::HashSet::new();
        let repeated: Vec<usize> = (self.layers.iter().enumerate())
            .filter(|(_, layer)| !seen.insert(layer.id))
            .map(|(index, _)| index)
            .collect();
        for &index in &repeated {
            self.layers[index].id = self.mint_layer_id();
        }
        !repeated.is_empty()
    }
}

#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Default)]
pub struct TilemapLayer {
    /// Unique in the map and never reused; the sidecar keys the layer's
    /// cells by it.
    pub id: u32,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(id: u32) -> TilemapLayer {
        TilemapLayer {
            id,
            ..Default::default()
        }
    }

    #[test]
    fn minted_layer_ids_are_never_reused_after_a_removal() {
        let mut map = Tilemap::default();
        let first = map.mint_layer_id();
        let second = map.mint_layer_id();
        map.layers = vec![layer(first), layer(second)];
        map.layers.pop();
        assert_eq!(map.mint_layer_id(), second + 1);
    }

    #[test]
    fn minted_layer_ids_skip_ids_past_the_counter() {
        let mut map = Tilemap {
            layers: vec![layer(7)],
            ..Default::default()
        };
        assert_eq!(map.mint_layer_id(), 8);
        assert_eq!(map.next_layer_id, 9);
    }

    #[test]
    fn repeated_layer_ids_get_fresh_ones_and_the_first_keeps_its_own() {
        let mut map = Tilemap {
            layers: vec![layer(0), layer(0), layer(3), layer(3)],
            ..Default::default()
        };
        assert!(map.dedup_layer_ids());
        let ids: Vec<u32> = map.layers.iter().map(|l| l.id).collect();
        assert_eq!(ids, [0, 4, 3, 5]);
        assert_eq!(map.layer_index(5), Some(3));
        assert!(!map.dedup_layer_ids());
    }
}
