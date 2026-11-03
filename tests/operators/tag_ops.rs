//! Tags and entity references: `entity.tag`, the reference grammar every
//! entity parameter takes (an id, a name path, a query), the refusals that
//! leave the scene alone, and tags surviving save, duplicate and prefab spawn.

use crate::util;
use crate::util::OperatorResultExt as _;

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw::commands::CommandHistory;
use jackdaw::entity_refs::{RefError, resolve};
use jackdaw::prefab::IsA;
use jackdaw::project::{ProjectConfig, ProjectRoot};
use jackdaw::remote::server::{
    batch_handler, call_operator_handler, scene_summary_handler, scene_tree_handler,
};
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_scene_types::{Brush, PropertyValue, Tags};
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

/// A named node under `parent`, registered in the document. Names repeat
/// freely here, as they do in an imported model.
fn node(app: &mut App, name: &str, parent: Option<Entity>) -> Entity {
    let mut spawned = app.world_mut().spawn((
        Name::new(name.to_string()),
        Transform::default(),
        Visibility::default(),
    ));
    if let Some(parent) = parent {
        spawned.insert(ChildOf(parent));
    }
    let entity = spawned.id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    app.update();
    entity
}

/// A cube brush node, so a material has faces to land on.
fn brush(app: &mut App, name: &str, parent: Option<Entity>) -> Entity {
    let entity = node(app, name, parent);
    app.world_mut()
        .entity_mut(entity)
        .insert(Brush::cuboid(0.5, 0.5, 0.5));
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    app.update();
    entity
}

/// Two houses in a village, each with a roof and a wall, and a shed outside it.
struct TagVillage {
    inn: Entity,
    inn_roof: Entity,
    inn_wall: Entity,
    mill_roof: Entity,
    mill_wall: Entity,
    shed_roof: Entity,
}

fn village(app: &mut App, make: fn(&mut App, &str, Option<Entity>) -> Entity) -> TagVillage {
    let village = node(app, "village", None);
    let inn = node(app, "inn", Some(village));
    let inn_roof = make(app, "roof", Some(inn));
    let inn_wall = make(app, "wall", Some(inn));
    let mill = node(app, "mill", Some(village));
    let mill_roof = make(app, "roof", Some(mill));
    let mill_wall = make(app, "wall", Some(mill));
    let shed = node(app, "shed", None);
    let shed_roof = make(app, "roof", Some(shed));
    TagVillage {
        inn,
        inn_roof,
        inn_wall,
        mill_roof,
        mill_wall,
        shed_roof,
    }
}

fn answer<M>(
    app: &mut App,
    handler: impl bevy::ecs::system::IntoSystem<In<Option<Value>>, bevy::remote::BrpResult, M> + 'static,
    params: Value,
) -> bevy::remote::BrpResult {
    let result = app
        .world_mut()
        .run_system_cached_with(handler, Some(params))
        .expect("the handler ran");
    app.update();
    result
}

#[track_caller]
fn call(app: &mut App, id: &str, params: Value) -> Value {
    answer(
        app,
        call_operator_handler,
        json!({ "id": id, "params": params }),
    )
    .unwrap_or_else(|err| panic!("{id} refused: {}", err.message))
}

fn tags(app: &App, entity: Entity) -> Vec<String> {
    app.world()
        .get::<Tags>(entity)
        .map(|tags| tags.0.clone())
        .unwrap_or_default()
}

fn history_len(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

fn undo(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    app.update();
}

fn redo(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.redo(world));
    app.update();
}

fn ids(entities: &[Entity]) -> Vec<u64> {
    entities.iter().map(|entity| entity.to_bits()).collect()
}

#[track_caller]
fn matches(app: &mut App, text: &str) -> Vec<Entity> {
    resolve(app.world_mut(), text).unwrap_or_else(|err| panic!("{}", err.describe(text)))
}

/// `matches`, sorted, for a set spread over several top-level nodes whose
/// order is not the point.
#[track_caller]
fn matched_set(app: &mut App, text: &str) -> Vec<Entity> {
    let mut found = matches(app, text);
    found.sort();
    found
}

