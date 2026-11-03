//! `brush.csg_subtract` and `brush.csg_intersect` on named operands: only the
//! target changes, the call reports what it removed and created, refusals and
//! no-ops leave the scene alone, and the result undoes, redoes and saves.

use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::prelude::*;
use jackdaw::commands::CommandHistory;
use jackdaw::material_browser::MaterialRegistry;
use jackdaw::remote::server::{batch_handler, call_operator_handler};
use jackdaw::selection::{Selected, Selection};
use jackdaw_api::prelude::*;
use jackdaw_scene_types::Brush;
use serde_json::{Value, json};

const EPS: f32 = 1e-3;

pub(super) fn csg_app() -> App {
    let mut app = util::editor_test_app();
    app.world_mut()
        .operator("scene.new")
        .call()
        .expect("scene.new dispatches")
        .assert_finished();
    app.update();
    app
}

pub(super) fn call(app: &mut App, id: &str, params: Value) -> Value {
    let outcome = app
        .world_mut()
        .run_system_cached_with(
            call_operator_handler,
            Some(json!({ "id": id, "params": params, "verbose": true })),
        )
        .expect("the handler ran")
        .unwrap_or_else(|err| panic!("{id} refused: {}", err.message));
    app.update();
    outcome
}

pub(super) fn entity_of(value: &Value) -> Entity {
    Entity::try_from_bits(value.as_u64().expect("an entity id")).expect("a valid entity id")
}

/// A cube brush of `size` centred at `pos`, turned by `rot` degrees.
pub(super) fn cube(
    app: &mut App,
    name: &str,
    size: [f32; 3],
    pos: [f32; 3],
    rot: [f32; 3],
) -> Entity {
    let outcome = call(
        app,
        "entity.add.cube",
        json!({ "name": name, "size": size, "pos": pos, "rot": rot }),
    );
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    entity_of(&outcome["entities"][0])
}

pub(super) fn clear_selection(app: &mut App) {
    let selected: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<Selected>>()
        .iter(app.world())
        .collect();
    for entity in selected {
        app.world_mut().entity_mut(entity).remove::<Selected>();
    }
    app.world_mut().resource_mut::<Selection>().entities.clear();
    app.update();
}

/// The world-space box around a brush's corners.
pub(super) fn measured(app: &App, entity: Entity) -> (Vec3, Vec3) {
    let global = app
        .world()
        .get::<GlobalTransform>(entity)
        .expect("a transform");
    let brush = app.world().get::<Brush>(entity).expect("a brush");
    brush
        .topology
        .vertices
        .iter()
        .map(|vert| global.transform_point(vert.position))
        .fold((Vec3::MAX, Vec3::MIN), |(min, max), point| {
            (min.min(point), max.max(point))
        })
}

#[track_caller]
pub(super) fn assert_box(actual: (Vec3, Vec3), min: Vec3, max: Vec3) {
    assert!(
        actual.0.abs_diff_eq(min, EPS) && actual.1.abs_diff_eq(max, EPS),
        "expected {min}..{max}, got {}..{}",
        actual.0,
        actual.1
    );
}

fn reported_box(value: &Value) -> (Vec3, Vec3) {
    let corner = |at: &Value| {
        let axes: Vec<f32> = at
            .as_array()
            .expect("a corner")
            .iter()
            .map(|axis| axis.as_f64().expect("a number") as f32)
            .collect();
        Vec3::new(axes[0], axes[1], axes[2])
    };
    (corner(&value[0]), corner(&value[1]))
}

pub(super) fn ids(value: &Value) -> Vec<Entity> {
    value
        .as_array()
        .map(|items| items.iter().map(entity_of).collect())
        .unwrap_or_default()
}

pub(super) fn named(app: &mut App, name: &str) -> Vec<Entity> {
    let mut found: Vec<(Entity, f32)> = app
        .world_mut()
        .query_filtered::<(Entity, &Name, &GlobalTransform), With<Brush>>()
        .iter(app.world())
        .filter(|(_, found, _)| found.as_str() == name)
        .map(|(entity, _, global)| (entity, global.translation().x))
        .collect();
    found.sort_by(|a, b| a.1.total_cmp(&b.1));
    found.into_iter().map(|(entity, _)| entity).collect()
}

pub(super) fn scene_text(app: &mut App) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), dir.path())
        .expect("the scene emits")
}

pub(super) fn history_len(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

fn vertices(app: &App, entity: Entity) -> Vec<Vec3> {
    app.world()
        .get::<Brush>(entity)
        .expect("a brush")
        .topology
        .vertices
        .iter()
        .map(|vert| vert.position)
        .collect()
}

/// A wall split through its middle by a slab cutter, with a crate that the
/// cutter also reaches standing beside it.
struct SplitWall {
    wall: Entity,
    cutter: Entity,
    neighbour: Entity,
}

fn split_wall(app: &mut App) -> SplitWall {
    let wall = cube(app, "Wall", [4.0, 2.0, 2.0], [0.0; 3], [0.0; 3]);
    let cutter = cube(app, "Cutter", [1.0, 4.0, 4.0], [0.0; 3], [0.0; 3]);
    let neighbour = cube(app, "Crate", [2.0, 2.0, 2.0], [0.0, 0.0, 2.5], [0.0; 3]);
    clear_selection(app);
    SplitWall {
        wall,
        cutter,
        neighbour,
    }
}

#[test]
fn subtracting_one_cutter_changes_only_the_named_target_and_returns_its_fragments() {
    let mut app = csg_app();
    let scene = split_wall(&mut app);
    let crate_before = vertices(&app, scene.neighbour);
    let crate_box = measured(&app, scene.neighbour);

    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "target": "Wall", "cutters": "Cutter" }),
    );

    assert_eq!(outcome["result"], "finished", "{outcome}");
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    assert_eq!(ids(&outcome["removed"]), vec![scene.wall], "{outcome}");
    let created = ids(&outcome["created"]);
    assert_eq!(created.len(), 2, "{outcome}");
    let mut fragments: Vec<(Vec3, Vec3)> = created.iter().map(|&e| measured(&app, e)).collect();
    fragments.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
    assert_box(
        fragments[0],
        Vec3::new(-2.0, -1.0, -1.0),
        Vec3::new(-0.5, 1.0, 1.0),
    );
    assert_box(
        fragments[1],
        Vec3::new(0.5, -1.0, -1.0),
        Vec3::new(2.0, 1.0, 1.0),
    );
    let reported = outcome["bounds"].as_array().expect("bounds");
    assert_eq!(reported.len(), 2, "{outcome}");
    for (bounds, &entity) in reported.iter().zip(&created) {
        let (min, max) = measured(&app, entity);
        assert_box(reported_box(bounds), min, max);
    }

    assert!(
        app.world().get_entity(scene.wall).is_err(),
        "the target went"
    );
    assert!(
        app.world().get::<Brush>(scene.cutter).is_some(),
        "cutters are kept"
    );
    assert_eq!(vertices(&app, scene.neighbour), crate_before);
    assert_eq!(measured(&app, scene.neighbour), crate_box);
    assert_eq!(
        named(&mut app, "Wall").len(),
        2,
        "the fragments keep the name"
    );
}

