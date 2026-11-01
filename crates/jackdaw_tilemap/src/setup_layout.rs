use std::collections::BTreeSet;

use glam::UVec2;

use crate::setup::transition;
use crate::{
    LayoutTemplate, MatchMode, Peer, Placement, Provenance, SetupError, SetupSet, SetupTile, Stamp,
    TerrainId, TileAddress, TileTerrains, TilesetSetup, fits, solver_takes,
};

/// How much of its full layout a terrain set's tiles cover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Coverage {
    pub mode: MatchMode,
    pub covered: usize,
    pub total: usize,
    /// Shapes of the full layout no tile of the set has.
    pub missing: Vec<TileTerrains>,
}

impl TilesetSetup {
    /// Lays `template`'s tiles of `inner` over `outer`, read at `placement`,
    /// by hand on the `size` block of sheet `sheet` (a grid of `grid` columns
    /// and rows) at `origin`, each in its [`LayoutTemplate::slots`] cell.
    /// Adds the set when it is missing. A set of another mode takes the
    /// placement's only while it draws no tile outside the block.
    /// Returns the tiles it set and the template tiles the block has no
    /// cell for.
    pub fn apply_template(
        &mut self,
        (template, placement): (LayoutTemplate, Placement),
        (inner, outer): (TerrainId, TerrainId),
        sheet: usize,
        grid: UVec2,
        (origin, size): (UVec2, UVec2),
    ) -> Result<(Vec<TileAddress>, usize), SetupError> {
        if !template.placements().contains(&placement) {
            return Err(SetupError::Placement(template, placement));
        }
        let shapes = template.placed_tiles(placement, inner, outer);
        let slots = template.slots(size);
        let tiles = block(sheet, grid, origin, size)?;
        let at = |cell: UVec2| tiles[(cell.y * size.x + cell.x) as usize];
        for cell in slots.iter().flatten() {
            self.check_loose(at(*cell))?;
        }
        let mode = template.mode_at(placement);
        match self.set(inner, outer).map(|s| s.mode.value) {
            None => self.add_set(inner, outer, mode)?,
            Some(had) if had != mode => match self.drawn_elsewhere((inner, outer), &tiles) {
                0 => self.set_match_mode(inner, outer, mode)?,
                n => return Err(SetupError::SetDrawn(had, n)),
            },
            Some(_) => {}
        }
        let mut set = Vec::with_capacity(shapes.len());
        for (cell, peers) in slots.iter().zip(shapes) {
            let Some(address) = cell.map(at) else {
                continue;
            };
            self.set_peers(address, peers)?;
            if let Some(tile) = self.tiles.get_mut(&address) {
                tile.set = Some((inner, outer));
            }
            set.push(address);
        }
        let skipped = slots.iter().filter(|s| s.is_none()).count();
        Ok((set, skipped))
    }

