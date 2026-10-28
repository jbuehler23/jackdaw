use bevy::math::UVec2;
use jackdaw_tilemap::TileAddress;
use jackdaw_tileset::{Tileset, TilesetError, TilesetSheet};

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
fn layers_map_back_to_their_sheet_and_tile() {
    let tileset = tileset();
    assert_eq!(tileset.tile_of_layer(3), Some((0, 3)));
    assert_eq!(tileset.tile_of_layer(4), Some((1, 0)));
    assert_eq!(tileset.tile_of_layer(11), Some((1, 7)));
    assert_eq!(tileset.tile_of_layer(12), None);
}

#[test]
fn setup_addresses_need_a_tile_on_a_sheet_under_setup() {
    let tileset = tileset();
    assert_eq!(
        tileset.setup_address(1, 7),
        Ok(TileAddress { sheet: 0, tile: 7 })
    );
    assert_eq!(
        tileset.setup_address(1, 8),
        Err(TilesetError::NoTile { sheet: 1, tile: 8 })
    );
    assert_eq!(
        tileset.setup_address(0, 1),
        Err(TilesetError::OutsideSetup(0))
    );
    assert_eq!(
        tileset.setup_address(2, 0),
        Err(TilesetError::NoTile { sheet: 2, tile: 0 })
    );
}
