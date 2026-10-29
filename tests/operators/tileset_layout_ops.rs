//! The template, reset, property, object and describe operators over an
//! auto-imported overworld tileset.

use crate::tileset_edit_ops::{
    GROUND, WATER, call, edit, entries, on_disk, over_the_remote, overworld, pairs, peers_of,
    refused, tile_of,
};
use crate::tileset_ops::{SHEET, reports, warnings};

use bevy::prelude::*;
use jackdaw_tilemap::LayoutTemplate;
use jackdaw_tileset::TilesetObject;
use serde_json::json;

#[test]
fn apply_template_lays_its_shapes_from_the_origin() {
    let (mut app, dir) = overworld();
    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.apply_template",
        &[
            ("inner", "11"),
            ("outer", "2"),
            ("template", "corner2x2"),
            ("origin", "10,0"),
        ],
    );
    let shapes = LayoutTemplate::Corner2x2.tiles(WATER, GROUND);
    for (tile, shape) in [10, 11, 37, 38].into_iter().zip(shapes) {
        assert_eq!(peers_of(&after, tile), Some(shape.peers), "tile {tile}");
    }
    assert!(pairs(&after).contains(&(WATER, GROUND)));
    let apply = "tileset.apply_template";
    let at = |origin| {
        [
            ("inner", "11"),
            ("outer", "2"),
            ("template", "side16"),
            ("origin", origin),
        ]
    };
    refused(&mut app, &dir, apply, &at("24,0"), "leave the sheet");
    refused(&mut app, &dir, apply, &at("1"), "two numbers");
    refused(
        &mut app,
        &dir,
        apply,
        &at("0,4"),
        "part of the object at column 0 row 4",
    );
}

#[test]
fn reset_to_auto_hands_a_tile_back_to_the_proposal() {
    let (mut app, dir) = overworld();
    let tile = tile_of(&on_disk(&dir), GROUND);
    let imported = on_disk(&dir);
    call(
        &mut app,
        "tileset.tile.set_bits",
        &[
            ("tile", &tile.to_string()),
            ("peers", "11,11,11,11,11,11,11,11,11"),
        ],
    );
    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.tile.reset_to_auto",
        &[("tiles", &format!("0:{tile}"))],
    );
    assert_eq!(after.tiles, imported.tiles);
}

#[test]
fn paint_property_sets_and_clears_a_value() {
    let (mut app, dir) = overworld();
    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.paint_property",
        &[("tiles", "0:1, 5"), ("key", "water"), ("value", "deep")],
    );
    let layers: Vec<u32> = after.properties.keys().copied().collect();
    assert_eq!(layers, [1, 5]);
    assert_eq!(after.properties[&5]["water"], "deep");
    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.paint_property",
        &[("tiles", "5"), ("key", "water"), ("value", " deep ")],
    );
    assert_eq!(
        after.properties[&5]["water"], " deep ",
        "a value is kept as given"
    );
    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.paint_property",
        &[("tiles", "1,5"), ("key", "water"), ("value", "")],
    );
    assert!(after.properties.is_empty());
    refused(
        &mut app,
        &dir,
        "tileset.paint_property",
        &[("tiles", "1")],
        "no key",
    );
}

