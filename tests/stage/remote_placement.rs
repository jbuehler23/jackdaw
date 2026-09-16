//! Placing things in a scene from outside the editor.
//!
//! A caller with no pointer cannot see the ground, cannot tell two entities
//! of one name apart, and cannot watch a file change under an open tab. These
//! cover the operators that answer each of those, and the reopen and undo
//! paths they lean on.

use crate::util;

use bevy::prelude::*;
use jackdaw::boot_ops::{run_op_clause, run_op_clause_as_user};
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::OperatorReports;
use jackdaw_commands::CommandHistory;

const ISA_TYPE: &str = "jackdaw::prefab::components::IsA";

const LAMP_PREFAB: &str = "#Lamp\n\
     jackdaw::prefab::components::Prefab\n\
     jackdaw::prefab::components::PrefabEntityId(0)\n\
     bevy_transform::components::transform::Transform\n";

const ZONE_SCENE: &str = "#Zone\n\
     bevy_transform::components::transform::Transform\n";

#[track_caller]
fn run(app: &mut App, clause: &str) -> OperatorResult {
    let result = run_op_clause(app.world_mut(), clause)
        .unwrap_or_else(|err| panic!("{clause}: dispatch errored: {err}"));
    app.update();
    result
}

#[track_caller]
fn run_finished(app: &mut App, clause: &str) {
    let result = run(app, clause);
    assert_eq!(
        result,
        OperatorResult::Finished,
        "{clause} reported {result:?}"
    );
}

/// What the operator that just ran told its caller.
fn reports(app: &mut App) -> Vec<String> {
    app.world_mut()
        .get_resource_or_init::<OperatorReports>()
        .0
        .clone()
}

fn clear_reports(app: &mut App) {
    app.world_mut()
        .get_resource_or_init::<OperatorReports>()
        .0
        .clear();
}

/// A scene holding one terrain whose ground stands five metres up around the
/// origin, and nothing beyond the cells that were sculpted.
fn scene_with_ground() -> App {
    scene_with_ground_at(Transform::IDENTITY)
}

/// [`scene_with_ground`] with the terrain standing somewhere other than the
/// origin, which is where a zone's ground usually sits.
fn scene_with_ground_at(stands: Transform) -> App {
    let mut app = util::editor_test_app();
    run_finished(&mut app, "scene.new");
    run_finished(&mut app, "entity.add.terrain");
    let mut query = app
        .world_mut()
        .query_filtered::<Entity, With<jackdaw_scene_types::Terrain>>();
    let terrain = query
        .iter(app.world())
        .next()
        .expect("entity.add.terrain authored a terrain");
    app.world_mut().entity_mut(terrain).insert(stands);

    let data_path = "scene.terrain-0.jdterrain";
    let mut document = jackdaw_terrain::RegionTerrainData::default();
    for z in 0..4 {
        for x in 0..4 {
            document.regions.set_height(x, z, 5.0);
        }
    }
    let mut store = app
        .world_mut()
        .resource_mut::<jackdaw::terrain::TerrainDataStore>();
    store.insert(data_path, document);
    // One metre per cell with vertex (0, 0) at the entity, so a world point
    // is a grid coordinate and the test can say where the ground is.
    store.set_grid(data_path, jackdaw_terrain::sidecar::GridGeometry::DEFAULT);
    app.world_mut()
        .get_mut::<jackdaw_scene_types::Terrain>(terrain)
        .expect("a terrain")
        .data_path = data_path.to_string();
    app.update();
    app
}

