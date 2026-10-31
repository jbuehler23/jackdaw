//! The Tileset window: one tab per tileset carrying its path, its plain sheet
//! page and controls, each a call of a tileset operator, and the clearing of
//! automatic setup on open.

use crate::tileset_edit_ops::{FILE, call, entries, on_disk, overworld, run};
use crate::tileset_ops::{SHEET, editor_on, import, project_app, warnings};
use crate::util;
use crate::util::OperatorResultExt as _;

use std::path::Path;
use std::time::Duration;

use bevy::prelude::*;
use bevy::ui_widgets::Activate;
use jackdaw::asset_index::AssetIndex;
use jackdaw::tileset_dock::{
    CollisionCombo, DockAction, DockControl, DockPage, DockView, TILESET_WINDOW_ID, TilesetDock,
    TilesetDockViews,
};
use jackdaw_api::prelude::*;
use jackdaw_feathers::combobox::ComboBoxChangeEvent;
use jackdaw_panels::tree::{DockTree, Edge, TabId};
use jackdaw_scene_types::PropertyValue;
use jackdaw_scene_types::tilemap::{Tilemap, TilemapLayer};
use jackdaw_tileset::{TerrainCollision, Tileset};

const SECOND: &str = "second.tileset.bsn";

const GRASS: u8 = 1;
const WATER: u8 = 2;

/// The overworld sheet imported with nothing set up, then grass and water, a
/// water over grass set and tile 0 of grass with a water corner, by hand.
fn handmade() -> (App, tempfile::TempDir) {
    let (mut app, dir) = project_app();
    import(&mut app, &[("paths", SHEET)]);
    call(&mut app, "tileset.terrain.add", &[("name", "grass")]);
    call(&mut app, "tileset.terrain.add", &[("name", "water")]);
    let set = [("inner", "2"), ("outer", "1"), ("mode", "corners")];
    call(&mut app, "tileset.terrain_set.add", &set);
    let bits = [("tile", "0:0"), ("peers", "1,1,1,1,1,1,1,1,2")];
    call(&mut app, "tileset.tile.set_bits", &bits);
    assert!(warnings(&app).is_empty(), "{:?}", warnings(&app));
    settle(&mut app);
    (app, dir)
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
    let (mut app, dir) = handmade();
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
    let (mut app, _dir) = handmade();
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
    let (mut app, _dir) = handmade();
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
    let (mut app, _dir) = handmade();
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
    let (app, _dir) = handmade();
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

fn actions(app: &mut App, root: Entity) -> Vec<DockAction> {
    app.world_mut()
        .query::<&DockControl>()
        .iter(app.world())
        .filter(|c| c.root == root)
        .map(|c| c.action)
        .collect()
}

#[test]
fn the_dock_opens_on_a_plain_sheet_page() {
    let (mut app, dir) = handmade();
    let (_, root) = opened(&mut app);
    assert_eq!(view(&app).page, DockPage::Sheet);
    let pages: Vec<DockAction> = (actions(&mut app, root).into_iter())
        .filter(|a| matches!(a, DockAction::Page(_)))
        .collect();
    assert_eq!(
        pages,
        [
            DockAction::Page(DockPage::Sheet),
            DockAction::Page(DockPage::Objects)
        ]
    );
    let shown = texts(&mut app, root);
    for gone in ["Review", "Accept", "Reset to auto"] {
        assert!(!shown.iter().any(|t| t == gone), "{gone}: {shown:?}");
    }
    assert!(
        !shown.iter().any(|t| t.to_lowercase().contains("flagged")),
        "{shown:?}"
    );

    let sheet = &on_disk(&dir).sheets[0];
    let cells: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &DockControl)>()
        .iter(app.world())
        .filter(|(_, c)| c.root == root && matches!(c.action, DockAction::Tile(..)))
        .map(|(e, _)| e)
        .collect();
    assert_eq!(cells.len() as u32, sheet.columns * sheet.rows);
    for cell in cells {
        let tint = app.world().get::<BackgroundColor>(cell).map(|c| c.0);
        assert!(tint.is_none_or(|c| c.alpha() == 0.0), "{tint:?}");
        let marks = (app.world().get::<Children>(cell)).map_or(0, |c| c.iter().count());
        assert_eq!(marks, 0, "a sheet tile carries no marks");
    }
}

