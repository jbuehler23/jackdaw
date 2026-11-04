//! Brush elements addressed without a pointer: `brush.select_elements` by
//! index or by rule, `jackdaw/brush_topology` for the arrays behind the
//! indices, a selection that drives the one-shot mesh operators in the same
//! batch, and a loop cut aimed by a rule.

use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_editor_core::brush::{BrushEditMode, BrushSelection, EditMode};
use jackdaw_editor_core::commands::CommandHistory;
use jackdaw_editor_core::remote::server::{
    batch_handler, brush_topology_handler, call_operator_handler,
};
use jackdaw_editor_core::selection::Selection;
use jackdaw_scene_types::Brush;
use serde_json::{Value, json};

fn scene_app() -> App {
    let mut app = util::editor_test_app();
    app.world_mut()
        .operator("scene.new")
        .call()
        .expect("scene.new dispatches")
        .assert_finished();
    app.update();
    app
}

fn handle<M>(
    app: &mut App,
    handler: impl IntoSystem<In<Option<Value>>, bevy::remote::BrpResult, M> + Copy + 'static,
    params: Value,
) -> Result<Value, String> {
    let answer = app
        .world_mut()
        .run_system_cached_with(handler, Some(params))
        .expect("the handler ran")
        .map_err(|err| err.message);
    app.update();
    answer
}

fn call(app: &mut App, id: &str, params: Value) -> Value {
    handle(
        app,
        call_operator_handler,
        json!({ "id": id, "params": params }),
    )
    .unwrap_or_else(|err| panic!("{id} refused: {err}"))
}

fn cube(app: &mut App) -> Entity {
    let outcome = call(app, "entity.add.cube", json!({}));
    let bits = outcome["entities"][0].as_u64().expect("an entity id");
    Entity::try_from_bits(bits).expect("a valid entity id")
}

fn select(app: &mut App, entity: Entity, params: Value) -> Value {
    let mut params = params;
    params["entity"] = json!(entity.to_bits());
    call(app, "brush.select_elements", params)
}

fn brush(app: &App, entity: Entity) -> Brush {
    app.world().get::<Brush>(entity).expect("a brush").clone()
}

fn face_with_normal(brush: &Brush, normal: Vec3) -> usize {
    (0..brush.topology.polygons.len())
        .find(|&face| brush.topology.face_normal(face).abs_diff_eq(normal, 1e-4))
        .expect("a face with that normal")
}

fn faces(app: &App, entity: Entity) -> Vec<usize> {
    app.world()
        .resource::<BrushSelection>()
        .sub(entity)
        .map(|sub| sub.faces.clone())
        .unwrap_or_default()
}

fn history_len(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

fn edit_mode(app: &App) -> EditMode {
    *app.world().resource::<EditMode>()
}

#[test]
fn the_top_face_of_a_cube_is_selected_by_its_normal() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    app.world_mut().resource_mut::<Selection>().entities.clear();
    let history = history_len(&app);

    let outcome = select(
        &mut app,
        entity,
        json!({ "kind": "face", "where": { "normal": "+y" } }),
    );
    assert_eq!(outcome["result"], "finished", "{outcome}");
    assert_eq!(outcome["outcome"], "changed", "{outcome}");

    let top = face_with_normal(&brush(&app, entity), Vec3::Y);
    assert_eq!(faces(&app, entity), vec![top]);
    assert_eq!(edit_mode(&app), EditMode::BrushEdit(BrushEditMode::Face));
    assert_eq!(
        app.world().resource::<BrushSelection>().active_brush,
        Some(entity)
    );
    assert!(
        app.world()
            .resource::<Selection>()
            .entities
            .contains(&entity),
        "the brush is selected so edit mode holds"
    );
    assert_eq!(history_len(&app), history, "selection is not an undo step");

    // A frame later the mode and the pick still stand.
    app.update();
    assert_eq!(edit_mode(&app), EditMode::BrushEdit(BrushEditMode::Face));
    assert_eq!(faces(&app, entity), vec![top]);
}

