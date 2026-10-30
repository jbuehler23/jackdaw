//! The Tile Palette window and the brush it loads: terrains and tiles of the
//! active layer's tileset, picked through operators, flipped and rotated, and
//! the Bucket's Contiguous toggle.

use crate::tile_tools::{click, enter, entries, run, stored, stroke};
use crate::tilemap_render::{
    grass_patch, ground_terrain, layer, project_app, settle, spawn_map, tile_layer,
};

use bevy::prelude::*;
use bevy::ui::Checked;
use bevy::ui_widgets::Activate;
use jackdaw::selection::Selection;
use jackdaw::tile_palette::{
    ContiguousToggle, PaletteBody, PaletteCell, PaletteItem, PaletteSection, PaletteSelected,
    TILE_PALETTE_WINDOW_ID, TilePalette,
};
use jackdaw::tile_tools::{BrushSource, TileBrush, TileEditMode, TileTool};
use jackdaw_api::prelude::*;
use jackdaw_scene_types::{PropertyValue, TilemapLayerKind};
use jackdaw_tilemap::{CellCoord, TileRef};
use jackdaw_tileset::{TerrainMatch, Tileset};

const PICK: &str = "tilemap.palette.pick";

fn call(
    app: &mut App,
    id: &'static str,
    params: &[(&'static str, PropertyValue)],
) -> OperatorResult {
    let mut call = app.world_mut().operator(id);
    for (name, value) in params {
        call = call.param(*name, value.clone());
    }
    let result = call.call().expect("dispatches");
    settle(app);
    result
}

#[track_caller]
fn ok(app: &mut App, id: &'static str, params: &[(&'static str, PropertyValue)]) {
    assert_eq!(call(app, id, params), OperatorResult::Finished, "{id}");
}

fn text(value: &str) -> PropertyValue {
    PropertyValue::from(value.to_string())
}

fn available(app: &mut App, id: &str) -> bool {
    app.world_mut()
        .operator(id.to_string())
        .is_available()
        .unwrap_or(false)
}

fn brush(app: &App) -> TileBrush {
    *app.world().resource::<TileBrush>()
}

fn tileset(app: &App) -> Tileset {
    (app.world().resource::<Assets<Tileset>>().iter())
        .next()
        .map(|(_, t)| t.clone())
        .expect("the tileset")
}

fn packed(app: &App, sheet: u32, tile: u32) -> TileRef {
    let layer = tileset(app).layer(sheet, tile).expect("a tile");
    TileRef::new(u16::try_from(layer).expect("fits"))
}

fn tile_map(app: &mut App, cells: &[(i32, i32, TileRef)]) -> Entity {
    let map = spawn_map(
        app,
        vec![layer(TilemapLayerKind::Tile, default())],
        tile_layer(cells),
    );
    enter(app, map);
    map
}

#[test]
fn picking_a_terrain_loads_it_and_selects_the_brush() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    enter(&mut app, map);
    run(&mut app, "tilemap.rectangle");
    assert_eq!(
        call(&mut app, PICK, &[("terrain", text("2"))]),
        OperatorResult::Finished
    );
    let picked = brush(&app);
    assert_eq!((picked.source, picked.terrain), (BrushSource::Terrain, 2));
    assert_eq!(app.world().resource::<TileEditMode>().tool, TileTool::Brush);

    assert_eq!(
        call(&mut app, PICK, &[("terrain", text("lava"))]),
        OperatorResult::Cancelled
    );
    assert_eq!(brush(&app), picked, "a refused pick leaves the brush");

    run(&mut app, "tilemap.line");
    ok(&mut app, PICK, &[("tile", text("0:5"))]);
    assert_eq!(
        app.world().resource::<TileEditMode>().tool,
        TileTool::Brush,
        "a tile pick chooses the Brush too"
    );
}

#[test]
fn a_picked_tile_paints_hand_placed_tiles_over_terrain() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    enter(&mut app, map);
    assert_eq!(
        call(&mut app, PICK, &[("tile", text("0:5"))]),
        OperatorResult::Finished
    );
    let tile = packed(&app, 0, 5);
    assert_eq!(
        (brush(&app).source, brush(&app).tile),
        (BrushSource::Tile, tile)
    );
    let count = entries(&app);
    stroke(&mut app, "tilemap.brush", MouseButton::Left, &[(2, 2)]);
    assert_eq!(entries(&app), count + 1);
    let data = stored(&mut app, map);
    let authoring = data.layers[0].authoring.as_ref().expect("authoring");
    assert_eq!(authoring.overrides.get(&CellCoord::new(2, 2)), Some(&tile));
    assert_eq!(
        authoring.terrain.get(CellCoord::new(2, 2)),
        1,
        "terrain stays"
    );
}