#[test]
fn objects_are_added_with_a_pivot_and_removed_by_any_tile() {
    let (mut app, dir) = overworld();
    let (before, after) = edit(
        &mut app,
        &dir,
        "tileset.object.add",
        &[("origin", "0,10"), ("size", "2,1"), ("pivot", "0.5,0.25")],
    );
    let object = (after.objects.iter())
        .find(|o| o.origin == UVec2::new(0, 10))
        .expect("added");
    assert_eq!(
        (object.size, object.pivot),
        (UVec2::new(2, 1), Vec2::new(0.5, 0.25))
    );
    assert!(object.by_user);
    assert_eq!(after.objects.len(), before.objects.len() + 1);

    refused(
        &mut app,
        &dir,
        "tileset.object.remove",
        &[("origin", "9,6")],
        "no object covers",
    );
    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.object.remove",
        &[("sheet", SHEET), ("origin", "10,8")],
    );
    assert!(after.objects.iter().all(|o| o.origin != UVec2::new(9, 7)));
    assert!(
        (after.objects.iter()).all(|o| o.pivot == TilesetObject::BOTTOM_CENTRE || o.by_user),
        "the others keep their pivots"
    );
    let remove = "tileset.object.remove";
    refused(
        &mut app,
        &dir,
        remove,
        &[("origin", "10,8")],
        "no object covers",
    );
    let add = "tileset.object.add";
    refused(
        &mut app,
        &dir,
        add,
        &[("origin", "26,10"), ("size", "2,1")],
        "leave the sheet",
    );
    refused(
        &mut app,
        &dir,
        add,
        &[("sheet", "4"), ("origin", "0,0"), ("size", "1,1")],
        "no sheet",
    );
    for pivot in ["2,0", "NaN,0", "0.5,-0.1"] {
        refused(
            &mut app,
            &dir,
            add,
            &[("origin", "0,0"), ("size", "1,1"), ("pivot", pivot)],
            "two fractions from 0 to 1",
        );
    }
}

#[test]
fn describe_reports_the_overworld_tileset_without_history() {
    let (mut app, dir) = overworld();
    let (before, count) = (on_disk(&dir), entries(&app));
    call(&mut app, "tileset.describe", &[]);
    assert!(warnings(&app).is_empty(), "{:?}", warnings(&app));
    let text = reports(&app).join("\n");
    for line in [
        "tileset \"overworld.tileset.bsn\"",
        "image \"overworld.tiles.png\": tile 16x16, 27x38 layers",
        "terrains: 8",
        "  terrain 1 \"grass\": swatch #8aa744, collision passable",
        "  terrain 2 \"grass 2\": swatch #c6b15d, collision passable",
        "  terrain 6 \"grass 4\": swatch #555d47, background, collision passable",
        "  terrain 11 \"water\": swatch #06a1b3, collision passable",
        "terrain sets: 6",
        "  set 2 over 1: CornersAndSides, 34/47 shapes, missing ",
        "  set 11 over 1: CornersAndSides, 29/47 shapes, missing ",
        "flagged tiles: 0",
        "objects: 34",
        "  object on sheet 0 at 0,4: 5x3, 14 tiles, pivot 0.5,0",
        "properties: 0 layers",
        "sheet 0 \"overworld.png\": 27x38 tiles from layer 0",
    ] {
        assert!(text.contains(line), "missing {line:?} in\n{text}");
    }
    let grid_rows = text
        .lines()
        .filter(|l| l.len() == 29 && l.starts_with("  "))
        .count();
    assert!(grid_rows >= 38, "a row of symbols per sheet row:\n{text}");
    assert_eq!((on_disk(&dir), entries(&app)), (before, count));
    refused(
        &mut app,
        &dir,
        "tileset.describe",
        &[("path", "nothing.bsn")],
        "not an asset",
    );
}

/// Every template, object, property and describe operator, as an agent
/// drives it.
#[test]
fn the_layout_operators_run_over_the_remote() {
    let (mut app, dir) = overworld();
    let tile = tile_of(&on_disk(&dir), GROUND).to_string();
    over_the_remote(
        &mut app,
        vec![
            (
                "tileset.apply_template",
                json!({ "inner": "11", "outer": "2", "template": "corner2x2", "origin": "10,0" }),
            ),
            (
                "tileset.tile.set_bits",
                json!({ "tile": &tile, "peer": "centre", "terrain": "11" }),
            ),
            ("tileset.tile.reset_to_auto", json!({ "tiles": &tile })),
            (
                "tileset.paint_property",
                json!({ "tiles": "3", "key": "k", "value": "v" }),
            ),
            (
                "tileset.object.add",
                json!({ "origin": "0,10", "size": "1,1" }),
            ),
            ("tileset.object.remove", json!({ "origin": "0,10" })),
            ("tileset.describe", json!({})),
        ],
    );
    assert_eq!(on_disk(&dir).properties[&3]["k"], "v");
}
