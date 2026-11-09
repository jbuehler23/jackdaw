//! Terrain bits on the sheet: the zone overlay of the terrains mode,
//! painting them by dragging across tiles, the coverage of each set and its
//! missing shapes.

use crate::operators::tileset_dock::{
    control, handmade, input, more_menu, opened, press, screen_rect, settle, texts, view,
};
use crate::operators::tileset_edit_ops::{call, entries, on_disk, peers_of, refused, run};
use crate::operators::tileset_ops::warnings;
use crate::util::OperatorResultExt as _;

use std::collections::BTreeSet;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_editor_tile::tileset_dock::{DockAction, NewSetPick};
use jackdaw_editor_tile::tileset_layout::GhostZones;
use jackdaw_editor_tile::tileset_modes::ModeId;
use jackdaw_editor_tile::zone_overlay::{ZoneHover, ZoneOverlay, ZonePiece};
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

/// The tiles and peers holding a terrain in the zone overlay of the dock
/// under `root`.
fn marks(app: &mut App, root: Entity) -> BTreeSet<(u32, usize)> {
    app.world_mut()
        .query_filtered::<&ZoneOverlay, Without<GhostZones>>()
        .iter(app.world())
        .filter(|o| o.root == root)
        .flat_map(|o| o.values.zones().map(|(tile, peer, _)| (tile, peer)))
        .collect()
}

/// The drawn pieces of the zone overlay of the dock under `root`.
fn pieces(app: &mut App, root: Entity) -> Vec<ZonePiece> {
    let overlays: Vec<Entity> = app
        .world_mut()
        .query_filtered::<(Entity, &ZoneOverlay), Without<GhostZones>>()
        .iter(app.world())
        .filter(|(_, o)| o.root == root)
        .map(|(e, _)| e)
        .collect();
    (overlays.into_iter())
        .flat_map(|o| {
            app.world()
                .get::<Children>(o)
                .into_iter()
                .flatten()
                .copied()
        })
        .filter_map(|e| app.world().get::<ZonePiece>(e).copied())
        .collect()
}

/// The drawn pieces of the zone overlay of the dock under `root`, with the
/// fill each node has.
fn piece_nodes(app: &mut App, root: Entity) -> Vec<(ZonePiece, Option<Color>)> {
    let overlays: Vec<Entity> = app
        .world_mut()
        .query_filtered::<(Entity, &ZoneOverlay), Without<GhostZones>>()
        .iter(app.world())
        .filter(|(_, o)| o.root == root)
        .map(|(e, _)| e)
        .collect();
    let world = app.world();
    (overlays.into_iter())
        .flat_map(|o| world.get::<Children>(o).into_iter().flatten().copied())
        .filter_map(|e| {
            let piece = world.get::<ZonePiece>(e).copied()?;
            Some((piece, world.get::<BackgroundColor>(e).map(|c| c.0)))
        })
        .collect()
}

/// The open dock under `root`, in the terrains mode.
fn painting(app: &mut App) -> Entity {
    let (_, root) = opened(app);
    press(app, root, DockAction::Mode(ModeId::TERRAINS));
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
        .find(|t| t.starts_with("Terrain set 1 (Corners):"))
        .unwrap_or_default()
}

