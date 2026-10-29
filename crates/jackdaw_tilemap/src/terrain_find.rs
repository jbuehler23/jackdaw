//! Terrain finding: proposes terrains from tileset pixels and labels every
//! tile's nine peers.
//!
//! - Regions (per axis of `n` pixels): `n/8` corner squares, `n/16` deep edge
//!   strips between them and an `n/4` centre square, each with its
//!   alpha-weighted Oklab mean, coverage and variance.
//! - Clustering: low-variance regions of non-sprite tiles, less prop centres,
//!   are sorted, led, refined and merged within [`CLUSTER_RADIUS`].
//! - Confidence: `1 - best / second`, or the pixel vote margin for a region far
//!   from every terrain, scaled by coverage above [`EMPTY_COVERAGE`].
//! - Decor: a sprite has a mostly transparent border with no fully covered
//!   region fitting a terrain; a prop's centre is far from every terrain, or
//!   weakly differs from a one-terrain border.

use glam::{Vec3, Vec4};

use crate::{GridLayout, Peer, RgbaImage, TerrainId, TileTerrains, empty_tiles};

/// Fainter pixels count as transparent.
pub(crate) const MIN_ALPHA: f32 = 0.2;
/// Least covered share of a non-empty region.
const EMPTY_COVERAGE: f32 = 0.5;
/// Most variance of a seed region.
const SEED_VARIANCE: f32 = 0.002;
const TEXTURE_WEIGHT: f32 = 0.5;
/// Oklab distance within one terrain.
pub(crate) const CLUSTER_RADIUS: f32 = 0.06;
const REFINE_PASSES: usize = 16;
/// Distinct tiles seeding a terrain.
const MIN_TERRAIN_TILES: usize = 2;
/// Summed border transparency of a sprite.
const DECOR_TRANSPARENT_BORDER: f32 = 4.0;
/// Weak centre against its border's mean confidence.
const PROP_RATIO: f32 = 0.25;
const MAX_TERRAINS: usize = TerrainId::MAX as usize;
const CENTRE: usize = Peer::Centre.index();

/// One sheet of the tileset; `name` is its file name or path.
#[derive(Clone, Copy, Debug)]
pub struct SheetInput<'a> {
    pub name: &'a str,
    pub image: RgbaImage<'a>,
    pub layout: GridLayout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileAddress {
    pub sheet: usize,
    pub tile: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TileKind {
    Terrain,
    Decor,
}

/// Proposed peers of a non-empty tile, with a confidence in `0..=1` per peer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileLabels {
    pub kind: TileKind,
    pub terrains: TileTerrains,
    pub confidence: [f32; 9],
}

#[derive(Clone, Debug, PartialEq)]
pub struct FoundTerrain {
    pub id: TerrainId,
    pub oklab: Vec3,
    pub swatch: [u8; 3],
    /// Mean variance of its seed regions.
    pub variance: f32,
    pub pure_tiles: Vec<TileAddress>,
    /// Its bare ground material from [`find_terrains`], so not unique;
    /// auto setup gives every terrain its final, unique name.
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TerrainAnalysis {
    /// Terrain `id` is at index `id - 1`.
    pub terrains: Vec<FoundTerrain>,
    /// Per sheet, per tile id, `None` for an empty tile; empty when the
    /// layout leaves the image.
    pub tiles: Vec<Vec<Option<TileLabels>>>,
}

impl TerrainAnalysis {
    pub fn tile(&self, address: TileAddress) -> Option<&TileLabels> {
        self.tiles
            .get(address.sheet)?
            .get(address.tile as usize)?
            .as_ref()
    }

    pub fn terrain(&self, id: TerrainId) -> Option<&FoundTerrain> {
        self.terrains.get(usize::from(id).checked_sub(1)?)
    }
}

#[derive(Clone, Copy)]
struct Sample {
    oklab: Vec3,
    rgb: Vec3,
    coverage: f32,
    variance: f32,
    rect: [u32; 4],
}

impl Sample {
    fn opaque(&self) -> bool {
        self.coverage >= EMPTY_COVERAGE
    }

