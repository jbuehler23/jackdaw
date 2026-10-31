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
    CollisionCombo, DockAction, DockControl, DockPage, DockView, NameField, SheetPicture,
    TILESET_WINDOW_ID, TerrainNameField, TerrainSwatch, TilesetDock, TilesetDockInputContext,
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
    let mut pages: Vec<DockAction> = (actions(&mut app, root).into_iter())
        .filter(|a| matches!(a, DockAction::Page(_)))
        .collect();
    pages.sort_by_key(|a| *a != DockAction::Page(DockPage::Sheet));
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
fn the_missing_shapes_of_a_set_open_on_request() {
    let (mut app, _dir) = handmade();
    let (_, root) = opened(&mut app);
    let listed = |app: &mut App| {
        (texts(app, root).into_iter())
            .filter(|t| t.trim_start().starts_with("missing "))
            .count()
    };
    let shown = texts(&mut app, root);
    assert!(
        shown.iter().any(|t| t == "water over grass: Corners 1/16"),
        "{shown:?}"
    );
    assert_eq!(listed(&mut app), 0, "collapsed: {shown:?}");
    press(&mut app, root, DockAction::ToggleMissing(WATER, GRASS));
    assert_eq!(listed(&mut app), 15);
    press(&mut app, root, DockAction::ToggleMissing(WATER, GRASS));
    assert_eq!(listed(&mut app), 0);
    press(&mut app, root, DockAction::ToggleMissing(WATER, GRASS));
    press(&mut app, root, DockAction::Page(DockPage::Objects));
    assert!(!texts(&mut app, root).iter().any(|t| t.contains("missing")));
}

/// Run a synthetic input clause and let it drain.
fn input(app: &mut App, clause: &str) {
    jackdaw::boot_ops::run_op_clause(app.world_mut(), clause)
        .expect("the clause dispatches")
        .assert_finished();
    for _ in 0..600 {
        app.update();
        let input = app
            .world()
            .resource::<jackdaw::test_input::SyntheticInput>();
        if input.is_idle() {
            break;
        }
    }
    settle(app);
}

/// The open name field of the dock under `root`: its kind, its editable
/// entity and its text.
fn name_field(app: &mut App, root: Entity) -> Option<(NameField, Entity, String)> {
    let fields: Vec<(Entity, NameField)> = app
        .world_mut()
        .query::<(Entity, &TerrainNameField)>()
        .iter(app.world())
        .filter(|(_, f)| f.root == root)
        .map(|(e, f)| (e, f.field))
        .collect();
    assert!(fields.len() <= 1, "{fields:?}");
    let (outer, field) = fields.first().copied()?;
    let mut stack = vec![outer];
    while let Some(entity) = stack.pop() {
        if let Some(editable) = app.world().get::<bevy::text::EditableText>(entity) {
            return Some((field, entity, editable.value().into_owned()));
        }
        if let Some(children) = app.world().get::<Children>(entity) {
            stack.extend(children.iter());
        }
    }
    None
}

fn focused(app: &App) -> Option<Entity> {
    app.world()
        .resource::<bevy::input_focus::InputFocus>()
        .get()
}

#[test]
fn n_on_a_selected_tile_names_a_new_terrain_shown_by_it() {
    let (mut app, dir) = handmade();
    let (_, root) = opened(&mut app);
    input(&mut app, "input.key key=KeyN");
    assert_eq!(name_field(&mut app, root), None, "no tile, no field");

    press(&mut app, root, DockAction::Tile(0, 5));
    let count = entries(&app);
    input(&mut app, "input.key key=KeyN");
    assert_eq!(view(&app).naming, Some(NameField::New));
    let (_, field, prefill) = name_field(&mut app, root).expect("the field opens");
    assert_eq!(prefill, "Terrain 3");
    assert_eq!(focused(&app), Some(field), "it has the keyboard");
    assert_eq!(entries(&app), count, "opening it is no edit");

    input(&mut app, "input.text text=dirt");
    input(&mut app, "input.key key=Enter");
    let terrains = on_disk(&dir).terrains;
    assert_eq!(terrains.len(), 3, "{terrains:?}");
    let dirt = &terrains[2];
    assert_eq!((dirt.name.as_str(), dirt.icon), ("dirt", Some((0, 5))));
    assert_eq!(entries(&app), count + 1, "one undo entry");
    assert_eq!(view(&app).naming, None);
    assert_eq!(name_field(&mut app, root), None);

    let brush = view(&app).brush;
    press(&mut app, root, DockAction::Tile(0, 9));
    press(&mut app, root, DockAction::Tile(0, 5));
    assert_eq!(
        view(&app).brush,
        brush,
        "an icon tile is a tile, not a pick"
    );

    run(&mut app, "history.undo");
    settle(&mut app);
    assert_eq!(on_disk(&dir).terrains.len(), 2);
}

