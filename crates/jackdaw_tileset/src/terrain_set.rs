use std::fmt;

use jackdaw_tilemap::{TerrainId, TerrainSet, TerrainTile, TileRef, TileTerrains};

use crate::{TerrainMatch, Tileset, match_mode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerrainSetError {
    /// The tileset has no terrain sets.
    NoTerrainSets,
    /// No terrain set of the tileset uses the mode the layer names.
    NoSetWithMode(TerrainMatch),
    /// The layer names no mode and the tileset's sets use these.
    MixedModes(Vec<TerrainMatch>),
}

impl fmt::Display for TerrainSetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoTerrainSets => write!(f, "the tileset has no terrain sets"),
            Self::NoSetWithMode(mode) => {
                write!(f, "no terrain set of the tileset matches by {mode:?}")
            }
            Self::MixedModes(modes) => write!(
                f,
                "the tileset's terrain sets match by different modes ({modes:?}); \
                 pick one match mode for this layer"
            ),
        }
    }
}

impl std::error::Error for TerrainSetError {}

impl Tileset {
    /// The solver set a terrain layer resolves with.
    ///
    /// A layer solves under one match mode, `mode`, or the tileset's only
    /// mode when `None`. Every terrain set of that mode joins one solver set;
    /// sets of other modes do not resolve on the layer and need a layer of
    /// their own. A tile joins by the set it was laid out for when it records
    /// one. Otherwise it joins when its peers lie within one joined set's
    /// two terrains and, unless it is a pure tile, within no other mode's
    /// set, since its peers alone cannot say which of those it was made for.
    /// Terrains stack background first, then each set's outer below its
    /// inner, then by id.
    pub fn terrain_set(&self, mode: Option<TerrainMatch>) -> Result<TerrainSet, TerrainSetError> {
        let mode = match mode {
            Some(mode) => mode,
            None => self.only_mode()?,
        };
        let sets: Vec<_> = (self.terrain_sets.iter())
            .filter(|set| set.mode == mode)
            .collect();
        if sets.is_empty() {
            return Err(TerrainSetError::NoSetWithMode(mode));
        }
        let mut terrains: Vec<TerrainId> = Vec::new();
        for set in &sets {
            for id in [set.outer, set.inner] {
                if id != 0 && !terrains.contains(&id) {
                    terrains.push(id);
                }
            }
        }
        let mut depth = vec![0usize; terrains.len()];
        let index = |id: TerrainId| terrains.iter().position(|t| *t == id);
        for _ in 0..terrains.len() {
            for set in &sets {
                if let (Some(outer), Some(inner)) = (index(set.outer), index(set.inner)) {
                    depth[inner] = depth[inner].max(depth[outer] + 1).min(terrains.len());
                }
            }
        }
        let background = |id: TerrainId| {
            (self.terrains.iter()).any(|terrain| terrain.id == id && terrain.background)
        };
        let mut order: Vec<(bool, usize, TerrainId)> = (terrains.iter().zip(&depth))
            .map(|(&id, &depth)| (!background(id), depth, id))
            .collect();
        order.sort_unstable();

        let mut set =
            TerrainSet::new(match_mode(mode)).with_order(order.into_iter().map(|(.., id)| id));
        let pairs = |joined: bool| -> Vec<(TerrainId, TerrainId)> {
            (self.terrain_sets.iter())
                .filter(|set| (set.mode == mode) == joined)
                .map(|set| (set.inner, set.outer))
                .collect()
        };
        let (joined, others) = (pairs(true), pairs(false));
        let within = |(a, b): (TerrainId, TerrainId), peers: &[TerrainId; 9]| {
            peers.iter().all(|&p| p == 0 || p == a || p == b) && peers.iter().any(|&p| p != 0)
        };
        for tile in &self.tiles {
            let pure = tile.peers.iter().all(|&p| p == tile.peers[0]);
            let joins = if tile.set_inner != 0 {
                let pair = (tile.set_inner, tile.set_outer);
                joined.contains(&pair) && within(pair, &tile.peers)
            } else {
                joined.iter().any(|p| within(*p, &tile.peers))
                    && (pure || !others.iter().any(|p| within(*p, &tile.peers)))
            };
            let layer = self.layer(tile.sheet, tile.tile);
            let index = layer.and_then(|layer| u16::try_from(layer).ok());
            if let Some(index) = index.filter(|i| joins && *i != TileRef::EMPTY_INDEX) {
                let terrains = TileTerrains { peers: tile.peers };
                set = set.with_tile(TerrainTile::new(TileRef::new(index), terrains, 1));
            }
        }
        Ok(set)
    }

    fn only_mode(&self) -> Result<TerrainMatch, TerrainSetError> {
        let mut modes: Vec<TerrainMatch> = Vec::new();
        for set in &self.terrain_sets {
            if !modes.contains(&set.mode) {
                modes.push(set.mode);
            }
        }
        match modes.as_slice() {
            [] => Err(TerrainSetError::NoTerrainSets),
            [mode] => Ok(*mode),
            _ => Err(TerrainSetError::MixedModes(modes)),
        }
    }
}
