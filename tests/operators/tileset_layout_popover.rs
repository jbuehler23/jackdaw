//! The layout popover: a block dragged on the sheet offers the layouts that
//! fit it, previews their bits and applies one as a single edit.

use crate::tileset_dock::{control, handmade, input, opened, press, screen_rect, settle, texts};
use crate::tileset_edit_ops::{call, entries, on_disk, peers_of, run};
use crate::tileset_ops::warnings;

use std::collections::{BTreeSet, HashSet};

use bevy::prelude::*;
use jackdaw::tileset_dock::{DockAction, DockControl, TilesetDockViews};
use jackdaw::tileset_layout::{CanPaintPreview, GhostMark, LayoutPopover, LayoutTerrainPick};
use jackdaw_api::prelude::*;
use jackdaw_feathers::combobox::ComboBoxChangeEvent;
use jackdaw_tilemap::{LayoutTemplate, Placement};
use jackdaw_tileset::TerrainMatch;

const GRASS: u8 = 1;
const SAND: u8 = 3;
const GROUND: DockAction =
    DockAction::LayoutPick(LayoutTemplate::Minimal3x3, Placement::BetweenCells);
const WALLS: DockAction = DockAction::LayoutPick(LayoutTemplate::Minimal3x3, Placement::OnCells);

/// The sheet tile at column `x` and row `y`.
fn tile_at(dir: &tempfile::TempDir, x: u32, y: u32) -> u32 {
    y * on_disk(dir).sheets[0].columns + x
}

fn centre(app: &mut App, root: Entity, tile: u32) -> Vec2 {
    let cell = control(app, root, DockAction::Tile(0, tile));
    screen_rect(app, cell).center()
}

/// Drag from the tile at column 1 row 1 across a block of `w` by `h`.
fn drag_block(app: &mut App, dir: &tempfile::TempDir, root: Entity, w: u32, h: u32) {
    let from = centre(app, root, tile_at(dir, 1, 1));
    let to = centre(app, root, tile_at(dir, w, h));
    input(
        app,
        &format!("input.pointer x={} y={} action=move", from.x, from.y),
    );
    input(
        app,
        &format!(
            "input.pointer x={} y={} action=drag_to steps=12 button=primary",
            to.x, to.y
        ),
    );
}

fn block(dir: &tempfile::TempDir, w: u32, h: u32) -> Vec<u32> {
    (1..=h)
        .flat_map(|y| (1..=w).map(move |x| (x, y)))
        .map(|(x, y)| tile_at(dir, x, y))
        .collect()
}

fn popovers(app: &mut App, root: Entity) -> usize {
    (app.world_mut().query::<&LayoutPopover>().iter(app.world()))
        .filter(|p| p.root == root)
        .count()
}

/// The layouts the popover offers.
fn cards(app: &mut App, root: Entity) -> HashSet<(LayoutTemplate, Placement)> {
    (app.world_mut().query::<&DockControl>().iter(app.world()))
        .filter(|c| c.root == root)
        .filter_map(|c| match c.action {
            DockAction::LayoutPick(template, placement) => Some((template, placement)),
            _ => None,
        })
        .collect()
}

fn draft_pick(app: &App) -> Option<(LayoutTemplate, Placement)> {
    let views = app.world().resource::<TilesetDockViews>();
    let view = views.0.values().next().expect("a view");
    view.layout.as_ref().expect("a draft").pick
}

fn has_draft(app: &App) -> bool {
    let views = app.world().resource::<TilesetDockViews>();
    views.0.values().any(|v| v.layout.is_some())
}

fn disabled(app: &mut App, root: Entity, action: DockAction) -> bool {
    let entity = control(app, root, action);
    (app.world())
        .get::<bevy::ui::InteractionDisabled>(entity)
        .is_some()
}

fn ghosts(app: &mut App, root: Entity) -> BTreeSet<(u32, usize, u8)> {
    (app.world_mut().query::<&GhostMark>().iter(app.world()))
        .filter(|g| g.root == root)
        .map(|g| (g.tile, g.peer, g.terrain))
        .collect()
}

fn pick_terrain(app: &mut App, root: Entity, outer: bool, selected: usize) {
    let combo: Vec<Entity> = (app.world_mut())
        .query::<(Entity, &LayoutTerrainPick)>()
        .iter(app.world())
        .filter(|(_, p)| p.root == root && p.outer == outer)
        .map(|(e, _)| e)
        .collect();
    assert_eq!(combo.len(), 1, "one picker");
    app.world_mut().trigger(ComboBoxChangeEvent {
        entity: combo[0],
        selected,
        label: String::new(),
        value: None,
    });
    settle(app);
}

