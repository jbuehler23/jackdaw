//! `brush.elements.transform`: the selected faces, edges or vertices moved,
//! rotated and scaled by value, with exact results, refusals that leave the
//! scene alone, one undo step, a save round trip and soft falloff.

use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::prelude::*;
use jackdaw::commands::CommandHistory;
use jackdaw::remote::server::call_operator_handler;
use jackdaw_api::prelude::*;
use jackdaw_scene_types::Brush;
use serde_json::{Value, json};

const OP: &str = "brush.elements.transform";
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

fn add_cube(app: &mut App, params: Value) -> Entity {
    let outcome = call(app, "entity.add.cube", params);
    let bits = outcome["entities"][0].as_u64().expect("an entity id");
    Entity::try_from_bits(bits).expect("a valid entity id")
}

fn cube(app: &mut App) -> Entity {
    add_cube(app, json!({ "name": "Block" }))
}

fn select(app: &mut App, entity: Entity, kind: &str, rule: Value) {
    let outcome = call(
        app,
        "brush.select_elements",
        json!({ "entity": entity.to_bits(), "kind": kind, "where": rule }),
    );
    assert_eq!(outcome["result"], "finished", "{outcome}");
}

fn select_in_box(app: &mut App, entity: Entity, min: [f32; 3], max: [f32; 3]) {
    select(
        app,
        entity,
        "vertex",
        json!({ "inside_box": { "min": min, "max": max } }),
    );
}

fn brush(app: &App, entity: Entity) -> Brush {
    app.world().get::<Brush>(entity).expect("a brush").clone()
}

fn positions(app: &App, entity: Entity) -> Vec<Vec3> {
    brush(app, entity)
        .topology
        .vertices
        .iter()
        .map(|vert| vert.position)
        .collect()
}

