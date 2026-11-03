//! `brush.mesh.randomize`: seeded vertex offsets with exact, repeatable
//! results, refusals that leave the scene alone, one undo step, a save round
//! trip, a closed surface and one call reaching several brushes.

use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::prelude::*;
use jackdaw::commands::CommandHistory;
use jackdaw::remote::server::call_operator_handler;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::selection::Selection;
use jackdaw_scene_types::Brush;
use serde_json::{Value, json};

const OP: &str = "brush.mesh.randomize";

fn randomize_app() -> App {
    let mut app = util::editor_test_app();
    app.world_mut()
        .operator("scene.new")
        .call()
        .expect("scene.new dispatches")
        .assert_finished();
    app.update();
    app
}

fn call(app: &mut App, id: &str, params: Value) -> Value {
    let answer = app
        .world_mut()
        .run_system_cached_with(
            call_operator_handler,
            Some(json!({ "id": id, "params": params })),
        )
        .expect("the handler ran")
        .unwrap_or_else(|err| panic!("refused: {}", err.message));
    app.update();
    answer
}

fn added(app: &mut App, id: &str, params: Value) -> Entity {
    let outcome = call(app, id, params);
    let bits = outcome["entities"][0].as_u64().expect("an entity id");
    Entity::try_from_bits(bits).expect("a valid entity id")
}

fn rock(app: &mut App) -> Entity {
    added(
        app,
        "entity.add.sphere",
        json!({ "kind": "ico", "name": "Rock" }),
    )
}

fn stone(app: &mut App, x: f32) -> Entity {
    added(
        app,
        "entity.add.cube",
        json!({ "name": format!("Stone {x}"), "pos": [x, 0.0, 0.0], "size": [0.4, 0.2, 0.4] }),
    )
}

fn positions(app: &App, entity: Entity) -> Vec<Vec3> {
    app.world()
        .get::<Brush>(entity)
        .expect("a brush")
        .topology
        .vertices
        .iter()
        .map(|vert| vert.position)
        .collect()
}

fn history_len(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

fn scene_text(app: &mut App) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), dir.path())
        .expect("the scene emits")
}