fn sorted(mut entities: Vec<Entity>) -> Vec<Entity> {
    entities.sort();
    entities
}

fn tag(app: &mut App, entities: &[Entity], add: &str) {
    let outcome = call(
        app,
        "entity.tag",
        json!({ "entities": ids(entities), "add": add }),
    );
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
}

#[test]
fn name_paths_and_wildcards_resolve_to_the_nodes_they_spell() {
    let mut app = scene_app();
    let v = village(&mut app, node);

    assert_eq!(matches(&mut app, "village/inn/roof"), vec![v.inn_roof]);
    assert_eq!(
        matches(&mut app, "village/*/roof"),
        vec![v.inn_roof, v.mill_roof],
        "`*` stands for one level, in outliner order"
    );
    assert_eq!(matches(&mut app, "*/*/wall"), vec![v.inn_wall, v.mill_wall]);
    assert_eq!(
        matched_set(&mut app, "roof"),
        sorted(vec![v.inn_roof, v.mill_roof, v.shed_roof]),
        "a bare name matches anywhere"
    );
    assert_eq!(matches(&mut app, "in*"), vec![v.inn]);
    assert_eq!(
        matches(&mut app, &v.inn.to_bits().to_string()),
        vec![v.inn],
        "digits are an id"
    );
    assert_eq!(
        resolve(app.world_mut(), "inn/roof"),
        Err(RefError::NoMatch),
        "a path starts at the top of the scene"
    );
    assert_eq!(
        resolve(app.world_mut(), "village/Inn/roof"),
        Err(RefError::NoMatch),
        "names match with their case"
    );
}

#[test]
fn tag_queries_resolve_to_the_tagged_set_and_narrow_with_terms() {
    let mut app = scene_app();
    let v = village(&mut app, node);
    tag(&mut app, &[v.inn_roof, v.mill_roof, v.shed_roof], "roof");
    tag(&mut app, &[v.mill_roof], "mossy");

    assert_eq!(
        matched_set(&mut app, "#roof"),
        sorted(vec![v.inn_roof, v.mill_roof, v.shed_roof])
    );
    assert_eq!(
        matches(&mut app, "#roof & under:village"),
        vec![v.inn_roof, v.mill_roof]
    );
    assert_eq!(
        matches(&mut app, "#roof&under:village/inn"),
        vec![v.inn_roof]
    );
    assert_eq!(
        matched_set(&mut app, "#roof&!#mossy"),
        sorted(vec![v.inn_roof, v.shed_roof])
    );
    assert_eq!(
        matches(&mut app, "under:village & wall"),
        vec![v.inn_wall, v.mill_wall]
    );
    assert_eq!(resolve(app.world_mut(), "#Roof"), Err(RefError::NoMatch));
}

#[test]
fn entity_tag_adds_and_removes_as_one_undo_step() {
    let mut app = scene_app();
    let v = village(&mut app, node);
    let before = history_len(&app);

    let outcome = call(
        &mut app,
        "entity.tag",
        json!({ "entities": "village/*/roof", "add": ["roof", "old"] }),
    );
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    assert_eq!(tags(&app, v.inn_roof), vec!["old", "roof"]);
    assert_eq!(tags(&app, v.mill_roof), vec!["old", "roof"]);
    assert!(tags(&app, v.shed_roof).is_empty());
    assert_eq!(
        history_len(&app),
        before + 1,
        "one undo step for both roofs"
    );

    let outcome = call(
        &mut app,
        "entity.tag",
        json!({ "entities": "#old", "remove": "old" }),
    );
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    assert_eq!(tags(&app, v.inn_roof), vec!["roof"]);

    let again = call(
        &mut app,
        "entity.tag",
        json!({ "entities": "#roof", "add": "roof" }),
    );
    assert_eq!(again["outcome"], "unchanged", "{again}");

    undo(&mut app);
    assert_eq!(tags(&app, v.mill_roof), vec!["old", "roof"]);
    undo(&mut app);
    assert!(tags(&app, v.inn_roof).is_empty());
    assert!(app.world().get::<Tags>(v.inn_roof).is_none());
    redo(&mut app);
    assert_eq!(tags(&app, v.inn_roof), vec!["old", "roof"]);
}

