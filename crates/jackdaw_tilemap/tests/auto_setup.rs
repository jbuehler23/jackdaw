//! Auto setup over the Tiny Farm sheets: the full report is a golden file.

use jackdaw_tilemap::{AutoSetup, RgbaImage, auto_setup};

struct Sheet {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Sheet {
    fn load(name: &str) -> Self {
        let path = format!("{}/tests/data/tiny_farm/{name}", env!("CARGO_MANIFEST_DIR"));
        let image = image::open(&path).expect("fixture decodes").to_rgba8();
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

fn setup(name: &str) -> AutoSetup {
    let sheet = Sheet::load(name);
    auto_setup(&[(name, sheet.image())], &[]).expect("auto setup runs")
}

fn golden() -> String {
    let path = format!(
        "{}/tests/data/tiny_farm/expected_report.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path).expect("golden report")
}

#[test]
fn packed_report_matches_golden() {
    assert_eq!(setup("tilemap_packed.png").report.to_string(), golden());
}

#[test]
fn spaced_report_matches_golden_but_its_grid_line() {
    let spaced = setup("tilemap.png").report.to_string();
    let renamed = spaced
        .replace("\"tilemap.png\"", "\"tilemap_packed.png\"")
        .replace("\"tilemap ", "\"tilemap_packed ");
    let golden = golden();
    let (got, want): (Vec<&str>, Vec<&str>) = (renamed.lines().collect(), golden.lines().collect());
    assert_eq!(got.len(), want.len());
    for (i, (g, w)) in got.iter().zip(&want).enumerate() {
        if w.starts_with("  sheet ") {
            assert!(
                g.contains("margin 0x0, spacing 1x1, 12 columns, 11 rows"),
                "{g}"
            );
        } else {
            assert_eq!(g, w, "line {i}");
        }
    }
}

#[test]
fn stamps_follow_the_art() {
    let found = setup("tilemap_packed.png");
    let stamps: Vec<Vec<u32>> = (found.stamps.iter())
        .map(|s| s.cells.iter().map(|(_, tile)| *tile).collect())
        .collect();
    let barn = vec![90, 91, 92, 102, 103, 104, 114, 115, 116, 126, 127, 128];
    let roof = vec![82, 93, 94, 95, 105, 106, 107, 117, 118, 119, 129, 130, 131];
    let expected = [
        vec![2, 14],
        vec![3, 15],
        vec![26, 38],
        roof,
        barn,
        vec![98, 99],
        vec![100, 101],
        vec![110, 111],
        vec![112, 113],
    ];
    assert_eq!(stamps, expected);
    let roof = &found.stamps[3];
    assert_eq!(
        (roof.origin.to_array(), roof.size.to_array()),
        ([9, 6], [3, 5])
    );
}

#[test]
fn absorbed_terrains_leave_the_proposal() {
    let found = setup("tilemap_packed.png");
    let kept: Vec<u8> = found.proposal.terrains.iter().map(|t| t.id).collect();
    assert_eq!(kept, [1, 2, 3]);
    assert_eq!(found.sheets, [0]);
    let unassigned: Vec<u32> = found.proposal.unassigned.iter().map(|a| a.tile).collect();
    assert_eq!(
        found.report.unassigned,
        [("tilemap_packed.png".to_string(), unassigned)]
    );
}

#[test]
fn collision_is_reported_only_for_solid_terrains() {
    let sheet = Sheet::load("tilemap_packed.png");
    let found = auto_setup(&[("tilemap_packed.png", sheet.image())], &[2, 3]).expect("runs");
    let text = found.report.to_string();
    let last = text.lines().last().unwrap_or_default();
    assert!(
        last.starts_with("collision: solid terrains 2 3, "),
        "{text}"
    );
    assert!(!golden().contains("collision"));
}