fn history(app: &mut App, id: &'static str) {
    app.world_mut()
        .operator(id)
        .call()
        .expect("history dispatches")
        .assert_finished();
    app.update();
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

fn rock_call(entity: Entity) -> Value {
    json!({ "entity": entity.to_bits(), "amount": 0.08, "seed": 7 })
}

#[track_caller]
fn assert_changed(outcome: &Value) {
    assert_eq!(outcome["result"], "finished", "{outcome}");
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
}

#[track_caller]
fn assert_closed(app: &App, entity: Entity) {
    let brush = app.world().get::<Brush>(entity).expect("a brush");
    assert_eq!(
        brush.topology.open_edges(),
        Ok(Vec::new()),
        "the brush stays watertight"
    );
}

fn displacements(before: &[Vec3], after: &[Vec3]) -> Vec<f32> {
    assert_eq!(before.len(), after.len());
    before
        .iter()
        .zip(after)
        .map(|(a, b)| a.distance(*b))
        .collect()
}

#[test]
fn a_randomized_ico_sphere_reads_as_a_rock_and_repeats_per_seed() {
    let mut app = randomize_app();
    let first = rock(&mut app);
    let second = rock(&mut app);
    let before = positions(&app, first);
    assert_eq!(before.len(), 12);

    let outcome = call(&mut app, OP, rock_call(first));
    assert_changed(&outcome);
    assert!(reports(&outcome).contains("moved 12 vertices"), "{outcome}");
    let after = positions(&app, first);
    history(&mut app, "history.undo");
    assert_eq!(positions(&app, first), before);
    assert_changed(&call(&mut app, OP, rock_call(first)));
    assert_eq!(
        positions(&app, first),
        after,
        "the same seed, the same rock"
    );

    assert_changed(&call(&mut app, OP, rock_call(second)));
    assert_ne!(
        positions(&app, second),
        after,
        "another brush, its own rock"
    );
    let moved = displacements(&before, &after);
    assert!(
        moved.iter().all(|&d| d > 0.0 && d <= 0.08 + 1e-5),
        "{moved:?}"
    );
    let brush = app.world().get::<Brush>(first).expect("a brush");
    assert_eq!(brush.topology.polygons.len(), 20);
    assert_eq!(brush.faces.len(), 20);
    assert_closed(&app, first);

    let third = rock(&mut app);
    assert_changed(&call(
        &mut app,
        OP,
        json!({ "entity": third.to_bits(), "amount": 0.08, "seed": 8 }),
    ));
    assert_ne!(positions(&app, third), after, "another seed, another rock");
}

#[test]
fn a_randomized_rock_saved_and_reopened_keeps_its_shape() {
    let mut app = randomize_app();
    let entity = rock(&mut app);
    assert_changed(&call(&mut app, OP, rock_call(entity)));
    let shaped = positions(&app, entity);

    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("rock.bsn");
    let saved = call(
        &mut app,
        "scene.save",
        json!({ "path": path.display().to_string() }),
    );
    assert_eq!(saved["result"], "finished", "{saved}");

    let mut app = util::editor_test_app();
    jackdaw::migrate_dialog::request_open_with_conversion(app.world_mut(), &path);
    for _ in 0..8 {
        app.update();
    }
    let reopened = app
        .world_mut()
        .query::<(Entity, &Name, &Brush)>()
        .iter(app.world())
        .find(|(_, name, _)| name.as_str() == "Rock")
        .map(|(entity, ..)| entity)
        .expect("the reopened scene has the rock");
    let again = positions(&app, reopened);
    assert_eq!(again.len(), shaped.len());
    for (a, b) in again.iter().zip(&shaped) {
        assert!(a.distance(*b) < 1e-5, "{a} is not {b}");
    }
    assert_closed(&app, reopened);
}

#[test]
fn one_call_wobbles_every_selected_stone_differently() {
    let mut app = randomize_app();
    let stones: Vec<Entity> = (0..4).map(|i| stone(&mut app, i as f32 * 0.5)).collect();
    let before: Vec<Vec<Vec3>> = stones.iter().map(|&s| positions(&app, s)).collect();
    app.world_mut().resource_mut::<Selection>().entities = stones.clone();
    let entries = history_len(&app);

    let outcome = call(&mut app, OP, json!({ "amount": 0.03, "seed": 1 }));
    assert_changed(&outcome);
    assert!(
        reports(&outcome).contains("moved 32 vertices on 4 brushes"),
        "{outcome}"
    );
    assert_eq!(history_len(&app), entries + 1, "one undo entry");
    let wobbles: Vec<Vec<Vec3>> = stones
        .iter()
        .zip(&before)
        .map(|(&s, before)| {
            assert_closed(&app, s);
            positions(&app, s)
                .iter()
                .zip(before)
                .map(|(a, b)| *a - *b)
                .collect()
        })
        .collect();
    for (index, wobble) in wobbles.iter().enumerate() {
        assert!(wobble.iter().all(|d| d.length() <= 0.03 + 1e-5));
        for other in &wobbles[index + 1..] {
            assert_ne!(wobble, other, "each stone gets its own wobble");
        }
    }
}

#[test]
fn only_the_selected_elements_move_when_some_are_selected() {
    let mut app = randomize_app();
    let entity = stone(&mut app, 0.0);
    let outcome = call(
        &mut app,
        "brush.select_elements",
        json!({ "entity": entity.to_bits(), "kind": "face", "where": { "normal": "+y" } }),
    );
    assert_eq!(outcome["result"], "finished", "{outcome}");
    let before = positions(&app, entity);

    let outcome = call(&mut app, OP, json!({ "amount": 0.02, "seed": 4 }));
    assert_changed(&outcome);
    assert!(reports(&outcome).contains("moved 4 vertices"), "{outcome}");
    for (a, b) in before.iter().zip(positions(&app, entity)) {
        if a.y < 0.0 {
            assert_eq!(*a, b, "the bottom stays");
        } else {
            assert!(a.distance(b) > 0.0 && a.distance(b) <= 0.02 + 1e-5);
        }
    }
    assert_closed(&app, entity);
}

#[test]
fn the_axis_mask_and_normal_only_steer_the_offsets() {
    let mut app = randomize_app();
    let flat = rock(&mut app);
    let before = positions(&app, flat);
    assert_changed(&call(
        &mut app,
        OP,
        json!({ "entity": flat.to_bits(), "amount": 0.1, "seed": 3, "axis_mask": [1.0, 0.0, 1.0] }),
    ));
    for (a, b) in before.iter().zip(positions(&app, flat)) {
        assert!((a.y - b.y).abs() < 1e-5, "{a} kept its height in {b}");
    }

    let puffed = rock(&mut app);
    assert_changed(&call(
        &mut app,
        OP,
        json!({ "entity": puffed.to_bits(), "amount": 0.1, "seed": 3, "normal_only": true }),
    ));
    for (a, b) in before.iter().zip(positions(&app, puffed)) {
        let offset = b - *a;
        assert!(
            offset.cross(a.normalize()).length() < 1e-4,
            "{offset} runs along the normal at {a}"
        );
    }
}

#[test]
fn invalid_parameters_are_refused_and_leave_the_scene_byte_identical() {
    let mut app = randomize_app();
    let entity = rock(&mut app);
    let id = entity.to_bits();

    for params in [
        json!({ "entity": id }),
        json!({ "entity": id, "amount": -0.1 }),
        json!({ "entity": id, "amount": 0.1, "axis_mask": [0.0, 0.0, 0.0] }),
        json!({ "entity": id, "amount": 0.1, "axis_mask": [1.0, 2.0, 1.0] }),
        json!({ "entity": id, "amount": 0.1, "axis_mask": [1.0, -1.0, 1.0] }),
        json!({ "entity": id, "amount": 0.1, "seed": -1 }),
        json!({ "entity": id, "amount": 0.1, "space": "sideways" }),
        json!({ "amount": 0.1 }),
    ] {
        app.world_mut().resource_mut::<Selection>().entities.clear();
        let before = scene_text(&mut app);
        let entries = history_len(&app);
        let shape = positions(&app, entity);

        let outcome = call(&mut app, OP, params.clone());
        assert_eq!(outcome["outcome"], "invalid", "{params}: {outcome}");
        assert_eq!(outcome["result"], "cancelled", "{params}: {outcome}");
        assert!(
            !outcome["warnings"].as_array().expect("warnings").is_empty(),
            "{params}: the refusal says why"
        );
        assert_eq!(positions(&app, entity), shape, "{params}");
        assert_eq!(history_len(&app), entries, "{params}");
        assert_eq!(scene_text(&mut app), before, "{params}");
    }
}

#[test]
fn a_zero_amount_is_unchanged_with_no_history_entry() {
    let mut app = randomize_app();
    let entity = rock(&mut app);
    let before = scene_text(&mut app);
    let entries = history_len(&app);

    let outcome = call(
        &mut app,
        OP,
        json!({ "entity": entity.to_bits(), "amount": 0.0 }),
    );
    assert_eq!(outcome["outcome"], "unchanged", "{outcome}");
    assert_eq!(history_len(&app), entries);
    assert_eq!(scene_text(&mut app), before);
}

#[test]
fn a_randomize_is_one_undo_step_and_redo_reapplies_it() {
    let mut app = randomize_app();
    let entity = rock(&mut app);
    let before = scene_text(&mut app);
    let shape = positions(&app, entity);
    let entries = history_len(&app);

    assert_changed(&call(&mut app, OP, rock_call(entity)));
    assert_eq!(history_len(&app), entries + 1, "one undo entry");
    let shaped = positions(&app, entity);
    let after = scene_text(&mut app);

    history(&mut app, "history.undo");
    assert_eq!(positions(&app, entity), shape);
    assert_eq!(scene_text(&mut app), before);

    history(&mut app, "history.redo");
    assert_eq!(positions(&app, entity), shaped);
    assert_eq!(scene_text(&mut app), after);
}

#[test]
fn a_remote_call_reports_the_outcome_and_moved_count() {
    let mut app = randomize_app();
    let entity = rock(&mut app);
    let outcome = app
        .world_mut()
        .run_system_cached_with(
            call_operator_handler,
            Some(json!({
                "id": OP,
                "params": { "entity": entity.to_bits(), "amount": 0.05, "seed": 2 },
            })),
        )
        .expect("the handler ran")
        .expect("the call is accepted");
    assert_eq!(outcome["result"], "finished", "{outcome}");
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    assert!(
        reports(&outcome).contains("Randomize Vertices: moved 12 vertices"),
        "{outcome}"
    );
}

fn node_id(app: &App, entity: Entity) -> u64 {
    app.world()
        .get::<jackdaw_scene_types::SceneNodeId>(entity)
        .expect("a scene node id")
        .0
}

#[test]
fn a_brush_gets_the_same_shape_alone_or_in_a_group() {
    let mut app = randomize_app();
    let stones: Vec<Entity> = (0..3).map(|i| stone(&mut app, i as f32 * 0.5)).collect();
    let middle = stones[1];
    let params = json!({ "entity": middle.to_bits(), "amount": 0.03, "seed": 5 });
    assert_changed(&call(&mut app, OP, params));
    let alone = positions(&app, middle);
    history(&mut app, "history.undo");

    app.world_mut().resource_mut::<Selection>().entities = stones.clone();
    assert_changed(&call(&mut app, OP, json!({ "amount": 0.03, "seed": 5 })));
    assert_eq!(
        positions(&app, middle),
        alone,
        "one shape alone or in a group"
    );
}

#[test]
fn a_brush_gets_the_same_shape_after_a_reload() {
    let mut app = randomize_app();
    let entity = rock(&mut app);
    let id = node_id(&app, entity);
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("plain.bsn");
    let saved = call(
        &mut app,
        "scene.save",
        json!({ "path": path.display().to_string() }),
    );
    assert_eq!(saved["result"], "finished", "{saved}");
    assert_changed(&call(&mut app, OP, rock_call(entity)));
    let shaped = positions(&app, entity);

    let mut app = util::editor_test_app();
    jackdaw::migrate_dialog::request_open_with_conversion(app.world_mut(), &path);
    for _ in 0..8 {
        app.update();
    }
    let reopened = app
        .world_mut()
        .query::<(Entity, &Name, &Brush)>()
        .iter(app.world())
        .find(|(_, name, _)| name.as_str() == "Rock")
        .map(|(entity, ..)| entity)
        .expect("the reopened scene has the rock");
    assert_eq!(
        node_id(&app, reopened),
        id,
        "the node id survives the reload"
    );
    assert_changed(&call(&mut app, OP, rock_call(reopened)));
    for (a, b) in positions(&app, reopened).iter().zip(&shaped) {
        assert!(a.distance(*b) < 1e-6, "{a} is not {b}");
    }
}

#[test]
fn a_face_turned_inside_out_is_refused() {
    let mut app = randomize_app();
    let slab = added(
        &mut app,
        "entity.add.cube",
        json!({ "name": "Thin", "size": [0.4, 0.02, 0.4] }),
    );
    // Each brush's offsets are keyed on its node id, which is minted from a
    // random base per process; a fixed id makes this slab's flip certain.
    app.world_mut()
        .entity_mut(slab)
        .insert(jackdaw_scene_types::SceneNodeId(
            jackdaw_scene_types::SPARSE_MIN + 1,
        ));
    app.update();
    let before = scene_text(&mut app);
    let entries = history_len(&app);

    let outcome = call(
        &mut app,
        OP,
        json!({ "entity": slab.to_bits(), "amount": 0.3, "seed": 0 }),
    );
    assert_eq!(outcome["outcome"], "invalid", "{outcome}");
    let warnings = outcome["warnings"].to_string();
    assert!(warnings.contains("inside out"), "{warnings}");
    assert_eq!(history_len(&app), entries);
    assert_eq!(scene_text(&mut app), before);
}

fn mirrored_cube(app: &mut App, clip: bool) -> Entity {
    use jackdaw_geometry::{MeshMirror, Modifier, ModifierEntry, ModifierStack};
    let entity = stone(app, 0.0);
    app.world_mut().entity_mut(entity).insert(ModifierStack {
        modifiers: vec![ModifierEntry::new(Modifier::Mirror(MeshMirror {
            offset: Vec3::new(0.2, 0.0, 0.0),
            clip,
            ..default()
        }))],
    });
    app.update();
    entity
}

#[test]
fn vertices_on_a_mirror_plane_stay_on_it_with_or_without_clip() {
    for clip in [false, true] {
        let mut app = randomize_app();
        let entity = mirrored_cube(&mut app, clip);
        let before = positions(&app, entity);
        assert_eq!(
            before.iter().filter(|p| (p.x - 0.2).abs() < 1e-6).count(),
            4
        );

        assert_changed(&call(
            &mut app,
            OP,
            json!({ "entity": entity.to_bits(), "amount": 0.05, "seed": 9 }),
        ));
        for (a, b) in before.iter().zip(positions(&app, entity)) {
            if (a.x - 0.2).abs() < 1e-6 {
                assert_eq!(b.x, a.x, "clip {clip}: {a} stays on the plane");
                assert!(a.distance(b) > 0.0, "clip {clip}: {a} still moves along it");
            } else {
                assert!(b.x <= 0.2, "clip {clip}: {b} stays on its side");
            }
        }
        assert_closed(&app, entity);
    }
}

fn edit_elements(app: &mut App, entity: Entity) {
    let outcome = call(
        app,
        "brush.select_elements",
        json!({ "entity": entity.to_bits(), "kind": "face", "where": { "normal": "+y" } }),
    );
    assert_eq!(outcome["result"], "finished", "{outcome}");
}

#[test]
fn element_mode_with_nothing_selected_changes_nothing() {
    let mut app = randomize_app();
    let entity = stone(&mut app, 0.0);
    edit_elements(&mut app, entity);
    app.world_mut()
        .resource_mut::<jackdaw::brush::BrushSelection>()
        .sub_mut(entity)
        .clear();
    let before = scene_text(&mut app);
    let entries = history_len(&app);

    for params in [
        json!({ "amount": 0.05 }),
        json!({ "entity": entity.to_bits(), "amount": 0.05 }),
    ] {
        let outcome = call(&mut app, OP, params.clone());
        assert_eq!(outcome["outcome"], "unchanged", "{params}: {outcome}");
        let warnings = outcome["warnings"].to_string();
        assert!(
            warnings.contains("nothing is selected"),
            "{params}: {warnings}"
        );
        assert_eq!(history_len(&app), entries);
        assert_eq!(scene_text(&mut app), before);
    }
}

#[test]
fn a_selection_naming_no_vertex_of_the_brush_is_refused() {
    let mut app = randomize_app();
    let entity = stone(&mut app, 0.0);
    edit_elements(&mut app, entity);
    {
        let mut selection = app
            .world_mut()
            .resource_mut::<jackdaw::brush::BrushSelection>();
        let sub = selection.sub_mut(entity);
        sub.clear();
        sub.vertices = vec![999];
    }
    let outcome = call(
        &mut app,
        OP,
        json!({ "entity": entity.to_bits(), "amount": 0.05 }),
    );
    assert_eq!(outcome["outcome"], "invalid", "{outcome}");
    let warnings = outcome["warnings"].to_string();
    assert!(warnings.contains("names no vertex"), "{warnings}");
}

/// Whether some face of `entity` has a corner off the plane through the rest.
fn has_a_bent_face(app: &App, entity: Entity) -> bool {
    let brush = app.world().get::<Brush>(entity).expect("a brush");
    (0..brush.topology.polygons.len()).any(|face| {
        let ring: Vec<Vec3> = brush
            .topology
            .face_ring(face)
            .map(|vert| brush.topology.vertices[vert as usize].position)
            .collect();
        let normal = (ring[1] - ring[0]).cross(ring[2] - ring[0]).normalize();
        ring.iter().any(|p| (*p - ring[0]).dot(normal).abs() > 1e-3)
    })
}

#[test]
fn a_jittered_cube_takes_a_clean_subtract() {
    let mut app = randomize_app();
    let block = added(&mut app, "entity.add.cube", json!({ "name": "Block" }));
    assert_changed(&call(
        &mut app,
        OP,
        json!({ "entity": block.to_bits(), "amount": 0.08, "seed": 3 }),
    ));
    assert!(has_a_bent_face(&app, block), "the jitter bends some quads");
    assert_closed(&app, block);
    let cutter = added(
        &mut app,
        "entity.add.cube",
        json!({ "name": "Cutter", "pos": [0.5, 0.1, 0.0], "size": [0.4, 0.4, 0.4] }),
    );

    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "target": block.to_bits(), "cutters": cutter.to_bits() }),
    );
    assert_changed(&outcome);
    let created: Vec<Entity> = outcome["created"]
        .as_array()
        .expect("created brushes")
        .iter()
        .map(|id| Entity::try_from_bits(id.as_u64().expect("an id")).expect("an entity"))
        .collect();
    assert!(!created.is_empty(), "{outcome}");
    for fragment in created {
        let brush = app.world().get::<Brush>(fragment).expect("a brush");
        assert!(brush.topology.polygons.len() >= 4);
        assert_eq!(brush.faces.len(), brush.topology.polygons.len());
        assert_closed(&app, fragment);
    }
}
