//! Reading and checking the scene in compact form: `jackdaw/scene_summary`
//! rows, the diff since a revision, and the `jackdaw/check` assertions, each
//! reporting what it measured.

use bevy::prelude::*;
use jackdaw::remote::server::{call_operator_handler, check_handler, scene_summary_handler};
use jackdaw_api_internal::operator::OperatorWorldExt as _;
use jackdaw_commands::CommandHistory;
use jackdaw_scene_types::Brush;
use serde_json::{Value, json};

use crate::util;
use crate::util::OperatorResultExt as _;

/// Run one BRP handler, handing back its answer or its refusal.
fn answer<M>(
    app: &mut App,
    handler: impl bevy::ecs::system::IntoSystem<In<Option<Value>>, bevy::remote::BrpResult, M> + 'static,
    params: Value,
) -> bevy::remote::BrpResult {
    app.world_mut()
        .run_system_cached_with(handler, Some(params))
        .expect("the handler ran")
}

#[track_caller]
fn summary(app: &mut App, params: Value) -> Value {
    answer(app, scene_summary_handler, params)
        .unwrap_or_else(|err| panic!("the summary refused: {}", err.message))
}

/// The results of one `jackdaw/check` call holding `assertions`.
#[track_caller]
fn check(app: &mut App, assertions: Value) -> Vec<Value> {
    let answer = answer(app, check_handler, json!({ "assertions": assertions }))
        .unwrap_or_else(|err| panic!("the check refused: {}", err.message));
    answer["results"].as_array().expect("results").clone()
}

fn summary_scene() -> App {
    let mut app = util::editor_test_app();
    app.world_mut()
        .operator("scene.new")
        .call()
        .expect("scene.new dispatches")
        .assert_finished();
    app.update();
    app
}

/// Add a cube brush through the remote surface and hand back its id.
#[track_caller]
fn add_cube(app: &mut App, name: &str, size: [f32; 3], pos: [f32; 3]) -> u64 {
    let outcome = answer(
        app,
        call_operator_handler,
        json!({
            "id": "entity.add.cube",
            "params": { "size": size, "pos": pos, "name": name },
        }),
    )
    .unwrap_or_else(|err| panic!("entity.add.cube refused: {}", err.message));
    assert_eq!(outcome["result"], json!("finished"), "{outcome}");
    app.update();
    outcome["entities"][0].as_u64().expect("the new cube's id")
}