/// A model standing well above the ground, selected, as a caller that just
/// placed one leaves it.
fn floating_model(app: &mut App, at: Vec3) -> Entity {
    let entity = app
        .world_mut()
        .spawn((
            Name::new("Lamp"),
            Transform::from_translation(at),
            Visibility::default(),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    jackdaw::selection::select_only(app.world_mut(), entity);
    app.update();
    entity
}

#[test]
fn the_height_under_a_point_is_the_ground_there_and_nothing_off_the_terrain() {
    let mut app = scene_with_ground();

    clear_reports(&mut app);
    run_finished(&mut app, "terrain.height x=1.5 z=1.5");
    assert_eq!(reports(&mut app), vec!["5".to_string()]);

    clear_reports(&mut app);
    let result = run(&mut app, "terrain.height x=1000.0 z=1000.0");
    assert_eq!(result, OperatorResult::Cancelled, "no ground out there");
    assert!(
        reports(&mut app).is_empty(),
        "nothing to report off the terrain"
    );
}

#[test]
fn a_selected_model_drops_onto_the_ground_and_one_undo_lifts_it_back() {
    let mut app = scene_with_ground();
    let lamp = floating_model(&mut app, Vec3::new(1.5, 40.0, 1.5));

    run_finished(&mut app, "entity.snap_to_ground");

    assert_eq!(
        app.world()
            .get::<Transform>(lamp)
            .expect("a transform")
            .translation
            .y,
        5.0,
    );

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    app.update();

    assert_eq!(
        app.world()
            .get::<Transform>(lamp)
            .expect("a transform")
            .translation
            .y,
        40.0,
    );
}

#[test]
fn a_model_placed_without_a_height_lands_on_the_ground() {
    let mut app = scene_with_ground();

    run_finished(
        &mut app,
        "entity.place_gltf path=kit/Lamp.gltf pos_x=1.5 pos_z=1.5",
    );

    let mut query = app
        .world_mut()
        .query_filtered::<&Transform, With<jackdaw_scene_types::GltfSource>>();
    let placed: Vec<f32> = query.iter(app.world()).map(|tf| tf.translation.y).collect();
    assert_eq!(placed, vec![5.0]);
}

/// A project as one is laid out: the zone and the prefab it instances both
/// under `assets/`, and the editor running from somewhere else entirely.
fn project_with_a_lamp() -> (App, tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("assets/prefabs")).expect("assets tree");
    let scene = tmp.path().join("assets/zone.bsn");
    std::fs::write(&scene, ZONE_SCENE).expect("scene written");
    std::fs::write(tmp.path().join("assets/prefabs/lamp.bsn"), LAMP_PREFAB)
        .expect("prefab written");

    let mut app = util::editor_test_app();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: tmp.path().to_path_buf(),
            config: default(),
        });
    app.update();
    (app, tmp, scene)
}

/// The names of every entity in the scene, parents before children.
fn names(app: &mut App) -> Vec<String> {
    let mut query = app
        .world_mut()
        .query_filtered::<&Name, Without<jackdaw::EditorEntity>>();
    query.iter(app.world()).map(ToString::to_string).collect()
}

fn entities_named(app: &mut App, wanted: &str) -> Vec<Entity> {
    let mut query = app
        .world_mut()
        .query_filtered::<(Entity, &Name), Without<jackdaw::EditorEntity>>();
    query
        .iter(app.world())
        .filter(|(_, name)| name.as_str() == wanted)
        .map(|(entity, _)| entity)
        .collect()
}

#[test]
fn a_scene_finds_its_prefab_from_a_directory_the_editor_was_not_launched_in() {
    let (mut app, tmp, scene) = project_with_a_lamp();
    std::fs::write(
        &scene,
        "#Zone\n\
         bevy_transform::components::transform::Transform\n\
         Children [\n\
             jackdaw::prefab::components::IsA { source: \"prefabs/lamp.bsn\" }\n\
             jackdaw::prefab::components::PrefabEntityId(0)\n\
         ]\n",
    )
    .expect("scene written");

    run_finished(&mut app, "scene.open path=assets/zone.bsn");

    assert!(
        names(&mut app).iter().any(|name| name == "Lamp"),
        "the instance resolved against the scene's own directory, not the working directory",
    );
    assert!(
        app.world()
            .resource::<jackdaw::prefab::PrefabAstCache>()
            .get(&tmp.path().join("assets/prefabs/lamp.bsn"))
            .is_some(),
        "the prefab was cached under the file it names",
    );
}

#[test]
fn an_instance_lands_under_the_parent_it_names() {
    let (mut app, _tmp, _scene) = project_with_a_lamp();
    run_finished(&mut app, "scene.open path=assets/zone.bsn");

    jackdaw::commands::SpawnedEntities::watch(app.world_mut());
    run_finished(
        &mut app,
        "prefab.spawn_instance path=prefabs/lamp.bsn pos_x=1.0 pos_y=0.0 pos_z=2.0 parent=Zone",
    );

    let lamp = *entities_named(&mut app, "Lamp")
        .first()
        .expect("an instance");
    assert!(
        !jackdaw::commands::SpawnedEntities::take(app.world_mut()).is_empty(),
        "the call said which entity it added, however deep it landed",
    );
    let parent = app.world().get::<ChildOf>(lamp).map(ChildOf::parent);
    assert_eq!(
        parent
            .and_then(|parent| app.world().get::<Name>(parent))
            .map(Name::to_string),
        Some("Zone".to_string()),
    );
}