#[test]
fn a_peer_click_paints_the_brush_and_the_view_follows_undo() {
    let (mut app, dir) = handmade();
    count_builds(&mut app);
    let (_, root) = opened(&mut app);
    press(&mut app, root, DockAction::Tile(0, 0));
    press(&mut app, root, DockAction::Brush(WATER));
    let count = entries(&app);
    let built = builds(&app);
    press(&mut app, root, DockAction::Peer(1));

    let peers = |dir: &tempfile::TempDir| {
        (on_disk(dir).tiles.iter())
            .find(|t| t.sheet == 0 && t.tile == 0)
            .map(|t| t.peers)
    };
    assert_eq!(peers(&dir).map(|p| p[1]), Some(WATER));
    assert_eq!(entries(&app), count + 1, "one undo entry");
    let terrains = on_disk(&dir).terrains.len();
    assert_eq!(builds(&app) - built, terrains, "the dock is built once");

    run(&mut app, "history.undo");
    settle(&mut app);
    assert_eq!(peers(&dir).map(|p| p[1]), Some(GRASS));
    assert_eq!(view(&app).selected, Some((0, 0)));
}

/// An editor opened again on `dir`, once its tileset is indexed and loaded.
fn reopened(dir: &tempfile::TempDir) -> App {
    let mut app = editor_on(dir);
    util::update_until(
        &mut app,
        "the tileset to load",
        Duration::from_secs(20),
        |app| {
            app.world()
                .resource::<AssetIndex>()
                .get(Path::new(FILE))
                .is_some()
                && app
                    .world()
                    .resource::<Assets<Tileset>>()
                    .iter()
                    .next()
                    .is_some()
        },
    );
    settle(&mut app);
    app
}

/// The overworld set up automatically in an earlier session.
fn auto_from_before() -> (App, tempfile::TempDir) {
    let (app, dir) = overworld();
    drop(app);
    (reopened(&dir), dir)
}

#[track_caller]
fn assert_cleared(before: &Tileset, after: &Tileset) {
    assert!(after.terrains.is_empty() && after.terrain_sets.is_empty());
    assert!(after.tiles.is_empty() && after.objects.is_empty());
    assert_eq!(after.setup_sheets, before.setup_sheets, "the slicing stays");
    assert_eq!(after.next_terrain_id, before.next_terrain_id);
    assert!(!after.auto_setup);
}

#[test]
fn opening_clears_untouched_auto_setup_once_as_one_undo_entry() {
    let (mut app, dir) = auto_from_before();
    let (before, count) = (on_disk(&dir), entries(&app));
    assert!(before.auto_setup && !before.terrains.is_empty());

    open(&mut app, FILE);
    assert_cleared(&before, &on_disk(&dir));
    assert_eq!(entries(&app), count + 1, "the clear is one undo entry");
    assert_eq!(view(&app).page, DockPage::Sheet);

    run(&mut app, "history.undo");
    settle(&mut app);
    assert_eq!(on_disk(&dir), before);
    open(&mut app, FILE);
    assert_eq!(on_disk(&dir), before, "once a session, so undo sticks");
}

#[test]
fn an_auto_tileset_with_one_mark_by_hand_is_left_whole() {
    let (mut app, dir) = overworld();
    let rename = [("terrain", "1"), ("name", "meadow")];
    call(&mut app, "tileset.terrain.rename", &rename);
    drop(app);
    let mut app = reopened(&dir);
    let (before, count) = (on_disk(&dir), entries(&app));
    open(&mut app, FILE);
    assert_eq!(on_disk(&dir), before);
    assert_eq!(entries(&app), count);
}

#[test]
fn an_auto_import_keeps_its_setup_when_it_opens_the_tileset() {
    let (mut app, dir) = project_app();
    app.world_mut()
        .operator("tileset.import")
        .param("path", PropertyValue::from(SHEET))
        .param("setup", PropertyValue::from("auto"))
        .param("edit", PropertyValue::from(true))
        .call()
        .expect("dispatches")
        .assert_finished();
    settle(&mut app);
    assert_eq!(tabs(&app).len(), 1, "the import opens its tab");
    let kept = on_disk(&dir);
    assert!(kept.auto_setup && kept.terrains.len() == 8);
    open(&mut app, FILE);
    assert_eq!(on_disk(&dir), kept);
}

