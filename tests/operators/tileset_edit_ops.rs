//! The terrain, set and bits operators over an auto-imported Tiny Farm
//! tileset: each one's effect on the file, its single undo entry, and its
//! refusals.

use crate::tileset_ops::{SHEET, asset_file, import, project_app, read_tileset, warnings};
use crate::util::OperatorResultExt as _;

use bevy::prelude::*;
use jackdaw::remote::server::call_operator_handler;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{OperatorReports, OperatorWarnings};
use jackdaw_commands::CommandHistory;
use jackdaw_scene_types::PropertyValue;
use jackdaw_tileset::{TerrainCollision, TerrainMatch, Tileset};
use serde_json::json;

pub(crate) const FILE: &str = "tilemap_packed.tileset.bsn";
pub(crate) const GRASS: u8 = 1;
pub(crate) const SAND: u8 = 2;
pub(crate) const DIRT: u8 = 3;

pub(crate) fn farm() -> (App, tempfile::TempDir) {
    let (mut app, dir) = project_app();
    import(&mut app, &[("paths", SHEET), ("setup", "auto")]);
    assert!(warnings(&app).is_empty(), "{:?}", warnings(&app));
    (app, dir)
}

pub(crate) fn on_disk(dir: &tempfile::TempDir) -> Tileset {
    read_tileset(&asset_file(dir, FILE))
}

pub(crate) fn entries(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

pub(crate) fn call(app: &mut App, id: &'static str, params: &[(&'static str, &str)]) {
    app.world_mut()
        .get_resource_or_init::<OperatorReports>()
        .0
        .clear();
    app.world_mut()
        .get_resource_or_init::<OperatorWarnings>()
        .0
        .clear();
    let mut call = app.world_mut().operator(id);
    call = call.param("path", PropertyValue::from(FILE));
    for (key, value) in params {
        call = call.param(*key, PropertyValue::from(value.to_string()));
    }
    call.call().expect("dispatches").assert_finished();
    app.update();
}

pub(crate) fn run(app: &mut App, id: &'static str) {
    app.world_mut()
        .operator(id)
        .call()
        .expect("dispatches")
        .assert_finished();
    app.update();
}