#[test]
fn named_operands_by_id_and_keep_cutters_false_consume_the_cutter() {
    let mut app = csg_app();
    let scene = split_wall(&mut app);

    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({
            "target": scene.wall.to_bits(),
            "cutters": scene.cutter.to_bits(),
            "keep_cutters": false,
        }),
    );

    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    assert_eq!(
        ids(&outcome["removed"]),
        vec![scene.wall, scene.cutter],
        "{outcome}"
    );
    assert_eq!(ids(&outcome["created"]).len(), 2, "{outcome}");
    assert!(app.world().get_entity(scene.cutter).is_err());
    assert!(app.world().get::<Brush>(scene.neighbour).is_some());
}

#[test]
fn refused_operands_leave_the_scene_byte_identical() {
    let mut app = csg_app();
    let scene = split_wall(&mut app);
    let outcome = call(&mut app, "entity.add.group", json!({ "name": "Empty" }));
    let group = entity_of(&outcome["entities"][0]);
    clear_selection(&mut app);
    let before = scene_text(&mut app);
    let entries = history_len(&app);

    for (id, params) in [
        ("brush.csg_subtract", json!({ "target": "Wall" })),
        ("brush.csg_subtract", json!({ "cutters": "Cutter" })),
        (
            "brush.csg_subtract",
            json!({ "target": "Wall", "cutters": "Wall" }),
        ),
        (
            "brush.csg_subtract",
            json!({ "target": "Wall", "cutters": "NoSuchBrush" }),
        ),
        (
            "brush.csg_subtract",
            json!({ "target": "Wall", "cutters": group.to_bits() }),
        ),
        (
            "brush.csg_subtract",
            json!({ "target": group.to_bits(), "cutters": "Cutter" }),
        ),
        (
            "brush.csg_intersect",
            json!({ "target": scene.cutter.to_bits(), "cutters": scene.cutter.to_bits() }),
        ),
    ] {
        let outcome = call(&mut app, id, params.clone());
        assert_eq!(outcome["outcome"], "invalid", "{id} {params}: {outcome}");
        assert_eq!(outcome["result"], "cancelled", "{id} {params}: {outcome}");
        assert!(outcome.get("removed").is_none(), "{id} {params}: {outcome}");
        assert!(
            !outcome["warnings"].as_array().expect("warnings").is_empty(),
            "{id} {params}: the refusal says why: {outcome}"
        );
        assert_eq!(history_len(&app), entries, "{id} {params}");
        assert_eq!(scene_text(&mut app), before, "{id} {params}");
    }
    let refused = app
        .world_mut()
        .operator("brush.csg_subtract")
        .param("target", scene.wall)
        .param("cutters", scene.cutter)
        .param("keep_cutters", "maybe".to_string())
        .call()
        .expect("dispatches");
    app.update();
    assert_eq!(refused, OperatorResult::Cancelled);
    assert_eq!(history_len(&app), entries);
    assert_eq!(
        scene_text(&mut app),
        before,
        "a keep_cutters that is not a bool"
    );
    assert!(app.world().get::<Brush>(scene.wall).is_some());
}

#[test]
fn a_disjoint_cutter_is_unchanged_and_a_swallowing_one_empties_the_target() {
    let mut app = csg_app();
    let target = cube(&mut app, "Pebble", [1.0; 3], [0.0; 3], [0.0; 3]);
    cube(&mut app, "Far", [1.0; 3], [5.0, 0.0, 0.0], [0.0; 3]);
    cube(&mut app, "Hull", [3.0; 3], [0.0; 3], [0.0; 3]);
    clear_selection(&mut app);
    let before = scene_text(&mut app);
    let entries = history_len(&app);

    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "target": "Pebble", "cutters": "Far" }),
    );
    assert_eq!(outcome["outcome"], "unchanged", "{outcome}");
    assert_eq!(history_len(&app), entries);
    assert_eq!(scene_text(&mut app), before);

    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "target": "Pebble", "cutters": "Hull" }),
    );
    assert_eq!(outcome["result"], "finished", "{outcome}");
    assert_eq!(outcome["outcome"], "empty", "{outcome}");
    assert_eq!(ids(&outcome["removed"]), vec![target], "{outcome}");
    assert!(ids(&outcome["created"]).is_empty(), "{outcome}");
    assert!(
        named(&mut app, "Pebble").is_empty(),
        "the target is removed"
    );
    assert_eq!(named(&mut app, "Hull").len(), 1, "the cutter is kept");
}

