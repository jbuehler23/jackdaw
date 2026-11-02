//! Mesh edit operators run from parameters on the element selection: exact
//! amounts, created element counts, refusals that leave the scene alone, one
//! undo step each, a save round trip and batches.

use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::prelude::*;
use jackdaw::commands::CommandHistory;
use jackdaw::remote::server::{batch_handler, call_operator_handler};
use jackdaw_api::prelude::*;
use jackdaw_scene_types::Brush;
use serde_json::{Value, json};

const EPS: f32 = 1e-4;

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
) -> Value {
    let answer = app
        .world_mut()
        .run_system_cached_with(handler, Some(params))
        .expect("the handler ran")
        .unwrap_or_else(|err| panic!("refused: {}", err.message));
    app.update();
    answer
}

fn call(app: &mut App, id: &str, params: Value) -> Value {
    handle(
        app,
        call_operator_handler,
        json!({ "id": id, "params": params }),
    )
}

fn cube(app: &mut App) -> Entity {
    let outcome = call(app, "entity.add.cube", json!({ "name": "Block" }));
    let bits = outcome["entities"][0].as_u64().expect("an entity id");
    Entity::try_from_bits(bits).expect("a valid entity id")
}

fn select(app: &mut App, entity: Entity, kind: &str, rule: Value) {
    let outcome = call(
        app,
        "brush.select_elements",
        json!({ "entity": entity.to_bits(), "kind": kind, "where": rule }),
    );
    assert_eq!(outcome["result"], "finished", "{outcome}");
}

fn brush(app: &App, entity: Entity) -> Brush {
    app.world().get::<Brush>(entity).expect("a brush").clone()
}

fn counts(app: &App, entity: Entity) -> (usize, usize, usize) {
    let topology = brush(app, entity).topology;
    (
        topology.polygons.len(),
        topology.edges.len(),
        topology.vertices.len(),
    )
}

fn top(app: &App, entity: Entity) -> f32 {
    brush(app, entity)
        .topology
        .vertices
        .iter()
        .map(|vert| vert.position.y)
        .fold(f32::MIN, f32::max)
}

fn history_len(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

fn scene_text(app: &mut App) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), dir.path())
        .expect("the scene emits")
}

fn undo(app: &mut App) {
    app.world_mut()
        .operator("history.undo")
        .call()
        .expect("undo dispatches")
        .assert_finished();
    app.update();
}

fn redo(app: &mut App) {
    app.world_mut()
        .operator("history.redo")
        .call()
        .expect("redo dispatches")
        .assert_finished();
    app.update();
}

#[test]
fn a_one_shot_extrusion_is_one_undo_step() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    select(&mut app, entity, "face", json!({ "normal": "+y" }));
    let entries = history_len(&app);
    let before = scene_text(&mut app);

    app.world_mut()
        .operator("brush.mesh.extrude_region")
        .settings(CallOperatorSettings {
            creates_history_entry: true,
            ..default()
        })
        .call()
        .expect("dispatches")
        .assert_finished();
    app.update();
    assert_eq!(counts(&app, entity).2, 12);
    assert_eq!(history_len(&app), entries + 1, "one undo entry");
    let extruded = scene_text(&mut app);
    assert_ne!(extruded, before, "the document holds the extrusion");

    undo(&mut app);
    assert_eq!(counts(&app, entity), (6, 12, 8), "undo restores the cube");
    assert!((top(&app, entity) - 0.5).abs() < EPS);
    assert_eq!(scene_text(&mut app), before);

    redo(&mut app);
    assert_eq!(counts(&app, entity).2, 12, "redo extrudes again");
    assert!((top(&app, entity) - 1.0).abs() < EPS);
    assert_eq!(scene_text(&mut app), extruded);
}

