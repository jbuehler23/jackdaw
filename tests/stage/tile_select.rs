//! The Select tool: a dragged rectangle of cells on the active layer, erased,
//! copied into a stamp or cut as one history entry each, and cleared by
//! Escape before Escape leaves tile edit mode.

use crate::tile_palette::{brush, tile_map};
use crate::tile_tools::{assert_resolved, bytes, enter, entries, run, stored, stroke, tap};
use crate::tilemap_render::{
    grass_patch, ground_terrain, layer, project_app, settle, spawn_map, tile_layer,
};

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use jackdaw::tile_select::{TileArea, TileSelection, TileSelectionInputContext};
use jackdaw::tile_stamp::{StampBrush, TileStamp};
use jackdaw::tile_tools::{BrushSource, TileEditMode, TileStrokeState, TileTool};
use jackdaw_api::prelude::*;
use jackdaw_input::ContextActivity;
use jackdaw_scene_types::{PropertyValue, Tilemap, TilemapLayerKind};
use jackdaw_tilemap::{CellCoord, TileRef};

const SELECT: &str = "tilemap.select";
const ERASE: &str = "tilemap.selection.erase";
const COPY: &str = "tilemap.selection.copy";
const CUT: &str = "tilemap.selection.cut";

fn area(app: &App) -> Option<(CellCoord, CellCoord)> {
    (app.world().resource::<TileSelection>().area).map(|a: TileArea| (a.min, a.max))
}

fn cell(x: i32, y: i32) -> CellCoord {
    CellCoord::new(x, y)
}

/// Drags the Select tool from `from` to `to` and lets go.
fn select(app: &mut App, from: (i32, i32), to: (i32, i32)) {
    stroke(app, SELECT, MouseButton::Left, &[from, to]);
}

fn tile(app: &mut App, map: Entity, x: i32, y: i32) -> TileRef {
    stored(app, map).layers[0].tiles.get(cell(x, y))
}

#[track_caller]
fn assert_undo_redo(app: &mut App, map: Entity, before: &[u8], count: usize) {
    assert_eq!(entries(app), count + 1, "one entry");
    let after = bytes(app, map);
    assert_ne!(after, before);
    run(app, "history.undo");
    assert_eq!(bytes(app, map), before, "undo restores every cell");
    run(app, "history.redo");
    assert_eq!(bytes(app, map), after);
}

#[test]
fn dragging_with_select_selects_the_cell_rectangle() {
    let (mut app, _dir) = project_app();
    tile_map(&mut app, &[]);
    run(&mut app, SELECT);
    assert_eq!(
        app.world().resource::<TileEditMode>().tool,
        TileTool::Select
    );
    let count = entries(&app);

    crate::tile_tools::hold(&mut app, SELECT, MouseButton::Left, &[(2, 1), (0, 3)]);
    assert_eq!(
        area(&app),
        Some((cell(0, 1), cell(2, 3))),
        "the selection follows the drag"
    );
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    settle(&mut app);
    assert_eq!(area(&app), Some((cell(0, 1), cell(2, 3))), "it stays");
    assert!(!app.world_mut().operator(SELECT).is_running());
    assert_eq!(entries(&app), count, "selecting leaves no entry");

    run(&mut app, "tilemap.selection.clear");
    assert_eq!(area(&app), None);
}

#[test]
fn the_selection_operators_need_a_selection() {
    let (mut app, _dir) = project_app();
    tile_map(&mut app, &[]);
    for id in [ERASE, COPY, CUT, "tilemap.selection.clear"] {
        assert!(
            !crate::tile_palette::available(&mut app, id),
            "{id} without a selection"
        );
    }
    let result = app
        .world_mut()
        .operator("tilemap.selection.set")
        .param("rect", PropertyValue::from("3,4,1,2".to_string()))
        .call()
        .expect("dispatches");
    assert_eq!(result, OperatorResult::Finished);
    assert_eq!(area(&app), Some((cell(1, 2), cell(3, 4))));
    for id in [ERASE, COPY, CUT, "tilemap.selection.clear"] {
        assert!(crate::tile_palette::available(&mut app, id), "{id}");
    }
}

