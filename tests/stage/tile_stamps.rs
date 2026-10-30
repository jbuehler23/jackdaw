//! Stamps: a rectangle of palette tiles dragged into a temporary stamp, the
//! Brush placing it as one block per cell step, its preview, flips and turns,
//! and saving it with the tileset.

use crate::tile_palette::{
    available, brush, call, cells, ok, open_palette, packed, sections, selected, text, tile_map,
    tileset,
};
use crate::tile_tools::{bytes, enter, entries, run, stored, stroke};
use crate::tilemap_render::{TILESET, grass_patch, ground_terrain, project_app, settle, spawn_map};

use bevy::{
    camera::{NormalizedRenderTarget, RenderTarget},
    picking::{
        backend::HitData,
        events::{Pointer, PointerDragEnd, PointerDragEnter, PointerDragStart, PointerPress},
        pointer::{Location, PointerButton, PointerId},
    },
    prelude::*,
    window::{PrimaryWindow, WindowRef},
};
use jackdaw::selection::Selection;
use jackdaw::tile_palette::{PaletteItem, PaletteSection};
use jackdaw::tile_preview::{TileHover, TilePreview};
use jackdaw::tile_stamp::{StampBrush, TileStamp};
use jackdaw::tile_tools::BrushSource;
use jackdaw_api::prelude::*;
use jackdaw_scene_types::TilemapLayerKind;
use jackdaw_tilemap::{CellCoord, TileRef};
use jackdaw_tileset::Tileset;

const PICK: &str = "tilemap.palette.pick";

fn columns(app: &App) -> u32 {
    tileset(app).sheets[0].columns
}

/// Loads the 2 x 2 block at the top-left of sheet 0 as a stamp, and its
/// tiles row by row.
fn two_by_two(app: &mut App) -> [TileRef; 4] {
    let corner = format!("0:{}", columns(app) + 1);
    ok(app, PICK, &[("tile", text("0:0")), ("to", text(&corner))]);
    let c = columns(app);
    [
        packed(app, 0, 0),
        packed(app, 0, 1),
        packed(app, 0, c),
        packed(app, 0, c + 1),
    ]
}

fn loaded(app: &App) -> TileStamp {
    (app.world().resource::<StampBrush>().stamp.clone()).expect("a stamp")
}

#[test]
fn a_rectangle_of_tiles_becomes_a_stamp() {
    let (mut app, _dir) = project_app();
    tile_map(&mut app, &[]);
    let [a, b, c, d] = two_by_two(&mut app);
    assert_eq!(brush(&app).source, BrushSource::Stamp);
    let stamp = loaded(&app);
    assert_eq!(stamp.size, UVec2::new(2, 2));
    assert_eq!(
        stamp.cells,
        vec![
            (UVec2::new(0, 0), a),
            (UVec2::new(1, 0), b),
            (UVec2::new(0, 1), c),
            (UVec2::new(1, 1), d)
        ]
    );
    let reversed = format!("0:{}", columns(&app) + 1);
    ok(
        &mut app,
        PICK,
        &[("tile", text(&reversed)), ("to", text("0:0"))],
    );
    assert_eq!(loaded(&app), stamp, "either corner first");
}

fn pointer(app: &mut App) -> Pointer {
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(app.world())
        .expect("a primary window");
    let target: NormalizedRenderTarget = RenderTarget::Window(WindowRef::Primary)
        .normalize(Some(window))
        .expect("the primary window normalizes");
    Pointer::new(
        PointerId::Mouse,
        Location {
            target,
            position: Vec2::ZERO,
        },
    )
}

fn cell_of(app: &mut App, item: PaletteItem) -> Entity {
    (cells(app).into_iter())
        .find(|(_, i)| *i == item)
        .map(|(e, _)| e)
        .expect("the palette cell")
}

