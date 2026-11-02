//! `entity.add.*` primitives built from parameters: size baked into the brush,
//! segment counts, placement, name, parent and material in one call, refusals
//! that leave the scene alone, history and a save round trip.

use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::prelude::*;
use jackdaw::commands::CommandHistory;
use jackdaw::material_browser::MaterialRegistry;
use jackdaw::remote::server::call_operator_handler;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::OperatorOutcome;
use jackdaw_scene_types::{Brush, PropertyValue};
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

fn call(app: &mut App, id: &str, params: Value) -> Value {
    let outcome = app
        .world_mut()
        .run_system_cached_with(
            call_operator_handler,
            Some(json!({ "id": id, "params": params })),
        )
        .expect("the handler ran")
        .unwrap_or_else(|err| panic!("{id} refused: {}", err.message));
    app.update();
    outcome
}

fn created(app: &mut App, id: &str, params: Value) -> Entity {
    let outcome = call(app, id, params);
    assert_eq!(outcome["result"], "finished", "{id}: {outcome}");
    assert_eq!(outcome["outcome"], "changed", "{id}: {outcome}");
    let entities = outcome["entities"].as_array().expect("entities");
    assert_eq!(entities.len(), 1, "{id}: {outcome}");
    let bits = entities[0].as_u64().expect("an entity id");
    Entity::try_from_bits(bits).expect("a valid entity id")
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

fn extent(app: &App, entity: Entity) -> Vec3 {
    let verts = vertices(app, entity);
    let min = verts.iter().copied().fold(Vec3::MAX, Vec3::min);
    let max = verts.iter().copied().fold(Vec3::MIN, Vec3::max);
    max - min
}

#[track_caller]
fn assert_close(actual: Vec3, expected: Vec3) {
    assert!(
        actual.abs_diff_eq(expected, EPS),
        "expected {expected}, got {actual}"
    );
}

fn polygons(app: &App, entity: Entity) -> usize {
    app.world()
        .get::<Brush>(entity)
        .expect("a brush")
        .topology
        .polygons
        .len()
}

fn scene_text(app: &mut App) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), dir.path())
        .expect("the scene emits")
}

