//! Stamps: multi-tile objects among the tiles no terrain set claims.
//!
//! - Candidates: non-empty tiles that are neither in a terrain set nor pure
//!   tiles of a terrain in one (decor, unassigned tiles and pure tiles of
//!   terrains in no set).
//! - Join: two candidates side by side on a sheet join across their shared
//!   edge when opaque pixels touch across it (within one pixel along it),
//!   its seam is within [`STAMP_SEAM`], and under [`OUTLINE_SHARE`] of the
//!   positions where both edge pixels are opaque are a doubled outline.
//!   Objects drawn against each other split even where a strip of ground
//!   above them runs on.
//! - Outline: an edge pixel is one when the first pixel inward, within a
//!   quarter tile, that differs from it by more than [`OUTLINE_STEP`] is
//!   lighter by more than that; that pixel is the fill behind it. A doubled
//!   outline has one on both sides.
//! - Seam: `layout_fit`'s measure over the positions where both edge pixels
//!   are opaque (so a narrow part may meet a wide one), of the edge pixels or
//!   of the edge pixels with outlines replaced by their fills, whichever is
//!   lower, so a line drawn along a tile edge inside an object does not
//!   break it. Without such a position the tiles do not join.
//! - Stamps: connected groups of two or more tiles, ordered by sheet, origin
//!   row, origin column and first tile.
//! - Reconcile: tiles in stamps leave the proposal's unassigned, low
//!   confidence and pure tile lists; terrains left in no set and without
//!   pure tiles are dropped. Pure tiles of terrains in a set are never
//!   candidates, so background flags stand.

use std::collections::{BTreeMap, BTreeSet};

use glam::{UVec2, Vec3};

use crate::layout_fit::{check_inputs, seam, side_pixel, side_pixels, widen};
use crate::{FitError, SheetInput, TerrainAnalysis, TileAddress, TilesetProposal};

/// Largest seam across a joined edge; looser than a terrain seam because
/// object art keeps its texture across tiles.
const STAMP_SEAM: f32 = 0.1;
/// Oklab step from an outline pixel to the fill inside it.
const OUTLINE_STEP: f32 = 0.1;
/// Share of doubled outline positions that splits an edge.
const OUTLINE_SHARE: f32 = 0.4;
/// [`Peer::SIDES`](crate::Peer::SIDES) slots of the right and bottom edges.
const RIGHT: usize = 2;
const BOTTOM: usize = 3;

/// Tiles of one sheet that form one object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub sheet: usize,
    /// Column and row of the top-left corner of its bounds.
    pub origin: UVec2,
    /// Bounds in tiles.
    pub size: UVec2,
    /// Offset from `origin` and tile id of each member, by tile id; cells of
    /// the bounds without a member are gaps.
    pub cells: Vec<(UVec2, u32)>,
}

impl Stamp {
    pub fn tiles(&self) -> impl Iterator<Item = TileAddress> + '_ {
        let sheet = self.sheet;
        (self.cells.iter()).map(move |&(_, tile)| TileAddress { sheet, tile })
    }
}

/// Groups the analysed tiles that no set of `proposal` claims into stamps.
pub fn find_stamps(
    analysis: &TerrainAnalysis,
    inputs: &[SheetInput],
    proposal: &TilesetProposal,
) -> Result<Vec<Stamp>, FitError> {
    check_inputs(analysis, inputs)?;
    let set_terrains: BTreeSet<_> = proposal
        .sets
        .iter()
        .flat_map(|s| [s.inner, s.outer])
        .collect();
    let set_tiles = proposal
        .sets
        .iter()
        .flat_map(|s| s.tiles.iter().map(|t| t.address));
    let set_pure = (proposal.terrains.iter())
        .filter(|t| set_terrains.contains(&t.id))
        .flat_map(|t| t.pure_tiles.iter().copied());
    let claimed: BTreeSet<TileAddress> = set_tiles.chain(set_pure).collect();
    let mut candidates = BTreeSet::new();
    for (sheet, tiles) in analysis.tiles.iter().enumerate() {
        for (tile, labels) in (0..).zip(tiles) {
            let address = TileAddress { sheet, tile };
            if labels.is_some() && !claimed.contains(&address) {
                candidates.insert(address);
            }
        }
    }
    Ok(group(inputs, &candidates))
}

