//! Suggestions: blocks of a sheet that look like a layout of two terrains,
//! and groups of tiles that look like one object, for the user to confirm.
//!
//! - Free: a tile in no object of the setup that records no set and holds at
//!   most one terrain.
//! - Blocks: windows the shape of a layout, every cell free, holding at least
//!   the layout's tile count of one fitted set's tiles or its terrains' pure
//!   tiles (with a transparent outer, also blank tiles and tiles of only its inner terrain; see `layout_fit`), the set holding most of them read for the
//!   window. Each layout and placement of that shape is fitted in both orders
//!   of the pair (see `block_fit`); a window is kept when a fit is good,
//!   scored by its mean confidence. Windows are taken largest first, then
//!   most confident, never overlapping one taken.
//! - Terrains: a found terrain is the user's terrain whose icon tile centres
//!   it, and transparent is 0, never inner when the other order fits; a block is fitted against the icons it has,
//!   else against found pure tiles, or transparent.
//! - Objects: stamps (see `stamp`) of [`MIN_OBJECT`] tiles or more whose
//!   bounds are free and outside the blocks.
//! - Order: blocks naming both terrains, then one, then none, each by
//!   confidence, then objects by tile count; ties by sheet, row and column.

use std::collections::BTreeSet;

use glam::UVec2;

use crate::{
    BlockEvidence, LayoutTemplate, Placement, SetupError, SheetInput, TerrainId, TerrainLook,
    TileAddress, TilesetProposal, TilesetSetup, find_stamps, find_terrains, fit_layouts,
};

/// Window shapes tried, largest first.
const SHAPES: [UVec2; 5] = [
    UVec2::new(8, 6),
    UVec2::new(4, 4),
    UVec2::new(5, 3),
    UVec2::new(3, 3),
    UVec2::new(2, 2),
];
/// Fewest tiles of a suggested object.
pub const MIN_OBJECT: usize = 3;

/// A layout a block fits and how well, from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutGuess {
    pub template: LayoutTemplate,
    pub placement: Placement,
    pub confidence: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SuggestionKind {
    /// Layouts it fits, best first, and the user's terrains its inner and
    /// outer look like: 0 for transparent, `None` when unknown.
    Block {
        layouts: Vec<LayoutGuess>,
        inner: Option<TerrainId>,
        outer: Option<TerrainId>,
    },
    /// The tiles of one object, as offsets from the origin.
    Object { cells: Vec<UVec2> },
}

/// A block or object of a sheet worth setting up.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    /// Index into the setup's sheets.
    pub sheet: usize,
    /// Column and row of the top-left tile.
    pub origin: UVec2,
    pub size: UVec2,
    pub kind: SuggestionKind,
}

impl Suggestion {
    /// Its best layout's confidence; 0 for an object.
    pub fn confidence(&self) -> f32 {
        match &self.kind {
            SuggestionKind::Block { layouts, .. } => layouts.first().map_or(0.0, |l| l.confidence),
            SuggestionKind::Object { .. } => 0.0,
        }
    }

    /// Whether it is a block naming one of the user's terrains.
    pub fn named(&self) -> bool {
        self.named_terrains() > 0
    }

    /// How many of the user's terrains it names, transparency aside.
    pub fn named_terrains(&self) -> usize {
        let SuggestionKind::Block { inner, outer, .. } = &self.kind else {
            return 0;
        };
        [inner, outer]
            .iter()
            .filter(|id| id.is_some_and(|id| id != 0))
            .count()
    }

    /// The tiles of its bounds, on a sheet `columns` wide.
    pub fn tiles(&self, columns: u32) -> impl Iterator<Item = TileAddress> + '_ {
        let sheet = self.sheet;
        cells(self.origin, self.size, columns).map(move |tile| TileAddress { sheet, tile })
    }

    /// Whether no tile of its bounds is `taken`.
    pub fn free_in(&self, taken: &BTreeSet<TileAddress>, columns: u32) -> bool {
        self.tiles(columns).all(|a| !taken.contains(&a))
    }
}

/// Tiles in an object of `setup`, or recording a set, or holding two terrains.
pub fn taken_tiles(setup: &TilesetSetup) -> BTreeSet<TileAddress> {
    let in_sets = (setup.tiles.iter()).filter(|(_, t)| {
        let peers = t.terrains.peers;
        t.set.is_some() || peers.iter().any(|p| *p != peers[0])
    });
    let objects = setup.stamps.iter().flat_map(|s| s.value.tiles());
    in_sets.map(|(a, _)| *a).chain(objects).collect()
}

