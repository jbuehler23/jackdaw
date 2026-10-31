//! Tileset slicing over row-major RGBA8 bytes: grid detection and empty
//! cells. Pixels with alpha 0 compare equal regardless of colour.
//!
//! Grid detection works per axis. A candidate is a common tile size with a
//! leading margin in `0..=tile/2`, a spacing in `0..=4` and the fewest cells
//! after which the rest of the axis (the trailing padding) is one uniform
//! colour. Margin and spacing lines must share one uniform colour. Scores lie
//! in `0..=1`:
//! - with spacing, the share of content lines that are not uniform;
//! - without spacing, 0.9 times the stronger of two measures over the cells.
//!   Contrast is `1 - inner / boundary`: the mean colour step across cell
//!   boundaries against the mean step between other neighbouring lines.
//!   Repetition, for cells that join seamlessly, is `1 - near / at`: `at` is
//!   the share of pixels equal to the pixel one tile further along the axis,
//!   `near` the larger share one line closer or further, all three from the
//!   same non-uniform lines and ignoring pairs of transparent pixels. It
//!   counts only when `at - near` is at least 0.02 of `at` and exceeds both
//!   the gap between the two neighbouring shares and three sampling errors
//!   of `at`. For two sizes where one divides the other, contrast decides
//!   whenever either size has any, since a texture inside the cells can
//!   repeat at another period: the smaller wins when its extra lines are
//!   edges, their step at least 1.5 times its inner step and 0.3 times the
//!   larger size's boundary step, or carrying at least half of the larger
//!   size's boundary excess over the inner step. Without contrast, the
//!   smaller wins when the larger repeats and the smaller repeats at least
//!   half as strongly, as every multiple of the true pitch repeats wherever
//!   the pitch does. The winner keeps its score; the other takes half of the
//!   lower of its own and the winner's. A single cell is judged by contrast
//!   alone;
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
//!
//! Images imported together share one tile size: the one with the highest
//! sum of each image's best score at it, a square size within 0.05 per
//! image of the best winning. Each image then reports a layout at that size,
//! scoring or not, unless the size does not fit it or fits only in a
//! non-square layout the image does not support.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use glam::{UVec2, Vec2};

pub const COMMON_TILE_SIZES: [u32; 8] = [8, 12, 16, 24, 32, 48, 64, 128];
const MAX_SPACING: u32 = 4;
const MAX_ALTERNATIVES: usize = 4;
const GUTTERLESS_WEIGHT: f32 = 0.9;
const SINGLE_CELL_SCORE: f32 = 0.25;
const EDGE_RATIO: f32 = 1.5;
const EDGE_SHARE: f32 = 0.3;
const EDGE_EXCESS: f32 = 0.5;
const SQUARE_MARGIN: f32 = 0.05;
const REPEAT_FLOOR: f32 = 0.02;
const REPEAT_PAIR: f32 = 0.5;
const REPEAT_SIGMA: f32 = 3.0;

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

    /// The tile under pixel `at` and where in it, from (0, 0) at its top left
    /// to (1, 1); none in the margin, the spacing or past the grid.
    pub fn tile_at(&self, at: Vec2) -> Option<(u32, Vec2)> {
        let step = (self.tile + self.spacing).as_vec2();
        let from = at - self.margin.as_vec2();
        let cell = (from / step).floor();
        let inside = from - cell * step;
        let columns = (0.0..self.columns as f32).contains(&cell.x);
        let rows = (0.0..self.rows as f32).contains(&cell.y);
        if !columns || !rows || inside.cmpge(self.tile.as_vec2()).any() {
            return None;
        }
        let index = cell.y as u32 * self.columns + cell.x as u32;
        Some((index, inside / self.tile.as_vec2()))
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
/// when all its pixels match. `evidence` is keyed by `[start, end, tile]`
/// and `wins` by `[start, end, small, large]`; both are filled by `measure`.
struct Profile {
    step: Vec<f32>,
    uniform: Vec<Option<Rgba>>,
    evidence: HashMap<[u32; 3], Evidence>,
    wins: HashMap<[u32; 4], bool>,
}

/// Per offset, `[equal, compared]` for the pixels of each line `k` against
/// line `k + offset`, leaving out pairs where both pixels are transparent.
type Tallies = HashMap<u32, Vec<[u32; 2]>>;

/// Packed-cell evidence for one tile size: boundary contrast and pixel
/// repetition at the tile pitch, each in `0..=1`.
#[derive(Clone, Copy)]
struct Evidence {
    sharpness: f32,
    repetition: f32,
}

impl Evidence {
    fn strength(self) -> f32 {
        self.sharpness.max(self.repetition)
    }
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
            evidence: HashMap::new(),
            wins: HashMap::new(),
        },
        Profile {
            step: mean(row_step, w),
            uniform: row_uniform,
            evidence: HashMap::new(),
            wins: HashMap::new(),
        },
    ]
}