#[test]
fn one_shot_mesh_edits_in_a_batch_undo_together() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    let before = scene_text(&mut app);
    let entries = history_len(&app);
    let bits = entity.to_bits();

    let answer = handle(
        &mut app,
        batch_handler,
        json!({ "calls": [
            { "id": "brush.select_elements", "params": {
                "entity": bits, "kind": "face", "where": { "normal": "+y" } } },
            { "id": "brush.mesh.extrude_region", "params": {} },
            { "id": "brush.select_elements", "params": {
                "entity": bits, "kind": "edge", "where": { "normal": "+y" } } },
            { "id": "brush.mesh.subdivide", "params": {} },
        ] }),
    );
    assert_eq!(
        answer["calls"].as_array().map(Vec::len),
        Some(4),
        "{answer}"
    );
    assert_eq!(history_len(&app), entries + 1, "one undo entry");

    undo(&mut app);
    assert_eq!(counts(&app, entity), (6, 12, 8), "undo restores the cube");
    assert_eq!(scene_text(&mut app), before);
}

fn reports(outcome: &Value) -> String {
    outcome["reports"]
        .as_array()
        .map(|reports| {
            reports
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_default()
}

#[track_caller]
fn assert_changed(outcome: &Value) {
    assert_eq!(outcome["result"], "finished", "{outcome}");
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
}

fn selected_faces(app: &App, entity: Entity) -> Vec<usize> {
    app.world()
        .resource::<jackdaw::brush::BrushSelection>()
        .sub(entity)
        .map(|sub| sub.faces.clone())
        .unwrap_or_default()
}

fn face_heights(app: &App, entity: Entity, face: usize) -> Vec<f32> {
    let shape = brush(app, entity);
    shape
        .topology
        .face_ring(face)
        .map(|vert| shape.topology.vertices[vert as usize].position.y)
        .collect()
}

/// The index of the cube edge running along x at the top back.
fn top_back_edge(app: &App, entity: Entity) -> usize {
    let shape = brush(app, entity);
    (0..shape.topology.edges.len())
        .find(|&edge| {
            let [a, b] = shape.topology.edges[edge].v;
            let pa = shape.topology.vertices[a as usize].position;
            let pb = shape.topology.vertices[b as usize].position;
            (pa.y - 0.5).abs() < EPS
                && (pb.y - 0.5).abs() < EPS
                && (pa.z - 0.5).abs() < EPS
                && (pb.z - 0.5).abs() < EPS
        })
        .expect("a top back edge")
}

fn select_edge(app: &mut App, entity: Entity, edge: usize) {
    let outcome = call(
        app,
        "brush.select_elements",
        json!({ "entity": entity.to_bits(), "kind": "edge", "indices": [edge] }),
    );
    assert_eq!(outcome["result"], "finished", "{outcome}");
}

#[test]
fn an_extrusion_by_distance_moves_the_top_face_exactly_that_far() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    select(&mut app, entity, "face", json!({ "normal": "+y" }));

    let outcome = call(&mut app, "brush.mesh.extrude", json!({ "distance": 0.35 }));
    assert_changed(&outcome);
    assert!(
        reports(&outcome).contains("added 4 faces, 8 edges and 4 vertices"),
        "{outcome}"
    );
    let top_face = *selected_faces(&app, entity)
        .first()
        .expect("the top stays picked");
    for height in face_heights(&app, entity, top_face) {
        assert!((height - 0.85).abs() < EPS, "top face at {height}");
    }

    let outcome = call(
        &mut app,
        "brush.mesh.extrude_region",
        json!({ "distance": -0.1 }),
    );
    assert_changed(&outcome);
    let top_face = selected_faces(&app, entity)[0];
    for height in face_heights(&app, entity, top_face) {
        assert!((height - 0.75).abs() < EPS, "pulled back in to {height}");
    }
}

#[test]
fn an_inset_by_amount_rings_the_face_with_four_quads() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    select(&mut app, entity, "face", json!({ "normal": "+y" }));

    let outcome = call(&mut app, "brush.mesh.inset", json!({ "amount": 0.1 }));
    assert_changed(&outcome);
    assert!(reports(&outcome).contains("added 4 faces"), "{outcome}");
    assert_eq!(counts(&app, entity), (10, 20, 12));
    let inner = selected_faces(&app, entity);
    assert_eq!(inner.len(), 1, "the inner face stays picked");
    let shape = brush(&app, entity);
    let first = shape.topology.face_ring(inner[0]).next().expect("a corner");
    let corner = shape.topology.vertices[first as usize].position;
    let pulled = Vec2::new(0.5, 0.5) - Vec2::new(corner.x.abs(), corner.z.abs());
    assert!(
        (pulled.length() - 0.1).abs() < EPS,
        "each corner moves 0.1 toward the middle: {corner}"
    );

    let outcome = call(
        &mut app,
        "brush.mesh.inset",
        json!({ "amount": 0.05, "depth": 0.2 }),
    );
    assert_changed(&outcome);
    let raised = selected_faces(&app, entity)[0];
    for height in face_heights(&app, entity, raised) {
        assert!((height - 0.7).abs() < EPS, "raised to {height}");
    }
}

