//! The terrain, set and bits operators over an auto-imported overworld
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

pub(crate) const FILE: &str = "overworld.tileset.bsn";
pub(crate) const GRASS: u8 = 1;
pub(crate) const GROUND: u8 = 2;
pub(crate) const WATER: u8 = 11;

pub(crate) fn overworld() -> (App, tempfile::TempDir) {
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
    let (mut app, dir) = overworld();
    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.terrain.add",
        &[("name", "marsh"), ("swatch", "#3366ff")],
    );
    let marsh = after.terrains.last().expect("added");
    assert_eq!((marsh.id, marsh.name.as_str()), (13, "marsh"));
    assert_eq!(marsh.swatch.to_u8_array(), [0x33, 0x66, 0xff, 0xff]);
    assert!(!marsh.background && marsh.name_by_user);
    assert_eq!(after.next_terrain_id, 14);

    let add = "tileset.terrain.add";
    refused(&mut app, &dir, add, &[("name", "marsh")], "already named");
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
fn new_terrains_take_the_first_free_palette_hue() {
    let (mut app, dir) = project_app();
    import(&mut app, &[("paths", SHEET)]);
    for name in ["grass", "dirt"] {
        call(&mut app, "tileset.terrain.add", &[("name", name)]);
    }
    let swatches = |dir: &tempfile::TempDir| -> Vec<[u8; 4]> {
        (on_disk(dir).terrains.iter())
            .map(|t| t.swatch.to_u8_array())
            .collect()
    };
    let first = swatches(&dir);
    assert_ne!(first[0], first[1]);
    assert!(first.iter().all(|s| s[..3] != [0x80; 3]), "{first:?}");
    call(&mut app, "tileset.terrain.remove", &[("terrain", "grass")]);
    call(&mut app, "tileset.terrain.add", &[("name", "sand")]);
    assert_eq!(
        swatches(&dir),
        [first[1], first[0]],
        "the freed hue comes back"
    );
    assert!(warnings(&app).is_empty(), "{:?}", warnings(&app));
}

#[test]
fn terrain_remove_empties_its_peers_and_drops_its_sets() {
    let (mut app, dir) = overworld();
    let (before, after) = edit(
        &mut app,
        &dir,
        "tileset.terrain.remove",
        &[("terrain", "grass 2")],
    );
    let ground_tile = tile_of(&before, GROUND);
    assert!(after.terrains.iter().all(|t| t.id != GROUND));
    assert_eq!(
        pairs(&after),
        [(5, 6), (WATER, GRASS), (WATER, 9), (12, WATER)]
    );
    assert!(after.tiles.iter().all(|t| !t.peers.contains(&GROUND)));
    assert!(peers_of(&after, ground_tile).expect("kept").contains(&0));
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
    let (mut app, dir) = overworld();
    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.terrain.rename",
        &[("terrain", "1"), ("name", "meadow")],
    );
    let grass = &after.terrains[0];
    assert_eq!((grass.name.as_str(), grass.name_by_user), ("meadow", true));
    refused(
        &mut app,
        &dir,
        "tileset.terrain.rename",
        &[("terrain", "2"), ("name", "meadow")],
        "already named",
    );
}

#[test]
fn terrain_split_moves_the_listed_tiles_to_a_new_terrain() {
    let (mut app, dir) = overworld();
    let tile = tile_of(&on_disk(&dir), GROUND);
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
    assert_eq!((wet.id, wet.name.as_str()), (13, "wet sand"));
    let moved = before.tiles.iter().find(|t| t.tile == tile).expect("there");
    let expected = moved.peers.map(|p| if p == GROUND { 13 } else { p });
    assert_eq!(peers_of(&after, tile), Some(expected));
    assert!(pairs(&after).contains(&(13, GRASS)));
    refused(
        &mut app,
        &dir,
        "tileset.terrain.split",
        &[("terrain", "11"), ("tiles", "0:0")],
        "holds no terrain 11",
    );
    refused(
        &mut app,
        &dir,
        "tileset.terrain.split",
        &[("terrain", "11"), ("tiles", "5000")],
        "out of range",
    );
}