#[test]
fn a_drag_across_three_tiles_paints_their_corners_as_one_undo_entry() {
    let (mut app, dir) = handmade();
    let root = painting(&mut app);
    press(&mut app, root, DockAction::Brush(WATER));
    assert_eq!(
        view(&app).active.as_deref(),
        Some("Terrain set 1"),
        "the brush's set"
    );
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
    let paint = |zones: &'static str| [("set", "0"), ("terrain", "water"), ("zones", zones)];
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
    let sand = [("set", "0"), ("terrain", "sand"), ("zones", "0:6:centre")];
    refused(&mut app, &dir, "tileset.tile.paint_bits", &sand, "not in");
    let unset = [
        ("set", "Sand"),
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
    let notice = app
        .world()
        .resource::<jackdaw_editor_core::status_bar::StatusNotice>();
    assert!(notice.text().contains("sand"), "{}", notice.text());
}

#[test]
fn the_coverage_line_follows_the_stroke_as_it_paints() {
    let (mut app, _dir) = handmade();
    let root = painting(&mut app);
    assert_eq!(
        coverage(&mut app, root),
        "Terrain set 1 (Corners): 1 of 16 shapes"
    );
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
        "Terrain set 1 (Corners): 2 of 16 shapes",
        "live, before the stroke ends"
    );
    input(
        &mut app,
        &format!("input.pointer x={} y={} action=release", to.x, to.y),
    );
    assert_eq!(
        coverage(&mut app, root),
        "Terrain set 1 (Corners): 2 of 16 shapes"
    );
}

#[test]
fn the_terrains_mode_overlays_the_active_set_and_the_tiles_mode_nothing() {
    let (mut app, _dir) = handmade();
    call(&mut app, "tileset.terrain.add", &[("name", "sand")]);
    call(&mut app, "tileset.terrain_set.add", &[("mode", "corners")]);
    let join = [("set", "1"), ("terrain", "sand")];
    call(&mut app, "tileset.terrain_set.add_terrain", &join);
    let bits = [("tile", "0:10"), ("peers", "0,0,0,0,3,0,0,0,3")];
    call(&mut app, "tileset.tile.set_bits", &bits);
    let grass = [("tile", "0:20"), ("peers", "1,1,1,1,1,1,1,1,1")];
    call(&mut app, "tileset.tile.set_bits", &grass);
    settle(&mut app);
    let (_, root) = opened(&mut app);
    let tiles =
        |app: &mut App| -> BTreeSet<u32> { marks(app, root).into_iter().map(|(t, _)| t).collect() };
    assert_eq!(view(&app).mode, ModeId::TILES);
    press(&mut app, root, DockAction::ActiveSet(0));
    assert!(
        tiles(&mut app).is_empty(),
        "the tiles mode shows a clean sheet"
    );
    press(&mut app, root, DockAction::Mode(ModeId::TERRAINS));
    assert_eq!(tiles(&mut app), BTreeSet::from([0, 20]));
    let corners = BTreeSet::from([(0, 0), (0, 2), (0, 6), (0, 8)]);
    assert_eq!(
        (marks(&mut app, root).into_iter())
            .filter(|(t, _)| *t == 0)
            .collect::<BTreeSet<_>>(),
        corners,
        "a corners set holds no center zone"
    );
    press(&mut app, root, DockAction::ActiveSet(1));
    assert_eq!(tiles(&mut app), BTreeSet::from([10]));
    press(&mut app, root, DockAction::Mode(ModeId::OBJECTS));
    assert!(tiles(&mut app).is_empty());
}

#[test]
fn the_overlay_draws_each_tiles_shapes_and_dims_tiles_outside_the_set() {
    let (mut app, _dir) = handmade();
    let root = painting(&mut app);
    press(&mut app, root, DockAction::Brush(WATER));
    let found = pieces(&mut app, root);
    let has = |piece: ZonePiece| found.contains(&piece);
    let (water, grass) = (u32::from(WATER), u32::from(GRASS));
    let corner = ZonePiece::Shape {
        value: water,
        picked: true,
    };
    assert!(
        has(corner),
        "the water corner is one picked shape: {found:?}"
    );
    let quiet = ZonePiece::Edge {
        value: grass,
        picked: false,
    };
    assert!(has(quiet), "the grass L is outlined: {found:?}");
    let grass_tile = [("tile", "0:7"), ("peers", "1,1,1,1,1,1,1,1,1")];
    call(&mut app, "tileset.tile.set_bits", &grass_tile);
    settle(&mut app);
    let filled: Vec<ZonePiece> = (piece_nodes(&mut app, root).into_iter())
        .filter(|(p, fill)| {
            matches!(p, ZonePiece::Shape { .. }) && fill.is_some_and(|c| c.alpha() > 0.0)
        })
        .map(|(piece, _)| piece)
        .collect();
    let whole_grass = ZonePiece::Shape {
        value: grass,
        picked: false,
    };
    assert!(
        piece_nodes(&mut app, root)
            .iter()
            .any(|(p, _)| *p == whole_grass),
        "a whole grass tile is one outlined shape"
    );
    assert_eq!(filled, [corner], "only the picked terrain is filled");
    assert!(has(ZonePiece::Dim), "tiles outside the set are dimmed");
    assert!(
        found.len() < 400,
        "a few pieces per painted tile: {}",
        found.len()
    );
}