fn history_len(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

fn brushes(app: &mut App) -> usize {
    app.world_mut()
        .query_filtered::<(), With<Brush>>()
        .iter(app.world())
        .count()
}

#[test]
fn size_bakes_the_measured_bounding_box_into_each_primitive() {
    let mut app = scene_app();
    let size = Vec3::new(2.0, 3.0, 4.0);
    for id in [
        "entity.add.cube",
        "entity.add.cylinder",
        "entity.add.cone",
        "entity.add.wedge",
        "entity.add.pyramid",
    ] {
        let entity = created(&mut app, id, json!({ "size": [2.0, 3.0, 4.0] }));
        assert_close(extent(&app, entity), size);
        let transform = app.world().get::<Transform>(entity).expect("a transform");
        assert_eq!(transform.scale, Vec3::ONE, "{id}: size is not scale");
    }

    let plane = created(
        &mut app,
        "entity.add.plane",
        json!({ "size": [2.0, 9.0, 4.0] }),
    );
    let measured = extent(&app, plane);
    assert_close(
        Vec3::new(measured.x, 0.0, measured.z),
        Vec3::new(2.0, 0.0, 4.0),
    );
    assert!(measured.y < 0.1, "a plane keeps its thin slab: {measured}");

    let sphere = created(
        &mut app,
        "entity.add.sphere",
        json!({ "size": [2.0, 3.0, 4.0] }),
    );
    for vert in vertices(&app, sphere) {
        let on_surface = (vert / (size / 2.0)).length();
        assert!(
            (on_surface - 1.0).abs() < EPS,
            "{vert} is off the ellipsoid"
        );
    }
}

#[test]
fn a_three_sided_cylinder_and_a_six_sided_cone_have_those_sides() {
    let mut app = scene_app();
    let cylinder = created(&mut app, "entity.add.cylinder", json!({ "segments": 3 }));
    assert_eq!(vertices(&app, cylinder).len(), 6);
    assert_eq!(polygons(&app, cylinder), 5);
    assert_close(
        extent(&app, cylinder).with_x(0.0).with_z(0.0),
        Vec3::new(0.0, 1.0, 0.0),
    );

    let cone = created(&mut app, "entity.add.cone", json!({ "segments": 6 }));
    assert_eq!(vertices(&app, cone).len(), 7);
    assert_eq!(polygons(&app, cone), 7);
}

#[test]
fn a_sphere_with_segments_is_a_ring_sphere_of_that_many_sides() {
    let mut app = scene_app();
    let sphere = created(
        &mut app,
        "entity.add.sphere",
        json!({ "segments": 8, "size": [2.0, 2.0, 2.0] }),
    );
    // Eight around, four rings: two poles and three rings of eight.
    assert_eq!(vertices(&app, sphere).len(), 2 + 3 * 8);
    assert_eq!(polygons(&app, sphere), 4 * 8);
    assert_close(extent(&app, sphere), Vec3::splat(2.0));
}

#[test]
fn absent_parameters_keep_the_default_primitive() {
    let mut app = scene_app();
    let cube = created(&mut app, "entity.add.cube", json!({}));
    assert_close(extent(&app, cube), Vec3::ONE);
    assert_eq!(
        app.world().get::<Name>(cube).map(Name::as_str),
        Some("Cube")
    );
    let cylinder = created(&mut app, "entity.add.cylinder", json!({}));
    assert_eq!(vertices(&app, cylinder).len(), 32, "sixteen sides");
}

#[test]
fn one_call_places_names_parents_and_materials_the_primitive() {
    let mut app = scene_app();
    let handle = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    app.world_mut()
        .resource_mut::<MaterialRegistry>()
        .add("slate".into(), handle.clone());
    let group = group(&mut app, "House");

    let wall = created(
        &mut app,
        "entity.add.cube",
        json!({
            "size": [4.0, 2.5, 0.25],
            "pos": [1.0, 1.25, -2.0],
            "rot": [90.0, 0.0, 0.0],
            "material": "materials/slate.material.bsn",
            "parent": group.to_bits(),
            "name": "Back Wall",
        }),
    );
    let transform = *app.world().get::<Transform>(wall).expect("a transform");
    assert_close(transform.translation, Vec3::new(1.0, 1.25, -2.0));
    let (yaw, pitch, roll) = transform.rotation.to_euler(EulerRot::YXZ);
    assert_close(
        Vec3::new(yaw, pitch, roll),
        Vec3::new(90f32.to_radians(), 0.0, 0.0),
    );
    assert_eq!(
        app.world().get::<Name>(wall).map(Name::as_str),
        Some("Back Wall")
    );
    assert_eq!(
        app.world().get::<ChildOf>(wall).map(ChildOf::parent),
        Some(group)
    );
    let brush = app.world().get::<Brush>(wall).expect("a brush");
    assert!(
        brush.faces.iter().all(|face| face.material == handle),
        "every face wears the named material"
    );
    assert_close(extent(&app, wall), Vec3::new(4.0, 2.5, 0.25));
}

#[test]
fn invalid_parameters_are_refused_and_leave_the_scene_byte_identical() {
    let mut app = scene_app();
    created(&mut app, "entity.add.cube", json!({ "name": "Keep" }));
    let before = scene_text(&mut app);
    let entries = history_len(&app);
    let count = brushes(&mut app);

    for (id, params) in [
        ("entity.add.cylinder", json!({ "segments": 2 })),
        ("entity.add.cone", json!({ "segments": 0 })),
        ("entity.add.sphere", json!({ "segments": 2 })),
        ("entity.add.cube", json!({ "size": [1.0, 0.0, 1.0] })),
        ("entity.add.wedge", json!({ "size": [1.0, -1.0, 1.0] })),
        ("entity.add.plane", json!({ "size": [0.0, 1.0, 1.0] })),
        (
            "entity.add.cube",
            json!({ "material": "materials/missing.material.bsn" }),
        ),
        ("entity.add.cube", json!({ "parent": "NoSuchNode" })),
        ("entity.add.cube", json!({ "name": "   " })),
        ("entity.add.cube", json!({ "segments": 6 })),
    ] {
        let outcome = call(&mut app, id, params.clone());
        assert_eq!(outcome["outcome"], "invalid", "{id} {params}: {outcome}");
        assert_eq!(outcome["result"], "cancelled", "{id} {params}: {outcome}");
        assert_eq!(outcome["entities"], json!([]), "{id} {params}: {outcome}");
        assert!(
            !outcome["warnings"].as_array().expect("warnings").is_empty(),
            "{id} {params}: the refusal says why: {outcome}"
        );
        assert_eq!(brushes(&mut app), count, "{id} {params}");
        assert_eq!(history_len(&app), entries, "{id} {params}");
        assert_eq!(scene_text(&mut app), before, "{id} {params}");
    }
}

#[test]
fn one_undo_takes_the_whole_creation_back_and_redo_reapplies_it() {
    let mut app = scene_app();
    let group = group(&mut app, "Yard");
    let entries = history_len(&app);
    created(
        &mut app,
        "entity.add.cone",
        json!({
            "size": [1.0, 2.0, 1.0],
            "segments": 6,
            "pos": [0.0, 3.0, 0.0],
            "parent": group.to_bits(),
            "name": "Spire",
        }),
    );
    assert_eq!(history_len(&app), entries + 1, "one undo entry");

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    app.update();
    assert!(named(&mut app, "Spire").is_none(), "undo removes it");

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.redo(world));
    app.update();
    let spire = named(&mut app, "Spire").expect("redo brings it back");
    assert_eq!(vertices(&app, spire).len(), 7);
    assert_close(extent(&app, spire).with_z(0.0), Vec3::new(1.0, 2.0, 0.0));
    let transform = app.world().get::<Transform>(spire).expect("a transform");
    assert_close(transform.translation, Vec3::new(0.0, 3.0, 0.0));
    let parent = app.world().get::<ChildOf>(spire).map(ChildOf::parent);
    assert_eq!(
        parent
            .and_then(|parent| app.world().get::<Name>(parent))
            .map(Name::as_str),
        Some("Yard")
    );
}