    /// Tiles outside `block` the set of the pair draws with its inner terrain.
    pub fn drawn_elsewhere(
        &self,
        (inner, outer): (TerrainId, TerrainId),
        block: &[TileAddress],
    ) -> usize {
        (self.tiles.iter())
            .filter(|(address, tile)| {
                !block.contains(address)
                    && tile.terrains.peers.contains(&inner)
                    && self.holds(inner, outer, tile)
            })
            .count()
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

    /// The shapes of the set's full layout that the tiles it holds have, each
    /// read by the peers its mode tells tiles apart by.
    pub fn coverage(&self, inner: TerrainId, outer: TerrainId) -> Result<Coverage, SetupError> {
        let mode = (self.set(inner, outer))
            .ok_or(SetupError::UnknownSet(inner, outer))?
            .mode
            .value;
        let read: Vec<Peer> = Peer::ALL.into_iter().filter(|p| mode.reads(*p)).collect();
        let key = |t: &TileTerrains| read.iter().map(|p| t.get(*p)).collect::<Vec<_>>();
        let present: BTreeSet<Vec<TerrainId>> = (self.tiles.values())
            .filter(|t| self.holds(inner, outer, t))
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
            mode,
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

    /// Whether the set of `inner` over `outer` holds `tile`: its peers are
    /// the set's terrains and the solver takes it for the set's mode, as
    /// [`TilesetSetup::coverage`] counts it.
    pub fn holds(&self, inner: TerrainId, outer: TerrainId, tile: &SetupTile) -> bool {
        let Some(set) = self.set(inner, outer) else {
            return false;
        };
        let sets: Vec<_> = (self.sets.iter())
            .map(|s| ((s.inner, s.outer), s.mode.value))
            .collect();
        let peers = &tile.terrains.peers;
        fits((inner, outer), peers) && solver_takes(set.mode.value, peers, tile.set, &sets)
    }

    /// Paints `terrain` by hand on each zone of `zones` the mode of the set
    /// of `inner` over `outer` matches on, skipping the rest and the tiles of
    /// objects. Returns how many zones changed.
    pub fn paint_zones(
        &mut self,
        inner: TerrainId,
        outer: TerrainId,
        terrain: TerrainId,
        zones: &[(TileAddress, Peer)],
    ) -> Result<usize, SetupError> {
        let mode = (self.set(inner, outer))
            .ok_or(SetupError::UnknownSet(inner, outer))?
            .mode
            .value;
        let mut changed = 0;
        for &(tile, peer) in zones {
            let now = self.tiles.get(&tile).map_or(0, |t| t.terrains.get(peer));
            if !mode.uses(peer) || now == terrain || self.stamp_holding(tile).is_some() {
                continue;
            }
            self.set_peer_by_hand(tile, peer, terrain)?;
            changed += 1;
        }
        Ok(changed)
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
        let on = |t: LayoutTemplate| (t, Placement::OnCells);
        let block = |origin: UVec2, t: LayoutTemplate| (origin, t.shape());
        let corners = LayoutTemplate::Corner2x2;
        let pair = (water, GRASS);
        let (tiles, skipped) =
            (setup.apply_template(on(corners), pair, 0, GRID, block(UVec2::new(6, 3), corners)))
                .expect("fits");
        assert_eq!((tiles, skipped), (vec![at(30), at(31), at(38), at(39)], 0));
        let shapes = corners.tiles(water, GRASS);
        assert_eq!(setup.tiles[&at(39)].terrains, shapes[3]);
        let set = setup.set(water, GRASS).expect("added");
        assert_eq!(set.mode.value, MatchMode::CornersAndSides);
        assert_eq!(setup.coverage(water, GRASS).expect("set").covered, 4);

        let side = LayoutTemplate::Side16;
        let refused = setup.apply_template(on(side), pair, 0, GRID, block(UVec2::X * 5, side));
        assert_eq!(
            refused,
            Err(SetupError::OutsideSheet {
                origin: UVec2::new(5, 0),
                size: UVec2::new(4, 4)
            })
        );
        let blob = LayoutTemplate::Blob47;
        let laid = setup.apply_template(on(blob), pair, 0, GRID, block(UVec2::ZERO, blob));
        assert!(laid.is_ok());
        let past = setup.apply_template(on(blob), pair, 0, GRID, block(UVec2::Y * 3, blob));
        assert!(past.is_err());
        let between = (LayoutTemplate::Side16, Placement::BetweenCells);
        let wrong = setup.apply_template(between, pair, 0, GRID, block(UVec2::ZERO, side));
        assert!(matches!(wrong, Err(SetupError::Placement(..))));
    }

    #[test]
    fn a_set_drawing_tiles_elsewhere_keeps_its_placement() {
        let mut setup = farm();
        let water = setup.add_terrain("water", [0, 0, 255], false).expect("id");
        let minimal = LayoutTemplate::Minimal3x3;
        let block = |x: u32| (UVec2::new(x, 0), UVec2::splat(3));
        let pair = (water, GRASS);
        let ground = (minimal, Placement::BetweenCells);
        setup
            .apply_template(ground, pair, 0, GRID, block(0))
            .expect("laid");
        let walls = (minimal, Placement::OnCells);
        let refused = setup.apply_template(walls, pair, 0, GRID, block(4));
        assert_eq!(refused, Err(SetupError::SetDrawn(MatchMode::Corners, 9)));
        assert_eq!(
            refused.map(|_| ()).unwrap_err().to_string(),
            "this pair already draws between cells; it has 9 tiles elsewhere"
        );
        let again = setup.apply_template(walls, pair, 0, GRID, block(0));
        assert!(again.is_ok(), "nothing else is drawn, so it may switch");
    }

    #[test]
    fn a_ground_3x3_lays_corners_only_and_turns_its_set_to_corners() {
        let mut setup = farm();
        let water = setup.add_terrain("water", [0, 0, 255], false).expect("id");
        let minimal = LayoutTemplate::Minimal3x3;
        let ground = (minimal, Placement::BetweenCells);
        let size = UVec2::splat(3);
        setup
            .apply_template(
                (minimal, Placement::OnCells),
                (water, GRASS),
                0,
                GRID,
                (UVec2::ZERO, size),
            )
            .expect("on cells");
        let (tiles, _) =
            (setup.apply_template(ground, (water, GRASS), 0, GRID, (UVec2::ZERO, size)))
                .expect("fits");
        assert_eq!(tiles.len(), 9);
        let set = setup.set(water, GRASS).expect("kept");
        assert_eq!(set.mode.value, MatchMode::Corners, "the placement's mode");
        let top_left = setup.tiles[&at(0)].terrains;
        let expected = TileTerrains::filled(GRASS)
            .with(Peer::Centre, water)
            .with(Peer::BottomRight, water);
        assert_eq!(top_left, expected, "one inner corner, sides outer");
        let centre = setup.tiles[&at(9)].terrains;
        assert!(Peer::CORNERS.iter().all(|c| centre.get(*c) == water));
    }

    #[test]
    fn a_blob_in_a_wide_block_flows_row_by_row_and_reports_what_it_skips() {
        let mut setup = farm();
        let water = setup.add_terrain("water", [0, 0, 255], false).expect("id");
        let blob = (LayoutTemplate::Blob47, Placement::OnCells);
        let size = UVec2::new(8, 5);
        let (tiles, skipped) =
            (setup.apply_template(blob, (water, GRASS), 0, GRID, (UVec2::ZERO, size)))
                .expect("fits");
        assert_eq!((tiles.len(), skipped), (40, 7));
        assert_eq!(tiles[39], at(39));
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
    fn corner_coverage_counts_the_sixteen_corner_patterns_with_shared_plain_tiles() {
        let mut setup = farm();
        let dirt = setup.add_terrain("dirt", [120, 80, 40], false).expect("id");
        setup.add_set(dirt, GRASS, MatchMode::Corners).expect("set");
        let shapes = LayoutTemplate::Corner16.tiles(dirt, GRASS);
        for (tile, shape) in (40..).zip(&shapes) {
            let corners = (Peer::CORNERS.iter())
                .fold(TileTerrains::default(), |t, p| t.with(*p, shape.get(*p)));
            setup.set_peers(at(tile), corners).expect("bits");
        }
        let coverage = setup.coverage(dirt, GRASS).expect("set");
        assert_eq!(
            (coverage.covered, coverage.total),
            (16, 16),
            "{:?}",
            coverage.missing
        );

        setup.tiles.remove(&at(0));
        let plain_corners = setup.tiles[&at(55)].clone();
        assert!(
            !setup.holds(dirt, GRASS, &plain_corners),
            "grass corners alone also fit the sides set, so the solver leaves the tile out"
        );
        let coverage = setup.coverage(dirt, GRASS).expect("set");
        assert_eq!(coverage.covered, 15, "only the whole grass tile served it");
        setup
            .set_peers(at(0), TileTerrains::filled(GRASS))
            .expect("bits");
        assert!(setup.holds(dirt, GRASS, &setup.tiles[&at(0)]));
        assert!(setup.holds(SAND, GRASS, &setup.tiles[&at(0)]));
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
        let refused = setup.apply_template(
            (LayoutTemplate::Corner2x2, Placement::OnCells),
            (SAND, GRASS),
            0,
            GRID,
            (UVec2::ZERO, UVec2::splat(2)),
        );
        assert_eq!(
            refused,
            Err(SetupError::InObject {
                tile: at(9),
                origin: UVec2::ONE
            })
        );
        assert!(!setup.tiles.contains_key(&at(8)), "nothing is laid");
    }

    #[test]
    fn a_point_in_a_tile_picks_the_zone_its_mode_matches_on() {
        let under = |x: f32, y: f32, mode| Peer::under(glam::Vec2::new(x, y), mode);
        let all = MatchMode::CornersAndSides;
        assert_eq!(under(0.1, 0.1, all), Peer::TopLeft);
        assert_eq!(under(0.5, 0.1, all), Peer::Top);
        assert_eq!(under(0.9, 0.5, all), Peer::Right);
        assert_eq!(under(0.5, 0.5, all), Peer::Centre);
        assert_eq!(under(0.2, 0.9, all), Peer::BottomLeft);
        let corners = MatchMode::Corners;
        assert_eq!(under(0.45, 0.1, corners), Peer::TopLeft);
        assert_eq!(under(0.55, 0.2, corners), Peer::TopRight);
        assert_eq!(under(0.1, 0.6, corners), Peer::BottomLeft);
        assert_eq!(under(0.5, 0.5, corners), Peer::Centre);
        let sides = MatchMode::Sides;
        assert_eq!(under(0.1, 0.1 + 0.05, sides), Peer::Left);
        assert_eq!(under(0.2, 0.1, sides), Peer::Top);
        assert_eq!(under(0.9, 0.8, sides), Peer::Right);
        assert_eq!(under(0.6, 0.95, sides), Peer::Bottom);
        assert_eq!(under(0.5, 0.5, sides), Peer::Centre);
        assert_eq!(under(1.0, 1.0, all), Peer::BottomRight, "the far edge");
    }

    #[test]
    fn painting_zones_sets_only_the_peers_its_set_matches_on() {
        let mut setup = farm();
        let zones = [
            (at(3), Peer::Top),
            (at(3), Peer::TopLeft),
            (at(4), Peer::Centre),
        ];
        assert_eq!(setup.paint_zones(SAND, GRASS, SAND, &zones), Ok(2));
        assert_eq!(setup.tiles[&at(3)].terrains.get(Peer::Top), SAND);
        assert_eq!(setup.tiles[&at(3)].terrains.get(Peer::TopLeft), 0);
        assert_eq!(setup.tiles[&at(4)].terrains.centre(), SAND);
        assert!(setup.tiles[&at(3)].provenance[Peer::Top.index()] == Provenance::User);
        assert!(setup.holds(SAND, GRASS, &setup.tiles[&at(3)]));
        assert_eq!(
            setup.paint_zones(SAND, GRASS, SAND, &zones),
            Ok(0),
            "nothing left to change"
        );
        assert_eq!(setup.paint_zones(SAND, GRASS, 0, &zones[..1]), Ok(1));
        assert_eq!(setup.tiles[&at(3)].terrains.get(Peer::Top), 0, "erased");
        assert_eq!(
            setup.paint_zones(GRASS, SAND, GRASS, &zones),
            Err(SetupError::UnknownSet(GRASS, SAND))
        );
        setup
            .put_block(0, GRID, UVec2::new(5, 0), UVec2::ONE)
            .expect("an object");
        let object = [(at(5), Peer::Top)];
        assert_eq!(setup.paint_zones(SAND, GRASS, SAND, &object), Ok(0));
        assert!(!setup.tiles.contains_key(&at(5)), "an object tile is left");
    }

    #[test]
    fn a_grid_finds_the_tile_and_the_point_in_it() {
        let grid = crate::GridLayout {
            tile: UVec2::splat(16),
            margin: UVec2::splat(2),
            spacing: UVec2::splat(1),
            columns: 3,
            rows: 2,
        };
        let v = glam::Vec2::new;
        assert_eq!(grid.tile_at(v(2.0, 2.0)), Some((0, v(0.0, 0.0))));
        assert_eq!(
            grid.tile_at(v(19.0 + 4.0, 2.0 + 8.0)),
            Some((1, v(0.25, 0.5)))
        );
        assert_eq!(
            grid.tile_at(v(36.0 + 15.0, 19.0 + 15.0)),
            Some((5, v(15.0 / 16.0, 15.0 / 16.0)))
        );
        assert_eq!(grid.tile_at(v(1.0, 5.0)), None, "the margin");
        assert_eq!(grid.tile_at(v(18.5, 5.0)), None, "the spacing");
        assert_eq!(grid.tile_at(v(60.0, 5.0)), None, "past the grid");
        assert_eq!(grid.tile_at(v(-3.0, 5.0)), None);
    }
}
