//! The tile tools' live preview, Connect and Path strokes, and the hover
//! readout in the status bar.

use std::collections::BTreeSet;

use crate::tile_tools::{assert_resolved, enter, hold, run, set_brush, stored, stroke};
use crate::tilemap_render::{grass_patch, ground_terrain, project_app, settle, spawn_map};

use bevy::prelude::*;
use jackdaw::tile_preview::{
    TileHover, TileHoverReadout, TilePreview, TilePreviewChunk, TilePreviewEmptied,
};
use jackdaw::tile_tools::{StrokeMode, TileBrush, TileEditMode, TileStrokeState};
use jackdaw::tilemap::TilemapDataStore;
use jackdaw::tilemap_stroke::{TileWrite, paint_layer};
use jackdaw_api::prelude::*;
use jackdaw_feathers::status_bar::StatusBarRight;
use jackdaw_scene_types::PropertyValue;
use jackdaw_tilemap::{
    CellCoord, ChunkedLayer, LayerData, LayerKind, Links, MatchMode, Peer, TerrainAuthoring,
    TerrainSet, TerrainTile, TileRef, TileTerrains, TilemapData,
};
use jackdaw_tileset::Tileset;

const BRUSH: &str = "tilemap.brush";
const ERASER: &str = "tilemap.eraser";
const RECTANGLE: &str = "tilemap.rectangle";
const LINE: &str = "tilemap.line";

fn hover(app: &mut App, cell: Option<(i32, i32)>) {
    app.world_mut().resource_mut::<TileHover>().cell = cell.map(|(x, y)| CellCoord::new(x, y));
    settle(app);
}

fn preview(app: &App) -> Vec<(CellCoord, TileRef)> {
    let mut tiles = app.world().resource::<TilePreview>().tiles.clone();
    tiles.sort_unstable_by_key(|(c, _)| *c);
    tiles
}

/// The cells of layer 0 whose tile differs between `before` and `after`,
/// with their tile in `after`.
fn diff(before: &TilemapData, after: &TilemapData) -> Vec<(CellCoord, TileRef)> {
    let (old, new) = (&before.layers[0].tiles, &after.layers[0].tiles);
    let cells: BTreeSet<CellCoord> = old.cells().chain(new.cells()).map(|(c, _)| c).collect();
    let mut changed: Vec<_> = (cells.into_iter())
        .filter(|&c| old.get(c) != new.get(c))
        .map(|c| (c, new.get(c)))
        .collect();
    changed.sort_unstable_by_key(|(c, _)| *c);
    changed
}

fn grass_map(app: &mut App) -> Entity {
    let map = spawn_map(app, vec![ground_terrain()], grass_patch());
    enter(app, map);
    map
}

fn preview_chunks(app: &mut App) -> usize {
    (app.world_mut()
        .query::<&TilePreviewChunk>()
        .iter(app.world()))
    .count()
}

#[test]
fn the_hover_preview_shows_what_a_brush_press_writes() {
    let (mut app, _dir) = project_app();
    let map = grass_map(&mut app);
    assert_eq!(
        set_brush(&mut app, "terrain", "2"),
        OperatorResult::Finished
    );
    hover(&mut app, Some((2, 2)));
    let shown = preview(&app);
    assert!(!shown.is_empty(), "the press would change tiles");
    assert_eq!(
        app.world().resource::<TilePreview>().cells,
        [CellCoord::new(2, 2)]
    );
    assert!(preview_chunks(&mut app) > 0, "the changed tiles are drawn");

    let before = stored(&mut app, map);
    stroke(&mut app, BRUSH, MouseButton::Left, &[(2, 2)]);
    assert_eq!(diff(&before, &stored(&mut app, map)), shown);
    assert_resolved(&mut app, map);
}

#[test]
fn a_rectangle_drag_previews_what_its_release_writes() {
    let (mut app, _dir) = project_app();
    let map = grass_map(&mut app);
    assert_eq!(
        set_brush(&mut app, "terrain", "2"),
        OperatorResult::Finished
    );
    run(&mut app, RECTANGLE);
    let before = stored(&mut app, map);
    hold(&mut app, RECTANGLE, MouseButton::Left, &[(1, 1), (3, 4)]);
    settle(&mut app);
    let shown = preview(&app);
    assert_eq!(app.world().resource::<TilePreview>().cells.len(), 12);
    assert!(!shown.is_empty());

    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    settle(&mut app);
    assert_eq!(diff(&before, &stored(&mut app, map)), shown);
}

