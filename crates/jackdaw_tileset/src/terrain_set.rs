use std::fmt;

use jackdaw_tilemap::{
    MatchMode, Placement, TerrainId, TerrainSet, TerrainTile, TileRef, TileTerrains, solver_takes,
};

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
    /// their own. A tile joins by the transition it was laid out for when its
    /// peers still fit it and a joined set holds both its terrains. Otherwise
    /// it joins when its peers lie within one joined set's terrains and,
    /// unless it is a pure tile, within no other mode's set, since its peers
    /// alone cannot say which of those it was made for.
    /// Terrains stack by their place in their set, bottom first; terrains of
    /// different sets level with each other stack background first, then by
    /// id.
    pub fn terrain_set(&self, mode: Option<TerrainMatch>) -> Result<TerrainSet, TerrainSetError> {
        let mode = match mode {
            Some(mode) => mode,
            None => self.only_mode()?,
        };
        let all = self.sets();
        let sets: Vec<_> = all.iter().filter(|set| set.mode == mode).collect();
        if sets.is_empty() {
            return Err(TerrainSetError::NoSetWithMode(mode));
        }
        let background = |id: TerrainId| {
            (self.terrains.iter()).any(|terrain| terrain.id == id && terrain.background)
        };
        let mut order: Vec<(usize, bool, TerrainId)> = Vec::new();
        for set in &sets {
            for (place, &id) in set.terrains.iter().enumerate() {
                match order.iter_mut().find(|(.., t)| *t == id) {
                    Some(found) => found.0 = found.0.min(place),
                    None => order.push((place, !background(id), id)),
                }
            }
        }
        order.sort_unstable();

        let mut set =
            TerrainSet::new(match_mode(mode)).with_order(order.into_iter().map(|(.., id)| id));
        let sets: Vec<(&[TerrainId], MatchMode)> = (all.iter())
            .map(|s| (s.terrains.as_slice(), match_mode(s.mode)))
            .collect();
        for tile in &self.tiles {
            let recorded = (tile.set_inner != 0).then_some((tile.set_inner, tile.set_outer));
            let joins = solver_takes(match_mode(mode), &tile.peers, recorded, &sets);
            let layer = self.layer(tile.sheet, tile.tile);
            let index = layer.and_then(|layer| u16::try_from(layer).ok());
            if let Some(index) = index.filter(|i| joins && *i != TileRef::EMPTY_INDEX) {
                let terrains = TileTerrains { peers: tile.peers };
                set = set.with_tile(TerrainTile::new(TileRef::new(index), terrains, 1));
            }
        }
        Ok(set)
    }

    /// Where a terrain layer solving under `mode`, or the tileset's only
    /// mode when `None`, draws its tiles; `None` when there is no such mode.
    pub fn placement(&self, mode: Option<TerrainMatch>) -> Option<Placement> {
        let mode = match mode {
            Some(mode) => mode,
            None => self.only_mode().ok()?,
        };
        Some(match_mode(mode).placement())
    }

    fn only_mode(&self) -> Result<TerrainMatch, TerrainSetError> {
        let mut modes: Vec<TerrainMatch> = Vec::new();
        for set in self.sets().iter() {
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
