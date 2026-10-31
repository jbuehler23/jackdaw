//! Terrain bits on the sheet: the overlay, painting them by dragging across
//! tiles, the coverage of each set and its missing shapes.

use crate::tileset_dock::{
    control, handmade, input, opened, press, screen_rect, settle, texts, view,
};
use crate::tileset_edit_ops::{call, entries, on_disk, peers_of, refused, run};
use crate::tileset_ops::warnings;
use crate::util::OperatorResultExt as _;

use std::collections::BTreeSet;

use bevy::prelude::*;
use jackdaw::tileset_bits::BitMark;
use jackdaw::tileset_dock::{BitsOverlay, DockAction, DockTool, NewSetPick};
use jackdaw_api::prelude::*;
use jackdaw_feathers::combobox::ComboBoxChangeEvent;

const GRASS: u8 = 1;
const WATER: u8 = 2;
const SAND: u8 = 3;

/// A point of tile `tile` on the drawn sheet, from (0, 0) at its top left
/// to (1, 1), in the logical pixels `input.pointer` takes.
fn on_tile(app: &mut App, root: Entity, tile: u32, at: Vec2) -> Vec2 {
    let cell = control(app, root, DockAction::Tile(0, tile));
    let rect = screen_rect(app, cell);
    rect.min + at * rect.size()
}

fn drag(app: &mut App, from: Vec2, to: Vec2, button: &str) {
    input(
        app,
        &format!("input.pointer x={} y={} action=move", from.x, from.y),
    );
    input(
        app,
        &format!(
            "input.pointer x={} y={} action=drag_to steps=24 button={button}",
            to.x, to.y
        ),
    );
}

/// The tiles and peers the overlay of the dock under `root` marks.
fn marks(app: &mut App, root: Entity) -> BTreeSet<(u32, usize)> {
    app.world_mut()
        .query::<&BitMark>()
        .iter(app.world())
        .filter(|m| m.root == root)
        .map(|m| (m.tile, m.peer))
        .collect()
}

/// The open dock under `root`, in paint mode.
fn painting(app: &mut App) -> Entity {
    let (_, root) = opened(app);
    press(app, root, DockAction::Tool(DockTool::Paint));
    root
}

fn pointer(app: &mut App, at: Vec2, action: &str, button: &str) {
    let clause = format!(
        "input.pointer x={} y={} action={action} button={button}",
        at.x, at.y
    );
    input(app, &clause);
}

/// UI nodes with no parent: the window roots, plus anything orphaned.
fn root_nodes(app: &mut App) -> usize {
    app.world_mut()
        .query_filtered::<(), (With<Node>, Without<ChildOf>)>()
        .iter(app.world())
        .count()
}

fn coverage(app: &mut App, root: Entity) -> String {
    (texts(app, root).into_iter())
        .find(|t| t.starts_with("water over grass:"))
        .unwrap_or_default()
}

#[test]
fn a_drag_across_three_tiles_paints_their_corners_as_one_undo_entry() {
    let (mut app, dir) = handmade();
    let root = painting(&mut app);
    press(&mut app, root, DockAction::Brush(WATER));
    assert_eq!(view(&app).active, Some((WATER, GRASS)), "the brush's set");
    let before = on_disk(&dir);
    let count = entries(&app);
    let from = on_tile(&mut app, root, 1, Vec2::new(0.15, 0.15));
    let to = on_tile(&mut app, root, 3, Vec2::new(0.85, 0.15));
    drag(&mut app, from, to, "primary");

    let after = on_disk(&dir);
    let top = [WATER, 0, WATER, 0, 0, 0, 0, 0, 0];
    for tile in 1..=3 {
        assert_eq!(peers_of(&after, tile), Some(top), "tile {tile}");
    }
    assert_eq!(peers_of(&after, 4), None, "past the stroke");
    assert_eq!(entries(&app), count + 1, "one undo entry for the stroke");
    assert!(warnings(&app).is_empty(), "{:?}", warnings(&app));

    run(&mut app, "history.undo");
    settle(&mut app);
    assert_eq!(on_disk(&dir), before, "undo restores the sheet exactly");
}

