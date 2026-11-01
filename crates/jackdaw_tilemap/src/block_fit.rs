//! Block fitting: orders a block of tiles into a layout template's slots, in
//! whatever order the sheet draws them.
//!
//! - Evidence: each border peer of a tile is compared with each terrain's
//!   look by the seam (as in layout fitting) between the tile's edges the peer
//!   touches and the look's opposite edges. A peer's error for a terrain is
//!   that terrain's share of the two seams; its confidence is the gap.
//! - Order: slots take block cells by the least total error over the peers
//!   each slot's mode reads. The template's own order stays unless the
//!   fitted one is lower by more than [`CANONICAL_SLACK`] per tile, or moves
//!   a tile its slot confidently refuses into one that confidently takes it.
//! - Blank: a tile with no opaque edge pixel errs fully on every peer.
//! - Unsure: a tile with a read peer that disagrees with its slot, or decides
//!   by less than [`UNSURE`].

use glam::UVec2;

use crate::layout_fit::{BORDER, Edges, reads, seam, segment, tile_edges, touches};
use crate::{LayoutTemplate, Peer, Placement, SheetInput};

/// Peer confidence below which a tile is unsure.
const UNSURE: f32 = 0.2;
/// Error per tile the fitted order must save over the template's own to
/// replace it.
const CANONICAL_SLACK: f32 = 0.05;
/// Share of sure tiles a good fit has.
const SURE_SHARE: f32 = 0.75;

/// How a terrain looks at a tile's edges: those of one of its plain tiles,
/// or transparent for no terrain.
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainLook(Edges);

impl TerrainLook {
    pub fn of_tile(input: &SheetInput, tile: u32) -> Option<Self> {
        tile_edges(input, tile).map(Self)
    }

    pub fn empty(tile: UVec2) -> Self {
        let (across, down) = (tile.x as usize, tile.y as usize);
        Self([across, down, down, across].map(|n| vec![None; n]))
    }

    fn size(&self) -> (usize, usize) {
        (self.0[0].len(), self.0[1].len())
    }
}

/// A block's tiles ordered into a template's slots.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockFit {
    /// The block cell, row by row, each template tile takes.
    pub order: Vec<Option<u32>>,
    /// Per template tile, from 0 to 1.
    pub confidence: Vec<f32>,
    /// Cells whose template tile is in doubt.
    pub unsure: Vec<u32>,
    /// Whether `order` is the template's own.
    pub canonical: bool,
}

impl BlockFit {
    /// Whether most tiles are sure.
    pub fn good(&self) -> bool {
        let placed = self.order.iter().flatten().count();
        let sure = placed.saturating_sub(self.unsure.len());
        placed > 0 && sure as f32 >= SURE_SHARE * placed as f32
    }
}

/// Each cell's peer errors for the inner and the outer look, measured once
/// for a block and costed for any template.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockEvidence {
    size: UVec2,
    cells: Vec<[[f32; 2]; 9]>,
}

impl BlockEvidence {
    /// The evidence of the `size` block of `input` at `origin`; `None` when
    /// the block leaves the sheet or the looks are not of its tile size.
    pub fn measure(
        input: &SheetInput,
        (origin, size): (UVec2, UVec2),
        (inner, outer): (&TerrainLook, &TerrainLook),
    ) -> Option<Self> {
        let tile = input.layout.tile;
        let inside = (origin + size).cmple(UVec2::new(input.layout.columns, input.layout.rows));
        let sized = [inner, outer]
            .iter()
            .all(|l| l.size() == (tile.x as usize, tile.y as usize));
        if !inside.all() || !sized {
            return None;
        }
        let cells = (0..size.y)
            .flat_map(|y| (0..size.x).map(move |x| origin + UVec2::new(x, y)))
            .map(|c| errors(input, c.y * input.layout.columns + c.x, [inner, outer]))
            .collect();
        Some(Self { size, cells })
    }