#[test]
fn a_restored_tab_clears_untouched_auto_setup_when_it_first_fills() {
    let (mut app, dir) = overworld();
    open(&mut app, FILE);
    let before = on_disk(&dir);
    assert!(!before.terrains.is_empty(), "made this session, so kept");
    jackdaw::scene_io::save_layout_to_project(app.world_mut());
    let saved = (app
        .world()
        .resource::<jackdaw::project::ProjectRoot>()
        .config)
        .clone();
    drop(app);

    let mut app = reopened(&dir);
    app.world_mut()
        .resource_mut::<jackdaw::project::ProjectRoot>()
        .config = saved;
    *app.world_mut().resource_mut::<DockTree>() = DockTree::default();
    jackdaw::init_layout(app.world_mut());
    settle(&mut app);
    assert_eq!(tabs(&app).len(), 1);
    assert_cleared(&before, &on_disk(&dir));
}

#[test]
fn an_auto_tileset_a_map_paints_with_is_kept_with_a_notice() {
    let notice = |app: &App| {
        let notice = app.world().resource::<jackdaw::status_bar::StatusNotice>();
        notice.text().to_string()
    };
    let (mut app, dir) = auto_from_before();
    app.world_mut().spawn(Tilemap {
        layers: vec![TilemapLayer {
            name: "ground".into(),
            tileset: FILE.into(),
            ..default()
        }],
        ..default()
    });
    let before = on_disk(&dir);
    open(&mut app, FILE);
    assert_eq!(on_disk(&dir), before, "an open map paints with it");
    assert!(
        notice(&app).contains("automatic terrains"),
        "{}",
        notice(&app)
    );

    let (app, dir) = overworld();
    drop(app);
    let scene = format!("Tilemap {{ layers: [TilemapLayer {{ tileset: \"{FILE}\" }}] }}\n");
    std::fs::write(dir.path().join("assets/level.bsn"), scene).expect("a scene");
    let mut app = reopened(&dir);
    let before = on_disk(&dir);
    open(&mut app, FILE);
    assert_eq!(
        on_disk(&dir),
        before,
        "a scene of the project paints with it"
    );
    assert!(
        notice(&app).contains("automatic terrains"),
        "{}",
        notice(&app)
    );
}

#[test]
fn opening_leaves_a_hand_made_tileset_as_it_is() {
    let (mut app, dir) = handmade();
    let (before, count) = (on_disk(&dir), entries(&app));
    opened(&mut app);
    assert_eq!(on_disk(&dir), before);
    assert_eq!(
        entries(&app),
        count,
        "no history for a tileset with nothing to clear"
    );
}

#[test]
fn the_collision_combo_sets_the_terrain_collision() {
    let (mut app, dir) = handmade();
    let (_, root) = opened(&mut app);
    let combo: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &CollisionCombo)>()
        .iter(app.world())
        .filter(|(_, c)| c.root == root && c.terrain == WATER)
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
    let water = (on_disk(&dir).terrains.into_iter()).find(|t| t.id == WATER);
    assert_eq!(water.map(|t| t.collision), Some(TerrainCollision::Solid));
    assert_eq!(entries(&app), count + 1);
}

#[test]
fn objects_are_added_from_a_rectangle_and_removed() {
    let (mut app, dir) = handmade();
    let (_, root) = opened(&mut app);
    press(&mut app, root, DockAction::Page(DockPage::Objects));
    // Two tiles of the bottom row, which no found object covers.
    let columns = on_disk(&dir).sheets[0].columns;
    let last = on_disk(&dir).sheets[0].rows - 1;
    press(&mut app, root, DockAction::Tile(0, last * columns));
    press(&mut app, root, DockAction::Tile(0, last * columns + 1));

    let count = entries(&app);
    press(&mut app, root, DockAction::AddObject);
    let added = on_disk(&dir);
    let object = (added.objects.iter())
        .find(|o| o.origin == UVec2::new(0, last))
        .expect("the object is added");
    assert_eq!(object.size, UVec2::new(2, 1));
    assert_eq!(entries(&app), count + 1);

    let count = entries(&app);
    press(
        &mut app,
        root,
        DockAction::RemoveObject(0, UVec2::new(0, last)),
    );
    let left = on_disk(&dir).objects;
    assert!(!left.iter().any(|o| o.origin == UVec2::new(0, last)));
    assert_eq!(entries(&app), count + 1);
}

#[test]
fn the_sheet_page_lists_the_missing_shapes() {
    let (mut app, _dir) = handmade();
    let (_, root) = opened(&mut app);
    let shown = texts(&mut app, root);
    assert!(shown.iter().any(|t| t.contains("missing")), "{shown:?}");
    press(&mut app, root, DockAction::Page(DockPage::Objects));
    assert!(!texts(&mut app, root).iter().any(|t| t.contains("missing")));
}