#[test]
fn one_undo_reverts_the_cut_and_redo_reapplies_it() {
    let mut app = csg_app();
    let scene = split_wall(&mut app);
    let wall_box = measured(&app, scene.wall);
    let entries = history_len(&app);

    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "target": "Wall", "cutters": "Cutter" }),
    );
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    assert_eq!(history_len(&app), entries + 1, "one undo entry");

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    app.update();
    let walls = named(&mut app, "Wall");
    assert_eq!(walls.len(), 1, "undo brings the whole wall back");
    assert_eq!(measured(&app, walls[0]), wall_box);

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.redo(world));
    app.update();
    let walls = named(&mut app, "Wall");
    assert_eq!(walls.len(), 2, "redo splits it again");
    assert_box(
        measured(&app, walls[0]),
        Vec3::new(-2.0, -1.0, -1.0),
        Vec3::new(-0.5, 1.0, 1.0),
    );
    assert_box(
        measured(&app, walls[1]),
        Vec3::new(0.5, -1.0, -1.0),
        Vec3::new(2.0, 1.0, 1.0),
    );
}

#[test]
fn a_saved_and_reopened_cut_keeps_its_fragments() {
    let mut app = csg_app();
    split_wall(&mut app);
    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "target": "Wall", "cutters": "Cutter" }),
    );
    assert_eq!(outcome["outcome"], "changed", "{outcome}");

    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("cut.bsn");
    let saved = call(
        &mut app,
        "scene.save",
        json!({ "path": path.to_string_lossy() }),
    );
    assert_eq!(saved["result"], "finished", "{saved}");
    assert!(path.exists(), "scene.save wrote the file");

    let mut app = util::editor_test_app();
    jackdaw::migrate_dialog::request_open_with_conversion(app.world_mut(), &path);
    for _ in 0..8 {
        app.update();
    }
    let walls = named(&mut app, "Wall");
    assert_eq!(walls.len(), 2, "both fragments are saved");
    assert_box(
        measured(&app, walls[0]),
        Vec3::new(-2.0, -1.0, -1.0),
        Vec3::new(-0.5, 1.0, 1.0),
    );
    assert_box(
        measured(&app, walls[1]),
        Vec3::new(0.5, -1.0, -1.0),
        Vec3::new(2.0, 1.0, 1.0),
    );
    assert_eq!(named(&mut app, "Cutter").len(), 1);
    assert_eq!(named(&mut app, "Crate").len(), 1);
}

#[test]
fn transformed_operands_cut_in_world_space() {
    let mut app = csg_app();
    cube(&mut app, "Diamond", [2.0; 3], [0.0; 3], [45.0, 0.0, 0.0]);
    cube(&mut app, "Half", [4.0; 3], [2.0, 0.0, 0.0], [0.0; 3]);
    clear_selection(&mut app);

    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "target": "Diamond", "cutters": "Half" }),
    );

    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    let created = ids(&outcome["created"]);
    assert_eq!(created.len(), 1, "{outcome}");
    let reach = std::f32::consts::SQRT_2;
    assert_box(
        measured(&app, created[0]),
        Vec3::new(-reach, -1.0, -reach),
        Vec3::new(0.0, 1.0, reach),
    );
}

#[test]
fn each_face_keeps_the_material_of_the_solid_it_came_from() {
    let mut app = csg_app();
    let scene = split_wall(&mut app);
    let (stone, cut) = {
        let mut materials = app.world_mut().resource_mut::<Assets<StandardMaterial>>();
        (
            materials.add(StandardMaterial::default()),
            materials.add(StandardMaterial::default()),
        )
    };
    for (entity, handle) in [(scene.wall, &stone), (scene.cutter, &cut)] {
        let mut brush = app.world().get::<Brush>(entity).expect("a brush").clone();
        for face in &mut brush.faces {
            face.material = handle.clone();
        }
        app.world_mut().entity_mut(entity).insert(brush);
    }
    app.update();

    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "target": "Wall", "cutters": "Cutter" }),
    );
    assert_eq!(outcome["outcome"], "changed", "{outcome}");

    for fragment in ids(&outcome["created"]) {
        let brush = app.world().get::<Brush>(fragment).expect("a brush");
        let global = app
            .world()
            .get::<GlobalTransform>(fragment)
            .expect("a transform");
        for face in &brush.faces {
            let normal = face.plane.normal;
            let on_cut = normal.y.abs() < EPS && normal.z.abs() < EPS && {
                let point = global.transform_point(normal * face.plane.distance);
                (point.x.abs() - 0.5).abs() < EPS
            };
            let expected = if on_cut { &cut } else { &stone };
            assert_eq!(
                &face.material, expected,
                "face with normal {normal} wears the material of the solid it came from"
            );
        }
        assert!(
            brush.faces.iter().any(|face| face.material == cut),
            "each fragment has a cut face"
        );
    }
}

#[test]
fn intersecting_named_operands_trims_only_the_target_and_keeps_the_cutter() {
    let mut app = csg_app();
    let target = cube(&mut app, "Block", [2.0; 3], [0.0; 3], [0.0; 3]);
    let cutter = cube(&mut app, "Mould", [2.0; 3], [1.0, 0.0, 0.0], [0.0; 3]);
    cube(&mut app, "Apart", [1.0; 3], [9.0, 0.0, 0.0], [0.0; 3]);
    clear_selection(&mut app);

    let outcome = call(
        &mut app,
        "brush.csg_intersect",
        json!({ "target": "Block", "cutters": "Mould" }),
    );
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    assert_eq!(ids(&outcome["removed"]), vec![target], "{outcome}");
    let created = ids(&outcome["created"]);
    assert_eq!(created.len(), 1, "{outcome}");
    assert_box(
        measured(&app, created[0]),
        Vec3::new(0.0, -1.0, -1.0),
        Vec3::new(1.0, 1.0, 1.0),
    );
    assert!(app.world().get::<Brush>(cutter).is_some());

    let outcome = call(
        &mut app,
        "brush.csg_intersect",
        json!({ "target": "Apart", "cutters": "Mould" }),
    );
    assert_eq!(outcome["outcome"], "empty", "{outcome}");
    assert!(named(&mut app, "Apart").is_empty());
}