fn undo_depth(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

/// A scene with one terrain whose ground stands five metres up over the
/// cells from the origin to (4, 4).
fn summary_scene_with_ground() -> App {
    let mut app = summary_scene();
    app.world_mut()
        .operator("entity.add.terrain")
        .call()
        .expect("entity.add.terrain dispatches")
        .assert_finished();
    app.update();
    let mut query = app
        .world_mut()
        .query_filtered::<Entity, With<jackdaw_scene_types::Terrain>>();
    let terrain = query
        .iter(app.world())
        .next()
        .expect("entity.add.terrain authored a terrain");
    app.world_mut()
        .entity_mut(terrain)
        .insert((Name::new("Ground"), Transform::IDENTITY));

    let data_path = "scene.terrain-0.jdterrain";
    let mut document = jackdaw_terrain::RegionTerrainData::default();
    for z in 0..4 {
        for x in 0..4 {
            document.regions.set_height(x, z, 5.0);
        }
    }
    let mut store = app
        .world_mut()
        .resource_mut::<jackdaw::terrain::TerrainDataStore>();
    store.insert(data_path, document);
    store.set_grid(data_path, jackdaw_terrain::sidecar::GridGeometry::DEFAULT);
    app.world_mut()
        .get_mut::<jackdaw_scene_types::Terrain>(terrain)
        .expect("a terrain")
        .data_path = data_path.to_string();
    app.update();
    app
}

#[test]
fn a_known_cube_summarises_to_exact_rounded_rows() {
    let mut app = summary_scene();
    let id = add_cube(&mut app, "Box", [2.0, 1.0, 3.0], [1.0, 2.0, 3.0]);
    let depth = undo_depth(&app);

    let rows = summary(
        &mut app,
        json!({
            "fields": ["name", "kind", "pos", "size", "bounds", "faces", "verts", "modifiers"],
            "filter": { "name_prefix": "Bo" },
        }),
    );
    assert_eq!(
        rows["columns"],
        json!([
            "id",
            "name",
            "kind",
            "pos",
            "size",
            "bounds",
            "faces",
            "verts",
            "modifiers"
        ]),
        "{rows}"
    );
    assert_eq!(
        rows["rows"],
        json!([[
            id,
            "Box",
            "brush",
            [1.0, 2.0, 3.0],
            [2.0, 1.0, 3.0],
            [[0.0, 1.5, 1.5], [2.0, 2.5, 4.5]],
            6,
            8,
            null
        ]]),
        "{rows}"
    );
    assert!(rows["revision"].is_u64(), "{rows}");
    assert_eq!(undo_depth(&app), depth, "reading the scene is no undo step");
}

#[test]
fn the_default_summary_names_every_scene_node_with_its_parent() {
    let mut app = summary_scene();
    let outer = add_cube(&mut app, "Outer", [1.0, 1.0, 1.0], [0.0, 0.0, 0.0]);
    let outcome = answer(
        &mut app,
        call_operator_handler,
        json!({
            "id": "entity.add.cube",
            "params": { "name": "Inner", "parent": outer, "pos": [0.0, 2.0, 0.0] },
        }),
    )
    .expect("the child cube");
    app.update();
    let inner = outcome["entities"][0].as_u64().expect("an id");

    let rows = summary(&mut app, json!({}));
    assert_eq!(
        rows["columns"],
        json!(["id", "name", "parent", "kind", "pos", "size"]),
        "{rows}"
    );
    let all = rows["rows"].as_array().expect("rows");
    let row_of = |id: u64| {
        all.iter()
            .find(|row| row[0] == json!(id))
            .unwrap_or_else(|| panic!("no row for {id}: {rows}"))
            .clone()
    };
    assert_eq!(row_of(outer)[2], Value::Null, "{rows}");
    assert_eq!(row_of(inner)[2], json!(outer), "{rows}");
    assert_eq!(row_of(inner)[4], json!([0.0, 2.0, 0.0]), "{rows}");

    let under = summary(&mut app, json!({ "root": outer, "depth": 0 }));
    assert_eq!(
        under["rows"].as_array().map(Vec::len),
        Some(1),
        "depth 0 is the root alone: {under}"
    );
}

#[test]
fn a_diff_since_a_revision_holds_one_added_row_then_one_removed_row() {
    let mut app = summary_scene();
    add_cube(&mut app, "Wall", [1.0, 1.0, 1.0], [0.0, 0.0, 0.0]);
    let before = summary(&mut app, json!({}));
    let start = before["revision"].as_u64().expect("a revision");

    let roof = add_cube(&mut app, "Roof", [1.0, 0.2, 1.0], [0.0, 0.6, 0.0]);
    let added = summary(&mut app, json!({ "since_revision": start }));
    assert_eq!(added["added"].as_array().map(Vec::len), Some(1), "{added}");
    assert_eq!(added["added"][0][0], json!(roof), "{added}");
    assert_eq!(added["changed"], json!([]), "{added}");
    assert_eq!(added["removed"], json!([]), "{added}");
    let after_add = added["revision"].as_u64().expect("a revision");
    assert!(after_add > start, "{added}");

    app.world_mut()
        .operator("history.undo")
        .call()
        .expect("history.undo dispatches")
        .assert_finished();
    app.update();
    let removed = summary(&mut app, json!({ "since_revision": after_add }));
    assert_eq!(removed["added"], json!([]), "{removed}");
    assert_eq!(removed["removed"], json!([roof]), "{removed}");
    assert!(
        removed["revision"].as_u64().expect("a revision") > after_add,
        "{removed}"
    );

    let none = summary(
        &mut app,
        json!({ "since_revision": removed["revision"].clone() }),
    );
    assert_eq!(none["added"], json!([]), "{none}");
    assert_eq!(none["changed"], json!([]), "{none}");
    assert_eq!(none["removed"], json!([]), "{none}");
}

#[test]
fn an_unknown_field_entity_or_revision_is_refused() {
    let mut app = summary_scene();
    add_cube(&mut app, "Box", [1.0, 1.0, 1.0], [0.0, 0.0, 0.0]);
    for params in [
        json!({ "fields": ["name", "colour"] }),
        json!({ "root": 999_999_999u64 }),
        json!({ "root": "Nobody" }),
        json!({ "since_revision": 999_999_999u64 }),
    ] {
        let refused = answer(&mut app, scene_summary_handler, params.clone());
        let err = refused.expect_err(&format!("{params} was answered"));
        assert_eq!(
            err.code,
            bevy::remote::error_codes::INVALID_PARAMS,
            "{params}: {}",
            err.message
        );
    }

    let refused = answer(
        &mut app,
        check_handler,
        json!({ "assertions": [{ "bounds": { "entity": "Nobody" } }] }),
    );
    assert!(refused.is_err(), "an unknown entity is refused");
    let refused = answer(
        &mut app,
        check_handler,
        json!({ "assertions": [{ "touches": { "entity": "Box" } }] }),
    );
    assert!(refused.is_err(), "an unknown assertion is refused");
}

#[test]
fn rests_on_measures_the_gap_to_the_top_below() {
    let mut app = summary_scene();
    add_cube(&mut app, "Walls", [2.0, 2.0, 2.0], [0.0, 1.0, 0.0]);
    add_cube(&mut app, "Roof", [2.0, 0.5, 2.0], [0.0, 2.25, 0.0]);
    add_cube(&mut app, "Hat", [1.0, 0.5, 1.0], [0.0, 2.3, 0.0]);
    add_cube(&mut app, "Aside", [1.0, 1.0, 1.0], [5.0, 2.5, 0.0]);
    let depth = undo_depth(&app);

    let results = check(
        &mut app,
        json!([
            { "rests_on": { "entity": "Roof", "on": "Walls" } },
            { "rests_on": { "entity": "Hat", "on": "Walls", "tolerance": 0.01 } },
            { "rests_on": { "entity": "Hat", "on": "Walls", "tolerance": 0.1 } },
            { "rests_on": { "entity": "Aside", "on": "Walls" } },
        ]),
    );
    assert_eq!(results[0]["pass"], json!(true), "{results:?}");
    assert_eq!(results[0]["gap"], json!(0.0), "{results:?}");
    assert_eq!(results[1]["pass"], json!(false), "{results:?}");
    assert_eq!(results[1]["gap"], json!(0.05), "{results:?}");
    assert_eq!(results[2]["pass"], json!(true), "{results:?}");
    assert_eq!(results[3]["pass"], json!(false), "{results:?}");
    assert_eq!(results[3]["overlap_xz"], json!(false), "{results:?}");
    assert_eq!(
        undo_depth(&app),
        depth,
        "checking the scene is no undo step"
    );
}

#[test]
fn rests_on_a_terrain_measures_the_gap_to_the_ground_under_it() {
    let mut app = summary_scene_with_ground();
    add_cube(&mut app, "Hut", [1.0, 1.0, 1.0], [1.5, 5.5, 1.5]);
    add_cube(&mut app, "Kite", [1.0, 1.0, 1.0], [1.5, 6.0, 1.5]);

    let results = check(
        &mut app,
        json!([
            { "rests_on": { "entity": "Hut", "on": "Ground" } },
            { "rests_on": { "entity": "Kite", "on": "Ground" } },
        ]),
    );
    assert_eq!(results[0]["pass"], json!(true), "{results:?}");
    assert_eq!(results[0]["gap"], json!(0.0), "{results:?}");
    assert_eq!(results[1]["pass"], json!(false), "{results:?}");
    assert_eq!(results[1]["gap"], json!(0.5), "{results:?}");
}

#[test]
fn inside_reports_how_far_a_part_sticks_out() {
    let mut app = summary_scene();
    add_cube(&mut app, "Room", [4.0, 4.0, 4.0], [0.0, 0.0, 0.0]);
    add_cube(&mut app, "Table", [1.0, 1.0, 1.0], [0.0, 0.0, 0.0]);
    add_cube(&mut app, "Beam", [1.0, 1.0, 1.0], [2.0, 0.0, 0.0]);

    let results = check(
        &mut app,
        json!([
            { "inside": { "entity": "Table", "of": "Room" } },
            { "inside": { "entity": "Beam", "of": "Room" } },
        ]),
    );
    assert_eq!(results[0]["pass"], json!(true), "{results:?}");
    assert_eq!(results[0]["outside"], json!(0.0), "{results:?}");
    assert_eq!(results[1]["pass"], json!(false), "{results:?}");
    assert_eq!(results[1]["outside"], json!(0.5), "{results:?}");
}

#[test]
fn no_overlap_reports_the_deepest_overlap() {
    let mut app = summary_scene();
    add_cube(&mut app, "A", [1.0, 1.0, 1.0], [0.0, 0.0, 0.0]);
    add_cube(&mut app, "B", [1.0, 1.0, 1.0], [1.0, 0.0, 0.0]);
    add_cube(&mut app, "C", [1.0, 1.0, 1.0], [0.5, 0.0, 0.0]);

    let results = check(
        &mut app,
        json!([
            { "no_overlap": { "entities": ["A", "B"] } },
            { "no_overlap": { "entities": ["A", "B", "C"], "tolerance": 0.01 } },
        ]),
    );
    assert_eq!(
        results[0]["pass"],
        json!(true),
        "touching is not overlapping"
    );
    assert_eq!(results[0]["depth"], json!(0.0), "{results:?}");
    assert_eq!(results[1]["pass"], json!(false), "{results:?}");
    assert_eq!(results[1]["depth"], json!(0.5), "{results:?}");
    assert_eq!(
        results[1]["pairs"].as_array().map(Vec::len),
        Some(2),
        "{results:?}"
    );
}

#[test]
fn bounds_reports_the_measured_box() {
    let mut app = summary_scene();
    add_cube(&mut app, "Box", [2.0, 1.0, 3.0], [1.0, 2.0, 3.0]);

    let results = check(
        &mut app,
        json!([
            { "bounds": { "entity": "Box", "min": [0.0, 1.0, 1.0], "max": [2.0, 3.0, 5.0] } },
            { "bounds": { "entity": "Box", "max": [1.0, 3.0, 5.0] } },
        ]),
    );
    assert_eq!(results[0]["pass"], json!(true), "{results:?}");
    assert_eq!(
        results[0]["bounds"],
        json!([[0.0, 1.5, 1.5], [2.0, 2.5, 4.5]]),
        "{results:?}"
    );
    assert_eq!(results[1]["pass"], json!(false), "{results:?}");
}

#[test]
fn count_reports_how_many_nodes_matched() {
    let mut app = summary_scene();
    add_cube(&mut app, "Fence 1", [1.0, 1.0, 1.0], [0.0, 0.0, 0.0]);
    add_cube(&mut app, "Fence 2", [1.0, 1.0, 1.0], [2.0, 0.0, 0.0]);
    add_cube(&mut app, "Gate", [1.0, 1.0, 1.0], [4.0, 0.0, 0.0]);

    let results = check(
        &mut app,
        json!([
            { "count": { "filter": { "name_prefix": "Fence" }, "eq": 2 } },
            { "count": { "filter": { "kind": "brush" }, "min": 4 } },
            { "count": { "filter": { "kind": "brush" }, "max": 3 } },
        ]),
    );
    assert_eq!(results[0]["pass"], json!(true), "{results:?}");
    assert_eq!(results[0]["count"], json!(2), "{results:?}");
    assert_eq!(results[1]["pass"], json!(false), "{results:?}");
    assert_eq!(results[1]["count"], json!(3), "{results:?}");
    assert_eq!(results[2]["pass"], json!(true), "{results:?}");
}

#[test]
fn watertight_names_the_open_edges() {
    let mut app = summary_scene();
    add_cube(&mut app, "Closed", [1.0, 1.0, 1.0], [0.0, 0.0, 0.0]);
    let open = add_cube(&mut app, "Open", [2.0, 2.0, 2.0], [0.0, 10.0, 0.0]);
    let open = Entity::from_bits(open);
    app.world_mut()
        .get_mut::<Brush>(open)
        .expect("a brush")
        .topology
        .polygons
        .pop();

    let results = check(
        &mut app,
        json!([
            { "watertight": { "entity": "Closed" } },
            { "watertight": { "entity": "Open" } },
        ]),
    );
    assert_eq!(results[0]["pass"], json!(true), "{results:?}");
    assert_eq!(results[0]["open_edges"], json!(0), "{results:?}");
    assert_eq!(results[1]["pass"], json!(false), "{results:?}");
    assert_eq!(results[1]["open_edges"], json!(4), "{results:?}");
    let at = results[1]["at"].as_array().expect("open edge positions");
    assert!(!at.is_empty() && at.len() <= 4, "{results:?}");
    assert!(
        at.iter()
            .all(|point| point[1].as_f64().is_some_and(|y| y > 8.0)),
        "the positions are in world space: {results:?}"
    );
}

/// A node with nothing of its own to measure, standing at `at`.
fn add_group(app: &mut App, name: &str, at: Vec3) -> Entity {
    let entity = app
        .world_mut()
        .spawn((
            Name::new(name.to_string()),
            Transform::from_translation(at),
            Visibility::default(),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    app.update();
    entity
}

#[test]
fn a_group_is_as_big_as_what_it_holds() {
    let mut app = summary_scene();
    let group = add_group(&mut app, "Group", Vec3::new(10.0, 0.0, 0.0));
    for (name, x) in [("Left", 0.0), ("Right", 2.0)] {
        let outcome = answer(
            &mut app,
            call_operator_handler,
            json!({
                "id": "entity.add.cube",
                "params": { "name": name, "parent": group.to_bits(), "pos": [x, 0.0, 0.0] },
            }),
        )
        .expect("a child cube");
        assert_eq!(outcome["result"], json!("finished"), "{outcome}");
        app.update();
    }

    let rows = summary(
        &mut app,
        json!({ "fields": ["bounds"], "root": "Group", "depth": 0 }),
    );
    assert_eq!(
        rows["rows"],
        json!([[group.to_bits(), [[9.5, -0.5, -0.5], [12.5, 0.5, 0.5]]]]),
        "{rows}"
    );
}

#[test]
fn a_mirrored_brush_is_measured_with_its_mirror() {
    use jackdaw_geometry::{MeshMirror, Modifier, ModifierEntry, ModifierStack};

    let mut app = summary_scene();
    let id = add_cube(&mut app, "Wing", [1.0, 1.0, 1.0], [0.0, 0.0, 0.0]);
    app.world_mut()
        .entity_mut(Entity::from_bits(id))
        .insert(ModifierStack {
            modifiers: vec![ModifierEntry::new(Modifier::Mirror(MeshMirror {
                offset: Vec3::new(1.0, 0.0, 0.0),
                ..default()
            }))],
        });
    app.update();
    app.update();

    let rows = summary(
        &mut app,
        json!({ "fields": ["bounds", "modifiers"], "root": id, "depth": 0 }),
    );
    assert_eq!(
        rows["rows"],
        json!([[id, [[-0.5, -0.5, -0.5], [2.5, 0.5, 0.5]], ["mirror"]]]),
        "{rows}"
    );
}

#[test]
fn an_assertion_that_cannot_measure_fails_alone_with_a_reason() {
    let mut app = summary_scene();
    add_cube(&mut app, "Box", [1.0, 1.0, 1.0], [0.0, 0.0, 0.0]);
    add_group(&mut app, "Marker", Vec3::ZERO);

    let results = check(
        &mut app,
        json!([
            { "bounds": { "entity": "Marker", "max": [1.0, 1.0, 1.0] } },
            { "watertight": { "entity": "Marker" } },
            { "inside": { "entity": "Box", "of": "Marker" } },
            { "bounds": { "entity": "Box", "max": [1.0, 1.0, 1.0] } },
        ]),
    );
    for failed in &results[..3] {
        assert_eq!(failed["pass"], json!(false), "{results:?}");
        assert!(failed["reason"].is_string(), "{results:?}");
    }
    assert_eq!(results[3]["pass"], json!(true), "{results:?}");
}

#[test]
fn watertight_fails_a_brush_it_cannot_judge() {
    let mut app = summary_scene();
    let empty = Entity::from_bits(add_cube(&mut app, "Empty", [1.0; 3], [0.0; 3]));
    let broken = Entity::from_bits(add_cube(&mut app, "Broken", [1.0; 3], [3.0, 0.0, 0.0]));
    app.world_mut()
        .get_mut::<Brush>(empty)
        .expect("a brush")
        .topology
        .polygons
        .clear();
    app.world_mut()
        .get_mut::<Brush>(broken)
        .expect("a brush")
        .topology
        .polygons[0]
        .loop_start = 9_999;

    let results = check(
        &mut app,
        json!([
            { "watertight": { "entity": "Empty" } },
            { "watertight": { "entity": "Broken" } },
        ]),
    );
    for failed in &results {
        assert_eq!(failed["pass"], json!(false), "{results:?}");
        assert!(failed["reason"].is_string(), "{results:?}");
    }
}

#[test]
fn no_overlap_counts_a_repeated_entity_once() {
    let mut app = summary_scene();
    add_cube(&mut app, "A", [1.0; 3], [0.0; 3]);
    add_cube(&mut app, "B", [1.0; 3], [1.0, 0.0, 0.0]);
    let a = app
        .world_mut()
        .query::<(Entity, &Name)>()
        .iter(app.world())
        .find(|(_, name)| name.as_str() == "A")
        .map(|(entity, _)| entity.to_bits())
        .expect("A");

    let results = check(
        &mut app,
        json!([{ "no_overlap": { "entities": ["A", a, "B"] } }]),
    );
    assert_eq!(results[0]["pass"], json!(true), "{results:?}");

    let refused = answer(
        &mut app,
        check_handler,
        json!({ "assertions": [{ "no_overlap": { "entities": ["A", a] } }] }),
    );
    assert!(refused.is_err(), "one entity twice is not two entities");
}

#[test]
fn a_depth_that_is_not_a_whole_number_is_refused() {
    let mut app = summary_scene();
    for depth in [json!(-1), json!(1.5), json!("2"), json!(u64::MAX)] {
        let refused = answer(&mut app, scene_summary_handler, json!({ "depth": depth }));
        assert!(refused.is_err(), "depth {depth} was answered");
    }
}

/// An edit that bypasses history leaves the revision where it was; a second
/// read at that revision must not move the baseline a diff is measured from.
#[test]
fn a_later_read_at_the_same_revision_keeps_the_first_baseline() {
    let mut app = summary_scene();
    let id = add_cube(&mut app, "Crate", [1.0; 3], [0.0; 3]);
    let first = summary(&mut app, json!({ "fields": ["pos"] }));
    let revision = first["revision"].clone();

    app.world_mut()
        .get_mut::<Transform>(Entity::from_bits(id))
        .expect("a transform")
        .translation
        .x = 4.0;
    app.update();
    let second = summary(&mut app, json!({ "fields": ["pos"] }));
    assert_eq!(second["revision"], revision, "{second}");

    let diff = summary(
        &mut app,
        json!({ "fields": ["pos"], "since_revision": revision }),
    );
    assert_eq!(diff["changed"], json!([[id, [4.0, 0.0, 0.0]]]), "{diff}");
}