/// Positions with `y` near `height`.
fn at_height(app: &App, entity: Entity, height: f32) -> Vec<Vec3> {
    positions(app, entity)
        .into_iter()
        .filter(|p| (p.y - height).abs() < EPS)
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

#[track_caller]
fn assert_changed(outcome: &Value) {
    assert_eq!(outcome["result"], "finished", "{outcome}");
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
}

#[track_caller]
fn assert_near(actual: f32, expected: f32, what: &str) {
    assert!(
        (actual - expected).abs() < EPS,
        "{what}: {actual} is not {expected}"
    );
}

/// The four top corners lie on a square of half-width `half` at `height`.
#[track_caller]
fn assert_top_square(app: &App, entity: Entity, height: f32, half: f32) {
    let top = at_height(app, entity, height);
    assert_eq!(top.len(), 4, "four corners at {height}: {top:?}");
    for corner in top {
        assert_near(corner.x.abs(), half, "top corner x");
        assert_near(corner.z.abs(), half, "top corner z");
    }
}

fn taper(app: &mut App, entity: Entity) -> Value {
    select(app, entity, "face", json!({ "normal": "+y" }));
    call(app, OP, json!({ "scale": [0.6, 1.0, 0.6] }))
}

#[test]
fn scaling_the_top_face_tapers_a_cube_into_a_chimney() {
    let mut app = scene_app();
    let entity = cube(&mut app);

    let outcome = taper(&mut app, entity);
    assert_changed(&outcome);
    assert!(reports(&outcome).contains("moved 4 vertices"), "{outcome}");
    assert_top_square(&app, entity, 0.5, 0.3);
    assert_top_square(&app, entity, -0.5, 0.5);
    let shape = brush(&app, entity);
    assert_eq!(shape.topology.polygons.len(), 6);
    assert_eq!(shape.faces.len(), 6);
    let sides_lean_in = shape
        .faces
        .iter()
        .filter(|face| face.plane.normal.y > 0.1 && face.plane.normal.y < 0.9)
        .count();
    assert_eq!(sides_lean_in, 4, "the four sides lean in toward the top");
}

/// A cube with a gable roof: the ridge runs along z at x = 0, cut in three,
/// raised to y = 1.
fn roof(app: &mut App) -> Entity {
    let entity = add_cube(app, json!({ "name": "House" }));
    let shape = brush(app, entity);
    let edge_where = |along_x: bool| {
        (0..shape.topology.edges.len())
            .find(|&edge| {
                let [a, b] = shape.topology.edges[edge].v;
                let pa = shape.topology.vertices[a as usize].position;
                let pb = shape.topology.vertices[b as usize].position;
                let top = (pa.y - 0.5).abs() < EPS && (pb.y - 0.5).abs() < EPS;
                if along_x {
                    top && (pa.z - 0.5).abs() < EPS && (pb.z - 0.5).abs() < EPS
                } else {
                    top && (pa.x + 0.5).abs() < EPS && (pb.x + 0.5).abs() < EPS
                }
            })
            .expect("a top edge")
    };
    let along_x = edge_where(true);
    assert_changed(&call(
        app,
        "brush.mesh.loop_cut_at",
        json!({ "entity": entity.to_bits(), "edge": along_x }),
    ));
    let shape = brush(app, entity);
    let along_z = (0..shape.topology.edges.len())
        .find(|&edge| {
            let [a, b] = shape.topology.edges[edge].v;
            let pa = shape.topology.vertices[a as usize].position;
            let pb = shape.topology.vertices[b as usize].position;
            (pa.y - 0.5).abs() < EPS
                && (pb.y - 0.5).abs() < EPS
                && (pa.x + 0.5).abs() < EPS
                && (pb.x + 0.5).abs() < EPS
        })
        .expect("a top left edge");
    assert_changed(&call(
        app,
        "brush.mesh.loop_cut_at",
        json!({ "entity": entity.to_bits(), "edge": along_z, "count": 2 }),
    ));
    select_in_box(app, entity, [-0.01, 0.49, -1.0], [0.01, 0.51, 1.0]);
    assert_changed(&call(app, OP, json!({ "translate": [0.0, 0.5, 0.0] })));
    entity
}

fn ridge(app: &App, entity: Entity) -> Vec<Vec3> {
    let mut ridge: Vec<Vec3> = positions(app, entity)
        .into_iter()
        .filter(|p| p.x.abs() < EPS && p.y > 0.6)
        .collect();
    ridge.sort_by(|a, b| a.z.total_cmp(&b.z));
    ridge
}

fn sag(app: &mut App, entity: Entity) -> Value {
    select_in_box(app, entity, [-0.01, 0.99, -0.2], [0.01, 1.01, 0.2]);
    call(app, OP, json!({ "translate": [0.0, -0.1, 0.0] }))
}

#[track_caller]
fn assert_sagged(app: &App, entity: Entity) {
    let ridge = ridge(app, entity);
    assert_eq!(ridge.len(), 4, "{ridge:?}");
    assert_near(ridge[0].y, 1.0, "ridge end");
    assert_near(ridge[1].y, 0.9, "ridge middle");
    assert_near(ridge[2].y, 0.9, "ridge middle");
    assert_near(ridge[3].y, 1.0, "ridge end");
    assert_near(ridge[1].z, -1.0 / 6.0, "ridge middle z");
    assert_near(ridge[2].z, 1.0 / 6.0, "ridge middle z");
}

#[test]
fn lowering_a_ridge_loops_middle_vertices_sags_the_roof() {
    let mut app = scene_app();
    let entity = roof(&mut app);
    for point in ridge(&app, entity) {
        assert_near(point.y, 1.0, "the raised ridge");
    }

    let outcome = sag(&mut app, entity);
    assert_changed(&outcome);
    assert!(reports(&outcome).contains("moved 2 vertices"), "{outcome}");
    assert_sagged(&app, entity);
}

#[test]
fn a_rotation_turns_the_top_face_about_its_centre() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    select(&mut app, entity, "face", json!({ "normal": "+y" }));

    assert_changed(&call(&mut app, OP, json!({ "rotate": [45.0, 0.0, 0.0] })));
    let top = at_height(&app, entity, 0.5);
    assert_eq!(top.len(), 4);
    let half_diagonal = 0.5_f32.sqrt();
    for corner in top {
        let on_x = corner.z.abs() < EPS && (corner.x.abs() - half_diagonal).abs() < EPS;
        let on_z = corner.x.abs() < EPS && (corner.z.abs() - half_diagonal).abs() < EPS;
        assert!(on_x || on_z, "a corner turned onto an axis: {corner}");
    }
}

#[test]
fn the_pivot_is_the_centroid_the_brush_origin_or_a_point() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    select(&mut app, entity, "face", json!({ "normal": "+y" }));
    let before = scene_text(&mut app);

    let outcome = call(&mut app, OP, json!({ "scale": [1.0, 2.0, 1.0] }));
    assert_eq!(outcome["outcome"], "unchanged", "{outcome}");
    assert_eq!(
        scene_text(&mut app),
        before,
        "a flat face scaled about itself"
    );

    assert_changed(&call(
        &mut app,
        OP,
        json!({ "scale": [1.0, 2.0, 1.0], "pivot": "origin" }),
    ));
    assert_top_square(&app, entity, 1.0, 0.5);

    assert_changed(&call(
        &mut app,
        OP,
        json!({ "scale": [1.0, 2.0, 1.0], "pivot": "point", "point": [0.0, -0.5, 0.0] }),
    ));
    assert_top_square(&app, entity, 2.5, 0.5);
}

