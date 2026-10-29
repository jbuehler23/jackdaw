//! The Tileset window: one tab per tileset carrying its path, and the review
//! controls, each a call of a tileset operator.

use crate::tileset_edit_ops::{
    DIRT, FILE, SAND, call, entries, farm, on_disk, peers_of, run, tile_of,
};
use crate::util::OperatorResultExt as _;

use bevy::prelude::*;
use bevy::ui_widgets::Activate;
use jackdaw::tileset_dock::{
    CollisionCombo, DockAction, DockControl, DockView, TILESET_WINDOW_ID, TilesetDock,
    TilesetDockViews,
};
use jackdaw_api::prelude::*;
use jackdaw_feathers::combobox::ComboBoxChangeEvent;
use jackdaw_panels::tree::{DockTree, Edge, TabId};
use jackdaw_scene_types::PropertyValue;
use jackdaw_tileset::{TerrainCollision, Tileset};

const SECOND: &str = "second.tileset.bsn";

/// The farm tileset with one tile made flagged by a peer of dirt, in the
/// editor's default dock, and that peer's terrain before.
fn docked() -> (App, tempfile::TempDir, u32, u8) {
    let (mut app, dir) = farm();
    let tile = tile_of(&on_disk(&dir), SAND);
    let was = peers_of(&on_disk(&dir), tile).expect("bits")[1];
    call(
        &mut app,
        "tileset.tile.set_bits",
        &[
            ("tile", &format!("0:{tile}")),
            ("peer", "1"),
            ("terrain", &DIRT.to_string()),
        ],
    );
    settle(&mut app);
    (app, dir, tile, was)
}

fn settle(app: &mut App) {
    for _ in 0..3 {
        app.update();
    }
}

fn open(app: &mut App, path: &str) {
    app.world_mut()
        .operator("tileset.edit")
        .param("path", PropertyValue::from(path.to_string()))
        .call()
        .expect("dispatches")
        .assert_finished();
    settle(app);
}

/// Tileset tabs as id, title and payload.
fn tabs(app: &App) -> Vec<(TabId, Option<String>, Option<String>)> {
    (app.world().resource::<DockTree>().tabs())
        .filter(|(_, t)| t.window_id == TILESET_WINDOW_ID)
        .map(|(_, t)| (t.id, t.title.clone(), t.payload.clone()))
        .collect()
}

fn in_front(app: &App, tab: TabId) -> bool {
    let tree = app.world().resource::<DockTree>();
    (tree.find_leaf_for_tab(tab))
        .and_then(|leaf| tree.get(leaf))
        .and_then(|node| node.as_leaf())
        .is_some_and(|leaf| leaf.active == Some(tab))
}

fn root(app: &mut App, tab: TabId) -> Entity {
    let found: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &TilesetDock)>()
        .iter(app.world())
        .filter(|(_, d)| d.tab == tab)
        .map(|(e, _)| e)
        .collect();
    assert_eq!(found.len(), 1, "one dock content for the tab");
    found[0]
}

fn view(app: &App) -> DockView {
    app.world().resource::<TilesetDockViews>().0[FILE].clone()
}

/// How many times the dock under `root` has been built, by the collision
/// combos each build spawns.
#[derive(Resource, Default)]
struct Builds(usize);

fn count_builds(app: &mut App) {
    app.init_resource::<Builds>();
    app.add_observer(|_: On<Add<CollisionCombo>>, mut builds: ResMut<Builds>| {
        builds.0 += 1;
    });
}

fn builds(app: &App) -> usize {
    app.world().resource::<Builds>().0
}

fn control(app: &mut App, root: Entity, action: DockAction) -> Entity {
    let found: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &DockControl)>()
        .iter(app.world())
        .filter(|(_, c)| c.root == root && c.action == action)
        .map(|(e, _)| e)
        .collect();
    assert_eq!(found.len(), 1, "one control for {action:?}: {found:?}");
    found[0]
}

fn press(app: &mut App, root: Entity, action: DockAction) {
    let entity = control(app, root, action);
    app.world_mut().trigger(Activate { entity });
    settle(app);
}

/// The texts the dock under `root` shows.
fn texts(app: &mut App, root: Entity) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        if let Some(text) = app.world().get::<Text>(entity) {
            found.push(text.0.clone());
        }
        if let Some(children) = app.world().get::<Children>(entity) {
            stack.extend(children.iter());
        }
    }
    found
}

fn opened(app: &mut App) -> (TabId, Entity) {
    open(app, FILE);
    let tab = tabs(app)[0].0;
    let root = root(app, tab);
    (tab, root)
}