#[test]
fn a_right_drag_erases_the_zones_it_crosses() {
    let (mut app, dir) = handmade();
    let root = painting(&mut app);
    press(&mut app, root, DockAction::Brush(WATER));
    let count = entries(&app);
    let from = on_tile(&mut app, root, 0, Vec2::new(0.15, 0.85));
    let to = on_tile(&mut app, root, 0, Vec2::new(0.85, 0.85));
    drag(&mut app, from, to, "secondary");
    let erased = [GRASS, GRASS, GRASS, GRASS, GRASS, GRASS, 0, GRASS, 0];
    assert_eq!(peers_of(&on_disk(&dir), 0), Some(erased));
    assert_eq!(entries(&app), count + 1);
}

#[test]
fn a_stroke_that_changes_nothing_leaves_no_history() {
    let (mut app, dir) = handmade();
    let root = painting(&mut app);
    press(&mut app, root, DockAction::Brush(GRASS));
    let (before, count) = (on_disk(&dir), entries(&app));
    let from = on_tile(&mut app, root, 0, Vec2::new(0.15, 0.15));
    let to = on_tile(&mut app, root, 0, Vec2::new(0.85, 0.15));
    drag(&mut app, from, to, "primary");
    assert_eq!(on_disk(&dir), before);
    assert_eq!(entries(&app), count);
    assert!(marks(&mut app, root).contains(&(0, 8)), "the dock is whole");
}

#[test]
fn zones_outside_the_sets_mode_are_left() {
    let (mut app, dir) = handmade();
    let paint = |zones: &'static str| {
        [
            ("inner", "water"),
            ("outer", "grass"),
            ("terrain", "water"),
            ("zones", zones),
        ]
    };
    let count = entries(&app);
    call(
        &mut app,
        "tileset.tile.paint_bits",
        &paint("0:4:top,0:4:top_left"),
    );
    assert_eq!(
        peers_of(&on_disk(&dir), 4),
        Some([WATER, 0, 0, 0, 0, 0, 0, 0, 0]),
        "a corners set paints no side"
    );
    assert_eq!(entries(&app), count + 1);
    call(
        &mut app,
        "tileset.tile.paint_bits",
        &paint("0:5:top,0:5:left"),
    );
    assert_eq!(peers_of(&on_disk(&dir), 5), None);
    assert_eq!(entries(&app), count + 1, "nothing changed, no entry");

    call(&mut app, "tileset.terrain.add", &[("name", "sand")]);
    let sand = [
        ("inner", "water"),
        ("outer", "grass"),
        ("terrain", "sand"),
        ("zones", "0:6:centre"),
    ];
    refused(&mut app, &dir, "tileset.tile.paint_bits", &sand, "not in");
    let unset = [
        ("inner", "sand"),
        ("outer", "grass"),
        ("terrain", "sand"),
        ("zones", "0:6:centre"),
    ];
    refused(
        &mut app,
        &dir,
        "tileset.tile.paint_bits",
        &unset,
        "terrain set",
    );
}

#[test]
fn a_brush_no_set_holds_paints_nothing_and_says_why() {
    let (mut app, dir) = handmade();
    call(&mut app, "tileset.terrain.add", &[("name", "sand")]);
    settle(&mut app);
    let root = painting(&mut app);
    press(&mut app, root, DockAction::Brush(SAND));
    let (before, count) = (on_disk(&dir), entries(&app));
    let from = on_tile(&mut app, root, 2, Vec2::new(0.15, 0.15));
    let to = on_tile(&mut app, root, 2, Vec2::new(0.85, 0.15));
    drag(&mut app, from, to, "primary");
    assert_eq!(on_disk(&dir), before);
    assert_eq!(entries(&app), count);
    let notice = app.world().resource::<jackdaw::status_bar::StatusNotice>();
    assert!(notice.text().contains("sand"), "{}", notice.text());
}

