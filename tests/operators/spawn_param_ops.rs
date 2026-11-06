//! `entity.add.*` lights and other nodes built from parameters, and
//! `entity.duplicate` making a placed run: params landing on the components,
//! aim, refusals that leave the scene alone, and one undo step per call.

use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::prelude::*;
use jackdaw::commands::CommandHistory;
use jackdaw::remote::server::call_operator_handler;
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

fn ids(outcome: &Value) -> Vec<Entity> {
    outcome["entities"]
        .as_array()
        .expect("entities")
        .iter()
        .map(|bits| {
            Entity::try_from_bits(bits.as_u64().expect("an entity id")).expect("a valid id")
        })
        .collect()
}

fn created(app: &mut App, id: &str, params: Value) -> Entity {
    let outcome = call(app, id, params);
    assert_eq!(outcome["result"], "finished", "{id}: {outcome}");
    assert_eq!(outcome["outcome"], "changed", "{id}: {outcome}");
    let entities = ids(&outcome);
    assert_eq!(entities.len(), 1, "{id}: {outcome}");
    entities[0]
}

fn scene_text(app: &mut App) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), dir.path())
        .expect("the scene emits")
}

fn history_len(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

fn undo(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    app.update();
}

fn transform(app: &App, entity: Entity) -> Transform {
    *app.world().get::<Transform>(entity).expect("a transform")
}

fn parent_name(app: &App, entity: Entity) -> Option<String> {
    let parent = app.world().get::<ChildOf>(entity)?.parent();
    app.world().get::<Name>(parent).map(ToString::to_string)
}

fn name(app: &App, entity: Entity) -> Option<String> {
    app.world().get::<Name>(entity).map(ToString::to_string)
}

#[track_caller]
fn assert_close(actual: Vec3, expected: Vec3) {
    assert!(
        actual.abs_diff_eq(expected, EPS),
        "expected {expected}, got {actual}"
    );
}

#[track_caller]
fn assert_colour(actual: Color, expected: Color) {
    let (actual, expected) = (actual.to_srgba(), expected.to_srgba());
    let delta = (actual.red - expected.red).abs()
        + (actual.green - expected.green).abs()
        + (actual.blue - expected.blue).abs();
    assert!(delta < 1e-3, "expected {expected:?}, got {actual:?}");
}

fn brushes_named(app: &mut App, wanted: &str) -> Vec<Entity> {
    app.world_mut()
        .query_filtered::<(Entity, &Name), With<Brush>>()
        .iter(app.world())
        .filter(|(_, found)| found.as_str().starts_with(wanted))
        .map(|(entity, _)| entity)
        .collect()
}

#[test]
fn every_point_light_param_lands_on_the_component() {
    let mut app = scene_app();
    let light = created(
        &mut app,
        "entity.add.point_light",
        json!({
            "color": "#ff8000",
            "intensity": 800.0,
            "range": 6.0,
            "radius": 0.25,
            "shadows": true,
            "pos": [1.0, 2.0, 3.0],
            "name": "Glow",
        }),
    );
    let point = app.world().get::<PointLight>(light).expect("a point light");
    assert_colour(point.color, Color::srgb(1.0, 128.0 / 255.0, 0.0));
    assert_eq!(point.intensity, 800.0);
    assert_eq!(point.range, 6.0);
    assert_eq!(point.radius, 0.25);
    assert!(point.shadow_maps_enabled);
    assert_close(transform(&app, light).translation, Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(name(&app, light).as_deref(), Some("Glow"));
}

#[test]
fn a_spot_light_takes_its_cone_and_aims_at_the_target() {
    let mut app = scene_app();
    let light = created(
        &mut app,
        "entity.add.spot_light",
        json!({
            "color": [0.2, 0.4, 1.0],
            "intensity": 5000.0,
            "range": 12.0,
            "radius": 0.1,
            "shadows": false,
            "cone": [20.0, 35.0],
            "pos": [4.0, 5.0, 0.0],
            "target": [0.0, 0.0, 2.0],
        }),
    );
    let spot = app.world().get::<SpotLight>(light).expect("a spot light");
    assert_colour(spot.color, Color::srgb(0.2, 0.4, 1.0));
    assert_eq!(spot.intensity, 5000.0);
    assert_eq!(spot.range, 12.0);
    assert_eq!(spot.radius, 0.1);
    assert!(!spot.shadow_maps_enabled);
    assert!((spot.inner_angle - 20f32.to_radians()).abs() < EPS);
    assert!((spot.outer_angle - 35f32.to_radians()).abs() < EPS);

    let placed = transform(&app, light);
    assert_close(placed.translation, Vec3::new(4.0, 5.0, 0.0));
    let wanted = (Vec3::new(0.0, 0.0, 2.0) - placed.translation).normalize();
    assert_close(*placed.forward(), wanted);
}

#[test]
fn a_directional_light_reads_intensity_as_lux_and_aims_without_a_position() {
    let mut app = scene_app();
    let sun = created(
        &mut app,
        "entity.add.directional_light",
        json!({ "intensity": 2500.0, "shadows": false, "target": [3.0, 0.0, 0.0] }),
    );
    let light = app
        .world()
        .get::<DirectionalLight>(sun)
        .expect("a directional light");
    assert_eq!(light.illuminance, 2500.0);
    assert!(!light.shadow_maps_enabled);
    let placed = transform(&app, sun);
    let wanted = (Vec3::new(3.0, 0.0, 0.0) - placed.translation).normalize();
    assert_close(*placed.forward(), wanted);

    let straight_down = created(
        &mut app,
        "entity.add.directional_light",
        json!({ "pos": [0.0, 10.0, 0.0], "target": [0.0, 0.0, 0.0] }),
    );
    assert_close(*transform(&app, straight_down).forward(), Vec3::NEG_Y);
}

#[test]
fn a_position_alone_keeps_the_template_tilt() {
    let mut app = scene_app();
    let plain = created(&mut app, "entity.add.directional_light", json!({}));
    let moved = created(
        &mut app,
        "entity.add.directional_light",
        json!({ "pos": [5.0, 20.0, 5.0] }),
    );
    assert!(
        transform(&app, moved)
            .rotation
            .abs_diff_eq(transform(&app, plain).rotation, EPS)
    );
    assert_close(
        transform(&app, moved).translation,
        Vec3::new(5.0, 20.0, 5.0),
    );
}

/// The lantern a village path wants: one call, under its post, one undo step.
#[test]
fn a_lantern_is_one_call_and_one_undo_step() {
    let mut app = scene_app();
    let post = created(&mut app, "entity.add.group", json!({ "name": "LampPost" }));
    let entries = history_len(&app);

    let lantern = created(
        &mut app,
        "entity.add.point_light",
        json!({
            "color": "1.0,0.75,0.4",
            "intensity": 600.0,
            "range": 8.0,
            "pos": [0.0, 2.2, 0.3],
            "parent": post.to_bits(),
            "name": "Lantern",
        }),
    );
    assert_eq!(history_len(&app), entries + 1, "one entry");
    assert_eq!(parent_name(&app, lantern).as_deref(), Some("LampPost"));
    assert_close(
        transform(&app, lantern).translation,
        Vec3::new(0.0, 2.2, 0.3),
    );
    let point = app
        .world()
        .get::<PointLight>(lantern)
        .expect("a point light");
    assert_eq!((point.intensity, point.range), (600.0, 8.0));

    let text = scene_text(&mut app);
    assert!(text.contains("Lantern"), "the lantern is in the document");
    assert!(text.contains("600"), "its intensity is authored: {text}");

    undo(&mut app);
    assert!(app.world().get_entity(lantern).is_err(), "undo removes it");
    assert_eq!(history_len(&app), entries);
}

#[test]
fn other_add_operators_take_placement_name_and_parent() {
    let mut app = scene_app();
    let yard = created(&mut app, "entity.add.group", json!({ "name": "Yard" }));
    assert_eq!(name(&app, yard).as_deref(), Some("Yard"));
    for id in [
        "entity.add.camera",
        "entity.add.empty",
        "entity.add.group",
        "entity.add.fog_volume",
        "entity.add.reflection_probe",
        "entity.add.spawn_point",
        "entity.add.audio_source",
        "entity.add.terrain",
        "entity.add.point_light",
        "entity.add.spot_light",
        "entity.add.directional_light",
    ] {
        let label = id.rsplit('.').next().expect("a suffix").replace('_', "");
        let entity = created(
            &mut app,
            id,
            json!({
                "pos": [1.0, 2.0, 3.0],
                "rot": [90.0, 0.0, 0.0],
                "parent": "Yard",
                "name": label,
            }),
        );
        let placed = transform(&app, entity);
        assert_close(placed.translation, Vec3::new(1.0, 2.0, 3.0));
        let (yaw, _, _) = placed.rotation.to_euler(EulerRot::YXZ);
        assert!((yaw - 90f32.to_radians()).abs() < EPS, "{id}: yaw {yaw}");
        assert_eq!(name(&app, entity), Some(label), "{id}");
        assert_eq!(parent_name(&app, entity).as_deref(), Some("Yard"), "{id}");
    }
}

#[test]
fn volumes_take_their_size_as_scale() {
    let mut app = scene_app();
    let fog = created(
        &mut app,
        "entity.add.fog_volume",
        json!({ "size": [8.0, 3.0, 8.0] }),
    );
    assert_close(transform(&app, fog).scale, Vec3::new(8.0, 3.0, 8.0));
    let probe = created(&mut app, "entity.add.reflection_probe", json!({}));
    assert_close(transform(&app, probe).scale, Vec3::splat(10.0));
    let probe = created(
        &mut app,
        "entity.add.reflection_probe",
        json!({ "size": [4.0, 4.0, 6.0] }),
    );
    assert_close(transform(&app, probe).scale, Vec3::new(4.0, 4.0, 6.0));
}

#[test]
fn an_image_with_a_path_is_placed_without_a_picker() {
    let mut app = scene_app();
    let image = created(
        &mut app,
        "entity.add.image",
        json!({ "path": "refs/elevation.png", "pos": [0.0, 1.0, -4.0], "name": "Elevation" }),
    );
    let reference = app
        .world()
        .get::<jackdaw::reference_image::ReferenceImage>(image)
        .expect("a reference image");
    assert_eq!(reference.path, "refs/elevation.png");
    assert_close(
        transform(&app, image).translation,
        Vec3::new(0.0, 1.0, -4.0),
    );
    assert_eq!(name(&app, image).as_deref(), Some("Elevation"));
}

#[test]
fn invalid_params_leave_the_scene_unchanged() {
    let mut app = scene_app();
    let keep = created(&mut app, "entity.add.cube", json!({ "name": "Keep" }));
    let before = scene_text(&mut app);
    let entries = history_len(&app);
    let keep_id = keep.to_bits();
    for (id, params) in [
        ("entity.add.point_light", json!({ "color": "nope" })),
        (
            "entity.add.point_light",
            json!({ "color": [2.0, 0.0, 0.0] }),
        ),
        ("entity.add.point_light", json!({ "intensity": -1.0 })),
        ("entity.add.point_light", json!({ "range": 0.0 })),
        ("entity.add.point_light", json!({ "radius": -0.5 })),
        ("entity.add.point_light", json!({ "cone": "10,20" })),
        ("entity.add.point_light", json!({ "target": "0,0,0" })),
        ("entity.add.point_light", json!({ "material": "nope.mat" })),
        ("entity.add.directional_light", json!({ "range": 5.0 })),
        ("entity.add.directional_light", json!({ "radius": 1.0 })),
        ("entity.add.spot_light", json!({ "cone": [40.0, 20.0] })),
        ("entity.add.spot_light", json!({ "cone": [10.0, 95.0] })),
        (
            "entity.add.spot_light",
            json!({ "target": [1.0, 0.0, 0.0], "rot": [0.0, 10.0, 0.0] }),
        ),
        (
            "entity.add.spot_light",
            json!({ "pos": [1.0, 1.0, 1.0], "target": [1.0, 1.0, 1.0] }),
        ),
        ("entity.add.camera", json!({ "intensity": 5.0 })),
        ("entity.add.empty", json!({ "name": "" })),
        ("entity.add.fog_volume", json!({ "size": [1.0, 0.0, 1.0] })),
        ("entity.add.terrain", json!({ "size": "1,1,1" })),
        ("entity.add.image", json!({ "pos": [0.0, 0.0, 0.0] })),
        ("entity.add.image", json!({ "path": "notes.txt" })),
        ("entity.add.group", json!({ "parent": 4_000_000_000_u64 })),
        (
            "entity.duplicate",
            json!({ "entities": [keep_id], "count": 0 }),
        ),
        (
            "entity.duplicate",
            json!({ "entities": [keep_id], "count": 257 }),
        ),
        ("entity.duplicate", json!({ "entities": "[4000000000]" })),
        (
            "entity.duplicate",
            json!({ "entities": [keep_id], "spread": 2 }),
        ),
    ] {
        let outcome = call(&mut app, id, params.clone());
        assert_eq!(outcome["outcome"], "invalid", "{id} {params}: {outcome}");
        assert_eq!(scene_text(&mut app), before, "{id} {params}");
        assert_eq!(history_len(&app), entries, "{id} {params}");
    }

    for (id, params) in [
        ("entity.add.point_light", json!({ "shadows": "maybe" })),
        ("entity.add.spot_light", json!({ "cone": "wide" })),
        (
            "entity.duplicate",
            json!({ "entities": [keep_id], "offset": "far" }),
        ),
    ] {
        let refused = app.world_mut().run_system_cached_with(
            call_operator_handler,
            Some(json!({ "id": id, "params": params })),
        );
        assert!(
            matches!(refused, Ok(Err(_))),
            "{id} {params}: a value of the wrong type is refused before dispatch"
        );
        app.update();
        assert_eq!(scene_text(&mut app), before, "{id} {params}");
    }
}

/// A fence run: one post, then one duplicate call for the rest of it.
#[test]
fn a_duplicate_run_places_every_copy_in_one_undo_step() {
    let mut app = scene_app();
    let post = created(
        &mut app,
        "entity.add.cube",
        json!({ "name": "Post", "size": [0.2, 1.2, 0.2], "pos": [0.0, 0.6, 0.0] }),
    );
    let entries = history_len(&app);

    let outcome = call(
        &mut app,
        "entity.duplicate",
        json!({ "entities": [post.to_bits()], "count": 5, "offset": [1.0, 0.0, 0.0] }),
    );
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    let copies = ids(&outcome);
    assert_eq!(copies.len(), 5, "{outcome}");
    for (step, copy) in copies.iter().enumerate() {
        let at = Vec3::new(1.0 + step as f32, 0.6, 0.0);
        assert_close(transform(&app, *copy).translation, at);
        assert!(app.world().get::<Brush>(*copy).is_some());
    }
    assert_eq!(brushes_named(&mut app, "Post").len(), 6);
    assert_eq!(history_len(&app), entries + 1, "one undo step");
    let mut selected = app.world().resource::<Selection>().entities.to_vec();
    selected.sort();
    let mut wanted = copies.clone();
    wanted.sort();
    assert_eq!(selected, wanted, "the copies are selected");

    let text = scene_text(&mut app);
    for x in 1..=5 {
        assert!(
            text.contains(&format!("x: {x}.0, y: 0.6")),
            "copy {x} is authored where it stands: {text}"
        );
    }

    undo(&mut app);
    assert_eq!(
        brushes_named(&mut app, "Post"),
        vec![post],
        "one undo takes the run back"
    );
    assert_eq!(history_len(&app), entries);
}

#[test]
fn a_duplicate_from_the_keybind_is_one_undo_step() {
    let mut app = scene_app();
    let post = created(&mut app, "entity.add.cube", json!({ "name": "Post" }));
    app.world_mut().resource_mut::<Selection>().entities = vec![post];
    let entries = history_len(&app);
    app.world_mut()
        .operator("entity.duplicate")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        })
        .call()
        .expect("entity.duplicate dispatches")
        .assert_finished();
    app.update();
    assert_eq!(brushes_named(&mut app, "Post").len(), 2);
    assert_eq!(history_len(&app), entries + 1);
    undo(&mut app);
    assert_eq!(brushes_named(&mut app, "Post"), vec![post]);
}

