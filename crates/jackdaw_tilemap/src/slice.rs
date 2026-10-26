//! Tileset slicing over row-major RGBA8 bytes: grid detection and empty
//! cells. Pixels with alpha 0 compare equal regardless of colour.
//!
//! Grid detection works per axis. A candidate is a common tile size with a
//! leading margin in `0..=tile/2`, a spacing in `0..=4` and the fewest cells
//! after which the rest of the axis (the trailing padding) is one uniform
//! colour. Margin and spacing lines must share one uniform colour. Scores lie
//! in `0..=1`:
//! - with spacing, the share of content lines that are not uniform;
//! - without spacing, `0.9 * (1 - inner / boundary)` over the cells: the mean
//!   colour step across cell boundaries against the mean step between other
//!   neighbouring lines. For two sizes where one divides the other, the
//!   smaller size's extra lines are edges when their step is at least 1.5
//!   times its inner step and 0.3 times the larger size's boundary step, or
//!   when they carry at least half of the larger size's boundary excess over
//!   the inner step. If they are edges the larger size is halved, otherwise
//!   the smaller one is;
//! - a single cell scores 0.25 times its share of non-uniform lines, and a
//!   one-cell layout is returned only when nothing else scores.
//!
//! A spaced reading with `spacing == 2 * margin` whose pitch is a common size
//! is reported as packed cells of that pitch, padding included.
//!
//! A layout scores the mean of its axes and needs both above zero. A
//! non-square layout is considered when no square one fits, when both axes
//! have spacing, or when on each axis the pair test above favours that axis's
//! size over the other's. A square layout within 0.05 of the best wins. The
//! confidence is the chosen score less half the next one's. Tile ids are grid
//! positions (`row * columns + column`); empty cells are flagged, never
//! renumbered.

use glam::UVec2;

pub const COMMON_TILE_SIZES: [u32; 8] = [8, 12, 16, 24, 32, 48, 64, 128];
const MAX_SPACING: u32 = 4;
const MAX_ALTERNATIVES: usize = 4;
const GUTTERLESS_WEIGHT: f32 = 0.9;
const SINGLE_CELL_SCORE: f32 = 0.25;
const EDGE_RATIO: f32 = 1.5;
const EDGE_SHARE: f32 = 0.3;
const EDGE_EXCESS: f32 = 0.5;
const SQUARE_MARGIN: f32 = 0.05;

type Rgba = [u8; 4];

pub(crate) fn canonical(p: &[u8]) -> Rgba {
    if p[3] == 0 {
        [0; 4]
    } else {
        [p[0], p[1], p[2], p[3]]
    }
}

fn colour_step(a: Rgba, b: Rgba) -> u32 {
    (0..4).map(|i| a[i].abs_diff(b[i]) as u32).sum()
}

/// Borrowed row-major RGBA8 pixels.
#[derive(Clone, Copy, Debug)]
pub struct RgbaImage<'a> {
    pub width: u32,
    pub height: u32,
    pub pixels: &'a [u8],
}

impl<'a> RgbaImage<'a> {
    /// `None` when `pixels` is not exactly `width * height * 4` bytes.
    pub fn new(width: u32, height: u32, pixels: &'a [u8]) -> Option<Self> {
        let len = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        (pixels.len() == len).then_some(Self {
            width,
            height,
            pixels,
        })
    }

    /// The pixel at `(x, y)` with fully transparent pixels mapped to `[0; 4]`.
    pub fn pixel(&self, x: u32, y: u32) -> Option<Rgba> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let at = (y as usize * self.width as usize + x as usize) * 4;
        self.pixels.get(at..at + 4).map(canonical)
    }

    pub(crate) fn rows(&self) -> impl Iterator<Item = &'a [u8]> {
        self.pixels.chunks_exact((self.width as usize * 4).max(4))
    }

    fn pixels_in(&self, origin: UVec2, size: UVec2) -> impl Iterator<Item = Option<Rgba>> + '_ {
        (origin.y..origin.y.saturating_add(size.y)).flat_map(move |y| {
            (origin.x..origin.x.saturating_add(size.x)).map(move |x| self.pixel(x, y))
        })
    }
}