/// Tallies for columns (`axis == 0`) or rows at the offsets next to each of
/// `tiles`, from row slices.
fn repeats(image: &RgbaImage, axis: usize, tiles: &[u32]) -> Tallies {
    let len = if axis == 0 { image.width } else { image.height };
    let mut out: Tallies = tiles
        .iter()
        .flat_map(|&t| [t - 1, t, t + 1])
        .filter(|&d| d < len)
        .map(|d| (d, vec![[0; 2]; (len - d) as usize]))
        .collect();
    let rows: Vec<&[[u8; 4]]> = image.rows().map(|r| r.as_chunks::<4>().0).collect();
    let tally = |pair: &mut [u32; 2], a: &[u8; 4], b: &[u8; 4]| {
        if a[3] != 0 || b[3] != 0 {
            pair[0] += u32::from(canonical(a) == canonical(b));
            pair[1] += 1;
        }
    };
    for (y, row) in rows.iter().enumerate() {
        for (&offset, pairs) in &mut out {
            let d = offset as usize;
            if axis == 0 {
                for (pair, (a, b)) in pairs.iter_mut().zip(row.iter().zip(&row[d..])) {
                    tally(pair, a, b);
                }
            } else if let (Some(pair), Some(next)) = (pairs.get_mut(y), rows.get(y + d)) {
                for (a, b) in row.iter().zip(next.iter()) {
                    tally(pair, a, b);
                }
            }
        }
    }
    out
}