#[test]
fn an_instance_with_no_parent_joins_the_scene_root() {
    let (mut app, _tmp, _scene) = project_with_a_lamp();
    run_finished(&mut app, "scene.open path=assets/zone.bsn");

    run_finished(
        &mut app,
        "prefab.spawn_instance path=prefabs/lamp.bsn pos_x=1.0 pos_y=0.0 pos_z=2.0",
    );

    let lamp = *entities_named(&mut app, "Lamp")
        .first()
        .expect("an instance");
    assert!(
        app.world().get::<ChildOf>(lamp).is_some(),
        "the instance joined the root rather than standing beside it",
    );
    assert_eq!(
        app.world()
            .resource::<jackdaw_bsn::SceneBsnAst>()
            .roots
            .len(),
        1,
        "the document still has one root, so a save reindents nothing",
    );
}

/// Two instances of one prefab carry one name between them, which is the
/// case a caller cannot spell.
fn two_namesakes(app: &mut App) -> (Entity, Entity) {
    for _ in 0..2 {
        run_finished(
            app,
            "prefab.spawn_instance path=prefabs/lamp.bsn pos_x=0.0 pos_y=0.0 pos_z=0.0",
        );
    }
    let lamps = entities_named(app, "Lamp");
    assert_eq!(lamps.len(), 2, "two instances stand in the scene");
    (lamps[0], lamps[1])
}

#[test]
fn selecting_by_id_tells_two_entities_of_one_name_apart() {
    let (mut app, _tmp, _scene) = project_with_a_lamp();
    run_finished(&mut app, "scene.open path=assets/zone.bsn");
    let (first, second) = two_namesakes(&mut app);

    run_finished(
        &mut app,
        &format!("selection.select entity={}", second.to_bits()),
    );

    assert_eq!(app.world().resource::<Selection>().entities, vec![second]);

    run_finished(
        &mut app,
        &format!(
            "selection.select entities={},{}",
            first.to_bits(),
            second.to_bits()
        ),
    );
    assert_eq!(app.world().resource::<Selection>().entities.len(), 2);
}

#[test]
fn reparenting_by_id_moves_the_namesake_that_was_named() {
    let (mut app, _tmp, _scene) = project_with_a_lamp();
    run_finished(&mut app, "scene.open path=assets/zone.bsn");
    let (first, second) = two_namesakes(&mut app);

    run_finished(
        &mut app,
        &format!(
            "entity.reparent child={} parent={}",
            second.to_bits(),
            first.to_bits()
        ),
    );

    assert_eq!(
        app.world().get::<ChildOf>(second).map(ChildOf::parent),
        Some(first),
    );
}

#[test]
fn deleting_by_id_takes_one_namesake_and_leaves_the_other() {
    let (mut app, _tmp, _scene) = project_with_a_lamp();
    run_finished(&mut app, "scene.open path=assets/zone.bsn");
    let (first, second) = two_namesakes(&mut app);

    run_finished(
        &mut app,
        &format!("entity.delete entities={}", second.to_bits()),
    );

    assert!(
        app.world().get_entity(first).is_ok(),
        "the other one stands"
    );
    assert!(
        app.world().get_entity(second).is_err(),
        "the named one is gone"
    );
}

#[test]
fn reopening_a_scene_rereads_the_file_and_keeps_the_selection_by_name() {
    let (mut app, _tmp, scene) = project_with_a_lamp();
    run_finished(&mut app, "scene.open path=assets/zone.bsn");
    let zone = *entities_named(&mut app, "Zone").first().expect("the root");
    jackdaw::selection::select_only(app.world_mut(), zone);
    app.update();

    std::fs::write(&scene, ZONE_WITH_A_BENCH).expect("scene rewritten");

    run_finished(&mut app, "scene.open path=assets/zone.bsn reload=true");

    assert!(
        names(&mut app).iter().any(|name| name == "Bench"),
        "the tab shows what the file now holds",
    );
    let zone = *entities_named(&mut app, "Zone").first().expect("the root");
    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![zone],
        "the selection came back by name",
    );
}