/// `proposal` with the tiles of `stamps` taken out of its unassigned, low
/// confidence and pure tile lists, less terrains left in no set and without
/// pure tiles.
pub fn reconcile(mut proposal: TilesetProposal, stamps: &[Stamp]) -> TilesetProposal {
    let claimed: BTreeSet<TileAddress> = stamps.iter().flat_map(Stamp::tiles).collect();
    proposal.unassigned.retain(|a| !claimed.contains(a));
    proposal.low_confidence.retain(|a| !claimed.contains(a));
    let in_sets: BTreeSet<_> = proposal
        .sets
        .iter()
        .flat_map(|s| [s.inner, s.outer])
        .collect();
    for terrain in &mut proposal.terrains {
        terrain.pure_tiles.retain(|a| !claimed.contains(a));
    }
    proposal
        .terrains
        .retain(|t| in_sets.contains(&t.id) || !t.pure_tiles.is_empty());
    proposal
}

fn group(inputs: &[SheetInput], candidates: &BTreeSet<TileAddress>) -> Vec<Stamp> {
    let index: Vec<TileAddress> = candidates.iter().copied().collect();
    let mut parent: Vec<usize> = (0..index.len()).collect();
    for (i, a) in index.iter().enumerate() {
        let Some(input) = inputs.get(a.sheet) else {
            continue;
        };
        let columns = input.layout.columns.max(1);
        let right = (a.tile % columns + 1 < columns).then_some((a.tile + 1, RIGHT));
        for (tile, slot) in [right, Some((a.tile + columns, BOTTOM))]
            .into_iter()
            .flatten()
        {
            let Ok(j) = index.binary_search(&TileAddress { tile, ..*a }) else {
                continue;
            };
            if joins(input, a.tile, tile, slot) {
                let (ri, rj) = (root(&mut parent, i), root(&mut parent, j));
                parent[ri.max(rj)] = ri.min(rj);
            }
        }
    }
    let mut members: BTreeMap<usize, Vec<TileAddress>> = BTreeMap::new();
    for (i, a) in index.iter().enumerate() {
        members.entry(root(&mut parent, i)).or_default().push(*a);
    }
    let mut stamps: Vec<Stamp> = (members.into_values())
        .filter(|m| m.len() >= 2)
        .filter_map(|m| stamp(inputs, &m))
        .collect();
    stamps.sort_by_key(|s| {
        (
            s.sheet,
            s.origin.y,
            s.origin.x,
            s.cells.first().map(|c| c.1),
        )
    });
    stamps
}

