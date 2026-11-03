//! `mesh.add_brush` from an outline: points and arcs in a named plane, depth,
//! taper and depth slices, refusals that leave the scene alone, history, a
//! save round trip, and the brushes working as CSG cutters.

use crate::util;

use bevy::prelude::*;
use jackdaw::commands::CommandHistory;
use jackdaw::remote::server::call_operator_handler;
use jackdaw_scene_types::Brush;
use serde_json::{Value, json};

use super::csg_ops::{call, csg_app, cube, ids};

const EPS: f32 = 1e-4;

fn added(app: &mut App, params: Value) -> Entity {
    let outcome = call(app, "mesh.add_brush", params);
    assert_eq!(outcome["result"], "finished", "{outcome}");
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    let entities = ids(&outcome["entities"]);
    assert_eq!(entities.len(), 1, "{outcome}");
    entities[0]
}

fn world_vertices(app: &App, entity: Entity) -> Vec<Vec3> {
    let global = app
        .world()
        .get::<GlobalTransform>(entity)
        .expect("a transform");
    app.world()
        .get::<Brush>(entity)
        .expect("a brush")
        .topology
        .vertices
        .iter()
        .map(|vert| global.transform_point(vert.position))
        .collect()
}

fn polygons(app: &App, entity: Entity) -> usize {
    app.world()
        .get::<Brush>(entity)
        .expect("a brush")
        .topology
        .polygons
        .len()
}

/// The enclosed volume in world space, positive when every face winds
/// outward.
fn volume(app: &App, entity: Entity) -> f32 {
    let global = app
        .world()
        .get::<GlobalTransform>(entity)
        .expect("a transform");
    let brush = app.world().get::<Brush>(entity).expect("a brush");
    let topology = &brush.topology;
    let mut total = 0.0;
    for poly in &topology.polygons {
        let start = poly.loop_start as usize;
        let ring: Vec<Vec3> = topology.loops[start..start + poly.loop_total as usize]
            .iter()
            .map(|corner| global.transform_point(topology.vertices[corner.vert as usize].position))
            .collect();
        for i in 1..ring.len() - 1 {
            total += ring[0].dot(ring[i].cross(ring[i + 1])) / 6.0;
        }
    }
    total
}

fn bounds(points: &[Vec3]) -> (Vec3, Vec3) {
    points
        .iter()
        .fold((Vec3::MAX, Vec3::MIN), |(min, max), point| {
            (min.min(*point), max.max(*point))
        })
}

#[track_caller]
fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1e-3,
        "expected {expected}, got {actual}"
    );
}

#[track_caller]
fn assert_vec(actual: Vec3, expected: Vec3) {
    assert!(
        actual.abs_diff_eq(expected, EPS),
        "expected {expected}, got {actual}"
    );
}

fn scene_text(app: &mut App) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), dir.path())
        .expect("the scene emits")
}