/// Run an edit that must change the file as one undo entry that undo and
/// redo walk, and hand back the file before and after it.
#[track_caller]
pub(crate) fn edit(
    app: &mut App,
    dir: &tempfile::TempDir,
    id: &'static str,
    params: &[(&'static str, &str)],
) -> (Tileset, Tileset) {
    let available = app.world_mut().operator(id).is_available();
    assert!(available.expect("registered"), "{id} is available");
    let before = on_disk(dir);
    let count = entries(app);
    call(app, id, params);
    assert!(warnings(app).is_empty(), "{id}: {:?}", warnings(app));
    let after = on_disk(dir);
    assert_ne!(after, before, "{id} changes the file");
    assert_eq!(entries(app), count + 1, "{id} is one undo entry");
    run(app, "history.undo");
    assert_eq!(on_disk(dir), before, "{id} undoes");
    run(app, "history.redo");
    assert_eq!(on_disk(dir), after, "{id} redoes");
    (before, after)
}

#[track_caller]
pub(crate) fn refused(
    app: &mut App,
    dir: &tempfile::TempDir,
    id: &'static str,
    params: &[(&'static str, &str)],
    says: &str,
) {
    let (before, count) = (on_disk(dir), entries(app));
    call(app, id, params);
    let told = warnings(app).join("\n");
    assert!(
        told.contains(id) && told.contains(says),
        "{id} {params:?}: {told}"
    );
    assert_eq!(on_disk(dir), before, "{id} {params:?} leaves the file");
    assert_eq!(entries(app), count, "{id} {params:?} mints no history");
}

pub(crate) fn pairs(tileset: &Tileset) -> Vec<(u8, u8)> {
    (tileset.terrain_sets.iter())
        .map(|s| (s.inner, s.outer))
        .collect()
}

pub(crate) fn peers_of(tileset: &Tileset, tile: u32) -> Option<[u8; 9]> {
    (tileset.tiles.iter())
        .find(|t| t.sheet == 0 && t.tile == tile)
        .map(|t| t.peers)
}

/// The first tile holding a peer of `terrain`.
pub(crate) fn tile_of(tileset: &Tileset, terrain: u8) -> u32 {
    (tileset.tiles.iter())
        .find(|t| t.peers.contains(&terrain))
        .map(|t| t.tile)
        .expect("a tile of the terrain")
}

/// Run each call over the remote as an agent would, checking it finishes
/// cleanly with a report and, but for describe, one undo entry.
pub(crate) fn over_the_remote(app: &mut App, calls: Vec<(&str, serde_json::Value)>) {
    for (id, mut params) in calls {
        params["path"] = json!(FILE);
        let count = entries(app);
        let outcome = app
            .world_mut()
            .run_system_cached_with(
                call_operator_handler,
                Some(json!({ "id": id, "params": params })),
            )
            .expect("the handler ran")
            .unwrap_or_else(|err| panic!("{id} refused: {}", err.message));
        app.update();
        assert_eq!(outcome["result"], "finished", "{id}: {outcome}");
        assert_eq!(outcome["warnings"], json!([]), "{id}: {outcome}");
        assert!(
            !outcome["reports"].as_array().expect("reports").is_empty(),
            "{id}"
        );
        let added = usize::from(id != "tileset.describe");
        assert_eq!(entries(app), count + added, "{id}");
    }
}

#[test]
fn terrain_add_takes_the_next_id_and_refuses_taken_names_and_spent_ids() {
    let (mut app, dir) = farm();
    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.terrain.add",
        &[("name", "water"), ("swatch", "#3366ff")],
    );
    let water = after.terrains.last().expect("added");
    assert_eq!((water.id, water.name.as_str()), (4, "water"));
    assert_eq!(water.swatch.to_u8_array(), [0x33, 0x66, 0xff, 0xff]);
    assert!(!water.background && water.name_by_user);
    assert_eq!(after.next_terrain_id, 5);

    let add = "tileset.terrain.add";
    refused(&mut app, &dir, add, &[("name", "water")], "already named");
    refused(&mut app, &dir, add, &[("name", "7")], "cannot name");
    refused(
        &mut app,
        &dir,
        add,
        &[("name", "x"), ("swatch", "blue")],
        "#rrggbb",
    );
    let id = (app.world().resource::<Assets<Tileset>>().ids().next()).expect("loaded");
    {
        let mut tilesets = app.world_mut().resource_mut::<Assets<Tileset>>();
        tilesets.get_mut(id).expect("loaded").next_terrain_id = 256;
    }
    refused(
        &mut app,
        &dir,
        add,
        &[("name", "lava")],
        "no terrain ids left",
    );
}

#[test]
fn terrain_remove_empties_its_peers_and_drops_its_sets() {
    let (mut app, dir) = farm();
    let (before, after) = edit(
        &mut app,
        &dir,
        "tileset.terrain.remove",
        &[("terrain", "tilemap_packed sand")],
    );
    let sand_tile = tile_of(&before, SAND);
    assert!(after.terrains.iter().all(|t| t.id != SAND));
    assert_eq!(pairs(&after), [(DIRT, GRASS)]);
    assert!(after.tiles.iter().all(|t| !t.peers.contains(&SAND)));
    assert!(peers_of(&after, sand_tile).expect("kept").contains(&0));
    refused(
        &mut app,
        &dir,
        "tileset.terrain.remove",
        &[("terrain", "sand")],
        "no terrain is `sand`",
    );
}

#[test]
fn terrain_rename_sets_a_name_by_hand() {
    let (mut app, dir) = farm();
    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.terrain.rename",
        &[("terrain", "1"), ("name", "grass")],
    );
    let grass = &after.terrains[0];
    assert_eq!((grass.name.as_str(), grass.name_by_user), ("grass", true));
    refused(
        &mut app,
        &dir,
        "tileset.terrain.rename",
        &[("terrain", "2"), ("name", "grass")],
        "already named",
    );
}

#[test]
fn terrain_split_moves_the_listed_tiles_to_a_new_terrain() {
    let (mut app, dir) = farm();
    let tile = tile_of(&on_disk(&dir), SAND);
    let (before, after) = edit(
        &mut app,
        &dir,
        "tileset.terrain.split",
        &[
            ("terrain", "2"),
            ("tiles", &format!("0:{tile}")),
            ("name", "wet sand"),
        ],
    );
    let wet = after.terrains.last().expect("new");
    assert_eq!((wet.id, wet.name.as_str()), (4, "wet sand"));
    let moved = before.tiles.iter().find(|t| t.tile == tile).expect("there");
    let expected = moved.peers.map(|p| if p == SAND { 4 } else { p });
    assert_eq!(peers_of(&after, tile), Some(expected));
    assert!(pairs(&after).contains(&(4, GRASS)));
    refused(
        &mut app,
        &dir,
        "tileset.terrain.split",
        &[("terrain", "3"), ("tiles", "0:0")],
        "holds no terrain 3",
    );
    refused(
        &mut app,
        &dir,
        "tileset.terrain.split",
        &[("terrain", "3"), ("tiles", "999")],
        "out of range",
    );
}

#[test]
fn terrain_merge_folds_one_terrain_into_another() {
    let (mut app, dir) = farm();
    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.terrain.merge",
        &[("from", "3"), ("into", "tilemap_packed sand")],
    );
    assert_eq!(after.terrains.len(), 2);
    assert_eq!(pairs(&after), [(SAND, GRASS)]);
    assert!(after.tiles.iter().all(|t| !t.peers.contains(&DIRT)));
    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.terrain.merge",
        &[("from", "1"), ("into", "2")],
    );
    assert_eq!(after.terrains.len(), 1);
    assert!(
        after.terrains[0].background,
        "the background moves to the kept terrain"
    );
    refused(
        &mut app,
        &dir,
        "tileset.terrain.merge",
        &[("from", "2"), ("into", "2")],
        "cannot border itself",
    );
}