#[test]
fn erasing_a_selection_clears_its_cells_as_one_entry() {
    let (mut app, _dir) = project_app();
    let a = TileRef::new(3);
    let map = tile_map(&mut app, &[(0, 0, a), (1, 0, a), (1, 1, a), (5, 5, a)]);
    select(&mut app, (0, 0), (1, 1));
    let (before, count) = (bytes(&mut app, map), entries(&app));

    run(&mut app, ERASE);
    for (x, y) in [(0, 0), (1, 0), (1, 1)] {
        assert_eq!(tile(&mut app, map, x, y), TileRef::EMPTY, "{x},{y}");
    }
    assert_eq!(tile(&mut app, map, 5, 5), a, "outside the selection");
    assert_eq!(area(&app), Some((cell(0, 0), cell(1, 1))), "still selected");
    assert_undo_redo(&mut app, map, &before, count);
}

#[test]
fn erasing_a_terrain_selection_clears_terrain_and_hand_placed_tiles() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    let result = app
        .world_mut()
        .operator("tilemap.set_cells")
        .param("entity", PropertyValue::Entity(map))
        .param("layer", PropertyValue::from("Ground".to_string()))
        .param("kind", PropertyValue::from("tile".to_string()))
        .param("cells", PropertyValue::from("2,2".to_string()))
        .param("value", PropertyValue::from("7".to_string()))
        .call()
        .expect("tilemap.set_cells dispatches");
    assert_eq!(result, OperatorResult::Finished);
    settle(&mut app);
    enter(&mut app, map);
    select(&mut app, (1, 1), (3, 3));
    let (before, count) = (bytes(&mut app, map), entries(&app));

    run(&mut app, ERASE);
    let data = stored(&mut app, map);
    let authoring = data.layers[0].authoring.as_ref().expect("authoring");
    for y in 1..=3 {
        for x in 1..=3 {
            assert_eq!(authoring.terrain.get(cell(x, y)), 0, "{x},{y}");
        }
    }
    assert!(authoring.overrides.is_empty(), "the hand-placed tile went");
    assert_resolved(&mut app, map);
    assert_undo_redo(&mut app, map, &before, count);
}

/// A, B on the top row and C below A, selected.
fn three_tiles(app: &mut App) -> (Entity, [TileRef; 3]) {
    let (a, b, c) = (TileRef::new(3), TileRef::new(4), TileRef::new(5));
    let map = tile_map(app, &[(0, 0, a), (1, 0, b), (0, -1, c)]);
    select(app, (0, -1), (1, 0));
    (map, [a, b, c])
}

#[test]
fn copying_loads_the_selection_as_a_stamp_the_brush_paints() {
    let (mut app, _dir) = project_app();
    let (map, [a, b, c]) = three_tiles(&mut app);
    let (before, count) = (bytes(&mut app, map), entries(&app));

    run(&mut app, COPY);
    assert_eq!(entries(&app), count, "a copy leaves no entry");
    assert_eq!(bytes(&mut app, map), before);
    let stamp = app.world().resource::<StampBrush>().clone();
    assert_eq!(
        stamp.stamp,
        Some(TileStamp {
            size: UVec2::new(2, 2),
            cells: vec![
                (UVec2::new(0, 0), a),
                (UVec2::new(1, 0), b),
                (UVec2::new(0, 1), c)
            ],
        })
    );
    assert!(stamp.tileset.is_some(), "from the layer's tileset");
    assert_eq!(brush(&app).source, BrushSource::Stamp);
    assert_eq!(app.world().resource::<TileEditMode>().tool, TileTool::Brush);

    stroke(&mut app, "tilemap.brush", MouseButton::Left, &[(5, 5)]);
    assert_eq!(entries(&app), count + 1, "one stroke, one entry");
    assert_eq!(
        [
            tile(&mut app, map, 5, 5),
            tile(&mut app, map, 6, 5),
            tile(&mut app, map, 5, 4),
            tile(&mut app, map, 6, 4)
        ],
        [a, b, c, TileRef::EMPTY],
        "the copied block, an empty cell left alone"
    );
}