#[test]
fn a_two_segment_bevel_on_a_cube_edge_adds_two_quads() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    let edge = top_back_edge(&app, entity);
    select_edge(&mut app, entity, edge);

    let outcome = call(
        &mut app,
        "brush.mesh.edge_bevel",
        json!({ "width": 0.05, "segments": 2 }),
    );
    assert_changed(&outcome);
    assert_eq!(counts(&app, entity), (8, 18, 12));
    let shape = brush(&app, entity);
    let top = shape
        .topology
        .vertices
        .iter()
        .filter(|vert| (vert.position.y - 0.5).abs() < EPS)
        .map(|vert| vert.position.z)
        .fold(f32::MIN, f32::max);
    assert!(
        (top - 0.45).abs() < EPS,
        "the top face stops 0.05 short: {top}"
    );

    let mut app = scene_app();
    let entity = cube(&mut app);
    let edge = top_back_edge(&app, entity);
    select_edge(&mut app, entity, edge);
    let flat = call(&mut app, "brush.mesh.edge_bevel", json!({ "width": 0.05 }));
    assert_changed(&flat);
    assert_eq!(counts(&app, entity).0, 7, "one segment is a flat chamfer");
}

#[test]
fn a_vertex_bevel_by_width_cuts_the_corner_off() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    call(
        &mut app,
        "brush.select_elements",
        json!({ "entity": entity.to_bits(), "kind": "vertex", "indices": [0] }),
    );
    let outcome = call(&mut app, "brush.mesh.vertex_bevel", json!({ "width": 0.1 }));
    assert_changed(&outcome);
    assert_eq!(counts(&app, entity), (7, 15, 10));
    assert_eq!(selected_faces(&app, entity), vec![6], "the new corner face");
}

#[test]
fn loop_cuts_take_a_count_or_a_factor() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    let three = call(
        &mut app,
        "brush.mesh.loop_cut_at",
        json!({ "entity": entity.to_bits(), "where": { "direction": "y" }, "count": 3 }),
    );
    assert_changed(&three);
    assert!(reports(&three).contains("added 12 faces"), "{three}");
    assert_eq!(counts(&app, entity).0, 18);
    let mut heights: Vec<f32> = brush(&app, entity)
        .topology
        .vertices
        .iter()
        .map(|vert| vert.position.y)
        .collect();
    heights.sort_by(f32::total_cmp);
    heights.dedup_by(|a, b| (*a - *b).abs() < EPS);
    assert_eq!(heights.len(), 5, "{heights:?}");
    for (height, expected) in heights.iter().zip([-0.5, -0.25, 0.0, 0.25, 0.5]) {
        assert!((height - expected).abs() < EPS, "{heights:?}");
    }

    let mut app = scene_app();
    let entity = cube(&mut app);
    let (edge, from) = {
        let shape = brush(&app, entity);
        let edge = (0..shape.topology.edges.len())
            .find(|&edge| {
                let [a, b] = shape.topology.edges[edge].v;
                let run = shape.topology.vertices[b as usize].position
                    - shape.topology.vertices[a as usize].position;
                run.x.abs() < EPS && run.z.abs() < EPS
            })
            .expect("an upright edge");
        let first = shape.topology.edges[edge].v[0] as usize;
        (edge, shape.topology.vertices[first].position.y)
    };
    let quarter = call(
        &mut app,
        "brush.mesh.loop_cut_at",
        json!({ "entity": entity.to_bits(), "edge": edge, "factor": 0.25 }),
    );
    assert_changed(&quarter);
    let expected = from + 0.25 * (-from - from);
    let at = brush(&app, entity)
        .topology
        .vertices
        .iter()
        .filter(|vert| (vert.position.y - expected).abs() < EPS)
        .count();
    assert_eq!(
        at, 4,
        "the loop sits a quarter of the way from the first vertex"
    );
}