    /// Fits `template` read at `placement`; `None` when it does not fit the
    /// block.
    pub fn fit(&self, (template, placement): (LayoutTemplate, Placement)) -> Option<BlockFit> {
        if !template.fits(self.size) || !template.placements().contains(&placement) {
            return None;
        }
        let mode = template.mode_at(placement);
        let shapes = template.placed_tiles(placement, 2, 1);
        let read: Vec<Vec<(Peer, usize)>> = (shapes.iter())
            .map(|s| {
                let peers = BORDER.into_iter().filter(|p| reads(mode, s, *p));
                peers.map(|p| (p, usize::from(s.get(p) != 2))).collect()
            })
            .collect();
        let cost: Vec<Vec<f32>> = (read.iter())
            .map(|peers| {
                let error = |e: &[[f32; 2]; 9]| peers.iter().map(|(p, t)| e[p.index()][*t]).sum();
                self.cells.iter().map(error).collect()
            })
            .collect();
        let total = |order: &[Option<u32>]| -> f32 {
            let taken = order.iter().enumerate();
            taken
                .filter_map(|(s, c)| Some(cost[s][(*c)? as usize]))
                .sum()
        };
        let score = |slot: usize, cell: Option<u32>| self.score(&read[slot], cell);
        let fitted: Vec<Option<u32>> = assign(&cost, self.cells.len())
            .into_iter()
            .map(Some)
            .collect();
        let canonical_order = template.canonical_order(self.size);
        let slack = CANONICAL_SLACK * read.len() as f32;
        let confident = (canonical_order.iter().zip(&fitted).enumerate())
            .filter(|(_, (was, now))| was != now)
            .any(|(s, (was, now))| score(s, *was) <= -UNSURE && score(s, *now) >= UNSURE);
        let canonical = !confident && total(&fitted) + slack >= total(&canonical_order);
        let order = if canonical { canonical_order } else { fitted };
        let confidence: Vec<f32> = (order.iter().enumerate())
            .map(|(s, cell)| score(s, *cell).max(0.0))
            .collect();
        let unsure = (order.iter().zip(&confidence))
            .filter(|(_, c)| **c < UNSURE)
            .filter_map(|(cell, _)| *cell)
            .collect();
        Some(BlockFit {
            order,
            confidence,
            unsure,
            canonical,
        })
    }

    /// How far `cell` decides each of `peers` for its slot's terrain, the
    /// least of them: negative when one disagrees, -1 with no cell.
    fn score(&self, peers: &[(Peer, usize)], cell: Option<u32>) -> f32 {
        let Some(e) = cell.and_then(|c| self.cells.get(c as usize)) else {
            return -1.0;
        };
        (peers.iter())
            .map(|(p, t)| e[p.index()][1 - *t] - e[p.index()][*t])
            .fold(1.0, f32::min)
    }
}

/// Fits `template` read at `placement` to the `size` block of `input` at
/// `origin`; see [`BlockEvidence`].
pub fn fit_block(
    input: &SheetInput,
    block: (UVec2, UVec2),
    layout: (LayoutTemplate, Placement),
    looks: (&TerrainLook, &TerrainLook),
) -> Option<BlockFit> {
    BlockEvidence::measure(input, block, looks)?.fit(layout)
}

/// Each peer's error for the inner and the outer look, see the module docs;
/// a blank tile errs on both, so no slot takes it while a drawn tile is left.
fn errors(input: &SheetInput, tile: u32, looks: [&TerrainLook; 2]) -> [[f32; 2]; 9] {
    let mut errors = [[0.5; 2]; 9];
    let Some(own) = tile_edges(input, tile) else {
        return errors;
    };
    if own.iter().flatten().all(Option::is_none) {
        return [[1.0; 2]; 9];
    }
    let (w, h) = (own[0].len() as u32, own[1].len() as u32);
    for peer in BORDER {
        let seams = looks.map(|look| {
            let sides = Peer::SIDES.into_iter().enumerate();
            let touching = sides.filter(|(_, side)| touches(peer, *side));
            let seams: Vec<f32> = touching
                .map(|(slot, side)| {
                    let theirs = &look.0[Peer::SIDES.len() - 1 - slot];
                    seam(&own[slot], theirs, segment(peer, side, w, h)).unwrap_or(0.0)
                })
                .collect();
            seams.iter().sum::<f32>() / seams.len().max(1) as f32
        });
        let sum = seams[0] + seams[1];
        if sum > f32::EPSILON {
            errors[peer.index()] = seams.map(|s| s / sum);
        }
    }
    errors
}