#[test]
fn a_dragged_block_opens_the_popover_with_the_layouts_that_fit_it() {
    let (mut app, dir) = handmade();
    let (_, root) = opened(&mut app);
    assert_eq!(popovers(&mut app, root), 0);
    drag_block(&mut app, &dir, root, 3, 3);
    assert_eq!(popovers(&mut app, root), 1, "the popover opens on release");
    let both = HashSet::from([
        (LayoutTemplate::Minimal3x3, Placement::BetweenCells),
        (LayoutTemplate::Minimal3x3, Placement::OnCells),
    ]);
    assert_eq!(cards(&mut app, root), both);

    press(&mut app, root, DockAction::LayoutCancel);
    drag_block(&mut app, &dir, root, 4, 4);
    let sixteen = HashSet::from([
        (LayoutTemplate::Corner16, Placement::BetweenCells),
        (LayoutTemplate::Side16, Placement::OnCells),
    ]);
    assert_eq!(cards(&mut app, root), sixteen);
    let corner = (LayoutTemplate::Corner16, Placement::BetweenCells);
    assert_eq!(draft_pick(&app), Some(corner), "a 4x4 reads as 16 corner");
}

#[test]
fn a_3x3_block_asks_ground_or_walls_with_nothing_picked() {
    let (mut app, dir) = handmade();
    let (_, root) = opened(&mut app);
    drag_block(&mut app, &dir, root, 3, 3);
    let shown = texts(&mut app, root);
    for words in [
        "Ground, tiles between cells",
        "Walls and paths, tiles on cells",
    ] {
        assert!(shown.iter().any(|t| t == words), "{words}: {shown:?}");
    }
    assert_eq!(draft_pick(&app), None, "nothing preselected");
    assert!(disabled(&mut app, root, DockAction::LayoutApply));
    press(&mut app, root, WALLS);
    assert_eq!(
        draft_pick(&app),
        Some((LayoutTemplate::Minimal3x3, Placement::OnCells))
    );
    assert!(!disabled(&mut app, root, DockAction::LayoutApply));
    assert_eq!(popovers(&mut app, root), 1, "a pick keeps the popover open");
}

#[test]
fn the_ghost_marks_are_the_bits_apply_writes() {
    let (mut app, dir) = handmade();
    let (_, root) = opened(&mut app);
    drag_block(&mut app, &dir, root, 3, 3);
    assert!(ghosts(&mut app, root).is_empty(), "no ghost before a pick");
    press(&mut app, root, GROUND);
    let shown = ghosts(&mut app, root);
    assert!(!shown.is_empty());
    press(&mut app, root, DockAction::LayoutApply);
    let after = on_disk(&dir);
    let corners = [0, 2, 6, 8];
    let written: BTreeSet<(u32, usize, u8)> = (block(&dir, 3, 3).into_iter())
        .flat_map(|tile| {
            let peers = peers_of(&after, tile).unwrap_or_default();
            corners
                .into_iter()
                .filter(move |p| peers[*p] != 0)
                .map(move |p| (tile, p, peers[p]))
        })
        .collect();
    assert_eq!(shown, written);
}

#[test]
fn apply_is_one_undo_entry_and_adds_the_set_in_the_placements_mode() {
    let (mut app, dir) = handmade();
    call(&mut app, "tileset.terrain.add", &[("name", "sand")]);
    settle(&mut app);
    let (_, root) = opened(&mut app);
    let (before, count) = (on_disk(&dir), entries(&app));
    drag_block(&mut app, &dir, root, 3, 3);
    pick_terrain(&mut app, root, false, 2);
    pick_terrain(&mut app, root, true, 1);
    press(&mut app, root, GROUND);
    press(&mut app, root, DockAction::LayoutApply);
    assert!(warnings(&app).is_empty(), "{:?}", warnings(&app));
    let after = on_disk(&dir);
    let set = (after.terrain_sets.iter())
        .find(|s| (s.inner, s.outer) == (SAND, GRASS))
        .expect("the set is added");
    assert_eq!(set.mode, TerrainMatch::Corners, "ground sits between cells");
    assert_eq!(entries(&app), count + 1, "one undo entry");
    assert_eq!(popovers(&mut app, root), 0, "apply closes the popover");

    run(&mut app, "history.undo");
    settle(&mut app);
    assert_eq!(on_disk(&dir), before);

    drag_block(&mut app, &dir, root, 3, 3);
    pick_terrain(&mut app, root, false, 2);
    pick_terrain(&mut app, root, true, 1);
    press(&mut app, root, WALLS);
    input(&mut app, "input.key key=Enter");
    let set = (on_disk(&dir).terrain_sets.into_iter())
        .find(|s| (s.inner, s.outer) == (SAND, GRASS))
        .expect("Enter applies");
    assert_eq!(set.mode, TerrainMatch::CornersAndSides);
}