#[test]
fn subdivide_with_two_cuts_splits_the_top_into_three_strips() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    select(
        &mut app,
        entity,
        "edge",
        json!({ "normal": "+y", "direction": "x" }),
    );
    let outcome = call(&mut app, "brush.mesh.subdivide", json!({ "cuts": 2 }));
    assert_changed(&outcome);
    assert_eq!(counts(&app, entity), (8, 18, 12));
    let picked = app
        .world()
        .resource::<jackdaw::brush::BrushSelection>()
        .sub(entity)
        .map(|sub| sub.edges.len())
        .unwrap_or_default();
    assert_eq!(picked, 2, "the two strip edges are picked");
}

#[test]
fn slides_move_by_the_given_fraction() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    call(
        &mut app,
        "brush.select_elements",
        json!({ "entity": entity.to_bits(), "kind": "vertex", "indices": [0] }),
    );
    let before = brush(&app, entity).topology.vertices[0].position;
    let outcome = call(&mut app, "brush.mesh.vertex_slide", json!({ "t": 0.25 }));
    assert_changed(&outcome);
    let after = brush(&app, entity).topology.vertices[0].position;
    assert!(
        ((after - before).length() - 0.25).abs() < EPS,
        "{before} to {after}"
    );

    let edge = top_back_edge(&app, entity);
    select_edge(&mut app, entity, edge);
    let [a, b] = brush(&app, entity).topology.edges[edge].v;
    let ends = |app: &App| {
        let shape = brush(app, entity);
        [
            shape.topology.vertices[a as usize].position,
            shape.topology.vertices[b as usize].position,
        ]
    };
    let before = ends(&app);
    let outcome = call(&mut app, "brush.mesh.edge_slide", json!({ "t": 0.25 }));
    assert_changed(&outcome);
    for (start, end) in before.into_iter().zip(ends(&app)) {
        assert!(
            ((end - start).length() - 0.25).abs() < EPS,
            "{start} to {end}"
        );
    }
}

