//! `mesh.add_revolve` from a `[radius, height]` profile: barrels, tori and
//! rings in one call, partial turns with flat caps, refusals that leave the
//! scene alone, history, a save round trip, and the results as CSG operands.

use crate::util;

use bevy::prelude::*;
use jackdaw::commands::CommandHistory;
use jackdaw::remote::server::call_operator_handler;
use jackdaw_scene_types::Brush;
use serde_json::{Value, json};

use super::csg_ops::{call, csg_app, cube, ids};

const EPS: f32 = 1e-4;

fn barrel_profile() -> Value {
    json!([
        [0.4, 0.0],
        [0.47, 0.25],
        [0.5, 0.5],
        [0.47, 0.75],
        [0.4, 1.0]
    ])
}

/// A closed eight-point circle of radius 0.2 at radius 1.
fn torus_profile() -> Value {
    let mut points: Vec<[f32; 2]> = (0..8)
        .map(|i| {
            let at = Vec2::new(1.0, 0.0)
                + Vec2::from_angle(i as f32 / 8.0 * std::f32::consts::TAU) * 0.2;
            [at.x, at.y]
        })
        .collect();
    points.push(points[0]);
    json!(points)
}

fn well_ring() -> Value {
    json!([[0.8, 0.0], [1.0, 0.0], [1.0, 0.6], [0.8, 0.6], [0.8, 0.0]])
}

fn added(app: &mut App, params: Value) -> Entity {
    let outcome = call(app, "mesh.add_revolve", params);
    assert_eq!(outcome["result"], "finished", "{outcome}");
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    let entities = ids(&outcome["entities"]);
    assert_eq!(entities.len(), 1, "{outcome}");
    entities[0]
}

fn brush(app: &App, entity: Entity) -> &Brush {
    app.world().get::<Brush>(entity).expect("a brush")
}

fn faces(app: &App, entity: Entity) -> usize {
    brush(app, entity).topology.polygons.len()
}

fn euler(app: &App, entity: Entity) -> i64 {
    let topology = &brush(app, entity).topology;
    topology.vertices.len() as i64 - topology.edges.len() as i64 + topology.polygons.len() as i64
}

/// Each face's corners in world space.
fn face_rings(app: &App, entity: Entity) -> Vec<Vec<Vec3>> {
    let global = app
        .world()
        .get::<GlobalTransform>(entity)
        .expect("a transform");
    let topology = &brush(app, entity).topology;
    topology
        .polygons
        .iter()
        .map(|poly| {
            let start = poly.loop_start as usize;
            topology.loops[start..start + poly.loop_total as usize]
                .iter()
                .map(|corner| {
                    global.transform_point(topology.vertices[corner.vert as usize].position)
                })
                .collect()
        })
        .collect()
}

fn volume(app: &App, entity: Entity) -> f32 {
    face_rings(app, entity)
        .iter()
        .flat_map(|ring| (1..ring.len() - 1).map(|i| ring[0].dot(ring[i].cross(ring[i + 1])) / 6.0))
        .sum()
}

fn bounds(app: &App, entity: Entity) -> (Vec3, Vec3) {
    face_rings(app, entity)
        .iter()
        .flatten()
        .fold((Vec3::MAX, Vec3::MIN), |(min, max), point| {
            (min.min(*point), max.max(*point))
        })
}

/// The volume a `[radius, height]` profile turned by `segments` steps around
/// a full turn encloses: every ring is a regular polygon of area `k r^2`.
fn turned_volume(profile: &Value, segments: usize) -> f32 {
    let points: Vec<Vec2> = profile
        .as_array()
        .expect("a list")
        .iter()
        .map(|point| {
            Vec2::new(
                point[0].as_f64().unwrap() as f32,
                point[1].as_f64().unwrap() as f32,
            )
        })
        .collect();
    let k = segments as f32 / 2.0 * (std::f32::consts::TAU / segments as f32).sin();
    points
        .windows(2)
        .map(|pair| {
            let (a, b) = (pair[0], pair[1]);
            (b.y - a.y) * (a.x * a.x + a.x * b.x + b.x * b.x) / 3.0
        })
        .sum::<f32>()
        .abs()
        * k
}