#[test]
fn a_duplicate_turns_each_step_and_lands_under_the_parent() {
    let mut app = scene_app();
    let ring = created(&mut app, "entity.add.group", json!({ "name": "Ring" }));
    let stone = created(
        &mut app,
        "entity.add.cube",
        json!({ "name": "Stone", "pos": [2.0, 0.0, 0.0] }),
    );
    let outcome = call(
        &mut app,
        "entity.duplicate",
        json!({
            "entities": format!("{}", stone.to_bits()),
            "count": 3,
            "offset": [0.0, 1.0, 0.0],
            "rot": [30.0, 0.0, 0.0],
            "parent": ring.to_bits(),
        }),
    );
    let copies = ids(&outcome);
    assert_eq!(copies.len(), 3, "{outcome}");
    for (step, copy) in copies.iter().enumerate() {
        let k = (step + 1) as f32;
        let placed = transform(&app, *copy);
        assert_close(placed.translation, Vec3::new(2.0, k, 0.0));
        let (yaw, _, _) = placed.rotation.to_euler(EulerRot::YXZ);
        assert!((yaw - (30.0 * k).to_radians()).abs() < EPS, "yaw {yaw}");
        assert_eq!(parent_name(&app, *copy).as_deref(), Some("Ring"));
    }
    assert_eq!(parent_name(&app, stone), None, "the source stays put");
}

