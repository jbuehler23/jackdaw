//! Checks sheets that cannot be redistributed. Runs only when
//! `JACKDAW_LOCAL_TILESETS` names a directory holding
//! `jackdaw_expected/slice.txt`; the sheets and their expected results stay
//! in that directory. Each non-comment line of the file is one of:
//!
//! ```text
//! grid <path> <tile width> <tile height> <columns> <rows>
//! anim <base path> <strip path> <frames> <exact|loose>
//! ```
//!
//! Paths are relative to the directory; `#` starts a comment line.

use std::path::{Path, PathBuf};

use glam::UVec2;
use jackdaw_tilemap::{LOOSE_FRAME_MATCH, RgbaImage, detect_animation, detect_grid};

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

#[expect(clippy::print_stdout, reason = "reports each checked case")]
fn check(root: &Path, line: &str) {
    let mut fields = line.split_whitespace();
    let kind = fields.next().unwrap_or_default();
    let path = fields
        .next()
        .unwrap_or_else(|| panic!("missing path in: {line}"));
    let sheet = Sheet::load(root, path);
    let found = detect_grid(&sheet.image()).unwrap_or_else(|| panic!("no grid: {path}"));
    if kind == "grid" {
        let tile = UVec2::new(number(fields.next(), line), number(fields.next(), line));
        let cells = (number(fields.next(), line), number(fields.next(), line));
        println!("{path}: {:?} confidence {}", found.layout, found.confidence);
        assert_eq!(found.layout.tile, tile, "{line}");
        assert_eq!((found.layout.columns, found.layout.rows), cells, "{line}");
    } else if kind == "anim" {
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
    } else {
        panic!("unknown line: {line}");
    }
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