#[test]
fn subtract_needs_one_selected_cutter_unless_the_call_names_its_operands() {
    let mut app = csg_app();
    let wall = cube(&mut app, "Wall", [4.0, 2.0, 2.0], [0.0; 3], [0.0; 3]);
    clear_selection(&mut app);
    assert!(
        !app.world_mut()
            .operator("brush.csg_subtract")
            .is_available()
            .expect("known operator")
    );
    app.world_mut().resource_mut::<Selection>().entities = vec![wall];
    app.update();
    assert!(
        app.world_mut()
            .operator("brush.csg_subtract")
            .is_available()
            .expect("known operator"),
        "one selected cutter is enough"
    );
    assert!(
        !app.world_mut()
            .operator("brush.csg_intersect")
            .is_available()
            .expect("known operator"),
        "intersect still needs two"
    );
    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![wall],
        "asking leaves the selection alone"
    );
}

#[test]
fn fragments_under_a_turned_and_scaled_parent_made_in_the_same_batch_stay_in_place() {
    let mut app = csg_app();
    let answer = app
        .world_mut()
        .run_system_cached_with(
            batch_handler,
            Some(json!({ "verbose": true, "calls": [
                { "id": "entity.add.group", "params": { "name": "Frame" } },
                { "id": "entity.set_transform", "params": {
                    "entity": "$0.entities.0", "yaw": 90.0, "sx": 2.0, "sy": 2.0, "sz": 2.0,
                } },
                { "id": "entity.add.cube", "params": {
                    "name": "Wall", "size": [2.0, 1.0, 1.0], "parent": "$0.entities.0",
                } },
                { "id": "entity.add.cube", "params": { "name": "Cutter", "size": [4.0, 4.0, 1.0] } },
                { "id": "brush.csg_subtract", "params": {
                    "target": "$2.entities.0", "cutters": "$3.entities.0",
                } },
            ] })),
        )
        .expect("the handler ran")
        .unwrap_or_else(|err| panic!("the batch refused: {}", err.message));
    app.update();

    let frame = entity_of(&answer["calls"][0]["entities"][0]);
    let outcome = &answer["calls"][4];
    assert_eq!(outcome["outcome"], "changed", "{answer}");
    let mut created = ids(&outcome["created"]);
    assert_eq!(created.len(), 2, "{answer}");
    created.sort_by(|a, b| measured(&app, *a).0.z.total_cmp(&measured(&app, *b).0.z));
    assert_box(
        measured(&app, created[0]),
        Vec3::new(-1.0, -1.0, -2.0),
        Vec3::new(1.0, 1.0, -0.5),
    );
    assert_box(
        measured(&app, created[1]),
        Vec3::new(-1.0, -1.0, 0.5),
        Vec3::new(1.0, 1.0, 2.0),
    );
    for (bounds, &entity) in outcome["bounds"]
        .as_array()
        .expect("bounds")
        .iter()
        .zip(&ids(&outcome["created"]))
    {
        let (min, max) = measured(&app, entity);
        assert_box(reported_box(bounds), min, max);
    }
    for fragment in created {
        assert_eq!(
            app.world().get::<ChildOf>(fragment).map(ChildOf::parent),
            Some(frame),
            "the fragment stays under the target's parent"
        );
    }
}

#[test]
fn a_cutter_under_the_target_is_refused_and_the_targets_children_move_up() {
    let mut app = csg_app();
    let wall = cube(&mut app, "Wall", [4.0, 2.0, 2.0], [0.0; 3], [0.0; 3]);
    let lamp = call(
        &mut app,
        "entity.add.cube",
        json!({ "name": "Lamp", "size": [0.5, 0.5, 0.5], "pos": [1.5, 0.0, 0.0], "parent": wall.to_bits() }),
    );
    let lamp = entity_of(&lamp["entities"][0]);
    cube(&mut app, "Cutter", [1.0, 4.0, 4.0], [0.0; 3], [0.0; 3]);
    clear_selection(&mut app);
    let lamp_box = measured(&app, lamp);
    let before = scene_text(&mut app);

    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "target": "Wall", "cutters": lamp.to_bits() }),
    );
    assert_eq!(outcome["outcome"], "invalid", "{outcome}");
    assert_eq!(scene_text(&mut app), before);

    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "target": "Wall", "cutters": "Cutter" }),
    );
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    assert_eq!(ids(&outcome["removed"]), vec![wall], "{outcome}");
    assert!(
        app.world().get_entity(lamp).is_ok(),
        "the child outlives the target"
    );
    assert_eq!(
        app.world().get::<ChildOf>(lamp),
        None,
        "it moves up to the target's parent"
    );
    assert_box(measured(&app, lamp), lamp_box.0, lamp_box.1);
}

#[test]
fn a_kernel_failure_leaves_the_target_alone() {
    let mut app = csg_app();
    cube(&mut app, "Wall", [4.0, 2.0, 2.0], [0.0; 3], [0.0; 3]);
    let cutter = cube(&mut app, "Broken", [1.0, 4.0, 4.0], [0.0; 3], [0.0; 3]);
    let mut brush = app.world().get::<Brush>(cutter).expect("a brush").clone();
    brush.topology.polygons.clear();
    brush.topology.loops.clear();
    app.world_mut().entity_mut(cutter).insert(brush);
    clear_selection(&mut app);
    let before = scene_text(&mut app);
    let entries = history_len(&app);

    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "target": "Wall", "cutters": "Broken" }),
    );
    assert_eq!(outcome["outcome"], "unchanged", "{outcome}");
    assert!(
        outcome["warnings"].to_string().contains("kernel failed"),
        "{outcome}"
    );
    assert!(outcome.get("removed").is_none(), "{outcome}");
    assert_eq!(history_len(&app), entries);
    assert_eq!(scene_text(&mut app), before);
}

