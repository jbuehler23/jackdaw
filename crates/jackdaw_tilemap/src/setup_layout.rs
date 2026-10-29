use std::collections::BTreeSet;

use glam::UVec2;

use crate::setup::{set_holds, transition};
use crate::{
    LayoutTemplate, MatchMode, Peer, Provenance, SetupError, SetupSet, Stamp, TerrainId,
    TileAddress, TileTerrains, TilesetSetup,
};

/// How much of its full layout a terrain set's tiles cover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Coverage {
    pub covered: usize,
    pub total: usize,
    /// Shapes of the full layout no tile of the set has.
    pub missing: Vec<TileTerrains>,
}

impl TilesetSetup {
    /// Lays `template`'s tiles of `inner` over `outer` by hand on the tiles
    /// of sheet `sheet`, a grid of `grid` columns and rows, from `origin`
    /// row by row in rows of [`LayoutTemplate::columns`], adding the set
    /// when it is missing. Returns the tiles it set.
    pub fn apply_template(
        &mut self,
        template: LayoutTemplate,
        inner: TerrainId,
        outer: TerrainId,
        sheet: usize,
        grid: UVec2,
        origin: UVec2,
    ) -> Result<Vec<TileAddress>, SetupError> {
        let shapes = template.tiles(inner, outer);
        let columns = template.columns();
        let size = UVec2::new(columns, (shapes.len() as u32).div_ceil(columns));
        let tiles = block(sheet, grid, origin, size)?;
        for tile in &tiles {
            self.check_loose(*tile)?;
        }
        if self.set(inner, outer).is_none() {
            self.add_set(inner, outer, template.mode())?;
        }
        let mut set = Vec::with_capacity(shapes.len());
        for (address, peers) in tiles.into_iter().zip(shapes) {
            self.set_peers(address, peers)?;
            set.push(address);
        }
        Ok(set)
    }

    /// Marks the `size` block of sheet `sheet` at `origin` as one object by
    /// hand, in place of every stamp sharing a tile with it.
    pub fn put_block(
        &mut self,
        sheet: usize,
        grid: UVec2,
        origin: UVec2,
        size: UVec2,
    ) -> Result<Stamp, SetupError> {
        let cells = (block(sheet, grid, origin, size)?.into_iter())
            .map(|a| {
                let offset = UVec2::new(a.tile % grid.x, a.tile / grid.x) - origin;
                (offset, a.tile)
            })
            .collect();
        let stamp = Stamp {
            sheet,
            origin,
            size,
            cells,
        };
        self.put_stamp(stamp.clone());
        Ok(stamp)
    }

    /// Removes the stamp with a tile at `cell` of sheet `sheet`; a gap in
    /// its bounds is not its.
    pub fn remove_stamp_at(&mut self, sheet: usize, cell: UVec2) -> Result<Stamp, SetupError> {
        let inside = |s: &Stamp| {
            let offset = (s.sheet == sheet && cell.cmpge(s.origin).all()).then(|| cell - s.origin);
            offset.is_some_and(|o| s.cells.iter().any(|(at, _)| *at == o))
        };
        let at = (self.stamps.iter().position(|s| inside(&s.value)))
            .ok_or(SetupError::NoStamp { sheet, cell })?;
        Ok(self.stamps.remove(at).value)
    }

    /// The shapes of the set's full layout that its tiles have, read under
    /// its mode: corners for `Corners`, sides and centre for `Sides`.
    pub fn coverage(&self, inner: TerrainId, outer: TerrainId) -> Result<Coverage, SetupError> {
        let mode = (self.set(inner, outer))
            .ok_or(SetupError::UnknownSet(inner, outer))?
            .mode
            .value;
        let read: &[Peer] = match mode {
            MatchMode::Corners => &Peer::CORNERS,
            MatchMode::Sides => &[
                Peer::Top,
                Peer::Left,
                Peer::Centre,
                Peer::Right,
                Peer::Bottom,
            ],
            MatchMode::CornersAndSides => &Peer::ALL,
        };
        let key = |t: &TileTerrains| read.iter().map(|p| t.get(*p)).collect::<Vec<_>>();
        let present: BTreeSet<Vec<TerrainId>> = (self.tiles.values())
            .filter(|t| set_holds((inner, outer), &t.terrains, &self.sets))
            .map(|t| key(&t.terrains))
            .collect();
        let full = match mode {
            MatchMode::Corners => LayoutTemplate::Corner16,
            MatchMode::Sides => LayoutTemplate::Side16,
            MatchMode::CornersAndSides => LayoutTemplate::Blob47,
        };
        let universe = full.tiles(inner, outer);
        let missing: Vec<TileTerrains> = (universe.iter())
            .filter(|t| !present.contains(&key(t)))
            .copied()
            .collect();
        Ok(Coverage {
            covered: universe.len() - missing.len(),
            total: universe.len(),
            missing,
        })
    }