#[test]
fn invalid_parameters_are_refused_and_leave_the_scene_byte_identical() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    let edge = top_back_edge(&app, entity);

    let face = json!({ "kind": "face", "where": { "normal": "+y" } });
    let one_edge = json!({ "kind": "edge", "indices": [edge] });
    let vertex = json!({ "kind": "vertex", "indices": [0] });
    for (pick, id, params) in [
        (&face, "brush.mesh.extrude", json!({ "distance": 0.0 })),
        (
            &face,
            "brush.mesh.extrude_region",
            json!({ "distance": 0.0 }),
        ),
        (&face, "brush.mesh.inset", json!({ "amount": 0.0 })),
        (&face, "brush.mesh.inset", json!({ "amount": 0.9 })),
        (&face, "brush.mesh.inset", json!({ "depth": 0.2 })),
        (&one_edge, "brush.mesh.edge_bevel", json!({ "width": -0.1 })),
        (&one_edge, "brush.mesh.edge_bevel", json!({ "width": 0.6 })),
        (
            &one_edge,
            "brush.mesh.edge_bevel",
            json!({ "width": 0.05, "segments": 0 }),
        ),
        (
            &one_edge,
            "brush.mesh.edge_bevel",
            json!({ "width": 0.05, "segments": 1000 }),
        ),
        (&vertex, "brush.mesh.vertex_bevel", json!({ "width": 0.0 })),
        (&one_edge, "brush.mesh.subdivide", json!({ "cuts": 0 })),
        (&one_edge, "brush.mesh.edge_slide", json!({ "t": 1.5 })),
        (&one_edge, "brush.mesh.edge_slide", json!({ "t": 0.0 })),
        (&vertex, "brush.mesh.vertex_slide", json!({ "t": -0.5 })),
        (
            &one_edge,
            "brush.mesh.loop_cut_at",
            json!({ "edge": edge, "count": 0 }),
        ),
        (
            &one_edge,
            "brush.mesh.loop_cut_at",
            json!({ "edge": edge, "factor": 1.0 }),
        ),
        (
            &one_edge,
            "brush.mesh.loop_cut_at",
            json!({ "edge": edge, "count": 2, "factor": 0.3 }),
        ),
    ] {
        let mut pick = pick.clone();
        pick["entity"] = json!(entity.to_bits());
        call(&mut app, "brush.select_elements", pick);
        let before = scene_text(&mut app);
        let entries = history_len(&app);
        let shape = counts(&app, entity);

        let outcome = call(&mut app, id, params.clone());
        assert_eq!(outcome["outcome"], "invalid", "{id} {params}: {outcome}");
        assert_eq!(outcome["result"], "cancelled", "{id} {params}: {outcome}");
        assert!(
            !outcome["warnings"].as_array().expect("warnings").is_empty(),
            "{id} {params}: the refusal says why"
        );
        assert_eq!(counts(&app, entity), shape, "{id} {params}");
        assert_eq!(history_len(&app), entries, "{id} {params}");
        assert_eq!(scene_text(&mut app), before, "{id} {params}");
    }
}

#[test]
fn a_parametric_edit_is_one_undo_step_and_redo_reapplies_it() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    let edge = top_back_edge(&app, entity);
    select_edge(&mut app, entity, edge);
    let before = scene_text(&mut app);

    for (id, params, grown) in [
        (
            "brush.mesh.edge_bevel",
            json!({ "width": 0.05, "segments": 2 }),
            (8, 18, 12),
        ),
        ("brush.mesh.subdivide", json!({ "cuts": 2 }), (6, 14, 10)),
    ] {
        select_edge(&mut app, entity, edge);
        let entries = history_len(&app);
        let outcome = call(&mut app, id, params.clone());
        assert_changed(&outcome);
        assert_eq!(history_len(&app), entries + 1, "{id}: one undo entry");
        assert_eq!(counts(&app, entity), grown, "{id}");

        undo(&mut app);
        assert_eq!(
            counts(&app, entity),
            (6, 12, 8),
            "{id}: undo restores the cube"
        );
        assert_eq!(scene_text(&mut app), before, "{id}");
        redo(&mut app);
        assert_eq!(counts(&app, entity), grown, "{id}: redo reapplies it");
        undo(&mut app);
    }

    select(&mut app, entity, "face", json!({ "normal": "+y" }));
    let entries = history_len(&app);
    assert_changed(&call(
        &mut app,
        "brush.mesh.extrude",
        json!({ "distance": 0.35 }),
    ));
    assert_eq!(
        history_len(&app),
        entries + 1,
        "a modal op run from parameters"
    );
    undo(&mut app);
    assert!((top(&app, entity) - 0.5).abs() < EPS);
    redo(&mut app);
    assert!((top(&app, entity) - 0.85).abs() < EPS);
}