#[test]
fn intersecting_with_a_group_keeps_the_target_inside_any_of_its_brushes() {
    let mut app = csg_app();
    cube(&mut app, "Beam", [6.0, 1.0, 1.0], [0.0; 3], [0.0; 3]);
    let group = call(&mut app, "entity.add.group", json!({ "name": "Moulds" }));
    let group = entity_of(&group["entities"][0]);
    for x in [-2.0, 2.0] {
        call(
            &mut app,
            "entity.add.cube",
            json!({ "size": [1.0, 2.0, 2.0], "pos": [x, 0.0, 0.0], "parent": group.to_bits() }),
        );
    }
    clear_selection(&mut app);

    let outcome = call(
        &mut app,
        "brush.csg_intersect",
        json!({ "target": "Beam", "cutters": "Moulds" }),
    );
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    let mut parts: Vec<(Vec3, Vec3)> = ids(&outcome["created"])
        .into_iter()
        .map(|entity| measured(&app, entity))
        .collect();
    parts.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
    assert_eq!(parts.len(), 2, "{outcome}");
    assert_box(
        parts[0],
        Vec3::new(-2.5, -0.5, -0.5),
        Vec3::new(-1.5, 0.5, 0.5),
    );
    assert_box(
        parts[1],
        Vec3::new(1.5, -0.5, -0.5),
        Vec3::new(2.5, 0.5, 0.5),
    );
}

#[test]
fn keep_cutters_alone_runs_on_the_selection_and_is_honoured() {
    let mut app = csg_app();
    let scene = split_wall(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![scene.cutter];
    app.update();

    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "keep_cutters": false }),
    );
    assert_eq!(outcome["result"], "finished", "{outcome}");
    assert!(
        app.world().get_entity(scene.cutter).is_err(),
        "the cutter is consumed"
    );
    assert!(
        app.world().get_entity(scene.wall).is_err(),
        "the wall is cut"
    );
    assert!(
        app.world().get_entity(scene.neighbour).is_err(),
        "the selection path cuts every brush the cutter reaches"
    );
    assert_eq!(named(&mut app, "Wall").len(), 2);
}

fn wear(app: &mut App, entity: Entity, handle: &Handle<StandardMaterial>) {
    let mut brush = app.world().get::<Brush>(entity).expect("a brush").clone();
    for face in &mut brush.faces {
        face.material = handle.clone();
    }
    app.world_mut().entity_mut(entity).insert(brush);
    app.update();
}

fn registered_material(app: &mut App, name: &str) -> Handle<StandardMaterial> {
    let handle = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    app.world_mut()
        .resource_mut::<MaterialRegistry>()
        .add(name.into(), handle.clone());
    handle
}

/// A face's world-space plane point and normal.
fn world_plane(app: &App, entity: Entity, face: usize) -> (Vec3, Vec3) {
    let global = app
        .world()
        .get::<GlobalTransform>(entity)
        .expect("a transform");
    let plane = &app.world().get::<Brush>(entity).expect("a brush").faces[face].plane;
    let point = global.transform_point(plane.normal * plane.distance);
    let normal = global.rotation() * plane.normal;
    (point, normal)
}

/// Whether a world plane is one of the planes listed as `(normal, point)`.
fn on_any(plane: (Vec3, Vec3), planes: &[(Vec3, Vec3)]) -> bool {
    planes.iter().any(|(normal, point)| {
        plane.1.abs_diff_eq(*normal, EPS) && (plane.0 - *point).dot(*normal).abs() < EPS
    })
}

/// The texture coordinate a face gives `world_point`.
fn uv_at(app: &App, entity: Entity, face: usize, world_point: Vec3) -> Vec2 {
    let global = app
        .world()
        .get::<GlobalTransform>(entity)
        .expect("a transform");
    let face = &app.world().get::<Brush>(entity).expect("a brush").faces[face];
    let local = global.affine().inverse().transform_point3(world_point);
    let (u, v) = (local.dot(face.uv_u_axis), local.dot(face.uv_v_axis));
    let (sin, cos) = face.uv_rotation.sin_cos();
    Vec2::new(
        (u * cos - v * sin) / face.uv_scale.x.max(0.001) + face.uv_offset.x,
        (u * sin + v * cos) / face.uv_scale.y.max(0.001) + face.uv_offset.y,
    )
}

#[test]
fn a_dry_run_leaves_the_scene_byte_identical_and_reports_what_the_real_run_does() {
    for (id, params, selected) in [
        (
            "brush.csg_subtract",
            json!({ "target": "Wall", "cutters": "Cutter" }),
            false,
        ),
        (
            "brush.csg_intersect",
            json!({ "target": "Wall", "cutters": "Cutter" }),
            false,
        ),
        ("brush.csg_subtract", json!({}), true),
        ("brush.csg_intersect", json!({}), true),
    ] {
        let mut app = csg_app();
        let scene = split_wall(&mut app);
        if selected {
            app.world_mut().resource_mut::<Selection>().entities = if id == "brush.csg_subtract" {
                vec![scene.cutter]
            } else {
                vec![scene.wall, scene.cutter]
            };
            app.update();
        }
        let before = scene_text(&mut app);
        let entries = history_len(&app);

        let mut dry = params.clone();
        dry["dry_run"] = json!(true);
        let preview = call(&mut app, id, dry);
        assert_eq!(preview["result"], "finished", "{id} {params}: {preview}");
        assert_eq!(preview["outcome"], "unchanged", "{id} {params}: {preview}");
        assert!(preview.get("removed").is_none(), "{id} {params}: {preview}");
        assert!(preview.get("created").is_none(), "{id} {params}: {preview}");
        assert_eq!(history_len(&app), entries, "{id} {params}");
        assert_eq!(scene_text(&mut app), before, "{id} {params}");
        assert!(
            app.world().get::<Brush>(scene.wall).is_some(),
            "{id} {params}"
        );

        let real = call(&mut app, id, params.clone());
        assert_eq!(real["outcome"], "changed", "{id} {params}: {real}");
        assert_eq!(
            real["report"]["outcome"], "changed",
            "{id} {params}: {real}"
        );
        assert_eq!(preview["report"], real["report"], "{id} {params}");
    }
}

