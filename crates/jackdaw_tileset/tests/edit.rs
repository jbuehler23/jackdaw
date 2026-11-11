use bevy::math::UVec2;
use jackdaw_tilemap::{
    MatchMode, Peer, Provenance, ShapePreset, TileAddress, TileCollision, TileTerrains,
};
use jackdaw_tileset::{CollisionPreset, Tileset, TilesetCollision, TilesetSheet};

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
    let set = setup.add_set("", MatchMode::Sides).expect("set");
    setup.add_set_terrain(set, grass, None).expect("free");
    setup.add_set_terrain(set, sand, None).expect("free");
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
    let water = setup.add_terrain("water", [0, 0, 255], false).expect("id");
    let pond = setup.add_set("", MatchMode::Corners).expect("set");
    setup.add_set_terrain(pond, water, None).expect("free");
    let mixed = TileAddress { sheet: 0, tile: 2 };
    setup
        .set_peers(mixed, edge.with(Peer::Top, water))
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
    tileset.physics_layers_mut().push("Water".into());
    let mut setup = tileset.setup().expect("converts");
    let solid = TileCollision::preset(ShapePreset::Solid, 0, false).expect("a preset");
    let trigger = TileCollision::preset(ShapePreset::TopHalf, 1, true).expect("a preset");
    setup.set_collision(TileAddress { sheet: 0, tile: 0 }, solid);
    setup.set_collision(TileAddress { sheet: 0, tile: 0 }, trigger);
    tileset.set_setup(&setup).expect("converts");
    tileset.terrains[1].icon = Some((1, 3));

    let text = tileset.describe().expect("describes");
    for line in [
        "image \"farm.tiles.png\": tile 16x16, 4x3 layers",
        "  terrain 1 \"grass\": swatch #84c669, background\n",
        "  terrain 2 \"sand\": swatch #e7a16a, icon 1:3",
        "  set \"Terrain set 1\" (Sides): terrains bottom first [\"grass\", \"sand\"], 1/16 shapes, missing 1,2,1,2,2,2,1,2,1;",
        "flagged tiles: 1",
        "  sheet 1: 2",
        "  object on sheet 1 at 2,1: 2x1, 2 tiles, pivot 0.5,0, by hand",
        "  layer 5: slow=yes",
        "physics layers: 2",
        "  layer 0 \"World\": 1 solid, 0 trigger tiles",
        "  layer 1 \"Water\": 0 solid, 1 trigger tiles",
        "sheet 0 \"props.png\": 4x1 tiles from layer 0",
        "  ....",
        "sheet 1 \"ground.png\": 4x2 tiles from layer 4",
        "  1+?.",
        "  ..##",
    ] {
        assert!(text.contains(line), "missing {line:?} in\n{text}");
    }
}

#[test]
fn an_icon_stays_with_its_terrain_through_a_setup_edit() {
    let mut tileset = tileset();
    let mut setup = tileset.setup().expect("converts");
    let grass = setup.add_terrain("grass", [1, 2, 3], false).expect("id");
    let sand = setup.add_terrain("sand", [4, 5, 6], false).expect("id");
    tileset.set_setup(&setup).expect("converts");
    tileset.terrains[1].icon = Some((1, 2));

    let mut setup = tileset.setup().expect("converts");
    setup.remove_terrain(grass).expect("removed");
    setup.add_terrain("dirt", [7, 8, 9], false).expect("id");
    tileset.set_setup(&setup).expect("converts");
    let icons: Vec<_> = (tileset.terrains.iter())
        .map(|t| (t.name.as_str(), t.icon))
        .collect();
    assert_eq!(icons, [("sand", Some((1, 2))), ("dirt", None)]);
    assert_eq!(tileset.terrains[0].id, sand);
}

#[test]
fn the_collision_lookup_reads_each_packed_layer() {
    let mut tileset = tileset();
    let mut setup = tileset.setup().expect("converts");
    let slope = TileCollision::preset(ShapePreset::SlopeDown, 0, false).expect("a preset");
    setup.set_collision(TileAddress { sheet: 0, tile: 2 }, slope);
    tileset.set_setup(&setup).expect("converts");
    let lookup = tileset.collision_lookup();
    let shapes = lookup.get(6);
    assert_eq!(shapes.len(), 1);
    assert_eq!(
        shapes[0].polygons,
        ShapePreset::SlopeDown.polygons().expect("a ring")
    );
    assert!(lookup.get(5).is_empty());
    assert!(lookup.get(99).is_empty());
    assert_eq!(*tileset.physics_layer_names(), ["World".to_string()]);
}

#[test]
fn a_sheet_outside_setup_takes_collision_that_a_setup_edit_keeps() {
    let mut tileset = tileset();
    let solid = TilesetCollision {
        layer: 0,
        trigger: false,
        one_way: true,
        preset: CollisionPreset::Solid,
        polygons: CollisionPreset::Solid.polygons().expect("a ring"),
    };
    assert_eq!(tileset.set_collision(0, 1, solid.clone()), Ok(true));
    assert_eq!(tileset.set_collision(0, 1, solid.clone()), Ok(false));
    assert!(
        tileset.set_collision(0, 9, solid.clone()).is_err(),
        "past the sheet"
    );
    let mut setup = tileset.setup().expect("tiles outside setup are left out");
    setup.add_terrain("grass", [1, 2, 3], false).expect("id");
    tileset.set_setup(&setup).expect("converts");
    assert_eq!(tileset.collision(0, 1), [solid]);
    assert_eq!(tileset.collision_lookup().get(1).len(), 1);
    assert_eq!(tileset.clear_collision(0, 1, 0), Ok(true));
    assert!(
        tileset.tiles.iter().all(|t| t.sheet != 0),
        "a blank tile goes"
    );
    assert_eq!(tileset.clear_collision(0, 1, 0), Ok(false));
}