#[test]
fn an_edit_saved_and_reopened_keeps_its_shape() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    let edge = top_back_edge(&app, entity);
    select_edge(&mut app, entity, edge);
    assert_changed(&call(
        &mut app,
        "brush.mesh.edge_bevel",
        json!({ "width": 0.05, "segments": 2 }),
    ));
    select(&mut app, entity, "face", json!({ "normal": "+y" }));
    assert_changed(&call(
        &mut app,
        "brush.mesh.extrude",
        json!({ "distance": 0.35 }),
    ));
    let shape = counts(&app, entity);

    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("edited.bsn");
    let text = jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), tmp.path())
        .expect("the scene emits");
    std::fs::write(&path, text).expect("write the scene");

    let mut app = util::editor_test_app();
    jackdaw::migrate_dialog::request_open_with_conversion(app.world_mut(), &path);
    for _ in 0..8 {
        app.update();
    }
    let reopened = app
        .world_mut()
        .query::<(Entity, &Name, &Brush)>()
        .iter(app.world())
        .find(|(_, name, _)| name.as_str() == "Block")
        .map(|(entity, ..)| entity)
        .expect("the reopened scene has the block");
    assert_eq!(counts(&app, reopened), shape);
    assert!((top(&app, reopened) - 0.85).abs() < EPS);
}

#[test]
fn an_empty_element_selection_changes_nothing() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    select(&mut app, entity, "face", json!({ "normal": "+y" }));
    call(
        &mut app,
        "brush.select_elements",
        json!({ "entity": entity.to_bits(), "kind": "face", "where": { "all": true },
                "mode": "remove" }),
    );
    assert!(selected_faces(&app, entity).is_empty());
    let entries = history_len(&app);
    let before = scene_text(&mut app);

    for (id, params) in [
        ("brush.mesh.extrude", json!({ "distance": 0.35 })),
        ("brush.mesh.extrude_region", json!({})),
        ("brush.mesh.inset", json!({ "amount": 0.1 })),
    ] {
        let outcome = call(&mut app, id, params);
        assert_eq!(outcome["result"], "cancelled", "{id}: {outcome}");
        assert_eq!(outcome["outcome"], "unchanged", "{id}: {outcome}");
        assert_eq!(history_len(&app), entries, "{id}: no history entry");
        assert_eq!(scene_text(&mut app), before, "{id}");
    }
}

#[test]
fn a_batch_with_parametric_modal_ops_runs_to_the_end() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    let bits = entity.to_bits();
    let entries = history_len(&app);

    let answer = handle(
        &mut app,
        batch_handler,
        json!({ "calls": [
            { "id": "brush.select_elements", "params": {
                "entity": bits, "kind": "face", "where": { "normal": "+y" } } },
            { "id": "brush.mesh.inset", "params": { "amount": 0.1 } },
            { "id": "brush.mesh.extrude", "params": { "distance": 0.35 } },
        ] }),
    );
    let calls = answer["calls"].as_array().expect("calls");
    assert_eq!(calls.len(), 3, "{answer}");
    for call in calls {
        assert_eq!(call["result"], "finished", "{answer}");
    }
    assert_eq!(calls[2]["outcome"], "changed", "{answer}");
    assert!((top(&app, entity) - 0.85).abs() < EPS);
    assert_eq!(history_len(&app), entries + 1, "one undo entry");
    let modal = app
        .world_mut()
        .query_filtered::<(), With<jackdaw_api_internal::lifecycle::ActiveModalOperator>>()
        .iter(app.world())
        .count();
    assert_eq!(modal, 0, "nothing is left holding the editor");
}

/// Tag each face of `entity` with its index in `uv_offset.x`, which an edit
/// copies along with the rest of a face's look.
fn tag_faces(app: &mut App, entity: Entity) {
    let mut brush = app.world_mut().get_mut::<Brush>(entity).expect("a brush");
    for (index, face) in brush.faces.iter_mut().enumerate() {
        face.uv_offset = Vec2::new(index as f32, 0.0);
    }
}