/// The zone with one more thing standing in it, as an edit from outside the
/// editor leaves the file.
const ZONE_WITH_A_BENCH: &str = "#Zone\n\
     bevy_transform::components::transform::Transform\n\
     Children [\n\
         #Bench\n\
         bevy_transform::components::transform::Transform\n\
     ]\n";

#[test]
fn reopening_a_scene_the_file_has_moved_on_from_rereads_it() {
    let (mut app, _tmp, scene) = project_with_a_lamp();
    run_finished(&mut app, "scene.open path=assets/zone.bsn");

    std::fs::write(&scene, ZONE_WITH_A_BENCH).expect("scene rewritten");

    run_finished(&mut app, "scene.open path=assets/zone.bsn");

    assert!(
        names(&mut app).iter().any(|name| name == "Bench"),
        "the tab shows what the file now holds, without being asked to reread",
    );
}

#[test]
fn reopening_a_scene_holding_unsaved_edits_does_not_reread_it() {
    let (mut app, _tmp, scene) = project_with_a_lamp();
    run_finished(&mut app, "scene.open path=assets/zone.bsn");
    let index = app.world().resource::<jackdaw::scenes::Scenes>().active;
    app.world_mut()
        .resource_mut::<jackdaw::scenes::Scenes>()
        .tabs[index]
        .dirty = true;

    std::fs::write(&scene, ZONE_WITH_A_BENCH).expect("scene rewritten");

    run_finished(&mut app, "scene.open path=assets/zone.bsn reload=true");

    assert!(
        !names(&mut app).iter().any(|name| name == "Bench"),
        "the unsaved edits stand and the file was left on disk",
    );
}

#[test]
fn undoing_a_group_added_before_a_prefab_spawn_takes_it_back_without_panicking() {
    let (mut app, _tmp, _scene) = project_with_a_lamp();
    run_finished(&mut app, "scene.open path=assets/zone.bsn");

    let added = run_op_clause_as_user(app.world_mut(), "entity.add.group name=Props parent=Zone")
        .expect("the group dispatched");
    assert_eq!(added, OperatorResult::Finished);
    app.update();
    // The spawn respawns the scene, so the entity the group's move named is
    // gone by the time the undo reaches it.
    run_finished(
        &mut app,
        "prefab.spawn_instance path=prefabs/lamp.bsn pos_x=0.0 pos_y=0.0 pos_z=0.0",
    );

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    app.update();

    assert!(
        app.world()
            .resource::<jackdaw_bsn::SceneBsnAst>()
            .entities_with_component(ISA_TYPE)
            .len()
            <= 1,
        "the editor is still standing and the document still reads",
    );
}

#[test]
fn the_ground_under_a_point_follows_the_terrain_that_holds_it() {
    let mut app = scene_with_ground_at(Transform::from_translation(Vec3::new(100.0, 7.0, 100.0)));

    clear_reports(&mut app);
    run_finished(&mut app, "terrain.height x=101.5 z=101.5");
    assert_eq!(
        reports(&mut app),
        vec!["12".to_string()],
        "the height is read in the terrain's own space and answered in the world's",
    );

    clear_reports(&mut app);
    assert_eq!(
        run(&mut app, "terrain.height x=1.5 z=1.5"),
        OperatorResult::Cancelled,
        "the origin is no longer over the terrain",
    );
}

#[test]
fn an_entity_under_a_moved_parent_drops_to_the_ground_in_world_space() {
    let mut app = scene_with_ground();
    let parent = app
        .world_mut()
        .spawn((
            Name::new("Props"),
            Transform::from_translation(Vec3::new(0.0, 12.0, 0.0)),
            Visibility::default(),
        ))
        .id();
    let lamp = floating_model(&mut app, Vec3::new(1.5, 40.0, 1.5));
    app.world_mut().entity_mut(lamp).insert(ChildOf(parent));
    app.update();

    run_finished(&mut app, "entity.snap_to_ground");

    assert_eq!(
        app.world()
            .get::<GlobalTransform>(lamp)
            .expect("a transform")
            .translation()
            .y,
        5.0,
        "the drop is measured in the world the ground stands in",
    );
    assert_eq!(
        app.world()
            .get::<Transform>(lamp)
            .expect("a transform")
            .translation
            .y,
        -7.0,
        "and written as the height its own parent spells",
    );
}