#[test]
fn set_add_and_remove_combine_with_the_selection() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    let shape = brush(&app, entity);
    let top = face_with_normal(&shape, Vec3::Y);
    let bottom = face_with_normal(&shape, Vec3::NEG_Y);

    select(
        &mut app,
        entity,
        json!({ "kind": "face", "indices": [top] }),
    );
    let added = select(
        &mut app,
        entity,
        json!({ "kind": "face", "where": { "normal": "-y" }, "mode": "add" }),
    );
    assert_eq!(added["outcome"], "changed");
    assert_eq!(faces(&app, entity), vec![top, bottom]);

    select(
        &mut app,
        entity,
        json!({ "kind": "face", "indices": [top], "mode": "remove" }),
    );
    assert_eq!(faces(&app, entity), vec![bottom]);

    select(
        &mut app,
        entity,
        json!({ "kind": "face", "where": { "all": true }, "mode": "set" }),
    );
    assert_eq!(faces(&app, entity).len(), 6);
}

#[test]
fn the_same_call_twice_keeps_the_mode_and_reports_unchanged() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    let params = json!({ "kind": "face", "where": { "normal": "+y" } });
    select(&mut app, entity, params.clone());
    let again = select(&mut app, entity, params);
    assert_eq!(again["result"], "finished");
    assert_eq!(again["outcome"], "unchanged");
    assert_eq!(edit_mode(&app), EditMode::BrushEdit(BrushEditMode::Face));
    assert_eq!(faces(&app, entity).len(), 1);
}

#[test]
fn switching_kind_enters_that_mode_and_drops_the_other_picks() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    select(
        &mut app,
        entity,
        json!({ "kind": "face", "where": { "normal": "+y" } }),
    );
    select(
        &mut app,
        entity,
        json!({ "kind": "vertex", "where": { "extreme": "max_y" } }),
    );
    assert_eq!(edit_mode(&app), EditMode::BrushEdit(BrushEditMode::Vertex));
    let sub = app
        .world()
        .resource::<BrushSelection>()
        .sub(entity)
        .cloned()
        .expect("an edit brush");
    assert!(sub.faces.is_empty());
    assert_eq!(sub.vertices.len(), 4);

    select(
        &mut app,
        entity,
        json!({ "kind": "edge", "where": { "direction": "y" } }),
    );
    let sub = app
        .world()
        .resource::<BrushSelection>()
        .sub(entity)
        .cloned()
        .expect("an edit brush");
    assert_eq!(sub.edges.len(), 4, "the four upright edges");
    assert!(sub.vertices.is_empty());
}

#[test]
fn refused_calls_leave_the_selection_and_mode_alone() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    let top = face_with_normal(&brush(&app, entity), Vec3::Y);
    select(
        &mut app,
        entity,
        json!({ "kind": "face", "indices": [top] }),
    );
    let group = call(&mut app, "entity.add.group", json!({ "name": "NotABrush" }));
    let group_bits = group["entities"][0].as_u64().expect("an entity id");

    for (params, why) in [
        (json!({ "kind": "face", "indices": [6] }), "out of range"),
        (
            json!({ "kind": "edge", "indices": "0, 12" }),
            "out of range",
        ),
        (
            json!({ "kind": "face", "where": { "loop_of": 0 } }),
            "a rule for edges",
        ),
        (
            json!({ "kind": "face", "where": { "facing": "+y" } }),
            "unknown rule",
        ),
        (json!({ "kind": "corner", "indices": [0] }), "unknown kind"),
        (json!({ "kind": "face" }), "no rule"),
        (
            json!({ "kind": "face", "indices": [0], "mode": "toggle" }),
            "unknown mode",
        ),
    ] {
        let outcome = select(&mut app, entity, params);
        assert_eq!(outcome["result"], "cancelled", "{why}: {outcome}");
        assert_eq!(outcome["outcome"], "invalid", "{why}: {outcome}");
        assert!(!outcome["warnings"].as_array().unwrap().is_empty(), "{why}");
        assert_eq!(faces(&app, entity), vec![top], "{why}");
        assert_eq!(
            edit_mode(&app),
            EditMode::BrushEdit(BrushEditMode::Face),
            "{why}"
        );
    }

    let outcome = call(
        &mut app,
        "brush.select_elements",
        json!({ "entity": group_bits, "kind": "face", "where": { "all": true } }),
    );
    assert_eq!(outcome["outcome"], "invalid", "{outcome}");
    assert_eq!(faces(&app, entity), vec![top]);
}

