//! Auto setup: slices the sheets of one import with one tile size, lists
//! animation strips instead of analysing them, finds terrains, fits layouts,
//! groups stamps, reconciles the proposal with them, and reports the result
//! as deterministic plain text.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use glam::UVec2;

use crate::terrain_find::name_terrains;
use crate::{
    AnimationStrip, FitError, GridLayout, LOOSE_FRAME_MATCH, LayoutTemplate, MatchMode, RgbaImage,
    SharedDetection, SheetInput, Stamp, TerrainAnalysis, TerrainId, TileAddress, TileKind,
    TileShape, TilesetProposal, detect_grid_shared, find_stamps, find_terrains, fit_layouts,
    pair_animations, reconcile, trace_proposal,
};

/// Everything auto setup proposes for one import.
#[derive(Clone, Debug, PartialEq)]
pub struct AutoSetup {
    /// Input index of each analysed sheet; [`TileAddress::sheet`] and
    /// [`Stamp::sheet`] index this.
    pub sheets: Vec<usize>,
    pub analysis: TerrainAnalysis,
    pub proposal: TilesetProposal,
    pub stamps: Vec<Stamp>,
    /// Collision shapes traced for the solid terrains asked for, by tile.
    pub shapes: BTreeMap<TileAddress, TileShape>,
    pub report: SetupReport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupError {
    /// No tile size scores in any sheet.
    NoGrid,
    Fit(FitError),
    /// A sheet index that names no input; a pipeline bug, not bad input.
    UnknownSheet(usize),
    /// A terrain missing from the analysis or the setup.
    UnknownTerrain(TerrainId),
    /// No terrain set of the setup has this inner and outer terrain.
    UnknownSet(TerrainId, TerrainId),
    /// Every terrain id is taken; ids are never reused.
    TooManyTerrains,
    /// The input at this index repeats an earlier sheet name.
    DuplicateSheet(usize),
    /// A set of a terrain over itself.
    SameTerrain(TerrainId),
    /// A terrain set with this inner and outer terrain already exists.
    SetExists(TerrainId, TerrainId),
    /// A tile that holds no peer of the terrain.
    NotOnTile(TileAddress, TerrainId),
    /// A block of tiles that leaves its sheet's grid.
    OutsideSheet {
        origin: UVec2,
        size: UVec2,
    },
    /// A tile that belongs to the stamp at this origin.
    InObject {
        tile: TileAddress,
        origin: UVec2,
    },
    /// No stamp covers this cell of the sheet.
    NoStamp {
        sheet: usize,
        cell: UVec2,
    },
}

impl fmt::Display for SetupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SetupError::NoGrid => write!(f, "no tile grid found in any sheet"),
            SetupError::Fit(e) => write!(f, "{e}"),
            SetupError::UnknownSheet(i) => write!(f, "sheet index {i} names no input"),
            SetupError::UnknownTerrain(id) => write!(f, "terrain {id} is not known"),
            SetupError::UnknownSet(inner, outer) => {
                write!(f, "no terrain set {inner} over {outer}")
            }
            SetupError::TooManyTerrains => write!(f, "no terrain ids left"),
            SetupError::DuplicateSheet(i) => write!(f, "input {i} repeats a sheet name"),
            SetupError::SameTerrain(id) => write!(f, "terrain {id} cannot border itself"),
            SetupError::SetExists(inner, outer) => {
                write!(f, "terrain set {inner} over {outer} already exists")
            }
            SetupError::NotOnTile(at, id) => {
                write!(
                    f,
                    "tile {} of sheet {} holds no terrain {id}",
                    at.tile, at.sheet
                )
            }
            SetupError::OutsideSheet { origin, size } => write!(
                f,
                "{}x{} tiles at column {} row {} leave the sheet",
                size.x, size.y, origin.x, origin.y
            ),
            SetupError::InObject { tile, origin } => write!(
                f,
                "tile {} of sheet {} is part of the object at column {} row {}",
                tile.tile, tile.sheet, origin.x, origin.y
            ),
            SetupError::NoStamp { sheet, cell } => write!(
                f,
                "no object covers column {} row {} of sheet {sheet}",
                cell.x, cell.y
            ),
        }
    }
}

impl std::error::Error for SetupError {}