#[test]
fn flip_and_rotate_turn_the_loaded_tile_and_what_it_paints() {
    let (mut app, _dir) = project_app();
    let map = tile_map(&mut app, &[]);
    assert_eq!(
        brush(&app).source,
        BrushSource::Terrain,
        "the default source"
    );
    let plain = brush(&app).tile;
    for id in [
        "tilemap.brush.flip_x",
        "tilemap.brush.flip_y",
        "tilemap.brush.rotate",
    ] {
        assert!(available(&mut app, id), "a tile layer paints tiles: {id}");
    }

    run(&mut app, "tilemap.brush.flip_x");
    assert_eq!(brush(&app).tile, plain.with_flip_x(true));
    run(&mut app, "tilemap.brush.flip_y");
    assert_eq!(brush(&app).tile, plain.with_flip_x(true).with_flip_y(true));
    run(&mut app, "tilemap.brush.flip_x");
    run(&mut app, "tilemap.brush.flip_y");
    assert_eq!(brush(&app).tile, plain);

    run(&mut app, "tilemap.brush.rotate");
    let quarter = brush(&app).tile;
    assert_eq!(quarter, plain.with_transpose(true).with_flip_x(true));
    run(&mut app, "tilemap.brush.rotate");
    assert_eq!(
        brush(&app).tile,
        plain.with_flip_x(true).with_flip_y(true),
        "two quarter turns are a half turn"
    );
    run(&mut app, "tilemap.brush.rotate");
    run(&mut app, "tilemap.brush.rotate");
    assert_eq!(brush(&app).tile, plain, "four quarter turns are none");

    run(&mut app, "tilemap.brush.rotate");
    stroke(&mut app, "tilemap.brush", MouseButton::Left, &[(1, 1)]);
    assert_eq!(
        stored(&mut app, map).layers[0]
            .tiles
            .get(CellCoord::new(1, 1)),
        quarter
    );
}

#[test]
fn a_terrain_brush_on_a_terrain_layer_does_not_turn() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    enter(&mut app, map);
    for id in [
        "tilemap.brush.flip_x",
        "tilemap.brush.flip_y",
        "tilemap.brush.rotate",
    ] {
        assert!(!available(&mut app, id), "{id} needs a tile brush");
    }
    ok(&mut app, PICK, &[("tile", text("0:3"))]);
    assert!(available(&mut app, "tilemap.brush.rotate"));
}

#[test]
fn the_contiguous_toggle_reaches_a_bucket_press() {
    let (mut app, _dir) = project_app();
    let five = TileRef::new(5);
    let map = tile_map(&mut app, &[(0, 0, five), (4, 4, five)]);
    ok(&mut app, PICK, &[("tile", text("0:7"))]);
    assert!(brush(&app).contiguous, "contiguous at first");
    assert_eq!(
        call(&mut app, "tilemap.bucket.contiguous", &[]),
        OperatorResult::Finished
    );
    assert!(!brush(&app).contiguous, "unset toggles");

    assert_eq!(
        click(&mut app, "tilemap.bucket", MouseButton::Left, (0, 0)),
        OperatorResult::Finished
    );
    let tiles = stored(&mut app, map).layers[0].tiles.clone();
    let seven = packed(&app, 0, 7);
    assert_eq!(tiles.get(CellCoord::new(0, 0)), seven);
    assert_eq!(
        tiles.get(CellCoord::new(4, 4)),
        seven,
        "the press filled across the layer"
    );

    ok(
        &mut app,
        "tilemap.bucket.contiguous",
        &[("contiguous", PropertyValue::Bool(true))],
    );
    assert!(brush(&app).contiguous);
}

fn open_palette(app: &mut App) -> Entity {
    ok(
        app,
        "window.open",
        &[("window_id", text(TILE_PALETTE_WINDOW_ID))],
    );
    app.world_mut()
        .query_filtered::<Entity, With<TilePalette>>()
        .single(app.world())
        .expect("one palette")
}

fn sections(app: &mut App) -> Vec<PaletteSection> {
    app.world_mut()
        .query::<&PaletteSection>()
        .iter(app.world())
        .copied()
        .collect()
}

fn cells(app: &mut App) -> Vec<(Entity, PaletteItem)> {
    let mut found: Vec<(Entity, PaletteItem)> = app
        .world_mut()
        .query::<(Entity, &PaletteCell)>()
        .iter(app.world())
        .map(|(e, c)| (e, c.item))
        .collect();
    found.sort_by_key(|(e, _)| *e);
    found
}

fn selected(app: &mut App) -> Vec<PaletteItem> {
    app.world_mut()
        .query_filtered::<&PaletteCell, With<PaletteSelected>>()
        .iter(app.world())
        .map(|c| c.item)
        .collect()
}

