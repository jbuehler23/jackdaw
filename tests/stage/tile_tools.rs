//! Tile edit mode's Brush and Eraser: strokes driven by a held button over a
//! moving cursor cell, one history entry each, undone and redone exactly.

use crate::tilemap_render::{
    grass_patch, ground_terrain, key_of, layer, project_app, settle, spawn_map, tile_layer,
};

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use jackdaw::brush::{BrushEditMode, EditMode};
use jackdaw::selection::Selection;
use jackdaw::tile_tools::{TileBrush, TileEditMode, TileStrokeState, TileTool};
use jackdaw::tilemap::TilemapDataStore;
use jackdaw::tilemap_resolve::full_resolve;
use jackdaw_api::prelude::*;
use jackdaw_commands::CommandHistory;
use jackdaw_scene_types::{PropertyValue, TilemapLayerKind};
use jackdaw_tilemap::{CellCoord, TileRef, TilemapData, orthogonal_line};
use jackdaw_tileset::{TerrainMatch, Tileset};

const BRUSH: &str = "tilemap.brush";
const ERASER: &str = "tilemap.eraser";

fn enter(app: &mut App, map: Entity) {
    if app.world().resource::<Selection>().primary() != Some(map) {
        app.world_mut().resource_mut::<Selection>().entities = vec![map];
    }
    let result = app
        .world_mut()
        .operator("tilemap.edit_mode")
        .call()
        .expect("tilemap.edit_mode dispatches");
    assert_eq!(result, OperatorResult::Finished);
    settle(app);
}

fn available(app: &mut App, id: &str) -> bool {
    app.world_mut()
        .operator(id.to_string())
        .is_available()
        .unwrap_or(false)
}

fn set_brush(app: &mut App, param: &'static str, value: &str) -> OperatorResult {
    app.world_mut()
        .operator(BRUSH)
        .param(param, PropertyValue::from(value.to_string()))
        .call()
        .expect("tilemap.brush dispatches")
}

/// Presses `button` on the first cell, drags through the rest a frame each,
/// and lets go.
fn stroke(app: &mut App, id: &'static str, button: MouseButton, cells: &[(i32, i32)]) {
    hold(app, id, button, cells);
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(button);
    settle(app);
}

fn hold(app: &mut App, id: &'static str, button: MouseButton, cells: &[(i32, i32)]) {
    let cell = |(x, y): (i32, i32)| Some(CellCoord::new(x, y));
    let mut state = app.world_mut().resource_mut::<TileStrokeState>();
    state.cell = cell(cells[0]);
    state.button = Some(button);
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(button);
    let result = app
        .world_mut()
        .operator(id)
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: false,
        })
        .call()
        .expect("the tool dispatches");
    assert_eq!(result, OperatorResult::Running, "{id}");
    for &next in &cells[1..] {
        app.world_mut().resource_mut::<TileStrokeState>().cell = cell(next);
        app.update();
    }
}

fn run(app: &mut App, id: &'static str) {
    let result = app.world_mut().operator(id).call().expect("dispatches");
    assert_eq!(result, OperatorResult::Finished, "{id}");
    settle(app);
}

