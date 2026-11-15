//! The Tile Palette window and the brush it loads: terrains and tiles of the
//! active layer's tileset, picked through operators, flipped and rotated, the
//! Terrains tab with its stroke mode and patterns, and the Bucket's
//! Contiguous toggle.

use crate::stage::tile_tools::{click, enter, entries, run, stored, stroke};
use crate::stage::tilemap_render::{
    grass_patch, ground_terrain, layer, project_app, settle, spawn_map, tile_layer,
};

use bevy::feathers::controls::ButtonVariant;
use bevy::prelude::*;
use bevy::ui::{Checked, ComputedNode, InteractionDisabled, UiGlobalTransform};
use bevy::ui_widgets::{Activate, ValueChange};
use jackdaw_api::prelude::*;
use jackdaw_editor_core::selection::Selection;
use jackdaw_editor_tile::tile_palette::{
    ContiguousToggle, MIN_TILE_PX, PaletteBody, PaletteCell, PaletteHeader, PaletteItem,
    PaletteSection, PaletteSelected, StrokeModeSegment, TILE_PALETTE_WINDOW_ID, TerrainPatterns,
    TilePalette, full_tiles,
};
use jackdaw_editor_tile::tile_tools::{
    BrushSource, StrokeMode, TileBrush, TileEditMode, TileTool,
};
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_feathers::tab_strip::TabStripTab;
use jackdaw_feathers::tooltip::Tooltip;
use jackdaw_scene_types::{PropertyValue, TilemapLayerKind};
use jackdaw_tilemap::{CellCoord, TerrainId, TileRef};
use jackdaw_tileset::{TerrainMatch, Tileset};

const PICK: &str = "tilemap.palette.pick";
const TAB: &str = "tilemap.palette.tab";
const STROKE_MODE: &str = "tilemap.brush.stroke_mode";

pub(crate) fn call(
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
pub(crate) fn ok(app: &mut App, id: &'static str, params: &[(&'static str, PropertyValue)]) {
    assert_eq!(call(app, id, params), OperatorResult::Finished, "{id}");
}

pub(crate) fn text(value: &str) -> PropertyValue {
    PropertyValue::from(value.to_string())
}

pub(crate) fn available(app: &mut App, id: &str) -> bool {
    app.world_mut()
        .operator(id.to_string())
        .is_available()
        .unwrap_or(false)
}

pub(crate) fn brush(app: &App) -> TileBrush {
    *app.world().resource::<TileBrush>()
}

pub(crate) fn tileset(app: &App) -> Tileset {
    (app.world().resource::<Assets<Tileset>>().iter())
        .next()
        .map(|(_, t)| t.clone())
        .expect("the tileset")
}

pub(crate) fn packed(app: &App, sheet: u32, tile: u32) -> TileRef {
    let layer = tileset(app).layer(sheet, tile).expect("a tile");
    TileRef::new(u16::try_from(layer).expect("fits"))
}

pub(crate) fn tile_map(app: &mut App, cells: &[(i32, i32, TileRef)]) -> Entity {
    let map = spawn_map(
        app,
        vec![layer(TilemapLayerKind::Tile, default())],
        tile_layer(cells),
    );
    enter(app, map);
    map
}