#[test]
fn terrain_set_collision_chooses_the_mode() {
    let (mut app, dir) = farm();
    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.terrain.set_collision",
        &[("terrain", "3"), ("mode", "solid")],
    );
    let dirt = &after.terrains[2];
    assert_eq!(dirt.collision, TerrainCollision::Solid);
    assert!(dirt.collision_by_user);
    refused(
        &mut app,
        &dir,
        "tileset.terrain.set_collision",
        &[("terrain", "3"), ("mode", "sticky")],
        "solid, trigger or passable",
    );
}

#[test]
fn terrain_sets_are_added_and_their_mode_changed() {
    let (mut app, dir) = farm();
    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.terrain_set.add",
        &[("inner", "3"), ("outer", "2"), ("mode", "corners")],
    );
    let set = (after.terrain_sets.iter())
        .find(|s| (s.inner, s.outer) == (DIRT, SAND))
        .expect("added");
    assert_eq!(set.mode, TerrainMatch::Corners);
    let add = "tileset.terrain_set.add";
    refused(
        &mut app,
        &dir,
        add,
        &[("inner", "3"), ("outer", "2")],
        "already exists",
    );
    refused(
        &mut app,
        &dir,
        add,
        &[("inner", "9"), ("outer", "2")],
        "no terrain is `9`",
    );

    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.terrain_set.set",
        &[
            ("inner", "2"),
            ("outer", "1"),
            ("mode", "corners_and_sides"),
        ],
    );
    let set = &after.terrain_sets[0];
    assert_eq!(
        (set.mode, set.mode_by_user),
        (TerrainMatch::CornersAndSides, true)
    );
    refused(
        &mut app,
        &dir,
        "tileset.terrain_set.set",
        &[("inner", "1"), ("outer", "2"), ("mode", "sides")],
        "no terrain set 1 over 2",
    );
}