fn entries(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

fn stored(app: &mut App, map: Entity) -> TilemapData {
    let key = key_of(app, map);
    (app.world().resource::<TilemapDataStore>().get(&key))
        .cloned()
        .expect("the map has cells")
}

fn bytes(app: &mut App, map: Entity) -> Vec<u8> {
    jackdaw_tilemap::encode(&stored(app, map))
}

fn terrain(app: &mut App, map: Entity, x: i32, y: i32) -> u8 {
    let data = stored(app, map);
    let authoring = data.layers[0].authoring.as_ref().expect("authoring");
    authoring.terrain.get(CellCoord::new(x, y))
}

fn tile(app: &mut App, map: Entity, x: i32, y: i32) -> TileRef {
    stored(app, map).layers[0].tiles.get(CellCoord::new(x, y))
}

fn assert_resolved(app: &mut App, map: Entity) {
    let tileset = (app.world().resource::<Assets<Tileset>>().iter())
        .next()
        .map(|(_, t)| t.clone())
        .expect("the tileset");
    let set = tileset
        .terrain_set(Some(TerrainMatch::CornersAndSides))
        .expect("a corners and sides set");
    let data = stored(app, map);
    let fresh = full_resolve(&set, data.layers[0].authoring.as_ref().expect("authoring")).0;
    assert!(
        data.layers[0].tiles == fresh,
        "stored tiles match a fresh resolve"
    );
}

#[track_caller]
fn assert_undo_redo(app: &mut App, map: Entity, before: &[u8], count: usize) {
    assert_eq!(entries(app), count + 1, "one stroke, one entry");
    let after = bytes(app, map);
    assert_ne!(after, before);
    run(app, "history.undo");
    assert_eq!(bytes(app, map), before);
    run(app, "history.redo");
    assert_eq!(bytes(app, map), after);
}

#[test]
fn the_tools_need_a_map_in_tile_edit_mode() {
    let (mut app, _dir) = project_app();
    assert!(!available(&mut app, "tilemap.edit_mode"), "no map to edit");
    assert!(!available(&mut app, BRUSH));
    assert!(!available(&mut app, ERASER));

    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    assert!(
        !available(&mut app, "tilemap.edit_mode"),
        "the map is not selected"
    );
    app.world_mut().resource_mut::<Selection>().entities = vec![map];
    assert!(available(&mut app, "tilemap.edit_mode"));
    assert!(!available(&mut app, BRUSH), "not in tile edit mode yet");
    enter(&mut app, map);
    assert!(!available(&mut app, "tilemap.edit_mode"), "already on");
    assert!(available(&mut app, BRUSH));
    assert!(available(&mut app, ERASER));

    run(&mut app, ERASER);
    assert_eq!(
        app.world().resource::<TileEditMode>().tool,
        TileTool::Eraser
    );
    run(&mut app, BRUSH);
    assert_eq!(app.world().resource::<TileEditMode>().tool, TileTool::Brush);

    run(&mut app, "tilemap.edit_mode.exit");
    assert_eq!(app.world().resource::<TileEditMode>().map, None);
    assert!(!available(&mut app, "tilemap.edit_mode.exit"));
    assert!(!available(&mut app, BRUSH));
}

#[test]
fn brush_params_set_the_source() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    enter(&mut app, map);
    assert_eq!(
        set_brush(&mut app, "terrain", "2"),
        OperatorResult::Finished
    );
    assert_eq!(set_brush(&mut app, "tile", "7"), OperatorResult::Finished);
    let brush = *app.world().resource::<TileBrush>();
    assert_eq!((brush.terrain, brush.tile), (2, TileRef::new(7)));
    assert_eq!(
        set_brush(&mut app, "terrain", "lava"),
        OperatorResult::Cancelled
    );
    assert_eq!(*app.world().resource::<TileBrush>(), brush);
}

#[test]
fn a_brush_drag_fills_the_cells_between_samples_as_one_entry() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    enter(&mut app, map);
    assert_eq!(
        set_brush(&mut app, "terrain", "2"),
        OperatorResult::Finished
    );
    let (before, count) = (bytes(&mut app, map), entries(&app));

    stroke(&mut app, BRUSH, MouseButton::Left, &[(2, 2), (8, 5)]);
    let path = orthogonal_line(CellCoord::new(2, 2), CellCoord::new(8, 5));
    assert_eq!(path.len(), 10);
    for CellCoord(IVec2 { x, y }) in path {
        assert_eq!(terrain(&mut app, map, x, y), 2, "{x},{y} painted");
    }
    assert_resolved(&mut app, map);
    assert_undo_redo(&mut app, map, &before, count);
}

#[test]
fn a_stroke_across_a_chunk_border_undoes_in_one_step() {
    let (mut app, _dir) = project_app();
    let data = tile_layer(&[(40, 0, TileRef::new(5))]);
    let map = spawn_map(
        &mut app,
        vec![layer(TilemapLayerKind::Tile, default())],
        data,
    );
    enter(&mut app, map);
    assert_eq!(set_brush(&mut app, "tile", "7"), OperatorResult::Finished);
    let (before, count) = (bytes(&mut app, map), entries(&app));

    stroke(
        &mut app,
        BRUSH,
        MouseButton::Left,
        &[(30, 0), (31, 0), (34, 0)],
    );
    for x in 30..=34 {
        assert_eq!(tile(&mut app, map, x, 0), TileRef::new(7), "{x},0");
    }
    assert_undo_redo(&mut app, map, &before, count);
    assert_eq!(tile(&mut app, map, 40, 0), TileRef::new(5));
}