#[test]
fn the_report_gives_the_analytic_volume_for_a_box_cut_from_a_box() {
    let mut app = csg_app();
    split_wall(&mut app);

    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "target": "Wall", "cutters": "Cutter" }),
    );
    let report = &outcome["report"];
    assert_eq!(report["outcome"], "changed", "{outcome}");
    assert_eq!(report["fragments"], 2, "{outcome}");
    let volume: Vec<f64> = report["volume"]
        .as_array()
        .expect("volume before and after")
        .iter()
        .map(|v| v.as_f64().expect("a number"))
        .collect();
    assert!((volume[0] - 16.0).abs() < 1e-3, "{outcome}");
    assert!((volume[1] - 12.0).abs() < 1e-3, "{outcome}");
    assert_box(
        reported_box(&report["bounds"]),
        Vec3::new(-2.0, -1.0, -1.0),
        Vec3::new(2.0, 1.0, 1.0),
    );
    assert_eq!(report["watertight"], true, "{outcome}");
    assert_eq!(report["cut_faces"], 2, "{outcome}");
    assert_eq!(report["degenerate"], 0, "{outcome}");
}

#[test]
fn cut_faces_picks_the_material_of_exactly_the_new_faces() {
    let wall_planes = [
        (Vec3::X, Vec3::new(2.0, 0.0, 0.0)),
        (Vec3::NEG_X, Vec3::new(-2.0, 0.0, 0.0)),
        (Vec3::Y, Vec3::new(0.0, 1.0, 0.0)),
        (Vec3::NEG_Y, Vec3::new(0.0, -1.0, 0.0)),
        (Vec3::Z, Vec3::new(0.0, 0.0, 1.0)),
        (Vec3::NEG_Z, Vec3::new(0.0, 0.0, -1.0)),
    ];
    for choice in ["cutter", "target", "materials/slate.material.bsn"] {
        let mut app = csg_app();
        let scene = split_wall(&mut app);
        let stone = registered_material(&mut app, "stone");
        let cut = registered_material(&mut app, "cut");
        let slate = registered_material(&mut app, "slate");
        wear(&mut app, scene.wall, &stone);
        wear(&mut app, scene.cutter, &cut);

        let outcome = call(
            &mut app,
            "brush.csg_subtract",
            json!({ "target": "Wall", "cutters": "Cutter", "cut_faces": choice }),
        );
        assert_eq!(outcome["outcome"], "changed", "{choice}: {outcome}");
        let expected = match choice {
            "cutter" => &cut,
            "target" => &stone,
            _ => &slate,
        };
        let mut new_faces = 0;
        for fragment in ids(&outcome["created"]) {
            let count = app
                .world()
                .get::<Brush>(fragment)
                .expect("a brush")
                .faces
                .len();
            for face in 0..count {
                let plane = world_plane(&app, fragment, face);
                let worn = app.world().get::<Brush>(fragment).expect("a brush").faces[face]
                    .material
                    .clone();
                if on_any(plane, &wall_planes) {
                    assert_eq!(worn, stone, "{choice}: a kept face keeps its material");
                } else {
                    new_faces += 1;
                    assert_eq!(&worn, expected, "{choice}: a new face wears the choice");
                }
            }
        }
        assert_eq!(new_faces, 2, "{choice}: one new face per fragment");
        assert_eq!(outcome["report"]["cut_faces"], 2, "{choice}: {outcome}");
    }
}

#[test]
fn an_unknown_cut_faces_material_is_refused() {
    let mut app = csg_app();
    split_wall(&mut app);
    let before = scene_text(&mut app);
    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "target": "Wall", "cutters": "Cutter", "cut_faces": "materials/missing.material.bsn" }),
    );
    assert_eq!(outcome["outcome"], "invalid", "{outcome}");
    assert_eq!(scene_text(&mut app), before);
}