#[test]
fn escape_or_cancel_leaves_the_tileset_as_it_was() {
    let (mut app, dir) = handmade();
    let (_, root) = opened(&mut app);
    let (before, count) = (on_disk(&dir), entries(&app));
    drag_block(&mut app, &dir, root, 3, 3);
    press(&mut app, root, GROUND);
    input(&mut app, "input.key key=Escape");
    assert_eq!(popovers(&mut app, root), 0, "Escape closes it");
    assert!(!has_draft(&app));
    assert!(ghosts(&mut app, root).is_empty());

    drag_block(&mut app, &dir, root, 3, 3);
    press(&mut app, root, GROUND);
    press(&mut app, root, DockAction::LayoutCancel);
    assert_eq!(popovers(&mut app, root), 0);
    assert!(!has_draft(&app));
    assert_eq!(on_disk(&dir), before);
    assert_eq!(entries(&app), count);
}

#[test]
fn the_can_paint_strip_shows_four_previews_of_the_blocks_tiles() {
    let (mut app, dir) = handmade();
    let (_, root) = opened(&mut app);
    drag_block(&mut app, &dir, root, 3, 3);
    press(&mut app, root, GROUND);
    let previews: Vec<(Entity, String)> = (app.world_mut())
        .query::<(Entity, &CanPaintPreview)>()
        .iter(app.world())
        .filter(|(_, p)| p.root == root)
        .map(|(e, p)| (e, p.name.to_string()))
        .collect();
    let names: Vec<&str> = previews.iter().map(|(_, n)| n.as_str()).collect();
    assert_eq!(names, ["spot", "thin path", "turn", "area"]);
    for (entity, name) in &previews {
        let tiles = (app.world().get::<Children>(*entity)).map_or(0, |c| {
            c.iter()
                .filter(|e| app.world().get::<ImageNode>(*e).is_some())
                .count()
        });
        assert!(tiles > 0, "{name} draws the block's tiles");
    }
}

fn ready_to_apply(app: &mut App) -> bool {
    (app.world_mut()
        .operator("tileset.layout.apply")
        .is_available())
    .expect("registered")
}

fn selected_tiles(app: &mut App, root: Entity) -> usize {
    (app.world_mut())
        .query::<(&DockControl, &BorderColor)>()
        .iter(app.world())
        .filter(|(c, b)| {
            c.root == root
                && matches!(c.action, DockAction::Tile(..))
                && b.top == jackdaw_feathers::tokens::SELECTED_BORDER
        })
        .count()
}

#[test]
fn an_apply_that_fails_keeps_the_popover_open_with_its_error() {
    let (mut app, dir) = handmade();
    call(
        &mut app,
        "tileset.object.add",
        &[("origin", "2,2"), ("size", "1,1")],
    );
    let (_, root) = opened(&mut app);
    let (before, count) = (on_disk(&dir), entries(&app));
    drag_block(&mut app, &dir, root, 3, 3);
    press(&mut app, root, GROUND);
    input(&mut app, "input.key key=Enter");
    assert_eq!(popovers(&mut app, root), 1, "the popover stays open");
    assert!(has_draft(&app), "the draft is kept");
    let shown = texts(&mut app, root);
    assert!(
        shown.iter().any(|t| t.contains("object at column 2 row 2")),
        "{shown:?}"
    );
    assert_eq!((on_disk(&dir), entries(&app)), (before, count));
}