#[test]
fn escape_closes_the_name_field_and_adds_nothing() {
    let (mut app, dir) = handmade();
    let (_, root) = opened(&mut app);
    press(&mut app, root, DockAction::Tile(0, 5));
    let (before, count) = (on_disk(&dir), entries(&app));
    input(&mut app, "input.key key=KeyN");
    assert!(name_field(&mut app, root).is_some());
    input(&mut app, "input.text text=dirt");
    input(&mut app, "input.key key=Escape");
    assert_eq!(view(&app).naming, None);
    assert_eq!(name_field(&mut app, root), None);
    assert_eq!((on_disk(&dir), entries(&app)), (before, count));
}

#[test]
fn the_new_terrain_button_opens_the_same_field() {
    let (mut app, dir) = handmade();
    let (_, root) = opened(&mut app);
    assert!(
        !actions(&mut app, root).contains(&DockAction::NewTerrain),
        "no tile, no button"
    );
    press(&mut app, root, DockAction::Tile(0, 6));
    press(&mut app, root, DockAction::NewTerrain);
    let (field, _, prefill) = name_field(&mut app, root).expect("the field opens");
    assert_eq!((field, prefill.as_str()), (NameField::New, "Terrain 3"));
    input(&mut app, "input.key key=Enter");
    let added = on_disk(&dir).terrains.pop().expect("added");
    assert_eq!(
        (added.name.as_str(), added.icon),
        ("Terrain 3", Some((0, 6)))
    );
}

#[test]
fn the_brush_terrain_is_renamed_and_removed_from_the_list() {
    let (mut app, dir) = handmade();
    let (_, root) = opened(&mut app);
    assert!(!actions(&mut app, root).contains(&DockAction::RenameTerrain(WATER)));
    press(&mut app, root, DockAction::Brush(WATER));
    press(&mut app, root, DockAction::RenameTerrain(WATER));
    let (field, _, prefill) = name_field(&mut app, root).expect("the field opens");
    assert_eq!(
        (field, prefill.as_str()),
        (NameField::Rename(WATER), "water")
    );
    let count = entries(&app);
    input(&mut app, "input.text text=lake");
    input(&mut app, "input.key key=Enter");
    let names: Vec<String> = (on_disk(&dir).terrains.into_iter())
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["grass", "lake"]);
    assert_eq!(entries(&app), count + 1);

    press(&mut app, root, DockAction::RemoveTerrain(WATER));
    let names: Vec<String> = (on_disk(&dir).terrains.into_iter())
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["grass"]);
    assert_eq!(entries(&app), count + 2);
}

#[test]
fn terrains_are_listed_by_their_icon_tile_and_sets_by_their_names() {
    let (mut app, dir) = handmade();
    call(
        &mut app,
        "tileset.terrain.add",
        &[("name", "dirt"), ("icon", "0:5")],
    );
    let (_, root) = opened(&mut app);
    let swatches: Vec<(Entity, u8)> = app
        .world_mut()
        .query::<(Entity, &TerrainSwatch)>()
        .iter(app.world())
        .filter(|(_, s)| s.root == root)
        .map(|(e, s)| (e, s.terrain))
        .collect();
    let of = |terrain: u8| {
        (swatches.iter())
            .find(|(_, t)| *t == terrain)
            .map(|(e, _)| *e)
            .expect("listed")
    };
    let tileset = on_disk(&dir);
    let dirt = tileset.terrains[2].id;
    let rect = app
        .world()
        .get::<ImageNode>(of(dirt))
        .and_then(|image| image.rect);
    let (min, size) = tileset.sheet_rect(0, 5).expect("a tile");
    assert_eq!(
        rect,
        Some(Rect::from_corners(min.as_vec2(), (min + size).as_vec2()))
    );
    let grass = of(GRASS);
    assert!(app.world().get::<ImageNode>(grass).is_none());
    let colour = app.world().get::<BackgroundColor>(grass).map(|c| c.0);
    assert!(colour.is_some_and(|c| c.alpha() == 1.0), "{colour:?}");

    let shown = texts(&mut app, root);
    assert!(
        shown.iter().any(|t| t.starts_with("water over grass:")),
        "{shown:?}"
    );
    assert!(
        !shown.iter().any(|t| t.contains("set 2 over 1")),
        "{shown:?}"
    );
}

/// The screen rect of `entity`, in logical pixels.
fn screen_rect(app: &App, entity: Entity) -> Rect {
    let node = app.world().get::<ComputedNode>(entity).expect("laid out");
    let at = app
        .world()
        .get::<bevy::ui::UiGlobalTransform>(entity)
        .expect("placed");
    let scale = node.inverse_scale_factor();
    let centre = at.translation * scale;
    let size = node.size() * scale;
    Rect::from_center_size(centre, size)
}

