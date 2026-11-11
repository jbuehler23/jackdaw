use std::collections::BTreeSet;

use glam::UVec2;

use crate::setup::transition;
use crate::{
    LayoutTemplate, MatchMode, Peer, Placement, Provenance, SetupError, SetupSet, SetupTile, Stamp,
    TerrainId, TileAddress, TileTerrains, TilesetSetup, fits, solver_takes,
};

/// What a layout draws: `terrain` on `base` (0 for empty), in the set at
/// `set`, or when `None` in the set holding them, made when neither has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayoutTerrains {
    pub set: Option<usize>,
    pub terrain: TerrainId,
    pub base: TerrainId,
}

impl From<(TerrainId, TerrainId)> for LayoutTerrains {
    fn from((terrain, base): (TerrainId, TerrainId)) -> Self {
        Self {
            set: None,
            terrain,
            base,
        }
    }
}

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
    /// Lays `template`'s tiles of one terrain on another, read at
    /// `placement`, by hand on the `size` block of sheet `sheet` (a grid of
    /// `grid` columns and rows) at `origin`, each in its
    /// [`LayoutTemplate::slots`] cell. A terrain with no set joins the set of
    /// the other, the drawn one above it, or a new set named by default is
    /// made for both. A set of another mode takes the placement's only while
    /// it draws no tile outside the block.
    /// Returns the tiles it set and the template tiles the block has no
    /// cell for.
    pub fn apply_template(
        &mut self,
        layout: (LayoutTemplate, Placement),
        pair: impl Into<LayoutTerrains>,
        sheet: usize,
        grid: UVec2,
        (origin, size): (UVec2, UVec2),
    ) -> Result<(Vec<TileAddress>, usize), SetupError> {
        let order = layout.0.canonical_order(size);
        self.apply_template_ordered(layout, pair, sheet, grid, (origin, size), &order)
    }

    /// [`Self::apply_template`] with tile `i` on block cell `order[i]`, row
    /// by row; see [`LayoutTemplate::ordered_slots`].
    pub fn apply_template_ordered(
        &mut self,
        (template, placement): (LayoutTemplate, Placement),
        terrains: impl Into<LayoutTerrains>,
        sheet: usize,
        grid: UVec2,
        (origin, size): (UVec2, UVec2),
        order: &[Option<u32>],
    ) -> Result<(Vec<TileAddress>, usize), SetupError> {
        if !template.placements().contains(&placement) {
            return Err(SetupError::Placement(template, placement));
        }
        let LayoutTerrains {
            set: chosen,
            terrain: inner,
            base: outer,
        } = terrains.into();
        if inner == outer {
            return Err(SetupError::SameTerrain(inner));
        }
        let shapes = template.placed_tiles(placement, inner, outer);
        let slots = (template.ordered_slots(size, order)).ok_or(SetupError::Order)?;
        let tiles = block(sheet, grid, origin, size)?;
        let at = |cell: UVec2| tiles[(cell.y * size.x + cell.x) as usize];
        for cell in slots.iter().flatten() {
            self.check_loose(at(*cell))?;
        }
        let mode = template.mode_at(placement);
        let target = self.layout_set(chosen, inner, outer, Some(mode))?;
        if let Some(index) = target {
            let had = self.set_mut(index)?.mode.value;
            if had != mode {
                match self.drawn_elsewhere(index, &tiles) {
                    0 => self.set_match_mode(index, mode)?,
                    n => return Err(SetupError::SetDrawn(had, n)),
                }
            }
        }
        let index = match target {
            Some(index) => index,
            None => self.add_set("", mode)?,
        };
        let held =
            |setup: &Self, id: TerrainId| id == 0 || setup.sets[index].terrains.contains(&id);
        if !held(self, outer) {
            let at = (self.sets[index].terrains.iter()).position(|t| *t == inner);
            self.add_set_terrain(index, outer, Some(at.unwrap_or(0)))?;
        }
        if !held(self, inner) {
            let below = (self.sets[index].terrains.iter()).position(|t| *t == outer);
            self.add_set_terrain(index, inner, below.map(|b| b + 1))?;
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

    /// The set a layout of `inner` on `outer` drawn in `mode` goes in:
    /// `chosen`, else a set of `mode` holding both terrains, else any set
    /// stacking `inner` above `outer`, else a set holding either, of `mode`
    /// first; `None` when a new set is to be made. A terrain in a set other
    /// than the target is refused, since a terrain belongs to one set. With
    /// no `mode`, the set holding both terrains or either one.
    pub fn layout_set(
        &self,
        chosen: Option<usize>,
        inner: TerrainId,
        outer: TerrainId,
        mode: Option<MatchMode>,
    ) -> Result<Option<usize>, SetupError> {
        if let Some(chosen) = chosen {
            self.sets
                .get(chosen)
                .ok_or(SetupError::UnknownSet(chosen))?;
            self.clash(chosen, [inner, outer])?;
            return Ok(Some(chosen));
        }
        let holds = |s: &SetupSet, id: TerrainId| id != 0 && s.terrains.contains(&id);
        let both = |s: &SetupSet| s.has(inner) && s.has(outer) && (inner, outer) != (0, 0);
        let either = |s: &SetupSet| holds(s, inner) || holds(s, outer);
        let Some(mode) = mode else {
            return Ok(
                (self.sets.iter().position(both)).or_else(|| self.sets.iter().position(either))
            );
        };
        let of_mode = |s: &SetupSet| s.mode.value == mode;
        let rank = |s: &SetupSet, id: TerrainId| match id {
            0 => Some(0),
            id => s.terrains.iter().position(|t| *t == id).map(|at| at + 1),
        };
        let stacked = |s: &SetupSet| {
            rank(s, inner)
                .zip(rank(s, outer))
                .is_some_and(|(a, b)| a > b)
        };
        let found = (self.sets.iter().position(|s| of_mode(s) && both(s)))
            .or_else(|| self.sets.iter().position(stacked))
            .or_else(|| self.sets.iter().position(|s| of_mode(s) && either(s)))
            .or_else(|| self.sets.iter().position(either));
        self.clash(found.unwrap_or(usize::MAX), [inner, outer])?;
        Ok(found)
    }

    /// Refuses a terrain of `terrains` that another set than `target` holds.
    fn clash(&self, target: usize, terrains: [TerrainId; 2]) -> Result<(), SetupError> {
        for terrain in terrains.into_iter().filter(|t| *t != 0) {
            let other = (self.sets.iter().enumerate())
                .find(|(i, s)| *i != target && s.terrains.contains(&terrain));
            if let Some((set, _)) = other {
                let in_target = self
                    .sets
                    .get(target)
                    .is_some_and(|s| s.terrains.contains(&terrain));
                if !in_target {
                    return Err(SetupError::TerrainInSet { terrain, set });
                }
            }
        }
        Ok(())
    }

    /// Tiles outside `block` the set at `set` holds.
    pub fn drawn_elsewhere(&self, set: usize, block: &[TileAddress]) -> usize {
        (self.tiles.iter())
            .filter(|(address, tile)| !block.contains(address) && self.holds(set, tile))
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

    /// The shapes of the full layouts of the set's transitions that the
    /// tiles it holds have, each read by the peers its mode tells tiles apart
    /// by. A transition of two of its terrains counts once a held tile shows
    /// it; a set with none counts its two lowest terrains. A set of one
    /// terrain counts it on empty.
    pub fn coverage(&self, set: usize) -> Result<Coverage, SetupError> {
        let found = self.sets.get(set).ok_or(SetupError::UnknownSet(set))?;
        let mode = found.mode.value;
        let read: Vec<Peer> = Peer::ALL.into_iter().filter(|p| mode.reads(*p)).collect();
        let key = |t: &TileTerrains| read.iter().map(|p| t.get(*p)).collect::<Vec<_>>();
        let full = match mode {
            MatchMode::Corners => LayoutTemplate::Corner16,
            MatchMode::Sides => LayoutTemplate::Side16,
            MatchMode::CornersAndSides => LayoutTemplate::Blob47,
        };
        let terrains = &found.terrains;
        let pairs: Vec<(TerrainId, TerrainId)> = (0..terrains.len())
            .flat_map(|upper| {
                let lower = std::iter::once(0).chain(terrains[..upper].iter().copied());
                lower.map(move |lower| (terrains[upper], lower))
            })
            .collect();
        let held: Vec<&SetupTile> = self.tiles.values().filter(|t| self.holds(set, t)).collect();
        let present = |(inner, outer): (TerrainId, TerrainId)| -> BTreeSet<Vec<TerrainId>> {
            (held.iter())
                .filter(|t| fits(&[inner, outer], &t.terrains.peers))
                .map(|t| key(&t.terrains))
                .collect()
        };
        let shown = |pair: &(TerrainId, TerrainId)| {
            present(*pair)
                .iter()
                .any(|k| k.contains(&pair.0) && k.contains(&pair.1))
        };
        let mut counted: Vec<(TerrainId, TerrainId)> =
            pairs.iter().copied().filter(shown).collect();
        if counted.is_empty() {
            counted = match terrains.as_slice() {
                [] => Vec::new(),
                [only] => vec![(*only, 0)],
                [lowest, next, ..] => vec![(*next, *lowest)],
            };
        }
        let (mut covered, mut total, mut missing) = (0, 0, Vec::new());
        for (inner, outer) in counted {
            let present = present((inner, outer));
            let universe = full.tiles(inner, outer);
            total += universe.len();
            for shape in universe {
                if present.contains(&key(&shape)) {
                    covered += 1;
                } else if !missing.contains(&shape) {
                    missing.push(shape);
                }
            }
        }
        Ok(Coverage {
            mode,
            covered,
            total,
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
                !pure
                    && !accepted
                    && !self
                        .sets
                        .iter()
                        .any(|s| transition(&s.terrains, &t.terrains))
            })
            .map(|(a, _)| *a)
            .collect()
    }

    /// Whether the set at `set` holds `tile`: its peers are the set's
    /// terrains and the solver takes it for the set's mode, as
    /// [`TilesetSetup::coverage`] counts it.
    pub fn holds(&self, set: usize, tile: &SetupTile) -> bool {
        let Some(found) = self.sets.get(set) else {
            return false;
        };
        let sets: Vec<(&[TerrainId], MatchMode)> = (self.sets.iter())
            .map(|s| (s.terrains.as_slice(), s.mode.value))
            .collect();
        let peers = &tile.terrains.peers;
        fits(&found.terrains, peers) && solver_takes(found.mode.value, peers, tile.set, &sets)
    }

    /// Paints `terrain` by hand on each zone of `zones` the mode of the set at
    /// `set` matches on, skipping the rest and the tiles of objects. Returns
    /// how many zones changed.
    pub fn paint_zones(
        &mut self,
        set: usize,
        terrain: TerrainId,
        zones: &[(TileAddress, Peer)],
    ) -> Result<usize, SetupError> {
        let mode = self.set_mut(set)?.mode.value;
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
    use crate::setup_edit::tests::{GRASS, SAND, at, edge, farm, pair_set};

    /// Water and shore terrains in no set yet, beside the farm's set.
    fn water_and_shore(setup: &mut TilesetSetup) -> (TerrainId, TerrainId) {
        let water = setup.add_terrain("water", [0, 0, 255], false).expect("id");
        let shore = setup
            .add_terrain("shore", [200, 200, 0], false)
            .expect("id");
        (water, shore)
    }

    const GRID: UVec2 = UVec2::new(8, 8);
    use crate::{SetupTile, TileCollision, Tracked};

    #[test]
    fn a_template_lays_its_shapes_row_by_row_and_adds_its_set() {
        let mut setup = farm();
        let (water, shore) = water_and_shore(&mut setup);
        let on = |t: LayoutTemplate| (t, Placement::OnCells);
        let block = |origin: UVec2, t: LayoutTemplate| (origin, t.shape());
        let corners = LayoutTemplate::Corner2x2;
        let pair = (water, shore);
        let (tiles, skipped) =
            (setup.apply_template(on(corners), pair, 0, GRID, block(UVec2::new(6, 3), corners)))
                .expect("fits");
        assert_eq!((tiles, skipped), (vec![at(30), at(31), at(38), at(39)], 0));
        let shapes = corners.tiles(water, shore);
        assert_eq!(setup.tiles[&at(39)].terrains, shapes[3]);
        let set = setup.set(water, shore).expect("added");
        assert_eq!(set.mode.value, MatchMode::CornersAndSides);
        assert_eq!(
            (set.name.as_str(), set.terrains.as_slice()),
            ("Terrain set 2", &[shore, water][..])
        );
        assert_eq!(setup.coverage(1).expect("set").covered, 4);

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
        let (water, shore) = water_and_shore(&mut setup);
        let minimal = LayoutTemplate::Minimal3x3;
        let block = |x: u32| (UVec2::new(x, 0), UVec2::splat(3));
        let pair = (water, shore);
        let ground = (minimal, Placement::BetweenCells);
        setup
            .apply_template(ground, pair, 0, GRID, block(0))
            .expect("laid");
        let walls = (minimal, Placement::OnCells);
        let refused = setup.apply_template(walls, pair, 0, GRID, block(4));
        assert_eq!(refused, Err(SetupError::SetDrawn(MatchMode::Corners, 9)));
        assert_eq!(
            refused.map(|_| ()).unwrap_err().to_string(),
            "this terrain set already draws between cells; it has 9 tiles elsewhere"
        );
        let again = setup.apply_template(walls, pair, 0, GRID, block(0));
        assert!(again.is_ok(), "nothing else is drawn, so it may switch");
    }

    #[test]
    fn a_ground_3x3_lays_corners_only_and_turns_its_set_to_corners() {
        let mut setup = farm();
        let (water, shore) = water_and_shore(&mut setup);
        let minimal = LayoutTemplate::Minimal3x3;
        let ground = (minimal, Placement::BetweenCells);
        let size = UVec2::splat(3);
        setup
            .apply_template(
                (minimal, Placement::OnCells),
                (water, shore),
                0,
                GRID,
                (UVec2::ZERO, size),
            )
            .expect("on cells");
        let (tiles, _) =
            (setup.apply_template(ground, (water, shore), 0, GRID, (UVec2::ZERO, size)))
                .expect("fits");
        assert_eq!(tiles.len(), 9);
        let set = setup.set(water, shore).expect("kept");
        assert_eq!(set.mode.value, MatchMode::Corners, "the placement's mode");
        let top_left = setup.tiles[&at(0)].terrains;
        let expected = TileTerrains::filled(shore)
            .with(Peer::Centre, water)
            .with(Peer::BottomRight, water);
        assert_eq!(top_left, expected, "one inner corner, sides outer");
        let centre = setup.tiles[&at(9)].terrains;
        assert!(Peer::CORNERS.iter().all(|c| centre.get(*c) == water));
    }

    #[test]
    fn a_layout_goes_in_the_one_set_its_terrain_belongs_to() {
        let mut setup = farm();
        let water = setup.add_terrain("water", [0, 0, 255], false).expect("id");
        let corners = (LayoutTemplate::Corner2x2, Placement::OnCells);
        let block = (UVec2::new(6, 3), UVec2::splat(2));
        let refused = setup.apply_template(corners, (water, GRASS), 0, GRID, block);
        assert_eq!(refused, Err(SetupError::SetDrawn(MatchMode::Sides, 3)));
        assert_eq!(setup.sets.len(), 1, "no second set takes grass");
    }

    #[test]
    fn a_blob_in_a_wide_block_flows_row_by_row_and_reports_what_it_skips() {
        let mut setup = farm();
        let (water, shore) = water_and_shore(&mut setup);
        let blob = (LayoutTemplate::Blob47, Placement::OnCells);
        let size = UVec2::new(8, 5);
        let (tiles, skipped) =
            (setup.apply_template(blob, (water, shore), 0, GRID, (UVec2::ZERO, size)))
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
        let coverage = setup.coverage(0).expect("set");
        assert_eq!((coverage.covered, coverage.total), (2, 16));
        assert_eq!(coverage.missing.len(), 14);
        let side = LayoutTemplate::Side16.tiles(SAND, GRASS);
        assert!(!coverage.missing.contains(&side[0]), "the pure sand tile");
        assert!(!coverage.missing.contains(&side[1]), "the top edge");
        assert_eq!(setup.coverage(1), Err(SetupError::UnknownSet(1)));
    }

    #[test]
    fn corner_coverage_counts_the_sixteen_corner_patterns() {
        let mut setup = farm();
        let (dirt, shore) = water_and_shore(&mut setup);
        let corners = pair_set(&mut setup, dirt, shore, MatchMode::Corners);
        let shapes = LayoutTemplate::Corner16.tiles(dirt, shore);
        for (tile, shape) in (40..).zip(&shapes) {
            let corners = (Peer::CORNERS.iter())
                .fold(TileTerrains::default(), |t, p| t.with(*p, shape.get(*p)));
            setup.set_peers(at(tile), corners).expect("bits");
        }
        setup
            .set_peers(at(0), TileTerrains::filled(shore))
            .expect("bits");
        let coverage = setup.coverage(corners).expect("set");
        assert_eq!(
            (coverage.covered, coverage.total),
            (16, 16),
            "{:?}",
            coverage.missing
        );

        setup.tiles.remove(&at(0));
        let plain_corners = setup.tiles[&at(55)].clone();
        assert!(setup.holds(corners, &plain_corners), "shore corners alone");
        let coverage = setup.coverage(corners).expect("set");
        assert_eq!(coverage.covered, 16, "the plain corner tile serves it");
    }

    /// Auto setup may list a terrain in sets of two match modes.
    #[test]
    fn a_plain_tile_two_modes_share_is_left_out() {
        let mut setup = farm();
        let dirt = setup.add_terrain("dirt", [0, 0, 255], false).expect("id");
        let corners = setup.add_set("", MatchMode::Corners).expect("a set");
        setup.sets[corners].terrains = vec![GRASS, dirt];
        let sides = 0;
        for (tile, shape) in (40..).zip(&LayoutTemplate::Corner16.tiles(dirt, GRASS)) {
            let corner_peers = (Peer::CORNERS.iter())
                .fold(TileTerrains::default(), |t, p| t.with(*p, shape.get(*p)));
            setup.set_peers(at(tile), corner_peers).expect("bits");
        }
        setup.tiles.remove(&at(0));
        let plain_corners = setup.tiles[&at(55)].clone();
        assert!(
            !setup.holds(corners, &plain_corners),
            "grass corners alone also fit the sides set as tightly, so the solver leaves the tile out"
        );
        setup
            .set_peers(at(0), TileTerrains::filled(GRASS))
            .expect("bits");
        assert!(setup.holds(corners, &setup.tiles[&at(0)]));
        assert!(setup.holds(sides, &setup.tiles[&at(0)]));
    }

    #[test]
    fn coverage_counts_a_terrain_on_empty_once_a_tile_shows_it() {
        let mut setup = farm();
        let before = setup.coverage(0).expect("set");
        let edge = SetupTile {
            terrains: edge(SAND, 0),
            ..SetupTile::default()
        };
        setup.tiles.insert(at(3), edge);
        let after = setup.coverage(0).expect("set");
        assert_eq!(after.total, before.total + 16, "sand on empty joins");
        assert_eq!(after.covered, before.covered + 2, "the edge and pure sand");
        assert!(setup.flagged().is_empty(), "a transition to empty");
    }

    #[test]
    fn flagged_tiles_mix_terrains_no_set_borders() {
        let mut setup = farm();
        assert!(setup.flagged().is_empty());
        let water = setup.add_terrain("water", [0, 0, 255], false).expect("id");
        pair_set(&mut setup, water, 0, MatchMode::Corners);
        for (tile, terrain) in [(4, GRASS), (5, SAND)] {
            let mixed = SetupTile {
                terrains: edge(terrain, water),
                ..SetupTile::default()
            };
            setup.tiles.insert(at(tile), mixed);
        }
        let shape = SetupTile {
            collision: Tracked::user(vec![TileCollision::default()]),
            ..SetupTile::default()
        };
        setup.tiles.insert(at(6), shape);
        assert_eq!(setup.flagged(), [at(4), at(5)]);
    }

    #[test]
    fn a_tile_set_wholly_by_hand_is_not_flagged() {
        let mut setup = farm();
        let water = setup.add_terrain("water", [0, 0, 255], false).expect("id");
        let mixed = SetupTile {
            terrains: edge(GRASS, water),
            ..SetupTile::default()
        };
        setup.tiles.insert(at(4), mixed);
        setup
            .set_peer_by_hand(at(4), Peer::TopLeft, GRASS)
            .expect("one peer");
        assert_eq!(setup.flagged(), [at(4)], "one peer by hand keeps the flag");
        setup.set_peers(at(4), edge(GRASS, water)).expect("bits");
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
        assert_eq!(setup.paint_zones(0, SAND, &zones), Ok(2));
        assert_eq!(setup.tiles[&at(3)].terrains.get(Peer::Top), SAND);
        assert_eq!(setup.tiles[&at(3)].terrains.get(Peer::TopLeft), 0);
        assert_eq!(setup.tiles[&at(4)].terrains.centre(), SAND);
        assert!(setup.tiles[&at(3)].provenance[Peer::Top.index()] == Provenance::User);
        assert!(setup.holds(0, &setup.tiles[&at(3)]));
        assert_eq!(
            setup.paint_zones(0, SAND, &zones),
            Ok(0),
            "nothing left to change"
        );
        assert_eq!(setup.paint_zones(0, 0, &zones[..1]), Ok(1));
        assert_eq!(setup.tiles[&at(3)].terrains.get(Peer::Top), 0, "erased");
        assert_eq!(
            setup.paint_zones(1, GRASS, &zones),
            Err(SetupError::UnknownSet(1))
        );
        setup
            .put_block(0, GRID, UVec2::new(5, 0), UVec2::ONE)
            .expect("an object");
        let object = [(at(5), Peer::Top)];
        assert_eq!(setup.paint_zones(0, SAND, &object), Ok(0));
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