#[test]
fn tileset_edit_opens_focuses_adds_and_closes_tabs() {
    let (mut app, dir, _, _) = docked();
    std::fs::copy(
        dir.path().join("assets").join(FILE),
        dir.path().join("assets").join(SECOND),
    )
    .expect("a second tileset");

    open(&mut app, FILE);
    let first = tabs(&app);
    assert_eq!(first.len(), 1);
    assert_eq!(
        first[0].1.as_deref(),
        Some(FILE),
        "the label is the file name"
    );
    assert_eq!(
        first[0].2.as_deref(),
        Some(FILE),
        "the tab carries the path"
    );
    root(&mut app, first[0].0);

    open(&mut app, SECOND);
    assert_eq!(tabs(&app).len(), 2, "a new path adds a tab");
    let second = tabs(&app)[1].0;
    assert!(in_front(&app, second));

    open(&mut app, FILE);
    assert_eq!(tabs(&app).len(), 2, "an open path is not opened twice");
    assert!(in_front(&app, first[0].0), "it comes to the front");

    app.world_mut()
        .operator("dock.close_tab")
        .param("tab_id", second.0 as i64)
        .call()
        .expect("dispatches")
        .assert_finished();
    settle(&mut app);
    assert_eq!(tabs(&app).len(), 1);
    assert_eq!(tabs(&app)[0].2.as_deref(), Some(FILE));
}

#[test]
fn the_open_tabs_survive_the_saved_layout() {
    let (mut app, _dir, _, _) = docked();
    open(&mut app, FILE);
    let before = tabs(&app);
    jackdaw::scene_io::save_layout_to_project(app.world_mut());

    *app.world_mut().resource_mut::<DockTree>() = DockTree::default();
    app.world_mut().resource_mut::<TilesetDockViews>().0.clear();
    jackdaw::init_layout(app.world_mut());
    settle(&mut app);

    let after = tabs(&app);
    assert_eq!(after.len(), 1, "{after:?}");
    assert_eq!((&after[0].1, &after[0].2), (&before[0].1, &before[0].2));
    let root = root(&mut app, after[0].0);
    assert!(texts(&mut app, root).iter().any(|t| t == FILE));
}

/// A layout restored before its tileset has loaded waits for it.
#[test]
fn a_restored_tab_shows_its_tileset_once_it_loads() {
    let (mut app, _dir, _, _) = docked();
    let (tab, first) = opened(&mut app);
    jackdaw::scene_io::save_layout_to_project(app.world_mut());
    let id = app.world().get::<TilesetDock>(first).and_then(|d| d.asset);
    let id = id.expect("the dock knows its asset");
    app.world_mut().resource_mut::<DockTree>().remove_tab(tab);
    settle(&mut app);
    let value = (app.world_mut().resource_mut::<Assets<Tileset>>())
        .remove(id)
        .expect("loaded");

    *app.world_mut().resource_mut::<DockTree>() = DockTree::default();
    jackdaw::init_layout(app.world_mut());
    let tab = tabs(&app)[0].0;
    let root = root(&mut app, tab);
    assert_eq!(texts(&mut app, root), ["loading..."]);

    let mut tilesets = app.world_mut().resource_mut::<Assets<Tileset>>();
    if !tilesets.contains(id) {
        tilesets.insert(id, value).expect("stored");
    }
    settle(&mut app);
    assert!(texts(&mut app, root).iter().any(|t| t == FILE));
}

/// A tab dragged to an edge is a new tab, and keeps what it showed.
#[test]
fn a_moved_tab_keeps_its_view_and_a_closed_one_drops_it() {
    let (mut app, _dir, _, _) = docked();
    let (tab, dock) = opened(&mut app);
    press(&mut app, dock, DockAction::Tile(0, 100));
    let moved = {
        let mut tree = app.world_mut().resource_mut::<DockTree>();
        let entry = tree.tab(tab).cloned().expect("the tab");
        let leaf = tree.find_leaf_for_tab(tab).expect("docked");
        tree.remove_tab(tab);
        let (_, moved) = (tree.split(leaf, Edge::Right, entry.window_id)).expect("split");
        let moved_entry = tree.tab_mut(moved).expect("moved");
        moved_entry.title = entry.title;
        moved_entry.payload = entry.payload;
        moved
    };
    settle(&mut app);
    let dock = root(&mut app, moved);
    assert_eq!(view(&app).selected, Some((0, 100)));
    assert!(texts(&mut app, dock).contains(&"tile 0:100".to_string()));

    app.world_mut()
        .operator("dock.close_tab")
        .param("tab_id", moved.0 as i64)
        .call()
        .expect("dispatches")
        .assert_finished();
    settle(&mut app);
    let views = app.world().resource::<TilesetDockViews>();
    assert!(!views.0.contains_key(FILE), "the closed tab's view is gone");
}