#[track_caller]
fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1e-3,
        "expected {expected}, got {actual}"
    );
}

#[track_caller]
fn assert_bounds(actual: (Vec3, Vec3), min: Vec3, max: Vec3) {
    assert!(
        actual.0.abs_diff_eq(min, EPS) && actual.1.abs_diff_eq(max, EPS),
        "expected {min}..{max}, got {}..{}",
        actual.0,
        actual.1
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

#[test]
fn a_barrel_builds_in_one_call_with_a_lid_and_base() {
    let mut app = csg_app();
    let barrel = added(
        &mut app,
        json!({ "profile": barrel_profile(), "name": "Barrel" }),
    );
    assert_eq!(
        faces(&app, barrel),
        12 * 4 + 2,
        "four bands of 12 and two discs"
    );
    assert_eq!(euler(&app, barrel), 2);
    assert_bounds(
        bounds(&app, barrel),
        Vec3::new(-0.5, 0.0, -0.5),
        Vec3::new(0.5, 1.0, 0.5),
    );
    assert_close(volume(&app, barrel), turned_volume(&barrel_profile(), 12));
    let lids = face_rings(&app, barrel)
        .iter()
        .filter(|ring| ring.len() == 12)
        .count();
    assert_eq!(lids, 2, "each end is one flat 12-gon");
}

#[test]
fn a_torus_is_watertight_with_euler_characteristic_zero() {
    let mut app = csg_app();
    let torus = added(
        &mut app,
        json!({ "profile": torus_profile(), "name": "Hoop" }),
    );
    assert_eq!(faces(&app, torus), 12 * 8);
    assert_eq!(euler(&app, torus), 0);
    let topology = &brush(&app, torus).topology;
    assert_eq!(
        topology.loops.len(),
        2 * topology.edges.len(),
        "every edge has two sides"
    );
    assert_bounds(
        bounds(&app, torus),
        Vec3::new(-1.2, -0.2, -1.2),
        Vec3::new(1.2, 0.2, 1.2),
    );
    assert_close(volume(&app, torus), turned_volume(&torus_profile(), 12));
}

#[test]
fn a_well_ring_is_a_closed_rectangle_turned_round() {
    let mut app = csg_app();
    let ring = added(&mut app, json!({ "profile": well_ring(), "segments": 16 }));
    assert_eq!(faces(&app, ring), 16 * 4);
    assert_eq!(euler(&app, ring), 0);
    assert_close(volume(&app, ring), turned_volume(&well_ring(), 16));
}

#[test]
fn segments_axis_and_poles_change_the_geometry() {
    let mut app = csg_app();
    let dome = added(
        &mut app,
        json!({ "profile": [[0.0, 1.0], [0.7, 0.7], [1.0, 0.0]], "segments": 6 }),
    );
    // The top fans to one pole; the open end off the axis closes as a disc.
    assert_eq!(faces(&app, dome), 6 * 2 + 1);
    assert_eq!(brush(&app, dome).topology.vertices.len(), 1 + 6 * 2);
    assert_eq!(euler(&app, dome), 2);

    let rod = json!([[0.25, 0.0], [0.25, 2.0]]);
    for (axis, min, max) in [
        (
            "x",
            Vec3::new(0.0, -0.25, -0.25),
            Vec3::new(2.0, 0.25, 0.25),
        ),
        (
            "y",
            Vec3::new(-0.25, 0.0, -0.25),
            Vec3::new(0.25, 2.0, 0.25),
        ),
        (
            "z",
            Vec3::new(-0.25, -0.25, 0.0),
            Vec3::new(0.25, 0.25, 2.0),
        ),
    ] {
        let rod = added(
            &mut app,
            json!({ "profile": rod, "segments": 4, "axis": axis }),
        );
        assert_eq!(faces(&app, rod), 4 + 2);
        assert_bounds(bounds(&app, rod), min, max);
        assert!(volume(&app, rod) > 0.0, "{axis}: faces wind outward");
    }
}

#[test]
fn a_half_turn_closes_with_two_planar_caps() {
    let mut app = csg_app();
    let half = added(
        &mut app,
        json!({ "profile": well_ring(), "angle_deg": 180.0, "segments": 6 }),
    );
    assert_eq!(faces(&app, half), 6 * 4 + 2);
    assert_eq!(euler(&app, half), 2);
    // From +x through -z to -x: both caps lie in the plane z = 0.
    let caps: Vec<Vec<Vec3>> = face_rings(&app, half)
        .into_iter()
        .filter(|ring| ring.iter().all(|corner| corner.z.abs() < EPS))
        .collect();
    assert_eq!(caps.len(), 2, "{caps:?}");
    for cap in &caps {
        assert_eq!(cap.len(), 4);
    }
    assert_bounds(
        bounds(&app, half),
        Vec3::new(-1.0, 0.0, -1.0),
        Vec3::new(1.0, 0.6, 0.0),
    );
    assert_close(volume(&app, half), turned_volume(&well_ring(), 12) / 2.0);
}

#[test]
fn a_peg_three_millimetres_across_builds() {
    let mut app = csg_app();
    let peg = added(
        &mut app,
        json!({ "profile": [[0.0015, 0.0], [0.0015, 0.003]], "segments": 8 }),
    );
    assert_eq!(faces(&app, peg), 8 + 2);
    assert_eq!(euler(&app, peg), 2);
    assert!(volume(&app, peg) > 0.0);
}

#[test]
fn a_profile_touching_the_axis_twice_turns_part_way_but_not_whole() {
    let mut app = csg_app();
    let dart = json!([
        [0.0, 0.0],
        [1.0, 0.0],
        [1.0, 2.0],
        [0.0, 2.0],
        [0.5, 1.0],
        [0.0, 0.0]
    ]);
    let half = added(
        &mut app,
        json!({ "profile": dart, "angle_deg": 180.0, "segments": 6 }),
    );
    assert_eq!(euler(&app, half), 2);
    assert!(volume(&app, half) > 0.0);
    let whole = call(&mut app, "mesh.add_revolve", json!({ "profile": dart }));
    assert_eq!(whole["outcome"], "invalid", "{whole}");
}

#[test]
fn one_call_places_turns_names_and_parents_the_brush() {
    let mut app = csg_app();
    let well = call(&mut app, "entity.add.group", json!({ "name": "Well" }));
    let well = ids(&well["entities"])[0];
    let ring = added(
        &mut app,
        json!({
            "profile": well_ring(),
            "pos": [2.0, 0.0, 0.0],
            "rot": [0.0, 0.0, 90.0],
            "parent": well.to_bits(),
            "name": "Well Ring",
        }),
    );
    assert_eq!(
        app.world().get::<ChildOf>(ring).map(ChildOf::parent),
        Some(well)
    );
    assert_eq!(
        app.world().get::<Name>(ring).map(Name::as_str),
        Some("Well Ring")
    );
    let (min, max) = bounds(&app, ring);
    assert_close(min.x, 2.0 - 0.6);
    assert_close(max.x, 2.0);
}

#[test]
fn invalid_profiles_are_refused_and_leave_the_scene_byte_identical() {
    let mut app = csg_app();
    added(
        &mut app,
        json!({ "profile": barrel_profile(), "name": "Keep" }),
    );
    let before = scene_text(&mut app);
    let entries = history_len(&app);
    let rod = json!([[0.5, 0.0], [0.5, 1.0]]);
    let long: Vec<[f32; 2]> = (0..20)
        .map(|i| [0.5 + 0.01 * i as f32, i as f32 * 0.1])
        .collect();

    for params in [
        json!({}),
        json!({ "profile": "not a list" }),
        json!({ "profile": [[0.5, 0.0]] }),
        json!({ "profile": [[0.5, 0.0], [0.5]] }),
        json!({ "profile": [[0.5, 0.0], [-0.1, 1.0]] }),
        json!({ "profile": [[0.0, 0.0], [0.0, 1.0]] }),
        json!({ "profile": [[1.0, 0.0], [2.0, 1.0], [2.0, 0.0], [1.0, 1.0], [1.0, 0.0]] }),
        json!({ "profile": [[0.0, 0.5], [1.0, 0.0], [1.0, 1.0], [0.0, 0.5]] }),
        json!({ "profile": [[0.0, 0.0], [1.0, 0.0], [1.0, 2.0], [0.0, 2.0], [0.5, 1.0], [0.0, 0.0]] }),
        json!({ "profile": [[0.4, 0.0], [0.0, 0.5], [0.4, 1.0]] }),
        json!({ "profile": rod, "segments": 2 }),
        json!({ "profile": rod, "segments": 257 }),
        json!({ "profile": rod, "angle_deg": 0.0 }),
        json!({ "profile": rod, "angle_deg": -90.0 }),
        json!({ "profile": rod, "angle_deg": 361.0 }),
        json!({ "profile": rod, "angle_deg": 0.001 }),
        json!({ "profile": rod, "angle_deg": 359.999 }),
        json!({ "profile": rod, "axis": "w" }),
        json!({ "profile": rod, "caps": "some" }),
        json!({ "profile": rod, "caps": "none" }),
        json!({ "profile": torus_profile(), "caps": "none", "angle_deg": 90.0 }),
        json!({ "profile": rod, "radius": 2.0 }),
        json!({ "profile": long, "segments": 256 }),
        json!({ "profile": rod, "parent": "NoSuchNode" }),
        json!({ "profile": rod, "name": "  " }),
    ] {
        let outcome = call(&mut app, "mesh.add_revolve", params.clone());
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

    let torus = added(
        &mut app,
        json!({ "profile": torus_profile(), "caps": "none" }),
    );
    assert_eq!(
        faces(&app, torus),
        12 * 8,
        "a closed full turn needs no caps"
    );
}

#[test]
fn one_undo_takes_the_brush_back_and_redo_reapplies_it() {
    let mut app = csg_app();
    let entries = history_len(&app);
    added(
        &mut app,
        json!({ "profile": barrel_profile(), "angle_deg": 270.0, "name": "Cut Barrel" }),
    );
    assert_eq!(history_len(&app), entries + 1, "one undo entry");

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    app.update();
    assert!(
        brush_named(&mut app, "Cut Barrel").is_none(),
        "undo removes it"
    );

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.redo(world));
    app.update();
    let barrel = brush_named(&mut app, "Cut Barrel").expect("redo brings it back");
    assert_eq!(faces(&app, barrel), 12 * 4 + 2 + 2);
    assert_close(
        volume(&app, barrel),
        turned_volume(&barrel_profile(), 16) * 0.75,
    );
}

#[test]
fn saved_and_reopened_barrel_torus_and_well_ring_keep_their_shape() {
    let mut app = csg_app();
    added(
        &mut app,
        json!({ "profile": barrel_profile(), "name": "Barrel" }),
    );
    added(
        &mut app,
        json!({ "profile": torus_profile(), "pos": [3.0, 0.0, 0.0], "name": "Hoop" }),
    );
    added(
        &mut app,
        json!({ "profile": well_ring(), "pos": [-3.0, 0.0, 0.0], "name": "Well Ring" }),
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("revolve.bsn");
    let saved = call(
        &mut app,
        "scene.save",
        json!({ "path": path.to_string_lossy() }),
    );
    assert_eq!(saved["result"], "finished", "{saved}");

    let mut app = util::editor_test_app();
    jackdaw::migrate_dialog::request_open_with_conversion(app.world_mut(), &path);
    for _ in 0..8 {
        app.update();
    }
    for (name, profile, face_count, euler_characteristic) in [
        ("Barrel", barrel_profile(), 50, 2),
        ("Hoop", torus_profile(), 96, 0),
        ("Well Ring", well_ring(), 48, 0),
    ] {
        let entity = brush_named(&mut app, name).expect("the reopened scene has it");
        assert_eq!(faces(&app, entity), face_count, "{name}");
        assert_eq!(euler(&app, entity), euler_characteristic, "{name}");
        assert_close(volume(&app, entity), turned_volume(&profile, 12));
    }
}

/// Subtract `cutter` from `target` and return the pieces left, checking the
/// cut reports `fragments` watertight pieces.
fn clean_cut(app: &mut App, target: &str, cutter: Entity, fragments: usize) -> Vec<Entity> {
    let outcome = call(
        app,
        "brush.csg_subtract",
        json!({ "target": target, "cutters": cutter.to_bits() }),
    );
    let report = &outcome["report"];
    assert_eq!(report["outcome"], "changed", "{outcome}");
    assert_eq!(report["fragments"], fragments, "{outcome}");
    assert_eq!(report["watertight"], true, "{outcome}");
    let created = ids(&outcome["created"]);
    assert_eq!(created.len(), fragments, "{outcome}");
    created
}

#[test]
fn a_barrel_and_a_well_ring_take_a_clean_subtract() {
    let mut app = csg_app();
    let barrel = added(
        &mut app,
        json!({ "profile": barrel_profile(), "name": "Barrel" }),
    );
    let whole = volume(&app, barrel);
    let bung = cube(&mut app, "Bung", [0.2, 0.2, 0.2], [0.5, 0.5, 0.0], [0.0; 3]);
    let cut = clean_cut(&mut app, "Barrel", bung, 1)[0];
    let left = volume(&app, cut);
    assert!(
        left < whole && left > whole - 0.2 * 0.2 * 0.2,
        "{left} of {whole}"
    );

    let ring = added(
        &mut app,
        json!({ "profile": well_ring(), "name": "Well Ring" }),
    );
    let whole = volume(&app, ring);
    let notch = cube(
        &mut app,
        "Notch",
        [0.4, 0.2, 0.4],
        [0.9, 0.6, 0.0],
        [0.0; 3],
    );
    let cut = clean_cut(&mut app, "Well Ring", notch, 1)[0];
    let left = volume(&app, cut);
    assert!(
        left < whole && left > whole - 0.4 * 0.1 * 0.4,
        "{left} of {whole}"
    );
}

#[test]
fn a_torus_is_cut_as_a_target_and_cuts_as_a_cutter() {
    let mut app = csg_app();
    let hoop = added(
        &mut app,
        json!({ "profile": torus_profile(), "name": "Hoop" }),
    );
    let whole = volume(&app, hoop);
    let gap = cube(&mut app, "Gap", [0.6, 0.6, 0.6], [1.0, 0.0, 0.0], [0.0; 3]);
    let cut = clean_cut(&mut app, "Hoop", gap, 1)[0];
    let left = volume(&app, cut);
    assert!(
        left < whole * 0.95 && left > whole * 0.8,
        "{left} of {whole}"
    );

    // A slab through the hoop's middle plane splits into a disc and a frame.
    let slab = cube(&mut app, "Slab", [3.0, 0.1, 3.0], [0.0; 3], [0.0; 3]);
    let slab_volume = volume(&app, slab);
    let cutter = added(
        &mut app,
        json!({ "profile": torus_profile(), "name": "Cutter" }),
    );
    let pieces = clean_cut(&mut app, "Slab", cutter, 2);
    let left: f32 = pieces.iter().map(|&piece| volume(&app, piece)).sum();
    assert!(
        left < slab_volume && left > slab_volume - whole,
        "{left} of {slab_volume}"
    );
}

#[test]
fn a_remote_call_reports_the_created_brush() {
    let mut app = csg_app();
    let outcome = app
        .world_mut()
        .run_system_cached_with(
            call_operator_handler,
            Some(json!({
                "id": "mesh.add_revolve",
                "params": { "profile": well_ring(), "segments": 8 },
            })),
        )
        .expect("the handler ran")
        .expect("the call is accepted");
    assert_eq!(outcome["result"], "finished", "{outcome}");
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    let created = ids(&outcome["entities"]);
    assert_eq!(created.len(), 1, "{outcome}");
    app.update();
    assert_eq!(faces(&app, created[0]), 8 * 4);
}