    fn feature(&self) -> Vec4 {
        self.oklab.extend(TEXTURE_WEIGHT * self.variance.sqrt())
    }
}

type Regions = [Sample; 9];
type Sheets = Vec<Vec<Option<Regions>>>;

#[derive(Clone, Copy)]
struct Seed {
    feature: Vec4,
    rgb: Vec3,
    variance: f32,
    tile: usize,
}

#[derive(Clone, Copy)]
struct Cluster {
    feature: Vec4,
    rgb: Vec3,
    variance: f32,
    tiles: usize,
}

#[derive(Clone, Copy)]
struct Label {
    cluster: Option<usize>,
    confidence: f32,
    /// The mean lies within [`CLUSTER_RADIUS`] of the terrain.
    near: bool,
    /// Near, or chosen by a pixel vote.
    fits: bool,
}

/// Finds terrains across every sheet and labels each non-empty tile.
pub fn find_terrains(inputs: &[SheetInput]) -> TerrainAnalysis {
    let sampled: Sheets = inputs.iter().map(sample_sheet).collect();
    let mut seeds = Vec::new();
    for (tile, regions) in sampled.iter().flatten().flatten().enumerate() {
        let touches_edge = border(regions).any(|s| s.coverage >= 1.0);
        if transparency(regions) >= DECOR_TRANSPARENT_BORDER && !touches_edge {
            continue;
        }
        let prop_centre = prop_candidate(regions);
        for (i, s) in regions.iter().enumerate() {
            if s.opaque() && s.variance <= SEED_VARIANCE && !(i == CENTRE && prop_centre) {
                let (feature, rgb, variance) = (s.feature(), s.rgb, s.variance);
                seeds.push(Seed {
                    feature,
                    rgb,
                    variance,
                    tile,
                });
            }
        }
    }
    seeds.sort_by(|a, b| compare(a.feature, b.feature));
    let mut clusters = cluster(&seeds);
    clusters.retain(|c| c.tiles >= MIN_TERRAIN_TILES);
    if clusters.len() > MAX_TERRAINS {
        clusters.sort_by_key(|c| std::cmp::Reverse(c.tiles));
        clusters.truncate(MAX_TERRAINS);
    }

    let mut labels = label_all(inputs, &sampled, &clusters);
    for _ in 0..clusters.len() + 1 {
        let order = first_use(&labels);
        if order.iter().copied().eq(0..clusters.len()) {
            break;
        }
        clusters = order.iter().map(|&i| clusters[i]).collect();
        labels = label_all(inputs, &sampled, &clusters);
    }

    let tiles: Vec<Vec<Option<TileLabels>>> = sampled
        .iter()
        .zip(&labels)
        .map(|(sheet, l)| {
            let tiles = sheet.iter().zip(l);
            tiles
                .map(|(r, l)| r.as_ref().zip(*l).map(|(r, l)| tile_labels(r, l)))
                .collect()
        })
        .collect();
    let mut terrains: Vec<FoundTerrain> = Vec::with_capacity(clusters.len());
    for (i, c) in clusters.iter().enumerate() {
        let terrain = summarise(i, c, &tiles);
        terrains.push(terrain);
    }
    TerrainAnalysis { terrains, tiles }
}

fn border<T>(regions: &[T; 9]) -> impl Iterator<Item = &T> {
    let all = regions.iter().enumerate();
    all.filter(|(i, _)| *i != CENTRE).map(|(_, r)| r)
}

fn transparency(regions: &Regions) -> f32 {
    border(regions).map(|s| 1.0 - s.coverage).sum()
}

/// An opaque centre unlike a border of one colour.
fn prop_candidate(regions: &Regions) -> bool {
    let mut opaque = border(regions).filter(|s| s.opaque()).map(Sample::feature);
    let first = opaque.next();
    let centre = regions[CENTRE].feature();
    let near = |a: Vec4, b: Vec4| a.distance(b) <= CLUSTER_RADIUS;
    regions[CENTRE].opaque()
        && first.is_some_and(|f| !near(f, centre) && opaque.all(|o| near(o, f)))
}

fn compare(a: Vec4, b: Vec4) -> std::cmp::Ordering {
    let pairs = a.to_array().into_iter().zip(b.to_array());
    let mut order = pairs.map(|(x, y)| x.total_cmp(&y));
    order
        .find(|o| o.is_ne())
        .unwrap_or(std::cmp::Ordering::Equal)
}

fn sample_sheet(input: &SheetInput) -> Vec<Option<Regions>> {
    let size = glam::UVec2::new(input.image.width, input.image.height);
    if !input.layout.fits(size) {
        return Vec::new();
    }
    let empty = empty_tiles(&input.image, &input.layout);
    (0..input.layout.tile_count())
        .map(|index| {
            let blank = empty.get(index as usize).copied().unwrap_or(true);
            let (origin, size) = input.layout.tile_rect(index);
            let rects =
                region_rects(size.x, size.y).map(|[x, y, w, h]| [origin.x + x, origin.y + y, w, h]);
            (!blank).then(|| rects.map(|rect| sample(&input.image, rect)))
        })
        .collect()
}

/// Region rectangles `[x, y, width, height]` within a `w x h` tile, in
/// [`Peer`] order.
pub(crate) fn region_rects(w: u32, h: u32) -> [[u32; 4]; 9] {
    let spans = |n: u32| {
        let c = (n / 8).max(1).min(n);
        let d = (n / 16).max(1).min(n);
        let q = (n / 4).max(1).min(n);
        let corner = [(0, c), (c, n.saturating_sub(2 * c)), (n - c, c)];
        let strip = [(0, d), ((n - q) / 2, q), (n - d, d)];
        (corner, strip)
    };
    let (x_corner, x_strip) = spans(w);
    let (y_corner, y_strip) = spans(h);
    std::array::from_fn(|i| {
        let (col, row) = (i % 3, i / 3);
        let x = [x_corner, x_strip][usize::from(row == 1)][col];
        let y = [y_corner, y_strip][usize::from(col == 1)][row];
        [x.0, y.0, x.1, y.1]
    })
}

/// Alpha and value of each pixel at least [`MIN_ALPHA`] opaque.
fn counted(image: &RgbaImage, [x, y, w, h]: [u32; 4]) -> impl Iterator<Item = (f32, [u8; 4])> {
    let rows = y..y.saturating_add(h);
    rows.flat_map(move |py| (x..x.saturating_add(w)).filter_map(move |px| image.pixel(px, py)))
        .map(|p| (f32::from(p[3]) / 255.0, p))
        .filter(|(a, _)| *a >= MIN_ALPHA)
}

fn sample(image: &RgbaImage, rect: [u32; 4]) -> Sample {
    let (mut weight, mut count, mut oklab, mut rgb) = (0.0, 0usize, Vec3::ZERO, Vec3::ZERO);
    for (a, p) in counted(image, rect) {
        weight += a;
        count += 1;
        oklab += a * srgb_to_oklab(p);
        rgb += a * Vec3::new(p[0].into(), p[1].into(), p[2].into());
    }
    let weight = weight.max(f32::MIN_POSITIVE);
    let oklab = oklab / weight;
    let spread = counted(image, rect).map(|(a, p)| a * srgb_to_oklab(p).distance_squared(oklab));
    Sample {
        oklab,
        rgb: rgb / weight,
        coverage: count as f32 / (rect[2] as f32 * rect[3] as f32).max(1.0),
        variance: spread.sum::<f32>() / weight,
        rect,
    }
}

/// Leader pass over sorted seeds, nearest-mean refinement, then merging of
/// centres closer than [`CLUSTER_RADIUS`].
fn cluster(seeds: &[Seed]) -> Vec<Cluster> {
    let mut centres: Vec<Vec4> = Vec::new();
    for seed in seeds {
        if nearest(&centres, seed.feature).is_none_or(|(_, d, _)| d > CLUSTER_RADIUS) {
            centres.push(seed.feature);
        }
    }
    let mut assignment = refine(seeds, &mut centres);
    while let Some((i, j)) = closest_pair(&centres) {
        let weight = |k: usize| assignment.iter().filter(|&&a| a == k).count() as f32;
        let (wi, wj) = (weight(i), weight(j));
        if wi + wj > 0.0 {
            centres[i] = (centres[i] * wi + centres[j] * wj) / (wi + wj);
        }
        centres.remove(j);
        assignment = refine(seeds, &mut centres);
    }
    let mut members: Vec<Vec<&Seed>> = vec![Vec::new(); centres.len()];
    for (seed, &k) in seeds.iter().zip(&assignment) {
        members[k].push(seed);
    }
    let stats = centres.iter().zip(members).map(|(&feature, m)| {
        let n = m.len().max(1) as f32;
        let mut tiles: Vec<usize> = m.iter().map(|s| s.tile).collect();
        tiles.sort_unstable();
        tiles.dedup();
        let rgb = m.iter().map(|s| s.rgb).sum::<Vec3>() / n;
        let variance = m.iter().map(|s| s.variance).sum::<f32>() / n;
        let tiles = tiles.len();
        Cluster {
            feature,
            rgb,
            variance,
            tiles,
        }
    });
    stats.collect()
}

fn refine(seeds: &[Seed], centres: &mut [Vec4]) -> Vec<usize> {
    let mut assignment = Vec::new();
    for _ in 0..REFINE_PASSES {
        let next: Vec<usize> = seeds
            .iter()
            .map(|s| nearest(centres, s.feature).map_or(0, |(i, _, _)| i))
            .collect();
        if next == assignment {
            break;
        }
        assignment = next;
        let mut sums = vec![(Vec4::ZERO, 0.0); centres.len()];
        for (seed, &k) in seeds.iter().zip(&assignment) {
            sums[k].0 += seed.feature;
            sums[k].1 += 1.0;
        }
        for (centre, (sum, n)) in centres.iter_mut().zip(sums) {
            if n > 0.0 {
                *centre = sum / n;
            }
        }
    }
    assignment
}

fn closest_pair(centres: &[Vec4]) -> Option<(usize, usize)> {
    let pairs = (0..centres.len()).flat_map(|i| (i + 1..centres.len()).map(move |j| (i, j)));
    let close = pairs.map(|(i, j)| (centres[i].distance(centres[j]), i, j));
    let best = close
        .filter(|(d, _, _)| *d < CLUSTER_RADIUS)
        .min_by(|a, b| a.0.total_cmp(&b.0));
    best.map(|(_, i, j)| (i, j))
}

/// Index and distance of the nearest centre, the lowest index on ties, plus
/// the second-nearest distance.
fn nearest(centres: &[Vec4], feature: Vec4) -> Option<(usize, f32, f32)> {
    let mut best: Option<(usize, f32, f32)> = None;
    for (i, c) in centres.iter().enumerate() {
        let d = c.distance(feature);
        best = match best {
            None => Some((i, d, f32::INFINITY)),
            Some((_, b, _)) if d < b => Some((i, d, b)),
            Some((j, b, s)) => Some((j, b, s.min(d))),
        };
    }
    best
}

struct Labeller<'a> {
    centres: &'a [Vec4],
    colours: Vec<Vec4>,
    votes: Vec<u32>,
}