#[test]
fn a_bad_tag_or_no_tags_at_all_is_refused() {
    let mut app = scene_app();
    let v = village(&mut app, node);
    let before = history_len(&app);
    for params in [
        json!({ "entities": [v.inn.to_bits()], "add": "two words" }),
        json!({ "entities": [v.inn.to_bits()], "add": "#roof" }),
        json!({ "entities": [v.inn.to_bits()] }),
        json!({ "entities": [v.inn.to_bits()], "add": "a", "remove": "a" }),
    ] {
        let outcome = call(&mut app, "entity.tag", params.clone());
        assert_eq!(outcome["outcome"], "invalid", "{params}: {outcome}");
    }
    assert!(tags(&app, v.inn).is_empty());
    assert_eq!(history_len(&app), before);
}

#[test]
fn a_reference_that_matches_nothing_is_invalid_and_changes_nothing() {
    let mut app = scene_app();
    let v = village(&mut app, node);
    let before = history_len(&app);

    for (id, params) in [
        ("entity.tag", json!({ "entities": "#nothing", "add": "x" })),
        (
            "entity.tag",
            json!({ "entities": ["village/inn", "village/castle"], "add": "x" }),
        ),
        ("entity.delete", json!({ "entities": "#nothing" })),
        (
            "entity.set_transform",
            json!({ "entity": "village/castle", "x": 4.0 }),
        ),
        (
            "entity.add.cube",
            json!({ "parent": "#nothing", "name": "Box" }),
        ),
        (
            "entity.set_transform",
            json!({ "entity": "village//inn", "x": 4.0 }),
        ),
    ] {
        let outcome = call(&mut app, id, params.clone());
        assert_eq!(outcome["outcome"], "invalid", "{id} {params}: {outcome}");
        assert_eq!(outcome["result"], "cancelled", "{id} {params}: {outcome}");
        assert!(
            outcome["warnings"]
                .as_array()
                .is_some_and(|w| !w.is_empty()),
            "a refusal says why: {outcome}"
        );
    }
    assert!(tags(&app, v.inn).is_empty());
    assert_eq!(
        app.world().get::<Transform>(v.inn).map(|t| t.translation),
        Some(Vec3::ZERO)
    );
    let boxes = app
        .world_mut()
        .query::<&Name>()
        .iter(app.world())
        .filter(|name| name.as_str() == "Box")
        .count();
    assert_eq!(boxes, 0, "no cube was added without its parent");
    assert_eq!(history_len(&app), before, "nothing reached the history");
}

#[test]
fn a_single_entity_param_matching_several_is_refused_with_the_count() {
    let mut app = scene_app();
    let v = village(&mut app, node);
    let before = history_len(&app);

    let outcome = call(
        &mut app,
        "entity.set_transform",
        json!({ "entity": "roof", "x": 9.0 }),
    );
    assert_eq!(outcome["outcome"], "invalid", "{outcome}");
    let warning = outcome["warnings"][0].as_str().unwrap_or_default();
    assert!(warning.contains("matches 3 entities"), "{warning}");
    for roof in [v.inn_roof, v.mill_roof, v.shed_roof] {
        assert_eq!(
            app.world().get::<Transform>(roof).map(|t| t.translation.x),
            Some(0.0)
        );
    }
    assert_eq!(history_len(&app), before);

    let outcome = call(
        &mut app,
        "entity.set_transform",
        json!({ "entity": "village/inn/roof", "x": 9.0 }),
    );
    assert_eq!(outcome["result"], "finished", "{outcome}");
    assert_eq!(
        app.world()
            .get::<Transform>(v.inn_roof)
            .map(|t| t.translation.x),
        Some(9.0)
    );
}