#[test]
fn terrain_merge_folds_one_terrain_into_another() {
    let (mut app, dir) = overworld();
    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.terrain.merge",
        &[("from", "11"), ("into", "grass 2")],
    );
    assert_eq!(after.terrains.len(), 7);
    assert_eq!(
        pairs(&after),
        [
            (GROUND, GRASS),
            (GROUND, 9),
            (5, 6),
            (9, GROUND),
            (12, GROUND)
        ]
    );
    assert!(after.tiles.iter().all(|t| !t.peers.contains(&WATER)));
    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.terrain.merge",
        &[("from", "6"), ("into", "5")],
    );
    assert_eq!(after.terrains.len(), 6);
    assert!(
        (after.terrains.iter()).any(|t| t.id == 5 && t.background),
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
    let (mut app, dir) = overworld();
    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.terrain.set_collision",
        &[("terrain", "11"), ("mode", "solid")],
    );
    let water = (after.terrains.iter())
        .find(|t| t.id == WATER)
        .expect("water");
    assert_eq!(water.collision, TerrainCollision::Solid);
    assert!(water.collision_by_user);
    refused(
        &mut app,
        &dir,
        "tileset.terrain.set_collision",
        &[("terrain", "11"), ("mode", "sticky")],
        "solid, trigger or passable",
    );
}