#[test]
fn the_secondary_button_erases_terrain_and_resolves_the_neighbours() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    enter(&mut app, map);
    let (before, count) = (bytes(&mut app, map), entries(&app));
    let ring = |app: &mut App| -> Vec<TileRef> {
        (1..=3)
            .flat_map(|y| (1..=3).map(move |x| (x, y)))
            .map(|(x, y)| tile(app, map, x, y))
            .collect()
    };
    let around = ring(&mut app);

    stroke(&mut app, BRUSH, MouseButton::Right, &[(2, 2)]);
    assert_eq!(terrain(&mut app, map, 2, 2), 0);
    assert_ne!(ring(&mut app), around, "the ring resolved again");
    assert_resolved(&mut app, map);
    assert_eq!(app.world().resource::<TileEditMode>().tool, TileTool::Brush);
    assert_undo_redo(&mut app, map, &before, count);
}

#[test]
fn the_eraser_clears_a_tile_layer() {
    let (mut app, _dir) = project_app();
    let data = tile_layer(&[(0, 0, TileRef::new(5)), (1, 0, TileRef::new(5))]);
    let map = spawn_map(
        &mut app,
        vec![layer(TilemapLayerKind::Tile, default())],
        data,
    );
    enter(&mut app, map);
    let (before, count) = (bytes(&mut app, map), entries(&app));

    stroke(&mut app, ERASER, MouseButton::Left, &[(0, 0), (1, 0)]);
    assert!(tile(&mut app, map, 0, 0).is_empty());
    assert!(tile(&mut app, map, 1, 0).is_empty());
    assert_undo_redo(&mut app, map, &before, count);
}

#[test]
fn a_stroke_that_changes_nothing_leaves_no_entry() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    enter(&mut app, map);
    let (before, count) = (bytes(&mut app, map), entries(&app));
    stroke(&mut app, BRUSH, MouseButton::Left, &[(1, 1), (3, 1)]);
    assert_eq!(entries(&app), count);
    assert_eq!(bytes(&mut app, map), before);
}

#[test]
fn cancelling_a_stroke_restores_its_cells() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    enter(&mut app, map);
    assert_eq!(
        set_brush(&mut app, "terrain", "2"),
        OperatorResult::Finished
    );
    let (before, count) = (bytes(&mut app, map), entries(&app));

    hold(&mut app, BRUSH, MouseButton::Left, &[(2, 2), (4, 4)]);
    assert_eq!(terrain(&mut app, map, 4, 4), 2);
    app.world_mut()
        .operator(BRUSH)
        .cancel()
        .expect("cancel dispatches");
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    settle(&mut app);
    assert_eq!(bytes(&mut app, map), before);
    assert_eq!(entries(&app), count);
}

fn tap(app: &mut App, key_code: KeyCode, logical_key: Key) {
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<bevy::window::PrimaryWindow>>()
        .single(app.world())
        .expect("a primary window");
    for state in [ButtonState::Pressed, ButtonState::Released] {
        app.world_mut().write_message(KeyboardInput {
            key_code,
            logical_key: logical_key.clone(),
            state,
            text: None,
            repeat: false,
            window,
        });
        app.update();
    }
    settle(app);
}