#[test]
fn the_zone_under_the_pointer_is_outlined() {
    let (mut app, _dir) = handmade();
    let root = painting(&mut app);
    press(&mut app, root, DockAction::Brush(WATER));
    let at = on_tile(&mut app, root, 1, Vec2::new(0.85, 0.15));
    input(
        &mut app,
        &format!("input.pointer x={} y={} action=move", at.x, at.y),
    );
    let hovers: Vec<(ZoneHover, Display)> = app
        .world_mut()
        .query::<(&ZoneHover, &Node)>()
        .iter(app.world())
        .map(|(h, n)| (*h, n.display))
        .collect();
    assert_eq!(hovers.len(), 1, "one outline per sheet");
    assert_eq!(hovers[0].0.tile, Some(1));
    assert_ne!(hovers[0].1, Display::None);

    pointer(&mut app, at, "click", "primary");
    let kept: Vec<(Option<u32>, Display)> = app
        .world_mut()
        .query::<(&ZoneHover, &Node)>()
        .iter(app.world())
        .map(|(h, n)| (h.tile, n.display))
        .collect();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].0, Some(1), "the outline stays through the redraw");
    assert_ne!(kept[0].1, Display::None);
}

#[test]
fn a_zone_painted_with_a_shared_terrain_shows_in_the_set_it_was_painted_in() {
    let (mut app, _dir) = handmade();
    call(&mut app, "tileset.terrain.add", &[("name", "sand")]);
    call(&mut app, "tileset.terrain_set.add", &[("mode", "sides")]);
    for terrain in ["grass", "sand"] {
        let join = [("set", "1"), ("terrain", terrain)];
        call(&mut app, "tileset.terrain_set.add_terrain", &join);
    }
    let corner = [
        ("set", "0"),
        ("terrain", "grass"),
        ("zones", "0:5:top_left"),
    ];
    call(&mut app, "tileset.tile.paint_bits", &corner);
    assert!(warnings(&app).is_empty(), "{:?}", warnings(&app));
    settle(&mut app);
    let root = painting(&mut app);
    press(&mut app, root, DockAction::ActiveSet(0));
    assert!(
        marks(&mut app, root).contains(&(5, 0)),
        "{:?}",
        marks(&mut app, root)
    );
}

#[test]
fn a_click_in_select_mode_selects_and_paints_nothing() {
    let (mut app, dir) = handmade();
    let (_, root) = opened(&mut app);
    press(&mut app, root, DockAction::Brush(WATER));
    let (before, count) = (on_disk(&dir), entries(&app));
    let at = on_tile(&mut app, root, 1, Vec2::new(0.15, 0.15));
    pointer(&mut app, at, "click", "primary");
    assert_eq!(view(&app).mode, ModeId::TILES);
    assert_eq!(view(&app).selected, Some((0, 1)));
    assert_eq!(on_disk(&dir), before);
    assert_eq!(entries(&app), count);
}