#[test]
fn a_rule_selection_drives_extrude_and_subdivide_in_one_batch() {
    let mut app = scene_app();
    let answer = handle(
        &mut app,
        batch_handler,
        json!({ "calls": [
            { "id": "entity.add.cube", "params": {} },
            { "id": "brush.select_elements", "params": {
                "entity": "$0.entities.0", "kind": "face", "where": { "normal": "+y" } } },
            { "id": "brush.mesh.extrude_region", "params": {} },
            { "id": "brush.select_elements", "params": {
                "entity": "$0.entities.0", "kind": "edge", "where": { "normal": "+y" } } },
            { "id": "brush.mesh.subdivide", "params": {} },
        ] }),
    )
    .unwrap_or_else(|err| panic!("the batch refused: {err}"));
    let calls = answer["calls"].as_array().expect("calls");
    assert_eq!(calls.len(), 5, "{answer}");
    for call in calls {
        assert_eq!(call["result"], "finished", "{answer}");
    }

    let bits = calls[0]["entities"][0].as_u64().expect("an entity id");
    let entity = Entity::try_from_bits(bits).expect("a valid entity id");
    let shape = brush(&app, entity);
    let top = shape
        .topology
        .vertices
        .iter()
        .map(|vert| vert.position.y)
        .fold(f32::MIN, f32::max);
    assert!(
        (top - 1.0).abs() < 1e-4,
        "the top moved up by the extrusion: {top}"
    );
    assert_eq!(
        shape.topology.vertices.len(),
        16,
        "the extrusion's 4 new corners and a midpoint on each of the 4 top edges"
    );
    assert!(shape.topology.polygons.len() >= 10);
}

#[test]
fn a_loop_cut_aims_by_rule_or_by_edge_index() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    let history = history_len(&app);

    let cut = call(
        &mut app,
        "brush.mesh.loop_cut_at",
        json!({ "entity": entity.to_bits(), "where": { "direction": "y" } }),
    );
    assert_eq!(cut["result"], "finished", "{cut}");
    assert_eq!(cut["outcome"], "changed", "{cut}");
    assert_eq!(brush(&app, entity).topology.polygons.len(), 10);
    assert_eq!(edit_mode(&app), EditMode::BrushEdit(BrushEditMode::Edge));
    assert_eq!(history_len(&app), history + 1, "one undo entry");

    app.world_mut()
        .operator("history.undo")
        .call()
        .expect("undo dispatches")
        .assert_finished();
    app.update();
    assert_eq!(brush(&app, entity).topology.polygons.len(), 6);

    let vertical = {
        let shape = brush(&app, entity);
        (0..shape.topology.edges.len())
            .find(|&edge| {
                let [a, b] = shape.topology.edges[edge].v;
                let run = shape.topology.vertices[b as usize].position
                    - shape.topology.vertices[a as usize].position;
                run.x.abs() < 1e-5 && run.z.abs() < 1e-5
            })
            .expect("an upright edge")
    };
    let by_index = call(
        &mut app,
        "brush.mesh.loop_cut_at",
        json!({ "entity": entity.to_bits(), "edge": vertical }),
    );
    assert_eq!(by_index["outcome"], "changed", "{by_index}");
    assert_eq!(brush(&app, entity).topology.polygons.len(), 10);

    let none = call(
        &mut app,
        "brush.mesh.loop_cut_at",
        json!({ "entity": entity.to_bits(), "where": { "direction": [1, 1, 1] } }),
    );
    assert_eq!(none["result"], "cancelled", "{none}");
    assert_eq!(none["outcome"], "unchanged", "{none}");
    assert_eq!(brush(&app, entity).topology.polygons.len(), 10);
}