#[test]
fn the_coverage_line_follows_the_stroke_as_it_paints() {
    let (mut app, _dir) = handmade();
    let root = painting(&mut app);
    assert_eq!(coverage(&mut app, root), "water over grass: Corners 1/16");
    call(
        &mut app,
        "tileset.tile.set_bits",
        &[("tile", "0:1"), ("peers", "2,0,2,0,2,0,0,0,0")],
    );
    settle(&mut app);
    press(&mut app, root, DockAction::Brush(GRASS));
    let from = on_tile(&mut app, root, 1, Vec2::new(0.15, 0.85));
    let to = on_tile(&mut app, root, 1, Vec2::new(0.85, 0.85));
    input(
        &mut app,
        &format!("input.pointer x={} y={} action=press", from.x, from.y),
    );
    input(
        &mut app,
        &format!("input.pointer x={} y={} action=move", to.x, to.y),
    );
    assert_eq!(
        coverage(&mut app, root),
        "water over grass: Corners 2/16",
        "live, before the stroke ends"
    );
    input(
        &mut app,
        &format!("input.pointer x={} y={} action=release", to.x, to.y),
    );
    assert_eq!(coverage(&mut app, root), "water over grass: Corners 2/16");
}

#[test]
fn the_overlay_draws_off_the_active_set_or_all() {
    let (mut app, _dir) = handmade();
    call(&mut app, "tileset.terrain.add", &[("name", "sand")]);
    let set = [("inner", "sand"), ("outer", "grass"), ("mode", "corners")];
    call(&mut app, "tileset.terrain_set.add", &set);
    let bits = [("tile", "0:10"), ("peers", "1,1,1,1,3,1,1,1,3")];
    call(&mut app, "tileset.tile.set_bits", &bits);
    let grass = [("tile", "0:20"), ("peers", "1,1,1,1,1,1,1,1,1")];
    call(&mut app, "tileset.tile.set_bits", &grass);
    settle(&mut app);
    let (_, root) = opened(&mut app);
    let tiles =
        |app: &mut App| -> BTreeSet<u32> { marks(app, root).into_iter().map(|(t, _)| t).collect() };
    assert_eq!(view(&app).bits, BitsOverlay::Set);
    press(&mut app, root, DockAction::ActiveSet(WATER, GRASS));
    assert_eq!(
        tiles(&mut app),
        BTreeSet::from([0, 20]),
        "a pure grass tile of two sets shows in each"
    );
    let corners_and_centre = BTreeSet::from([(0, 0), (0, 2), (0, 4), (0, 6), (0, 8)]);
    assert_eq!(
        (marks(&mut app, root).into_iter())
            .filter(|(t, _)| *t == 0)
            .collect::<BTreeSet<_>>(),
        corners_and_centre,
        "the mode's zones"
    );
    press(&mut app, root, DockAction::ActiveSet(SAND, GRASS));
    assert_eq!(tiles(&mut app), BTreeSet::from([10, 20]));
    press(&mut app, root, DockAction::Bits(BitsOverlay::All));
    assert_eq!(tiles(&mut app), BTreeSet::from([0, 10, 20]));
    press(&mut app, root, DockAction::Bits(BitsOverlay::Off));
    assert!(tiles(&mut app).is_empty());

    press(&mut app, root, DockAction::Tile(0, 3));
    input(&mut app, "input.key key=KeyO");
    assert_eq!(view(&app).bits, BitsOverlay::Set, "O steps off to set");
    input(&mut app, "input.key key=KeyO");
    assert_eq!(view(&app).bits, BitsOverlay::All, "then all");
    assert_eq!(tiles(&mut app), BTreeSet::from([0, 10, 20]));
}

#[test]
fn a_click_in_select_mode_selects_and_paints_nothing() {
    let (mut app, dir) = handmade();
    let (_, root) = opened(&mut app);
    press(&mut app, root, DockAction::Brush(WATER));
    let (before, count) = (on_disk(&dir), entries(&app));
    let at = on_tile(&mut app, root, 1, Vec2::new(0.15, 0.15));
    pointer(&mut app, at, "click", "primary");
    assert_eq!(view(&app).tool, DockTool::Select);
    assert_eq!(view(&app).selected, Some((0, 1)));
    assert_eq!(on_disk(&dir), before);
    assert_eq!(entries(&app), count);
}