fn history_len(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

fn brush_named(app: &mut App, name: &str) -> Option<Entity> {
    app.world_mut()
        .query_filtered::<(Entity, &Name), With<Brush>>()
        .iter(app.world())
        .find(|(_, found)| found.as_str() == name)
        .map(|(entity, _)| entity)
}

fn gable() -> Value {
    json!([[-1.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.5], [-1.0, 1.0]])
}

/// A door: a rectangle from below the ground to 1, under a half circle of
/// radius 0.5 sampled in eight segments.
fn arch() -> Value {
    json!([
        [-0.5, -1.0],
        [0.5, -1.0],
        { "arc": { "center": [0.0, 1.0], "radius": 0.5, "from_deg": 0.0, "to_deg": 180.0, "segments": 8 } },
    ])
}

fn half_disc_area(radius: f32, segments: usize) -> f32 {
    let step = std::f32::consts::PI / segments as f32;
    segments as f32 * 0.5 * radius * radius * step.sin()
}

#[test]
fn a_gable_pentagon_in_xy_builds_its_exact_points_and_volume() {
    let mut app = csg_app();
    let gable = added(
        &mut app,
        json!({ "outline": gable(), "plane": "xy", "depth": 0.5, "name": "Gable" }),
    );
    assert_eq!(polygons(&app, gable), 7, "two caps and five sides");
    let verts = world_vertices(&app, gable);
    assert_eq!(verts.len(), 10);
    let (min, max) = bounds(&verts);
    assert_vec(min, Vec3::new(-1.0, 0.0, -0.25));
    assert_vec(max, Vec3::new(1.0, 1.5, 0.25));
    assert!(
        verts
            .iter()
            .any(|vert| vert.abs_diff_eq(Vec3::new(0.0, 1.5, 0.25), EPS)),
        "the ridge point sits on the far cap: {verts:?}"
    );
    assert_close(volume(&app, gable), 2.5 * 0.5);
    assert_eq!(
        app.world().get::<Name>(gable).map(Name::as_str),
        Some("Gable")
    );
}

#[test]
fn each_plane_names_the_axes_u_and_v_run_along() {
    let mut app = csg_app();
    let outline = json!([[0.0, 0.0], [2.0, 0.0], [2.0, 1.0], [0.0, 1.0]]);
    for (plane, min, max) in [
        ("xy", Vec3::new(0.0, 0.0, -0.5), Vec3::new(2.0, 1.0, 0.5)),
        ("xz", Vec3::new(0.0, -0.5, 0.0), Vec3::new(2.0, 0.5, 1.0)),
        ("yz", Vec3::new(-0.5, 0.0, 0.0), Vec3::new(0.5, 2.0, 1.0)),
    ] {
        let brush = added(&mut app, json!({ "outline": outline, "plane": plane }));
        let (found_min, found_max) = bounds(&world_vertices(&app, brush));
        assert_vec(found_min, min);
        assert_vec(found_max, max);
        assert_close(volume(&app, brush), 2.0);
    }
}

#[test]
fn a_clockwise_concave_outline_builds_one_outward_facing_brush() {
    let mut app = csg_app();
    let ell = added(
        &mut app,
        json!({
            "outline": [[0.0, 0.0], [0.0, 2.0], [1.0, 2.0], [1.0, 1.0], [2.0, 1.0], [2.0, 0.0]],
            "depth": 2.0,
        }),
    );
    assert_eq!(polygons(&app, ell), 8);
    assert_close(volume(&app, ell), 3.0 * 2.0);
    assert_eq!(
        app.world().get::<Name>(ell).map(Name::as_str),
        Some("Brush")
    );
}

#[test]
fn an_arc_item_samples_its_segments_and_joins_the_points_beside_it() {
    let mut app = csg_app();
    let door = added(&mut app, json!({ "outline": arch(), "plane": "xy" }));
    // Two rectangle corners and nine arc points.
    assert_eq!(world_vertices(&app, door).len(), 2 * 11);
    assert_eq!(polygons(&app, door), 2 + 11);
    let (min, max) = bounds(&world_vertices(&app, door));
    assert_vec(min, Vec3::new(-0.5, -1.0, -0.5));
    assert_vec(max, Vec3::new(0.5, 1.5, 0.5));
    assert_close(volume(&app, door), 1.0 * 2.0 + half_disc_area(0.5, 8));
}

#[test]
fn a_full_turn_arc_alone_is_a_closed_ring() {
    let mut app = csg_app();
    let disc = added(
        &mut app,
        json!({
            "outline": [{ "arc": { "center": [0.0, 0.0], "radius": 1.0, "from_deg": 0.0, "to_deg": 360.0, "segments": 6 } }],
        }),
    );
    assert_eq!(world_vertices(&app, disc).len(), 12);
    assert_eq!(polygons(&app, disc), 8);
}

#[test]
fn taper_scales_the_far_cap_about_the_centroid_and_zero_ends_in_a_point() {
    let mut app = csg_app();
    let square = json!([[1.0, 1.0], [3.0, 1.0], [3.0, 3.0], [1.0, 3.0]]);
    let frustum = added(
        &mut app,
        json!({ "outline": square, "plane": "xz", "depth": 2.0, "taper": 0.5 }),
    );
    let verts = world_vertices(&app, frustum);
    let (top_min, top_max) = bounds(
        &verts
            .iter()
            .copied()
            .filter(|vert| vert.y > 0.0)
            .collect::<Vec<_>>(),
    );
    assert_vec(top_min, Vec3::new(1.5, 1.0, 1.5));
    assert_vec(top_max, Vec3::new(2.5, 1.0, 2.5));
    // A frustum: h / 3 * (A1 + A2 + sqrt(A1 * A2)).
    assert_close(volume(&app, frustum), 2.0 / 3.0 * (4.0 + 1.0 + 2.0));

    let spire = added(
        &mut app,
        json!({ "outline": square, "plane": "xz", "depth": 3.0, "taper": 0.0 }),
    );
    let verts = world_vertices(&app, spire);
    assert_eq!(verts.len(), 5);
    assert_eq!(polygons(&app, spire), 5);
    assert!(
        verts
            .iter()
            .any(|vert| vert.abs_diff_eq(Vec3::new(2.0, 1.5, 2.0), EPS)),
        "{verts:?}"
    );
    assert_close(volume(&app, spire), 4.0 * 3.0 / 3.0);
}

#[test]
fn depth_segments_slice_the_sides_into_rings() {
    let mut app = csg_app();
    let square = json!([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
    let sliced = added(
        &mut app,
        json!({ "outline": square, "depth": 3.0, "depth_segments": 3 }),
    );
    let verts = world_vertices(&app, sliced);
    assert_eq!(verts.len(), 4 * 4);
    assert_eq!(polygons(&app, sliced), 2 + 4 * 3);
    let mut heights: Vec<f32> = verts.iter().map(|vert| vert.y).collect();
    heights.sort_by(f32::total_cmp);
    heights.dedup_by(|a, b| (*a - *b).abs() < EPS);
    assert_eq!(heights.len(), 4);
    for (found, expected) in heights.iter().zip([-1.5, -0.5, 0.5, 1.5]) {
        assert_close(*found, expected);
    }
    assert_close(volume(&app, sliced), 3.0);

    let tapered = added(
        &mut app,
        json!({ "outline": square, "depth": 2.0, "depth_segments": 2, "taper": 0.0 }),
    );
    assert_eq!(world_vertices(&app, tapered).len(), 4 * 2 + 1);
    assert_eq!(polygons(&app, tapered), 1 + 4 * 2);
    assert_close(volume(&app, tapered), 2.0 / 3.0);
}

#[test]
fn one_call_places_turns_names_and_parents_the_brush() {
    let mut app = csg_app();
    let house = call(&mut app, "entity.add.group", json!({ "name": "House" }));
    let house = ids(&house["entities"])[0];
    let gable = added(
        &mut app,
        json!({
            "outline": gable(),
            "plane": "xy",
            "depth": 0.5,
            "pos": [0.0, 2.0, 3.0],
            "rot": [90.0, 0.0, 0.0],
            "parent": house.to_bits(),
            "name": "East Gable",
        }),
    );
    assert_eq!(
        app.world().get::<ChildOf>(gable).map(ChildOf::parent),
        Some(house)
    );
    let transform = *app.world().get::<Transform>(gable).expect("a transform");
    assert_vec(transform.translation, Vec3::new(0.0, 2.0, 3.0));
    let (yaw, _, _) = transform.rotation.to_euler(EulerRot::YXZ);
    assert_close(yaw, 90f32.to_radians());
    let (min, max) = bounds(&world_vertices(&app, gable));
    assert_vec(min, Vec3::new(-0.25, 2.0, 2.0));
    assert_vec(max, Vec3::new(0.25, 3.5, 4.0));
}

#[test]
fn invalid_outlines_are_refused_and_leave_the_scene_byte_identical() {
    let mut app = csg_app();
    added(&mut app, json!({ "outline": gable(), "name": "Keep" }));
    let before = scene_text(&mut app);
    let entries = history_len(&app);
    let square = json!([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);

    for params in [
        json!({}),
        json!({ "outline": [[0.0, 0.0], [1.0, 0.0]] }),
        json!({ "outline": [[0.0, 0.0], [1.0, 0.0], [1.0, 0.0], [0.0, 0.0]] }),
        json!({ "outline": [[0.0, 0.0], [1.0, 1.0], [2.0, 2.0]] }),
        json!({ "outline": [[0.0, 0.0], [2.0, 2.0], [2.0, 0.0], [0.0, 1.0]] }),
        json!({ "outline": [[0.0, 0.0], [1.0], [0.0, 1.0]] }),
        json!({ "outline": "not a list" }),
        json!({ "outline": [[0.0, 0.0], { "line": [1.0, 1.0] }, [0.0, 1.0]] }),
        json!({ "outline": [[0.0, 0.0], { "arc": { "center": [0.0, 0.0], "radius": 0.0, "from_deg": 0.0, "to_deg": 90.0 } }] }),
        json!({ "outline": [[0.0, 0.0], { "arc": { "center": [0.0, 0.0], "radius": 1.0, "from_deg": 0.0, "to_deg": 0.0 } }] }),
        json!({ "outline": [{ "arc": { "center": [0.0, 0.0], "radius": 1.0, "from_deg": 0.0, "to_deg": 360.0, "segments": 0 } }] }),
        json!({ "outline": square, "depth": 0.0 }),
        json!({ "outline": square, "depth": -1.0 }),
        json!({ "outline": square, "taper": 1.5 }),
        json!({ "outline": square, "taper": -0.1 }),
        json!({ "outline": square, "depth_segments": 0 }),
        json!({ "outline": square, "depth_segments": 65 }),
        json!({ "outline": square, "depth": 0.005 }),
        json!({ "outline": square, "depth": 1e300 }),
        json!({
            "outline": [{ "arc": { "center": [0.0, 0.0], "radius": 1.0, "from_deg": 0.0, "to_deg": 360.0, "segments": 256 } }],
            "depth_segments": 8,
        }),
        json!({ "outline": square, "plane": "xw" }),
        json!({ "outline": square, "parent": "NoSuchNode" }),
        json!({ "outline": square, "name": "  " }),
        json!({ "outline": square, "radius": 2.0 }),
    ] {
        let outcome = call(&mut app, "mesh.add_brush", params.clone());
        assert_eq!(outcome["outcome"], "invalid", "{params}: {outcome}");
        assert_eq!(outcome["result"], "cancelled", "{params}: {outcome}");
        assert_eq!(outcome["entities"], json!([]), "{params}: {outcome}");
        assert!(
            !outcome["warnings"].as_array().expect("warnings").is_empty(),
            "{params}: the refusal says why: {outcome}"
        );
        assert_eq!(history_len(&app), entries, "{params}");
        assert_eq!(scene_text(&mut app), before, "{params}");
    }
}

#[test]
fn one_undo_takes_the_brush_back_and_redo_reapplies_it() {
    let mut app = csg_app();
    let entries = history_len(&app);
    added(
        &mut app,
        json!({ "outline": gable(), "plane": "xy", "depth": 0.5, "taper": 0.5, "name": "Roof End" }),
    );
    assert_eq!(history_len(&app), entries + 1, "one undo entry");

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    app.update();
    assert!(
        brush_named(&mut app, "Roof End").is_none(),
        "undo removes it"
    );

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.redo(world));
    app.update();
    let roof = brush_named(&mut app, "Roof End").expect("redo brings it back");
    assert_eq!(world_vertices(&app, roof).len(), 10);
    let verts = world_vertices(&app, roof);
    let (min, max) = bounds(&verts);
    assert_vec(min, Vec3::new(-1.0, 0.0, -0.25));
    assert_vec(max, Vec3::new(1.0, 1.5, 0.25));
    // The far cap is half size about the centroid at y 0.95 / 1.5.
    let centroid_y = 0.95 / 1.5;
    let (cap_min, cap_max) = bounds(
        &verts
            .iter()
            .copied()
            .filter(|vert| vert.z > 0.0)
            .collect::<Vec<_>>(),
    );
    assert_vec(cap_min, Vec3::new(-0.5, centroid_y * 0.5, 0.25));
    assert_vec(
        cap_max,
        Vec3::new(0.5, centroid_y + (1.5 - centroid_y) * 0.5, 0.25),
    );
}

#[test]
fn a_saved_and_reopened_outline_brush_keeps_its_shape() {
    let mut app = csg_app();
    added(
        &mut app,
        json!({ "outline": arch(), "plane": "xy", "depth": 0.25, "pos": [1.0, 0.0, 0.0], "name": "Door" }),
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("prism.bsn");
    let saved = call(
        &mut app,
        "scene.save",
        json!({ "path": path.to_string_lossy() }),
    );
    assert_eq!(saved["result"], "finished", "{saved}");
    assert!(path.exists(), "scene.save wrote {}", path.display());

    let mut app = util::editor_test_app();
    jackdaw::migrate_dialog::request_open_with_conversion(app.world_mut(), &path);
    for _ in 0..8 {
        app.update();
    }
    let door = brush_named(&mut app, "Door").expect("the reopened scene has it");
    assert_eq!(world_vertices(&app, door).len(), 22);
    assert_close(volume(&app, door), 0.25 * (2.0 + half_disc_area(0.5, 8)));
}

/// A wall from y 0 to 3, 4 wide and `thickness` deep, centred on z 0.
fn wall(app: &mut App, thickness: f32) -> Entity {
    cube(
        app,
        "Wall",
        [4.0, 3.0, thickness],
        [0.0, 1.5, 0.0],
        [0.0, 0.0, 0.0],
    )
}

#[test]
fn a_taper_too_small_to_see_ends_in_a_point() {
    let mut app = csg_app();
    let square = json!([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
    let spire = added(&mut app, json!({ "outline": square, "taper": 0.0005 }));
    assert_eq!(world_vertices(&app, spire).len(), 5);
    assert_eq!(polygons(&app, spire), 5);
}

#[test]
fn an_arch_window_inside_a_wall_cuts_an_enclosed_hole() {
    let mut app = csg_app();
    wall(&mut app, 0.25);
    let window = added(
        &mut app,
        json!({
            "outline": [
                [-0.5, 0.75],
                [0.5, 0.75],
                { "arc": { "center": [0.0, 1.5], "radius": 0.5, "from_deg": 0.0, "to_deg": 180.0, "segments": 8 } },
            ],
            "plane": "xy",
            "depth": 1.0,
            "name": "Window Cutter",
        }),
    );
    let outcome = call(
        &mut app,
        "brush.csg_subtract",
        json!({ "target": "Wall", "cutters": window.to_bits() }),
    );
    let report = &outcome["report"];
    assert_eq!(report["outcome"], "changed", "{outcome}");
    assert_eq!(report["fragments"], 1, "{outcome}");
    assert_eq!(report["watertight"], true, "{outcome}");
    let created = ids(&outcome["created"]);
    assert_eq!(created.len(), 1, "one wall with a hole: {outcome}");
    let removed = (0.75 + half_disc_area(0.5, 8)) * 0.25;
    assert_close(volume(&app, created[0]), 4.0 * 3.0 * 0.25 - removed);
}

fn cut_volume(app: &mut App, cutter: Entity) -> f32 {
    let outcome = call(
        app,
        "brush.csg_subtract",
        json!({ "target": "Wall", "cutters": cutter.to_bits() }),
    );
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    let created = ids(&outcome["created"]);
    assert!(!created.is_empty(), "{outcome}");
    created.iter().map(|&piece| volume(app, piece)).sum()
}

#[test]
fn an_arch_outline_cuts_a_door_through_a_wall() {
    let mut app = csg_app();
    wall(&mut app, 0.25);
    let door = added(
        &mut app,
        json!({ "outline": arch(), "plane": "xy", "depth": 1.0, "name": "Door Cutter" }),
    );
    let left = cut_volume(&mut app, door);
    let removed = (1.0 + half_disc_area(0.5, 8)) * 0.25;
    assert_close(left, 4.0 * 3.0 * 0.25 - removed);
}

#[test]
fn a_gable_pentagon_cuts_its_shape_out_of_a_wall() {
    let mut app = csg_app();
    wall(&mut app, 1.0);
    let gable = added(
        &mut app,
        json!({
            "outline": [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [0.0, 1.5], [-1.0, 1.0]],
            "plane": "xy",
            "depth": 2.0,
            "name": "Gable Cutter",
        }),
    );
    let left = cut_volume(&mut app, gable);
    assert_close(left, 4.0 * 3.0 - 2.5);
}

#[test]
fn a_remote_call_reports_the_created_brush() {
    let mut app = csg_app();
    let outcome = app
        .world_mut()
        .run_system_cached_with(
            call_operator_handler,
            Some(json!({
                "id": "mesh.add_brush",
                "params": { "outline": gable(), "plane": "xy", "depth": 0.5 },
            })),
        )
        .expect("the handler ran")
        .expect("the call is accepted");
    assert_eq!(outcome["result"], "finished", "{outcome}");
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    let created = ids(&outcome["entities"]);
    assert_eq!(created.len(), 1, "{outcome}");
    app.update();
    assert_eq!(polygons(&app, created[0]), 7);
}
