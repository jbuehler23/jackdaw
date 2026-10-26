//! Checks sheets that cannot be redistributed. Runs only when
//! `JACKDAW_LOCAL_TILESETS` names a directory holding
//! `jackdaw_expected/slice.txt`; the sheets and their expected results stay
//! in that directory. Each non-comment line of the file is one of:
//!
//! ```text
//! grid <path> <tile width> <tile height> <columns> <rows>
//! anim <base path> <strip path> <frames> <exact|loose>
//! terrains <path> <tile width> <tile height> <count>
//! ```
//!
//! `terrains` analyses the sheet alone as a packed grid of that tile size and
//! counts the terrains found.
//!
//! Paths are relative to the directory; `#` starts a comment line. Lines of
//! any other kind are skipped.

use std::path::{Path, PathBuf};

use glam::UVec2;
use jackdaw_tilemap::{
    GridLayout, LOOSE_FRAME_MATCH, RgbaImage, SheetInput, detect_animation, detect_grid,
    find_terrains,
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
    if !matches!(kind, "grid" | "anim" | "terrains") {
        return;
    }
    let path = fields
        .next()
        .unwrap_or_else(|| panic!("missing path in: {line}"));
    let sheet = Sheet::load(root, path);
    match kind {
        "grid" => check_grid(&sheet, path, fields, line),
        "anim" => check_anim(root, &sheet, path, fields, line),
        _ => check_terrains(&sheet, path, fields, line),
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

#[expect(clippy::print_stdout, reason = "reports each checked case")]
fn check_terrains<'a>(
    sheet: &Sheet,
    path: &str,
    mut fields: impl Iterator<Item = &'a str>,
    line: &str,
) {
    let image = sheet.image();
    let tile = UVec2::new(number(fields.next(), line), number(fields.next(), line));
    let layout = GridLayout {
        tile,
        margin: UVec2::ZERO,
        spacing: UVec2::ZERO,
        columns: sheet.width / tile.x.max(1),
        rows: sheet.height / tile.y.max(1),
    };
    let found = find_terrains(&[SheetInput {
        name: path,
        image,
        layout,
    }]);
    for t in &found.terrains {
        println!(
            "{path}: terrain {} {:?} {:?} pure {}",
            t.id,
            t.name,
            t.swatch,
            t.pure_tiles.len()
        );
    }
    let count = number(fields.next(), line) as usize;
    assert_eq!(found.terrains.len(), count, "{line}");
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