impl Labeller<'_> {
    fn label(&mut self, image: &RgbaImage, sample: &Sample) -> Label {
        if !sample.opaque() {
            let confidence = 1.0 - sample.coverage / EMPTY_COVERAGE;
            return Label {
                cluster: None,
                confidence,
                near: true,
                fits: true,
            };
        }
        let Some((i, best, second)) = nearest(self.centres, sample.feature()) else {
            return Label {
                cluster: None,
                confidence: 0.0,
                near: false,
                fits: false,
            };
        };
        let margin = if second.is_infinite() {
            1.0
        } else {
            1.0 - best / second.max(f32::MIN_POSITIVE)
        };
        let (cluster, margin, fits) =
            match (best > CLUSTER_RADIUS).then(|| self.vote(image, sample)) {
                Some(Some((k, m))) => (k, m, true),
                Some(None) => (i, margin, false),
                None => (i, margin, true),
            };
        let opacity = ((sample.coverage - EMPTY_COVERAGE) / (1.0 - EMPTY_COVERAGE)).min(1.0);
        let confidence = (margin * opacity).clamp(0.0, 1.0);
        Label {
            cluster: Some(cluster),
            confidence,
            near: best <= CLUSTER_RADIUS,
            fits,
        }
    }

    /// The terrain most counted pixels lie within [`CLUSTER_RADIUS`] of, the
    /// lowest index on ties, and the vote margin over the counted pixels.
    fn vote(&mut self, image: &RgbaImage, sample: &Sample) -> Option<(usize, f32)> {
        self.votes.iter_mut().for_each(|v| *v = 0);
        let mut counted_pixels = 0u32;
        for (_, p) in counted(image, sample.rect) {
            counted_pixels += 1;
            let near = nearest(&self.colours, srgb_to_oklab(p).extend(0.0));
            if let Some((i, _, _)) = near.filter(|n| n.1 <= CLUSTER_RADIUS) {
                self.votes[i] += 1;
            }
        }
        let (best, &most) = self
            .votes
            .iter()
            .enumerate()
            .rev()
            .max_by_key(|(_, v)| **v)?;
        let others = self.votes.iter().enumerate().filter(|(i, _)| *i != best);
        let second = others.map(|(_, v)| *v).max().unwrap_or(0);
        (most > 0).then(|| (best, (most - second) as f32 / counted_pixels.max(1) as f32))
    }
}