fn tag(shape: &Brush, face: usize) -> usize {
    shape.faces[face].uv_offset.x as usize
}

#[test]
fn an_inset_keeps_every_face_look_and_its_ring_copies_the_parent() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    tag_faces(&mut app, entity);
    let normals: Vec<Vec3> = {
        let shape = brush(&app, entity);
        (0..6)
            .map(|face| shape.topology.face_normal(face))
            .collect()
    };
    select(&mut app, entity, "face", json!({ "normal": "+y" }));
    assert_changed(&call(
        &mut app,
        "brush.mesh.inset",
        json!({ "amount": 0.1 }),
    ));

    let shape = brush(&app, entity);
    assert_eq!(shape.faces.len(), 10);
    for face in 0..shape.faces.len() {
        let normal = shape.topology.face_normal(face);
        assert!(
            normal.abs_diff_eq(normals[tag(&shape, face)], 1e-4),
            "face {face} looks like face {} but faces {normal}",
            tag(&shape, face)
        );
    }
}

#[test]
fn each_extruded_face_walls_copy_that_face() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    tag_faces(&mut app, entity);
    let (top, side) = {
        let shape = brush(&app, entity);
        let find = |normal: Vec3| {
            (0..6)
                .find(|&face| shape.topology.face_normal(face).abs_diff_eq(normal, 1e-4))
                .expect("a face")
        };
        (find(Vec3::Y), find(Vec3::X))
    };
    call(
        &mut app,
        "brush.select_elements",
        json!({ "entity": entity.to_bits(), "kind": "face", "indices": [top, side] }),
    );
    assert_changed(&call(
        &mut app,
        "brush.mesh.extrude",
        json!({ "distance": 0.3 }),
    ));

    let shape = brush(&app, entity);
    assert_eq!(shape.faces.len(), 14);
    for face in 0..shape.faces.len() {
        let centre = shape.topology.face_centroid(face);
        if centre.y > 0.5 + EPS {
            assert_eq!(tag(&shape, face), top, "face {face} at {centre}");
        } else if centre.x > 0.5 + EPS {
            assert_eq!(tag(&shape, face), side, "face {face} at {centre}");
        }
    }
}

#[test]
fn a_rounded_bevel_stays_watertight_and_is_refused_where_it_cannot_close() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    let edge = top_back_edge(&app, entity);
    select_edge(&mut app, entity, edge);
    assert_changed(&call(
        &mut app,
        "brush.mesh.edge_bevel",
        json!({ "width": 0.05, "segments": 3 }),
    ));
    let shape = brush(&app, entity);
    let mut uses = vec![0usize; shape.topology.edges.len()];
    for corner in &shape.topology.loops {
        uses[corner.edge as usize] += 1;
    }
    assert!(
        uses.iter().all(|&count| count == 2),
        "every edge borders two faces: {uses:?}"
    );

    let mut app = scene_app();
    let entity = cube(&mut app);
    assert_changed(&call(
        &mut app,
        "brush.mesh.loop_cut_at",
        json!({ "entity": entity.to_bits(), "where": { "direction": "y" } }),
    ));
    let ring = {
        let shape = brush(&app, entity);
        (0..shape.topology.edges.len())
            .find(|&edge| {
                let [a, b] = shape.topology.edges[edge].v;
                let ya = shape.topology.vertices[a as usize].position.y;
                let yb = shape.topology.vertices[b as usize].position.y;
                ya.abs() < EPS && yb.abs() < EPS
            })
            .expect("an edge of the new loop")
    };
    select_edge(&mut app, entity, ring);
    let before = scene_text(&mut app);
    let refused = call(
        &mut app,
        "brush.mesh.edge_bevel",
        json!({ "width": 0.05, "segments": 2 }),
    );
    assert_eq!(refused["outcome"], "invalid", "{refused}");
    assert_eq!(refused["result"], "cancelled", "{refused}");
    assert_eq!(scene_text(&mut app), before);
}
