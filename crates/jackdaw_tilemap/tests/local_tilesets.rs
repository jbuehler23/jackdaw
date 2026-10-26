//! Checks sheets that cannot be redistributed. Runs only when
//! `JACKDAW_LOCAL_TILESETS` names a directory holding
//! `jackdaw_expected/slice.txt`; the sheets and their expected results stay
//! in that directory. Each non-comment line of the file is one of:
//!
//! ```text
//! grid <path> <tile width> <tile height> <columns> <rows>
//! anim <base path> <strip path> <frames> <exact|loose>
//! terrains <path> <tile width> <tile height> <count>
//! fit <path> <tile width> <tile height> <sets> <mode> <covered> <total> <backgrounds>
//! ```
//!
//! `terrains` analyses the sheet alone as a packed grid of that tile size and
//! counts the terrains found. `fit` also fits layouts, counts the proposed
//! sets, checks the mode (`Sides`, `Corners` or `CornersAndSides`) and
//! coverage of the set with the most tiles, and counts background terrains.
//!
//! Paths are relative to the directory; `#` starts a comment line. Lines of
//! any other kind are skipped.

use std::path::{Path, PathBuf};

use glam::UVec2;
use jackdaw_tilemap::{
    GridLayout, LOOSE_FRAME_MATCH, RgbaImage, SheetInput, TerrainAnalysis, TileAddress,
    detect_animation, detect_grid, find_terrains, fit_layouts,
};

struct Sheet {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Sheet {
    fn load(root: &Path, relative: &str) -> Self {
        let image = image::open(root.join(relative))
            .unwrap_or_else(|e| panic!("{relative}: {e}"))
            .to_rgba8();
        Self {
            width: image.width(),
            height: image.height(),
            pixels: image.into_raw(),
        }
    }

    fn image(&self) -> RgbaImage<'_> {
        RgbaImage::new(self.width, self.height, &self.pixels).expect("sized")
    }
}

fn number(field: Option<&str>, line: &str) -> u32 {
    field
        .and_then(|f| f.parse().ok())
        .unwrap_or_else(|| panic!("bad number in: {line}"))
}

fn check(root: &Path, line: &str) {
    let mut fields = line.split_whitespace();
    let kind = fields.next().unwrap_or_default();
    if !matches!(kind, "grid" | "anim" | "terrains" | "fit") {
        return;
    }
    let path = fields
        .next()
        .unwrap_or_else(|| panic!("missing path in: {line}"));
    let sheet = Sheet::load(root, path);
    match kind {
        "grid" => check_grid(&sheet, path, fields, line),
        "anim" => check_anim(root, &sheet, path, fields, line),
        "terrains" => check_terrains(&sheet, path, fields, line),
        _ => check_fit(&sheet, path, fields, line),
    }
}

#[expect(clippy::print_stdout, reason = "reports each checked case")]
fn check_grid<'a>(
    sheet: &Sheet,
    path: &str,
    mut fields: impl Iterator<Item = &'a str>,
    line: &str,
) {
    let found = detect_grid(&sheet.image()).unwrap_or_else(|| panic!("no grid: {path}"));
    let tile = UVec2::new(number(fields.next(), line), number(fields.next(), line));
    let cells = (number(fields.next(), line), number(fields.next(), line));
    println!("{path}: {:?} confidence {}", found.layout, found.confidence);
    assert_eq!(found.layout.tile, tile, "{line}");
    assert_eq!((found.layout.columns, found.layout.rows), cells, "{line}");
}

#[expect(clippy::print_stdout, reason = "reports each checked case")]
fn check_anim<'a>(
    root: &Path,
    sheet: &Sheet,
    path: &str,
    mut fields: impl Iterator<Item = &'a str>,
    line: &str,
) {
    let found = detect_grid(&sheet.image()).unwrap_or_else(|| panic!("no grid: {path}"));
    let strip_path = fields
        .next()
        .unwrap_or_else(|| panic!("missing strip in: {line}"));
    let frames = number(fields.next(), line);
    let share = match fields.next() {
        Some("exact") => 0.0,
        Some("loose") => LOOSE_FRAME_MATCH,
        _ => panic!("expected exact or loose in: {line}"),
    };
    let strip = Sheet::load(root, strip_path);
    let animation = detect_animation(&sheet.image(), &found.layout, &strip.image(), share)
        .unwrap_or_else(|| panic!("not an animation: {line}"));
    println!("{strip_path}: {} frames of {path}", animation.frames);
    assert_eq!(animation.frames, frames, "{line}");
}