/// The user's inner and outer terrain of a pair, as in [`SuggestionKind`].
type Names = (Option<TerrainId>, Option<TerrainId>);

/// A fitted set read for windows: its tiles, looks and user terrains.
struct Pair {
    members: BTreeSet<TileAddress>,
    looks: (TerrainLook, TerrainLook),
    names: Names,
    /// Whether its outer terrain is transparent, so blank tiles and tiles
    /// of only its inner terrain and transparency are its own.
    clear: bool,
    inner: TerrainId,
}

/// Suggestions for the sheets `inputs`, in the order of `setup`'s sheets;
/// `icons` gives the icon tile of each user terrain that has one.
pub fn suggest_blocks(
    inputs: &[SheetInput],
    setup: &TilesetSetup,
    icons: &[(TerrainId, TileAddress)],
) -> Result<Vec<Suggestion>, SetupError> {
    let analysis = find_terrains(inputs);
    let proposal = fit_layouts(&analysis, inputs)?;
    let taken = taken_tiles(setup);
    let user = |found: TerrainId| {
        let mut icons = icons.iter();
        let centred = |at: &TileAddress| analysis.tile(*at).map(|l| l.terrains.centre());
        let icon = icons.find(|(_, at)| centred(at) == Some(found)).copied();
        if found == 0 {
            Some((0, None))
        } else {
            icon.map(|(id, at)| (id, Some(at)))
        }
    };
    let empty = inputs.first().map(|i| TerrainLook::empty(i.layout.tile));
    let pairs: Vec<Pair> = (proposal.sets.iter())
        .filter_map(|set| {
            let look = |found: TerrainId| {
                let icon = user(found).and_then(|(_, at)| at);
                match icon.or_else(|| pure_tile(&proposal, found)) {
                    Some(at) => TerrainLook::of_tile(inputs.get(at.sheet)?, at.tile),
                    None => empty.clone(),
                }
            };
            let names = (user(set.inner).map(|u| u.0), user(set.outer).map(|u| u.0));
            let tiles = set.tiles.iter().map(|t| t.address);
            Some(Pair {
                members: members(&proposal, set.inner, set.outer, tiles),
                looks: (look(set.inner)?, look(set.outer)?),
                names,
                clear: set.outer == 0,
                inner: set.inner,
            })
        })
        .collect();
    let mut windows = Vec::new();
    for (sheet, input) in inputs.iter().enumerate() {
        let (columns, rows) = (input.layout.columns, input.layout.rows);
        for size in SHAPES {
            let fits = LayoutTemplate::ALL
                .into_iter()
                .filter(|t| t.shape() == size);
            let Some(least) = fits.map(LayoutTemplate::tile_count).min() else {
                continue;
            };
            for y in 0..=rows.saturating_sub(size.y) {
                for x in 0..=columns.saturating_sub(size.x) {
                    let origin = UVec2::new(x, y);
                    let block: Vec<TileAddress> = cells(origin, size, columns)
                        .map(|tile| TileAddress { sheet, tile })
                        .collect();
                    if block.iter().any(|a| taken.contains(a)) {
                        continue;
                    }
                    let held = |p: &&Pair| {
                        let blank = |a: &TileAddress| {
                            let peers = analysis.tile(*a).map(|l| l.terrains.peers);
                            p.clear
                                && peers.is_none_or(|t| t.iter().all(|t| [0, p.inner].contains(t)))
                        };
                        let ours = |a: &&TileAddress| p.members.contains(a) || blank(a);
                        block.iter().filter(ours).count()
                    };
                    let pair = (pairs.iter().rev())
                        .filter(|p| held(p) >= least)
                        .max_by_key(held);
                    let Some(Pair { looks, names, .. }) = pair else {
                        continue;
                    };
                    let ahead = guesses(input, (origin, size), (&looks.0, &looks.1));
                    let behind = guesses(input, (origin, size), (&looks.1, &looks.0));
                    let best = [
                        ahead.map(|g| (g, *names)),
                        behind.map(|g| (g, (names.1, names.0))),
                    ]
                    .into_iter()
                    .flatten()
                    .max_by(|a, b| {
                        let opaque = |g: &(Vec<LayoutGuess>, Names)| g.1.0 != Some(0);
                        (opaque(a).cmp(&opaque(b)))
                            .then(a.0[0].confidence.total_cmp(&b.0[0].confidence))
                    });
                    if let Some((layouts, (inner, outer))) = best {
                        windows.push(Suggestion {
                            sheet,
                            origin,
                            size,
                            kind: SuggestionKind::Block {
                                layouts,
                                inner,
                                outer,
                            },
                        });
                    }
                }
            }
        }
    }
    windows.sort_by(|a, b| {
        let area = |s: &Suggestion| s.size.x * s.size.y;
        (area(b).cmp(&area(a)))
            .then(b.confidence().total_cmp(&a.confidence()))
            .then(position(a).cmp(&position(b)))
    });
    let columns = |sheet: usize| inputs.get(sheet).map_or(1, |i| i.layout.columns);
    let mut covered: BTreeSet<TileAddress> = BTreeSet::new();
    let mut blocks = Vec::new();
    for window in windows {
        if !window.free_in(&covered, columns(window.sheet)) {
            continue;
        }
        covered.extend(window.tiles(columns(window.sheet)));
        blocks.push(window);
    }
    blocks.sort_by(|a, b| {
        (b.named_terrains().cmp(&a.named_terrains()))
            .then(b.confidence().total_cmp(&a.confidence()))
            .then(position(a).cmp(&position(b)))
    });
    covered.extend(taken);
    let mut objects: Vec<Suggestion> = find_stamps(&analysis, inputs, &proposal)?
        .into_iter()
        .filter(|s| s.cells.len() >= MIN_OBJECT)
        .map(|s| Suggestion {
            sheet: s.sheet,
            origin: s.origin,
            size: s.size,
            kind: SuggestionKind::Object {
                cells: s.cells.iter().map(|(at, _)| *at).collect(),
            },
        })
        .filter(|s| s.free_in(&covered, columns(s.sheet)))
        .collect();
    let count = |s: &Suggestion| match &s.kind {
        SuggestionKind::Object { cells } => cells.len(),
        SuggestionKind::Block { .. } => 0,
    };
    objects.sort_by(|a, b| count(b).cmp(&count(a)).then(position(a).cmp(&position(b))));
    blocks.extend(objects);
    Ok(blocks)
}