#[test]
fn a_saved_and_reopened_primitive_keeps_its_shape_name_and_place() {
    let mut app = scene_app();
    created(
        &mut app,
        "entity.add.cylinder",
        json!({
            "size": [3.0, 1.0, 3.0],
            "segments": 3,
            "pos": [2.0, 0.5, 0.0],
            "rot": [0.0, 0.0, 45.0],
            "name": "Prism",
        }),
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("primitives.bsn");
    let text = jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), tmp.path())
        .expect("the scene emits");
    std::fs::write(&path, text).expect("write the scene");

    let mut app = util::editor_test_app();
    jackdaw::migrate_dialog::request_open_with_conversion(app.world_mut(), &path);
    for _ in 0..8 {
        app.update();
    }
    let prism = named(&mut app, "Prism").expect("the reopened scene has it");
    assert_eq!(vertices(&app, prism).len(), 6);
    assert_close(
        extent(&app, prism).with_x(0.0).with_z(0.0),
        Vec3::new(0.0, 1.0, 0.0),
    );
    let transform = app.world().get::<Transform>(prism).expect("a transform");
    assert_close(transform.translation, Vec3::new(2.0, 0.5, 0.0));
    let (_, _, roll) = transform.rotation.to_euler(EulerRot::YXZ);
    assert!((roll - 45f32.to_radians()).abs() < EPS);
}

fn named(app: &mut App, name: &str) -> Option<Entity> {
    app.world_mut()
        .query_filtered::<(Entity, &Name), With<Brush>>()
        .iter(app.world())
        .find(|(_, found)| found.as_str() == name)
        .map(|(entity, _)| entity)
}

fn group(app: &mut App, name: &str) -> Entity {
    let outcome = call(app, "entity.add.group", json!({ "name": name }));
    let bits = outcome["entities"][0].as_u64().expect("the group's id");
    Entity::try_from_bits(bits).expect("a valid entity id")
}