#[test]
fn world_space_follows_the_world_axes_and_local_space_the_brushs() {
    let mut app = scene_app();
    let entity = add_cube(
        &mut app,
        json!({ "name": "Turned", "rot": [90.0, 0.0, 0.0] }),
    );
    select(&mut app, entity, "face", json!({ "normal": "+y" }));

    assert_changed(&call(
        &mut app,
        OP,
        json!({ "translate": [1.0, 0.0, 0.0], "space": "local" }),
    ));
    let top = at_height(&app, entity, 0.5);
    assert_eq!(top.len(), 4);
    for corner in &top {
        assert!(
            (corner.x - 1.5).abs() < EPS || (corner.x - 0.5).abs() < EPS,
            "moved along the brush's x: {corner}"
        );
    }

    assert_changed(&call(
        &mut app,
        OP,
        json!({ "translate": [1.0, 0.0, 0.0], "space": "world" }),
    ));
    let turned = Quat::from_rotation_y(90_f32.to_radians());
    let local_step = turned.inverse() * Vec3::X;
    let top_after = at_height(&app, entity, 0.5);
    for corner in &top {
        let moved = *corner + local_step;
        assert!(
            top_after.iter().any(|p| p.distance(moved) < EPS),
            "{corner} moved along world x to {moved}: {top_after:?}"
        );
    }
}

#[test]
fn scale_zero_flattens_the_selection_onto_its_centroid() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    select_in_box(&mut app, entity, [0.49, 0.49, 0.49], [0.51, 0.51, 0.51]);
    assert_changed(&call(&mut app, OP, json!({ "translate": [0.0, 0.3, 0.0] })));

    select_in_box(&mut app, entity, [-1.0, 0.4, -1.0], [1.0, 0.9, 1.0]);
    let outcome = call(&mut app, OP, json!({ "scale": [1.0, 0.0, 1.0] }));
    assert_changed(&outcome);
    let top: Vec<Vec3> = positions(&app, entity)
        .into_iter()
        .filter(|p| p.y > 0.0)
        .collect();
    assert_eq!(top.len(), 4);
    for corner in top {
        assert_near(corner.y, 0.575, "flattened onto the mean height");
    }
}

#[test]
fn invalid_parameters_are_refused_and_leave_the_scene_byte_identical() {
    let mut app = scene_app();
    let entity = cube(&mut app);

    for params in [
        json!({}),
        json!({ "scale": [0.0, 1.0, 1.0] }),
        json!({ "scale": [1.0, 1.0, 0.0] }),
        json!({ "scale": [-1.0, 1.0, 1.0] }),
        json!({ "scale": [-1.0, -1.0, -1.0] }),
        json!({ "translate": [0.0, 1.0, 0.0], "pivot": "nowhere" }),
        json!({ "scale": [2.0, 2.0, 2.0], "pivot": "point" }),
        json!({ "scale": [2.0, 2.0, 2.0], "pivot": "origin", "point": [0.0, 0.0, 0.0] }),
        json!({ "translate": [0.0, 1.0, 0.0], "space": "sideways" }),
        json!({ "translate": [0.0, 1.0, 0.0], "falloff_radius": -1.0 }),
        json!({ "translate": [0.0, 1.0, 0.0], "falloff_radius": 0.0 }),
        json!({ "translate": [0.0, 1.0, 0.0], "falloff_radius": 1.0, "falloff_curve": "wobbly" }),
        json!({ "translate": [0.0, 1.0, 0.0], "falloff_curve": "linear" }),
    ] {
        select(&mut app, entity, "face", json!({ "normal": "+y" }));
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
fn a_face_left_with_no_area_is_named_in_the_refusal() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    select(&mut app, entity, "face", json!({ "normal": "+y" }));

    let outcome = call(&mut app, OP, json!({ "scale": [0.0, 1.0, 1.0] }));
    assert_eq!(outcome["outcome"], "invalid", "{outcome}");
    let warnings = outcome["warnings"].to_string();
    assert!(warnings.contains("no area"), "{warnings}");
}

#[test]
fn two_vertices_of_a_face_meeting_are_refused() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    select_in_box(&mut app, entity, [0.49, 0.49, 0.49], [0.51, 0.51, 0.51]);
    let shape = positions(&app, entity);

    let outcome = call(&mut app, OP, json!({ "translate": [-1.0, 0.0, 0.0] }));
    assert_eq!(outcome["outcome"], "invalid", "{outcome}");
    let warnings = outcome["warnings"].to_string();
    assert!(warnings.contains("two vertices in one place"), "{warnings}");
    assert_eq!(positions(&app, entity), shape);
}

