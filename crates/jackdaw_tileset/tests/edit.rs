use bevy::math::UVec2;
use jackdaw_tilemap::{MatchMode, Peer, Provenance, TileAddress, TileTerrains};
use jackdaw_tileset::{Tileset, TilesetSheet};

/// Two sheets, the second alone under terrain setup.
fn tileset() -> Tileset {
    let sheet = |source: &str, columns, rows, first_layer| TilesetSheet {
        source: source.into(),
        columns,
        rows,
        first_layer,
        ..Default::default()
    };
    Tileset {
        image: "farm.tiles.png".into(),
        tile_size: UVec2::splat(16),
        grid: UVec2::new(4, 3),
        sheets: vec![sheet("props.png", 4, 1, 0), sheet("ground.png", 4, 2, 4)],
        setup_sheets: vec![1],
        next_terrain_id: 1,
        ..Default::default()
    }
}

#[test]
fn painting_a_property_sets_or_clears_it_and_counts_the_changes() {
    let mut tileset = tileset();
    assert_eq!(tileset.paint_property(&[4, 5], "water", "deep"), 2);
    assert_eq!(tileset.paint_property(&[4, 6], "water", "deep"), 1);
    assert_eq!(tileset.properties[&6]["water"], "deep");
    assert_eq!(tileset.paint_property(&[4, 5, 6], "water", ""), 3);
    assert!(tileset.properties.is_empty(), "an emptied layer goes");
}

#[test]
fn describe_lists_terrains_sets_flags_objects_and_a_grid_per_sheet() {
    let mut tileset = tileset();
    let mut setup = tileset.setup().expect("converts");
    let grass = setup
        .add_terrain("grass", [0x84, 0xc6, 0x69], true)
        .expect("id");
    let sand = setup
        .add_terrain("sand", [0xe7, 0xa1, 0x6a], false)
        .expect("id");
    setup.add_set(sand, grass, MatchMode::Sides).expect("set");
    let edge = TileTerrains::filled(sand).with(Peer::Top, grass);
    setup
        .set_peers(
            TileAddress { sheet: 0, tile: 0 },
            TileTerrains::filled(grass),
        )
        .expect("bits");
    setup
        .set_peers(TileAddress { sheet: 0, tile: 1 }, edge)
        .expect("bits");
    let mixed = TileAddress { sheet: 0, tile: 2 };
    setup
        .set_peers(mixed, edge.with(Peer::Top, 0))
        .expect("bits");
    // Auto setup's guess, not a hand-set tile, so it stays flagged.
    setup.tiles.get_mut(&mixed).expect("set").provenance = [Provenance::Auto; 9];
    setup
        .put_block(0, UVec2::new(4, 2), UVec2::new(2, 1), UVec2::new(2, 1))
        .expect("fits");
    setup
        .set_peers(TileAddress { sheet: 0, tile: 3 }, TileTerrains::filled(0))
        .expect("bits");
    tileset.set_setup(&setup).expect("converts");
    tileset.paint_property(&[5], "slow", "yes");

    let text = tileset.describe().expect("describes");
    for line in [
        "image \"farm.tiles.png\": tile 16x16, 4x3 layers",
        "  terrain 1 \"grass\": swatch #84c669, background, collision passable",
        "  terrain 2 \"sand\": swatch #e7a16a, collision passable",
        "  set 2 over 1: Sides, 1/16 shapes, missing 1,2,1,2,2,2,1,2,1;",
        "flagged tiles: 1",
        "  sheet 1: 2",
        "  object on sheet 1 at 2,1: 2x1, 2 tiles, pivot 0.5,0, by hand",
        "  layer 5: slow=yes",
        "sheet 0 \"props.png\": 4x1 tiles from layer 0",
        "  ....",
        "sheet 1 \"ground.png\": 4x2 tiles from layer 4",
        "  1+?.",
        "  ..##",
    ] {
        assert!(text.contains(line), "missing {line:?} in\n{text}");
    }
}