fn label_all(
    inputs: &[SheetInput],
    sampled: &Sheets,
    clusters: &[Cluster],
) -> Vec<Vec<Option<[Label; 9]>>> {
    let centres: Vec<Vec4> = clusters.iter().map(|c| c.feature).collect();
    let mut labeller = Labeller {
        colours: centres.iter().map(|c| c.truncate().extend(0.0)).collect(),
        votes: vec![0; centres.len()],
        centres: &centres,
    };
    let sheets = inputs.iter().zip(sampled);
    sheets
        .map(|(input, tiles)| {
            let mut label = |r: &Regions| r.each_ref().map(|s| labeller.label(&input.image, s));
            tiles.iter().map(|r| r.as_ref().map(&mut label)).collect()
        })
        .collect()
}

/// Cluster indices in order of first use as a label; unused ones are left out.
fn first_use(labels: &[Vec<Option<[Label; 9]>>]) -> Vec<usize> {
    let mut order = Vec::new();
    for label in labels.iter().flatten().flatten().flatten() {
        if let Some(i) = label.cluster.filter(|i| !order.contains(i)) {
            order.push(i);
        }
    }
    order
}

fn tile_labels(regions: &Regions, labels: [Label; 9]) -> TileLabels {
    let opaque: Vec<(&Sample, &Label)> = border(regions)
        .zip(border(&labels))
        .filter(|(s, _)| s.opaque())
        .collect();
    let one = opaque
        .first()
        .filter(|(_, f)| opaque.iter().all(|(_, l)| l.cluster == f.cluster && l.fits));
    let mean = opaque.iter().map(|(_, l)| l.confidence).sum::<f32>() / opaque.len().max(1) as f32;
    let centre = labels[CENTRE];
    let reaches_edge = opaque.iter().any(|(s, l)| s.coverage >= 1.0 && l.fits);
    let sprite = transparency(regions) >= DECOR_TRANSPARENT_BORDER && !reaches_edge;
    let prop = regions[CENTRE].opaque()
        && match one {
            Some((_, border)) => {
                let differs = !centre.fits || centre.cluster != border.cluster;
                let weak = centre.confidence < PROP_RATIO * mean;
                differs && (!centre.near || weak)
            }
            None => {
                let in_border = opaque
                    .iter()
                    .any(|(_, l)| l.fits && l.cluster == centre.cluster);
                !centre.near && !in_border
            }
        };
    let id = |l: &Label| l.cluster.map_or(0, |i| (i + 1) as TerrainId);
    TileLabels {
        kind: if sprite || prop {
            TileKind::Decor
        } else {
            TileKind::Terrain
        },
        terrains: TileTerrains {
            peers: labels.each_ref().map(id),
        },
        confidence: labels.map(|l| l.confidence),
    }
}

fn summarise(index: usize, cluster: &Cluster, tiles: &[Vec<Option<TileLabels>>]) -> FoundTerrain {
    let id = (index + 1) as TerrainId;
    let mut pure_tiles = Vec::new();
    for (sheet, labels) in tiles.iter().enumerate() {
        for (tile, l) in labels.iter().enumerate() {
            let Some(l) = l else { continue };
            if l.kind == TileKind::Terrain && l.terrains.peers.iter().all(|&p| p == id) {
                pure_tiles.push(TileAddress {
                    sheet,
                    tile: tile as u32,
                });
            }
        }
    }
    let oklab = cluster.feature.truncate();
    FoundTerrain {
        id,
        oklab,
        swatch: cluster.rgb.round().to_array().map(|c| c as u8),
        variance: cluster.variance,
        pure_tiles,
        name: material(oklab).to_string(),
    }
}