#[test]
fn an_identity_on_a_brush_far_from_the_origin_changes_nothing() {
    let mut app = scene_app();
    let entity = add_cube(
        &mut app,
        json!({ "name": "Far", "pos": [5000.0, 0.0, 0.0] }),
    );
    select(&mut app, entity, "face", json!({ "normal": "+y" }));
    let before = scene_text(&mut app);

    let outcome = call(&mut app, OP, json!({ "scale": [1.0, 1.0, 1.0] }));
    assert_eq!(outcome["outcome"], "unchanged", "{outcome}");
    assert_eq!(scene_text(&mut app), before);

    assert_changed(&call(
        &mut app,
        OP,
        json!({ "translate": [0.0, 0.25, 0.0] }),
    ));
    assert_top_square(&app, entity, 0.75, 0.5);
}

#[test]
fn a_mirror_plane_stops_vertices_crossing_it() {
    use jackdaw_geometry::{MeshMirror, Modifier, ModifierEntry, ModifierStack};
    let mut app = scene_app();
    let entity = cube(&mut app);
    app.world_mut().entity_mut(entity).insert(ModifierStack {
        modifiers: vec![ModifierEntry::new(Modifier::Mirror(MeshMirror::default()))],
    });
    app.update();
    select_in_box(&mut app, entity, [0.49, 0.49, -1.0], [0.51, 0.51, 1.0]);

    assert_changed(&call(
        &mut app,
        OP,
        json!({ "translate": [-1.0, 0.0, 0.0] }),
    ));
    let mut top: Vec<f32> = at_height(&app, entity, 0.5).iter().map(|p| p.x).collect();
    top.sort_by(f32::total_cmp);
    assert_eq!(top.len(), 4);
    assert_near(top[0], -0.5, "the far side stays");
    assert_near(top[1], -0.5, "the far side stays");
    assert_near(top[2], 0.0, "held on the plane");
    assert_near(top[3], 0.0, "held on the plane");
}

#[test]
fn every_brush_being_edited_moves_about_one_shared_centroid_in_one_step() {
    let mut app = scene_app();
    let left = cube(&mut app);
    let right = add_cube(&mut app, json!({ "name": "Right", "pos": [3.0, 0.0, 0.0] }));
    select(&mut app, right, "face", json!({ "normal": "+y" }));
    let right_top = app
        .world()
        .resource::<jackdaw::brush::BrushSelection>()
        .sub(right)
        .map(|sub| sub.faces.clone())
        .expect("the right top is picked");
    select(&mut app, left, "face", json!({ "normal": "+y" }));
    app.world_mut()
        .resource_mut::<jackdaw::brush::BrushSelection>()
        .sub_mut(right)
        .faces = right_top;
    let before = scene_text(&mut app);
    let entries = history_len(&app);

    let outcome = call(&mut app, OP, json!({ "scale": [2.0, 1.0, 1.0] }));
    assert_changed(&outcome);
    assert!(
        reports(&outcome).contains("moved 8 vertices on 2 brushes"),
        "{outcome}"
    );
    let xs = |app: &App, entity: Entity| {
        let mut xs: Vec<f32> = at_height(app, entity, 0.5).iter().map(|p| p.x).collect();
        xs.sort_by(f32::total_cmp);
        xs.dedup_by(|a, b| (*a - *b).abs() < EPS);
        xs
    };
    let left_xs = xs(&app, left);
    let right_xs = xs(&app, right);
    assert_eq!(left_xs.len(), 2, "{left_xs:?}");
    assert_eq!(right_xs.len(), 2, "{right_xs:?}");
    assert_near(left_xs[0], -2.5, "left top, about x = 1.5");
    assert_near(left_xs[1], -0.5, "left top, about x = 1.5");
    assert_near(right_xs[0], 0.5, "right top, local");
    assert_near(right_xs[1], 2.5, "right top, local");
    assert_eq!(history_len(&app), entries + 1, "one undo entry");

    history(&mut app, "history.undo");
    assert_top_square(&app, left, 0.5, 0.5);
    assert_top_square(&app, right, 0.5, 0.5);
    assert_eq!(scene_text(&mut app), before);
}