#[test]
fn picking_a_terrain_loads_it_and_keeps_a_painting_tool() {
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
    assert_eq!(
        app.world().resource::<TileEditMode>().tool,
        TileTool::Rectangle,
        "a pick keeps the Rectangle"
    );
    stroke(
        &mut app,
        "tilemap.rectangle",
        MouseButton::Left,
        &[(5, 5), (6, 6)],
    );
    let data = stored(&mut app, map);
    let authoring = data.layers[&0].authoring.as_ref().expect("authoring");
    assert_eq!(
        authoring.terrain.get(CellCoord::new(6, 6)),
        2,
        "the Rectangle paints the picked terrain"
    );

    assert_eq!(
        call(&mut app, PICK, &[("terrain", text("lava"))]),
        OperatorResult::Cancelled
    );
    assert_eq!(brush(&app), picked, "a refused pick leaves the brush");

    for (id, kept) in [
        ("tilemap.line", TileTool::Line),
        ("tilemap.bucket", TileTool::Bucket),
        ("tilemap.brush", TileTool::Brush),
    ] {
        run(&mut app, id);
        ok(&mut app, PICK, &[("tile", text("0:5"))]);
        assert_eq!(app.world().resource::<TileEditMode>().tool, kept, "{id}");
    }
    for id in ["tilemap.eraser", "tilemap.picker", "tilemap.select"] {
        run(&mut app, id);
        ok(&mut app, PICK, &[("terrain", text("2"))]);
        assert_eq!(
            app.world().resource::<TileEditMode>().tool,
            TileTool::Brush,
            "{id} gives way to the Brush"
        );
    }
    for id in ["tilemap.rectangle", "tilemap.line", "tilemap.bucket"] {
        run(&mut app, id);
        ok(
            &mut app,
            PICK,
            &[("tile", text("0:5")), ("to", text("0:33"))],
        );
        assert_eq!(brush(&app).source, BrushSource::Stamp);
        assert_eq!(
            app.world().resource::<TileEditMode>().tool,
            TileTool::Brush,
            "a stamp pick after {id} chooses the Brush"
        );
    }
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
    let authoring = data.layers[&0].authoring.as_ref().expect("authoring");
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
        stored(&mut app, map).layers[&0]
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
    let tiles = stored(&mut app, map).layers[&0].tiles.clone();
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

pub(crate) fn open_palette(app: &mut App) -> Entity {
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

pub(crate) fn sections(app: &mut App) -> Vec<PaletteSection> {
    app.world_mut()
        .query::<&PaletteSection>()
        .iter(app.world())
        .copied()
        .collect()
}

pub(crate) fn cells(app: &mut App) -> Vec<(Entity, PaletteItem)> {
    let mut found: Vec<(Entity, PaletteItem)> = app
        .world_mut()
        .query::<(Entity, &PaletteCell)>()
        .iter(app.world())
        .map(|(e, c)| (e, c.item))
        .collect();
    found.sort_by_key(|(e, _)| *e);
    found
}

pub(crate) fn selected(app: &mut App) -> Vec<PaletteItem> {
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
    assert_eq!(
        sections(&mut app),
        vec![PaletteSection::Terrains],
        "a terrain layer opens on its Terrains tab"
    );
    let set = tileset(&app);
    let all = cells(&mut app);
    let terrains = all
        .iter()
        .filter(|(_, i)| matches!(i, PaletteItem::Terrain(_)))
        .count();
    assert_eq!(terrains, set.terrains.len());
    assert_eq!(selected(&mut app), vec![PaletteItem::Terrain(1)]);

    let (water, _) = *all
        .iter()
        .find(|(_, i)| *i == PaletteItem::Terrain(2))
        .expect("terrain 2");
    app.world_mut().trigger(Activate { entity: water });
    settle(&mut app);
    assert_eq!(brush(&app).terrain, 2, "a click picks through the operator");
    assert_eq!(selected(&mut app), vec![PaletteItem::Terrain(2)]);

    click_tab(&mut app, "tiles");
    let mut shown = sections(&mut app);
    shown.sort();
    assert_eq!(shown, vec![PaletteSection::Tiles, PaletteSection::Stamps]);
    let tiles = cells(&mut app)
        .iter()
        .filter(|(_, i)| matches!(i, PaletteItem::Tile(..)))
        .count();
    let sheet_tiles: u32 = set.sheets.iter().map(|s| s.columns * s.rows).sum();
    assert_eq!(tiles, sheet_tiles as usize);
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
    ok(&mut app, TAB, &[("tab", text("tiles"))]);
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
    let mut shown = sections(&mut app);
    shown.sort();
    assert_eq!(shown, vec![PaletteSection::Tiles, PaletteSection::Stamps]);
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
fn a_corner_set_terrain_lists_its_corner_tiles() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    open_palette(&mut app);
    enter(&mut app, map);
    let set = tileset(&app);
    let corner_only: Vec<u8> = (set.terrains.iter())
        .map(|t| t.id)
        .filter(|&id| {
            let all = &set.terrain_sets;
            let sets: Vec<_> = (all.iter()).filter(|s| s.terrains.contains(&id)).collect();
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
    for id in corner_only {
        ok(&mut app, PICK, &[("terrain", text(&id.to_string()))]);
        let shown = patterns(&mut app);
        assert!(
            shown.iter().any(|(_, (sheet, tile))| {
                (set.tiles.iter()).any(|t| {
                    (t.sheet, t.tile) == (*sheet, *tile)
                        && [0, 2, 6, 8].iter().all(|&p| t.peers[p] == id)
                })
            }),
            "terrain {id} lists a tile of its corners"
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
        let authoring = data.layers[&0].authoring.as_ref().expect("authoring");
        (authoring.overrides.get(&cell).copied()).unwrap_or(data.layers[&0].tiles.get(cell))
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
    let authoring = after.layers[&0].authoring.as_ref().expect("authoring");
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
        after.layers[&0]
            .authoring
            .as_ref()
            .map(|a| a.terrain.clone()),
        data.layers[&0]
            .authoring
            .as_ref()
            .map(|a| a.terrain.clone()),
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

#[test]
fn a_terrain_icon_leads_its_patterns() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    open_palette(&mut app);
    enter(&mut app, map);
    ok(
        &mut app,
        "tileset.terrain.add",
        &[
            ("path", text(crate::stage::tilemap_render::TILESET)),
            ("name", text("marsh")),
            ("icon", text("0:5")),
        ],
    );
    let set = tileset(&app);
    let marsh = set.terrains.last().expect("added");
    assert_eq!((marsh.name.as_str(), marsh.icon), ("marsh", Some((0, 5))));
    ok(&mut app, PICK, &[("terrain", text("marsh"))]);
    let (min, size) = set.sheet_rect(0, 5).expect("a tile");
    let shown = patterns(&mut app);
    let (first, at) = *shown.first().expect("the icon pattern");
    assert_eq!(at, (0, 5));
    let rects: Vec<Option<Rect>> = (app.world().get::<Children>(first).expect("a tile"))
        .iter()
        .map(|child| app.world().get::<ImageNode>(child).and_then(|i| i.rect))
        .collect();
    assert_eq!(
        rects,
        [Some(Rect::from_corners(
            min.as_vec2(),
            (min + size).as_vec2()
        ))]
    );
}

const TURNS: [&str; 3] = [
    "tilemap.brush.flip_x",
    "tilemap.brush.flip_y",
    "tilemap.brush.rotate",
];

/// The header's name, operator buttons and toggles, in order.
fn header_controls(app: &mut App) -> Vec<String> {
    fn walk(world: &World, entity: Entity, out: &mut Vec<String>) {
        if let Some(call) = world.get::<ButtonOperatorCall>(entity) {
            out.push(call.id.to_string());
        } else if world.get::<ContiguousToggle>(entity).is_some() {
            out.push("contiguous".to_string());
        } else if let Some(text) = world.get::<Text>(entity) {
            out.push(text.0.clone());
        } else if let Some(children) = world.get::<Children>(entity) {
            for child in children.iter() {
                walk(world, child, out);
            }
        }
    }
    let header = app
        .world_mut()
        .query_filtered::<Entity, With<PaletteHeader>>()
        .single(app.world())
        .expect("one palette header");
    let mut out = Vec::new();
    walk(app.world(), header, &mut out);
    out
}

/// The operator button calling `id` somewhere under `root`.
fn op_button_under(app: &mut App, root: Entity, id: &str) -> Option<Entity> {
    let found: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &ButtonOperatorCall)>()
        .iter(app.world())
        .filter(|(_, call)| call.id == id)
        .map(|(e, _)| e)
        .collect();
    found.into_iter().find(|&e| {
        let mut at = e;
        while let Some(parent) = app.world().get::<ChildOf>(at) {
            at = parent.parent();
            if at == root {
                return true;
            }
        }
        false
    })
}

fn header_button(app: &mut App, id: &str) -> Entity {
    let header = app
        .world_mut()
        .query_filtered::<Entity, With<PaletteHeader>>()
        .single(app.world())
        .expect("one palette header");
    op_button_under(app, header, id).unwrap_or_else(|| panic!("a header button for {id}"))
}

#[test]
fn the_header_holds_the_name_the_turns_save_and_contiguous_in_order() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    open_palette(&mut app);
    enter(&mut app, map);
    run(&mut app, "tilemap.bucket");
    let mut expected = vec!["overworld".to_string(), TAB.to_string(), TAB.to_string()];
    expected.extend(TURNS.iter().map(ToString::to_string));
    expected.extend(["tilemap.stamp.save".to_string(), "contiguous".to_string()]);
    assert_eq!(header_controls(&mut app), expected);
}

#[test]
fn the_turn_buttons_show_the_brush_and_wait_for_a_tile() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    open_palette(&mut app);
    enter(&mut app, map);
    for id in TURNS {
        let button = header_button(&mut app, id);
        assert!(
            app.world().get::<InteractionDisabled>(button).is_some(),
            "{id} waits for a tile"
        );
    }
    ok(&mut app, PICK, &[("tile", text("0:5"))]);
    run(&mut app, "tilemap.brush.flip_x");
    let variant = |app: &mut App, id: &str| {
        let button = header_button(app, id);
        assert!(
            app.world().get::<InteractionDisabled>(button).is_none(),
            "{id}"
        );
        app.world()
            .get::<ButtonVariant>(button)
            .cloned()
            .expect("a variant")
    };
    assert_eq!(variant(&mut app, TURNS[0]), ButtonVariant::Primary);
    assert_eq!(variant(&mut app, TURNS[1]), ButtonVariant::Plain);
    run(&mut app, "tilemap.brush.flip_x");
    assert_eq!(variant(&mut app, TURNS[0]), ButtonVariant::Plain);
}

#[test]
fn contiguous_shows_only_while_the_bucket_is_active() {
    let (mut app, _dir) = project_app();
    open_palette(&mut app);
    tile_map(&mut app, &[]);
    let display = |app: &mut App| {
        app.world_mut()
            .query_filtered::<&Node, With<ContiguousToggle>>()
            .single(app.world())
            .expect("one toggle")
            .display
    };
    assert_eq!(
        display(&mut app),
        Display::None,
        "the Brush has no Contiguous"
    );
    run(&mut app, "tilemap.bucket");
    assert_eq!(display(&mut app), Display::Flex);
    run(&mut app, "tilemap.line");
    assert_eq!(display(&mut app), Display::None);
}

#[test]
fn the_tiles_fit_the_dock_width() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    let palette = open_palette(&mut app);
    if let Some(mut node) = app.world_mut().get_mut::<Node>(palette) {
        node.max_width = px(500.0);
    }
    enter(&mut app, map);
    ok(&mut app, TAB, &[("tab", text("tiles"))]);
    let edges = |app: &App, entity: Entity| {
        let node = app.world().get::<ComputedNode>(entity).expect("laid out");
        let at = app
            .world()
            .get::<UiGlobalTransform>(entity)
            .expect("placed");
        let half = node.size().x / 2.0;
        (at.translation.x - half, at.translation.x + half)
    };
    let scrolled = body(&mut app);
    let (_, right) = edges(&app, scrolled);
    assert!(right > 0.0, "the body is laid out");
    let tiles: Vec<Entity> = cells(&mut app)
        .into_iter()
        .filter(|(_, item)| matches!(item, PaletteItem::Tile(..)))
        .map(|(e, _)| e)
        .collect();
    let widest = tiles
        .iter()
        .map(|&t| edges(&app, t).1)
        .fold(f32::MIN, f32::max);
    assert!(
        widest <= right + 0.5,
        "a tile at {widest} past the body at {right}"
    );
    let narrowest = tiles
        .iter()
        .map(|&t| {
            app.world()
                .get::<ComputedNode>(t)
                .map_or(0.0, |n| n.size().x)
        })
        .fold(f32::MAX, f32::min);
    assert!(narrowest >= 8.0, "tiles stay visible: {narrowest}");
}

#[test]
fn the_palette_tab_has_an_icon() {
    let (app, _dir) = project_app();
    let registry = app.world().resource::<jackdaw_panels::WindowRegistry>();
    let window = registry.get(TILE_PALETTE_WINDOW_ID).expect("registered");
    assert!(
        window
            .icon
            .as_deref()
            .is_some_and(|i| !i.is_empty() && i != "?"),
        "{:?}",
        window.icon
    );
}

fn palette_button(app: &mut App, id: &str) -> Option<Entity> {
    let palette = app
        .world_mut()
        .query_filtered::<Entity, With<TilePalette>>()
        .single(app.world())
        .expect("one palette");
    op_button_under(app, palette, id)
}

#[test]
fn the_empty_palette_enters_tile_edit_mode() {
    let (mut app, _dir) = project_app();
    open_palette(&mut app);
    assert!(
        palette_button(&mut app, "tilemap.create").is_some(),
        "no tilemap yet offers one"
    );
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    app.world_mut().resource_mut::<Selection>().entities.clear();
    settle(&mut app);
    assert_eq!(
        palette_button(&mut app, "tilemap.create"),
        None,
        "a tilemap exists"
    );
    let edit = palette_button(&mut app, "tilemap.edit_mode").expect("an edit button");
    assert!(
        app.world().get::<InteractionDisabled>(edit).is_some(),
        "waits for a selected tilemap"
    );
    app.world_mut().resource_mut::<Selection>().entities = vec![map];
    settle(&mut app);
    let edit = palette_button(&mut app, "tilemap.edit_mode").expect("an edit button");
    assert!(app.world().get::<InteractionDisabled>(edit).is_none());
    app.world_mut().trigger(Activate { entity: edit });
    settle(&mut app);
    assert_eq!(app.world().resource::<TileEditMode>().map, Some(map));
}

#[test]
fn pattern_and_tile_cells_share_the_selection_outline() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    open_palette(&mut app);
    enter(&mut app, map);
    let border = |app: &mut App, entity: Entity| app.world().get::<Node>(entity).expect("a node").border;
    let (pattern, _) = *patterns(&mut app).first().expect("a pattern");
    let pattern = border(&mut app, pattern);
    ok(&mut app, TAB, &[("tab", text("tiles"))]);
    let (tile, _) = *cells(&mut app)
        .iter()
        .find(|(_, i)| matches!(i, PaletteItem::Tile(..)))
        .expect("a tile cell");
    assert_eq!(pattern, border(&mut app, tile));
}

#[test]
fn a_sheet_wider_than_the_dock_scrolls_sideways_at_the_smallest_tiles() {
    let (mut app, dir) = project_app();
    let overworld = image::open(crate::util::OVERWORLD_SHEET)
        .expect("the sheet decodes")
        .to_rgba8();
    let mut wide = image::RgbaImage::new(64 * 16, 4 * 16);
    for (x, y, pixel) in wide.enumerate_pixels_mut() {
        *pixel = *overworld.get_pixel((x / 16 % 27) * 16 + x % 16, y);
    }
    wide.save(dir.path().join("assets/wide.png"))
        .expect("write the wide sheet");
    ok(
        &mut app,
        "tileset.import",
        &[("path", text("wide.png")), ("setup", text("none"))],
    );
    let mut wide_layer = layer(TilemapLayerKind::Tile, default());
    wide_layer.tileset = "wide.tileset.bsn".to_string();
    let map = spawn_map(&mut app, vec![wide_layer], tile_layer(&[]));
    let palette = open_palette(&mut app);
    if let Some(mut node) = app.world_mut().get_mut::<Node>(palette) {
        node.max_width = px(300.0);
    }
    enter(&mut app, map);
    let tiles: Vec<Entity> = cells(&mut app)
        .into_iter()
        .filter(|(_, item)| matches!(item, PaletteItem::Tile(..)))
        .map(|(e, _)| e)
        .collect();
    assert_eq!(tiles.len(), 64 * 4, "one sheet of 64 columns");
    for &tile in &tiles {
        let node = app.world().get::<ComputedNode>(tile).expect("laid out");
        let width = node.size().x * node.inverse_scale_factor();
        assert!(width >= MIN_TILE_PX - 0.5, "a tile {width} wide");
    }
    let mut at = tiles[0];
    let scroller = loop {
        let parent = app
            .world()
            .get::<ChildOf>(at)
            .expect("inside a scroller")
            .parent();
        if app
            .world()
            .get::<Node>(parent)
            .is_some_and(|n| n.overflow.x == OverflowAxis::Scroll)
        {
            break parent;
        }
        at = parent;
    };
    let node = app.world().get::<ComputedNode>(scroller).expect("laid out");
    assert!(
        node.content_size().x > node.size().x + 1.0,
        "the sheet overflows sideways: {:?} in {:?}",
        node.content_size(),
        node.size()
    );
}

/// Clicks the palette tab whose operator call names `tab`.
fn click_tab(app: &mut App, tab: &str) {
    let found = app
        .world_mut()
        .query_filtered::<(Entity, &ButtonOperatorCall), With<TabStripTab>>()
        .iter(app.world())
        .find(|(_, call)| {
            call.id == TAB && call.params.iter().any(|(_, v)| *v == text(tab))
        })
        .map(|(e, _)| e);
    let entity = found.unwrap_or_else(|| panic!("a {tab} tab"));
    app.world_mut().trigger(Activate { entity });
    settle(app);
}

/// The pattern cells of the Terrains tab in order, with their sheet and tile.
fn patterns(app: &mut App) -> Vec<(Entity, (u32, u32))> {
    let holder = app
        .world_mut()
        .query_filtered::<Entity, With<TerrainPatterns>>()
        .single(app.world())
        .expect("one pattern list");
    let mut out = Vec::new();
    let mut stack = vec![holder];
    while let Some(at) = stack.pop() {
        if let Some(PaletteCell {
            item: PaletteItem::Pattern(sheet, tile),
        }) = app.world().get::<PaletteCell>(at)
        {
            out.push((at, (*sheet, *tile)));
            continue;
        }
        if let Some(children) = app.world().get::<Children>(at) {
            stack.extend(children.iter().rev());
        }
    }
    out
}

/// The texts under the Terrains tab's terrain list, in order.
fn terrain_list(app: &mut App) -> Vec<String> {
    fn walk(world: &World, entity: Entity, out: &mut Vec<String>) {
        if world.get::<TerrainPatterns>(entity).is_some()
            || world.get::<StrokeModeSegment>(entity).is_some()
        {
            return;
        }
        if let Some(text) = world.get::<Text>(entity) {
            out.push(text.0.clone());
        }
        if let Some(children) = world.get::<Children>(entity) {
            for child in children.iter() {
                walk(world, child, out);
            }
        }
    }
    let section = app
        .world_mut()
        .query_filtered::<(Entity, &PaletteSection), ()>()
        .iter(app.world())
        .find(|(_, s)| **s == PaletteSection::Terrains)
        .map(|(e, _)| e)
        .expect("the Terrains tab");
    let mut out = Vec::new();
    walk(app.world(), section, &mut out);
    out
}

/// The terrain list a tileset shows: set names as headers only with two or
/// more sets, each set's own terrains by name.
fn expected_list(set: &Tileset) -> Vec<String> {
    let mut out: Vec<String> = (set.terrains.iter())
        .filter(|t| set.set_of(t.id).is_none())
        .map(|t| t.name.clone())
        .collect();
    for (index, each) in set.terrain_sets.iter().enumerate() {
        if set.terrain_sets.len() > 1 {
            out.push(each.name.clone());
        }
        out.extend(
            (each.terrains.iter())
                .filter(|id| set.set_of(**id) == Some(index))
                .map(|id| set.terrain_name(*id)),
        );
    }
    out
}

fn segment(app: &mut App, mode: StrokeMode) -> Entity {
    app.world_mut()
        .query::<(Entity, &StrokeModeSegment)>()
        .iter(app.world())
        .find(|(_, s)| s.0 == mode)
        .map(|(e, _)| e)
        .expect("a stroke mode segment")
}

fn checked_mode(app: &mut App) -> Vec<StrokeMode> {
    app.world_mut()
        .query_filtered::<&StrokeModeSegment, With<Checked>>()
        .iter(app.world())
        .map(|s| s.0)
        .collect()
}

/// Presses a segment of the Connect / Path toggle as its radio group does.
fn press_segment(app: &mut App, mode: StrokeMode) {
    let pressed = segment(app, mode);
    let bar = app.world().get::<ChildOf>(pressed).expect("in a bar").parent();
    app.world_mut().trigger(ValueChange::<Entity> {
        source: bar,
        value: pressed,
        is_final: true,
    });
    settle(app);
}

#[test]
fn the_connect_path_toggle_sets_the_mode_a_stroke_paints_with() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    open_palette(&mut app);
    enter(&mut app, map);
    assert_eq!(checked_mode(&mut app), vec![StrokeMode::Connect]);
    for mode in [StrokeMode::Connect, StrokeMode::Path] {
        let tip = app.world().get::<Tooltip>(segment(&mut app, mode)).expect("a tooltip");
        assert!(
            !tip.description.is_empty() && !tip.description.contains('\n'),
            "{mode:?} explains itself in one line"
        );
    }

    press_segment(&mut app, StrokeMode::Path);
    assert_eq!(brush(&app).mode, StrokeMode::Path);
    assert_eq!(checked_mode(&mut app), vec![StrokeMode::Path]);
    ok(&mut app, PICK, &[("terrain", text("2"))]);
    stroke(&mut app, "tilemap.line", MouseButton::Left, &[(0, 2), (5, 2)]);
    let data = stored(&mut app, map);
    let authoring = data.layers[&0].authoring.as_ref().expect("authoring");
    assert!(
        authoring.links.get(CellCoord::new(2, 2)).is_path(),
        "the stroke paints a path"
    );

    press_segment(&mut app, StrokeMode::Connect);
    assert_eq!(brush(&app).mode, StrokeMode::Connect);
    assert_eq!(checked_mode(&mut app), vec![StrokeMode::Connect]);
}