#[test]
fn a_loop_cut_that_cannot_cut_leaves_mode_and_selection_alone() {
    let mut app = scene_app();
    let pyramid = call(&mut app, "entity.add.pyramid", json!({}));
    let bits = pyramid["entities"][0].as_u64().expect("an entity id");
    let entity = Entity::try_from_bits(bits).expect("a valid entity id");
    let apex = {
        let shape = brush(&app, entity);
        let top = (0..shape.topology.vertices.len())
            .max_by(|a, b| {
                let y = |v: usize| shape.topology.vertices[v].position.y;
                y(*a).total_cmp(&y(*b))
            })
            .expect("vertices");
        (0..shape.topology.edges.len())
            .find(|&edge| shape.topology.edges[edge].v.contains(&(top as u32)))
            .expect("an edge to the apex")
    };
    select(&mut app, entity, json!({ "kind": "face", "indices": [0] }));
    let before = brush(&app, entity).topology.polygons.len();

    let cut = call(
        &mut app,
        "brush.mesh.loop_cut_at",
        json!({ "entity": entity.to_bits(), "edge": apex }),
    );
    assert_eq!(cut["result"], "cancelled", "{cut}");
    assert_eq!(cut["outcome"], "unchanged", "{cut}");
    assert_eq!(edit_mode(&app), EditMode::BrushEdit(BrushEditMode::Face));
    assert_eq!(faces(&app, entity), vec![0]);
    assert_eq!(brush(&app, entity).topology.polygons.len(), before);
}

#[test]
fn a_set_leaves_only_the_target_brush_with_picks() {
    let mut app = scene_app();
    let first = cube(&mut app);
    let second = cube(&mut app);
    select(&mut app, first, json!({ "kind": "face", "indices": [0] }));
    app.world_mut()
        .resource_mut::<BrushSelection>()
        .sub_mut(second)
        .faces = vec![1];

    select(&mut app, first, json!({ "kind": "face", "indices": [2] }));
    assert_eq!(faces(&app, first), vec![2]);
    assert!(
        faces(&app, second).is_empty(),
        "the other brush lost its picks"
    );
}

#[test]
fn a_stale_edited_brush_falls_back_to_the_selected_one() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    let gone = app.world_mut().spawn_empty().id();
    app.world_mut()
        .resource_mut::<BrushSelection>()
        .active_brush = Some(gone);
    app.world_mut().resource_mut::<Selection>().entities = vec![entity];

    let outcome = call(
        &mut app,
        "brush.select_elements",
        json!({ "kind": "face", "where": { "normal": "+y" } }),
    );
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    assert_eq!(faces(&app, entity).len(), 1);
}

#[test]
fn a_selected_tilemap_blocks_element_selection() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    let tilemap = app
        .world_mut()
        .spawn(jackdaw_scene_types::Tilemap::default())
        .id();
    app.world_mut().resource_mut::<Selection>().entities = vec![tilemap];

    // Dispatched directly: a remote call would select the brush it names.
    let result = app
        .world_mut()
        .operator("brush.select_elements")
        .param("entity", entity)
        .param("kind", "face")
        .param("where", r#"{"all": true}"#)
        .call()
        .expect("dispatches");
    assert_eq!(result, OperatorResult::Cancelled);
    assert_eq!(edit_mode(&app), EditMode::Object);
    assert!(faces(&app, entity).is_empty());
}