#[test]
fn a_clause_takes_the_same_grammar() {
    let mut app = scene_app();
    let v = village(&mut app, node);

    let result = jackdaw::boot_ops::run_op_clause(
        app.world_mut(),
        "entity.tag entities=village/*/roof,shed/roof add=roof",
    )
    .expect("dispatches");
    app.update();
    assert_eq!(result, OperatorResult::Finished);
    assert_eq!(matches(&mut app, "#roof").len(), 3);

    let result = jackdaw::boot_ops::run_op_clause(
        app.world_mut(),
        "entity.set_transform entity=#roof&under:village/inn y=3",
    )
    .expect("dispatches");
    app.update();
    assert_eq!(result, OperatorResult::Finished);
    assert_eq!(
        app.world()
            .get::<Transform>(v.inn_roof)
            .map(|t| t.translation.y),
        Some(3.0)
    );

    let result =
        jackdaw::boot_ops::run_op_clause(app.world_mut(), "entity.set_transform entity=#roof y=5")
            .expect("dispatches");
    assert_eq!(result, OperatorResult::Cancelled, "three roofs, one entity");
}

#[test]
fn a_batch_resolves_each_call_against_what_the_calls_before_it_left() {
    let mut app = scene_app();
    let v = village(&mut app, node);
    let outcome = answer(
        &mut app,
        batch_handler,
        json!({ "calls": [
            { "id": "entity.tag", "params": { "entities": "village/inn/wall", "add": "doomed" } },
            { "id": "entity.delete", "params": { "entities": "#doomed" } },
        ] }),
    )
    .unwrap_or_else(|err| panic!("the batch refused: {}", err.message));
    assert!(outcome.get("error").is_none(), "{outcome}");
    assert!(app.world().get_entity(v.inn_wall).is_err(), "{outcome}");
    assert!(app.world().get_entity(v.mill_wall).is_ok());
}

#[test]
fn scene_tree_and_summary_filter_by_the_same_grammar() {
    let mut app = scene_app();
    let v = village(&mut app, node);
    tag(&mut app, &[v.inn_roof, v.mill_roof, v.shed_roof], "roof");

    let tree = answer(&mut app, scene_tree_handler, json!({ "filter": "#roof" }))
        .expect("the tree answers");
    let mut roots: Vec<u64> = tree["tree"]
        .as_array()
        .expect("a tree")
        .iter()
        .filter_map(|node| node["entity"].as_u64())
        .collect();
    roots.sort();
    let mut wanted = ids(&[v.inn_roof, v.mill_roof, v.shed_roof]);
    wanted.sort();
    assert_eq!(roots, wanted);

    let tree = answer(
        &mut app,
        scene_tree_handler,
        json!({ "filter": "#roof", "root": "village" }),
    )
    .expect("the tree answers");
    assert_eq!(tree["tree"].as_array().map(Vec::len), Some(2), "{tree}");

    let tree = answer(
        &mut app,
        scene_tree_handler,
        json!({ "filter": "village/*" }),
    )
    .expect("the tree answers");
    assert_eq!(tree["tree"].as_array().map(Vec::len), Some(2), "{tree}");

    let summary = answer(
        &mut app,
        scene_summary_handler,
        json!({ "filter": "#roof&under:village", "fields": ["name"] }),
    )
    .expect("the summary answers");
    let rows: Vec<u64> = summary["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .filter_map(|row| row[0].as_u64())
        .collect();
    assert_eq!(rows, ids(&[v.inn_roof, v.mill_roof]));

    let summary = answer(
        &mut app,
        scene_summary_handler,
        json!({ "filter": { "ref": "#nothing" } }),
    )
    .expect("a filter that matches nothing reads as no rows");
    assert_eq!(summary["rows"], json!([]));

    assert!(
        answer(&mut app, scene_tree_handler, json!({ "filter": "a//b" })).is_err(),
        "a malformed filter is refused"
    );
}

#[test]
fn tags_survive_save_and_reopen() {
    let mut app = scene_app();
    let roof = node(&mut app, "roof", None);
    tag(&mut app, &[roof], "roof,red");

    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("tagged.bsn");
    app.world_mut()
        .operator("scene.save")
        .param("path", path.to_string_lossy().to_string())
        .call()
        .expect("scene.save dispatches")
        .assert_finished();
    app.update();
    let text = std::fs::read_to_string(&path).expect("the scene was written");
    assert!(text.contains("Tags"), "{text}");

    let mut app = util::editor_test_app();
    jackdaw::migrate_dialog::request_open_with_conversion(app.world_mut(), &path);
    util::update_until(
        &mut app,
        "the tagged node to load",
        std::time::Duration::from_secs(45),
        |app| {
            app.world_mut()
                .query::<&Tags>()
                .iter(app.world())
                .next()
                .is_some()
        },
    );
    let found = matches(&mut app, "#red");
    assert_eq!(found.len(), 1);
    assert_eq!(tags(&app, found[0]), vec!["red", "roof"]);
}

#[test]
fn a_duplicate_keeps_its_tags() {
    let mut app = scene_app();
    let v = village(&mut app, node);
    tag(&mut app, &[v.inn_roof], "roof");

    let outcome = call(
        &mut app,
        "entity.duplicate",
        json!({ "entities": "village/inn", "count": 2 }),
    );
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    let roofs = matches(&mut app, "#roof");
    assert_eq!(roofs.len(), 3, "the copies' roofs carry the tag: {roofs:?}");
    assert!(roofs.contains(&v.inn_roof));
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/definition_project")
}

fn invoke(app: &mut App, id: &'static str, params: &[(&'static str, PropertyValue)]) {
    let mut call = app.world_mut().operator(id).settings(CallOperatorSettings {
        execution_context: ExecutionContext::Invoke,
        creates_history_entry: true,
    });
    for (key, value) in params {
        call = call.param(*key, value.clone());
    }
    let result = call
        .call()
        .unwrap_or_else(|err| panic!("{id}: dispatch errored: {err}"));
    assert_eq!(result, OperatorResult::Finished, "{id} reported {result:?}");
    for _ in 0..6 {
        app.update();
    }
}