#[test]
fn a_click_in_paint_mode_paints_one_zone_and_p_switches_modes() {
    let (mut app, dir) = handmade();
    let (_, root) = opened(&mut app);
    press(&mut app, root, DockAction::Bits(BitsOverlay::Off));
    press(&mut app, root, DockAction::Brush(WATER));
    input(&mut app, "input.key key=KeyP");
    assert_eq!(view(&app).tool, DockTool::Paint);
    assert_eq!(view(&app).bits, BitsOverlay::Set, "painting shows the bits");
    let count = entries(&app);
    let at = on_tile(&mut app, root, 1, Vec2::new(0.15, 0.15));
    pointer(&mut app, at, "click", "primary");
    let one = [WATER, 0, 0, 0, 0, 0, 0, 0, 0];
    assert_eq!(peers_of(&on_disk(&dir), 1), Some(one));
    assert_eq!(entries(&app), count + 1);
    input(&mut app, "input.key key=KeyP");
    assert_eq!(view(&app).tool, DockTool::Select);
}

#[test]
fn a_move_without_the_button_held_paints_nothing() {
    let (mut app, dir) = handmade();
    let root = painting(&mut app);
    press(&mut app, root, DockAction::Brush(WATER));
    let from = on_tile(&mut app, root, 1, Vec2::new(0.15, 0.15));
    let to = on_tile(&mut app, root, 3, Vec2::new(0.85, 0.15));
    pointer(&mut app, from, "press", "primary");
    let pointers: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<bevy::picking::pointer::PointerPressState>>()
        .iter(app.world())
        .collect();
    for entity in pointers {
        app.world_mut()
            .entity_mut(entity)
            .insert(bevy::picking::pointer::PointerPressState::default());
    }
    pointer(&mut app, to, "move", "primary");
    pointer(&mut app, to, "release", "primary");
    let after = on_disk(&dir);
    assert_eq!(peers_of(&after, 1), Some([WATER, 0, 0, 0, 0, 0, 0, 0, 0]));
    assert_eq!(peers_of(&after, 2), None, "no button, no paint");
    assert_eq!(peers_of(&after, 3), None);
}

#[test]
fn losing_window_focus_cancels_the_stroke() {
    let (mut app, dir) = handmade();
    let root = painting(&mut app);
    press(&mut app, root, DockAction::Brush(WATER));
    let (before, count) = (on_disk(&dir), entries(&app));
    let from = on_tile(&mut app, root, 1, Vec2::new(0.15, 0.15));
    let to = on_tile(&mut app, root, 3, Vec2::new(0.85, 0.15));
    pointer(&mut app, from, "press", "primary");
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<bevy::window::PrimaryWindow>>()
        .single(app.world())
        .expect("a window");
    app.world_mut().write_message(bevy::window::WindowFocused {
        window,
        focused: false,
    });
    settle(&mut app);
    pointer(&mut app, to, "move", "primary");
    pointer(&mut app, to, "release", "primary");
    assert_eq!(on_disk(&dir), before);
    assert_eq!(entries(&app), count);
    assert!(
        !marks(&mut app, root).iter().any(|(t, _)| *t == 1),
        "the preview is gone"
    );
}

#[test]
fn a_refill_mid_stroke_orphans_nothing_and_the_stroke_goes_on() {
    let (mut app, dir) = handmade();
    let root = painting(&mut app);
    press(&mut app, root, DockAction::Brush(WATER));
    let count = entries(&app);
    let roots = root_nodes(&mut app);
    let from = on_tile(&mut app, root, 1, Vec2::new(0.15, 0.15));
    let to = on_tile(&mut app, root, 3, Vec2::new(0.85, 0.15));
    pointer(&mut app, from, "press", "primary");
    call(&mut app, "tileset.bits_overlay", &[("mode", "all")]);
    settle(&mut app);
    pointer(&mut app, to, "move", "primary");
    pointer(&mut app, to, "release", "primary");
    assert_eq!(root_nodes(&mut app), roots, "no orphan nodes");
    let top = Some([WATER, 0, WATER, 0, 0, 0, 0, 0, 0]);
    assert_eq!(peers_of(&on_disk(&dir), 3), top);
    assert_eq!(entries(&app), count + 1);
}