fn set_width(app: &mut App, width: u32) {
    let mut windows = app
        .world_mut()
        .query_filtered::<&mut Window, With<bevy::window::PrimaryWindow>>();
    let mut window = windows.single_mut(app.world_mut()).expect("a window");
    window.resolution = bevy::window::WindowResolution::new(width, 1000);
    settle(app);
}

#[test]
fn the_sheet_image_and_its_tile_grid_share_one_layout() {
    let (mut app, dir) = handmade();
    let (_, root) = opened(&mut app);
    let tileset = on_disk(&dir);
    let sheet = &tileset.sheets[0];
    let last = sheet.columns * sheet.rows - 1;
    for width in [950, 1800] {
        set_width(&mut app, width);
        let pictures: Vec<(Entity, Handle<Image>)> = app
            .world_mut()
            .query::<(Entity, &SheetPicture, &ImageNode)>()
            .iter(app.world())
            .filter(|(_, p, _)| p.root == root && p.sheet == 0)
            .map(|(e, _, i)| (e, i.image.clone()))
            .collect();
        assert_eq!(pictures.len(), 1, "one picture of the sheet");
        let (picture, image) = pictures[0].clone();
        let pixels = (app.world().resource::<Assets<Image>>().get(&image))
            .expect("decoded")
            .size()
            .as_vec2();
        let drawn = screen_rect(&app, picture);
        let zoom = drawn.size() / pixels;
        assert_eq!(zoom.x, zoom.y, "{width}: one scale");
        assert!(zoom.x >= 1.0 && zoom.x.fract() == 0.0, "{width}: {zoom}");
        for tile in [0, 27, last / 2, last] {
            let cell = control(&mut app, root, DockAction::Tile(0, tile));
            let (min, size) = tileset.sheet_rect(0, tile).expect("a tile");
            let expected = Rect::from_corners(
                drawn.min + min.as_vec2() * zoom,
                drawn.min + (min + size).as_vec2() * zoom,
            );
            let got = screen_rect(&app, cell);
            assert!(
                (got.min - expected.min).abs().max_element() < 0.5
                    && (got.max - expected.max).abs().max_element() < 0.5,
                "{width}: tile {tile} is drawn at {expected:?}, its cell is at {got:?}",
            );
        }
    }
}

fn dock_context_active(app: &mut App) -> bool {
    app.world_mut()
        .query_filtered::<&jackdaw_input::ContextActivity, With<TilesetDockInputContext>>()
        .iter(app.world())
        .any(|a| a.0)
}

#[test]
fn the_keys_leave_with_the_tab_that_had_them() {
    let (mut app, _dir) = handmade();
    let (_, root) = opened(&mut app);
    assert!(!dock_context_active(&mut app));
    press(&mut app, root, DockAction::Tile(0, 5));
    assert!(
        dock_context_active(&mut app),
        "a click gives the tab the keys"
    );
    input(&mut app, "input.key key=KeyN");
    assert!(name_field(&mut app, root).is_some());
    app.world_mut().entity_mut(root).despawn();
    settle(&mut app);
    assert!(!dock_context_active(&mut app), "the focused field is gone");
}

#[test]
fn the_new_terrain_button_builds_the_dock_once() {
    let (mut app, _dir) = handmade();
    count_builds(&mut app);
    let (_, root) = opened(&mut app);
    press(&mut app, root, DockAction::Tile(0, 6));
    let built = builds(&app);
    press(&mut app, root, DockAction::NewTerrain);
    assert!(name_field(&mut app, root).is_some());
    assert_eq!(builds(&app) - built, 2, "one build of two terrain rows");
}

#[test]
fn the_selected_tile_becomes_the_brush_terrains_icon() {
    let (mut app, dir) = handmade();
    let (_, root) = opened(&mut app);
    press(&mut app, root, DockAction::Brush(WATER));
    assert!(
        !actions(&mut app, root).contains(&DockAction::SetIcon(WATER)),
        "no tile yet"
    );
    press(&mut app, root, DockAction::Tile(0, 9));
    let count = entries(&app);
    press(&mut app, root, DockAction::SetIcon(WATER));
    let water = (on_disk(&dir).terrains.into_iter()).find(|t| t.id == WATER);
    assert_eq!(water.and_then(|t| t.icon), Some((0, 9)));
    assert_eq!(entries(&app), count + 1);

    call(&mut app, "tileset.terrain.remove", &[("terrain", "grass")]);
    settle(&mut app);
    assert_eq!(
        view(&app).brush,
        WATER,
        "another terrain's removal keeps the brush"
    );
}