#[test]
fn an_eraser_hover_previews_what_erasing_the_cell_does() {
    let (mut app, _dir) = project_app();
    let map = grass_map(&mut app);
    run(&mut app, ERASER);
    hover(&mut app, Some((0, 0)));
    let shown = preview(&app);
    assert!(
        shown
            .iter()
            .any(|(c, t)| *c == CellCoord::new(0, 0) && t.is_empty())
    );

    let before = stored(&mut app, map);
    stroke(&mut app, ERASER, MouseButton::Left, &[(0, 0)]);
    assert_eq!(diff(&before, &stored(&mut app, map)), shown);
}

#[test]
fn the_preview_is_built_only_when_its_inputs_change_and_clears_on_leaving() {
    let (mut app, _dir) = project_app();
    grass_map(&mut app);
    assert_eq!(
        set_brush(&mut app, "terrain", "2"),
        OperatorResult::Finished
    );
    hover(&mut app, Some((2, 2)));
    let builds = app.world().resource::<TilePreview>().builds;
    settle(&mut app);
    hover(&mut app, Some((2, 2)));
    assert_eq!(app.world().resource::<TilePreview>().builds, builds);
    hover(&mut app, Some((3, 2)));
    assert_eq!(app.world().resource::<TilePreview>().builds, builds + 1);

    run(&mut app, "tilemap.edit_mode.exit");
    let preview = app.world().resource::<TilePreview>();
    assert!(preview.cells.is_empty() && preview.tiles.is_empty());
    assert_eq!(preview_chunks(&mut app), 0);
    assert_eq!(app.world().resource::<TileHoverReadout>().0, None);
}

fn set_mode(app: &mut App, id: &'static str, mode: &str) -> OperatorResult {
    app.world_mut()
        .operator(id)
        .param("mode", PropertyValue::from(mode.to_string()))
        .call()
        .expect("the tool dispatches")
}

#[test]
fn the_mode_param_sets_the_brush_mode() {
    let (mut app, _dir) = project_app();
    grass_map(&mut app);
    assert_eq!(
        app.world().resource::<TileBrush>().mode,
        StrokeMode::Connect
    );
    for id in [BRUSH, LINE, RECTANGLE] {
        assert_eq!(set_mode(&mut app, id, "path"), OperatorResult::Finished);
        assert_eq!(app.world().resource::<TileBrush>().mode, StrokeMode::Path);
        assert_eq!(set_mode(&mut app, id, "connect"), OperatorResult::Finished);
        assert_eq!(
            app.world().resource::<TileBrush>().mode,
            StrokeMode::Connect
        );
    }
    assert_eq!(set_mode(&mut app, BRUSH, "loop"), OperatorResult::Cancelled);
    assert_eq!(
        app.world().resource::<TileBrush>().mode,
        StrokeMode::Connect
    );
}

/// Two lines of terrain 2 over the grass, on rows 2 and 3, in `mode`.
fn two_roads(mode: &str) -> (TilemapData, Vec<(CellCoord, TileRef)>) {
    let (mut app, _dir) = project_app();
    let map = grass_map(&mut app);
    assert_eq!(
        set_brush(&mut app, "terrain", "2"),
        OperatorResult::Finished
    );
    assert_eq!(set_mode(&mut app, LINE, mode), OperatorResult::Finished);
    stroke(&mut app, LINE, MouseButton::Left, &[(0, 2), (5, 2)]);
    hold(&mut app, LINE, MouseButton::Left, &[(0, 3), (5, 3)]);
    settle(&mut app);
    let shown = preview(&app);
    let before = stored(&mut app, map);
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    settle(&mut app);
    let after = stored(&mut app, map);
    assert_eq!(diff(&before, &after), shown, "{mode}: the drag previews it");
    assert_resolved(&mut app, map);
    (after, shown)
}

#[test]
fn parallel_path_roads_stay_apart_and_connect_roads_join() {
    let (path, _) = two_roads("path");
    let (connect, _) = two_roads("connect");
    let links = |data: &TilemapData, x, y| {
        let authoring = data.layers[0].authoring.as_ref().expect("authoring");
        authoring.links.get(CellCoord::new(x, y))
    };
    for x in 0..=5 {
        assert!(!links(&path, x, 2).has(Peer::Top), "{x}: path rows apart");
        assert!(!links(&path, x, 3).has(Peer::Bottom));
        assert!(links(&path, x, 2).is_path());
        assert_eq!(links(&connect, x, 2), Links::EMPTY);
    }
    for x in 1..5 {
        assert!(links(&path, x, 2).has(Peer::Left) && links(&path, x, 2).has(Peer::Right));
    }
    assert_ne!(
        path.layers[0].tiles, connect.layers[0].tiles,
        "the roads resolve apart in one mode and joined in the other"
    );
}