    /// Tiles of more than one terrain that are a transition of no set, but
    /// for tiles whose every peer was set by hand.
    pub fn flagged(&self) -> Vec<TileAddress> {
        (self.tiles.iter())
            .filter(|(_, t)| {
                let peers = t.terrains.peers;
                let pure = peers.iter().all(|&p| p == peers[0]);
                let accepted = t.provenance.iter().all(|&p| p == Provenance::User);
                let pair = |s: &SetupSet| (s.inner, s.outer);
                !pure && !accepted && !self.sets.iter().any(|s| transition(pair(s), &t.terrains))
            })
            .map(|(a, _)| *a)
            .collect()
    }
}

/// The tiles of the `size` block at `origin` on a sheet of `grid` columns
/// and rows, row by row.
fn block(
    sheet: usize,
    grid: UVec2,
    origin: UVec2,
    size: UVec2,
) -> Result<Vec<TileAddress>, SetupError> {
    let end = origin.saturating_add(size);
    if size.cmpeq(UVec2::ZERO).any() || end.cmpgt(grid).any() {
        return Err(SetupError::OutsideSheet { origin, size });
    }
    let mut tiles = Vec::with_capacity((size.x * size.y) as usize);
    for y in origin.y..end.y {
        for x in origin.x..end.x {
            let tile = y * grid.x + x;
            tiles.push(TileAddress { sheet, tile });
        }
    }
    Ok(tiles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup_edit::tests::{GRASS, SAND, at, edge, farm};

    const GRID: UVec2 = UVec2::new(8, 8);
    use crate::{SetupTile, TileShape, Tracked};

    #[test]
    fn a_template_lays_its_shapes_row_by_row_and_adds_its_set() {
        let mut setup = farm();
        let water = setup.add_terrain("water", [0, 0, 255], false).expect("id");
        let tiles = (setup.apply_template(
            LayoutTemplate::Corner2x2,
            water,
            GRASS,
            0,
            GRID,
            UVec2::new(6, 3),
        ))
        .expect("fits");
        assert_eq!(tiles, [at(30), at(31), at(38), at(39)]);
        let shapes = LayoutTemplate::Corner2x2.tiles(water, GRASS);
        assert_eq!(setup.tiles[&at(39)].terrains, shapes[3]);
        let set = setup.set(water, GRASS).expect("added");
        assert_eq!(set.mode.value, MatchMode::CornersAndSides);
        assert_eq!(setup.coverage(water, GRASS).expect("set").covered, 4);

        let refused = setup.apply_template(
            LayoutTemplate::Side16,
            water,
            GRASS,
            0,
            GRID,
            UVec2::new(5, 0),
        );
        assert_eq!(
            refused,
            Err(SetupError::OutsideSheet {
                origin: UVec2::new(5, 0),
                size: UVec2::new(4, 4)
            })
        );
        let blob = LayoutTemplate::Blob47;
        assert!(
            setup
                .apply_template(blob, water, GRASS, 0, GRID, UVec2::ZERO)
                .is_ok()
        );
        assert!(
            setup
                .apply_template(blob, water, GRASS, 0, GRID, UVec2::Y * 3)
                .is_err()
        );
    }

    #[test]
    fn a_block_replaces_the_stamps_it_overlaps_and_comes_off_by_any_cell() {
        let mut setup = farm();
        let tree = setup
            .put_block(0, GRID, UVec2::new(2, 1), UVec2::new(2, 3))
            .expect("fits");
        assert_eq!(tree.cells.len(), 6);
        assert_eq!(tree.cells[0], (UVec2::ZERO, 10));
        assert_eq!(tree.cells[5], (UVec2::new(1, 2), 27));
        setup
            .put_block(0, GRID, UVec2::new(3, 3), UVec2::ONE * 2)
            .expect("fits");
        assert_eq!(setup.stamps.len(), 1, "the overlapped block gives way");
        assert!(setup.stamps[0].is_user());
        assert!(
            setup
                .put_block(0, GRID, UVec2::new(7, 0), UVec2::new(2, 1))
                .is_err()
        );

        let removed = setup.remove_stamp_at(0, UVec2::new(4, 4)).expect("covered");
        assert_eq!(removed.origin, UVec2::new(3, 3));
        assert_eq!(
            setup.remove_stamp_at(0, UVec2::new(4, 4)),
            Err(SetupError::NoStamp {
                sheet: 0,
                cell: UVec2::new(4, 4)
            })
        );
    }

    #[test]
    fn coverage_reads_the_shapes_of_the_sets_mode() {
        let setup = farm();
        let coverage = setup.coverage(SAND, GRASS).expect("set");
        assert_eq!((coverage.covered, coverage.total), (2, 16));
        assert_eq!(coverage.missing.len(), 14);
        let side = LayoutTemplate::Side16.tiles(SAND, GRASS);
        assert!(!coverage.missing.contains(&side[0]), "the pure sand tile");
        assert!(!coverage.missing.contains(&side[1]), "the top edge");
        assert_eq!(
            setup.coverage(GRASS, SAND),
            Err(SetupError::UnknownSet(GRASS, SAND))
        );
    }

    #[test]
    fn flagged_tiles_mix_terrains_no_set_borders() {
        let mut setup = farm();
        assert!(setup.flagged().is_empty());
        for (tile, terrain) in [(4, GRASS), (5, SAND)] {
            let mixed = SetupTile {
                terrains: edge(terrain, 0),
                ..SetupTile::default()
            };
            setup.tiles.insert(at(tile), mixed);
        }
        let shape = SetupTile {
            shape: Tracked::user(Some(TileShape::default())),
            ..SetupTile::default()
        };
        setup.tiles.insert(at(6), shape);
        assert_eq!(setup.flagged(), [at(4), at(5)]);
    }

    #[test]
    fn a_tile_set_wholly_by_hand_is_not_flagged() {
        let mut setup = farm();
        let mixed = SetupTile {
            terrains: edge(GRASS, 0),
            ..SetupTile::default()
        };
        setup.tiles.insert(at(4), mixed);
        setup
            .set_peer_by_hand(at(4), Peer::TopLeft, 0)
            .expect("one peer");
        assert_eq!(setup.flagged(), [at(4)], "one peer by hand keeps the flag");
        setup.set_peers(at(4), edge(GRASS, 0)).expect("bits");
        assert!(setup.flagged().is_empty(), "every peer by hand clears it");
    }

    #[test]
    fn a_gap_in_a_stamps_bounds_removes_nothing() {
        let mut setup = farm();
        let ell = Stamp {
            sheet: 0,
            origin: UVec2::new(4, 4),
            size: UVec2::new(2, 2),
            cells: vec![(UVec2::ZERO, 36), (UVec2::new(0, 1), 44), (UVec2::ONE, 45)],
        };
        setup.put_stamp(ell);
        let gap = UVec2::new(5, 4);
        assert_eq!(
            setup.remove_stamp_at(0, gap),
            Err(SetupError::NoStamp {
                sheet: 0,
                cell: gap
            })
        );
        assert!(setup.remove_stamp_at(0, UVec2::new(5, 5)).is_ok());
        assert!(setup.stamps.is_empty());
    }

    #[test]
    fn a_template_over_an_object_tile_is_refused_whole() {
        let mut setup = farm();
        setup
            .put_block(0, GRID, UVec2::new(1, 1), UVec2::ONE)
            .expect("fits");
        let refused =
            setup.apply_template(LayoutTemplate::Corner2x2, SAND, GRASS, 0, GRID, UVec2::ZERO);
        assert_eq!(
            refused,
            Err(SetupError::InObject {
                tile: at(9),
                origin: UVec2::ONE
            })
        );
        assert!(!setup.tiles.contains_key(&at(8)), "nothing is laid");
    }
}