/// Ground material of a colour: dark below lightness 0.35; snow from
/// lightness 0.85 under chroma 0.06; stone under chroma 0.03; then by hue
/// in degrees: grass 90..200, water 200..290, purple 290..20, and sand
/// (lightness 0.72 up) or dirt 20..90.
pub(crate) fn material(oklab: Vec3) -> &'static str {
    let chroma = oklab.y.hypot(oklab.z);
    let hue = oklab.z.atan2(oklab.y).to_degrees().rem_euclid(360.0);
    if oklab.x < 0.35 {
        "dark"
    } else if oklab.x >= 0.85 && chroma < 0.06 {
        "snow"
    } else if chroma < 0.03 {
        "stone"
    } else if (90.0..200.0).contains(&hue) {
        "grass"
    } else if (200.0..290.0).contains(&hue) {
        "water"
    } else if (20.0..90.0).contains(&hue) {
        if oklab.x >= 0.72 { "sand" } else { "dirt" }
    } else {
        "purple"
    }
}

/// Names every terrain once, kept terrains in id order and then dropped
/// ones, each unique against `reserved` and the names before it. A kept
/// inner of a set over a background terrain is `"<material> path"` when it
/// is sand, dirt, stone, or grass over a ground that is not grass; any
/// other terrain is its bare material.
pub(crate) fn name_terrains(
    terrains: &mut [FoundTerrain],
    kept: &[TerrainId],
    background: &[TerrainId],
    sets: &[(TerrainId, TerrainId)],
    reserved: &[String],
) {
    let materials: Vec<(TerrainId, &str)> =
        terrains.iter().map(|t| (t.id, material(t.oklab))).collect();
    let material_of = |id: TerrainId| materials.iter().find(|(i, _)| *i == id).map(|(_, m)| *m);
    let mut taken = reserved.to_vec();
    for kept_pass in [true, false] {
        for t in terrains
            .iter_mut()
            .filter(|t| kept.contains(&t.id) == kept_pass)
        {
            let own = material(t.oklab);
            let ground = (sets.iter())
                .filter(|&&(inner, outer)| inner == t.id && background.contains(&outer))
                .find_map(|&(_, outer)| material_of(outer));
            let path = ground.is_some_and(|g| match own {
                "sand" | "dirt" | "stone" => true,
                "grass" => g != "grass",
                _ => false,
            });
            let base = if kept_pass && path && !background.contains(&t.id) {
                format!("{own} path")
            } else {
                own.to_string()
            };
            t.name = unique(&base, &taken);
            taken.push(t.name.clone());
        }
    }
}

/// `base`, or `base` with the first free " 2", " 3" and so on.
pub(crate) fn unique(base: &str, taken: &[String]) -> String {
    let mut name = base.to_string();
    let mut n = 1;
    while taken.contains(&name) {
        n += 1;
        name = format!("{base} {n}");
    }
    name
}

