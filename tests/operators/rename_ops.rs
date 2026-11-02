//! `entity.rename`: the name a call leaves on the node, refusals that leave
//! the scene alone, one undo entry, a save round trip and the names it accepts.

use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::prelude::*;
use jackdaw::commands::CommandHistory;
use jackdaw::remote::server::call_operator_handler;
use jackdaw_api::prelude::*;
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

fn cube(app: &mut App, name: &str) -> Entity {
    let outcome = call(app, "entity.add.cube", json!({ "name": name }));
    let bits = outcome["entities"][0].as_u64().expect("an entity id");
    Entity::try_from_bits(bits).expect("a valid entity id")
}

fn rename(app: &mut App, entity: Entity, name: &str) -> Value {
    call(
        app,
        "entity.rename",
        json!({ "entity": entity.to_bits(), "name": name }),
    )
}

fn name_of(app: &App, entity: Entity) -> Option<String> {
    app.world()
        .get::<Name>(entity)
        .map(|name| name.as_str().to_string())
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
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    app.update();
}

fn redo(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.redo(world));
    app.update();
}

#[test]
fn a_rename_sets_exactly_the_name_asked_for() {
    let mut app = scene_app();
    let wall = cube(&mut app, "Wall");
    let other = cube(&mut app, "Roof");
    let outcome = rename(&mut app, wall, "North Wall");
    assert_eq!(outcome["result"], "finished", "{outcome}");
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    assert_eq!(name_of(&app, wall).as_deref(), Some("North Wall"));
    assert_eq!(name_of(&app, other).as_deref(), Some("Roof"));
}

#[test]
fn a_rename_is_found_by_its_new_name_and_not_its_old() {
    let mut app = scene_app();
    let wall = cube(&mut app, "Wall");
    rename(&mut app, wall, "Gate");
    let moved = call(
        &mut app,
        "entity.set_transform",
        json!({ "name": "Gate", "x": 4.0 }),
    );
    assert_eq!(moved["result"], "finished", "{moved}");
    let x = app
        .world()
        .get::<Transform>(wall)
        .expect("a transform")
        .translation
        .x;
    assert!((x - 4.0).abs() < 1e-4);
}

#[test]
fn invalid_parameters_are_refused_and_leave_the_scene_byte_identical() {
    let mut app = scene_app();
    let wall = cube(&mut app, "Wall");
    let before = scene_text(&mut app);
    let entries = history_len(&app);
    let gone = {
        let spare = app.world_mut().spawn_empty().id();
        app.world_mut().despawn(spare);
        spare
    };
    for params in [
        json!({ "entity": wall.to_bits() }),
        json!({ "name": "Gate" }),
        json!({ "entity": gone.to_bits(), "name": "Gate" }),
        json!({ "entity": "NoSuchNode", "name": "Gate" }),
    ] {
        let outcome = call(&mut app, "entity.rename", params.clone());
        assert_eq!(outcome["outcome"], "invalid", "{params}: {outcome}");
        assert_eq!(outcome["result"], "cancelled", "{params}: {outcome}");
        assert!(
            !outcome["warnings"].as_array().expect("warnings").is_empty(),
            "{params}: the refusal says why: {outcome}"
        );
        assert_eq!(history_len(&app), entries, "{params}");
        assert_eq!(scene_text(&mut app), before, "{params}");
    }
    assert_eq!(name_of(&app, wall).as_deref(), Some("Wall"));
}

#[test]
fn an_entity_outside_the_scene_is_refused() {
    let mut app = scene_app();
    let chrome = app.world_mut().spawn(Name::new("Chrome")).id();
    let outcome = rename(&mut app, chrome, "Gate");
    assert_eq!(outcome["outcome"], "invalid", "{outcome}");
    assert_eq!(name_of(&app, chrome).as_deref(), Some("Chrome"));
}

#[test]
fn one_undo_reverts_a_rename_and_redo_reapplies_it() {
    let mut app = scene_app();
    let wall = cube(&mut app, "Wall");
    let entries = history_len(&app);
    rename(&mut app, wall, "Gate");
    assert_eq!(history_len(&app), entries + 1, "one undo entry");

    undo(&mut app);
    assert_eq!(name_of(&app, wall).as_deref(), Some("Wall"));
    redo(&mut app);
    assert_eq!(name_of(&app, wall).as_deref(), Some("Gate"));
}

#[test]
fn renaming_to_the_current_name_is_unchanged_and_leaves_no_undo_entry() {
    let mut app = scene_app();
    let wall = cube(&mut app, "Wall");
    let before = scene_text(&mut app);
    let entries = history_len(&app);
    let outcome = rename(&mut app, wall, "Wall");
    assert_eq!(outcome["result"], "finished", "{outcome}");
    assert_eq!(outcome["outcome"], "unchanged", "{outcome}");
    assert_eq!(history_len(&app), entries);
    assert_eq!(scene_text(&mut app), before);
}