#[test]
fn a_prefab_spawn_carries_the_tags_its_source_was_packed_with() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = scene_app();
    app.world_mut().insert_resource(ProjectRoot::new(
        dir.path().to_path_buf(),
        ProjectConfig::default(),
    ));
    let house = node(&mut app, "house", None);
    let roof = node(&mut app, "roof", Some(house));
    tag(&mut app, &[house], "house");
    tag(&mut app, &[roof], "roof");
    app.world_mut().resource_mut::<Selection>().entities = vec![house];

    invoke(
        &mut app,
        "prefab.pack",
        &[("path", PropertyValue::from("prefabs/house.bsn"))],
    );
    let written = dir.path().join("assets/prefabs/house.bsn");
    let text = std::fs::read_to_string(&written).expect("the prefab was written");
    assert!(text.contains("Tags"), "{text}");

    invoke(
        &mut app,
        "prefab.spawn_instance",
        &[
            ("path", PropertyValue::from("prefabs/house.bsn")),
            ("pos_x", PropertyValue::Float(8.0)),
            ("pos_y", PropertyValue::Float(0.0)),
            ("pos_z", PropertyValue::Float(0.0)),
        ],
    );
    let instances: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<IsA>>()
        .iter(app.world())
        .collect();
    assert_eq!(instances.len(), 2, "the packed one and the spawned one");
    assert_eq!(matches(&mut app, "#house").len(), 2);
    assert_eq!(matches(&mut app, "#roof").len(), 2);
}