#[test]
fn a_transform_is_one_undo_step_and_redo_reapplies_it() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    let before = scene_text(&mut app);
    let entries = history_len(&app);

    assert_changed(&taper(&mut app, entity));
    assert_eq!(history_len(&app), entries + 1, "one undo entry");
    let tapered = scene_text(&mut app);

    history(&mut app, "history.undo");
    assert_top_square(&app, entity, 0.5, 0.5);
    assert_eq!(scene_text(&mut app), before);

    history(&mut app, "history.redo");
    assert_top_square(&app, entity, 0.5, 0.3);
    assert_eq!(scene_text(&mut app), tapered);
}

#[test]
fn a_taper_and_a_sag_saved_and_reopened_keep_their_shape() {
    let mut app = scene_app();
    let chimney = cube(&mut app);
    assert_changed(&taper(&mut app, chimney));
    let house = roof(&mut app);
    assert_changed(&sag(&mut app, house));

    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("village.bsn");
    let saved = call(
        &mut app,
        "scene.save",
        json!({ "path": path.display().to_string() }),
    );
    assert_eq!(saved["result"], "finished", "{saved}");
    assert!(path.exists(), "scene.save wrote {}", path.display());

    let mut app = util::editor_test_app();
    jackdaw::migrate_dialog::request_open_with_conversion(app.world_mut(), &path);
    for _ in 0..8 {
        app.update();
    }
    let named = |app: &mut App, wanted: &str| {
        app.world_mut()
            .query::<(Entity, &Name, &Brush)>()
            .iter(app.world())
            .find(|(_, name, _)| name.as_str() == wanted)
            .map(|(entity, ..)| entity)
            .unwrap_or_else(|| panic!("the reopened scene has {wanted}"))
    };
    let chimney = named(&mut app, "Block");
    let house = named(&mut app, "House");
    assert_top_square(&app, chimney, 0.5, 0.3);
    assert_sagged(&app, house);
}

fn lift_one_corner(app: &mut App, entity: Entity, falloff: Value) {
    select_in_box(app, entity, [0.49, 0.49, 0.49], [0.51, 0.51, 0.51]);
    let mut params = json!({ "translate": [0.0, 1.0, 0.0] });
    if let (Some(params), Some(falloff)) = (params.as_object_mut(), falloff.as_object()) {
        params.extend(falloff.clone());
    }
    assert_changed(&call(app, OP, params));
}

fn height_at(app: &App, entity: Entity, x: f32, y: f32, z: f32) -> f32 {
    positions(app, entity)
        .into_iter()
        .find(|p| (p.x - x).abs() < EPS && (p.z - z).abs() < EPS && (p.y > 0.0) == (y > 0.0))
        .map(|p| p.y)
        .unwrap_or_else(|| panic!("a vertex over ({x}, {y}, {z})"))
}

#[test]
fn a_soft_falloff_carries_nearby_vertices_part_of_the_way() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    lift_one_corner(
        &mut app,
        entity,
        json!({ "falloff_radius": 1.5, "falloff_curve": "linear" }),
    );
    let near = 1.0 - 1.0 / 1.5;
    let diagonal = 1.0 - 2_f32.sqrt() / 1.5;
    assert_near(
        height_at(&app, entity, 0.5, 0.5, 0.5),
        1.5,
        "the picked corner",
    );
    assert_near(
        height_at(&app, entity, -0.5, 0.5, 0.5),
        0.5 + near,
        "one edge away",
    );
    assert_near(
        height_at(&app, entity, 0.5, -0.5, 0.5),
        -0.5 + near,
        "below it",
    );
    assert_near(
        height_at(&app, entity, -0.5, 0.5, -0.5),
        0.5 + diagonal,
        "across the face",
    );
    assert_near(
        height_at(&app, entity, -0.5, -0.5, -0.5),
        -0.5,
        "out of reach",
    );
}

#[test]
fn the_default_falloff_curve_is_smooth() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    lift_one_corner(&mut app, entity, json!({ "falloff_radius": 1.5 }));
    let t = 1.0 - 1.0 / 1.5;
    let smooth = t * t * (3.0 - 2.0 * t);
    assert_near(
        height_at(&app, entity, -0.5, 0.5, 0.5),
        0.5 + smooth,
        "one edge away",
    );
}

#[test]
fn without_a_falloff_only_the_selection_moves() {
    let mut app = scene_app();
    let entity = cube(&mut app);
    lift_one_corner(&mut app, entity, json!({}));
    assert_near(
        height_at(&app, entity, 0.5, 0.5, 0.5),
        1.5,
        "the picked corner",
    );
    assert_near(
        height_at(&app, entity, -0.5, 0.5, 0.5),
        0.5,
        "its neighbour",
    );
}