#[test]
fn set_bits_takes_nine_peers_or_one() {
    let (mut app, dir) = farm();
    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.tile.set_bits",
        &[
            ("tile", "0:5"),
            ("peers", "1,1,1,1,tilemap_packed brown,1,,0,empty"),
        ],
    );
    assert_eq!(peers_of(&after, 5), Some([1, 1, 1, 1, 3, 1, 0, 0, 0]));
    let tile = after.tiles.iter().find(|t| t.tile == 5).expect("set");
    assert_eq!(tile.peers_by_user, [true; 9]);

    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.tile.set_bits",
        &[("tile", "5"), ("peer", "bottom"), ("terrain", "2")],
    );
    assert_eq!(peers_of(&after, 5), Some([1, 1, 1, 1, 3, 1, 0, 2, 0]));

    let auto = tile_of(&on_disk(&dir), SAND);
    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.tile.set_bits",
        &[
            ("tile", &auto.to_string()),
            ("peer", "top_left"),
            ("terrain", "3"),
        ],
    );
    let tile = after.tiles.iter().find(|t| t.tile == auto).expect("set");
    let mut only = [false; 9];
    only[0] = true;
    assert_eq!(tile.peers_by_user, only, "only the painted peer is by hand");

    let bits = "tileset.tile.set_bits";
    refused(
        &mut app,
        &dir,
        bits,
        &[("tile", "5"), ("peers", "1,1,1")],
        "9 terrains, not 3",
    );
    refused(
        &mut app,
        &dir,
        bits,
        &[("tile", "2"), ("peer", "top"), ("terrain", "1")],
        "part of the object at column 2 row 0",
    );
    refused(
        &mut app,
        &dir,
        bits,
        &[("tile", "132"), ("peers", "1")],
        "out of range",
    );
    refused(
        &mut app,
        &dir,
        bits,
        &[("tile", "5"), ("peer", "up")],
        "peer is 0 to 8",
    );
}

#[test]
fn a_failed_write_changes_nothing_and_mints_no_history() {
    use std::os::unix::fs::PermissionsExt as _;

    let (mut app, dir) = farm();
    let assets = dir.path().join("assets");
    let (before, count) = (on_disk(&dir), entries(&app));
    let stored = |app: &App| {
        let tilesets = app.world().resource::<Assets<Tileset>>();
        let id = tilesets.ids().next().expect("loaded");
        tilesets.get(id).cloned().expect("loaded")
    };
    let in_memory = stored(&app);
    std::fs::set_permissions(&assets, std::fs::Permissions::from_mode(0o555)).expect("lock");
    call(
        &mut app,
        "tileset.terrain.rename",
        &[("terrain", "1"), ("name", "grass")],
    );
    std::fs::set_permissions(&assets, std::fs::Permissions::from_mode(0o755)).expect("unlock");
    assert!(
        warnings(&app).join("\n").contains("tileset.terrain.rename"),
        "{:?}",
        warnings(&app)
    );
    assert_eq!(on_disk(&dir), before);
    assert_eq!(stored(&app), in_memory, "the store keeps the old value");
    assert_eq!(entries(&app), count);
}

/// Every terrain, set and bits operator, as an agent drives it.
#[test]
fn the_terrain_operators_run_over_the_remote() {
    let (mut app, dir) = farm();
    let tile = tile_of(&on_disk(&dir), SAND).to_string();
    over_the_remote(
        &mut app,
        vec![
            (
                "tileset.terrain.add",
                json!({ "name": "water", "swatch": "#3366ff" }),
            ),
            (
                "tileset.terrain.rename",
                json!({ "terrain": "water", "name": "pond" }),
            ),
            (
                "tileset.terrain.set_collision",
                json!({ "terrain": "pond", "mode": "trigger" }),
            ),
            (
                "tileset.terrain_set.add",
                json!({ "inner": "pond", "outer": "1", "mode": "corners" }),
            ),
            (
                "tileset.terrain_set.set",
                json!({ "inner": "pond", "outer": "1", "mode": "sides" }),
            ),
            (
                "tileset.tile.set_bits",
                json!({ "tile": "0:11", "peer": "centre", "terrain": "pond" }),
            ),
            (
                "tileset.terrain.split",
                json!({ "terrain": "2", "tiles": &tile, "name": "wet" }),
            ),
            (
                "tileset.terrain.merge",
                json!({ "from": "wet", "into": "2" }),
            ),
            ("tileset.terrain.remove", json!({ "terrain": "pond" })),
        ],
    );
    let tileset = on_disk(&dir);
    assert!((tileset.terrains.iter()).all(|t| t.name != "pond" && t.name != "wet"));
}