#[test]
fn a_duplicate_run_takes_ids_from_an_earlier_call_in_a_batch() {
    let mut app = scene_app();
    let answer = app
        .world_mut()
        .run_system_cached_with(
            jackdaw::remote::server::batch_handler,
            Some(json!({
                "calls": [
                    { "id": "entity.add.cube", "params": { "name": "Rail" } },
                    { "id": "entity.duplicate", "params": {
                        "entities": ["$0.entities.0"],
                        "count": 4,
                        "offset": [0.0, 0.0, 2.0],
                    } },
                ],
            })),
        )
        .expect("the handler ran")
        .unwrap_or_else(|err| panic!("batch refused: {}", err.message));
    app.update();
    assert_eq!(ids(&answer["calls"][1]).len(), 4, "{answer}");
    assert_eq!(brushes_named(&mut app, "Rail").len(), 5);
}

#[test]
fn a_duplicate_replaces_the_selection_with_its_copies() {
    let mut app = scene_app();
    let first = created(&mut app, "entity.add.cube", json!({ "name": "First" }));
    let second = created(&mut app, "entity.add.cube", json!({ "name": "Second" }));
    jackdaw::selection::select_only(app.world_mut(), first);
    app.update();

    let copies = ids(&call(
        &mut app,
        "entity.duplicate",
        json!({ "entities": [second.to_bits()] }),
    ));
    assert_eq!(copies.len(), 1);
    assert!(
        app.world().get::<Selected>(first).is_none(),
        "the earlier selection is let go"
    );
    assert_eq!(
        app.world().resource::<Selection>().entities.to_vec(),
        copies
    );
}