#[track_caller]
fn assert_on_ellipsoid(app: &App, entity: Entity, size: Vec3) {
    for vert in vertices(app, entity) {
        let on_surface = (vert / (size / 2.0)).length();
        assert!(
            (on_surface - 1.0).abs() < EPS,
            "{vert} is off the ellipsoid"
        );
    }
}

#[test]
fn a_sphere_is_a_ring_sphere_or_a_subdivided_twenty_face_sphere() {
    let mut app = scene_app();
    let size = Vec3::new(2.0, 3.0, 2.0);

    let plain = created(&mut app, "entity.add.sphere", json!({}));
    assert_eq!(
        vertices(&app, plain).len(),
        12,
        "the default stays 20 faces"
    );
    assert_eq!(polygons(&app, plain), 20);

    let rings = created(
        &mut app,
        "entity.add.sphere",
        json!({ "kind": "uv", "segments": 6, "rings": 3, "size": [2.0, 3.0, 2.0] }),
    );
    assert_eq!(vertices(&app, rings).len(), 2 + 2 * 6);
    assert_eq!(polygons(&app, rings), 3 * 6);
    assert_on_ellipsoid(&app, rings, size);
    assert!((extent(&app, rings).y - 3.0).abs() < EPS);

    let ico = created(
        &mut app,
        "entity.add.sphere",
        json!({ "kind": "ico", "subdivisions": 1, "size": [2.0, 3.0, 2.0] }),
    );
    assert_eq!(vertices(&app, ico).len(), 42);
    assert_eq!(polygons(&app, ico), 80);
    assert_on_ellipsoid(&app, ico, size);
}

#[test]
fn a_cylinder_with_a_top_radius_is_a_frustum() {
    let mut app = scene_app();
    let frustum = created(
        &mut app,
        "entity.add.cylinder",
        json!({ "segments": 4, "radius_top": 0.25, "size": [2.0, 2.0, 2.0] }),
    );
    let verts = vertices(&app, frustum);
    assert_eq!(verts.len(), 8);
    assert_eq!(polygons(&app, frustum), 6);
    let widest = |above: bool| {
        verts
            .iter()
            .filter(|vert| (vert.y > 0.0) == above)
            .map(|vert| vert.x.abs())
            .fold(0.0_f32, f32::max)
    };
    assert!((widest(true) - 0.25).abs() < EPS, "top {}", widest(true));
    assert!((widest(false) - 1.0).abs() < EPS, "base {}", widest(false));
    assert_close(extent(&app, frustum).with_z(0.0), Vec3::new(2.0, 2.0, 0.0));
}

#[test]
fn a_pyramid_takes_any_number_of_sides_and_fills_its_size() {
    let mut app = scene_app();
    let size = Vec3::new(2.0, 3.0, 4.0);
    for (sides, verts) in [(3, 4), (4, 5), (6, 7)] {
        let pyramid = created(
            &mut app,
            "entity.add.pyramid",
            json!({ "sides": sides, "size": [2.0, 3.0, 4.0] }),
        );
        assert_eq!(vertices(&app, pyramid).len(), verts, "{sides} sides");
        assert_eq!(polygons(&app, pyramid), sides + 1, "{sides} sides");
        assert_close(extent(&app, pyramid), size);
    }
}