#[test]
fn dropping_a_selection_leaves_what_has_no_ground_under_it_where_it_stands() {
    let mut app = scene_with_ground();
    let over = floating_model(&mut app, Vec3::new(1.5, 40.0, 1.5));
    let beyond = floating_model(&mut app, Vec3::new(500.0, 40.0, 500.0));
    jackdaw::selection::select_many(app.world_mut(), &[over, beyond]);
    app.update();

    run_finished(&mut app, "entity.snap_to_ground");

    let height = |app: &App, entity: Entity| {
        app.world()
            .get::<Transform>(entity)
            .expect("a transform")
            .translation
            .y
    };
    assert_eq!(height(&app, over), 5.0);
    assert_eq!(height(&app, beyond), 40.0, "nothing to drop it onto");

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    app.update();

    assert_eq!(
        height(&app, over),
        40.0,
        "one entry took the whole drop back"
    );
}

#[test]
fn an_editor_entity_answers_to_no_id() {
    let mut app = scene_with_ground();
    let chrome = app
        .world_mut()
        .spawn((Name::new("Gizmo"), jackdaw::EditorEntity))
        .id();
    app.update();

    assert_eq!(
        run(
            &mut app,
            &format!("selection.select entity={}", chrome.to_bits())
        ),
        OperatorResult::Cancelled,
    );
    assert!(
        !app.world()
            .resource::<Selection>()
            .entities
            .contains(&chrome),
        "the editor's own is not the scene's to select",
    );
}

#[test]
fn a_list_holding_an_id_the_scene_no_longer_answers_to_is_refused_whole() {
    let (mut app, _tmp, _scene) = project_with_a_lamp();
    run_finished(&mut app, "scene.open path=assets/zone.bsn");
    let (first, second) = two_namesakes(&mut app);
    let gone = app.world_mut().spawn_empty().id();
    app.world_mut().despawn(gone);

    assert_eq!(
        run(
            &mut app,
            &format!(
                "entity.delete entities={},{}",
                first.to_bits(),
                gone.to_bits()
            )
        ),
        OperatorResult::Cancelled,
    );
    assert!(
        app.world().get_entity(first).is_ok() && app.world().get_entity(second).is_ok(),
        "a list the scene cannot answer whole moves nothing",
    );
}

#[test]
fn an_instance_whose_parent_is_not_in_the_document_stands_where_it_was_placed() {
    let (mut app, _tmp, _scene) = project_with_a_lamp();
    run_finished(&mut app, "scene.open path=assets/zone.bsn");
    app.world_mut().spawn((
        Name::new("Loose"),
        Transform::from_translation(Vec3::new(50.0, 9.0, 50.0)),
        Visibility::default(),
    ));
    app.update();

    run_finished(
        &mut app,
        "prefab.spawn_instance path=prefabs/lamp.bsn pos_x=1.0 pos_y=0.0 pos_z=2.0 parent=Loose",
    );

    let lamp = *entities_named(&mut app, "Lamp")
        .first()
        .expect("an instance");
    assert_eq!(
        app.world()
            .get::<Transform>(lamp)
            .expect("a transform")
            .translation,
        Vec3::new(1.0, 0.0, 2.0),
        "a top-level instance carries the world position it was placed at",
    );
}

/// The zone at the assets root and a scene in a folder beside it, so the two
/// spellings of one prefab can be compared.
fn source_written_from(app: &mut App, tmp: &tempfile::TempDir, scene: &str) -> String {
    let file = tmp.path().join("assets").join(scene);
    run_finished(app, &format!("scene.new kind=3d path={}", file.display()));
    run_finished(
        app,
        "prefab.spawn_instance path=prefabs/lamp.bsn pos_x=1.0 pos_y=0.0 pos_z=2.0",
    );
    let saved = run_op_clause_as_user(app.world_mut(), "scene.save").expect("the save dispatched");
    assert_eq!(saved, OperatorResult::Finished);
    app.update();

    let text = std::fs::read_to_string(&file).expect("the scene was written");
    text.lines()
        .find(|line| line.contains("source:"))
        .unwrap_or_else(|| panic!("the saved scene names a prefab source:\n{text}"))
        .trim()
        .to_string()
}