/// A sides set for terrain 1 whose tile index is the mask of its joined
/// sides, as [`Links`] lays the side bits out.
fn mask_set() -> TerrainSet {
    let mut set = TerrainSet::new(MatchMode::Sides);
    for mask in 0..16u8 {
        let mut peers = TileTerrains::filled(0).with(Peer::Centre, 1);
        for side in Peer::SIDES {
            if mask & Links::side_bit(side) != 0 {
                peers = peers.with(side, 1);
            }
        }
        set = set.with_tile(TerrainTile::new(
            TileRef::new(u16::from(mask) + 1),
            peers,
            1,
        ));
    }
    set
}

fn terrain_layer() -> LayerData {
    LayerData {
        kind: LayerKind::Terrain,
        tiles: ChunkedLayer::new(),
        authoring: Some(TerrainAuthoring::default()),
    }
}

fn row(y: i32) -> Vec<CellCoord> {
    (0..4).map(|x| CellCoord::new(x, y)).collect()
}

#[test]
fn path_writes_join_only_their_own_cells() {
    let set = mask_set();
    let joined = |write: fn(u8) -> TileWrite| {
        let mut layer = terrain_layer();
        paint_layer(&mut layer, true, Some(&set), &row(0), write(1));
        paint_layer(&mut layer, true, Some(&set), &row(1), write(1));
        u8::try_from(layer.tiles.get(CellCoord::new(1, 0)).index() - 1).expect("a mask")
    };
    let path = joined(TileWrite::Path);
    assert_eq!(path, Links::LEFT | Links::RIGHT, "the rows stay apart");
    let connect = joined(TileWrite::Terrain);
    assert_eq!(connect, Links::LEFT | Links::RIGHT | Links::TOP);
}

#[test]
fn painting_returns_exactly_the_tiles_it_changes() {
    let set = mask_set();
    let mut layer = terrain_layer();
    let first = paint_layer(&mut layer, true, Some(&set), &row(0), TileWrite::Terrain(1));
    assert_eq!(first.len(), 4);
    let again = paint_layer(&mut layer, true, Some(&set), &row(0), TileWrite::Terrain(1));
    assert!(again.is_empty(), "nothing changes the second time");
    let erased = paint_layer(
        &mut layer,
        true,
        Some(&set),
        &[CellCoord::new(3, 0)],
        TileWrite::Erase,
    );
    assert_eq!(
        erased,
        [
            (
                CellCoord::new(2, 0),
                TileRef::new(1 + u16::from(Links::LEFT))
            ),
            (CellCoord::new(3, 0), TileRef::EMPTY)
        ]
    );
}

fn status_text(app: &mut App) -> Option<String> {
    (app.world_mut()
        .query_filtered::<&Text, With<StatusBarRight>>())
    .iter(app.world())
    .next()
    .map(|text| text.0.clone())
}

#[test]
fn the_hover_readout_names_the_cell_terrain_tile_and_override() {
    let (mut app, _dir) = project_app();
    let map = grass_map(&mut app);
    hover(&mut app, Some((2, 2)));
    let tileset = (app.world().resource::<Assets<Tileset>>().iter())
        .next()
        .map(|(_, t)| t.clone())
        .expect("the tileset");
    let grass = (tileset.terrains.iter().find(|t| t.id == 1))
        .map(|t| t.name.clone())
        .expect("terrain 1");
    let tile = stored(&mut app, map).layers[0]
        .tiles
        .get(CellCoord::new(2, 2));
    let (sheet, at) = tileset
        .tile_of_layer(u32::from(tile.index()))
        .expect("a resolved tile");
    let expected = format!("Cell 2,2 | Terrain {grass} | Tile {sheet}:{at}");
    assert_eq!(
        app.world().resource::<TileHoverReadout>().0.as_deref(),
        Some(expected.as_str())
    );
    if let Some(text) = status_text(&mut app) {
        assert_eq!(text, expected, "the status bar shows it");
    }

    let placed = app
        .world_mut()
        .operator("tilemap.set_cells")
        .param("entity", PropertyValue::Entity(map))
        .param("layer", PropertyValue::from("0".to_string()))
        .param("cells", PropertyValue::from("2,2".to_string()))
        .param("kind", PropertyValue::from("tile".to_string()))
        .param("value", PropertyValue::from("0:3".to_string()))
        .call()
        .expect("tilemap.set_cells dispatches");
    assert_eq!(placed, OperatorResult::Finished);
    settle(&mut app);
    let readout = app.world().resource::<TileHoverReadout>().0.clone();
    assert_eq!(
        readout.as_deref(),
        Some(format!("Cell 2,2 | Terrain {grass} | Tile 0:3 | Hand-placed").as_str())
    );

    hover(&mut app, None);
    assert_eq!(app.world().resource::<TileHoverReadout>().0, None);
    assert_eq!(app.world().resource::<TileEditMode>().map, Some(map));
}

