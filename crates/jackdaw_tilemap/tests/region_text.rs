use jackdaw_tilemap::{CellCoord, Legend, LegendEntry, RegionTextErrorKind, emit_rows, parse_rows};

fn legend() -> Legend {
    Legend::parse_entries([
        (".", "empty"),
        ("#", "wall"),
        ("g", "grass"),
        ("C", "tile:chest"),
    ])
    .expect("valid legend")
}

#[test]
fn forest_round_trips_exactly() {
    let text = include_str!("golden/region_text/forest.rows");
    let region = parse_rows(text, CellCoord::new(-4, 10), &legend()).expect("parses");
    assert_eq!((region.width, region.height), (10, 5));
    assert_eq!(emit_rows(&region), text);
    let reparsed = parse_rows(&emit_rows(&region), region.origin, &legend()).expect("reparses");
    assert_eq!(reparsed, region);
}

#[test]
fn forest_cells_resolve_through_the_legend() {
    let text = include_str!("golden/region_text/forest.rows");
    let legend = legend();
    let region = parse_rows(text, CellCoord::new(-4, 10), &legend).expect("parses");
    let chest: Vec<_> = region
        .cells()
        .filter(|(_, symbol)| legend.get(*symbol) == Some(&LegendEntry::Tile("chest".into())))
        .map(|(cell, _)| cell)
        .collect();
    assert_eq!(chest, vec![CellCoord::new(0, 8)]);
}

#[test]
fn indented_block_emits_without_indentation() {
    let text = include_str!("golden/region_text/indented.rows");
    let expected = include_str!("golden/region_text/indented.expected");
    let region = parse_rows(text, CellCoord::new(0, 0), &legend()).expect("parses");
    assert_eq!(emit_rows(&region), expected);
}

#[test]
fn blank_row_golden_reports_its_line() {
    let text = include_str!("golden/region_text/blank_row.rows");
    let error = parse_rows(text, CellCoord::new(0, 0), &legend()).expect_err("fails");
    assert_eq!((error.line, error.column), (2, 1));
    assert_eq!(error.kind, RegionTextErrorKind::BlankRow);
    assert_eq!(
        error.to_string(),
        "line 2, column 1: blank row inside the region"
    );
}