/// The common sizes that split `[start, end)` into two or more cells.
fn splits(start: u32, end: u32) -> impl Iterator<Item = u32> {
    let cells = end - start;
    COMMON_TILE_SIZES
        .into_iter()
        .filter(move |&t| cells.is_multiple_of(t) && cells / t >= 2)
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

    /// The rise of the equal-pair share at offset `tile` over the larger share
    /// one line closer or further, relative to the share at `tile`. All three
    /// are measured from the same non-uniform lines of `[start, end)`. Zero
    /// below `REPEAT_FLOOR`, or when the rise does not exceed the noise: the
    /// difference between the two neighbouring shares, or `REPEAT_SIGMA`
    /// sampling errors of the share at `tile`, whichever is larger. `tallies`
    /// must hold the offsets next to `tile`.
    fn repetition(&self, tallies: &Tallies, start: u32, end: u32, tile: u32) -> f32 {
        let lines =
            (start..end.saturating_sub(tile + 1)).filter(|&k| self.uniform[k as usize].is_none());
        let share = |d: u32| {
            let pairs = &tallies[&d];
            let [equal, compared] = lines.clone().fold([0, 0], |[e, n], k| {
                let [a, b] = pairs[k as usize];
                [e + a, n + b]
            });
            (equal as f32 / compared.max(1) as f32, compared)
        };
        let ((below, _), (at, compared), (above, _)) =
            (share(tile - 1), share(tile), share(tile + 1));
        let rise = at - below.max(above);
        let sampling = REPEAT_SIGMA * (at * (1.0 - at) / compared.max(1) as f32).sqrt();
        let noise = (below - above).abs().max(sampling);
        if at <= 0.0 || rise < REPEAT_FLOOR * at || rise <= noise {
            0.0
        } else {
            rise / at
        }
    }

    /// Fill `evidence` for every packed reading of two or more cells among
    /// `candidates`, at each size in `splits` of its range, and `wins` for
    /// each pair of those sizes where one divides the other. Pixel pairs are
    /// tallied only at the offsets these sizes need.
    fn measure(&mut self, image: &RgbaImage, axis: usize, candidates: &[Axis]) {
        let mut ranges: Vec<[u32; 2]> = candidates
            .iter()
            .map(|a| a.source)
            .filter(|c| c.spacing == 0 && c.count >= 2)
            .map(|c| [c.margin, c.end()])
            .collect();
        ranges.sort_unstable();
        ranges.dedup();
        let mut tiles: Vec<u32> = ranges.iter().flat_map(|&[a, b]| splits(a, b)).collect();
        tiles.sort_unstable();
        tiles.dedup();
        let tallies = repeats(image, axis, &tiles);
        for [start, end] in ranges {
            for tile in splits(start, end) {
                let evidence = Evidence {
                    sharpness: self.sharpness(start, end, tile),
                    repetition: self.repetition(&tallies, start, end, tile),
                };
                self.evidence.insert([start, end, tile], evidence);
            }
            for large in splits(start, end) {
                for small in splits(start, end).filter(|&t| t < large && large.is_multiple_of(t)) {
                    let wins = self.small_wins(start, end, small, large);
                    self.wins.insert([start, end, small, large], wins);
                }
            }
        }
    }

    /// Whether `small` reads `[start, end)` better than its multiple `large`.
    /// When either size shows contrast, its extra lines must be edges: a
    /// texture inside the cells can repeat at a finer or coarser period than
    /// the cells. Without contrast, `large` must repeat and `small` repeat at
    /// least `REPEAT_PAIR` times as strongly, as any multiple of the true
    /// pitch repeats wherever the pitch does. Needs `evidence` for both.
    fn small_wins(&self, start: u32, end: u32, small: u32, large: u32) -> bool {
        let (s, l) = (
            self.evidence[&[start, end, small]],
            self.evidence[&[start, end, large]],
        );
        if s.sharpness > 0.0 || l.sharpness > 0.0 {
            self.extra_lines_are_edges(start, end, small, large)
        } else {
            l.repetition > 0.0 && s.repetition >= REPEAT_PAIR * l.repetition
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
    /// colour, or the end when every line is: a single-colour image has no
    /// padding.
    fn padding_start(&self) -> u32 {
        let Some(Some(last)) = self.uniform.last() else {
            return self.len();
        };
        let run = self
            .uniform
            .iter()
            .rev()
            .take_while(|u| **u == Some(*last))
            .count() as u32;
        if run == self.len() {
            self.len()
        } else {
            self.len() - run
        }
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

    /// Whether this axis prefers `axis`'s tile size over `other`. A single
    /// cell has no repetition, so only contrast decides for it.
    fn supports(&self, axis: &Axis, other: u32) -> bool {
        let c = axis.source;
        if c.spacing > 0 {
            return true;
        }
        let (start, end) = (c.margin, c.end());
        let cells = end - start;
        let peer = |t: u32| cells.is_multiple_of(t) && cells / t >= 2;
        let wins = |small: u32, large: u32| {
            if c.count < 2 {
                self.extra_lines_are_edges(start, end, small, large)
            } else {
                self.wins[&[start, end, small, large]]
            }
        };
        if other < c.tile && c.tile.is_multiple_of(other) && peer(other) {
            !wins(other, c.tile)
        } else if other > c.tile && other.is_multiple_of(c.tile) && peer(other) {
            wins(c.tile, other)
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
        let raw = |t: u32| GUTTERLESS_WEIGHT * self.evidence[&[start, end, t]].strength();
        let mut score = raw(c.tile);
        let peers = peers
            .iter()
            .filter(|&&o| o != c.tile && cells.is_multiple_of(o) && cells / o >= 2);
        for &other in peers {
            let halve = if other < c.tile && c.tile.is_multiple_of(other) {
                self.wins[&[start, end, other, c.tile]]
            } else if other > c.tile && other.is_multiple_of(c.tile) {
                !self.wins[&[start, end, c.tile, other]]
            } else {
                false
            };
            if halve {
                score = 0.5 * score.min(raw(other));
            }
        }
        score
    }
}

fn rank(image: &RgbaImage) -> Vec<(GridLayout, f32)> {
    let mut profiles = profiles(image);
    let axes = [profiles[0].candidates(), profiles[1].candidates()];
    for (axis, profile) in profiles.iter_mut().enumerate() {
        profile.measure(image, axis, &axes[axis]);
    }
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
        let (x, y) = (axes[0][i].cand, axes[1][j].cand);
        let layout = GridLayout {
            tile: UVec2::new(x.tile, y.tile),
            margin: UVec2::new(x.margin, y.margin),
            spacing: UVec2::new(x.spacing, y.spacing),
            columns: x.count,
            rows: y.count,
        };
        let score = if sx > 0.0 && sy > 0.0 {
            (sx + sy) / 2.0
        } else {
            0.0
        };
        out.push((layout, score));
    }
    out
}

/// Every eligible layout, best first, without duplicates. A layout scores
/// zero unless both its axes score.
fn ranked(image: &RgbaImage) -> Vec<(GridLayout, f32)> {
    if image.width == 0 || image.height == 0 {
        return Vec::new();
    }
    let mut ranked = rank(image);
    ranked.sort_by(|(a, sa), (b, sb)| {
        best_first((a.tile, *sa), (b.tile, *sb))
            .then(a.margin.to_array().cmp(&b.margin.to_array()))
            .then(a.spacing.to_array().cmp(&b.spacing.to_array()))
    });
    let mut seen = HashSet::new();
    ranked.retain(|(l, _)| seen.insert(*l));
    ranked
}

/// Best score first; between equal scores the larger tile first.
fn best_first(a: (UVec2, f32), b: (UVec2, f32)) -> Ordering {
    b.1.total_cmp(&a.1)
        .then(b.0.x.cmp(&a.0.x))
        .then(b.0.y.cmp(&a.0.y))
}

/// In tiles sorted by `best_first`, the first square one within `margin` of
/// the best, else the best.
fn preferred(sorted: &[(UVec2, f32)], margin: f32) -> usize {
    let best = sorted.first().map_or(0.0, |(_, s)| *s);
    sorted
        .iter()
        .position(|(t, s)| t.x == t.y && *s >= best - margin)
        .unwrap_or(0)
}

/// An image's best score at `tile`, zero when it has no layout there.
fn best_score(ranked: &[(GridLayout, f32)], tile: UVec2) -> f32 {
    ranked
        .iter()
        .find(|(l, _)| l.tile == tile)
        .map_or(0.0, |(_, s)| *s)
}

/// The layout to report at `tile`: one that scores before one that does
/// not, then more than one cell before one, then the best score.
fn layout_at(ranked: &[(GridLayout, f32)], tile: UVec2) -> Option<(GridLayout, f32)> {
    ranked
        .iter()
        .filter(|(l, _)| l.tile == tile)
        .rev()
        .max_by_key(|(l, s)| (*s > 0.0, l.tile_count() > 1))
        .copied()
}

/// Detect tile size, margin and spacing. `None` for an empty image or one
/// where no layout scores above zero.
pub fn detect_grid(image: &RgbaImage) -> Option<GridDetection> {
    let mut ranked = ranked(image);
    ranked.retain(|(_, s)| *s > 0.0);
    if ranked.iter().any(|(l, _)| l.tile_count() > 1) {
        ranked.retain(|(l, _)| l.tile_count() > 1);
    }
    ranked.first()?;
    let tiles: Vec<_> = ranked.iter().map(|(l, s)| (l.tile, *s)).collect();
    let chosen = ranked.remove(preferred(&tiles, SQUARE_MARGIN));
    ranked.insert(0, chosen);
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

/// One tile size for images imported together.
#[derive(Clone, Debug, PartialEq)]
pub struct SharedDetection {
    pub tile: UVec2,
    /// `0..=1`: over the images where something scores, the mean of each
    /// one's best score at `tile` (zero where it has no layout there) less its
    /// best score at another size. Near zero when the images split evenly.
    pub confidence: f32,
    /// Per image, the layout reported at `tile` (a scoring one first, then
    /// more than one cell) with its score less half the image's best score
    /// at another size. `None` when `tile` does not fit the image, or when it
    /// fits only in a non-square layout the image does not support on both
    /// axes.
    pub layouts: Vec<Option<(GridLayout, f32)>>,
}

/// Pick one tile size for `images`, the one with the highest sum of each
/// image's best score at it, then report each image's layout at that size.
/// Images where a size does not fit, and single-colour images, add nothing
/// to it. `None` when nothing in any image scores.
pub fn detect_grid_shared(images: &[RgbaImage]) -> Option<SharedDetection> {
    let ranked: Vec<_> = images.iter().map(ranked).collect();
    let mut totals: Vec<(UVec2, f32)> = Vec::new();
    for (layout, _) in ranked.iter().flatten() {
        if totals.iter().all(|(t, _)| *t != layout.tile) {
            let total = ranked.iter().map(|r| best_score(r, layout.tile)).sum();
            totals.push((layout.tile, total));
        }
    }
    totals.sort_by(|a, b| best_first(*a, *b));
    if totals.first()?.1 <= 0.0 {
        return None;
    }
    let scoring: Vec<_> = ranked
        .iter()
        .filter(|r| r.first().is_some_and(|(_, s)| *s > 0.0))
        .collect();
    let tile = totals[preferred(&totals, SQUARE_MARGIN * scoring.len() as f32)].0;
    let elsewhere = |r: &[(GridLayout, f32)]| {
        r.iter()
            .find(|(l, _)| l.tile != tile)
            .map_or(0.0, |(_, s)| *s)
    };
    let leads: Vec<f32> = scoring
        .iter()
        .map(|r| best_score(r, tile) - elsewhere(r))
        .collect();
    let layouts = ranked
        .iter()
        .map(|r| {
            let (layout, score) = layout_at(r, tile)?;
            Some((layout, (score - 0.5 * elsewhere(r)).clamp(0.0, 1.0)))
        })
        .collect();
    let lead = leads.iter().sum::<f32>() / leads.len().max(1) as f32;
    Some(SharedDetection {
        tile,
        confidence: lead.clamp(0.0, 1.0),
        layouts,
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

    /// `cells` by `cells` square cells of `tile` pixels that join without a
    /// step: all cells share their border lines, and each inside is one of
    /// three patterns.
    fn seamless(tile: u32, cells: u32) -> Sheet {
        let size = tile * cells;
        let mut s = Sheet::new(size, size, [0; 4]);
        let colour = |h: u32| [h as u8, (h >> 8) as u8, (h >> 16) as u8, 255];
        for y in 0..size {
            for x in 0..size {
                let (u, v) = (x % tile, y % tile);
                let kind = hash(x / tile, y / tile, 3) % 3;
                let c = if u == 0 || u == tile - 1 {
                    colour(hash(v, 0, 4))
                } else if v == 0 || v == tile - 1 {
                    colour(hash(u, 1, 4))
                } else {
                    colour(hash(u, v, kind))
                };
                s.set(x, y, c);
            }
        }
        s
    }

    #[test]
    fn seamless_cells_found_by_repetition() {
        assert_eq!(detect(&seamless(16, 7)).layout, layout(16, 0, 0, 7, 7));
    }

    #[test]
    fn seamless_cells_keep_their_pitch_over_its_multiples() {
        assert_eq!(detect(&seamless(16, 16)).layout, layout(16, 0, 0, 16, 16));
        assert_eq!(detect(&seamless(64, 4)).layout, layout(64, 0, 0, 4, 4));
    }

    #[test]
    fn brick_texture_inside_tiles_keeps_the_tile_size() {
        let mut s = Sheet::new(128, 128, [0; 4]);
        for y in 0..128 {
            for x in 0..128 {
                let base = hash(x / 16, y / 16, 21);
                let mortar = x % 8 == 0 || y % 8 == 0;
                let grain = if mortar {
                    0
                } else {
                    4 + hash(x % 8, y % 8, 22) % 6
                };
                let channel = |shift: u32| (40 + (base >> shift) % 180 + grain) as u8;
                s.set(x, y, [channel(0), channel(8), channel(16), 255]);
            }
        }
        assert_eq!(detect(&s).layout, layout(16, 0, 0, 8, 8));
    }

    #[test]
    fn texture_without_a_grid_is_not_confident() {
        let mut s = Sheet::new(120, 120, [0; 4]);
        for y in 0..120 {
            for x in 0..120 {
                let v = (hash(x, y, 31) % 4 * 60) as u8;
                s.set(x, y, [v, 120, 255 - v, 255]);
            }
        }
        let found = detect_grid(&s.image());
        assert!(
            found.as_ref().is_none_or(|d| d.confidence < 0.2),
            "{found:?}"
        );
    }

    fn sharp(block: u32, seed: u32) -> Sheet {
        let mut s = Sheet::new(64, 48, [0; 4]);
        s.noisy_cells(&layout(8, 0, 0, 8, 6), UVec2::splat(block), seed);
        s
    }

    #[test]
    fn shared_detection_follows_the_other_images() {
        let mut single = Sheet::new(16, 16, [0; 4]);
        single.noisy_cells(&layout(16, 0, 0, 1, 1), UVec2::splat(16), 41);
        let mut small = Sheet::new(12, 12, [0; 4]);
        small.noisy_cells(&layout(12, 0, 0, 1, 1), UVec2::splat(12), 45);
        let sheets = [
            sharp(16, 42),
            sharp(16, 43),
            sharp(8, 44),
            single,
            Sheet::new(32, 32, [9, 9, 9, 255]),
            small,
        ];
        assert_eq!(detect(&sheets[2]).layout.tile, UVec2::splat(8));
        let images: Vec<_> = sheets.iter().map(Sheet::image).collect();
        let found = detect_grid_shared(&images).expect("detected");
        assert_eq!(found.tile, UVec2::splat(16));
        let layouts: Vec<_> = found.layouts.iter().map(|l| l.map(|(l, _)| l)).collect();
        let packed = Some(layout(16, 0, 0, 4, 3));
        let expected = [
            packed,
            packed,
            packed,
            Some(layout(16, 0, 0, 1, 1)),
            Some(layout(16, 0, 0, 2, 2)),
            None,
        ];
        assert_eq!(layouts, expected);
        assert!(found.confidence > 0.05, "{}", found.confidence);
    }

    #[test]
    fn shared_detection_split_evenly_is_unsure() {
        let sheets = [sharp(16, 51), sharp(8, 52), sharp(16, 53), sharp(8, 54)];
        let images: Vec<_> = sheets.iter().map(Sheet::image).collect();
        let found = detect_grid_shared(&images).expect("detected");
        let agreed = detect_grid_shared(&images[..1]).expect("detected");
        assert!(found.confidence < 0.25 * agreed.confidence, "{found:?}");
        assert!(agreed.confidence > 0.4, "{agreed:?}");
    }

    /// 16 px cells with a dark border line and one of `period` interior
    /// patterns, chosen by `(column + row) % period`, so the variants repeat
    /// every `period` cells.
    fn bordered_variants(period: u32) -> Sheet {
        let mut s = Sheet::new(96, 96, [0; 4]);
        for y in 0..96 {
            for x in 0..96 {
                let (u, v) = (x % 16, y % 16);
                let variant = (x / 16 + y / 16) % period;
                let c = if u == 0 || v == 0 {
                    [20, 20, 20, 255]
                } else {
                    let h = hash(u, v, 60 + variant);
                    [h as u8, (h >> 8) as u8, (h >> 16) as u8, 255]
                };
                s.set(x, y, c);
            }
        }
        s
    }

    #[test]
    fn bordered_cells_beat_the_period_of_their_variants() {
        for period in [2, 3] {
            let found = detect(&bordered_variants(period));
            assert_eq!(found.layout, layout(16, 0, 0, 6, 6), "period {period}");
        }
    }

    #[test]
    fn courses_inside_bordered_tiles_keep_the_tile_size() {
        let mut s = Sheet::new(96, 96, [0; 4]);
        for y in 0..96 {
            for x in 0..96 {
                let (u, v) = (x % 16, y % 16);
                let c = if u == 0 || v == 0 {
                    [20, 20, 20, 255]
                } else {
                    let h = hash(u, v % 8, 72);
                    [h as u8, (h >> 8) as u8, (h >> 16) as u8, 255]
                };
                s.set(x, y, c);
            }
        }
        assert_eq!(detect(&s).layout, layout(16, 0, 0, 6, 6));
    }

    #[test]
    fn padded_pitch_on_one_axis_meets_packed_cells_on_the_other() {
        let sprites = GridLayout {
            tile: UVec2::new(12, 16),
            margin: UVec2::new(2, 0),
            spacing: UVec2::new(4, 0),
            columns: 4,
            rows: 8,
        };
        let found = detect(&sheet(UVec2::new(64, 128), &sprites, [0; 4], 61));
        assert_eq!(found.layout, layout(16, 0, 0, 4, 8));
    }

    #[test]
    fn shared_confidence_drops_for_an_image_the_size_does_not_fit() {
        let mut twelve = Sheet::new(48, 36, [0; 4]);
        twelve.noisy_cells(&layout(12, 0, 0, 4, 3), UVec2::splat(12), 62);
        let sheets = [sharp(16, 63), sharp(16, 64), sharp(16, 65), twelve];
        let images: Vec<_> = sheets.iter().map(Sheet::image).collect();
        let alone = detect_grid_shared(&images[..3]).expect("detected");
        let found = detect_grid_shared(&images).expect("detected");
        assert_eq!((found.tile, found.layouts[3]), (UVec2::splat(16), None));
        assert!(
            found.confidence < alone.confidence - 0.1,
            "{} {}",
            found.confidence,
            alone.confidence
        );
    }

    #[test]
    fn shared_size_without_an_eligible_layout_leaves_the_image_out() {
        let tall = |seed: u32| {
            let mut s = Sheet::new(128, 128, [0; 4]);
            s.noisy_cells(&layout(8, 0, 0, 16, 16), UVec2::new(16, 32), seed);
            s
        };
        let mut square = Sheet::new(32, 32, [0; 4]);
        square.noisy_cells(&layout(16, 0, 0, 2, 2), UVec2::splat(16), 68);
        let mut sheets: Vec<_> = (66..72).map(tall).collect();
        sheets.push(square);
        let images: Vec<_> = sheets.iter().map(Sheet::image).collect();
        let found = detect_grid_shared(&images).expect("detected");
        assert_eq!(found.tile, UVec2::new(16, 32));
        assert!(found.layouts[..6].iter().all(Option::is_some));
        assert_eq!(found.layouts[6], None);
    }

    #[test]
    fn blank_image_has_no_grid() {
        assert!(detect_grid(&Sheet::new(64, 48, [0; 4]).image()).is_none());
        assert!(detect_grid(&RgbaImage::new(0, 0, &[]).expect("empty")).is_none());
    }
}