#[test]
fn the_stroke_mode_operator_sets_toggles_and_refuses_other_modes() {
    let (mut app, _dir) = project_app();
    ok(&mut app, STROKE_MODE, &[("mode", text("path"))]);
    assert_eq!(brush(&app).mode, StrokeMode::Path);
    ok(&mut app, STROKE_MODE, &[]);
    assert_eq!(brush(&app).mode, StrokeMode::Connect, "unset toggles");
    ok(&mut app, STROKE_MODE, &[]);
    assert_eq!(brush(&app).mode, StrokeMode::Path);
    assert_eq!(
        call(&mut app, STROKE_MODE, &[("mode", text("loop"))]),
        OperatorResult::Cancelled
    );
    assert_eq!(brush(&app).mode, StrokeMode::Path);
}

#[test]
fn terrain_names_show_in_rows_grouped_by_set() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    open_palette(&mut app);
    enter(&mut app, map);
    let set = tileset(&app);
    assert!(set.terrain_sets.len() > 1, "the fixture has several sets");
    let list = terrain_list(&mut app);
    assert_eq!(list, expected_list(&set));
    for terrain in &set.terrains {
        assert!(list.contains(&terrain.name), "{} is named", terrain.name);
    }
    for (row, item) in cells(&mut app) {
        if let PaletteItem::Terrain(id) = item {
            let children: Vec<Entity> =
                (app.world().get::<Children>(row).expect("a row")).iter().collect();
            assert_eq!(children.len(), 2, "terrain {id}: a swatch and a name");
            let swatch = app.world().get::<BackgroundColor>(children[0]).expect("a swatch").0;
            let own = set.terrains.iter().find(|t| t.id == id).expect("a terrain");
            assert_eq!(swatch, Color::from(own.swatch.with_alpha(1.0)));
            let name = app.world().get::<Text>(children[1]).expect("a name");
            assert_eq!(name.0, own.name);
        }
    }
}