#[test]
fn a_click_in_the_terrains_mode_paints_one_zone_and_the_mode_operator_steps_modes() {
    let (mut app, dir) = handmade();
    let (_, root) = opened(&mut app);
    press(&mut app, root, DockAction::Brush(WATER));
    input(&mut app, "tileset.mode");
    assert_eq!(view(&app).mode, ModeId::TERRAINS);
    let count = entries(&app);
    let at = on_tile(&mut app, root, 1, Vec2::new(0.15, 0.15));
    pointer(&mut app, at, "click", "primary");
    let one = [WATER, 0, 0, 0, 0, 0, 0, 0, 0];
    assert_eq!(peers_of(&on_disk(&dir), 1), Some(one));
    assert_eq!(entries(&app), count + 1);
    input(&mut app, "tileset.mode");
    assert_eq!(view(&app).mode, ModeId::COLLISION);
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
    call(&mut app, "tileset.mode", &[("mode", "terrains")]);
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
    let (mut app, dir) = crate::operators::tileset_ops::project_app();
    crate::operators::tileset_ops::import(
        &mut app,
        &[("paths", crate::operators::tileset_ops::SHEET)],
    );
    call(&mut app, "tileset.terrain.add", &[("name", "grass")]);
    settle(&mut app);
    let root = painting(&mut app);
    let (before, count) = (on_disk(&dir), entries(&app));
    let from = on_tile(&mut app, root, 1, Vec2::new(0.15, 0.15));
    let to = on_tile(&mut app, root, 2, Vec2::new(0.85, 0.15));
    drag(&mut app, from, to, "secondary");
    assert_eq!(on_disk(&dir), before);
    assert_eq!(entries(&app), count);
    let notice = app
        .world()
        .resource::<jackdaw_editor_core::status_bar::StatusNotice>();
    assert!(
        !notice.text().contains("no terrain set"),
        "{}",
        notice.text()
    );
}

fn pick_mode(app: &mut App, root: Entity, selected: usize) {
    let combo: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &NewSetPick)>()
        .iter(app.world())
        .filter(|(_, p)| p.root == root)
        .map(|(e, _)| e)
        .collect();
    assert_eq!(combo.len(), 1, "one mode picker");
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
    pick_mode(&mut app, root, 1);
    let count = entries(&app);
    press(&mut app, root, DockAction::CreateSet);
    let sets: Vec<_> = (on_disk(&dir).terrain_sets.into_iter())
        .map(|s| (s.name, format!("{:?}", s.mode), s.terrains))
        .collect();
    assert_eq!(
        sets[1],
        ("Terrain set 2".to_string(), "Sides".to_string(), vec![]),
        "{sets:?}"
    );
    assert_eq!(entries(&app), count + 1);
    assert_eq!(view(&app).active.as_deref(), Some("Terrain set 2"));

    press(&mut app, root, DockAction::Brush(SAND));
    press(&mut app, root, DockAction::AddBrushToSet(1));
    assert_eq!(on_disk(&dir).terrain_sets[1].terrains, [SAND]);
    let texts = texts(&mut app, root);
    assert!(
        texts
            .iter()
            .any(|t| t == "Terrain set 2 (Sides): 0 of 16 shapes"),
        "{texts:?}"
    );
    press(&mut app, root, DockAction::RemoveFromSet(1, SAND));
    assert!(on_disk(&dir).terrain_sets[1].terrains.is_empty());
}

#[test]
fn tileset_operators_without_a_path_edit_the_focused_tab() {
    let (mut app, dir) = handmade();
    call(&mut app, "tileset.terrain.add", &[("name", "sand")]);
    settle(&mut app);
    let (_, root) = opened(&mut app);
    press(&mut app, root, DockAction::Tile(0, 3));
    app.world_mut()
        .resource_mut::<jackdaw_editor_core::definition_assets::OpenDefinition>()
        .0 = None;
    let mut call = app.world_mut().operator("tileset.terrain_set.add");
    for (key, value) in [("name", "Sand"), ("mode", "sides")] {
        call = call.param(
            key,
            jackdaw_scene_types::PropertyValue::from(value.to_string()),
        );
    }
    call.call().expect("dispatches").assert_finished();
    settle(&mut app);
    assert!(warnings(&app).is_empty(), "{:?}", warnings(&app));
    let names: Vec<String> = (on_disk(&dir).terrain_sets.into_iter())
        .map(|s| s.name)
        .collect();
    assert_eq!(names, ["Terrain set 1", "Sand"]);
}