/// Digit1 enters tile edit mode on a selected tilemap and vertex mode on a
/// selected brush; Escape leaves tile edit mode.
#[test]
fn digit1_enters_the_mode_the_selection_takes() {
    let mut app = crate::util::editor_test_app();
    let map = app
        .world_mut()
        .spawn(jackdaw_scene_types::Tilemap {
            layers: vec![ground_terrain()],
            ..default()
        })
        .id();
    app.world_mut()
        .resource_mut::<bevy::input_focus::InputFocus>()
        .clear();
    app.world_mut().resource_mut::<Selection>().entities = vec![map];
    tap(&mut app, KeyCode::Digit1, Key::Character("1".into()));
    assert_eq!(app.world().resource::<TileEditMode>().map, Some(map));
    assert_eq!(
        *app.world().resource::<EditMode>(),
        EditMode::Object,
        "the tilemap is no brush"
    );
    assert!(
        !available(&mut app, "edit_mode.vertex"),
        "Digit1 is the tilemap's"
    );
    tap(&mut app, KeyCode::Escape, Key::Escape);
    assert_eq!(app.world().resource::<TileEditMode>().map, None);
    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![map],
        "Escape left the mode and kept the selection"
    );

    let brush = app
        .world_mut()
        .spawn(jackdaw_scene_types::Brush::cuboid(1.0, 1.0, 1.0))
        .id();
    app.world_mut().resource_mut::<Selection>().entities = vec![brush];
    assert!(
        !available(&mut app, "tilemap.edit_mode"),
        "Digit1 is the brush's"
    );
    tap(&mut app, KeyCode::Digit1, Key::Character("1".into()));
    assert_eq!(
        *app.world().resource::<EditMode>(),
        EditMode::BrushEdit(BrushEditMode::Vertex)
    );
    assert_eq!(app.world().resource::<TileEditMode>().map, None);
}

#[test]
fn a_brush_the_layers_tileset_lacks_refuses_the_stroke() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    enter(&mut app, map);
    app.world_mut().resource_mut::<TileBrush>().terrain = 99;
    let (before, count) = (bytes(&mut app, map), entries(&app));
    {
        let mut state = app.world_mut().resource_mut::<TileStrokeState>();
        state.cell = Some(CellCoord::new(2, 2));
        state.button = Some(MouseButton::Left);
    }
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    let result = app
        .world_mut()
        .operator(BRUSH)
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: false,
        })
        .call()
        .expect("the tool dispatches");
    assert_eq!(result, OperatorResult::Cancelled);
    settle(&mut app);
    assert_eq!(bytes(&mut app, map), before);
    assert_eq!(entries(&app), count);
    let notice = app.world().resource::<jackdaw::status_bar::StatusNotice>();
    assert!(notice.text().contains("terrain 99"), "{}", notice.text());
}

/// A press and release inside one frame still paints the pressed cell.
#[test]
fn a_tap_inside_one_frame_paints_its_cell() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    enter(&mut app, map);
    assert_eq!(
        set_brush(&mut app, "terrain", "2"),
        OperatorResult::Finished
    );
    let (before, count) = (bytes(&mut app, map), entries(&app));
    {
        let mut state = app.world_mut().resource_mut::<TileStrokeState>();
        state.cell = Some(CellCoord::new(3, 3));
        state.button = Some(MouseButton::Left);
    }
    let result = app
        .world_mut()
        .operator(BRUSH)
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: false,
        })
        .call()
        .expect("the tool dispatches");
    assert_eq!(result, OperatorResult::Finished);
    settle(&mut app);
    assert_eq!(terrain(&mut app, map, 3, 3), 2);
    assert_undo_redo(&mut app, map, &before, count);
}

#[test]
fn erasing_terrain_clears_hand_placed_tiles_in_the_same_entry() {
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
    let (before, count) = (bytes(&mut app, map), entries(&app));

    stroke(&mut app, ERASER, MouseButton::Left, &[(2, 2)]);
    let data = stored(&mut app, map);
    let authoring = data.layers[0].authoring.as_ref().expect("authoring");
    assert_eq!(authoring.terrain.get(CellCoord::new(2, 2)), 0);
    assert!(
        authoring.overrides.is_empty(),
        "the hand-placed tile went too"
    );
    assert_resolved(&mut app, map);
    assert_undo_redo(&mut app, map, &before, count);
    run(&mut app, "history.undo");
    let data = stored(&mut app, map);
    let authoring = data.layers[0].authoring.as_ref().expect("authoring");
    assert!(authoring.overrides.contains_key(&CellCoord::new(2, 2)));
}