/// The sheet alone as a packed grid of the next two fields' tile size.
fn packed_input<'a>(
    sheet: &'a Sheet,
    path: &'a str,
    fields: &mut impl Iterator<Item = &'a str>,
    line: &str,
) -> SheetInput<'a> {
    let tile = UVec2::new(number(fields.next(), line), number(fields.next(), line));
    let layout = GridLayout {
        tile,
        margin: UVec2::ZERO,
        spacing: UVec2::ZERO,
        columns: sheet.width / tile.x.max(1),
        rows: sheet.height / tile.y.max(1),
    };
    SheetInput {
        name: path,
        image: sheet.image(),
        layout,
    }
}

#[expect(clippy::print_stdout, reason = "reports each checked case")]
fn print_terrains(path: &str, found: &TerrainAnalysis) {
    for t in &found.terrains {
        println!(
            "{path}: terrain {} {:?} {:?} pure {}",
            t.id,
            t.name,
            t.swatch,
            t.pure_tiles.len()
        );
    }
}

fn check_terrains<'a>(
    sheet: &'a Sheet,
    path: &'a str,
    mut fields: impl Iterator<Item = &'a str>,
    line: &str,
) {
    let input = packed_input(sheet, path, &mut fields, line);
    let found = find_terrains(&[input]);
    print_terrains(path, &found);
    let count = number(fields.next(), line) as usize;
    assert_eq!(found.terrains.len(), count, "{line}");
}

#[expect(clippy::print_stdout, reason = "reports each checked case")]
fn check_fit<'a>(
    sheet: &'a Sheet,
    path: &'a str,
    mut fields: impl Iterator<Item = &'a str>,
    line: &str,
) {
    let input = packed_input(sheet, path, &mut fields, line);
    let found = find_terrains(&[input]);
    print_terrains(path, &found);
    let proposal = fit_layouts(&found, &[input]).unwrap_or_else(|e| panic!("{path}: {e}"));
    for set in &proposal.sets {
        let tiles: Vec<u32> = set.tiles.iter().map(|t| t.address.tile).collect();
        println!(
            "{path}: set {} over {} {:?} {}/{} {:?} tiles {tiles:?}",
            set.inner, set.outer, set.mode, set.covered, set.total, set.template
        );
    }
    println!("{path}: terrains {:?}", proposal.terrains);
    let tiles = |a: &[TileAddress]| a.iter().map(|a| a.tile).collect::<Vec<_>>();
    println!("{path}: unassigned {:?}", tiles(&proposal.unassigned));
    println!(
        "{path}: low confidence {:?}",
        tiles(&proposal.low_confidence)
    );
    let sets = number(fields.next(), line) as usize;
    assert_eq!(proposal.sets.len(), sets, "{line}");
    let Some(main) = proposal.sets.iter().max_by_key(|s| s.tiles.len()) else {
        return;
    };
    let mode = fields.next().unwrap_or_default();
    assert_eq!(format!("{:?}", main.mode), mode, "{line}");
    let coverage = (number(fields.next(), line), number(fields.next(), line));
    assert_eq!((main.covered as u32, main.total as u32), coverage, "{line}");
    let backgrounds = proposal.terrains.iter().filter(|t| t.background).count();
    assert_eq!(backgrounds, number(fields.next(), line) as usize, "{line}");
}

#[test]
#[expect(clippy::print_stdout, reason = "reports why the test did nothing")]
fn local_sheets_match_local_expectations() {
    let Some(root) = std::env::var_os("JACKDAW_LOCAL_TILESETS").map(PathBuf::from) else {
        println!("skipping: JACKDAW_LOCAL_TILESETS not set");
        return;
    };
    let expected = root.join("jackdaw_expected").join("slice.txt");
    let Ok(text) = std::fs::read_to_string(&expected) else {
        println!("skipping: {} not found", expected.display());
        return;
    };
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'));
    for line in lines {
        check(&root, line);
    }
}