/// The painted tile count the More menu of the dock under `root` shows.
fn bits_count(app: &mut App, root: Entity) -> String {
    (more_menu(app, root).into_iter())
        .find_map(|(action, _)| action.strip_prefix("##").map(str::to_string))
        .unwrap_or_default()
}

#[test]
fn the_more_menu_counts_the_tiles_with_bits_as_they_are_painted() {
    let (mut app, _dir) = handmade();
    let root = painting(&mut app);
    assert_eq!(bits_count(&mut app, root), "1 tile painted");
    press(&mut app, root, DockAction::Brush(WATER));
    let from = on_tile(&mut app, root, 1, Vec2::new(0.15, 0.15));
    let to = on_tile(&mut app, root, 3, Vec2::new(0.85, 0.15));
    drag(&mut app, from, to, "primary");
    assert_eq!(bits_count(&mut app, root), "4 tiles painted");
}

#[test]
fn the_overlay_marks_only_tiles_the_coverage_counts() {
    let (mut app, _dir) = handmade();
    call(&mut app, "tileset.terrain.add", &[("name", "sand")]);
    call(&mut app, "tileset.terrain_set.add", &[("mode", "sides")]);
    let join = [("set", "1"), ("terrain", "sand")];
    call(&mut app, "tileset.terrain_set.add_terrain", &join);
    let mixed = [("tile", "0:30"), ("peers", "1,0,1,0,3,0,1,0,1")];
    call(&mut app, "tileset.tile.set_bits", &mixed);
    settle(&mut app);
    let root = painting(&mut app);
    press(&mut app, root, DockAction::ActiveSet(0));
    let tiles: BTreeSet<u32> = marks(&mut app, root).into_iter().map(|(t, _)| t).collect();
    assert_eq!(
        tiles,
        BTreeSet::from([0]),
        "a tile mixing terrains of two sets is neither's, for the solver and the coverage"
    );
    assert_eq!(
        coverage(&mut app, root),
        "Terrain set 1 (Corners): 1 of 16 shapes"
    );
}

fn summary(app: &mut App, root: Entity) -> String {
    (texts(app, root).into_iter())
        .find(|t| t.starts_with("Corners:"))
        .unwrap_or_default()
}

fn glyphs(app: &mut App, root: Entity) -> usize {
    app.world_mut()
        .query::<&jackdaw_editor_tile::tileset_bits::MissingGlyph>()
        .iter(app.world())
        .filter(|g| g.root == root)
        .count()
}

#[test]
fn the_missing_shapes_follow_the_stroke_as_it_paints() {
    let (mut app, _dir) = handmade();
    let root = painting(&mut app);
    call(
        &mut app,
        "tileset.tile.set_bits",
        &[("tile", "0:1"), ("peers", "2,0,2,0,2,0,0,0,0")],
    );
    settle(&mut app);
    press(&mut app, root, DockAction::ToggleMissing(0));
    press(&mut app, root, DockAction::Brush(GRASS));
    assert_eq!(
        summary(&mut app, root),
        "Corners: 30 shapes missing",
        "tile 1 shows water on empty, so that transition counts too"
    );
    assert_eq!(glyphs(&mut app, root), 30);
    let from = on_tile(&mut app, root, 1, Vec2::new(0.15, 0.85));
    let to = on_tile(&mut app, root, 1, Vec2::new(0.85, 0.85));
    pointer(&mut app, from, "press", "primary");
    pointer(&mut app, to, "move", "primary");
    assert_eq!(
        summary(&mut app, root),
        "Corners: 14 shapes missing",
        "live, before the stroke ends"
    );
    assert_eq!(glyphs(&mut app, root), 14, "live, before the stroke ends");
    pointer(&mut app, to, "release", "primary");
    assert_eq!(summary(&mut app, root), "Corners: 14 shapes missing");
}