#[test]
fn a_world_box_composes_the_parent_transform() {
    let mut app = scene_app();
    let parent = call(&mut app, "entity.add.group", json!({ "name": "Lifted" }));
    let parent_bits = parent["entities"][0].as_u64().expect("an entity id");
    let parent = Entity::try_from_bits(parent_bits).expect("a valid entity id");
    let entity = {
        let child = call(
            &mut app,
            "entity.add.cube",
            json!({ "parent": parent_bits }),
        );
        Entity::try_from_bits(child["entities"][0].as_u64().expect("an entity id"))
            .expect("a valid entity id")
    };
    app.world_mut()
        .get_mut::<Transform>(parent)
        .expect("a transform")
        .translation = Vec3::new(0.0, 10.0, 0.0);

    // No frame runs between the move and the call, so a stale global would
    // still place the cube at the origin.
    let outcome = app
        .world_mut()
        .run_system_cached_with(
            call_operator_handler,
            Some(json!({ "id": "brush.select_elements", "params": {
                "entity": entity.to_bits(), "kind": "vertex",
                "where": { "inside_box": { "min": [-1, 10, -1], "max": [1, 11, 1], "space": "world" } },
            } })),
        )
        .expect("the handler ran")
        .expect("dispatched");
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    let picked = app
        .world()
        .resource::<BrushSelection>()
        .sub(entity)
        .map(|sub| sub.vertices.len())
        .unwrap_or_default();
    assert_eq!(picked, 4, "the upper corners, lifted by the parent");
}

#[test]
fn the_topology_of_a_cube_reads_small_and_tracks_renumbering() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    let read = |app: &mut App, params: Value| handle(app, brush_topology_handler, params);

    let answer = read(
        &mut app,
        json!({ "entity": entity.to_bits(), "fields": ["positions", "polygons"] }),
    )
    .expect("reads");
    let size = answer.to_string().len();
    assert!(size < 4096, "{size} bytes: {answer}");
    assert_eq!(answer["positions"].as_array().unwrap().len(), 8);
    assert_eq!(answer["polygons"].as_array().unwrap().len(), 6);
    assert_eq!(answer["counts"]["edges"], 12);
    assert!(answer.get("edges").is_none());
    let revision = answer["revision"].clone();

    let all = read(
        &mut app,
        json!({ "entity": entity.to_bits(), "fields": ["edges", "normals", "colors"] }),
    )
    .expect("reads");
    assert_eq!(all["edges"].as_array().unwrap().len(), 12);
    assert_eq!(all["normals"].as_array().unwrap().len(), 6);
    assert!(all["colors"].is_null());

    select(
        &mut app,
        entity,
        json!({ "kind": "face", "where": { "normal": "+y" } }),
    );
    let selected = read(&mut app, json!({ "entity": entity.to_bits() })).expect("reads");
    assert_eq!(
        selected["revision"], revision,
        "selecting renumbers nothing"
    );

    call(&mut app, "brush.mesh.extrude_region", json!({}));
    let extruded = read(&mut app, json!({ "entity": entity.to_bits() })).expect("reads");
    assert_ne!(extruded["revision"], revision, "an extrusion renumbers");

    assert!(
        read(
            &mut app,
            json!({ "entity": entity.to_bits(), "fields": ["uvs"] })
        )
        .is_err()
    );
    let group = call(&mut app, "entity.add.group", json!({}));
    assert!(read(&mut app, json!({ "entity": group["entities"][0] })).is_err());
}

#[test]
fn the_select_menu_shows_only_while_editing_elements() {
    use jackdaw_editor_modelling::topology_ops::select_elements::{
        editing_elements, select_menu_rows,
    };

    let mut app = scene_app();
    let entity = cube(&mut app);
    assert!(!editing_elements(app.world()));

    select(
        &mut app,
        entity,
        json!({ "kind": "face", "where": { "normal": "+y" } }),
    );
    assert!(editing_elements(app.world()));
    let rows = select_menu_rows();
    for action in [
        r#"op:brush.select_elements?where={"normal":"+y"}"#,
        r#"op:brush.select_elements?where={"extreme":"max_y"}"#,
        "op:brush.select.loop",
    ] {
        assert!(
            rows.iter().any(|(row, _)| row == action),
            "no {action} row: {rows:?}"
        );
    }

    app.world_mut()
        .operator("brush.exit_edit_mode")
        .call()
        .expect("dispatches")
        .assert_finished();
    app.update();
    assert!(!editing_elements(app.world()));
}