#[test]
fn dragging_over_palette_tiles_highlights_and_loads_a_stamp() {
    let (mut app, _dir) = project_app();
    open_palette(&mut app);
    tile_map(&mut app, &[]);
    let c = columns(&app);
    let from = cell_of(&mut app, PaletteItem::Tile(0, 0));
    let to = cell_of(&mut app, PaletteItem::Tile(0, c + 1));
    let hit = HitData::new(from, 0.0, None, None);
    let pointer = pointer(&mut app);
    app.world_mut().trigger(PointerDragStart {
        entity: from,
        pointer: pointer.clone(),
        button: PointerButton::Primary,
        hit: hit.clone(),
    });
    app.world_mut().trigger(PointerDragEnter {
        entity: to,
        pointer: pointer.clone(),
        button: PointerButton::Primary,
        dragged: from,
        hit,
    });
    settle(&mut app);
    let mut lit = selected(&mut app);
    lit.sort_by_key(|item| format!("{item:?}"));
    let mut rect = vec![
        PaletteItem::Tile(0, 0),
        PaletteItem::Tile(0, 1),
        PaletteItem::Tile(0, c),
        PaletteItem::Tile(0, c + 1),
    ];
    rect.sort_by_key(|item| format!("{item:?}"));
    assert_eq!(lit, rect, "the dragged rectangle is lit while dragging");

    app.world_mut().trigger(PointerDragEnd {
        entity: from,
        pointer,
        button: PointerButton::Primary,
        distance: Vec2::new(40.0, 40.0),
    });
    settle(&mut app);
    assert_eq!(brush(&app).source, BrushSource::Stamp);
    assert_eq!(loaded(&app).size, UVec2::new(2, 2));
    let mut lit = selected(&mut app);
    lit.sort_by_key(|item| format!("{item:?}"));
    assert_eq!(lit, rect, "the stamp's tiles stay lit");
}

#[test]
fn the_brush_places_a_stamp_per_cell_as_one_entry() {
    let (mut app, _dir) = project_app();
    let map = tile_map(&mut app, &[]);
    let [a, b, c, d] = two_by_two(&mut app);
    let (before, count) = (bytes(&mut app, map), entries(&app));
    stroke(
        &mut app,
        "tilemap.brush",
        MouseButton::Left,
        &[(0, 0), (1, 0)],
    );
    assert_eq!(entries(&app), count + 1, "one stroke, one entry");
    let tiles = stored(&mut app, map).layers[0].tiles.clone();
    let at = |x, y| tiles.get(CellCoord::new(x, y));
    assert_eq!(
        [at(0, 0), at(0, -1)],
        [a, c],
        "the first block's left column"
    );
    assert_eq!([at(1, 0), at(2, 0), at(1, -1), at(2, -1)], [a, b, c, d]);
    assert_eq!(tiles.cells().count(), 6);
    let after = bytes(&mut app, map);
    run(&mut app, "history.undo");
    assert_eq!(bytes(&mut app, map), before);
    run(&mut app, "history.redo");
    assert_eq!(bytes(&mut app, map), after);
}

#[test]
fn other_tools_refuse_a_stamp() {
    let (mut app, _dir) = project_app();
    let map = tile_map(&mut app, &[]);
    two_by_two(&mut app);
    let before = bytes(&mut app, map);
    for id in ["tilemap.rectangle", "tilemap.line"] {
        run(&mut app, id);
        let mut state = app
            .world_mut()
            .resource_mut::<jackdaw::tile_tools::TileStrokeState>();
        state.cell = Some(CellCoord::new(0, 0));
        state.button = Some(MouseButton::Left);
        let result = app
            .world_mut()
            .operator(id)
            .settings(CallOperatorSettings {
                execution_context: ExecutionContext::Invoke,
                creates_history_entry: false,
            })
            .call()
            .expect("dispatches");
        assert_eq!(result, OperatorResult::Cancelled, "{id}");
        settle(&mut app);
    }
    assert_eq!(bytes(&mut app, map), before);
}

#[test]
fn a_stamp_over_terrain_places_hand_placed_tiles() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    enter(&mut app, map);
    let [a, ..] = two_by_two(&mut app);
    stroke(&mut app, "tilemap.brush", MouseButton::Left, &[(3, 3)]);
    let data = stored(&mut app, map);
    let authoring = data.layers[0].authoring.as_ref().expect("authoring");
    assert_eq!(authoring.overrides.len(), 4);
    assert_eq!(authoring.overrides.get(&CellCoord::new(3, 3)), Some(&a));
}

fn preview_tiles(app: &App) -> Vec<(CellCoord, TileRef)> {
    let mut tiles = app.world().resource::<TilePreview>().tiles.clone();
    tiles.sort_by_key(|(cell, _)| *cell);
    tiles
}

#[test]
fn the_preview_shows_the_stamp_under_the_hover_cell() {
    let (mut app, _dir) = project_app();
    tile_map(&mut app, &[]);
    let [a, b, c, d] = two_by_two(&mut app);
    app.world_mut().resource_mut::<TileHover>().cell = Some(CellCoord::new(5, 5));
    settle(&mut app);
    let cell = CellCoord::new;
    assert_eq!(app.world().resource::<TilePreview>().cells.len(), 4);
    assert_eq!(
        preview_tiles(&app),
        vec![
            (cell(5, 4), c),
            (cell(6, 4), d),
            (cell(5, 5), a),
            (cell(6, 5), b)
        ]
    );
    run(&mut app, "tilemap.brush.flip_x");
    assert_eq!(
        preview_tiles(&app)[2],
        (cell(5, 5), b.with_flip_x(true)),
        "the preview follows a flip"
    );
}