#[test]
fn a_scene_in_a_folder_spells_its_prefab_source_as_the_zone_beside_it_does() {
    let (mut app, tmp, _scene) = project_with_a_lamp();
    std::fs::create_dir_all(tmp.path().join("assets/scratch")).expect("the folder is made");

    let at_the_root = source_written_from(&mut app, &tmp, "zone_probe.bsn");
    let in_a_folder = source_written_from(&mut app, &tmp, "scratch/probe.bsn");

    assert_eq!(
        in_a_folder, at_the_root,
        "one prefab is one path, whatever folder names it",
    );
    assert!(
        at_the_root.contains("\"prefabs/lamp.bsn\""),
        "and that path is the one under the assets folder, got {at_the_root}",
    );
}

/// A scene authored the way the editor authors one: a group holding an
/// instance, saved, and then the prefab file taken away underneath it. The
/// app it comes back in is in the editor state, since that is what gates the
/// systems judging a freshly spawned entity.
fn a_group_holding_an_instance_whose_prefab_went_missing()
-> (App, tempfile::TempDir, std::path::PathBuf) {
    let (mut app, tmp, _scene) = project_with_a_lamp();
    let quarter = tmp.path().join("assets/quarter.bsn");
    run_finished(
        &mut app,
        &format!("scene.new kind=3d path={}", quarter.display()),
    );
    run_finished(&mut app, "entity.add.group name=Lamps");
    run_finished(
        &mut app,
        "prefab.spawn_instance path=prefabs/lamp.bsn pos_x=1.0 pos_y=0.0 pos_z=2.0 parent=Lamps",
    );
    let saved = run_op_clause_as_user(app.world_mut(), "scene.save").expect("the save dispatched");
    assert_eq!(saved, OperatorResult::Finished);
    app.update();
    drop(app);

    std::fs::remove_file(tmp.path().join("assets/prefabs/lamp.bsn"))
        .expect("the prefab leaves the project");

    let mut app = util::editor_test_app();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: tmp.path().to_path_buf(),
            config: default(),
        });
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    for _ in 0..4 {
        app.update();
    }
    (app, tmp, quarter)
}

/// The panel every outliner row is indexed under.
fn outliner_panel(app: &mut App) -> Entity {
    let panel = app
        .world_mut()
        .spawn((
            jackdaw::hierarchy::HierarchyTreeContainer,
            Node::default(),
            Visibility::Inherited,
        ))
        .id();
    app.update();
    panel
}

/// Whether a row offers a disclosure to open, which is the only way to reach
/// what it holds.
fn row_offers_an_expansion(app: &App, row: Entity) -> bool {
    let world = app.world();
    row_part::<jackdaw_widgets::tree_view::TreeRowContent>(world, row)
        .and_then(|content| {
            row_part::<jackdaw_widgets::tree_view::TreeNodeExpandToggle>(world, content)
        })
        .and_then(|toggle| world.get::<Children>(toggle))
        .is_some_and(|disclosures| !disclosures.is_empty())
}

/// The child of `parent` carrying `C`, which is how a row's parts are reached.
fn row_part<C: Component>(world: &World, parent: Entity) -> Option<Entity> {
    world
        .get::<Children>(parent)?
        .iter()
        .find(|&child| world.get::<C>(child).is_some())
}

/// What a tree row's label reads.
fn row_label_text(app: &App, row: Entity) -> String {
    let world = app.world();
    let content = row_part::<jackdaw_widgets::tree_view::TreeRowContent>(world, row)
        .expect("a row has content");
    let label = row_part::<jackdaw_widgets::tree_view::TreeRowLabel>(world, content)
        .expect("a row carries a label");
    world
        .get::<Text>(label)
        .expect("the label carries text")
        .0
        .clone()
}

/// Every node the scene tree reports, parents before children.
fn tree_nodes(node: &serde_json::Value, into: &mut Vec<serde_json::Value>) {
    into.push(node.clone());
    for child in node["children"].as_array().into_iter().flatten() {
        tree_nodes(child, into);
    }
}