#[test]
fn a_doorway_cut_dresses_its_reveals_in_the_cutters_material_and_is_confirmed_by_the_report() {
    let mut app = csg_app();
    let wall = cube(
        &mut app,
        "Wall",
        [4.0, 3.0, 0.25],
        [0.0, 1.5, 0.0],
        [0.0; 3],
    );
    let door = cube(&mut app, "Door", [1.0, 2.0, 1.0], [0.0, 1.0, 0.0], [0.0; 3]);
    clear_selection(&mut app);
    let plaster = registered_material(&mut app, "plaster");
    let timber = registered_material(&mut app, "timber");
    wear(&mut app, wall, &plaster);
    wear(&mut app, door, &timber);
    let front = (0..app.world().get::<Brush>(wall).expect("a brush").faces.len())
        .find(|&face| world_plane(&app, wall, face).1.abs_diff_eq(Vec3::Z, EPS))
        .expect("a front face");
    let probe = Vec3::new(1.5, 2.5, 0.125);
    let front_uv = uv_at(&app, wall, front, probe);

    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "target": "Wall", "cutters": "Door" }),
    );
    let report = &outcome["report"];
    assert_eq!(report["outcome"], "changed", "{outcome}");
    assert_eq!(report["fragments"], 1, "{outcome}");
    let volume = report["volume"].as_array().expect("volumes");
    assert!(
        (volume[0].as_f64().expect("a number") - 3.0).abs() < 1e-3,
        "{outcome}"
    );
    assert!(
        (volume[1].as_f64().expect("a number") - 2.5).abs() < 1e-3,
        "{outcome}"
    );
    assert_box(
        reported_box(&report["bounds"]),
        Vec3::new(-2.0, 0.0, -0.125),
        Vec3::new(2.0, 3.0, 0.125),
    );
    assert_eq!(report["watertight"], true, "{outcome}");
    assert_eq!(report["cut_faces"], 3, "two jambs and a head: {outcome}");
    assert_eq!(
        report["coplanar"], 1,
        "the threshold sits on the floor plane: {outcome}"
    );

    let reveals = [
        (Vec3::X, Vec3::new(-0.5, 0.0, 0.0)),
        (Vec3::NEG_X, Vec3::new(0.5, 0.0, 0.0)),
        (Vec3::NEG_Y, Vec3::new(0.0, 2.0, 0.0)),
    ];
    let fragment = ids(&outcome["created"])[0];
    let count = app
        .world()
        .get::<Brush>(fragment)
        .expect("a brush")
        .faces
        .len();
    let mut seen = 0;
    let mut fronts = 0;
    for face in 0..count {
        let (point, normal) = world_plane(&app, fragment, face);
        let data = app.world().get::<Brush>(fragment).expect("a brush").faces[face].clone();
        if on_any((point, normal), &reveals) {
            seen += 1;
            assert_eq!(data.material, timber, "a reveal wears the door's material");
            let axes = jackdaw_uv::world_aligned(normal);
            let global = app
                .world()
                .get::<GlobalTransform>(fragment)
                .expect("a transform");
            assert!(
                (global.rotation() * data.uv_u_axis).abs_diff_eq(axes.u, EPS)
                    && (global.rotation() * data.uv_v_axis).abs_diff_eq(axes.v, EPS),
                "a reveal is world-aligned"
            );
            let corner = point + axes.u * 0.25 + axes.v * 0.5;
            let expected = Vec2::new(corner.dot(axes.u), corner.dot(axes.v)) / data.uv_scale;
            assert!(
                uv_at(&app, fragment, face, corner).abs_diff_eq(expected, EPS),
                "a reveal projects from the world grid"
            );
        } else {
            assert_eq!(
                data.material, plaster,
                "a kept face keeps the wall's material"
            );
            if normal.abs_diff_eq(Vec3::Z, EPS) && (point - probe).dot(Vec3::Z).abs() < EPS {
                fronts += 1;
                assert!(
                    uv_at(&app, fragment, face, probe).abs_diff_eq(front_uv, EPS),
                    "the wall's face keeps its texture where it was"
                );
            }
        }
    }
    assert_eq!(seen, 3, "two jambs and a head");
    assert!(fronts > 0, "the wall's front face survives");
}

fn set_uv_scale(app: &mut App, entity: Entity, scale: Vec2) {
    let mut brush = app.world().get::<Brush>(entity).expect("a brush").clone();
    for face in &mut brush.faces {
        face.uv_scale = scale;
    }
    app.world_mut().entity_mut(entity).insert(brush);
    app.update();
}

/// The faces of `entity` that lie on none of `planes`, by index.
fn faces_off(app: &App, entity: Entity, planes: &[(Vec3, Vec3)]) -> Vec<usize> {
    let count = app
        .world()
        .get::<Brush>(entity)
        .expect("a brush")
        .faces
        .len();
    (0..count)
        .filter(|&face| !on_any(world_plane(app, entity, face), planes))
        .collect()
}

fn box_planes(min: Vec3, max: Vec3) -> [(Vec3, Vec3); 6] {
    [
        (Vec3::X, max),
        (Vec3::NEG_X, min),
        (Vec3::Y, max),
        (Vec3::NEG_Y, min),
        (Vec3::Z, max),
        (Vec3::NEG_Z, min),
    ]
}

#[test]
fn a_cut_face_takes_the_scale_of_the_material_it_wears() {
    for (choice, wanted) in [("cutter", Vec2::splat(2.0)), ("target", Vec2::splat(0.5))] {
        let mut app = csg_app();
        let scene = split_wall(&mut app);
        set_uv_scale(&mut app, scene.wall, Vec2::splat(0.5));
        set_uv_scale(&mut app, scene.cutter, Vec2::splat(2.0));

        let outcome = call(
            &mut app,
            "brush.csg_subtract",
            json!({ "target": "Wall", "cutters": "Cutter", "cut_faces": choice }),
        );
        let wall = box_planes(Vec3::new(-2.0, -1.0, -1.0), Vec3::new(2.0, 1.0, 1.0));
        let mut seen = 0;
        for fragment in ids(&outcome["created"]) {
            for face in faces_off(&app, fragment, &wall) {
                seen += 1;
                let scale =
                    app.world().get::<Brush>(fragment).expect("a brush").faces[face].uv_scale;
                assert!(scale.abs_diff_eq(wanted, EPS), "{choice}: scale {scale}");
            }
        }
        assert_eq!(seen, 2, "{choice}: {outcome}");
    }
}