pub(crate) fn srgb_to_oklab(p: [u8; 4]) -> Vec3 {
    let linear = |c: u8| {
        let c = f32::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let rgb = Vec3::new(linear(p[0]), linear(p[1]), linear(p[2]));
    let lms = Vec3::new(
        rgb.dot(Vec3::new(0.412_221_46, 0.536_332_55, 0.051_445_995)),
        rgb.dot(Vec3::new(0.211_903_5, 0.680_699_5, 0.107_396_96)),
        rgb.dot(Vec3::new(0.088_302_46, 0.281_718_85, 0.629_978_7)),
    )
    .map(f32::cbrt);
    Vec3::new(
        lms.dot(Vec3::new(0.210_454_26, 0.793_617_8, -0.004_072_047)),
        lms.dot(Vec3::new(1.977_998_5, -2.428_592_2, 0.450_593_7)),
        lms.dot(Vec3::new(0.025_904_037, 0.782_771_77, -0.808_675_77)),
    )
}

#[cfg(test)]
mod tests {
    use glam::UVec2;

    use super::*;

    const T: u32 = 16;
    const GRASS: [u8; 4] = [90, 170, 70, 255];
    const SAND: [u8; 4] = [225, 185, 120, 255];
    const ROCK: [u8; 4] = [120, 120, 130, 255];
    const CLEAR: [u8; 4] = [0; 4];

    struct Sheet {
        width: u32,
        pixels: Vec<u8>,
    }

    impl Sheet {
        /// One row of tiles, each painted by `paint(tile, x, y)`.
        fn new(tiles: u32, paint: impl Fn(u32, u32, u32) -> [u8; 4]) -> Self {
            let width = tiles * T;
            let mut pixels = Vec::with_capacity((width * T * 4) as usize);
            for y in 0..T {
                for x in 0..width {
                    pixels.extend(paint(x / T, x % T, y));
                }
            }
            Self { width, pixels }
        }

        fn input<'a>(&'a self, name: &'a str) -> SheetInput<'a> {
            SheetInput {
                name,
                image: RgbaImage::new(self.width, T, &self.pixels).expect("sized"),
                layout: GridLayout {
                    tile: UVec2::splat(T),
                    margin: UVec2::ZERO,
                    spacing: UVec2::ZERO,
                    columns: self.width / T,
                    rows: 1,
                },
            }
        }
    }

    fn labels(analysis: &TerrainAnalysis, tile: u32) -> TileLabels {
        *analysis
            .tile(TileAddress { sheet: 0, tile })
            .expect("labelled")
    }

    fn peers(analysis: &TerrainAnalysis, tile: u32) -> [TerrainId; 9] {
        labels(analysis, tile).terrains.peers
    }

    fn pure(analysis: &TerrainAnalysis, id: TerrainId) -> Vec<u32> {
        let terrain = analysis.terrain(id).expect("terrain");
        terrain.pure_tiles.iter().map(|t| t.tile).collect()
    }

    fn blob(x: u32, y: u32) -> bool {
        (5..11).contains(&x) && (5..11).contains(&y)
    }

    /// Pure grass, pure sand, grass with a sand right quarter, grass with a
    /// sand top-left corner.
    fn transitions() -> Sheet {
        Sheet::new(4, |tile, x, y| match tile {
            1 => SAND,
            2 if x >= 12 => SAND,
            3 if x < 4 && y < 4 => SAND,
            _ => GRASS,
        })
    }

    #[test]
    fn two_flat_colours_and_transitions_give_two_terrains() {
        let sheet = transitions();
        let found = find_terrains(&[sheet.input("art/meadow.png")]);
        assert_eq!(found.terrains.len(), 2);
        let (g, s) = (1, 2);
        assert_eq!(peers(&found, 0), [g; 9]);
        assert_eq!(peers(&found, 1), [s; 9]);
        assert_eq!(peers(&found, 2), [g, g, s, g, g, s, g, g, s]);
        assert_eq!(peers(&found, 3), [s, g, g, g, g, g, g, g, g]);
        let confidence = labels(&found, 2).confidence;
        assert!(confidence.iter().all(|&c| c > 0.5), "{confidence:?}");
        let grass = found.terrain(g).expect("grass");
        assert_eq!(grass.name, "grass");
        assert_eq!(found.terrain(s).expect("sand").name, "sand");
        assert!(
            grass
                .swatch
                .iter()
                .zip(GRASS)
                .all(|(a, b)| a.abs_diff(b) <= 1)
        );
        assert_eq!(pure(&found, g), [0]);
        assert_eq!(pure(&found, s), [1]);
    }

    #[test]
    fn terrain_seen_only_in_corners_has_no_pure_tile() {
        let corner = |x: u32, y: u32| !(4..12).contains(&x) && !(4..12).contains(&y);
        let sheet = Sheet::new(3, |tile, x, y| match tile {
            1 if corner(x, y) => SAND,
            2 if x < 4 && y < 4 => SAND,
            _ => GRASS,
        });
        let found = find_terrains(&[sheet.input("corners.png")]);
        assert_eq!(found.terrains.len(), 2);
        assert_eq!(peers(&found, 1), [2, 1, 2, 1, 1, 1, 2, 1, 2]);
        assert_eq!(pure(&found, 1), [0]);
        assert!(pure(&found, 2).is_empty());
    }

    #[test]
    fn overlay_set_labels_its_transparent_side_empty() {
        let sheet = Sheet::new(4, |tile, x, y| {
            let opaque = match tile {
                1 => y >= 8,
                2 => x >= 8 && y >= 8,
                3 => x >= 8 || y >= 8,
                _ => true,
            };
            if opaque { GRASS } else { CLEAR }
        });
        let found = find_terrains(&[sheet.input("overlay.png")]);
        assert_eq!(found.terrains.len(), 1);
        assert_eq!(pure(&found, 1), [0]);
        for tile in 1..4 {
            assert_eq!(labels(&found, tile).kind, TileKind::Terrain, "tile {tile}");
        }
        let edge = peers(&found, 1);
        assert_eq!(edge[..3], [0, 0, 0]);
        assert_eq!(edge[6..], [1, 1, 1]);
        let corner = labels(&found, 2);
        assert_eq!(corner.terrains.peers[..4], [0, 0, 0, 0]);
        assert_eq!(corner.terrains.get(Peer::BottomLeft), 0);
        assert_eq!(corner.terrains.get(Peer::BottomRight), 1);
        assert_eq!(corner.confidence[Peer::TopLeft.index()], 1.0);
        assert_eq!(peers(&found, 3), [0, 1, 1, 1, 1, 1, 1, 1, 1]);
    }

    #[test]
    fn translucent_overlay_is_a_terrain() {
        let water = [40, 120, 220, 102];
        let sheet = Sheet::new(
            2,
            |tile, _, y| if tile == 1 && y < 8 { CLEAR } else { water },
        );
        let found = find_terrains(&[sheet.input("water.png")]);
        assert_eq!(found.terrains.len(), 1);
        assert_eq!(peers(&found, 0), [1; 9]);
        assert_eq!(peers(&found, 1)[6..], [1, 1, 1]);
        assert_eq!(peers(&found, 1)[..3], [0, 0, 0]);
    }

    #[test]
    fn mostly_transparent_sprite_is_decor() {
        let sheet = Sheet::new(4, |tile, x, y| match tile {
            0 | 1 => GRASS,
            2 if blob(x, y) => ROCK,
            _ => CLEAR,
        });
        let found = find_terrains(&[sheet.input("props.png")]);
        assert_eq!(found.terrains.len(), 1);
        assert_eq!(labels(&found, 2).kind, TileKind::Decor);
        assert!(found.tile(TileAddress { sheet: 0, tile: 3 }).is_none());
    }

    #[test]
    fn prop_over_terrain_is_decor_and_founds_no_terrain() {
        let sheet = Sheet::new(
            5,
            |tile, x, y| {
                if tile > 0 && blob(x, y) { ROCK } else { GRASS }
            },
        );
        let found = find_terrains(&[sheet.input("rocks.png")]);
        assert_eq!(found.terrains.len(), 1);
        assert_eq!(labels(&found, 0).kind, TileKind::Terrain);
        for tile in 1..5 {
            let rock = labels(&found, tile);
            assert_eq!(rock.kind, TileKind::Decor, "tile {tile}");
            assert_eq!(rock.terrains.get(Peer::TopLeft), 1);
        }
    }

    #[test]
    fn a_single_flat_object_tile_founds_no_terrain() {
        let sheet = Sheet::new(3, |tile, _, _| if tile == 2 { ROCK } else { GRASS });
        let found = find_terrains(&[sheet.input("crate.png")]);
        assert_eq!(found.terrains.len(), 1);
        assert_eq!(pure(&found, 1), [0, 1]);
    }

    #[test]
    fn noisy_tile_of_one_terrain_is_pure() {
        let speck = [60, 140, 45, 255];
        let sheet = Sheet::new(3, |tile, x, y| {
            let noisy = tile == 2 && (x * 7 + y * 3) % 5 == 0;
            if noisy { speck } else { GRASS }
        });
        let found = find_terrains(&[sheet.input("noisy.png")]);
        assert_eq!(found.terrains.len(), 1);
        assert_eq!(labels(&found, 2).kind, TileKind::Terrain);
        assert_eq!(pure(&found, 1), [0, 1, 2]);
    }

    #[test]
    fn shaded_terrain_is_one_terrain() {
        let shades = [
            [80, 160, 60, 255],
            [90, 170, 70, 255],
            [100, 180, 80, 255],
            [110, 190, 90, 255],
        ];
        let sheet = Sheet::new(4, |tile, _, _| shades[tile as usize]);
        let found = find_terrains(&[sheet.input("shaded.png")]);
        let span = srgb_to_oklab([110, 190, 90, 255]).x - srgb_to_oklab([80, 160, 60, 255]).x;
        assert!(span > 0.065, "{span}");
        assert_eq!(found.terrains.len(), 1);
    }

    #[test]
    fn exact_ties_follow_first_appearance() {
        let sheet = Sheet::new(5, |tile, x, y| match tile {
            0 if (x + y) % 2 == 0 => GRASS,
            0..=2 => SAND,
            _ => GRASS,
        });
        let found = find_terrains(&[sheet.input("tie.png")]);
        assert_eq!(found.terrains.len(), 2);
        assert_eq!(peers(&found, 0), [1; 9]);
        assert_eq!(labels(&found, 0).confidence, [0.0; 9]);
        let (sand, grass) = (peers(&found, 1)[0], peers(&found, 3)[0]);
        let mut ids = [sand, grass];
        ids.sort_unstable();
        assert_eq!(ids, [1, 2]);
        assert_eq!(found, find_terrains(&[sheet.input("tie.png")]));
    }

    #[test]
    fn analysis_is_deterministic_and_independent_of_sheet_order() {
        let a = transitions();
        let b = Sheet::new(
            2,
            |tile, x, _| if tile == 1 && x < 8 { SAND } else { GRASS },
        );
        let once = find_terrains(&[a.input("a.png")]);
        assert_eq!(once, find_terrains(&[a.input("a.png")]));

        let twice = find_terrains(&[a.input("a.png"), a.input("a.png")]);
        assert_eq!(twice.terrains.len(), once.terrains.len());
        for sheet in 0..2 {
            for tile in 0..4 {
                let lone = once.tile(TileAddress { sheet: 0, tile }).expect("tile");
                let copy = twice.tile(TileAddress { sheet, tile }).expect("tile");
                assert_eq!((copy.kind, copy.terrains), (lone.kind, lone.terrains));
                let close = copy.confidence.iter().zip(lone.confidence);
                assert!(close.into_iter().all(|(a, b)| (a - b).abs() < 1e-4));
            }
        }

        let colours = |found: &TerrainAnalysis| {
            let mut c: Vec<[u32; 3]> = found
                .terrains
                .iter()
                .map(|t| t.oklab.to_array().map(f32::to_bits))
                .collect();
            c.sort_unstable();
            c
        };
        let ab = find_terrains(&[a.input("a.png"), b.input("b.png")]);
        let ba = find_terrains(&[b.input("b.png"), a.input("a.png")]);
        assert_eq!(colours(&ab), colours(&ba));
    }

    #[test]
    fn region_between_two_colours_has_low_confidence() {
        let mid = [157, 177, 95, 255];
        let sheet = Sheet::new(4, |tile, x, _| match tile {
            1 | 2 => SAND,
            3 if x == T - 1 => mid,
            _ => GRASS,
        });
        let found = find_terrains(&[sheet.input("mix.png")]);
        let tile = labels(&found, 3);
        assert_eq!(found.terrains.len(), 2);
        let right = tile.confidence[Peer::Right.index()];
        assert!(right < 0.5, "{right}");
        assert!(tile.confidence[Peer::Left.index()] > 0.9);
    }

    #[test]
    fn layout_outside_the_image_yields_no_tiles() {
        let sheet = transitions();
        let mut input = sheet.input("a.png");
        input.layout.rows = 2;
        let found = find_terrains(&[input]);
        assert!(found.tiles[0].is_empty());
        assert!(found.terrains.is_empty());
    }

    #[test]
    fn regions_tile_the_corners_edges_and_centre() {
        let rects = region_rects(16, 16);
        assert_eq!(rects[Peer::TopLeft.index()], [0, 0, 2, 2]);
        assert_eq!(rects[Peer::Top.index()], [2, 0, 12, 1]);
        assert_eq!(rects[Peer::Left.index()], [0, 2, 1, 12]);
        assert_eq!(rects[Peer::Centre.index()], [6, 6, 4, 4]);
        assert_eq!(rects[Peer::Right.index()], [15, 2, 1, 12]);
        assert_eq!(rects[Peer::BottomRight.index()], [14, 14, 2, 2]);
    }

    fn material_of(hex: u32) -> &'static str {
        let [_, r, g, b] = hex.to_be_bytes();
        material(srgb_to_oklab([r, g, b, 255]))
    }

    #[test]
    fn colours_name_ground_materials() {
        assert_eq!(material_of(0x84c669), "grass");
        assert_eq!(material_of(0x2f6b2a), "grass");
        assert_eq!(material_of(0x3a7bd5), "water");
        assert_eq!(material_of(0x787882), "stone");
        assert_eq!(material_of(0xf0f0f0), "snow");
        assert_eq!(material_of(0xdde4ee), "snow");
        assert_eq!(material_of(0x101010), "dark");
        assert_eq!(material_of(0x1a3a1a), "dark");
        assert_eq!(material_of(0x2a1a10), "dark");
        assert_eq!(material_of(0xe7a16a), "sand");
        assert_eq!(material_of(0xe1b978), "sand");
        assert_eq!(material_of(0xcb7c51), "dirt");
        assert_eq!(material_of(0x5c4033), "dirt");
        assert_eq!(material_of(0xc040c0), "purple");
        assert_eq!(material_of(0xc03050), "purple");
    }

    fn found(id: TerrainId, hex: u32) -> FoundTerrain {
        let [_, r, g, b] = hex.to_be_bytes();
        FoundTerrain {
            id,
            oklab: srgb_to_oklab([r, g, b, 255]),
            swatch: [r, g, b],
            variance: 0.0,
            pure_tiles: Vec::new(),
            name: "old".into(),
        }
    }

    fn named(colours: &[u32], kept: &[TerrainId], sets: &[(TerrainId, TerrainId)]) -> Vec<String> {
        let mut terrains: Vec<FoundTerrain> =
            (1..).zip(colours).map(|(id, &c)| found(id, c)).collect();
        name_terrains(&mut terrains, kept, &[1], sets, &[]);
        terrains.into_iter().map(|t| t.name).collect()
    }

    const FARM: [u32; 5] = [0x84c669, 0xe7a16a, 0xcb7c51, 0x787882, 0xe1b978];

    #[test]
    fn path_materials_over_the_background_are_paths() {
        let sets = [(2, 1), (3, 1), (4, 1), (5, 1)];
        let names = named(&FARM, &[1, 2, 3, 4, 5], &sets);
        let want = [
            "grass",
            "sand path",
            "dirt path",
            "stone path",
            "sand path 2",
        ];
        assert_eq!(names, want);
        let names = named(&FARM, &[1, 2, 3, 4], &[(4, 2)]);
        assert_eq!(names[3], "stone");
    }

    #[test]
    fn a_pond_over_grass_and_snow_over_dirt_keep_their_materials() {
        let pond = named(
            &[0x84c669, 0x3a7bd5, 0x2f6b2a],
            &[1, 2, 3],
            &[(2, 1), (3, 1)],
        );
        assert_eq!(pond, ["grass", "water", "grass 2"]);
        let snow = named(
            &[0x5c4033, 0xf0f0f0, 0x84c669],
            &[1, 2, 3],
            &[(2, 1), (3, 1)],
        );
        assert_eq!(snow, ["dirt", "snow", "grass path"]);
    }

    #[test]
    fn dropped_terrains_are_named_after_the_kept_ones() {
        let names = named(&FARM, &[1, 3, 5], &[(5, 1)]);
        assert_eq!(names, ["grass", "sand", "dirt", "stone", "sand path"]);
        let names = named(&[0x84c669, 0x2f6b2a], &[2], &[]);
        assert_eq!(names, ["grass 2", "grass"]);
    }

    #[test]
    fn reserved_names_are_stepped_around() {
        let mut terrains = [found(1, 0x84c669), found(2, 0xe7a16a)];
        let reserved = ["grass".to_string(), "grass 3".to_string()];
        name_terrains(&mut terrains, &[1, 2], &[1], &[(2, 1)], &reserved);
        assert_eq!(
            [&terrains[0].name, &terrains[1].name],
            ["grass 2", "sand path"]
        );
    }

    #[test]
    fn unique_suffixes_from_two() {
        let taken = ["sand".to_string(), "sand 2".to_string()];
        assert_eq!(unique("sand", &taken), "sand 3");
        assert_eq!(unique("dirt", &taken), "dirt");
    }
}