#[test]
fn a_refused_duplicate_leaves_the_selection_alone() {
    let mut app = scene_app();
    let post = created(&mut app, "entity.add.cube", json!({ "name": "Post" }));
    jackdaw::selection::select_only(app.world_mut(), post);
    app.update();
    let refused = call(
        &mut app,
        "entity.duplicate",
        json!({ "entities": [post.to_bits()], "count": 0 }),
    );
    assert_eq!(refused["outcome"], "invalid", "{refused}");
    assert_eq!(
        app.world().resource::<Selection>().entities.to_vec(),
        vec![post]
    );
    assert!(app.world().get::<Selected>(post).is_some());
}

#[test]
fn a_spot_light_or_camera_given_only_a_position_still_faces_the_origin() {
    let mut app = scene_app();
    for id in ["entity.add.spot_light", "entity.add.camera"] {
        let at = Vec3::new(6.0, 4.0, -2.0);
        let entity = created(&mut app, id, json!({ "pos": [6.0, 4.0, -2.0] }));
        let placed = transform(&app, entity);
        assert_close(placed.translation, at);
        assert_close(*placed.forward(), (-at).normalize());
    }
}

#[test]
fn a_bare_name_clause_duplicates_the_named_node() {
    let mut app = scene_app();
    let group = created(&mut app, "entity.add.group", json!({ "name": "Yard" }));
    let crate_node = created(
        &mut app,
        "entity.add.cube",
        json!({ "name": "Crate", "parent": group.to_bits() }),
    );
    jackdaw::selection::clear_selection_in_world(app.world_mut());
    app.update();

    jackdaw::boot_ops::run_op_clause(
        app.world_mut(),
        "entity.duplicate name=Crate count=3 offset=1,0,0",
    )
    .expect("the clause dispatches")
    .assert_finished();
    app.update();

    let crates = brushes_named(&mut app, "Crate");
    assert_eq!(crates.len(), 4, "three copies of Crate");
    for copy in crates.into_iter().filter(|&copy| copy != crate_node) {
        assert_eq!(parent_name(&app, copy).as_deref(), Some("Yard"));
    }
}