#[test]
fn flip_and_rotate_turn_a_stamp() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    enter(&mut app, map);
    ok(
        &mut app,
        PICK,
        &[("tile", text("0:0")), ("to", text("0:1"))],
    );
    assert!(available(&mut app, "tilemap.brush.rotate"), "a stamp turns");
    let (a, b) = (packed(&app, 0, 0), packed(&app, 0, 1));
    run(&mut app, "tilemap.brush.flip_x");
    assert_eq!(
        loaded(&app).cells,
        vec![
            (UVec2::new(1, 0), a.with_flip_x(true)),
            (UVec2::new(0, 0), b.with_flip_x(true))
        ]
    );
    run(&mut app, "tilemap.brush.flip_x");
    run(&mut app, "tilemap.brush.rotate");
    let turned = loaded(&app);
    assert_eq!(turned.size, UVec2::new(1, 2), "a row turns into a column");
    let quarter = |t: TileRef| t.with_transpose(true).with_flip_x(true);
    assert_eq!(
        turned.cells,
        vec![
            (UVec2::new(0, 0), quarter(a)),
            (UVec2::new(0, 1), quarter(b))
        ]
    );
    run(&mut app, "tilemap.brush.flip_y");
    assert_eq!(loaded(&app).cells[0].0, UVec2::new(0, 1));
}

#[test]
fn a_saved_stamp_persists_with_the_tileset_and_undoes() {
    let (mut app, dir) = project_app();
    open_palette(&mut app);
    tile_map(&mut app, &[]);
    two_by_two(&mut app);
    run(&mut app, "tilemap.brush.rotate");
    let block = loaded(&app);
    let count = entries(&app);
    ok(&mut app, "tilemap.stamp.save", &[("name", text("well"))]);
    assert_eq!(entries(&app), count + 1, "one entry");
    assert_eq!(
        app.world().resource::<StampBrush>().name.as_deref(),
        Some("well")
    );
    let file = dir.path().join("assets").join(TILESET);
    let on_disk = || jackdaw_bsn::read_asset_file::<Tileset>(&file).expect("reads back");
    assert_eq!(on_disk().stamps.len(), 1);
    assert_eq!(on_disk().stamps[0].name, "well");
    assert_eq!(tileset(&app).stamps, on_disk().stamps);
    assert_eq!(
        call(&mut app, "tilemap.stamp.save", &[("name", text("well"))]),
        OperatorResult::Cancelled,
        "the loaded stamp is saved already"
    );
    assert!(sections(&mut app).contains(&PaletteSection::Stamps));
    assert_eq!(selected(&mut app), vec![PaletteItem::Stamp(0)]);

    ok(&mut app, PICK, &[("tile", text("0:3"))]);
    let saved = cell_of(&mut app, PaletteItem::Stamp(0));
    app.world_mut()
        .trigger(bevy::ui_widgets::Activate { entity: saved });
    settle(&mut app);
    assert_eq!(brush(&app).source, BrushSource::Stamp);
    assert_eq!(loaded(&app), block, "the saved stamp loads as it was saved");

    run(&mut app, "history.undo");
    assert!(tileset(&app).stamps.is_empty());
    assert!(on_disk().stamps.is_empty());
    run(&mut app, "history.redo");
    assert_eq!(on_disk().stamps.len(), 1);
}

#[test]
fn a_click_then_an_unrelated_drag_loads_no_stamp() {
    let (mut app, _dir) = project_app();
    open_palette(&mut app);
    tile_map(&mut app, &[]);
    let clicked = cell_of(&mut app, PaletteItem::Tile(0, 0));
    let other = cell_of(&mut app, PaletteItem::Tile(0, 3));
    let pointer = pointer(&mut app);
    let hit = HitData::new(clicked, 0.0, None, None);
    app.world_mut().trigger(PointerPress {
        entity: clicked,
        pointer: pointer.clone(),
        button: PointerButton::Primary,
        hit: hit.clone(),
        count: 1,
    });
    settle(&mut app);
    let elsewhere = app.world_mut().spawn_empty().id();
    app.world_mut().trigger(PointerDragEnter {
        entity: other,
        pointer: pointer.clone(),
        button: PointerButton::Primary,
        dragged: elsewhere,
        hit,
    });
    app.world_mut().trigger(PointerDragEnd {
        entity: elsewhere,
        pointer,
        button: PointerButton::Primary,
        distance: Vec2::new(40.0, 0.0),
    });
    settle(&mut app);
    assert_ne!(brush(&app).source, BrushSource::Stamp);
    assert!(app.world().resource::<StampBrush>().stamp.is_none());
}