impl From<FitError> for SetupError {
    fn from(e: FitError) -> Self {
        SetupError::Fit(e)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SheetReport {
    pub name: String,
    /// The layout at the shared tile size with its score in `0..=1`.
    pub grid: Option<(GridLayout, f32)>,
    /// Set when the sheet is an animation strip of another.
    pub animates: Option<StripReport>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StripReport {
    pub base: String,
    pub frames: u32,
    /// Animated base tiles.
    pub tiles: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerrainReport {
    pub id: TerrainId,
    pub name: String,
    pub swatch: [u8; 3],
    pub background: bool,
    pub pure_tiles: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetReport {
    pub mode: MatchMode,
    pub inner: TerrainId,
    pub outer: TerrainId,
    pub covered: usize,
    pub total: usize,
    pub missing: usize,
    pub template: Option<LayoutTemplate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StampReport {
    pub sheet: String,
    pub origin: UVec2,
    pub size: UVec2,
    pub tiles: usize,
}

/// Traced collision for the terrains marked Solid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollisionReport {
    pub solid: Vec<TerrainId>,
    pub shapes: usize,
    pub half_tiles: usize,
}

/// What auto setup found, by sheet name; rendered by `Display`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SetupReport {
    /// Every input sheet, in input order.
    pub sheets: Vec<SheetReport>,
    pub terrains: Vec<TerrainReport>,
    pub sets: Vec<SetReport>,
    pub stamps: Vec<StampReport>,
    /// Unassigned terrain tiles in no stamp, per sheet.
    pub unassigned: Vec<(String, Vec<u32>)>,
    /// Decor tiles in no stamp.
    pub decor: usize,
    pub low_confidence: Vec<(String, Vec<u32>)>,
    /// Present when Solid terrains were given.
    pub collision: Option<CollisionReport>,
}

/// Runs auto setup over named sheets of one import. Collision is traced
/// only when `solid` names terrains. The report is built from the proposal
/// after [`reconcile`], so the two always agree.
pub fn auto_setup(
    inputs: &[(&str, RgbaImage)],
    solid: &[TerrainId],
) -> Result<AutoSetup, SetupError> {
    auto_setup_reserving(inputs, solid, &[])
}

/// [`auto_setup`] naming no terrain one of `reserved`.
pub(crate) fn auto_setup_reserving(
    inputs: &[(&str, RgbaImage)],
    solid: &[TerrainId],
    reserved: &[String],
) -> Result<AutoSetup, SetupError> {
    let images: Vec<RgbaImage> = inputs.iter().map(|(_, image)| *image).collect();
    let shared = detect_grid_shared(&images).ok_or(SetupError::NoGrid)?;
    setup_on_grid(inputs, &shared, solid, reserved)
}

/// [`auto_setup`] over the grid [`detect_grid_shared`] already found in
/// `inputs`, in input order.
pub fn auto_setup_on_grid(
    inputs: &[(&str, RgbaImage)],
    shared: &SharedDetection,
    solid: &[TerrainId],
) -> Result<AutoSetup, SetupError> {
    setup_on_grid(inputs, shared, solid, &[])
}

fn setup_on_grid(
    inputs: &[(&str, RgbaImage)],
    shared: &SharedDetection,
    solid: &[TerrainId],
    reserved: &[String],
) -> Result<AutoSetup, SetupError> {
    let layout = |i: usize| shared.layouts.get(i).copied().flatten();
    let (gridded, gridded_inputs): (Vec<usize>, Vec<SheetInput>) = (inputs.iter().enumerate())
        .filter_map(|(i, &(name, image))| {
            let layout = layout(i)?.0;
            Some((
                i,
                SheetInput {
                    name,
                    image,
                    layout,
                },
            ))
        })
        .unzip();
    let gridded_images: Vec<RgbaImage> = gridded_inputs.iter().map(|s| s.image).collect();
    let layouts: Vec<GridLayout> = gridded_inputs.iter().map(|s| s.layout).collect();
    let input_of = |g: usize| gridded.get(g).copied().ok_or(SetupError::UnknownSheet(g));
    let mut strips: BTreeMap<usize, (usize, AnimationStrip)> = BTreeMap::new();
    for (base, strip, a) in pair_animations(&gridded_images, &layouts, LOOSE_FRAME_MATCH) {
        strips.insert(input_of(strip)?, (input_of(base)?, a));
    }
    let (analysed, sheet_inputs): (Vec<usize>, Vec<SheetInput>) = (gridded.iter().copied())
        .zip(gridded_inputs)
        .filter(|(i, _)| !strips.contains_key(i))
        .unzip();
    let mut analysis = find_terrains(&sheet_inputs);
    let proposal = fit_layouts(&analysis, &sheet_inputs)?;
    let stamps = find_stamps(&analysis, &sheet_inputs, &proposal)?;
    let proposal = reconcile(proposal, &stamps);
    let kept: Vec<TerrainId> = proposal.terrains.iter().map(|t| t.id).collect();
    let background: Vec<TerrainId> = (proposal.terrains.iter())
        .filter(|t| t.background)
        .map(|t| t.id)
        .collect();
    let sets: Vec<(TerrainId, TerrainId)> =
        (proposal.sets.iter()).map(|s| (s.inner, s.outer)).collect();
    name_terrains(&mut analysis.terrains, &kept, &background, &sets, reserved);
    let shapes = if solid.is_empty() {
        BTreeMap::new()
    } else {
        let is_solid = |id: TerrainId| solid.contains(&id);
        trace_proposal(&analysis, &sheet_inputs, &proposal, is_solid, &[])?
    };
    let collision = (!solid.is_empty()).then(|| CollisionReport {
        solid: solid.to_vec(),
        shapes: shapes.len(),
        half_tiles: shapes.values().filter(|s| s.half_tile).count(),
    });

    let input_name = |i: usize| {
        let (name, _) = inputs.get(i).ok_or(SetupError::UnknownSheet(i))?;
        Ok::<_, SetupError>(name.to_string())
    };
    let name =
        |sheet: usize| input_name(*analysed.get(sheet).ok_or(SetupError::UnknownSheet(sheet))?);
    let mut sheets = Vec::with_capacity(inputs.len());
    for i in 0..inputs.len() {
        let animates = match strips.get(&i) {
            Some((base, a)) => Some(StripReport {
                base: input_name(*base)?,
                frames: a.frames,
                tiles: a.runs.len(),
            }),
            None => None,
        };
        let grid = layout(i);
        sheets.push(SheetReport {
            name: input_name(i)?,
            grid,
            animates,
        });
    }
    let mut terrains = Vec::with_capacity(proposal.terrains.len());
    for p in &proposal.terrains {
        let t = analysis
            .terrain(p.id)
            .ok_or(SetupError::UnknownTerrain(p.id))?;
        terrains.push(TerrainReport {
            id: p.id,
            name: t.name.clone(),
            swatch: t.swatch,
            background: p.background,
            pure_tiles: p.pure_tiles.len(),
        });
    }
    let sets = (proposal.sets.iter())
        .map(|s| SetReport {
            mode: s.mode,
            inner: s.inner,
            outer: s.outer,
            covered: s.covered,
            total: s.total,
            missing: s.missing.len(),
            template: s.template,
        })
        .collect();
    let by_sheet = |tiles: &[TileAddress]| {
        let mut grouped: BTreeMap<usize, Vec<u32>> = BTreeMap::new();
        for a in tiles {
            grouped.entry(a.sheet).or_default().push(a.tile);
        }
        (grouped.into_iter())
            .map(|(s, t)| Ok((name(s)?, t)))
            .collect::<Result<Vec<_>, SetupError>>()
    };
    let claimed: BTreeSet<TileAddress> = stamps.iter().flat_map(Stamp::tiles).collect();
    let decor = (analysis.tiles.iter().enumerate())
        .flat_map(|(sheet, tiles)| (0..).zip(tiles).map(move |(tile, l)| (sheet, tile, l)))
        .filter(|&(sheet, tile, l)| {
            let decor = l.is_some_and(|l| l.kind == TileKind::Decor);
            decor && !claimed.contains(&TileAddress { sheet, tile })
        })
        .count();
    let mut stamp_reports = Vec::with_capacity(stamps.len());
    for s in &stamps {
        stamp_reports.push(StampReport {
            sheet: name(s.sheet)?,
            origin: s.origin,
            size: s.size,
            tiles: s.cells.len(),
        });
    }
    let report = SetupReport {
        sheets,
        terrains,
        sets,
        stamps: stamp_reports,
        unassigned: by_sheet(&proposal.unassigned)?,
        decor,
        low_confidence: by_sheet(&proposal.low_confidence)?,
        collision,
    };
    Ok(AutoSetup {
        sheets: analysed,
        analysis,
        proposal,
        stamps,
        shapes,
        report,
    })
}

/// `x` to two decimals, never `-0.00`.
fn fixed(x: f32) -> String {
    format!("{:.2}", (x * 100.0).round() / 100.0 + 0.0)
}

fn tiles(f: &mut fmt::Formatter<'_>, title: &str, list: &[(String, Vec<u32>)]) -> fmt::Result {
    let count: usize = list.iter().map(|(_, t)| t.len()).sum();
    writeln!(f, "{title}: {count}")?;
    for (sheet, tiles) in list {
        let ids: Vec<String> = tiles.iter().map(u32::to_string).collect();
        writeln!(f, "  \"{sheet}\": {}", ids.join(" "))?;
    }
    Ok(())
}

impl fmt::Display for SetupReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "sheets: {}", self.sheets.len())?;
        for sheet in &self.sheets {
            write!(f, "  sheet \"{}\": ", sheet.name)?;
            match &sheet.grid {
                Some((g, score)) => write!(
                    f,
                    "tile {}x{}, margin {}x{}, spacing {}x{}, {} columns, {} rows, confidence {}",
                    g.tile.x,
                    g.tile.y,
                    g.margin.x,
                    g.margin.y,
                    g.spacing.x,
                    g.spacing.y,
                    g.columns,
                    g.rows,
                    fixed(*score)
                )?,
                None => write!(f, "no grid")?,
            }
            if let Some(a) = &sheet.animates {
                write!(
                    f,
                    ", animation of \"{}\", {} frames, {} tiles",
                    a.base, a.frames, a.tiles
                )?;
            }
            writeln!(f)?;
        }
        writeln!(f, "terrains: {}", self.terrains.len())?;
        for t in &self.terrains {
            let [r, g, b] = t.swatch;
            let background = if t.background { ", background" } else { "" };
            writeln!(
                f,
                "  terrain {} \"{}\": swatch #{r:02x}{g:02x}{b:02x}{background}, pure tiles {}",
                t.id, t.name, t.pure_tiles
            )?;
        }
        writeln!(f, "terrain sets: {}", self.sets.len())?;
        for s in &self.sets {
            let template = s.template.map_or("none".to_string(), |t| format!("{t:?}"));
            let terrain = |id: TerrainId| match id {
                0 => "empty".to_string(),
                id => id.to_string(),
            };
            writeln!(
                f,
                "  set {} over {}: {:?}, {}/{} shapes, {} missing, template {template}",
                terrain(s.inner),
                terrain(s.outer),
                s.mode,
                s.covered,
                s.total,
                s.missing
            )?;
        }
        writeln!(f, "stamps: {}", self.stamps.len())?;
        for s in &self.stamps {
            writeln!(
                f,
                "  stamp \"{}\" at column {} row {}: {}x{}, {} tiles",
                s.sheet, s.origin.x, s.origin.y, s.size.x, s.size.y, s.tiles
            )?;
        }
        tiles(f, "unassigned tiles", &self.unassigned)?;
        writeln!(f, "decor tiles: {}", self.decor)?;
        tiles(f, "low confidence tiles", &self.low_confidence)?;
        if let Some(c) = &self.collision {
            let solid: Vec<String> = c.solid.iter().map(u8::to_string).collect();
            writeln!(
                f,
                "collision: solid terrains {}, {} shapes, {} half tiles",
                solid.join(" "),
                c.shapes,
                c.half_tiles
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: u32 = 16;

    /// `columns x 2` tiles of distinct colours, outlined, repeated `frames`
    /// times side by side.
    fn tiles(columns: u32, frames: u32) -> (u32, u32, Vec<u8>) {
        let (width, height) = (columns * T * frames, 2 * T);
        let mut pixels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let (cx, cy, px, py) = ((x / T) % columns, y / T, x % T, y % T);
                let edge = px < 2 || py < 2 || px >= T - 2 || py >= T - 2;
                let c = (cx * 50 + cy * 120) as u8;
                let colour = if edge {
                    [40, 30, 30, 255]
                } else {
                    [200, c, 255 - c, 255]
                };
                pixels.extend(colour);
            }
        }
        (width, height, pixels)
    }

    #[test]
    fn strips_are_listed_not_analysed() {
        let (bw, bh, base) = tiles(3, 1);
        let (sw, sh, strip) = tiles(3, 4);
        let blank = vec![0u8; 10 * 10 * 4];
        let inputs = [
            ("base.png", RgbaImage::new(bw, bh, &base).expect("sized")),
            ("strip.png", RgbaImage::new(sw, sh, &strip).expect("sized")),
            ("blank.png", RgbaImage::new(10, 10, &blank).expect("sized")),
        ];
        let found = auto_setup(&inputs, &[]).expect("runs");
        assert_eq!(found.sheets, [0]);
        let text = found.report.to_string();
        let lines: Vec<&str> = text.lines().take(4).collect();
        assert_eq!(lines[0], "sheets: 3");
        assert!(
            lines[1].starts_with("  sheet \"base.png\": tile 16x16"),
            "{text}"
        );
        assert!(
            lines[2].ends_with(", animation of \"base.png\", 4 frames, 6 tiles"),
            "{text}"
        );
        assert_eq!(lines[3], "  sheet \"blank.png\": no grid");
    }

    /// One row of 16 px tiles, each the left colour over its left half and
    /// the right over the rest; `None` is transparent.
    fn split_row(colours: &[[Option<[u8; 4]>; 2]]) -> (u32, Vec<u8>) {
        let width = colours.len() as u32 * T;
        let mut pixels = Vec::new();
        for _ in 0..T {
            for x in 0..width {
                let half = usize::from(x % T >= T / 2);
                pixels.extend(colours[(x / T) as usize][half].unwrap_or([0; 4]));
            }
        }
        (width, pixels)
    }

    #[test]
    fn terrains_whose_pure_tiles_join_stamps_shrink_or_drop() {
        let wood = Some([150, 90, 40, 255]);
        let stone = Some([120, 120, 130, 255]);
        let sky = Some([90, 150, 230, 255]);
        let row = [wood, wood, None, wood, None, stone, stone, None, None].map(|c| [c, c]);
        let mixed = [[wood, sky], [None, None], [wood, sky], [None, None]];
        let (width, pixels) = split_row(&[&row[..], &mixed[..]].concat());
        let image = RgbaImage::new(width, T, &pixels).expect("sized");
        let found = auto_setup(&[("props.png", image)], &[]).expect("runs");
        let tiles = |s: &Stamp| s.tiles().map(|a| a.tile).collect::<Vec<_>>();
        let stamps: Vec<Vec<u32>> = found.stamps.iter().map(tiles).collect();
        assert_eq!(stamps, [vec![0, 1], vec![5, 6]]);
        let kept: Vec<(TerrainId, usize)> = (found.proposal.terrains.iter())
            .map(|t| (t.id, t.pure_tiles.len()))
            .collect();
        assert_eq!(kept.len(), 1, "stone is dropped: {kept:?}");
        assert_eq!(kept[0].1, 1, "wood keeps tile 3");
        let reported: Vec<(TerrainId, usize)> = (found.report.terrains.iter())
            .map(|t| (t.id, t.pure_tiles))
            .collect();
        assert_eq!(reported, kept);
        let unassigned: Vec<u32> = found.proposal.unassigned.iter().map(|a| a.tile).collect();
        let listed: Vec<u32> = found
            .report
            .unassigned
            .iter()
            .flat_map(|(_, t)| t.clone())
            .collect();
        assert_eq!(unassigned, [9, 11]);
        assert_eq!(listed, unassigned);
    }

    #[test]
    fn no_grid_anywhere_is_an_error() {
        let blank = vec![0u8; 32 * 32 * 4];
        let image = RgbaImage::new(32, 32, &blank).expect("sized");
        assert_eq!(
            auto_setup(&[("blank.png", image)], &[]),
            Err(SetupError::NoGrid)
        );
    }

    #[test]
    fn rounding_never_shows_negative_zero() {
        assert_eq!(fixed(-0.001), "0.00");
        assert_eq!(fixed(0.375), "0.38");
    }
}