#[test]
fn scene_wide_nodes_stay_top_level_in_an_open_2d_scene() {
    let mut app = util::editor_test_app();
    jackdaw::entity_ops::seed_2d_scene_root(app.world_mut());
    app.update();
    for id in [
        "entity.add.terrain",
        "entity.add.reflection_probe",
        "entity.add.spawn_point",
    ] {
        let entity = created(&mut app, id, json!({}));
        assert!(
            app.world().get::<ChildOf>(entity).is_none(),
            "{id} went under the 2D scene root"
        );
    }
}

#[test]
fn a_stage_named_with_spaces_and_punctuation_gets_its_group() {
    let mut app = scene_app();
    for stage in ["East wing, phase 2 (draft)!", "   "] {
        let answer = app
            .world_mut()
            .run_system_cached_with(
                jackdaw::remote::server::batch_handler,
                Some(json!({
                    "stage": stage,
                    "calls": [{ "id": "entity.add.cube", "params": { "name": "Wall" } }],
                })),
            )
            .expect("the handler ran")
            .unwrap_or_else(|err| panic!("stage {stage:?} refused: {}", err.message));
        app.update();
        let group = Entity::try_from_bits(answer["stage"]["entity"].as_u64().expect("an id"))
            .expect("a valid id");
        assert_eq!(name(&app, group).as_deref(), Some(stage), "{answer}");
    }
}