/// The whole point: every roof turns one colour from one call naming `#roof`,
/// with no read of the scene first.
#[test]
fn one_material_call_with_a_tag_dresses_every_roof() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::copy(
        fixture_dir().join("jackdaw.toml"),
        tmp.path().join("jackdaw.toml"),
    )
    .expect("the manifest copies");
    std::fs::create_dir_all(tmp.path().join("assets/materials")).expect("a materials folder");
    let mut app = util::editor_test_app();
    app.world_mut().insert_resource(ProjectRoot {
        root: tmp.path().to_path_buf(),
        config: default(),
    });
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    invoke(
        &mut app,
        "asset.new",
        &[
            ("type", "material".into()),
            ("name", "clay".into()),
            ("path", "materials".into()),
        ],
    );
    let v = village(&mut app, brush);
    tag(&mut app, &[v.inn_roof, v.mill_roof, v.shed_roof], "roof");
    app.world_mut().resource_mut::<Selection>().entities = vec![v.inn_wall];
    app.update();
    let before = history_len(&app);

    let outcome = call(
        &mut app,
        "material.apply",
        json!({ "material": "materials/clay.bsn", "entities": "#roof" }),
    );
    assert_eq!(outcome["result"], "finished", "{outcome}");
    for _ in 0..4 {
        app.update();
    }

    let clay = app
        .world()
        .resource::<jackdaw::asset_index::AssetIndex>()
        .get(Path::new("materials/clay.bsn"))
        .and_then(|entry| entry.value.handle().cloned())
        .expect("the project holds the material");
    let wears_clay = |app: &App, entity: Entity| {
        app.world().get::<Brush>(entity).is_some_and(|brush| {
            brush
                .faces
                .iter()
                .all(|face| face.material.id().untyped() == clay.id())
        })
    };
    for roof in [v.inn_roof, v.mill_roof, v.shed_roof] {
        assert!(wears_clay(&app, roof), "every roof wears the material");
    }
    for wall in [v.inn_wall, v.mill_wall] {
        assert!(
            !wears_clay(&app, wall),
            "walls, selected or not, are left alone"
        );
    }
    assert_eq!(history_len(&app), before + 1, "one undo step");
    undo(&mut app);
    assert!(!wears_clay(&app, v.mill_roof), "undo takes every roof back");
}

#[test]
fn a_star_deletes_scene_nodes_and_never_the_editors_own() {
    let mut app = util::editor_test_app();
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    app.world_mut()
        .operator("scene.new")
        .call()
        .expect("scene.new dispatches")
        .assert_finished();
    for _ in 0..4 {
        app.update();
    }
    let v = village(&mut app, node);
    let furniture: Vec<(Entity, String)> = app
        .world_mut()
        .query::<(Entity, &Name)>()
        .iter(app.world())
        .map(|(entity, name)| (entity, name.as_str().to_string()))
        .collect::<Vec<_>>()
        .into_iter()
        .filter(|(entity, _)| {
            let world = app.world();
            let inside_scene = std::iter::successors(Some(*entity), |at| {
                world.get::<ChildOf>(*at).map(ChildOf::parent)
            })
            .any(|at| jackdaw::entity_refs::is_scene_member(world, at));
            !inside_scene
        })
        .collect();
    assert!(
        !furniture.is_empty(),
        "the editor has named entities of its own for `*` to pass over"
    );

    let starred = matches(&mut app, "*");
    for (entity, name) in &furniture {
        assert!(
            !starred.contains(entity),
            "`*` matched editor entity {entity} ({name})"
        );
    }

    let outcome = call(&mut app, "entity.delete", json!({ "entities": "*" }));
    assert_ne!(outcome["outcome"], "invalid", "{outcome}");
    for gone in [v.inn, v.inn_roof, v.shed_roof] {
        assert!(app.world().get_entity(gone).is_err(), "{gone} was deleted");
    }
    let chrome = app
        .world_mut()
        .query_filtered::<(), With<jackdaw::layout::HierarchyFilter>>()
        .iter(app.world())
        .count();
    assert_eq!(chrome, 1, "the outliner's own filter field is still there");
}

#[test]
fn a_filtered_tree_prunes_only_against_matches_inside_the_root() {
    let mut app = scene_app();
    let village = node(&mut app, "village", None);
    let inn = node(&mut app, "inn", Some(village));
    let door = node(&mut app, "door", Some(inn));
    tag(&mut app, &[village, door], "x");

    let tree = answer(
        &mut app,
        scene_tree_handler,
        json!({ "filter": "#x", "root": "village/inn" }),
    )
    .expect("the tree answers");
    let roots: Vec<u64> = tree["tree"]
        .as_array()
        .expect("a tree")
        .iter()
        .filter_map(|node| node["entity"].as_u64())
        .collect();
    assert_eq!(
        roots,
        ids(&[door]),
        "village sits outside the root, so it hides nothing: {tree}"
    );
}