fn root(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// Whether `tile` joins `other`, the tile across its side `slot`.
fn joins(input: &SheetInput, tile: u32, other: u32, slot: usize) -> bool {
    let facing = BOTTOM - slot;
    let (Some(a), Some(b)) = (
        side_pixels(input, tile, slot),
        side_pixels(input, other, facing),
    ) else {
        return false;
    };
    let touch = (a.iter().enumerate())
        .any(|(i, p)| p.is_some() && b[widen(i..i + 1, b.len())].iter().any(Option::is_some));
    if !touch || a.len() != b.len() {
        return false;
    }
    let (fills_a, fills_b) = (
        fills(input, tile, slot, &a),
        fills(input, other, facing, &b),
    );
    let both = |x: &[Option<Vec3>], y: &[Option<Vec3>]| -> Vec<Option<Vec3>> {
        x.iter().zip(y).map(|(p, q)| q.and(*p)).collect()
    };
    let raw = seam(&both(&a, &b), &both(&b, &a), 0..a.len());
    let behind = seam(
        &both(&fills_a, &fills_b),
        &both(&fills_b, &fills_a),
        0..a.len(),
    );
    if raw
        .into_iter()
        .chain(behind)
        .reduce(f32::min)
        .is_none_or(|s| s > STAMP_SEAM)
    {
        return false;
    }
    let (mut opaque, mut doubled) = (0usize, 0usize);
    for (i, (p, q)) in a.iter().zip(&b).enumerate() {
        if p.is_some() && q.is_some() {
            opaque += 1;
            doubled += usize::from(fills_a[i] != *p && fills_b[i] != *q);
        }
    }
    (doubled as f32) < OUTLINE_SHARE * opaque as f32
}

/// `edge`, side `slot` of `tile`, with each outline pixel replaced by the
/// fill behind it, read inward only as far as the first differing pixel.
fn fills(input: &SheetInput, tile: u32, slot: usize, edge: &[Option<Vec3>]) -> Vec<Option<Vec3>> {
    let depth = input.layout.tile.min_element() / 4;
    let behind = |i: u32, e: Vec3| {
        for d in 1..=depth {
            let p = side_pixel(input, tile, slot, d, i)?;
            if p.distance(e) > OUTLINE_STEP {
                return (p.x > e.x + OUTLINE_STEP).then_some(p);
            }
        }
        None
    };
    (0..)
        .zip(edge)
        .map(|(i, e)| e.map(|e| behind(i, e).unwrap_or(e)))
        .collect()
}

/// The stamp of `members`, tiles of one sheet in tile id order.
fn stamp(inputs: &[SheetInput], members: &[TileAddress]) -> Option<Stamp> {
    let sheet = members.first()?.sheet;
    let columns = inputs.get(sheet)?.layout.columns.max(1);
    let at = |tile: u32| UVec2::new(tile % columns, tile / columns);
    let tiles = members.iter().map(|a| at(a.tile));
    let origin = tiles.clone().fold(UVec2::MAX, UVec2::min);
    let far = tiles.fold(UVec2::ZERO, UVec2::max);
    let cells = (members.iter())
        .map(|a| (at(a.tile) - origin, a.tile))
        .collect();
    Some(Stamp {
        sheet,
        origin,
        size: far - origin + UVec2::ONE,
        cells,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        FittedTile, GridLayout, MatchMode, ProposedSet, ProposedTerrain, RgbaImage, TerrainId,
        TileKind, TileLabels, TileTerrains,
    };

    const T: u32 = 16;
    const DARK: [u8; 4] = [60, 40, 40, 255];
    const WOOD: [u8; 4] = [200, 140, 90, 255];
    const SKY: [u8; 4] = [90, 150, 230, 255];

    /// A sheet of `columns x rows` tiles painted per pixel.
    struct Sheet {
        columns: u32,
        rows: u32,
        pixels: Vec<u8>,
    }

    impl Sheet {
        fn new(columns: u32, rows: u32, paint: impl Fn(u32, u32) -> [u8; 4]) -> Self {
            let mut pixels = Vec::new();
            for y in 0..rows * T {
                for x in 0..columns * T {
                    pixels.extend(paint(x, y));
                }
            }
            Self {
                columns,
                rows,
                pixels,
            }
        }

        fn input(&self) -> SheetInput<'_> {
            let (width, height) = (self.columns * T, self.rows * T);
            SheetInput {
                name: "objects.png",
                image: RgbaImage::new(width, height, &self.pixels).expect("sized"),
                layout: GridLayout {
                    tile: UVec2::splat(T),
                    margin: UVec2::ZERO,
                    spacing: UVec2::ZERO,
                    columns: self.columns,
                    rows: self.rows,
                },
            }
        }
    }

    /// A box over pixels `min..max` with a two pixel outline.
    fn boxed(x: u32, y: u32, min: UVec2, max: UVec2, fill: [u8; 4]) -> Option<[u8; 4]> {
        let inside = (min.x..max.x).contains(&x) && (min.y..max.y).contains(&y);
        let border = x < min.x + 2 || y < min.y + 2 || x + 2 >= max.x || y + 2 >= max.y;
        inside.then_some(if border { DARK } else { fill })
    }

    fn all(sheet: &Sheet) -> BTreeSet<TileAddress> {
        (0..sheet.columns * sheet.rows)
            .map(|tile| TileAddress { sheet: 0, tile })
            .collect()
    }

    fn stamps(sheet: &Sheet, candidates: &BTreeSet<TileAddress>) -> Vec<Stamp> {
        group(&[sheet.input()], candidates)
    }

    fn cells(stamp: &Stamp) -> Vec<u32> {
        stamp.cells.iter().map(|(_, t)| *t).collect()
    }

    fn address(tile: u32) -> TileAddress {
        TileAddress { sheet: 0, tile }
    }

    fn terrain(id: TerrainId, pure: &[u32]) -> ProposedTerrain {
        ProposedTerrain {
            id,
            background: false,
            pure_tiles: pure.iter().copied().map(address).collect(),
        }
    }

    /// Dirt (2) over grass (1) with tile 0 as its one transition tile.
    fn dirt_over_grass(terrains: Vec<ProposedTerrain>) -> TilesetProposal {
        let tile = FittedTile {
            address: address(0),
            terrains: TileTerrains::filled(1).with(crate::Peer::Centre, 2),
            confidence: [1.0; 9],
            corrected: [false; 9],
        };
        let set = ProposedSet {
            mode: MatchMode::Sides,
            inner: 2,
            outer: 1,
            tiles: vec![tile],
            covered: 1,
            total: 16,
            missing: Vec::new(),
            template: None,
        };
        TilesetProposal {
            sets: vec![set],
            terrains,
            ..TilesetProposal::default()
        }
    }

    fn analysed(sheet: &Sheet) -> TerrainAnalysis {
        let labels = TileLabels {
            kind: TileKind::Terrain,
            terrains: TileTerrains::filled(1),
            confidence: [1.0; 9],
        };
        TerrainAnalysis {
            terrains: Vec::new(),
            tiles: vec![vec![Some(labels); (sheet.columns * sheet.rows) as usize]],
        }
    }

    #[test]
    fn pure_tiles_of_set_terrains_are_never_stamps() {
        const GRASS: [u8; 4] = [70, 140, 70, 255];
        let sheet = Sheet::new(3, 1, |x, _| if x < T { WOOD } else { GRASS });
        let found = analysed(&sheet);
        let in_set = dirt_over_grass(vec![terrain(1, &[1, 2]), terrain(2, &[])]);
        let stamps = find_stamps(&found, &[sheet.input()], &in_set).expect("fits");
        assert!(stamps.is_empty());
        let mut alone = in_set.clone();
        alone.sets.clear();
        let stamps = find_stamps(&found, &[sheet.input()], &alone).expect("fits");
        assert_eq!(stamps.len(), 1);
        assert_eq!(cells(&stamps[0]), [1, 2]);
    }

    #[test]
    fn reconcile_takes_stamp_tiles_out_and_drops_emptied_terrains() {
        let mut proposal = dirt_over_grass(vec![
            terrain(1, &[]),
            terrain(2, &[]),
            terrain(3, &[0, 1, 3]),
            terrain(4, &[4, 5]),
        ]);
        proposal.unassigned = vec![address(6), address(7), address(8)];
        proposal.low_confidence = vec![address(6), address(9)];
        let stamp = |tiles: &[u32]| Stamp {
            sheet: 0,
            origin: UVec2::ZERO,
            size: UVec2::ONE,
            cells: tiles.iter().map(|t| (UVec2::ZERO, *t)).collect(),
        };
        let stamps = [stamp(&[0, 1]), stamp(&[4, 5]), stamp(&[6, 7])];
        let reconciled = reconcile(proposal, &stamps);
        let kept: Vec<_> = (reconciled.terrains.iter())
            .map(|t| {
                (
                    t.id,
                    t.pure_tiles.iter().map(|a| a.tile).collect::<Vec<_>>(),
                )
            })
            .collect();
        assert_eq!(kept, [(1, vec![]), (2, vec![]), (3, vec![3])]);
        assert_eq!(reconciled.unassigned, [address(8)]);
        assert_eq!(reconciled.low_confidence, [address(9)]);
    }

    #[test]
    fn object_across_two_tiles_is_one_stamp() {
        let sheet = Sheet::new(3, 1, |x, y| {
            let long = boxed(x, y, UVec2::new(2, 2), UVec2::new(30, 14), WOOD);
            let small = boxed(x, y, UVec2::new(36, 4), UVec2::new(44, 12), WOOD);
            long.or(small).unwrap_or([0; 4])
        });
        let found = stamps(&sheet, &all(&sheet));
        assert_eq!(
            found,
            vec![Stamp {
                sheet: 0,
                origin: UVec2::ZERO,
                size: UVec2::new(2, 1),
                cells: vec![(UVec2::ZERO, 0), (UVec2::X, 1)],
            }]
        );
    }

    #[test]
    fn outlined_objects_drawn_against_each_other_split() {
        let sheet = Sheet::new(2, 2, |x, y| {
            let cell = UVec2::new(x / T, y / T) * T;
            boxed(x, y, cell, cell + T, WOOD).unwrap_or([0; 4])
        });
        assert!(stamps(&sheet, &all(&sheet)).is_empty());
    }

    #[test]
    fn faces_under_one_ground_strip_split() {
        const GRASS: [u8; 4] = [70, 140, 70, 255];
        const SLATE: [u8; 4] = [50, 50, 70, 255];
        const STONE: [u8; 4] = [130, 140, 170, 255];
        let sheet = Sheet::new(2, 1, |x, y| {
            let (px, right) = (x % T, x >= T);
            let (outline, fill) = if right { (SLATE, STONE) } else { (DARK, WOOD) };
            if y < 4 {
                GRASS
            } else if px == 0 || px == T - 1 {
                outline
            } else {
                fill
            }
        });
        assert!(stamps(&sheet, &all(&sheet)).is_empty());
    }

    #[test]
    fn narrow_part_joins_a_wide_one() {
        let sheet = Sheet::new(2, 1, |x, y| {
            let post = x < T + 4 && (6..10).contains(&y);
            if x < T || post { WOOD } else { [0; 4] }
        });
        assert_eq!(stamps(&sheet, &all(&sheet)).len(), 1);
    }

    #[test]
    fn line_along_the_edge_inside_an_object_joins() {
        let sheet = Sheet::new(1, 2, |_, y| if y == T - 1 { DARK } else { WOOD });
        assert_eq!(stamps(&sheet, &all(&sheet)).len(), 1);
    }

    #[test]
    fn colour_break_at_the_edge_splits() {
        let sheet = Sheet::new(2, 1, |x, _| if x < T { WOOD } else { SKY });
        assert!(stamps(&sheet, &all(&sheet)).is_empty());
    }

    #[test]
    fn untouching_tiles_split() {
        let sheet = Sheet::new(2, 1, |x, y| {
            let left = boxed(x, y, UVec2::new(2, 2), UVec2::new(14, 14), WOOD);
            let right = boxed(x, y, UVec2::new(18, 2), UVec2::new(30, 14), WOOD);
            left.or(right).unwrap_or([0; 4])
        });
        assert!(stamps(&sheet, &all(&sheet)).is_empty());
    }

    #[test]
    fn gaps_stay_out_of_cells() {
        // An L over tiles 0, 2 and 3 of a 2x2 sheet; tile 1 is empty.
        let sheet = Sheet::new(2, 2, |x, y| {
            let upright = boxed(x, y, UVec2::new(4, 4), UVec2::new(12, 32), WOOD);
            let foot = boxed(x, y, UVec2::new(4, 20), UVec2::new(28, 28), WOOD);
            upright.or(foot).unwrap_or([0; 4])
        });
        let found = stamps(&sheet, &all(&sheet));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].size, UVec2::new(2, 2));
        let offsets: Vec<_> = found[0].cells.iter().map(|(o, _)| *o).collect();
        assert_eq!(offsets, [UVec2::ZERO, UVec2::Y, UVec2::ONE]);
        assert_eq!(cells(&found[0]), [0, 2, 3]);
    }

    #[test]
    fn only_candidates_join() {
        let sheet = Sheet::new(3, 1, |x, y| {
            boxed(x, y, UVec2::new(2, 2), UVec2::new(46, 14), WOOD).unwrap_or([0; 4])
        });
        let first_two = [0, 1].map(|tile| TileAddress { sheet: 0, tile });
        let found = stamps(&sheet, &first_two.into_iter().collect());
        assert_eq!(found.len(), 1);
        assert_eq!(cells(&found[0]), [0, 1]);
    }

    #[test]
    fn stamps_order_by_sheet_then_origin_row_then_column() {
        let sheet = Sheet::new(4, 2, |x, y| {
            let low = boxed(x, y, UVec2::new(2, 18), UVec2::new(30, 30), WOOD);
            let high = boxed(x, y, UVec2::new(34, 2), UVec2::new(62, 14), WOOD);
            low.or(high).unwrap_or([0; 4])
        });
        let found = stamps(&sheet, &all(&sheet));
        let origins: Vec<_> = found.iter().map(|s| s.origin).collect();
        assert_eq!(origins, [UVec2::new(2, 0), UVec2::new(0, 1)]);
    }
}