/// A uniform tile grid: `margin` is the offset of the first cell, `spacing`
/// the gap between neighbouring cells. Anything past the last cell is
/// padding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GridLayout {
    pub tile: UVec2,
    pub margin: UVec2,
    pub spacing: UVec2,
    pub columns: u32,
    pub rows: u32,
}

impl GridLayout {
    pub fn tile_count(&self) -> u32 {
        self.columns.saturating_mul(self.rows)
    }

    /// Pixel origin and size of tile `index = row * columns + column`.
    pub fn tile_rect(&self, index: u32) -> (UVec2, UVec2) {
        let columns = self.columns.max(1);
        let cell = UVec2::new(index % columns, index / columns);
        (self.margin + cell * (self.tile + self.spacing), self.tile)
    }

    /// The far corner of the last cell.
    pub fn extent(&self) -> UVec2 {
        let cells = UVec2::new(self.columns, self.rows);
        self.margin + cells * self.tile + cells.saturating_sub(UVec2::ONE) * self.spacing
    }

    /// Whether the layout has cells and all of them lie inside `size`.
    pub fn fits(&self, size: UVec2) -> bool {
        let cells = UVec2::new(self.columns, self.rows).as_u64vec2();
        let extent = self.margin.as_u64vec2()
            + cells * self.tile.as_u64vec2()
            + cells.saturating_sub(glam::U64Vec2::ONE) * self.spacing.as_u64vec2();
        self.tile_count() > 0 && extent.cmple(size.as_u64vec2()).all()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GridDetection {
    pub layout: GridLayout,
    /// `0..=1`: the chosen score less half the next one's.
    pub confidence: f32,
    /// Runners-up with their scores, best first.
    pub alternatives: Vec<(GridLayout, f32)>,
}

#[derive(Clone, Copy, Debug)]
struct AxisCandidate {
    tile: u32,
    margin: u32,
    spacing: u32,
    count: u32,
}

impl AxisCandidate {
    fn end(&self) -> u32 {
        self.margin + self.count * self.tile + (self.count - 1) * self.spacing
    }
}

/// A candidate as reported (`cand`) and as scored (`source`); they differ
/// when a padded spaced reading is reported as packed cells.
#[derive(Clone, Copy, Debug)]
struct Axis {
    cand: AxisCandidate,
    source: AxisCandidate,
}

/// Per-line statistics along one axis: `step[k]` is the mean colour step
/// between line `k - 1` and line `k`, `uniform[k]` the colour of line `k`
/// when all its pixels match.
struct Profile {
    step: Vec<f32>,
    uniform: Vec<Option<Rgba>>,
}

/// Column and row profiles from one row-major pass.
fn profiles(image: &RgbaImage) -> [Profile; 2] {
    let (w, h) = (image.width as usize, image.height as usize);
    let mut col_step = vec![0u64; w];
    let mut row_step = vec![0u64; h];
    let mut col_uniform: Vec<Option<Rgba>> = vec![None; w];
    let mut row_uniform = Vec::with_capacity(h);
    let mut above: Vec<Rgba> = vec![[0; 4]; w];
    for (y, row) in image.rows().enumerate() {
        let mut row_sum = 0u64;
        let mut left = None;
        let mut row_colour = None;
        let mut row_same = true;
        let columns = col_step
            .iter_mut()
            .zip(col_uniform.iter_mut())
            .zip(above.iter_mut());
        for (px, ((cs, cu), up)) in row.as_chunks::<4>().0.iter().zip(columns) {
            let p = canonical(px);
            match left {
                Some(l) => {
                    *cs += u64::from(colour_step(l, p));
                    row_same &= row_colour == Some(p);
                }
                None => row_colour = Some(p),
            }
            if y == 0 {
                *cu = Some(p);
            } else {
                row_sum += u64::from(colour_step(*up, p));
                if *cu != Some(p) {
                    *cu = None;
                }
            }
            *up = p;
            left = Some(p);
        }
        if let Some(rs) = row_step.get_mut(y) {
            *rs = row_sum;
        }
        row_uniform.push(row_colour.filter(|_| row_same));
    }
    let mean = |sums: Vec<u64>, across: usize| -> Vec<f32> {
        sums.into_iter()
            .map(|s| s as f32 / across.max(1) as f32)
            .collect()
    };
    [
        Profile {
            step: mean(col_step, h),
            uniform: col_uniform,
        },
        Profile {
            step: mean(row_step, w),
            uniform: row_uniform,
        },
    ]
}

impl Profile {
    fn len(&self) -> u32 {
        self.step.len() as u32
    }

    fn mean(&self, lines: impl Iterator<Item = u32>) -> f32 {
        let (sum, n) = lines.fold((0.0, 0u32), |(s, n), k| (s + self.step[k as usize], n + 1));
        if n == 0 { 0.0 } else { sum / n as f32 }
    }

    /// Mean steps over `[start, end)` at multiples of `tile` from `start`
    /// (boundaries) and at the other lines past `start` (inner).
    fn grid_steps(&self, start: u32, end: u32, tile: u32) -> (f32, f32) {
        let boundary = self.mean((start + tile..end).step_by(tile as usize));
        let inner = self.mean((start + 1..end).filter(|k| !(k - start).is_multiple_of(tile)));
        (boundary, inner)
    }

    fn sharpness(&self, start: u32, end: u32, tile: u32) -> f32 {
        let (boundary, inner) = self.grid_steps(start, end, tile);
        if boundary <= 0.0 {
            0.0
        } else {
            (1.0 - inner / boundary).max(0.0)
        }
    }

    /// Whether the lines of the `small` grid over `[start, end)` that are not
    /// on the `large` grid are real edges.
    fn extra_lines_are_edges(&self, start: u32, end: u32, small: u32, large: u32) -> bool {
        let extra = self.mean(
            (start + small..end)
                .step_by(small as usize)
                .filter(|k| !(k - start).is_multiple_of(large)),
        );
        let (_, inner) = self.grid_steps(start, end, small);
        let (boundary, _) = self.grid_steps(start, end, large);
        let strong = extra >= EDGE_RATIO * inner && extra >= EDGE_SHARE * boundary;
        let excess = boundary > inner && extra - inner >= EDGE_EXCESS * (boundary - inner);
        extra > 0.0 && (strong || excess)
    }

    /// The first line from which every line to the end is uniform in one
    /// colour.
    fn padding_start(&self) -> u32 {
        let Some(Some(last)) = self.uniform.last() else {
            return self.len();
        };
        let run = self
            .uniform
            .iter()
            .rev()
            .take_while(|u| **u == Some(*last))
            .count();
        self.len() - run as u32
    }

    /// Margin and spacing lines are uniform in one shared colour.
    fn gutters_uniform(&self, c: &AxisCandidate) -> bool {
        let pitch = c.tile + c.spacing;
        let spacing = (1..c.count).flat_map(|i| {
            let from = c.margin + i * pitch - c.spacing;
            from..from + c.spacing
        });
        let mut lines = (0..c.margin)
            .chain(spacing)
            .map(|k| self.uniform[k as usize]);
        let Some(first) = lines.next() else {
            return true;
        };
        first.is_some() && lines.all(|l| l == first)
    }

    fn content_variety(&self, c: &AxisCandidate) -> f32 {
        let pitch = c.tile + c.spacing;
        let lines = (c.margin..c.end()).filter(|k| (k - c.margin) % pitch < c.tile);
        let (uniform, total) = lines.fold((0u32, 0u32), |(u, t), k| {
            (u + u32::from(self.uniform[k as usize].is_some()), t + 1)
        });
        if total == 0 {
            0.0
        } else {
            1.0 - uniform as f32 / total as f32
        }
    }

    fn candidates(&self) -> Vec<Axis> {
        let len = self.len();
        let pad = self.padding_start();
        let mut out = Vec::new();
        for tile in COMMON_TILE_SIZES {
            for spacing in 0..=MAX_SPACING {
                for margin in 0..=tile / 2 {
                    let pitch = tile + spacing;
                    let needed = pad.saturating_sub(margin) + spacing;
                    let min_count = if spacing > 0 { 2 } else { 1 };
                    let count = needed.div_ceil(pitch).max(min_count);
                    let cand = AxisCandidate {
                        tile,
                        margin,
                        spacing,
                        count,
                    };
                    if cand.end() > len || !self.gutters_uniform(&cand) {
                        continue;
                    }
                    let padded = spacing == 2 * margin
                        && spacing > 0
                        && len - cand.end() >= margin
                        && COMMON_TILE_SIZES.contains(&pitch);
                    let reported = if padded {
                        AxisCandidate {
                            tile: pitch,
                            margin: 0,
                            spacing: 0,
                            count,
                        }
                    } else {
                        cand
                    };
                    out.push(Axis {
                        cand: reported,
                        source: cand,
                    });
                }
            }
        }
        out
    }

    /// Whether this axis prefers `axis`'s tile size over `other`.
    fn supports(&self, axis: &Axis, other: u32) -> bool {
        let c = axis.source;
        if c.spacing > 0 {
            return true;
        }
        let (start, end) = (c.margin, c.end());
        let cells = end - start;
        let peer = |t: u32| cells.is_multiple_of(t) && cells / t >= 2;
        if other < c.tile && c.tile.is_multiple_of(other) && peer(other) {
            !self.extra_lines_are_edges(start, end, other, c.tile)
        } else if other > c.tile && other.is_multiple_of(c.tile) && peer(other) {
            self.extra_lines_are_edges(start, end, c.tile, other)
        } else {
            false
        }
    }

    /// `peers` are the tile sizes this axis may take in some layout.
    fn score(&self, axis: &Axis, peers: &[u32]) -> f32 {
        let c = axis.source;
        if c.spacing > 0 {
            return self.content_variety(&c);
        }
        if c.count < 2 {
            return SINGLE_CELL_SCORE * self.content_variety(&c);
        }
        let (start, end) = (c.margin, c.end());
        let cells = end - start;
        let mut score = GUTTERLESS_WEIGHT * self.sharpness(start, end, c.tile);
        let peers = peers
            .iter()
            .filter(|&&o| o != c.tile && cells.is_multiple_of(o) && cells / o >= 2);
        for &other in peers {
            let halve = if other < c.tile && c.tile.is_multiple_of(other) {
                self.extra_lines_are_edges(start, end, other, c.tile)
            } else if other > c.tile && other.is_multiple_of(c.tile) {
                !self.extra_lines_are_edges(start, end, c.tile, other)
            } else {
                false
            };
            if halve {
                score *= 0.5;
            }
        }
        score
    }
}

fn rank(image: &RgbaImage) -> Vec<(GridLayout, f32)> {
    let profiles = profiles(image);
    let axes = [profiles[0].candidates(), profiles[1].candidates()];
    let any_square = axes[0]
        .iter()
        .any(|x| axes[1].iter().any(|y| x.cand.tile == y.cand.tile));
    let mut pairs = Vec::new();
    for (i, x) in axes[0].iter().enumerate() {
        for (j, y) in axes[1].iter().enumerate() {
            let eligible = x.cand.tile == y.cand.tile
                || !any_square
                || (profiles[0].supports(x, y.cand.tile) && profiles[1].supports(y, x.cand.tile));
            if eligible {
                pairs.push((i, j));
            }
        }
    }
    let peers = |axis: usize| -> Vec<u32> {
        let mut tiles: Vec<u32> = pairs
            .iter()
            .map(|&(i, j)| if axis == 0 { i } else { j })
            .map(|k| axes[axis][k].source)
            .filter(|c| c.spacing == 0)
            .map(|c| c.tile)
            .collect();
        tiles.sort_unstable();
        tiles.dedup();
        tiles
    };
    let scores: Vec<Vec<f32>> = (0..2)
        .map(|a| {
            let peers = peers(a);
            axes[a]
                .iter()
                .map(|c| profiles[a].score(c, &peers))
                .collect()
        })
        .collect();
    let mut out = Vec::new();
    for (i, j) in pairs {
        let (sx, sy) = (scores[0][i], scores[1][j]);
        if sx > 0.0 && sy > 0.0 {
            let (x, y) = (axes[0][i].cand, axes[1][j].cand);
            let layout = GridLayout {
                tile: UVec2::new(x.tile, y.tile),
                margin: UVec2::new(x.margin, y.margin),
                spacing: UVec2::new(x.spacing, y.spacing),
                columns: x.count,
                rows: y.count,
            };
            out.push((layout, (sx + sy) / 2.0));
        }
    }
    out
}

/// Detect tile size, margin and spacing. `None` for an empty image or one
/// where no layout scores above zero.
pub fn detect_grid(image: &RgbaImage) -> Option<GridDetection> {
    if image.width == 0 || image.height == 0 {
        return None;
    }
    let mut ranked = rank(image);
    ranked.sort_by(|(a, sa), (b, sb)| {
        sb.total_cmp(sa)
            .then(b.tile.x.cmp(&a.tile.x))
            .then(b.tile.y.cmp(&a.tile.y))
            .then(a.margin.to_array().cmp(&b.margin.to_array()))
            .then(a.spacing.to_array().cmp(&b.spacing.to_array()))
    });
    let mut seen = Vec::new();
    ranked.retain(|(l, _)| {
        let fresh = !seen.contains(l);
        seen.push(*l);
        fresh
    });
    if ranked.iter().any(|(l, _)| l.tile_count() > 1) {
        ranked.retain(|(l, _)| l.tile_count() > 1);
    }
    let best = ranked.first()?.1;
    let square = ranked
        .iter()
        .position(|(l, s)| l.tile.x == l.tile.y && *s >= best - SQUARE_MARGIN);
    if let Some(at) = square {
        let chosen = ranked.remove(at);
        ranked.insert(0, chosen);
    }
    let mut ranked = ranked.into_iter();
    let (layout, score) = ranked.next()?;
    let alternatives: Vec<_> = ranked.take(MAX_ALTERNATIVES).collect();
    let next = alternatives.first().map_or(0.0, |(_, s)| *s);
    Some(GridDetection {
        layout,
        confidence: (score - 0.5 * next).clamp(0.0, 1.0),
        alternatives,
    })
}

/// One flag per tile id: `true` when every pixel of the tile has alpha 0.
/// Pixels outside the image count as transparent.
pub fn empty_tiles(image: &RgbaImage, layout: &GridLayout) -> Vec<bool> {
    (0..layout.tile_count())
        .map(|index| {
            let (origin, size) = layout.tile_rect(index);
            image
                .pixels_in(origin, size)
                .all(|p| p.is_none_or(|c| c[3] == 0))
        })
        .collect()
}

/// The colour of tile `index` when all its pixels match, `None` otherwise or
/// when the tile leaves the image.
pub fn uniform_tile(image: &RgbaImage, layout: &GridLayout, index: u32) -> Option<Rgba> {
    let (origin, size) = layout.tile_rect(index);
    let mut pixels = image.pixels_in(origin, size);
    let first = pixels.next()??;
    pixels.all(|p| p == Some(first)).then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Sheet {
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    }

    fn hash(a: u32, b: u32, c: u32) -> u32 {
        let mut h =
            a.wrapping_mul(0x9E37_79B1) ^ b.wrapping_mul(0x85EB_CA77) ^ c.wrapping_mul(0xC2B2_AE3D);
        h ^= h >> 15;
        h = h.wrapping_mul(0x2C1B_3C6D);
        h ^ (h >> 12)
    }

    impl Sheet {
        fn new(width: u32, height: u32, fill: [u8; 4]) -> Self {
            Self {
                width,
                height,
                pixels: fill.repeat((width * height) as usize),
            }
        }

        fn set(&mut self, x: u32, y: u32, c: [u8; 4]) {
            let at = ((y * self.width + x) * 4) as usize;
            self.pixels[at..at + 4].copy_from_slice(&c);
        }

        /// Fills each cell of `layout` with a colour per `block` of pixels
        /// (measured from the first cell) plus per-pixel noise. `spread` is
        /// the range of the block colours, `noise` the range of the noise.
        fn cells(&mut self, layout: &GridLayout, block: UVec2, seed: u32, spread: u32, noise: u32) {
            let origin = layout.tile_rect(0).0;
            for index in 0..layout.tile_count() {
                let (o, s) = layout.tile_rect(index);
                for y in o.y..o.y + s.y {
                    for x in o.x..o.x + s.x {
                        let b = (UVec2::new(x, y) - origin) / block;
                        let base = hash(b.x, b.y, seed);
                        let n = hash(x, y, seed + 1) % noise;
                        let channel = |shift: u32| (40 + (base >> shift) % spread + n) as u8;
                        self.set(x, y, [channel(0), channel(8), channel(16), 255]);
                    }
                }
            }
        }

        fn noisy_cells(&mut self, layout: &GridLayout, block: UVec2, seed: u32) {
            self.cells(layout, block, seed, 180, 12);
        }

        fn clear_tile(&mut self, layout: &GridLayout, index: u32) {
            let (o, s) = layout.tile_rect(index);
            for y in o.y..o.y + s.y {
                for x in o.x..o.x + s.x {
                    self.set(x, y, [0; 4]);
                }
            }
        }

        fn image(&self) -> RgbaImage<'_> {
            RgbaImage::new(self.width, self.height, &self.pixels).expect("sized")
        }
    }

    fn layout(tile: u32, margin: u32, spacing: u32, columns: u32, rows: u32) -> GridLayout {
        GridLayout {
            tile: UVec2::splat(tile),
            margin: UVec2::splat(margin),
            spacing: UVec2::splat(spacing),
            columns,
            rows,
        }
    }

    /// A sheet of `size` filled with `background` and noisy cells of `l`.
    fn sheet(size: UVec2, l: &GridLayout, background: [u8; 4], seed: u32) -> Sheet {
        let mut sheet = Sheet::new(size.x, size.y, background);
        sheet.noisy_cells(l, l.tile, seed);
        sheet
    }

    fn detect(sheet: &Sheet) -> GridDetection {
        detect_grid(&sheet.image()).expect("detected")
    }

    #[test]
    fn image_rejects_wrong_length() {
        assert!(RgbaImage::new(2, 2, &[0; 15]).is_none());
        assert!(RgbaImage::new(2, 2, &[0; 16]).is_some());
    }

    #[test]
    fn tile_rect_steps_by_tile_and_spacing() {
        let l = layout(16, 2, 1, 4, 3);
        assert_eq!(l.tile_rect(0), (UVec2::new(2, 2), UVec2::splat(16)));
        assert_eq!(l.tile_rect(5), (UVec2::new(19, 19), UVec2::splat(16)));
        assert_eq!(l.extent(), UVec2::new(2 + 64 + 3, 2 + 48 + 2));
        assert!(l.fits(UVec2::new(69, 52)));
        assert!(!l.fits(UVec2::new(68, 52)));
    }

    #[test]
    fn transparent_gutters_found_for_8_and_32() {
        for expected in [layout(8, 2, 1, 5, 4), layout(32, 2, 1, 3, 2)] {
            let s = sheet(expected.extent() + 2, &expected, [0; 4], 7);
            let found = detect(&s);
            assert_eq!(found.layout, expected);
            assert!(found.confidence > 0.5, "{}", found.confidence);
            assert!(empty_tiles(&s.image(), &found.layout).iter().all(|e| !e));
        }
    }

    #[test]
    fn uniform_colour_gutters_found() {
        let expected = layout(16, 1, 2, 4, 3);
        let found = detect(&sheet(
            expected.extent() + 1,
            &expected,
            [255, 0, 255, 255],
            3,
        ));
        assert_eq!(found.layout, expected);
        assert!(found.confidence > 0.5);
    }

    #[test]
    fn transparent_cells_flagged_without_renumbering() {
        let expected = layout(16, 0, 1, 4, 3);
        let mut s = sheet(expected.extent(), &expected, [0; 4], 11);
        s.clear_tile(&expected, 1);
        s.clear_tile(&expected, 6);
        let found = detect(&s);
        assert_eq!(found.layout, expected);
        let empties = empty_tiles(&s.image(), &found.layout);
        let flagged: Vec<usize> = (0..empties.len()).filter(|&i| empties[i]).collect();
        assert_eq!((empties.len(), flagged), (12, vec![1, 6]));
        assert_eq!(uniform_tile(&s.image(), &found.layout, 6), Some([0; 4]));
        assert_eq!(uniform_tile(&s.image(), &found.layout, 0), None);
    }

    #[test]
    fn packed_sheet_prefers_16_when_only_16_edges_are_sharp() {
        let mut s = Sheet::new(64, 48, [0; 4]);
        s.noisy_cells(&layout(8, 0, 0, 8, 6), UVec2::splat(16), 5);
        let found = detect(&s);
        assert_eq!(found.layout, layout(16, 0, 0, 4, 3));
        assert!(found.confidence > 0.5, "{}", found.confidence);
    }

    #[test]
    fn packed_sheet_picks_8_when_8_edges_are_sharp() {
        let l = layout(8, 0, 0, 8, 6);
        let mut s = Sheet::new(64, 48, [0; 4]);
        s.noisy_cells(&l, UVec2::splat(8), 5);
        let found = detect(&s);
        assert_eq!(found.layout, l);
        assert!(found.confidence > 0.5, "{}", found.confidence);
    }

    #[test]
    fn padded_sprites_read_as_packed_cells() {
        let sprites = layout(12, 2, 4, 4, 4);
        let found = detect(&sheet(UVec2::splat(64), &sprites, [0; 4], 8));
        assert_eq!(found.layout, layout(16, 0, 0, 4, 4));
    }

    #[test]
    fn non_square_tiles_win_with_evidence_on_both_axes() {
        let mut s = Sheet::new(128, 128, [0; 4]);
        s.noisy_cells(&layout(8, 0, 0, 16, 16), UVec2::new(16, 32), 6);
        let found = detect(&s);
        let expected = GridLayout {
            tile: UVec2::new(16, 32),
            rows: 4,
            ..layout(16, 0, 0, 8, 4)
        };
        assert_eq!(found.layout, expected);
    }

    #[test]
    fn soft_edges_do_not_collapse_to_one_tile() {
        let mut s = Sheet::new(128, 128, [0; 4]);
        s.cells(&layout(8, 0, 0, 16, 16), UVec2::splat(16), 9, 8, 12);
        let found = detect(&s);
        assert_eq!(found.layout, layout(16, 0, 0, 8, 8));
    }

    #[test]
    fn trailing_gutter_after_each_cell() {
        let expected = layout(16, 0, 1, 12, 3);
        let found = detect(&sheet(UVec2::new(204, 51), &expected, [0; 4], 12));
        assert_eq!(found.layout, expected);
    }

    #[test]
    fn padded_atlas_stops_before_the_padding() {
        let expected = layout(16, 0, 0, 12, 11);
        let found = detect(&sheet(UVec2::splat(256), &expected, [0; 4], 13));
        assert_eq!(found.layout, expected);
    }

    #[test]
    fn top_left_offset_with_trailing_padding() {
        let expected = GridLayout {
            margin: UVec2::new(3, 5),
            ..layout(16, 0, 0, 4, 3)
        };
        let found = detect(&sheet(UVec2::new(72, 60), &expected, [0; 4], 14));
        assert_eq!(found.layout, expected);
    }

    #[test]
    fn blank_image_has_no_grid() {
        assert!(detect_grid(&Sheet::new(64, 48, [0; 4]).image()).is_none());
        assert!(detect_grid(&RgbaImage::new(0, 0, &[]).expect("empty")).is_none());
    }
}