#[test]
fn an_instance_whose_prefab_is_missing_keeps_its_row_and_its_reference() {
    let (mut app, tmp, quarter) = a_group_holding_an_instance_whose_prefab_went_missing();
    let panel = outliner_panel(&mut app);

    run_finished(&mut app, &format!("scene.open path={}", quarter.display()));
    for _ in 0..12 {
        app.update();
    }

    let instances = app
        .world()
        .resource::<jackdaw_bsn::SceneBsnAst>()
        .entities_with_component(ISA_TYPE);
    assert_eq!(instances.len(), 1, "the document still holds the reference");

    let mut query = app
        .world_mut()
        .query_filtered::<Entity, With<jackdaw_prefab::components::IsA>>();
    let instance = query
        .iter(app.world())
        .next()
        .expect("the instance stands in the scene rather than vanishing");
    assert!(
        app.world().get::<Name>(instance).is_none(),
        "it inherited nothing, so its row has to name the file instead",
    );
    assert!(
        app.world()
            .get::<jackdaw_scene_types::EditorHidden>(instance)
            .is_none(),
        "a node an author placed is not an internal to hide, whatever it inherited",
    );

    let group = app
        .world()
        .get::<ChildOf>(instance)
        .map(ChildOf::parent)
        .expect("the instance hangs under the group it was placed in");
    let group_row = app
        .world()
        .resource::<jackdaw_widgets::tree_view::TreeIndex>()
        .get(panel, group)
        .expect("the group has a row");
    assert!(
        row_offers_an_expansion(&app, group_row),
        "the group says it holds something, or the instance cannot be reached",
    );
    app.world_mut()
        .entity_mut(group_row)
        .insert(jackdaw_widgets::tree_view::TreeNodeExpanded(true));
    for _ in 0..4 {
        app.update();
    }

    let row = app
        .world()
        .resource::<jackdaw_widgets::tree_view::TreeIndex>()
        .get(panel, instance)
        .expect("the outliner draws a row for the instance it could not resolve");
    assert_eq!(
        row_label_text(&app, row),
        "lamp",
        "the row names the file it points at, since it inherited no name",
    );

    let tree = app
        .world_mut()
        .run_system_cached_with(
            jackdaw::remote::server::scene_tree_handler,
            Some(serde_json::json!({})),
        )
        .expect("the handler ran")
        .expect("the handler answered");
    let mut nodes = Vec::new();
    for root in tree["tree"].as_array().into_iter().flatten() {
        tree_nodes(root, &mut nodes);
    }
    assert!(
        nodes.iter().any(|node| {
            node["components"].as_array().is_some_and(|components| {
                components
                    .iter()
                    .any(|component| component == "jackdaw_prefab::components::IsA")
            })
        }),
        "the scene tree reports the instance too, got {nodes:#?}",
    );

    let outcome =
        run_op_clause_as_user(app.world_mut(), "scene.save").expect("the save dispatched");
    assert_eq!(outcome, OperatorResult::Finished);
    app.update();
    let saved = std::fs::read_to_string(tmp.path().join("assets/quarter.bsn")).expect("the file");
    assert!(
        saved.contains("prefabs/lamp.bsn"),
        "the save keeps the reference the author wrote:\n{saved}",
    );
}

#[test]
fn a_scene_naming_a_prefab_by_an_absolute_path_is_not_saved_over() {
    let (mut app, tmp, _scene) = project_with_a_lamp();
    run_finished(
        &mut app,
        &format!(
            "scene.new kind=3d path={}",
            tmp.path().join("assets/zone_absolute.bsn").display()
        ),
    );
    run_finished(
        &mut app,
        "prefab.spawn_instance path=prefabs/lamp.bsn pos_x=0.0 pos_y=0.0 pos_z=0.0",
    );

    let elsewhere = tmp.path().join("outside/lamp.bsn");
    let live = app.world_mut().resource_mut::<jackdaw_bsn::SceneBsnAst>();
    let node = *live
        .entities_with_component(ISA_TYPE)
        .first()
        .expect("an instance");
    let value = jackdaw_prefab::isa_value(&elsewhere.to_string_lossy(), &[]);
    jackdaw_prefab::set_whole_component(
        app.world_mut()
            .resource_mut::<jackdaw_bsn::SceneBsnAst>()
            .into_inner(),
        node,
        ISA_TYPE,
        value,
    );

    let refusal =
        jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), &tmp.path().join("assets"))
            .expect_err("a path only this machine holds is refused");

    assert!(
        refusal.contains("IsA.source"),
        "the refusal names the field that holds it, got {refusal}",
    );
}