#[test]
fn a_scaled_target_keeps_its_texture_where_it_was() {
    let mut app = csg_app();
    let wall = cube(&mut app, "Wall", [1.0; 3], [0.0; 3], [0.0; 3]);
    app.world_mut()
        .get_mut::<Transform>(wall)
        .expect("a transform")
        .scale = Vec3::new(2.0, 1.0, 1.0);
    cube(&mut app, "Cutter", [0.5, 2.0, 2.0], [0.0; 3], [0.0; 3]);
    clear_selection(&mut app);
    let front = (0..app.world().get::<Brush>(wall).expect("a brush").faces.len())
        .find(|&face| world_plane(&app, wall, face).1.abs_diff_eq(Vec3::Z, EPS))
        .expect("a front face");
    let probes = [Vec3::new(-0.75, 0.25, 0.5), Vec3::new(0.6, -0.3, 0.5)];
    let before: Vec<Vec2> = probes
        .iter()
        .map(|&p| uv_at(&app, wall, front, p))
        .collect();

    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "target": "Wall", "cutters": "Cutter" }),
    );
    assert_eq!(outcome["report"]["fragments"], 2, "{outcome}");
    for (probe, before) in probes.iter().zip(&before) {
        let mut checked = 0;
        for fragment in ids(&outcome["created"]) {
            let (min, max) = measured(&app, fragment);
            if probe.x < min.x || probe.x > max.x {
                continue;
            }
            let count = app
                .world()
                .get::<Brush>(fragment)
                .expect("a brush")
                .faces
                .len();
            for face in 0..count {
                let (point, normal) = world_plane(&app, fragment, face);
                if normal.abs_diff_eq(Vec3::Z, EPS) && (point - *probe).dot(Vec3::Z).abs() < EPS {
                    checked += 1;
                    let after = uv_at(&app, fragment, face, *probe);
                    assert!(
                        after.abs_diff_eq(*before, EPS),
                        "{probe}: {before} became {after}"
                    );
                }
            }
        }
        assert!(checked > 0, "{probe} lies on a kept front face");
    }
}

#[test]
fn a_shallow_groove_floor_counts_as_a_cut_face() {
    let mut app = csg_app();
    let block = cube(&mut app, "Block", [2.0; 3], [0.0; 3], [0.0; 3]);
    let chisel = cube(
        &mut app,
        "Chisel",
        [0.5, 0.5, 3.0],
        [0.0, 1.245, 0.0],
        [0.0; 3],
    );
    clear_selection(&mut app);
    let stone = registered_material(&mut app, "stone");
    let cut = registered_material(&mut app, "cut");
    wear(&mut app, block, &stone);
    wear(&mut app, chisel, &cut);

    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "target": "Block", "cutters": "Chisel" }),
    );
    assert_eq!(outcome["report"]["fragments"], 1, "{outcome}");
    assert_eq!(
        outcome["report"]["cut_faces"], 3,
        "a floor and two walls: {outcome}"
    );
    let fragment = ids(&outcome["created"])[0];
    let count = app
        .world()
        .get::<Brush>(fragment)
        .expect("a brush")
        .faces
        .len();
    let floor = (0..count)
        .find(|&face| {
            let (point, normal) = world_plane(&app, fragment, face);
            normal.abs_diff_eq(Vec3::Y, EPS) && (point.y - 0.995).abs() < 1e-4
        })
        .expect("the groove floor");
    assert_eq!(
        app.world().get::<Brush>(fragment).expect("a brush").faces[floor].material,
        cut,
        "the floor 5 mm below the top is the chisel's"
    );
}

#[test]
fn on_the_selection_a_target_the_cutter_misses_is_left_alone() {
    let mut app = csg_app();
    let target = cube(&mut app, "Turned", [2.0; 3], [0.0; 3], [45.0, 0.0, 0.0]);
    let cutter = cube(&mut app, "Corner", [0.2; 3], [1.25, 0.0, 1.25], [0.0; 3]);
    clear_selection(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![cutter];
    app.update();
    let before = scene_text(&mut app);
    let entries = history_len(&app);

    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "keep_cutters": false }),
    );
    assert_eq!(outcome["outcome"], "unchanged", "{outcome}");
    assert!(
        app.world().get::<Brush>(target).is_some(),
        "the target stays"
    );
    assert!(
        app.world().get::<Brush>(cutter).is_some(),
        "the cutter stays"
    );
    assert_eq!(history_len(&app), entries);
    assert_eq!(scene_text(&mut app), before);
}

#[test]
fn on_the_selection_a_swallowed_target_is_removed_and_reported_empty() {
    let mut app = csg_app();
    let target = cube(&mut app, "Pebble", [1.0; 3], [0.0; 3], [0.0; 3]);
    let hull = cube(&mut app, "Hull", [3.0; 3], [0.0; 3], [0.0; 3]);
    clear_selection(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![hull];
    app.update();

    let outcome = call(&mut app, "brush.csg_subtract", json!({}));
    assert_eq!(outcome["outcome"], "empty", "{outcome}");
    assert_eq!(outcome["report"]["outcome"], "empty", "{outcome}");
    assert_eq!(outcome["report"]["fragments"], 0, "{outcome}");
    assert_eq!(ids(&outcome["removed"]), vec![target], "{outcome}");
    assert!(app.world().get_entity(target).is_err());
    assert!(app.world().get::<Brush>(hull).is_some());
}

#[test]
fn a_selection_intersect_reports_the_volume_of_every_brush_it_replaces() {
    for (keep, before) in [(false, 32.0), (true, 16.0)] {
        let mut app = csg_app();
        let scene = split_wall(&mut app);
        app.world_mut().resource_mut::<Selection>().entities = vec![scene.wall, scene.cutter];
        app.update();
        let outcome = call(
            &mut app,
            "brush.csg_intersect",
            json!({ "keep_cutters": keep, "dry_run": true }),
        );
        let reported = outcome["report"]["volume"][0].as_f64().expect("a volume");
        assert!((reported - before).abs() < 1e-3, "keep {keep}: {outcome}");
    }
}

#[test]
fn a_kept_cutter_keeps_its_modifier_stack() {
    let mut app = csg_app();
    let scene = split_wall(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![scene.cutter];
    app.update();
    app.world_mut()
        .operator("mesh.mirror.add")
        .call()
        .expect("mesh.mirror.add dispatches")
        .assert_finished();
    app.update();
    assert!(
        app.world()
            .get::<jackdaw_geometry::ModifierStack>(scene.cutter)
            .is_some()
    );

    let outcome = call(&mut app, "brush.csg_subtract", json!({}));
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    assert!(
        app.world()
            .get::<jackdaw_geometry::ModifierStack>(scene.cutter)
            .is_some(),
        "the cutter's stack is left live"
    );
}