/// The layouts of the block's shape it fits well read with `looks`, best
/// first; `None` when none does.
fn guesses(
    input: &SheetInput,
    (origin, size): (UVec2, UVec2),
    looks: (&TerrainLook, &TerrainLook),
) -> Option<Vec<LayoutGuess>> {
    let evidence = BlockEvidence::measure(input, (origin, size), looks)?;
    let mut guesses: Vec<LayoutGuess> = (LayoutTemplate::ALL.into_iter())
        .filter(|t| t.shape() == size)
        .flat_map(|t| t.placements().iter().map(move |p| (t, *p)))
        .filter_map(|(template, placement)| {
            let fit = evidence.fit((template, placement))?;
            let n = fit.confidence.len().max(1) as f32;
            let confidence = fit.confidence.iter().sum::<f32>() / n;
            fit.good().then_some(LayoutGuess {
                template,
                placement,
                confidence,
            })
        })
        .collect();
    guesses.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));
    Some(guesses).filter(|g| !g.is_empty())
}

fn position(s: &Suggestion) -> (usize, u32, u32) {
    (s.sheet, s.origin.y, s.origin.x)
}

/// Tile ids of the `size` block at `origin` of a sheet `columns` wide.
fn cells(origin: UVec2, size: UVec2, columns: u32) -> impl Iterator<Item = u32> + Clone {
    (0..size.y).flat_map(move |y| (0..size.x).map(move |x| (origin.y + y) * columns + origin.x + x))
}

/// The tiles of a set and the pure tiles of its two terrains.
fn members(
    proposal: &TilesetProposal,
    inner: TerrainId,
    outer: TerrainId,
    tiles: impl Iterator<Item = TileAddress>,
) -> BTreeSet<TileAddress> {
    let pure = (proposal.terrains.iter())
        .filter(|t| t.id == inner || t.id == outer)
        .flat_map(|t| t.pure_tiles.iter().copied());
    tiles.chain(pure).collect()
}

fn pure_tile(proposal: &TilesetProposal, found: TerrainId) -> Option<TileAddress> {
    let terrain = proposal.terrains.iter().find(|t| t.id == found)?;
    terrain.pure_tiles.first().copied()
}