#[test]
fn a_tileset_of_one_set_lists_its_terrains_without_a_header() {
    let (mut app, dir) = project_app();
    std::fs::copy(
        crate::util::OVERWORLD_SHEET,
        dir.path().join("assets/plain.png"),
    )
    .expect("copy the sheet");
    ok(
        &mut app,
        "tileset.import",
        &[("path", text("plain.png")), ("setup", text("none"))],
    );
    let path = "plain.tileset.bsn";
    for name in ["sand", "rock"] {
        ok(
            &mut app,
            "tileset.terrain.add",
            &[("path", text(path)), ("name", text(name))],
        );
    }
    let mut plain = ground_terrain();
    plain.tileset = path.to_string();
    let map = spawn_map(&mut app, vec![plain], grass_patch());
    open_palette(&mut app);
    enter(&mut app, map);
    let plain = |app: &App| {
        (app.world().resource::<Assets<Tileset>>().iter())
            .map(|(_, t)| t.clone())
            .find(|t| t.terrains.iter().any(|t| t.name == "sand"))
            .expect("the plain tileset")
    };
    assert_eq!(plain(&app).terrain_sets.len(), 1);
    assert_eq!(terrain_list(&mut app), ["sand", "rock"]);

    ok(
        &mut app,
        "tileset.terrain_set.add",
        &[("path", text(path)), ("name", text("Cliffs"))],
    );
    ok(
        &mut app,
        "tileset.terrain.add",
        &[
            ("path", text(path)),
            ("name", text("stone")),
            ("set", text("Cliffs")),
        ],
    );
    let set = plain(&app);
    assert_eq!(set.terrain_sets.len(), 2);
    let list = terrain_list(&mut app);
    assert_eq!(list, expected_list(&set));
    assert_eq!(list.len(), 5, "two headers and three terrains: {list:?}");
    assert_eq!(list[3], "Cliffs");
    assert_eq!(list[4], "stone");
}