#[test]
fn shape_parameters_out_of_range_or_for_another_kind_are_refused() {
    let mut app = scene_app();
    created(&mut app, "entity.add.cube", json!({ "name": "Keep" }));
    let before = scene_text(&mut app);
    for (id, params) in [
        ("entity.add.sphere", json!({ "kind": "cube" })),
        (
            "entity.add.sphere",
            json!({ "kind": "uv", "subdivisions": 1 }),
        ),
        ("entity.add.sphere", json!({ "kind": "ico", "segments": 8 })),
        (
            "entity.add.sphere",
            json!({ "kind": "ico", "subdivisions": 9 }),
        ),
        ("entity.add.sphere", json!({ "rings": 1 })),
        ("entity.add.cylinder", json!({ "radius_top": -0.5 })),
        ("entity.add.cone", json!({ "radius_top": 0.5 })),
        ("entity.add.pyramid", json!({ "sides": 2 })),
        ("entity.add.plane", json!({ "subdivisions": 3 })),
        ("entity.add.plane", json!({ "subdivisions": 65 })),
        ("entity.add.cube", json!({ "radius": 2.0 })),
    ] {
        let outcome = call(&mut app, id, params.clone());
        assert_eq!(outcome["outcome"], "invalid", "{id} {params}: {outcome}");
        assert_eq!(scene_text(&mut app), before, "{id} {params}");
    }
}

fn select_only(app: &mut App, entity: Entity) {
    app.world_mut().resource_mut::<Selection>().entities = vec![entity];
}

fn selected(app: &App) -> Vec<Entity> {
    app.world().resource::<Selection>().entities.to_vec()
}

/// The Add menu and keybinds dispatch without the remote's history span, so
/// the operator's own entry has to be the only one.
#[test]
fn an_add_from_the_menu_is_one_undo_entry() {
    let mut app = scene_app();
    let entries = history_len(&app);
    let count = brushes(&mut app);
    app.world_mut()
        .operator("entity.add.cube")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        })
        .call()
        .expect("entity.add.cube dispatches")
        .assert_finished();
    app.update();
    assert_eq!(history_len(&app), entries + 1, "exactly one entry");
    assert_eq!(brushes(&mut app), count + 1);

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    app.update();
    assert_eq!(brushes(&mut app), count, "one undo takes the cube back");
    assert_eq!(history_len(&app), entries);
}

#[test]
fn an_outcome_never_carries_over_to_the_next_call() {
    let mut app = scene_app();
    app.world_mut()
        .operator("entity.add.cylinder")
        .param("segments", PropertyValue::Int(2))
        .call()
        .expect("entity.add.cylinder dispatches")
        .assert_cancelled();
    let outcome = |app: &mut App| {
        app.world_mut()
            .get_resource_or_init::<OperatorOutcome>()
            .get()
    };
    assert_eq!(outcome(&mut app), Some(Outcome::Invalid));

    app.world_mut()
        .operator("entity.add.empty")
        .call()
        .expect("entity.add.empty dispatches")
        .assert_finished();
    assert_eq!(
        outcome(&mut app),
        None,
        "an operator that says nothing reports nothing"
    );
}

#[test]
fn a_shape_past_the_face_budget_is_refused() {
    let mut app = scene_app();
    let refused = call(
        &mut app,
        "entity.add.sphere",
        json!({ "kind": "uv", "segments": 256, "rings": 128 }),
    );
    assert_eq!(refused["outcome"], "invalid", "{refused}");
    let refused = call(&mut app, "entity.add.sphere", json!({ "subdivisions": 4 }));
    assert_eq!(refused["outcome"], "invalid", "{refused}");

    let sphere = created(
        &mut app,
        "entity.add.sphere",
        json!({ "kind": "uv", "segments": 64, "rings": 32 }),
    );
    assert_eq!(polygons(&app, sphere), 64 * 32);
}

#[test]
fn a_refused_call_leaves_the_selection_alone() {
    let mut app = scene_app();
    let house = group(&mut app, "House");
    let keep = created(&mut app, "entity.add.cube", json!({ "name": "Keep" }));
    select_only(&mut app, keep);

    let refused = call(
        &mut app,
        "entity.add.cylinder",
        json!({ "parent": "House", "segments": 2 }),
    );
    assert_eq!(refused["outcome"], "invalid", "{refused}");
    assert_eq!(
        selected(&app),
        vec![keep],
        "the named parent was not selected"
    );

    let post = created(
        &mut app,
        "entity.add.cylinder",
        json!({ "parent": house.to_bits() }),
    );
    assert_eq!(
        app.world().get::<ChildOf>(post).map(ChildOf::parent),
        Some(house)
    );
}