#[test]
fn a_second_button_mid_stroke_is_ignored() {
    let (mut app, dir) = handmade();
    let root = painting(&mut app);
    press(&mut app, root, DockAction::Brush(WATER));
    let count = entries(&app);
    let from = on_tile(&mut app, root, 1, Vec2::new(0.15, 0.15));
    let mid = on_tile(&mut app, root, 2, Vec2::new(0.15, 0.15));
    let to = on_tile(&mut app, root, 3, Vec2::new(0.85, 0.15));
    pointer(&mut app, from, "press", "primary");
    pointer(&mut app, mid, "press", "secondary");
    pointer(&mut app, mid, "release", "secondary");
    pointer(&mut app, to, "move", "primary");
    pointer(&mut app, to, "release", "primary");
    let after = on_disk(&dir);
    let top = [WATER, 0, WATER, 0, 0, 0, 0, 0, 0];
    for tile in 1..=3 {
        assert_eq!(peers_of(&after, tile), Some(top), "tile {tile}");
    }
    assert_eq!(entries(&app), count + 1);
}

#[test]
fn an_erase_with_no_sets_says_nothing_and_changes_nothing() {
    let (mut app, dir) = crate::tileset_ops::project_app();
    crate::tileset_ops::import(&mut app, &[("paths", crate::tileset_ops::SHEET)]);
    call(&mut app, "tileset.terrain.add", &[("name", "grass")]);
    settle(&mut app);
    let root = painting(&mut app);
    let (before, count) = (on_disk(&dir), entries(&app));
    let from = on_tile(&mut app, root, 1, Vec2::new(0.15, 0.15));
    let to = on_tile(&mut app, root, 2, Vec2::new(0.85, 0.15));
    drag(&mut app, from, to, "secondary");
    assert_eq!(on_disk(&dir), before);
    assert_eq!(entries(&app), count);
    let notice = app.world().resource::<jackdaw::status_bar::StatusNotice>();
    assert!(
        !notice.text().contains("no terrain set"),
        "{}",
        notice.text()
    );
}

fn pick(app: &mut App, root: Entity, field: usize, selected: usize) {
    let combo: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &NewSetPick)>()
        .iter(app.world())
        .filter(|(_, p)| p.root == root && p.field == field)
        .map(|(e, _)| e)
        .collect();
    assert_eq!(combo.len(), 1, "one picker for field {field}");
    app.world_mut().trigger(ComboBoxChangeEvent {
        entity: combo[0],
        selected,
        label: String::new(),
        value: None,
    });
    settle(app);
}

#[test]
fn the_new_set_control_adds_a_set_that_becomes_active() {
    let (mut app, dir) = handmade();
    call(&mut app, "tileset.terrain.add", &[("name", "sand")]);
    settle(&mut app);
    let (_, root) = opened(&mut app);
    pick(&mut app, root, 0, 2);
    pick(&mut app, root, 1, 1);
    pick(&mut app, root, 2, 0);
    let count = entries(&app);
    press(&mut app, root, DockAction::CreateSet);
    let sets: Vec<_> = (on_disk(&dir).terrain_sets.into_iter())
        .map(|s| (s.inner, s.outer, format!("{:?}", s.mode)))
        .collect();
    assert!(
        sets.contains(&(SAND, GRASS, "Corners".to_string())),
        "{sets:?}"
    );
    assert_eq!(entries(&app), count + 1);
    assert_eq!(view(&app).active, Some((SAND, GRASS)));
}

#[test]
fn tileset_operators_without_a_path_edit_the_focused_tab() {
    let (mut app, dir) = handmade();
    call(&mut app, "tileset.terrain.add", &[("name", "sand")]);
    settle(&mut app);
    let (_, root) = opened(&mut app);
    press(&mut app, root, DockAction::Tile(0, 3));
    app.world_mut()
        .resource_mut::<jackdaw::definition_assets::OpenDefinition>()
        .0 = None;
    let mut call = app.world_mut().operator("tileset.terrain_set.add");
    for (key, value) in [("inner", "sand"), ("outer", "grass"), ("mode", "sides")] {
        call = call.param(
            key,
            jackdaw_scene_types::PropertyValue::from(value.to_string()),
        );
    }
    call.call().expect("dispatches").assert_finished();
    settle(&mut app);
    assert!(warnings(&app).is_empty(), "{:?}", warnings(&app));
    let pairs = crate::tileset_edit_ops::pairs(&on_disk(&dir));
    assert!(pairs.contains(&(SAND, GRASS)), "{pairs:?}");
}