#[test]
fn copying_an_empty_selection_is_refused() {
    let (mut app, _dir) = project_app();
    tile_map(&mut app, &[]);
    select(&mut app, (0, 0), (2, 2));
    let result = app.world_mut().operator(COPY).call().expect("dispatches");
    assert_eq!(result, OperatorResult::Cancelled);
    assert_eq!(app.world().resource::<StampBrush>().stamp, None);
}

#[test]
fn cutting_copies_and_erases_as_one_entry() {
    let (mut app, _dir) = project_app();
    let (map, [a, ..]) = three_tiles(&mut app);
    let (before, count) = (bytes(&mut app, map), entries(&app));

    run(&mut app, CUT);
    assert_eq!(stored(&mut app, map).layers[0].tiles.cells().count(), 0);
    let stamp = app.world().resource::<StampBrush>().stamp.clone();
    assert_eq!(stamp.map(|s| s.cells[0]), Some((UVec2::ZERO, a)));
    assert_undo_redo(&mut app, map, &before, count);
}

fn key(app: &mut App, key_code: KeyCode, logical_key: Key, state: ButtonState) {
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<bevy::window::PrimaryWindow>>()
        .single(app.world())
        .expect("a primary window");
    app.world_mut().write_message(KeyboardInput {
        key_code,
        logical_key,
        state,
        text: None,
        repeat: false,
        window,
    });
    app.update();
}

/// Taps `key_code` with Ctrl held.
fn ctrl_tap(app: &mut App, key_code: KeyCode, logical_key: Key) {
    key(
        app,
        KeyCode::ControlLeft,
        Key::Control,
        ButtonState::Pressed,
    );
    tap(app, key_code, logical_key);
    key(
        app,
        KeyCode::ControlLeft,
        Key::Control,
        ButtonState::Released,
    );
    settle(app);
}

fn keyboard_app() -> (App, tempfile::TempDir) {
    let (mut app, dir) = project_app();
    app.world_mut()
        .resource_mut::<bevy::input_focus::InputFocus>()
        .clear();
    (app, dir)
}

#[test]
fn escape_clears_the_selection_then_leaves_the_mode() {
    let (mut app, _dir) = keyboard_app();
    let map = tile_map(&mut app, &[]);
    select(&mut app, (0, 0), (1, 1));

    tap(&mut app, KeyCode::Escape, Key::Escape);
    assert_eq!(area(&app), None, "the first Escape clears the selection");
    assert_eq!(app.world().resource::<TileEditMode>().map, Some(map));

    tap(&mut app, KeyCode::Escape, Key::Escape);
    assert_eq!(app.world().resource::<TileEditMode>().map, None);
    assert_eq!(
        app.world()
            .resource::<jackdaw::selection::Selection>()
            .entities,
        vec![map],
        "the map stays selected"
    );
}

#[test]
fn leaving_the_mode_drops_the_selection() {
    let (mut app, _dir) = project_app();
    tile_map(&mut app, &[]);
    select(&mut app, (0, 0), (1, 1));
    run(&mut app, "tilemap.edit_mode.exit");
    assert_eq!(area(&app), None);
}

#[test]
fn delete_and_cut_keys_act_on_the_selection_not_the_map() {
    let (mut app, _dir) = keyboard_app();
    let a = TileRef::new(3);
    let map = tile_map(&mut app, &[(0, 0, a), (4, 0, a)]);
    select(&mut app, (0, 0), (1, 1));
    let count = entries(&app);

    tap(&mut app, KeyCode::Delete, Key::Delete);
    assert!(app.world().get::<Tilemap>(map).is_some(), "the map stays");
    assert_eq!(tile(&mut app, map, 0, 0), TileRef::EMPTY);
    assert_eq!(entries(&app), count + 1);

    select(&mut app, (4, 0), (4, 0));
    ctrl_tap(&mut app, KeyCode::KeyX, Key::Character("x".into()));
    assert!(app.world().get::<Tilemap>(map).is_some(), "the map stays");
    assert_eq!(tile(&mut app, map, 4, 0), TileRef::EMPTY);
    assert_eq!(entries(&app), count + 2);
    let stamp = app.world().resource::<StampBrush>().stamp.clone();
    assert_eq!(stamp.map(|s| s.cells), Some(vec![(UVec2::ZERO, a)]));
}