#[test]
fn a_run_is_offset_from_the_authored_transform() {
    let mut app = scene_app();
    let post = created(
        &mut app,
        "entity.add.cube",
        json!({ "name": "Post", "pos": [0.0, 1.0, 0.0] }),
    );
    // A live-only nudge the document never saw.
    app.world_mut()
        .get_mut::<Transform>(post)
        .expect("a transform")
        .translation = Vec3::new(9.0, 9.0, 9.0);
    let copies = ids(&call(
        &mut app,
        "entity.duplicate",
        json!({ "entities": [post.to_bits()], "count": 2, "offset": [1.0, 0.0, 0.0] }),
    ));
    assert_close(
        transform(&app, copies[0]).translation,
        Vec3::new(1.0, 1.0, 0.0),
    );
    assert_close(
        transform(&app, copies[1]).translation,
        Vec3::new(2.0, 1.0, 0.0),
    );
}

#[test]
fn a_run_of_a_node_without_a_transform_is_refused() {
    let mut app = scene_app();
    let spawned = app
        .world_mut()
        .run_system_cached_with(
            jackdaw::remote::server::apply_bsn_handler,
            Some(json!({ "source": "#Marker\n" })),
        )
        .expect("the handler ran")
        .unwrap_or_else(|err| panic!("apply refused: {}", err.message));
    app.update();
    let marker = ids(&spawned)[0];
    assert!(app.world().get::<Transform>(marker).is_none());
    let before = scene_text(&mut app);

    let refused = call(
        &mut app,
        "entity.duplicate",
        json!({ "entities": [marker.to_bits()], "offset": [1.0, 0.0, 0.0] }),
    );
    assert_eq!(refused["outcome"], "invalid", "{refused}");
    assert!(
        refused["warnings"].to_string().contains("no Transform"),
        "{refused}"
    );
    assert_eq!(scene_text(&mut app), before);

    let copy = ids(&call(
        &mut app,
        "entity.duplicate",
        json!({ "entities": [marker.to_bits()] }),
    ))[0];
    assert!(
        app.world().get::<Transform>(copy).is_none(),
        "a plain copy gains no Transform"
    );
}

#[test]
fn a_long_run_costs_about_the_same_per_copy() {
    let mut app = scene_app();
    let post = created(&mut app, "entity.add.cube", json!({ "name": "Post" }));
    let mut timed = |count: u32| {
        let started = std::time::Instant::now();
        let copies = ids(&call(
            &mut app,
            "entity.duplicate",
            json!({ "entities": [post.to_bits()], "count": count, "offset": [1.0, 0.0, 0.0] }),
        ));
        assert_eq!(copies.len(), count as usize);
        started.elapsed()
    };
    let short = timed(32);
    let long = timed(256);
    assert!(
        long < short * 24,
        "256 copies took {long:?} against {short:?} for 32: not linear"
    );
}
