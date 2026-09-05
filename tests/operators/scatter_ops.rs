//! The scatter ops a caller with no pointer needs: moving a hand-authored
//! group into the terrain's stored scatter, naming a group to act on, and
//! taking one placement back out again.
//!
//! This is how a script or a remote client reaches a scene that was
//! generated rather than scattered: the groups are already in the file as
//! entities, beside the terrain, and every re-scatter would double them
//! until one of these has been run.

use crate::util;

use bevy::prelude::*;
use jackdaw::boot_ops::{SELECTION_FALLBACK_OPS, run_op_clause};
use jackdaw::commands::CommandHistory;
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_scene_types::{GltfSource, ScatterGroup};

/// Run one clause and tick the frame its queued work needs, the way the
/// boot queue's frame gap does.
#[track_caller]
fn run(app: &mut App, clause: &str) {
    let result = run_op_clause(app.world_mut(), clause)
        .unwrap_or_else(|err| panic!("{clause}: dispatch errored: {err}"));
    assert_eq!(
        result,
        OperatorResult::Finished,
        "{clause} reported {result:?}"
    );
    app.update();
}

/// A scene with one terrain that has a document to store scatter in.
///
/// The sidecar path is minted here rather than by the editor's own system,
/// which is gated on a state a headless test never enters.
fn scene_with_a_terrain() -> (App, Entity) {
    let mut app = util::editor_test_app();
    run(&mut app, "scene.new");
    run(&mut app, "entity.add.terrain");
    let mut query = app
        .world_mut()
        .query_filtered::<Entity, With<jackdaw_scene_types::Terrain>>();
    let terrain = query
        .iter(app.world())
        .next()
        .expect("entity.add.terrain authored a terrain");

    let data_path = "scene.terrain-0.jdterrain";
    let mut document = jackdaw_terrain::RegionTerrainData::default();
    document.regions.set_height(0, 0, 1.0);
    app.world_mut()
        .resource_mut::<jackdaw::terrain::TerrainDataStore>()
        .insert(data_path, document);
    app.world_mut()
        .get_mut::<jackdaw_scene_types::Terrain>(terrain)
        .expect("a terrain")
        .data_path = data_path.to_string();
    app.update();
    (app, terrain)
}

/// A group of models placed by hand beside the terrain, the shape
/// `gen_zone` and a drag from the asset browser both leave.
fn hand_authored_group(app: &mut App) -> (Entity, Entity) {
    let group = app
        .world_mut()
        .spawn((
            Name::new("Scatter_Trees"),
            Transform::from_xyz(3.0, 0.0, -2.0),
            Visibility::default(),
        ))
        .id();
    let model = app
        .world_mut()
        .spawn((
            Name::new("Tree"),
            GltfSource {
                path: "kit/Tree.gltf".to_string(),
                scene_index: 0,
            },
            Transform::from_xyz(1.0, 0.0, 1.0),
            Visibility::default(),
            ChildOf(group),
        ))
        .id();
    app.update();
    (group, model)
}

/// The stored placements of `terrain`, by group key and count.
fn stored_groups(app: &App, terrain: Entity) -> Vec<(String, usize)> {
    let data_path = app
        .world()
        .get::<jackdaw_scene_types::Terrain>(terrain)
        .expect("a terrain")
        .data_path
        .clone();
    jackdaw::terrain::scatter_data::group_counts(
        app.world().resource::<jackdaw::terrain::TerrainDataStore>(),
        &data_path,
    )
}

/// Adoption moves the group into the terrain's document, from the
/// selection alone: the entities go and a placement stands where each
/// model stood.
#[test]
fn an_adopt_clause_stores_the_selected_group_on_the_terrain() {
    let (mut app, terrain) = scene_with_a_terrain();
    let (group, model) = hand_authored_group(&mut app);
    let stood_at = app
        .world()
        .get::<GlobalTransform>(model)
        .copied()
        .expect("transform propagation ran")
        .translation();
    app.world_mut().resource_mut::<Selection>().entities = vec![group];

    run(&mut app, "terrain.scatter.adopt");

    assert!(app.world().get_entity(group).is_err(), "the group is gone");
    assert!(app.world().get_entity(model).is_err(), "the model is gone");
    assert_eq!(
        stored_groups(&app, terrain),
        vec![("Scatter_Trees".to_string(), 1)]
    );

    let data_path = app
        .world()
        .get::<jackdaw_scene_types::Terrain>(terrain)
        .unwrap()
        .data_path
        .clone();
    let store = app.world().resource::<jackdaw::terrain::TerrainDataStore>();
    let data = store.get(&data_path).expect("a document");
    let (coord, _, placement) = data.placements().next().expect("one placement");
    assert_eq!(
        data.scatter
            .asset(placement.asset)
            .map(|e| e.asset.as_str()),
        Some("kit/Tree.gltf")
    );
    assert!(
        data.placement_position(coord, placement)
            .abs_diff_eq(stood_at, 1e-3),
        "the placement stands where the model stood"
    );
}