#[test]
fn the_palette_shows_the_active_layers_terrains_and_tiles() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    open_palette(&mut app);
    assert_eq!(sections(&mut app), vec![PaletteSection::Empty]);

    enter(&mut app, map);
    let mut shown = sections(&mut app);
    shown.sort();
    assert_eq!(shown, vec![PaletteSection::Terrains, PaletteSection::Tiles]);
    let set = tileset(&app);
    let all = cells(&mut app);
    let terrains = all
        .iter()
        .filter(|(_, i)| matches!(i, PaletteItem::Terrain(_)))
        .count();
    let tiles = all
        .iter()
        .filter(|(_, i)| matches!(i, PaletteItem::Tile(..)))
        .count();
    let sheet_tiles: u32 = set.sheets.iter().map(|s| s.columns * s.rows).sum();
    assert_eq!(terrains, set.terrains.len());
    assert_eq!(tiles, sheet_tiles as usize);
    assert_eq!(selected(&mut app), vec![PaletteItem::Terrain(1)]);

    let (water, _) = *all
        .iter()
        .find(|(_, i)| *i == PaletteItem::Terrain(2))
        .expect("terrain 2");
    app.world_mut().trigger(Activate { entity: water });
    settle(&mut app);
    assert_eq!(brush(&app).terrain, 2, "a click picks through the operator");
    assert_eq!(selected(&mut app), vec![PaletteItem::Terrain(2)]);

    let (four, _) = *cells(&mut app)
        .iter()
        .find(|(_, i)| *i == PaletteItem::Tile(0, 4))
        .expect("tile 0:4");
    app.world_mut().trigger(Activate { entity: four });
    settle(&mut app);
    assert_eq!(brush(&app).source, BrushSource::Tile);
    assert_eq!(brush(&app).tile, packed(&app, 0, 4));
    assert_eq!(selected(&mut app), vec![PaletteItem::Tile(0, 4)]);

    run(&mut app, "tilemap.edit_mode.exit");
    assert_eq!(sections(&mut app), vec![PaletteSection::Empty]);
}

fn body(app: &mut App) -> Entity {
    app.world_mut()
        .query_filtered::<Entity, With<PaletteBody>>()
        .single(app.world())
        .expect("one palette body")
}

#[test]
fn a_pick_keeps_the_cells_and_the_scroll() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    open_palette(&mut app);
    enter(&mut app, map);
    let before = cells(&mut app);
    let scrolled = body(&mut app);
    app.world_mut()
        .entity_mut(scrolled)
        .insert(ScrollPosition(Vec2::new(0.0, 40.0)));
    ok(&mut app, PICK, &[("tile", text("0:4"))]);
    ok(&mut app, "tilemap.bucket.contiguous", &[]);
    assert_eq!(cells(&mut app), before, "the same cell entities");
    assert_eq!(body(&mut app), scrolled);
    assert_eq!(
        app.world().get::<ScrollPosition>(scrolled).map(|s| s.0),
        Some(Vec2::new(0.0, 40.0))
    );
    assert_eq!(selected(&mut app), vec![PaletteItem::Tile(0, 4)]);
    let unchecked = app
        .world_mut()
        .query_filtered::<(), (With<ContiguousToggle>, Without<Checked>)>()
        .iter(app.world())
        .count();
    assert_eq!(unchecked, 1, "the toggle follows the brush");
}

#[test]
fn a_tile_layer_shows_tiles_only_and_highlights_the_tile() {
    let (mut app, _dir) = project_app();
    open_palette(&mut app);
    tile_map(&mut app, &[]);
    assert_eq!(sections(&mut app), vec![PaletteSection::Tiles]);
    let held = tileset(&app)
        .tile_of_layer(u32::from(brush(&app).tile.index()))
        .expect("a tile");
    assert_eq!(
        selected(&mut app),
        vec![PaletteItem::Tile(held.0, held.1)],
        "the terrain source still paints a tile here"
    );
}

#[test]
fn leaving_by_selection_empties_the_palette() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    open_palette(&mut app);
    enter(&mut app, map);
    app.world_mut().resource_mut::<Selection>().entities.clear();
    settle(&mut app);
    assert_eq!(app.world().resource::<TileEditMode>().map, None);
    assert_eq!(sections(&mut app), vec![PaletteSection::Empty]);
}

#[test]
fn a_layer_naming_no_tileset_says_why() {
    let (mut app, _dir) = project_app();
    let mut missing = ground_terrain();
    missing.tileset = "missing.tileset.bsn".to_string();
    let map = spawn_map(&mut app, vec![missing], grass_patch());
    open_palette(&mut app);
    enter(&mut app, map);
    let hint: Vec<String> = app
        .world_mut()
        .query_filtered::<&Text, With<PaletteSection>>()
        .iter(app.world())
        .map(|t| t.0.clone())
        .collect();
    assert_eq!(hint.len(), 1);
    assert!(hint[0].contains("not an asset file"), "{hint:?}");
}