/// The column each row takes for the least total cost, every row a distinct
/// column; `columns` is at least the row count.
fn assign(cost: &[Vec<f32>], columns: usize) -> Vec<u32> {
    let rows = cost.len();
    let (mut u, mut v) = (vec![0.0f64; rows + 1], vec![0.0f64; columns + 1]);
    let (mut owner, mut way) = (vec![0usize; columns + 1], vec![0usize; columns + 1]);
    for row in 1..=rows {
        owner[0] = row;
        let mut free = 0;
        let mut least = vec![f64::INFINITY; columns + 1];
        let mut used = vec![false; columns + 1];
        loop {
            used[free] = true;
            let at = owner[free];
            let (mut delta, mut next) = (f64::INFINITY, 0);
            for j in 1..=columns {
                if used[j] {
                    continue;
                }
                let reduced = f64::from(cost[at - 1][j - 1]) - u[at] - v[j];
                if reduced < least[j] {
                    least[j] = reduced;
                    way[j] = free;
                }
                if least[j] < delta {
                    delta = least[j];
                    next = j;
                }
            }
            for j in 0..=columns {
                if used[j] {
                    u[owner[j]] += delta;
                    v[j] -= delta;
                } else {
                    least[j] -= delta;
                }
            }
            free = next;
            if owner[free] == 0 {
                break;
            }
        }
        while free != 0 {
            let back = way[free];
            owner[free] = owner[back];
            free = back;
        }
    }
    let mut taken = vec![0; rows];
    for (column, row) in owner.iter().enumerate().skip(1) {
        if *row != 0 {
            taken[row - 1] = (column - 1) as u32;
        }
    }
    taken
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GridLayout, RgbaImage, TileTerrains};

    const TILE: u32 = 16;
    const INNER: [u8; 3] = [200, 170, 90];
    const OUTER: [u8; 3] = [80, 140, 70];

    /// A sheet of `size` tiles, cell `i` drawing `tiles[i]`: corners as
    /// quadrants between cells, otherwise peers as thirds; slight noise
    /// stands in for texture.
    struct Art {
        size: UVec2,
        pixels: Vec<u8>,
    }

    impl Art {
        fn new(size: UVec2, placement: Placement, tiles: &[Option<TileTerrains>]) -> Self {
            let (w, h) = (size.x * TILE, size.y * TILE);
            let mut pixels = vec![0; (w * h * 4) as usize];
            for (i, tile) in tiles.iter().enumerate() {
                let Some(tile) = tile else { continue };
                let cell = UVec2::new(i as u32 % size.x, i as u32 / size.x) * TILE;
                for y in 0..TILE {
                    for x in 0..TILE {
                        let peer = match placement {
                            Placement::BetweenCells => (y / 8) * 6 + (x / 8) * 2,
                            Placement::OnCells => (y * 3 / TILE) * 3 + x * 3 / TILE,
                        };
                        let colour = if tile.peers[peer as usize] == 2 {
                            INNER
                        } else {
                            OUTER
                        };
                        let noise = ((x * 7 + y * 13 + i as u32 * 5) % 9) as u8;
                        let at = (((cell.y + y) * w + cell.x + x) * 4) as usize;
                        let rgb = colour.map(|c| c.saturating_add(noise));
                        pixels[at..at + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
                    }
                }
            }
            Self { size, pixels }
        }

        fn input(&self) -> SheetInput<'_> {
            let (w, h) = (self.size.x * TILE, self.size.y * TILE);
            SheetInput {
                name: "art",
                image: RgbaImage::new(w, h, &self.pixels).expect("sized"),
                layout: GridLayout {
                    tile: UVec2::splat(TILE),
                    margin: UVec2::ZERO,
                    spacing: UVec2::ZERO,
                    columns: self.size.x,
                    rows: self.size.y,
                },
            }
        }
    }

    /// A sheet of the template drawn with tile `i` on cell `cells[i]`, then
    /// a plain inner and a plain outer tile on the row below.
    fn shuffled(layout: (LayoutTemplate, Placement), size: UVec2, cells: &[u32]) -> Art {
        let (template, placement) = layout;
        let rows = size.y + 1;
        let mut tiles = vec![None; (size.x * rows) as usize];
        for (shape, cell) in template
            .placed_tiles(placement, 2, 1)
            .into_iter()
            .zip(cells)
        {
            tiles[*cell as usize] = Some(shape);
        }
        let below = (size.x * size.y) as usize;
        tiles[below] = Some(TileTerrains::filled(2));
        tiles[below + 1] = Some(TileTerrains::filled(1));
        Art::new(UVec2::new(size.x, rows), placement, &tiles)
    }

    fn looks(art: &Art, size: UVec2) -> [TerrainLook; 2] {
        let below = size.x * size.y;
        let input = art.input();
        [below, below + 1].map(|t| TerrainLook::of_tile(&input, t).expect("a tile"))
    }

    /// A fixed shuffle of `0..n` cells of `area`.
    fn permutation(n: usize, area: u32) -> Vec<u32> {
        let mut cells: Vec<u32> = (0..area).collect();
        let mut seed = 0x2545_f491_u32;
        for i in (1..cells.len()).rev() {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            cells.swap(i, (seed >> 8) as usize % (i + 1));
        }
        cells.truncate(n);
        cells
    }

    fn fit(art: &Art, layout: (LayoutTemplate, Placement), size: UVec2) -> BlockFit {
        let [inner, outer] = looks(art, size);
        fit_block(&art.input(), (UVec2::ZERO, size), layout, (&inner, &outer)).expect("fits")
    }

    #[test]
    fn a_shuffled_16_corner_block_is_recovered_exactly() {
        let layout = (LayoutTemplate::Corner16, Placement::BetweenCells);
        let size = UVec2::splat(4);
        let cells = permutation(16, 16);
        let fit = fit(&shuffled(layout, size, &cells), layout, size);
        let expected: Vec<Option<u32>> = cells.iter().map(|c| Some(*c)).collect();
        assert_eq!(fit.order, expected);
        assert!(fit.unsure.is_empty(), "{:?}", fit.unsure);
        assert!(!fit.canonical && fit.good());
    }

    #[test]
    fn a_shuffled_47_blob_block_is_recovered_exactly() {
        let layout = (LayoutTemplate::Blob47, Placement::OnCells);
        let size = UVec2::new(8, 6);
        let cells = permutation(47, 48);
        let fit = fit(&shuffled(layout, size, &cells), layout, size);
        let expected: Vec<Option<u32>> = cells.iter().map(|c| Some(*c)).collect();
        assert_eq!(fit.order, expected);
        assert!(fit.unsure.is_empty(), "{:?}", fit.unsure);
    }

    #[test]
    fn a_block_in_the_templates_own_order_keeps_it() {
        let layout = (LayoutTemplate::Minimal3x3, Placement::OnCells);
        let size = UVec2::splat(3);
        let cells: Vec<u32> = (0..9).collect();
        let fit = fit(&shuffled(layout, size, &cells), layout, size);
        assert!(fit.canonical);
        assert_eq!(fit.order, LayoutTemplate::Minimal3x3.canonical_order(size));
    }

    #[test]
    fn noisy_tiles_are_unsure_and_the_rest_are_placed() {
        let layout = (LayoutTemplate::Corner16, Placement::BetweenCells);
        let size = UVec2::splat(4);
        let cells = permutation(16, 16);
        let mut art = shuffled(layout, size, &cells);
        let width = (size.x * TILE * 4) as usize;
        for cell in [cells[5], cells[9]] {
            let at = UVec2::new(cell % size.x, cell / size.x) * TILE;
            for y in at.y..at.y + TILE {
                for x in at.x..at.x + TILE {
                    let i = y as usize * width + x as usize * 4;
                    art.pixels[i..i + 3].copy_from_slice(&[140, 155, 80]);
                }
            }
        }
        let fit = fit(&art, layout, size);
        let mut unsure = fit.unsure.clone();
        unsure.sort_unstable();
        let mut noisy = vec![cells[5], cells[9]];
        noisy.sort_unstable();
        assert_eq!(unsure, noisy);
        for (slot, cell) in cells
            .iter()
            .enumerate()
            .filter(|(s, _)| ![5, 9].contains(s))
        {
            assert_eq!(fit.order[slot], Some(*cell), "slot {slot}");
        }
    }

    #[test]
    fn two_swapped_tiles_in_a_canonical_47_block_are_put_back() {
        let layout = (LayoutTemplate::Blob47, Placement::OnCells);
        let size = UVec2::new(8, 6);
        let mut cells: Vec<u32> = (0..47).collect();
        cells.swap(3, 30);
        let fit = fit(&shuffled(layout, size, &cells), layout, size);
        let expected: Vec<Option<u32>> = cells.iter().map(|c| Some(*c)).collect();
        assert_eq!(fit.order, expected);
        assert!(!fit.canonical && fit.unsure.is_empty());
    }

    #[test]
    fn a_block_too_small_for_the_template_does_not_fit() {
        let layout = (LayoutTemplate::Blob47, Placement::OnCells);
        let art = shuffled(layout, UVec2::new(8, 6), &permutation(47, 48));
        let [inner, outer] = looks(&art, UVec2::new(8, 6));
        let small = (UVec2::ZERO, UVec2::splat(4));
        assert_eq!(
            fit_block(&art.input(), small, layout, (&inner, &outer)),
            None
        );
    }
}