#[test]
fn terrain_sets_are_added_and_their_mode_changed() {
    let (mut app, dir) = overworld();
    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.terrain_set.add",
        &[("inner", "11"), ("outer", "2"), ("mode", "corners")],
    );
    let set = (after.terrain_sets.iter())
        .find(|s| (s.inner, s.outer) == (WATER, GROUND))
        .expect("added");
    assert_eq!(set.mode, TerrainMatch::Corners);
    let add = "tileset.terrain_set.add";
    refused(
        &mut app,
        &dir,
        add,
        &[("inner", "11"), ("outer", "2")],
        "already exists",
    );
    refused(
        &mut app,
        &dir,
        add,
        &[("inner", "4"), ("outer", "2")],
        "no terrain is `4`",
    );

    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.terrain_set.set",
        &[("inner", "2"), ("outer", "1"), ("mode", "sides")],
    );
    let set = &after.terrain_sets[0];
    assert_eq!((set.mode, set.mode_by_user), (TerrainMatch::Sides, true));
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
    let (mut app, dir) = overworld();
    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.tile.set_bits",
        &[("tile", "0:5"), ("peers", "1,1,1,1,water,1,,0,empty")],
    );
    assert_eq!(peers_of(&after, 5), Some([1, 1, 1, 1, 11, 1, 0, 0, 0]));
    let tile = after.tiles.iter().find(|t| t.tile == 5).expect("set");
    assert_eq!(tile.peers_by_user, [true; 9]);

    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.tile.set_bits",
        &[("tile", "5"), ("peer", "bottom"), ("terrain", "2")],
    );
    assert_eq!(peers_of(&after, 5), Some([1, 1, 1, 1, 11, 1, 0, 2, 0]));

    let auto = tile_of(&on_disk(&dir), GROUND);
    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.tile.set_bits",
        &[
            ("tile", &auto.to_string()),
            ("peer", "top_left"),
            ("terrain", "11"),
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
        &[("tile", "108"), ("peer", "top"), ("terrain", "1")],
        "part of the object at column 0 row 4",
    );
    refused(
        &mut app,
        &dir,
        bits,
        &[("tile", "1026"), ("peers", "1")],
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

    let (mut app, dir) = overworld();
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
        &[("terrain", "1"), ("name", "meadow")],
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
    let (mut app, dir) = overworld();
    let tile = tile_of(&on_disk(&dir), GROUND).to_string();
    over_the_remote(
        &mut app,
        vec![
            (
                "tileset.terrain.add",
                json!({ "name": "marsh", "swatch": "#3366ff", "icon": "0:5" }),
            ),
            (
                "tileset.terrain.rename",
                json!({ "terrain": "marsh", "name": "pond" }),
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

#[test]
fn terrain_add_takes_an_icon_tile_that_survives_a_reload() {
    let (mut app, dir) = project_app();
    import(&mut app, &[("paths", SHEET)]);
    call(&mut app, "tileset.terrain.add", &[("name", "grass")]);
    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.terrain.add",
        &[("name", "dirt"), ("icon", "0:5")],
    );
    let icons: Vec<_> = (after.terrains.iter()).map(|t| t.icon).collect();
    assert_eq!(icons, [None, Some((0, 5))]);
    let layer = after.layer(0, 7).expect("a tile").to_string();
    call(
        &mut app,
        "tileset.terrain.add",
        &[("name", "water"), ("icon", &layer)],
    );
    assert_eq!(
        on_disk(&dir).terrains[2].icon,
        Some((0, 7)),
        "a packed layer"
    );
    call(
        &mut app,
        "tileset.terrain_set.add",
        &[("inner", "dirt"), ("outer", "grass")],
    );
    assert_eq!(
        on_disk(&dir).terrains[1].icon,
        Some((0, 5)),
        "kept by an edit"
    );

    refused(
        &mut app,
        &dir,
        "tileset.terrain.add",
        &[("name", "sand"), ("icon", "0:99999")],
        "out of range",
    );
    drop(app);
    let reloaded = read_tileset(&asset_file(&dir, FILE));
    assert_eq!(reloaded.terrains[1].icon, Some((0, 5)));
}

/// `text` without its `field: value` entries, as a file written before the
/// field existed.
fn without_field(text: &str, field: &str) -> String {
    let key = format!("{field}:");
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(&key) {
        out.push_str(&rest[..at]);
        let mut depth = 0i32;
        let mut end = at + key.len();
        for (offset, c) in rest[end..].char_indices() {
            match c {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' if depth > 0 => depth -= 1,
                ')' | ']' | '}' | ',' | '\n' if depth == 0 => {
                    end += offset + usize::from(c == ',');
                    break;
                }
                _ => {}
            }
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

#[test]
fn a_tileset_from_before_icons_loads_without_them() {
    let (mut app, dir) = project_app();
    import(&mut app, &[("paths", SHEET)]);
    call(
        &mut app,
        "tileset.terrain.add",
        &[("name", "grass"), ("icon", "0:5")],
    );
    drop(app);
    let file = asset_file(&dir, FILE);
    let text = std::fs::read_to_string(&file).expect("written");
    assert!(text.contains("icon:"), "{text}");
    let old = without_field(&text, "icon");
    assert!(!old.contains("icon"), "{old}");
    std::fs::write(&file, old).expect("rewritten");
    let loaded = read_tileset(&file);
    assert_eq!(loaded.terrains.len(), 1);
    assert_eq!(
        (loaded.terrains[0].name.as_str(), loaded.terrains[0].icon),
        ("grass", None)
    );
}

#[test]
fn an_icon_belongs_to_one_terrain_and_can_be_changed() {
    let (mut app, dir) = project_app();
    import(&mut app, &[("paths", SHEET)]);
    call(
        &mut app,
        "tileset.terrain.add",
        &[("name", "grass"), ("icon", "0:5")],
    );
    refused(
        &mut app,
        &dir,
        "tileset.terrain.add",
        &[("name", "dirt"), ("icon", "0:5")],
        "tile 0:5 is already the icon of `grass`",
    );
    call(&mut app, "tileset.terrain.add", &[("name", "dirt")]);
    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.terrain.set_icon",
        &[("terrain", "dirt"), ("icon", "0:7")],
    );
    assert_eq!(after.terrains[1].icon, Some((0, 7)));
    refused(
        &mut app,
        &dir,
        "tileset.terrain.set_icon",
        &[("terrain", "dirt"), ("icon", "0:5")],
        "already the icon of `grass`",
    );
    let (_, after) = edit(
        &mut app,
        &dir,
        "tileset.terrain.set_icon",
        &[("terrain", "grass"), ("icon", "")],
    );
    assert_eq!(after.terrains[0].icon, None, "an empty icon clears it");
    refused(
        &mut app,
        &dir,
        "tileset.terrain.set_icon",
        &[("terrain", "sand"), ("icon", "0:1")],
        "no terrain is `sand`",
    );
}

#[test]
fn renaming_a_terrain_to_its_own_name_changes_nothing() {
    let (mut app, dir) = project_app();
    import(&mut app, &[("paths", SHEET)]);
    call(&mut app, "tileset.terrain.add", &[("name", "grass")]);
    let (before, count) = (on_disk(&dir), entries(&app));
    call(
        &mut app,
        "tileset.terrain.rename",
        &[("terrain", "grass"), ("name", "grass")],
    );
    assert!(warnings(&app).is_empty(), "{:?}", warnings(&app));
    assert_eq!((on_disk(&dir), entries(&app)), (before, count));
}

#[test]
fn quotes_and_control_characters_cannot_name_a_terrain() {
    let (mut app, dir) = project_app();
    import(&mut app, &[("paths", SHEET)]);
    for name in ["say \"hi\"", "tab\there", "line\nbreak"] {
        refused(
            &mut app,
            &dir,
            "tileset.terrain.add",
            &[("name", name)],
            "cannot name a terrain",
        );
    }
}