#[test]
fn an_apply_that_changes_nothing_closes_with_no_stale_ghosts() {
    let (mut app, dir) = handmade();
    let (_, root) = opened(&mut app);
    drag_block(&mut app, &dir, root, 3, 3);
    press(&mut app, root, GROUND);
    press(&mut app, root, DockAction::LayoutApply);
    let count = entries(&app);
    drag_block(&mut app, &dir, root, 3, 3);
    press(&mut app, root, GROUND);
    press(&mut app, root, DockAction::LayoutApply);
    assert_eq!(entries(&app), count, "nothing changed");
    assert_eq!(popovers(&mut app, root), 0);
    assert!(!has_draft(&app));
    assert!(ghosts(&mut app, root).is_empty());
}

#[test]
fn switching_tool_or_page_drops_the_draft() {
    let (mut app, dir) = handmade();
    let (_, root) = opened(&mut app);
    drag_block(&mut app, &dir, root, 3, 3);
    call(&mut app, "tileset.paint_mode", &[("mode", "paint")]);
    settle(&mut app);
    assert!(!has_draft(&app), "paint mode drops it");
    assert_eq!(popovers(&mut app, root), 0);
    call(&mut app, "tileset.paint_mode", &[("mode", "select")]);
    settle(&mut app);
    drag_block(&mut app, &dir, root, 3, 3);
    press(
        &mut app,
        root,
        DockAction::Page(jackdaw::tileset_dock::DockPage::Objects),
    );
    assert!(!has_draft(&app), "a page switch drops it");
}

#[test]
fn an_existing_sets_placement_is_kept_unless_it_draws_nothing_else() {
    let (mut app, dir) = handmade();
    let (_, root) = opened(&mut app);
    let (before, count) = (on_disk(&dir), entries(&app));
    drag_block(&mut app, &dir, root, 3, 3);
    pick_terrain(&mut app, root, false, 1);
    pick_terrain(&mut app, root, true, 1);
    assert_eq!(
        draft_pick(&app),
        Some((LayoutTemplate::Minimal3x3, Placement::BetweenCells)),
        "water over grass already draws between cells"
    );
    press(&mut app, root, WALLS);
    let shown = texts(&mut app, root);
    let note = "this pair already draws between cells; it has 1 tiles elsewhere";
    assert!(shown.iter().any(|t| t.contains(note)), "{shown:?}");
    assert!(disabled(&mut app, root, DockAction::LayoutApply));
    input(&mut app, "input.key key=Enter");
    assert_eq!((on_disk(&dir), entries(&app)), (before, count));
}

#[test]
fn apply_and_cancel_leave_one_tile_selected() {
    let (mut app, dir) = handmade();
    let (_, root) = opened(&mut app);
    drag_block(&mut app, &dir, root, 3, 3);
    press(&mut app, root, DockAction::LayoutCancel);
    assert_eq!(selected_tiles(&mut app, root), 1, "after Cancel");
    drag_block(&mut app, &dir, root, 3, 3);
    press(&mut app, root, GROUND);
    press(&mut app, root, DockAction::LayoutApply);
    settle(&mut app);
    assert_eq!(selected_tiles(&mut app, root), 1, "after Apply");
}

#[test]
fn enter_applies_only_in_the_focused_dock_and_not_while_typing() {
    let (mut app, dir) = handmade();
    let (_, root) = opened(&mut app);
    drag_block(&mut app, &dir, root, 3, 3);
    press(&mut app, root, GROUND);
    assert!(ready_to_apply(&mut app));
    let mut focus = app
        .world_mut()
        .resource_mut::<bevy::input_focus::InputFocus>();
    focus.clear();
    assert!(!ready_to_apply(&mut app), "no dock has the keyboard");
    press(&mut app, root, DockAction::NewTerrain);
    assert!(has_draft(&app));
    assert!(!ready_to_apply(&mut app), "a name field has the keyboard");
}

#[test]
fn apply_gives_bare_icon_tiles_their_terrain_in_the_same_entry() {
    let (mut app, dir) = handmade();
    let icon = tile_at(&dir, 6, 0);
    let icon_ref = format!("0:{icon}");
    call(
        &mut app,
        "tileset.terrain.set_icon",
        &[("terrain", "1"), ("icon", &icon_ref)],
    );
    let (_, root) = opened(&mut app);
    let count = entries(&app);
    drag_block(&mut app, &dir, root, 3, 3);
    press(&mut app, root, GROUND);
    press(&mut app, root, DockAction::LayoutApply);
    assert_eq!(peers_of(&on_disk(&dir), icon), Some([GRASS; 9]));
    assert_eq!(entries(&app), count + 1, "one undo entry");
}