#[test]
fn a_stamp_from_another_tileset_is_refused() {
    let (mut app, _dir) = project_app();
    let map = tile_map(&mut app, &[]);
    two_by_two(&mut app);
    app.world_mut().resource_mut::<StampBrush>().tileset = Some(AssetId::default());
    let (before, count) = (bytes(&mut app, map), entries(&app));
    assert_eq!(
        crate::tile_tools::click(&mut app, "tilemap.brush", MouseButton::Left, (0, 0)),
        OperatorResult::Cancelled
    );
    assert_eq!(bytes(&mut app, map), before);
    assert_eq!(entries(&app), count);
}

#[test]
fn entering_a_map_on_another_tileset_drops_the_stamp() {
    let (mut app, dir) = project_app();
    let assets = dir.path().join("assets");
    std::fs::copy(assets.join("overworld.png"), assets.join("second.png")).expect("copy");
    ok(
        &mut app,
        "tileset.import",
        &[("paths", text("second.png")), ("setup", text("auto"))],
    );
    tile_map(&mut app, &[]);
    two_by_two(&mut app);
    let mut other = crate::tilemap_render::layer(TilemapLayerKind::Tile, default());
    other.tileset = "second.tileset.bsn".to_string();
    let second = spawn_map(
        &mut app,
        vec![other],
        crate::tilemap_render::tile_layer(&[]),
    );
    app.world_mut().resource_mut::<Selection>().entities = vec![second];
    settle(&mut app);
    enter(&mut app, second);
    assert!(app.world().resource::<StampBrush>().stamp.is_none());
    assert_ne!(brush(&app).source, BrushSource::Stamp);
}

#[test]
fn a_stamp_index_is_not_shadowed_by_a_stamp_name() {
    let (mut app, _dir) = project_app();
    open_palette(&mut app);
    tile_map(&mut app, &[]);
    two_by_two(&mut app);
    ok(&mut app, "tilemap.stamp.save", &[("name", text("1"))]);
    ok(
        &mut app,
        PICK,
        &[("tile", text("0:5")), ("to", text("0:7"))],
    );
    let row = loaded(&app);
    ok(&mut app, "tilemap.stamp.save", &[("name", text("row"))]);
    ok(&mut app, PICK, &[("tile", text("0:3"))]);
    let second = cell_of(&mut app, PaletteItem::Stamp(1));
    app.world_mut()
        .trigger(bevy::ui_widgets::Activate { entity: second });
    settle(&mut app);
    assert_eq!(
        loaded(&app),
        row,
        "the button loads stamp 1, not the stamp named 1"
    );
}

#[test]
fn leaving_the_stamp_source_drops_the_stamp() {
    let (mut app, _dir) = project_app();
    tile_map(&mut app, &[(4, 4, TileRef::new(9))]);
    two_by_two(&mut app);
    assert!(available(&mut app, "tilemap.stamp.save"));
    assert_eq!(
        crate::tile_tools::click(&mut app, "tilemap.picker", MouseButton::Left, (4, 4)),
        OperatorResult::Finished
    );
    assert_eq!(brush(&app).source, BrushSource::Tile);
    assert!(app.world().resource::<StampBrush>().stamp.is_none());
    assert!(!available(&mut app, "tilemap.stamp.save"));

    two_by_two(&mut app);
    assert_eq!(
        crate::tile_tools::set_brush(&mut app, "tile", "0:2"),
        OperatorResult::Finished
    );
    settle(&mut app);
    assert!(app.world().resource::<StampBrush>().stamp.is_none());
    assert!(!available(&mut app, "tilemap.stamp.save"));
}

#[test]
fn turning_a_saved_stamp_makes_it_unsaved() {
    let (mut app, _dir) = project_app();
    tile_map(&mut app, &[]);
    two_by_two(&mut app);
    ok(&mut app, "tilemap.stamp.save", &[("name", text("well"))]);
    assert!(!available(&mut app, "tilemap.stamp.save"));
    run(&mut app, "tilemap.brush.flip_x");
    assert_eq!(app.world().resource::<StampBrush>().name, None);
    assert!(
        available(&mut app, "tilemap.stamp.save"),
        "a flipped stamp is new"
    );
}

#[test]
fn undoing_a_save_makes_the_stamp_unsaved() {
    let (mut app, _dir) = project_app();
    open_palette(&mut app);
    tile_map(&mut app, &[]);
    two_by_two(&mut app);
    ok(&mut app, "tilemap.stamp.save", &[("name", text("well"))]);
    run(&mut app, "history.undo");
    assert!(available(&mut app, "tilemap.stamp.save"));
    ok(&mut app, "tilemap.stamp.save", &[("name", text("well"))]);
    assert_eq!(tileset(&app).stamps.len(), 1);
}