/// One placement comes back out as an ordinary model entity, which is how
/// a hand edits one instance of a stored group.
#[test]
fn a_promote_clause_turns_one_placement_back_into_an_entity() {
    let (mut app, terrain) = scene_with_a_terrain();
    let (group, _) = hand_authored_group(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![group];
    run(&mut app, "terrain.scatter.adopt");

    run(
        &mut app,
        "terrain.scatter.promote key=Scatter_Trees index=0",
    );

    assert!(stored_groups(&app, terrain).is_empty());
    let mut query = app.world_mut().query::<&GltfSource>();
    assert_eq!(
        query
            .iter(app.world())
            .filter(|source| source.path == "kit/Tree.gltf")
            .count(),
        1,
        "the promoted placement is a model entity again"
    );
}

/// A clear names a stored group and empties it.
#[test]
fn a_clear_clause_empties_a_stored_group() {
    let (mut app, terrain) = scene_with_a_terrain();
    let (group, _) = hand_authored_group(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![group];
    run(&mut app, "terrain.scatter.adopt");
    assert_eq!(stored_groups(&app, terrain).len(), 1);

    run(&mut app, "terrain.scatter.clear group=Scatter_Trees");

    assert!(stored_groups(&app, terrain).is_empty());
}

/// The whole adoption is one history entry: the user asked for one thing,
/// so one undo puts the scene back and empties the document again.
#[test]
fn an_adoption_is_one_undo_entry() {
    let (mut app, terrain) = scene_with_a_terrain();
    let (group, _) = hand_authored_group(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![group];

    let before = app.world().resource::<CommandHistory>().undo_stack.len();
    run(&mut app, "terrain.scatter.adopt");
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        before + 1,
        "adoption must land as a single entry"
    );

    run(&mut app, "history.undo");

    assert!(
        stored_groups(&app, terrain).is_empty(),
        "undo left placements behind in the document"
    );
    let mut query = app.world_mut().query::<&GltfSource>();
    assert_eq!(
        query.iter(app.world()).count(),
        1,
        "undo did not put the model entity back"
    );
}

/// A group a previous build stamped as entities is adopted under the key
/// it already carries, which is how such a scene moves to stored scatter
/// without its groups changing name.
#[test]
fn adopting_a_group_that_is_already_stamped_keeps_its_key() {
    let (mut app, terrain) = scene_with_a_terrain();
    let group = app
        .world_mut()
        .spawn((
            Name::new("Renamed"),
            ScatterGroup {
                generator: "terrain.scatter".to_string(),
                key: "Trees".to_string(),
            },
            Transform::default(),
            Visibility::default(),
            ChildOf(terrain),
        ))
        .id();
    app.world_mut().spawn((
        Name::new("Tree"),
        GltfSource {
            path: "kit/Tree.gltf".to_string(),
            scene_index: 0,
        },
        Transform::from_xyz(1.0, 0.0, 1.0),
        Visibility::default(),
        ChildOf(group),
    ));
    app.update();
    app.world_mut().resource_mut::<Selection>().entities = vec![group];

    run(&mut app, "terrain.scatter.adopt");

    assert_eq!(stored_groups(&app, terrain), vec![("Trees".to_string(), 1)]);
    assert!(app.world().get_entity(group).is_err());
}

/// Naming a group selects it, which is what the panel's buttons and a
/// script both need before acting on one.
#[test]
fn a_group_select_clause_selects_the_group_with_that_key() {
    let (mut app, terrain) = scene_with_a_terrain();
    let group = app
        .world_mut()
        .spawn((
            Name::new("Trees"),
            ScatterGroup {
                generator: "terrain.scatter".to_string(),
                key: "Trees".to_string(),
            },
            Transform::default(),
            Visibility::default(),
            ChildOf(terrain),
        ))
        .id();
    app.update();

    run(&mut app, "terrain.scatter.group.select key=Trees");

    assert_eq!(app.world().resource::<Selection>().primary(), Some(group));
}

/// The clause form of adoption acts on the selection, so the operator has
/// to be in the fallback list or every scripted run of it is short a target.
#[test]
fn adopt_takes_its_target_from_the_selection() {
    assert!(SELECTION_FALLBACK_OPS.contains(&"terrain.scatter.adopt"));
}

/// A scripted clear names its group and has no selection to fall back on.
/// Finishing silently there reads as "the group is gone", which is the
/// one answer the caller must not be given.
#[test]
fn clearing_with_no_terrain_says_so_rather_than_finishing_quietly() {
    let mut app = util::editor_test_app();
    run(&mut app, "scene.new");
    app.world_mut()
        .get_resource_or_init::<jackdaw_api_internal::operator::OperatorWarnings>()
        .0
        .clear();

    run(&mut app, "terrain.scatter.clear group=Undergrowth");

    let warnings = app
        .world()
        .resource::<jackdaw_api_internal::operator::OperatorWarnings>()
        .0
        .clone();
    assert!(
        warnings.iter().any(|w| w.contains("no terrain resolved")),
        "the caller has to hear that nothing was cleared: {warnings:?}"
    );
    let report = app
        .world()
        .resource::<jackdaw::terrain::scatter::TerrainScatterReport>();
    assert!(
        report.message.contains("no terrain resolved"),
        "the panel says the same thing: {:?}",
        report.message
    );
}