fn emptied_runs(app: &mut App) -> usize {
    (app.world_mut()
        .query::<&TilePreviewEmptied>()
        .iter(app.world()))
    .count()
}

#[test]
fn cells_an_erase_would_empty_are_tinted() {
    let (mut app, _dir) = project_app();
    grass_map(&mut app);
    run(&mut app, ERASER);
    hover(&mut app, Some((0, 0)));
    assert_eq!(emptied_runs(&mut app), 1);
    run(&mut app, BRUSH);
    settle(&mut app);
    assert_eq!(emptied_runs(&mut app), 0);
}

#[test]
fn a_huge_rectangle_drag_is_outlined_by_its_corners_and_built_once_per_cell() {
    let (mut app, _dir) = project_app();
    grass_map(&mut app);
    assert_eq!(
        set_brush(&mut app, "terrain", "2"),
        OperatorResult::Finished
    );
    run(&mut app, RECTANGLE);
    hold(
        &mut app,
        RECTANGLE,
        MouseButton::Left,
        &[(0, 0), (999, 999)],
    );
    settle(&mut app);
    let preview = app.world().resource::<TilePreview>();
    assert!(
        preview.cells.is_empty() && preview.tiles.is_empty(),
        "no million cell list"
    );
    assert_eq!(preview.edges.len(), 4);
    assert_eq!(preview.edges[0].0, IVec2::ZERO);
    assert_eq!(preview.edges[1].1, IVec2::splat(1000));
    let builds = preview.builds;
    settle(&mut app);
    assert_eq!(
        app.world().resource::<TilePreview>().builds,
        builds,
        "same drag cell"
    );
    app.world_mut().resource_mut::<TileStrokeState>().cell = Some(CellCoord::new(3, 3));
    settle(&mut app);
    let preview = app.world().resource::<TilePreview>();
    assert_eq!(preview.builds, builds + 1);
    assert_eq!(preview.cells.len(), 16);
}

fn resolved(app: &App) -> usize {
    app.world().resource::<TilemapDataStore>().resolved_cells()
}

fn set_cells(app: &mut App, map: Entity, kind: &str, value: &str) {
    let result = app
        .world_mut()
        .operator("tilemap.set_cells")
        .param("entity", PropertyValue::Entity(map))
        .param("layer", PropertyValue::from("0".to_string()))
        .param("cells", PropertyValue::from("2,2".to_string()))
        .param("kind", PropertyValue::from(kind.to_string()))
        .param("value", PropertyValue::from(value.to_string()))
        .call()
        .expect("tilemap.set_cells dispatches");
    assert_eq!(result, OperatorResult::Finished);
    settle(app);
}

#[test]
fn a_hand_placed_tile_and_its_erase_resolve_the_cell_and_its_ring_once() {
    let (mut app, _dir) = project_app();
    let map = grass_map(&mut app);
    for (kind, value) in [("tile", "0:3"), ("tile", "erase"), ("terrain", "2")] {
        let before = resolved(&app);
        set_cells(&mut app, map, kind, value);
        assert_eq!(resolved(&app) - before, 9, "{kind} {value}");
    }
    assert_resolved(&mut app, map);
}

#[test]
fn holding_alt_previews_only_the_picked_cell() {
    let (mut app, _dir) = project_app();
    grass_map(&mut app);
    assert_eq!(
        set_brush(&mut app, "terrain", "2"),
        OperatorResult::Finished
    );
    hover(&mut app, Some((2, 2)));
    let shown = preview(&app);
    assert!(!shown.is_empty());
    let alt = |app: &mut App, held: bool| {
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        if held {
            keys.press(KeyCode::AltLeft);
        } else {
            keys.release(KeyCode::AltLeft);
        }
        settle(app);
    };
    alt(&mut app, true);
    assert!(app.world().resource::<TilePreview>().tiles.is_empty());
    assert_eq!(
        app.world().resource::<TilePreview>().cells,
        [CellCoord::new(2, 2)]
    );
    assert_eq!(preview_chunks(&mut app), 0);
    alt(&mut app, false);
    assert_eq!(preview(&app), shown);
}