#[test]
fn a_corner_set_terrain_shows_its_corner_tile() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    open_palette(&mut app);
    enter(&mut app, map);
    let set = tileset(&app);
    let corner_only: Vec<u8> = (set.terrains.iter())
        .map(|t| t.id)
        .filter(|&id| {
            let sets: Vec<_> = (set.terrain_sets.iter())
                .filter(|s| s.inner == id || s.outer == id)
                .collect();
            !sets.is_empty() && sets.iter().all(|s| s.mode == TerrainMatch::Corners)
        })
        .filter(|&id| {
            (set.tiles.iter())
                .any(|t| t.set_inner == id && [0, 2, 6, 8].iter().all(|&p| t.peers[p] == id))
        })
        .collect();
    assert!(
        !corner_only.is_empty(),
        "the fixture has a corners-only terrain"
    );
    let drawn: Vec<u8> = app
        .world_mut()
        .query::<(&PaletteCell, &Children)>()
        .iter(app.world())
        .filter_map(|(cell, children)| match cell.item {
            PaletteItem::Terrain(id) if !children.is_empty() => Some(id),
            _ => None,
        })
        .collect();
    for id in corner_only {
        assert!(
            drawn.contains(&id),
            "terrain {id} shows a tile, not a colour"
        );
    }
}

#[test]
fn a_tile_bucket_on_terrain_matches_the_shown_tiles() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    enter(&mut app, map);
    let data = stored(&mut app, map);
    let shown = |data: &jackdaw_tilemap::TilemapData, x: i32, y: i32| {
        let cell = CellCoord::new(x, y);
        let authoring = data.layers[0].authoring.as_ref().expect("authoring");
        (authoring.overrides.get(&cell).copied()).unwrap_or(data.layers[0].tiles.get(cell))
    };
    let start = shown(&data, 2, 2);
    ok(&mut app, PICK, &[("tile", text("0:200"))]);
    let tile = brush(&app).tile;
    let count = entries(&app);
    assert_eq!(
        click(&mut app, "tilemap.bucket", MouseButton::Left, (2, 2)),
        OperatorResult::Finished
    );
    assert_eq!(entries(&app), count + 1);
    let after = stored(&mut app, map);
    let authoring = after.layers[0].authoring.as_ref().expect("authoring");
    assert!(!authoring.overrides.is_empty());
    for cell in authoring.overrides.keys() {
        assert_eq!(
            shown(&data, cell.0.x, cell.0.y),
            start,
            "{cell:?} showed the pressed tile"
        );
    }
    assert_eq!(authoring.overrides.get(&CellCoord::new(2, 2)), Some(&tile));
    assert_eq!(
        after.layers[0]
            .authoring
            .as_ref()
            .map(|a| a.terrain.clone()),
        data.layers[0].authoring.as_ref().map(|a| a.terrain.clone()),
        "terrain stays"
    );

    assert_eq!(
        click(&mut app, "tilemap.bucket", MouseButton::Left, (2, 2)),
        OperatorResult::Finished
    );
    assert_eq!(entries(&app), count + 1, "a repeat fill changes nothing");
}

#[test]
fn the_picker_copies_a_hand_placed_tile_as_a_tile() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    enter(&mut app, map);
    ok(&mut app, PICK, &[("tile", text("0:5"))]);
    stroke(&mut app, "tilemap.brush", MouseButton::Left, &[(2, 2)]);
    ok(&mut app, PICK, &[("terrain", text("2"))]);

    assert_eq!(
        click(&mut app, "tilemap.picker", MouseButton::Left, (2, 2)),
        OperatorResult::Finished
    );
    let placed = brush(&app);
    assert_eq!(
        (placed.source, placed.tile),
        (BrushSource::Tile, packed(&app, 0, 5))
    );
    assert_eq!(
        click(&mut app, "tilemap.picker", MouseButton::Left, (3, 3)),
        OperatorResult::Finished
    );
    let plain = brush(&app);
    assert_eq!((plain.source, plain.terrain), (BrushSource::Terrain, 1));
}

#[test]
fn brush_params_naming_both_follow_the_layer() {
    let both = |app: &mut App| {
        app.world_mut()
            .operator("tilemap.brush")
            .param("terrain", text("2"))
            .param("tile", text("0:5"))
            .call()
            .expect("dispatches")
    };
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    enter(&mut app, map);
    assert_eq!(both(&mut app), OperatorResult::Finished);
    assert_eq!(
        brush(&app).source,
        BrushSource::Terrain,
        "terrain wins on terrain"
    );

    let (mut app, _dir) = project_app();
    tile_map(&mut app, &[]);
    assert_eq!(both(&mut app), OperatorResult::Finished);
    assert_eq!(brush(&app).source, BrushSource::Tile, "tile wins on tiles");
}