#[test]
fn selecting_something_else_leaves_the_mode() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    enter(&mut app, map);
    let other = app.world_mut().spawn(Transform::default()).id();
    app.world_mut().resource_mut::<Selection>().entities = vec![map, other];
    settle(&mut app);
    assert_eq!(app.world().resource::<TileEditMode>().map, None);
    assert!(!available(&mut app, BRUSH));
}

/// The 2D panel driven through the synthetic pointer: a press on the map
/// paints through the stage observers.
mod on_the_stage {
    use super::*;
    use crate::sprite_gizmo::{add_panel, run, sprite, transform};
    use bevy::sprite::Anchor;
    use bevy::window::{PrimaryWindow, WindowResolution};
    use jackdaw::active_tool::ActiveTool;

    fn stage_app() -> (App, tempfile::TempDir, Entity, Entity) {
        let (mut app, dir) = project_app();
        crate::util::fixed_frame_clock(&mut app);
        {
            let mut windows = app
                .world_mut()
                .query_filtered::<&mut Window, With<PrimaryWindow>>();
            let mut window = windows
                .single_mut(app.world_mut())
                .expect("a primary window");
            window.resolution = WindowResolution::new(1600, 1000);
        }
        add_panel(&mut app, 1200.0);
        let root = jackdaw::entity_ops::seed_2d_scene_root(app.world_mut());
        let data = tile_layer(&[]);
        let map = spawn_map(
            &mut app,
            vec![layer(TilemapLayerKind::Tile, default())],
            data,
        );
        settle(&mut app);
        (app, dir, map, root)
    }

    fn pointer(app: &mut App, at: Vec2, action: &str, button: &str) {
        run(
            app,
            &format!(
                "input.pointer space=world x={} y={} action={action} button={button}",
                at.x, at.y
            ),
        );
    }

    fn stroking(app: &mut App) -> bool {
        app.world_mut().operator(BRUSH).is_running()
    }

    #[test]
    fn a_second_button_mid_stroke_is_ignored() {
        let (mut app, _dir, map, _) = stage_app();
        app.world_mut().resource_mut::<Selection>().entities = vec![map];
        enter(&mut app, map);
        let count = entries(&app);

        pointer(&mut app, Vec2::new(8.0, 8.0), "press", "left");
        assert!(stroking(&mut app));
        pointer(&mut app, Vec2::new(40.0, 8.0), "press", "right");
        pointer(&mut app, Vec2::new(40.0, 8.0), "release", "left");
        assert!(
            !stroking(&mut app),
            "letting go of the first button ends it"
        );
        assert_eq!(entries(&app), count + 1);
        for x in 0..=2 {
            assert_eq!(tile(&mut app, map, x, 0), TileRef::new(0), "{x},0 painted");
        }
        pointer(&mut app, Vec2::new(40.0, 8.0), "release", "right");
        assert_eq!(
            entries(&app),
            count + 1,
            "the second button painted nothing"
        );
    }

    #[test]
    fn a_stroke_over_a_selected_sprite_leaves_it_in_place() {
        let (mut app, _dir, map, root) = stage_app();
        let square = Sprite::from_color(Color::WHITE, Vec2::splat(20.0));
        let at = Vec3::new(-60.0, 40.0, 0.0);
        let body = sprite(&mut app, root, "Body", square, Anchor::CENTER, at);
        app.world_mut().insert_resource(ActiveTool::Translate);
        pointer(&mut app, at.truncate(), "click", "left");
        assert_eq!(app.world().resource::<Selection>().entities, vec![body]);
        app.world_mut().resource_mut::<Selection>().entities = vec![body, map];
        enter(&mut app, map);

        pointer(&mut app, at.truncate(), "press", "left");
        pointer(
            &mut app,
            at.truncate() + Vec2::new(30.0, 0.0),
            "move",
            "left",
        );
        pointer(
            &mut app,
            at.truncate() + Vec2::new(30.0, 0.0),
            "release",
            "left",
        );
        assert_eq!(
            transform(&app, body).translation,
            at,
            "the sprite stayed put"
        );
        assert_eq!(
            tile(&mut app, map, -4, 2),
            TileRef::new(0),
            "the map took the stroke"
        );
    }
}