#[test]
fn picking_a_terrain_arms_its_brush_and_lists_its_full_tiles() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    open_palette(&mut app);
    enter(&mut app, map);
    ok(&mut app, PICK, &[("tile", text("0:5"))]);
    let rows: Vec<(Entity, PaletteItem)> = cells(&mut app)
        .into_iter()
        .filter(|(_, i)| matches!(i, PaletteItem::Terrain(_)))
        .collect();
    let (water, _) = *rows
        .iter()
        .find(|(_, i)| *i == PaletteItem::Terrain(2))
        .expect("terrain 2");
    app.world_mut().trigger(Activate { entity: water });
    settle(&mut app);
    let armed = brush(&app);
    assert_eq!((armed.source, armed.terrain), (BrushSource::Terrain, 2));
    let set = tileset(&app);
    let listed: Vec<(u32, u32)> = patterns(&mut app).into_iter().map(|(_, at)| at).collect();
    assert_eq!(listed, full_tiles(&set, 2));
    assert!(!listed.is_empty(), "water has full tiles");
    for (sheet, tile) in &listed {
        let t = (set.tiles.iter())
            .find(|t| (t.sheet, t.tile) == (*sheet, *tile))
            .expect("a set up tile");
        let corners = [0, 2, 6, 8].iter().all(|&p| t.peers[p] == 2);
        let sides = [1, 3, 4, 5, 7].iter().all(|&p| t.peers[p] == 2);
        assert!(corners || sides, "{sheet}:{tile} is all water");
    }

    let after: Vec<(Entity, PaletteItem)> = cells(&mut app)
        .into_iter()
        .filter(|(_, i)| matches!(i, PaletteItem::Terrain(_)))
        .collect();
    assert_eq!(after, rows, "a pick keeps the terrain rows");

    let (first, (sheet, tile)) = patterns(&mut app)[0];
    app.world_mut().trigger(Activate { entity: first });
    settle(&mut app);
    let plain = brush(&app);
    assert_eq!(
        (plain.source, plain.tile),
        (BrushSource::Tile, packed(&app, sheet, tile)),
        "a pattern arms a plain tile brush"
    );
    assert_eq!(plain.terrain, 2, "the patterns stay on water");
    assert_eq!(selected(&mut app), vec![PaletteItem::Pattern(sheet, tile)]);
    stroke(&mut app, "tilemap.brush", MouseButton::Left, &[(2, 2)]);
    let data = stored(&mut app, map);
    let authoring = data.layers[&0].authoring.as_ref().expect("authoring");
    assert_eq!(
        authoring.overrides.get(&CellCoord::new(2, 2)),
        Some(&packed(&app, sheet, tile))
    );

    let other: TerrainId = 1;
    ok(&mut app, PICK, &[("terrain", text(&other.to_string()))]);
    let listed: Vec<(u32, u32)> = patterns(&mut app).into_iter().map(|(_, at)| at).collect();
    assert_eq!(listed, full_tiles(&set, other), "the patterns follow the pick");
}

#[test]
fn a_tile_layer_has_no_tabs_and_a_terrain_layer_switches_them() {
    let (mut app, _dir) = project_app();
    open_palette(&mut app);
    tile_map(&mut app, &[]);
    let tabs = |app: &mut App| {
        app.world_mut()
            .query_filtered::<(), With<TabStripTab>>()
            .iter(app.world())
            .count()
    };
    assert_eq!(tabs(&mut app), 0, "a tile layer shows its tiles alone");

    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    enter(&mut app, map);
    assert_eq!(tabs(&mut app), 2);
    assert_eq!(sections(&mut app), vec![PaletteSection::Terrains]);
    click_tab(&mut app, "tiles");
    assert!(!sections(&mut app).contains(&PaletteSection::Terrains));
    click_tab(&mut app, "terrains");
    assert_eq!(sections(&mut app), vec![PaletteSection::Terrains]);
    assert_eq!(
        call(&mut app, TAB, &[("tab", text("stamps"))]),
        OperatorResult::Cancelled
    );
}