#[test]
fn the_select_tool_leaves_the_stroke_state_clear() {
    let (mut app, _dir) = project_app();
    tile_map(&mut app, &[]);
    select(&mut app, (0, 0), (1, 1));
    let state = app.world().resource::<TileStrokeState>();
    assert_eq!((state.cell, state.button), (None, None));
}

fn maps(app: &mut App) -> usize {
    (app.world_mut().query_filtered::<(), With<Tilemap>>())
        .iter(app.world())
        .count()
}

#[test]
fn a_refused_cut_leaves_the_brush_alone() {
    let (mut app, _dir) = project_app();
    let (map, _) = three_tiles(&mut app);
    (app.world_mut().get_mut::<Tilemap>(map))
        .expect("the map")
        .layers
        .push(layer(TilemapLayerKind::Tile, default()));
    let (before, count) = (bytes(&mut app, map), entries(&app));
    let (source, tool) = (
        brush(&app).source,
        app.world().resource::<TileEditMode>().tool,
    );

    let result = app.world_mut().operator(CUT).call().expect("dispatches");
    assert_eq!(result, OperatorResult::Cancelled, "the erase is refused");
    assert_eq!(app.world().resource::<StampBrush>().stamp, None);
    assert_eq!(brush(&app).source, source);
    assert_eq!(app.world().resource::<TileEditMode>().tool, tool);
    assert_eq!(bytes(&mut app, map), before);
    assert_eq!(entries(&app), count);
}

#[test]
fn the_selection_operators_wait_for_their_layer() {
    let (mut app, _dir) = project_app();
    let mut data = tile_layer(&[(0, 0, TileRef::new(3))]);
    data.layers.push(data.layers[0].clone());
    let ground = || layer(TilemapLayerKind::Tile, default());
    let map = spawn_map(&mut app, vec![ground(), ground()], data);
    enter(&mut app, map);
    select(&mut app, (0, 0), (1, 0));
    app.world_mut().resource_mut::<TileEditMode>().layer = 1;
    for id in [ERASE, COPY, CUT, "tilemap.selection.clear"] {
        assert!(
            !crate::tile_palette::available(&mut app, id),
            "{id} on another layer"
        );
    }
    settle(&mut app);
    assert_eq!(area(&app), None, "the selection went with its layer");
}

#[test]
fn ctrl_v_in_tile_mode_chooses_the_stamp_brush_not_an_entity_paste() {
    let (mut app, _dir) = keyboard_app();
    three_tiles(&mut app);
    run(&mut app, "entity.copy");
    run(&mut app, COPY);
    run(&mut app, "tilemap.selection.clear");
    run(&mut app, SELECT);
    assert_eq!(
        app.world().resource::<TileEditMode>().tool,
        TileTool::Select
    );

    ctrl_tap(&mut app, KeyCode::KeyV, Key::Character("v".into()));
    assert_eq!(maps(&mut app), 1, "no entity was pasted");
    assert_eq!(app.world().resource::<TileEditMode>().tool, TileTool::Brush);
    assert_eq!(brush(&app).source, BrushSource::Stamp);
}

#[test]
fn a_selection_context_spawned_later_follows_the_selection() {
    let (mut app, _dir) = project_app();
    tile_map(&mut app, &[]);
    select(&mut app, (0, 0), (1, 1));
    let context = app
        .world_mut()
        .spawn((TileSelectionInputContext, ContextActivity::INACTIVE))
        .id();
    app.update();
    assert!(
        app.world()
            .get::<ContextActivity>(context)
            .is_some_and(|active| active.0),
        "active while cells are selected"
    );
}