#[test]
fn a_name_that_reads_like_a_reference_still_names_its_node() {
    let mut app = scene_app();
    let slashed = node(&mut app, "Wall/Left", None);
    let ampersand = node(&mut app, "A&B", None);
    let digits = node(&mut app, "2", None);
    for (name, entity) in [("Wall/Left", slashed), ("A&B", ampersand), ("2", digits)] {
        assert_eq!(matches(&mut app, name), vec![entity], "{name}");
        let outcome = call(
            &mut app,
            "entity.set_transform",
            json!({ "entity": name, "x": 2.0 }),
        );
        assert_eq!(outcome["result"], "finished", "{name}: {outcome}");
        assert_eq!(
            app.world()
                .get::<Transform>(entity)
                .map(|t| t.translation.x),
            Some(2.0),
            "{name}"
        );
    }
    let tree = answer(&mut app, scene_tree_handler, json!({ "root": "Wall/Left" }))
        .expect("the tree answers");
    assert_eq!(tree["tree"][0]["entity"], json!(slashed.to_bits()));
}

#[test]
fn selection_ops_take_the_grammar() {
    let mut app = scene_app();
    let v = village(&mut app, node);
    tag(&mut app, &[v.inn_roof, v.mill_roof], "roof");

    let outcome = call(
        &mut app,
        "selection.select",
        json!({ "name": "village/inn" }),
    );
    assert_eq!(outcome["result"], "finished", "{outcome}");
    assert_eq!(app.world().resource::<Selection>().entities, vec![v.inn]);

    let outcome = call(&mut app, "selection.extend", json!({ "name": "#roof" }));
    assert_eq!(outcome["result"], "finished", "{outcome}");
    let selected = app.world().resource::<Selection>().entities.clone();
    for wanted in [v.inn, v.inn_roof, v.mill_roof] {
        assert!(selected.contains(&wanted), "{selected:?}");
    }

    let outcome = call(&mut app, "selection.select", json!({ "name": "roof" }));
    assert_eq!(outcome["result"], "cancelled", "three roofs: {outcome}");
}

#[test]
fn taking_an_inherited_tag_off_an_instance_stays_off_after_a_reopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = scene_app();
    app.world_mut().insert_resource(ProjectRoot::new(
        dir.path().to_path_buf(),
        ProjectConfig::default(),
    ));
    let house = node(&mut app, "house", None);
    tag(&mut app, &[house], "house");
    app.world_mut().resource_mut::<Selection>().entities = vec![house];
    invoke(
        &mut app,
        "prefab.pack",
        &[("path", PropertyValue::from("prefabs/house.bsn"))],
    );
    let instance = matches(&mut app, "#house");
    assert_eq!(instance.len(), 1, "the packed house carries its tag");
    let instance = instance[0];
    assert!(
        app.world()
            .get::<jackdaw::prefab::PrefabEntityId>(instance)
            .is_some(),
        "the tagged node is now provided by the prefab"
    );

    let outcome = call(
        &mut app,
        "entity.tag",
        json!({ "entities": [instance.to_bits()], "remove": "house" }),
    );
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    assert!(tags(&app, instance).is_empty());

    let path = dir.path().join("assets/village.bsn");
    app.world_mut()
        .operator("scene.save")
        .param("path", path.to_string_lossy().to_string())
        .call()
        .expect("scene.save dispatches")
        .assert_finished();
    app.update();

    let mut app = util::editor_test_app();
    app.world_mut().insert_resource(ProjectRoot::new(
        dir.path().to_path_buf(),
        ProjectConfig::default(),
    ));
    jackdaw::migrate_dialog::request_open_with_conversion(app.world_mut(), &path);
    util::update_until(
        &mut app,
        "the instance to load",
        std::time::Duration::from_secs(45),
        |app| {
            app.world_mut()
                .query_filtered::<(), With<IsA>>()
                .iter(app.world())
                .next()
                .is_some()
        },
    );
    for _ in 0..6 {
        app.update();
    }
    assert_eq!(
        resolve(app.world_mut(), "#house"),
        Err(RefError::NoMatch),
        "the source's tag came back on reload"
    );
}