#[test]
fn the_window_menus_leave_the_tileset_window_out() {
    let (app, _dir, _, _) = docked();
    let registry = app.world().resource::<jackdaw_panels::WindowRegistry>();
    assert!(
        registry
            .get(TILESET_WINDOW_ID)
            .expect("registered")
            .unlisted
    );
    assert!(
        tabs(&app).is_empty(),
        "the default layout has no Tileset tab"
    );
}

#[test]
fn review_opens_on_the_first_flagged_tile() {
    let (mut app, _dir, flagged, _) = docked();
    let (_, root) = opened(&mut app);
    let view = view(&app);
    assert_eq!(view.selected, Some((0, flagged)));
    let shown = texts(&mut app, root);
    assert!(shown.contains(&format!("tile 0:{flagged}")), "{shown:?}");
    assert!(shown.iter().any(|t| t == "FLAGGED (1)"), "{shown:?}");
}

#[test]
fn a_peer_click_paints_the_brush_and_the_view_follows_undo() {
    let (mut app, dir, flagged, was) = docked();
    count_builds(&mut app);
    let (_, root) = opened(&mut app);
    press(&mut app, root, DockAction::Brush(was));
    let count = entries(&app);
    let built = builds(&app);
    press(&mut app, root, DockAction::Peer(1));

    let peers = |dir: &tempfile::TempDir| {
        (on_disk(dir).tiles.iter())
            .find(|t| t.sheet == 0 && t.tile == flagged)
            .map(|t| t.peers)
    };
    assert_eq!(peers(&dir).map(|p| p[1]), Some(was));
    assert_eq!(entries(&app), count + 1, "one undo entry");
    let terrains = on_disk(&dir).terrains.len();
    assert_eq!(builds(&app) - built, terrains, "the dock is built once");
    let shown = texts(&mut app, root);
    assert!(shown.iter().any(|t| t == "FLAGGED (0)"), "{shown:?}");

    run(&mut app, "history.undo");
    settle(&mut app);
    assert_eq!(peers(&dir).map(|p| p[1]), Some(DIRT));
    let shown = texts(&mut app, root);
    assert!(shown.iter().any(|t| t == "FLAGGED (1)"), "{shown:?}");
}

#[test]
fn accept_and_reset_go_through_their_operators() {
    let (mut app, dir, flagged, _) = docked();
    let (_, root) = opened(&mut app);

    let auto = (on_disk(&dir).tiles.into_iter())
        .find(|t| t.sheet == 0 && t.tile != flagged)
        .map(|t| t.tile)
        .expect("an auto tile");
    press(&mut app, root, DockAction::Tile(0, auto));
    let count = entries(&app);
    press(&mut app, root, DockAction::Accept);
    let tile = (on_disk(&dir).tiles.into_iter()).find(|t| t.sheet == 0 && t.tile == auto);
    assert_eq!(tile.map(|t| t.peers_by_user), Some([true; 9]));
    assert_eq!(entries(&app), count + 1, "accept is one undo entry");
    assert_eq!(
        view(&app).selected,
        Some((0, flagged)),
        "accept moves to the flagged tile"
    );

    press(&mut app, root, DockAction::Accept);
    let shown = texts(&mut app, root);
    assert!(shown.iter().any(|t| t == "FLAGGED (0)"), "{shown:?}");
    assert!(
        shown.iter().any(|t| t.starts_with("flagged: 0 of")),
        "{shown:?}"
    );

    let count = entries(&app);
    press(&mut app, root, DockAction::Reset);
    let tile = (on_disk(&dir).tiles.into_iter()).find(|t| t.sheet == 0 && t.tile == flagged);
    assert_eq!(tile.map(|t| t.peers_by_user), Some([false; 9]));
    assert_eq!(entries(&app), count + 1, "reset is one undo entry");
}

#[test]
fn the_collision_combo_sets_the_terrain_collision() {
    let (mut app, dir, _, _) = docked();
    let (_, root) = opened(&mut app);
    let combo: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &CollisionCombo)>()
        .iter(app.world())
        .filter(|(_, c)| c.root == root && c.terrain == DIRT)
        .map(|(e, _)| e)
        .collect();
    let count = entries(&app);
    app.world_mut().trigger(ComboBoxChangeEvent {
        entity: combo[0],
        selected: 0,
        label: String::new(),
        value: None,
    });
    settle(&mut app);
    let dirt = (on_disk(&dir).terrains.into_iter()).find(|t| t.id == DIRT);
    assert_eq!(dirt.map(|t| t.collision), Some(TerrainCollision::Solid));
    assert_eq!(entries(&app), count + 1);
}