#[test]
fn the_readout_follows_the_tileset_loading_and_changing() {
    let (mut app, _dir) = project_app();
    grass_map(&mut app);
    hover(&mut app, Some((2, 2)));
    let readout = |app: &App| {
        app.world()
            .resource::<TileHoverReadout>()
            .0
            .clone()
            .unwrap_or_default()
    };
    let (id, tileset) = (app.world().resource::<Assets<Tileset>>().iter())
        .next()
        .map(|(id, t)| (id, t.clone()))
        .expect("the tileset");
    let named = readout(&app);

    app.world_mut().resource_mut::<Assets<Tileset>>().remove(id);
    settle(&mut app);
    assert!(readout(&app).contains("Terrain 1 |"), "{}", readout(&app));

    app.world_mut()
        .resource_mut::<Assets<Tileset>>()
        .insert(id, tileset)
        .expect("the id is free");
    settle(&mut app);
    assert_eq!(readout(&app), named, "loaded again");

    {
        let mut tilesets = app.world_mut().resource_mut::<Assets<Tileset>>();
        let mut edited = tilesets.get_mut(id).expect("loaded");
        let terrain = (edited.terrains.iter_mut())
            .find(|t| t.id == 1)
            .expect("terrain 1");
        terrain.name = "Meadow".to_string();
    }
    settle(&mut app);
    assert!(
        readout(&app).contains("Terrain Meadow |"),
        "{}",
        readout(&app)
    );
}

/// A 2D panel framing the world origin at one pixel per unit.
fn panel(app: &mut App) -> Entity {
    use bevy::window::{PrimaryWindow, WindowResolution};
    let mut windows = (app.world_mut()).query_filtered::<&mut Window, With<PrimaryWindow>>();
    if let Ok(mut window) = windows.single_mut(app.world_mut()) {
        window.resolution = WindowResolution::new(1600, 1000);
    }
    let panel = app
        .world_mut()
        .spawn((
            jackdaw::EditorEntity,
            Node {
                width: px(1200.0),
                height: px(800.0),
                ..default()
            },
        ))
        .id();
    jackdaw::viewport_2d::build_viewport_2d_panel(app.world_mut(), panel);
    let mut host = app
        .world_mut()
        .get_mut::<jackdaw::viewport_2d::Viewport2dPanelHost>(panel)
        .expect("a panel host");
    host.set_view(jackdaw::viewport_2d::Ui2dView {
        pan: Vec2::ZERO,
        zoom: 1.0,
        grid: 16.0,
    });
    host.fit_pending = false;
    settle(app);
    panel
}

fn move_pointer(app: &mut App, at: Vec2) {
    let clause = format!(
        "input.pointer space=world x={} y={} action=move",
        at.x, at.y
    );
    let result = jackdaw::boot_ops::run_op_clause(app.world_mut(), &clause).expect("dispatches");
    assert_eq!(result, OperatorResult::Finished);
    for _ in 0..200 {
        app.update();
        if app
            .world()
            .resource::<jackdaw::test_input::SyntheticInput>()
            .is_idle()
        {
            break;
        }
    }
    settle(app);
}

#[test]
fn the_hover_clears_when_the_panel_stops_painting_the_map() {
    let (mut app, _dir) = project_app();
    let panel = panel(&mut app);
    grass_map(&mut app);
    move_pointer(&mut app, Vec2::new(40.0, 40.0));
    assert_eq!(
        app.world().resource::<TileHover>().cell,
        Some(CellCoord::new(2, 2))
    );
    assert!(app.world().resource::<TileHoverReadout>().0.is_some());

    app.world_mut()
        .get_mut::<jackdaw::viewport_2d::Viewport2dPanelHost>(panel)
        .expect("a panel host")
        .mode = jackdaw::viewport_2d::Viewport2dMode::Interact;
    move_pointer(&mut app, Vec2::new(56.0, 40.0));
    assert_eq!(app.world().resource::<TileHover>().cell, None);
    assert_eq!(app.world().resource::<TileHoverReadout>().0, None);
    assert!(app.world().resource::<TilePreview>().cells.is_empty());
}