#[test]
fn unicode_and_duplicate_names_are_accepted() {
    let mut app = scene_app();
    let first = cube(&mut app, "Wall");
    let second = cube(&mut app, "Roof");
    let outcome = rename(
        &mut app,
        first,
        "Mur \u{00e9}t\u{00e9} \u{5c4b}\u{6839} \"q\" \\ x",
    );
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    assert_eq!(
        name_of(&app, first).as_deref(),
        Some("Mur \u{00e9}t\u{00e9} \u{5c4b}\u{6839} \"q\" \\ x")
    );

    rename(&mut app, first, "Same");
    let outcome = rename(&mut app, second, "Same");
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    assert_eq!(name_of(&app, first).as_deref(), Some("Same"));
    assert_eq!(name_of(&app, second).as_deref(), Some("Same"));
}

#[test]
fn an_empty_name_clears_the_name_and_undo_brings_it_back() {
    let mut app = scene_app();
    let wall = cube(&mut app, "Wall");
    let entries = history_len(&app);
    let outcome = rename(&mut app, wall, "");
    assert_eq!(outcome["outcome"], "changed", "{outcome}");
    assert!(name_of(&app, wall).is_none(), "the Name is gone");
    assert_eq!(history_len(&app), entries + 1);

    undo(&mut app);
    assert_eq!(name_of(&app, wall).as_deref(), Some("Wall"));

    rename(&mut app, wall, "");
    let again = rename(&mut app, wall, "");
    assert_eq!(again["outcome"], "unchanged", "{again}");
}

#[test]
fn a_saved_and_reopened_rename_persists() {
    let mut app = scene_app();
    let wall = cube(&mut app, "Wall");
    rename(&mut app, wall, "Gate \u{5c4b}");

    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("renamed.bsn");
    app.world_mut()
        .operator("scene.save")
        .param("path", path.to_string_lossy().to_string())
        .call()
        .expect("scene.save dispatches")
        .assert_finished();
    app.update();
    assert!(path.exists(), "the scene was written");

    let mut app = util::editor_test_app();
    jackdaw::migrate_dialog::request_open_with_conversion(app.world_mut(), &path);
    let named = |app: &mut App| -> Vec<String> {
        app.world_mut()
            .query::<&Name>()
            .iter(app.world())
            .map(|name| name.as_str().to_string())
            .collect()
    };
    util::update_until(
        &mut app,
        "the renamed node to load",
        std::time::Duration::from_secs(45),
        |app| named(app).iter().any(|name| name == "Gate \u{5c4b}"),
    );
    let names = named(&mut app);
    assert!(!names.iter().any(|name| name == "Wall"), "{names:?}");
}

#[test]
fn an_inline_hierarchy_rename_commits_as_one_undo_step() {
    let mut app = scene_app();
    let wall = cube(&mut app, "Wall");
    let entries = history_len(&app);
    let label = app.world_mut().spawn_empty().id();
    app.world_mut()
        .trigger(jackdaw_widgets::tree_view::TreeRowRenamed {
            entity: label,
            source_entity: wall,
            new_name: "Gate".to_string(),
        });
    app.update();
    assert_eq!(name_of(&app, wall).as_deref(), Some("Gate"));
    assert_eq!(history_len(&app), entries + 1);
    undo(&mut app);
    assert_eq!(name_of(&app, wall).as_deref(), Some("Wall"));
}

#[test]
fn a_refused_inline_rename_shows_the_old_name_again() {
    let mut app = scene_app();
    let chrome = app.world_mut().spawn(Name::new("Chrome")).id();
    let label = app.world_mut().spawn(Text::new("typed")).id();
    app.world_mut()
        .trigger(jackdaw_widgets::tree_view::TreeRowRenamed {
            entity: label,
            source_entity: chrome,
            new_name: "typed".to_string(),
        });
    app.update();
    assert_eq!(name_of(&app, chrome).as_deref(), Some("Chrome"));
    let text = app.world().get::<Text>(label).expect("a label");
    assert_eq!(text.0, "Chrome");
}

#[test]
fn undoing_the_rename_of_a_node_with_no_authored_name_restores_what_it_showed() {
    let mut app = scene_app();
    let node = app.world_mut().spawn_empty().id();
    jackdaw_bsn::create_entity_in_ast(app.world_mut(), node, None);
    app.world_mut()
        .entity_mut(node)
        .insert(Name::new("Inherited"));
    let authored = |app: &App| {
        let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
        ast.ast_for(node)
            .and_then(|doc| ast.get_name(doc))
            .map(str::to_string)
    };
    assert_eq!(authored(&app), None);

    rename(&mut app, node, "Gate");
    assert_eq!(authored(&app).as_deref(), Some("Gate"));
    undo(&mut app);
    assert_eq!(authored(&app), None, "the patch is removed");
    assert_eq!(name_of(&app, node).as_deref(), Some("Inherited"));
}
