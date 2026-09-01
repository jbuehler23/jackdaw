//! A whole authoring session driven the way a script drives it: no
//! pointer, no dialog, one `JACKDAW_RUN_OP` clause at a time.
//!
//! What is pinned here:
//!  * `scene.new ui=true` leaves a UI root the next clause can build on.
//!  * `scene.open path=` opens a file without asking for one.
//!  * An `Entity` parameter resolves from `name=`, and from the selection
//!    for the operators that act on it, and refuses, out loud, for the
//!    ones that do not.
//!  * `field.set` goes through the real field-edit gesture, so the
//!    document is authored, exactly one undo entry appears, and undo puts
//!    the value back.
//!  * `widget.add`, `binding.add`/`binding.set` and `entity.reparent`
//!    reach the widget palette, the bindings card's commit funnel and the
//!    reparent command, and a whole bound scene authored from clauses
//!    loads like the same scene authored by hand.

use crate::util;

use bevy::input_focus::tab_navigation::TabGroup;
use bevy::prelude::*;
use jackdaw::boot_ops::{
    EntityParam, SELECTION_FALLBACK_OPS, parse_run_ops, resolve_entity_params, run_op_clause,
};
use jackdaw::commands::CommandHistory;
use jackdaw::scene_io::SceneFilePath;
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::OperatorEntity;
use jackdaw_bind::{BindPath, Binding, Bindings};
use jackdaw_bsn::{BsnValue, SceneBsnAst};
use jackdaw_scene_types::UiSceneRoot;

const NODE: &str = "bevy_ui::ui_node::Node";

/// A reflected component to hang on an authored entity. The authoring ops
/// care about the plumbing, not the type, so the smallest one that can be
/// added, saved, and read back does.
#[derive(Component, Reflect, Default, Debug, PartialEq)]
#[reflect(Component, Default)]
struct AuthoringMarker {
    value: i32,
}

/// Game state for a binding to read. `current` is a fraction so a
/// percentage write has something to be a percentage of.
#[derive(Component, Reflect, Default, Debug, PartialEq)]
#[reflect(Component, Default)]
struct AuthoringHealth {
    current: f32,
    alive: bool,
}

fn authoring_app() -> App {
    let mut app = util::editor_test_app();
    app.register_type::<AuthoringMarker>();
    app.register_type::<AuthoringHealth>();
    app
}

/// Run one clause, then tick the frame its queued work needs, standing in
/// for the boot queue's frame gap.
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

fn ui_roots(world: &mut World) -> Vec<Entity> {
    world
        .query_filtered::<Entity, jackdaw::prefab::AuthoredUiSceneRoot>()
        .iter(world)
        .collect()
}

fn authored(app: &App, entity: Entity, type_path: &str, field_path: &str) -> Option<BsnValue> {
    let ast = app.world().resource::<SceneBsnAst>();
    let node = ast.ast_for(entity)?;
    jackdaw_bsn::get_bsn_field(ast, node, type_path, field_path)
}

/// Resolve one clause's entity parameters without dispatching it, so a
/// test can tell a resolver refusal from an availability gate that would
/// have refused anyway.
#[track_caller]
fn resolve(app: &mut App, clause: &str) -> Vec<EntityParam> {
    let mut op = parse_run_ops(clause)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("{clause} parses to a clause"));
    resolve_entity_params(app.world_mut(), &mut op)
}

/// A named, document-tracked node carrying a default `Node`.
fn authored_node(app: &mut App, name: &str) -> Entity {
    let entity = app
        .world_mut()
        .spawn((Name::new(name.to_string()), Node::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    app.update();
    entity
}

fn width(app: &App, entity: Entity) -> Option<Val> {
    app.world().get::<Node>(entity).map(|node| node.width)
}

fn undo_depth(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

#[test]
fn a_new_ui_scene_arrives_with_a_root_to_build_on() {
    let mut app = authoring_app();

    run_finished(&mut app, "scene.new ui=true");

    let roots = ui_roots(app.world_mut());
    assert_eq!(roots.len(), 1, "one UI root, got {roots:?}");
    let root = roots[0];
    assert!(
        app.world().get::<UiSceneRoot>(root).is_some(),
        "the root carries UiSceneRoot, so the 2D stage and the runtime both know what it is"
    );
    assert!(
        app.world().get::<TabGroup>(root).is_some(),
        "the root carries TabGroup, so the screen a script authors is keyboard-reachable"
    );
    assert!(
        app.world()
            .resource::<SceneBsnAst>()
            .ast_for(root)
            .is_some(),
        "the root is in the document, so it survives a save"
    );
    assert_eq!(
        app.world().resource::<Selection>().primary(),
        Some(root),
        "the root is selected, so the next clause can act on it without naming it"
    );
}

/// The flag is the whole difference: a world scene stays a world scene.
#[test]
fn a_new_scene_without_the_flag_seeds_no_ui_root() {
    let mut app = authoring_app();

    run_finished(&mut app, "scene.new");

    assert!(
        ui_roots(app.world_mut()).is_empty(),
        "scene.new seeded a UI root without being asked"
    );
}

#[test]
fn scene_open_takes_a_path_instead_of_asking_for_one() {
    let mut app = authoring_app();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("opened.bsn");
    std::fs::write(
        &path,
        "#Greeting\nbevy_ui::ui_node::Node { width: bevy_ui::geometry::Val::Px(64.0) }\n",
    )
    .expect("write the scene");

    run_finished(&mut app, &format!("scene.open path={}", path.display()));

    let named: Vec<String> = app
        .world_mut()
        .query::<&Name>()
        .iter(app.world())
        .map(|name| name.as_str().to_string())
        .collect();
    assert!(
        named.iter().any(|name| name == "Greeting"),
        "the file's entity is live after the open; names present: {named:?}"
    );
    let active_path = {
        let scenes = app.world().resource::<jackdaw::scenes::Scenes>();
        scenes.tabs[scenes.active].path.clone()
    };
    assert_eq!(
        active_path.map(|p| dunce::canonicalize(&p).unwrap_or(p)),
        Some(dunce::canonicalize(&path).unwrap_or(path)),
        "the opened file is the active tab"
    );
}

#[test]
fn a_component_lands_on_the_selection_when_the_clause_names_no_entity() {
    let mut app = authoring_app();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.bsn");

    run_finished(
        &mut app,
        &format!("scene.new ui=true path={}", path.display()),
    );
    run_finished(&mut app, "entity.add.empty");

    let target = app
        .world()
        .resource::<Selection>()
        .primary()
        .expect("entity.add.empty selects what it made");

    run_finished(
        &mut app,
        "component.add type_path=operators::authoring_ops::AuthoringMarker",
    );

    assert!(
        app.world().entity(target).contains::<AuthoringMarker>(),
        "component.add resolved no entity from the selection"
    );
    let ast = app.world().resource::<SceneBsnAst>();
    let node = ast.ast_for(target).expect("the empty is in the document");
    assert!(
        ast.component_type_paths(node)
            .iter()
            .any(|type_path| type_path == "operators::authoring_ops::AuthoringMarker"),
        "the component is in the document, not only on the entity; document holds {:?}",
        ast.component_type_paths(node)
    );

    run_finished(&mut app, "scene.save");
    let written = std::fs::read_to_string(&path).expect("the session saved");
    assert!(
        written.contains("operators::authoring_ops::AuthoringMarker"),
        "the saved scene does not name the added component; wrote:\n{written}"
    );
}

/// The other half of the resolver: `name=` picks a target the selection
/// is not on, so a script can act on a node without clicking it first.
#[test]
fn a_component_lands_on_the_entity_a_clause_names() {
    let mut app = authoring_app();
    run_finished(&mut app, "scene.new ui=true");

    let target = app.world_mut().spawn(Name::new("Target")).id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), target);

    run_finished(&mut app, "entity.add.empty");
    let selected = app
        .world()
        .resource::<Selection>()
        .primary()
        .expect("the empty is selected");
    assert_ne!(
        selected, target,
        "precondition: the named entity is not the selection"
    );

    run_finished(
        &mut app,
        "component.add name=Target type_path=operators::authoring_ops::AuthoringMarker",
    );

    assert!(
        app.world().entity(target).contains::<AuthoringMarker>(),
        "component.add ignored `name=` and used the selection"
    );
    assert!(
        !app.world().entity(selected).contains::<AuthoringMarker>(),
        "component.add hit the selection as well as the named entity"
    );
}

#[test]
fn component_remove_takes_it_off_the_named_entity_again() {
    let mut app = authoring_app();
    run_finished(&mut app, "scene.new ui=true");

    let target = app
        .world_mut()
        .spawn((Name::new("Target"), AuthoringMarker { value: 3 }))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), target);
    app.update();

    run_finished(
        &mut app,
        "component.remove name=Target type_path=operators::authoring_ops::AuthoringMarker",
    );

    assert!(
        !app.world().entity(target).contains::<AuthoringMarker>(),
        "component.remove left the component on the named entity"
    );
}

#[test]
fn field_set_authors_the_document_as_one_undoable_edit() {
    let mut app = authoring_app();
    run_finished(&mut app, "scene.new ui=true");
    let root = ui_roots(app.world_mut())[0];
    let before = undo_depth(&app);

    run_finished(
        &mut app,
        &format!("field.set type_path={NODE} field=width value={{\"Px\":120.0}}"),
    );

    assert_eq!(
        app.world().get::<Node>(root).map(|node| node.width),
        Some(Val::Px(120.0)),
        "the live node did not move"
    );
    let Some(BsnValue::TupleStruct(data)) = authored(&app, root, NODE, "width") else {
        panic!(
            "the document holds no authored width; got {:?}",
            authored(&app, root, NODE, "width")
        );
    };
    assert!(
        data.type_path.ends_with("Val::Px"),
        "the document holds Val::Px; got {}",
        data.type_path
    );
    assert_eq!(
        undo_depth(&app),
        before + 1,
        "field.set minted {} undo entries, not one",
        undo_depth(&app) - before
    );

    run_finished(&mut app, "history.undo");
    assert_eq!(
        app.world().get::<Node>(root).map(|node| node.width),
        Some(percent(100.0)),
        "undo did not restore the width the root started with"
    );
}

/// The resolver's own refusal, told apart from the availability gate's.
///
/// Something is selected here, so `has_primary_selection` would wave the
/// clause through; the only thing that can stop it is the resolver
/// refusing a name no entity answers to.
#[test]
fn a_name_that_matches_nothing_is_refused_by_the_resolver_not_the_gate() {
    let mut app = authoring_app();
    run_finished(&mut app, "scene.new ui=true");
    let root = ui_roots(app.world_mut())[0];
    assert!(
        app.world().resource::<Selection>().primary().is_some(),
        "precondition: something is selected, so the availability gate would pass"
    );
    let clause = format!("field.set name=Nope type_path={NODE} field=width value={{\"Px\":120.0}}");

    let outcomes = resolve(&mut app, &clause);

    assert_eq!(
        outcomes,
        vec![EntityParam::NoSuchName {
            param: "entity",
            name: "Nope".to_string(),
        }]
    );
    let line = outcomes[0].line("field.set").expect("a refusal says why");
    assert!(
        line.contains("`Nope` names no entity in this scene, or more than one"),
        "the refusal names what it could not find: {line}"
    );

    let result = run(&mut app, &clause);
    assert_eq!(result, OperatorResult::Cancelled);
    assert_eq!(
        width(&app, root),
        Some(percent(100.0)),
        "the selected root was edited as a consolation prize"
    );
}

/// A `name=` that resolves also selects, so a clause lands from a cold
/// start, the state a boot run is in before anything clicks.
#[test]
fn a_named_target_becomes_the_selection_so_a_cold_clause_lands() {
    let mut app = authoring_app();
    run_finished(&mut app, "scene.new");
    let target = authored_node(&mut app, "Target");
    app.world_mut().resource_mut::<Selection>().entities.clear();
    app.update();
    assert_eq!(
        app.world().resource::<Selection>().primary(),
        None,
        "precondition: nothing is selected, so the gate would refuse"
    );

    run_finished(
        &mut app,
        "component.add name=Target type_path=operators::authoring_ops::AuthoringMarker",
    );

    assert!(
        app.world().entity(target).contains::<AuthoringMarker>(),
        "a cold `name=` clause did not land"
    );
    assert_eq!(
        app.world().resource::<Selection>().primary(),
        Some(target),
        "`name=` means select-then-act, so the operator's gate is satisfied the way a click \
         satisfies it"
    );
}

/// An operator whose gate is not the selection never guesses from it.
///
/// `prefab.apply_to_source` writes the prefab source document to disk, so
/// a guessed target in an unattended run edits a file the author never
/// pointed at.
#[test]
fn an_ungated_prefab_op_refuses_to_guess_its_target_from_the_selection() {
    let mut app = authoring_app();
    run_finished(&mut app, "scene.new");
    let instance = authored_node(&mut app, "Instance");
    jackdaw::selection::select_only(app.world_mut(), instance);
    app.update();
    let clause = "prefab.apply_to_source type_path=X field_path=y value_json=1";

    let outcomes = resolve(&mut app, clause);

    assert_eq!(
        outcomes,
        vec![EntityParam::NeedsAName {
            param: "instance_entity"
        }],
        "the selection filled in a parameter for an operator that writes a file"
    );
    let line = outcomes[0]
        .line("prefab.apply_to_source")
        .expect("a refusal says why");
    assert!(
        line.contains("instance_entity=<Name>"),
        "the refusal names the parameter to pass: {line}"
    );
    assert_eq!(run(&mut app, clause), OperatorResult::Cancelled);
}

/// Two entity parameters means no answer to which one is "the" target, so
/// both are named explicitly and neither touches the selection.
#[test]
fn a_two_entity_op_takes_both_targets_by_name_and_neither_from_the_selection() {
    let mut app = authoring_app();
    run_finished(&mut app, "scene.new");
    let child = authored_node(&mut app, "Child");
    let drop_target = authored_node(&mut app, "Drop");
    let elsewhere = authored_node(&mut app, "Elsewhere");
    jackdaw::selection::select_only(app.world_mut(), elsewhere);
    app.update();

    let outcomes = resolve(
        &mut app,
        "prefab.unpack_child child_entity=Child drop_target_entity=Drop",
    );

    assert_eq!(
        outcomes,
        vec![
            EntityParam::Named {
                param: "child_entity",
                name: "Child".to_string(),
                entity: child,
                entity_name: "Child".to_string(),
            },
            EntityParam::Named {
                param: "drop_target_entity",
                name: "Drop".to_string(),
                entity: drop_target,
                entity_name: "Drop".to_string(),
            },
        ]
    );
    assert_eq!(
        app.world().resource::<Selection>().primary(),
        Some(elsewhere),
        "a multi-target operator moved the selection to one of its targets"
    );

    let omitted = resolve(&mut app, "prefab.unpack_child");
    assert_eq!(
        omitted,
        vec![
            EntityParam::NeedsAName {
                param: "child_entity"
            },
            EntityParam::NeedsAName {
                param: "drop_target_entity"
            },
        ]
    );
}

/// The boot path must not be the hole in the `as_entity` guard: a number
/// where an entity belongs is refused there too, not coerced.
#[test]
fn a_number_where_an_entity_belongs_is_refused_on_the_boot_path_too() {
    let mut app = authoring_app();
    run_finished(&mut app, "scene.new ui=true");
    let root = ui_roots(app.world_mut())[0];
    let clause = "component.add entity=42 type_path=operators::authoring_ops::AuthoringMarker";

    let outcomes = resolve(&mut app, clause);

    assert!(
        matches!(
            outcomes.as_slice(),
            [EntityParam::NotAName {
                param: "entity",
                ..
            }]
        ),
        "an Int for `entity` was treated as a target: {outcomes:?}"
    );
    let line = outcomes[0]
        .line("component.add")
        .expect("a refusal says why");
    assert!(
        line.contains("neither an entity nor a name"),
        "the refusal says what is wrong with the value: {line}"
    );

    assert_eq!(run(&mut app, clause), OperatorResult::Cancelled);
    assert!(
        !app.world().entity(root).contains::<AuthoringMarker>(),
        "the refused clause still added the component to the selection"
    );
}

/// Every operator declaring an `Entity` parameter, and whether the
/// resolver may fill it from the selection.
///
/// Each operator has to be classified here explicitly. Left to a default
/// it would inherit whichever behaviour the resolver happens to have, and
/// for the prefab family the wrong default writes files.
const ENTITY_PARAM_OPS: &[(&str, &[&str], bool)] = &[
    ("animation.toggle_keyframe", &["entity"], true),
    ("binding.add", &["entity"], true),
    ("binding.set", &["entity"], true),
    ("component.add", &["entity"], true),
    ("component.remove", &["entity"], true),
    ("component.revert_baseline", &["entity"], true),
    ("entity.reparent", &["child", "parent"], false),
    ("field.set", &["entity"], true),
    ("hierarchy.rename_begin", &["entity"], true),
    ("physics.disable", &["entity"], true),
    ("physics.enable", &["entity"], true),
    ("prefab.apply_to_source", &["instance_entity"], false),
    ("prefab.open_source", &["entity"], false),
    ("prefab.revert_all", &["instance_entity"], false),
    ("prefab.revert_component", &["entity"], false),
    ("prefab.revert_field", &["entity"], false),
    ("prefab.unbundle_instance", &["instance_entity"], false),
    (
        "prefab.unpack_child",
        &["child_entity", "drop_target_entity"],
        false,
    ),
    ("widget.add", &["parent"], false),
];

#[test]
fn every_entity_taking_operator_is_classified() {
    let mut app = authoring_app();

    let mut found: Vec<(String, Vec<&str>)> = app
        .world_mut()
        .query::<&OperatorEntity>()
        .iter(app.world())
        .filter_map(|op| {
            let params: Vec<&str> = op
                .parameters()
                .iter()
                .filter(|spec| spec.ty == "Entity")
                .map(|spec| spec.name)
                .collect();
            (!params.is_empty()).then(|| (op.id().to_string(), params))
        })
        .collect();
    found.sort();
    found.dedup();

    let expected: Vec<(String, Vec<&str>)> = ENTITY_PARAM_OPS
        .iter()
        .map(|(id, params, _)| (id.to_string(), params.to_vec()))
        .collect();
    assert_eq!(
        found, expected,
        "an operator gained or lost an `Entity` parameter; classify it in ENTITY_PARAM_OPS and \
         decide whether the selection may fill it in"
    );

    let allowed: Vec<&str> = ENTITY_PARAM_OPS
        .iter()
        .filter(|(_, _, fallback)| *fallback)
        .map(|(id, ..)| *id)
        .collect();
    assert_eq!(
        SELECTION_FALLBACK_OPS, allowed,
        "SELECTION_FALLBACK_OPS and this table disagree about who may guess from the selection"
    );
}

/// A target outside the selection takes the selection over. Documented on
/// the operator, pinned here, because a later clause reads that selection.
#[test]
fn field_set_takes_over_a_selection_that_was_somewhere_else() {
    let mut app = authoring_app();
    run_finished(&mut app, "scene.new");
    let selected = authored_node(&mut app, "Selected");
    let target = authored_node(&mut app, "Target");
    jackdaw::selection::select_only(app.world_mut(), selected);
    app.update();

    let result = app
        .world_mut()
        .operator("field.set")
        .param("entity", target)
        .param("type_path", NODE.to_string())
        .param("field", "width".to_string())
        .param("value", "{\"Px\":120.0}".to_string())
        .call()
        .expect("field.set dispatches");
    app.update();

    assert_eq!(result, OperatorResult::Finished);
    assert_eq!(width(&app, target), Some(Val::Px(120.0)));
    assert_eq!(
        width(&app, selected),
        Some(Val::Auto),
        "the edit reached the entity that merely happened to be selected"
    );
    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![target],
        "the target did not take the selection over"
    );
}

/// A target already inside a multi-selection leaves it alone, so the
/// gesture's broadcast survives: every selected entity moves, as one entry.
#[test]
fn field_set_broadcasts_across_a_multi_selection_as_one_entry() {
    let mut app = authoring_app();
    run_finished(&mut app, "scene.new");
    let first = authored_node(&mut app, "First");
    let second = authored_node(&mut app, "Second");
    app.world_mut().resource_mut::<Selection>().entities = vec![first, second];
    app.update();
    let before = undo_depth(&app);

    let result = app
        .world_mut()
        .operator("field.set")
        .param("entity", first)
        .param("type_path", NODE.to_string())
        .param("field", "width".to_string())
        .param("value", "{\"Px\":120.0}".to_string())
        .call()
        .expect("field.set dispatches");
    app.update();
    assert_eq!(result, OperatorResult::Finished);

    assert_eq!(width(&app, first), Some(Val::Px(120.0)));
    assert_eq!(
        width(&app, second),
        Some(Val::Px(120.0)),
        "the broadcast was narrowed to the named target"
    );
    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![first, second],
        "a target already selected still rewrote the selection"
    );
    assert_eq!(
        undo_depth(&app),
        before + 1,
        "a two-entity edit is one undo entry, not two"
    );

    run_finished(&mut app, "history.undo");
    assert_eq!(width(&app, first), Some(Val::Auto));
    assert_eq!(
        width(&app, second),
        Some(Val::Auto),
        "one undo took back only half the broadcast"
    );
    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![first, second],
        "undo restores the field, not the selection"
    );
}

/// A new untitled tab must not inherit the last file's save path, or the
/// next save writes an empty scene over the file the user was editing.
///
/// Pinned at the resource rather than by calling `scene.save`: an untitled
/// tab sends `scene.save` to the native Save As dialog, which a headless
/// run must not open.
#[test]
fn a_new_untitled_scene_stops_pointing_at_the_file_that_was_open() {
    let mut app = authoring_app();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("kept.bsn");
    std::fs::write(&path, "#Greeting\nbevy_ui::ui_node::Node\n").expect("write the scene");
    run_finished(&mut app, &format!("scene.open path={}", path.display()));
    assert!(
        app.world().resource::<SceneFilePath>().path.is_some(),
        "precondition: the opened file is the save target"
    );

    run_finished(&mut app, "scene.new");

    assert_eq!(
        app.world().resource::<SceneFilePath>().path,
        None,
        "the untitled tab still points at the file that was open, so the next save overwrites it"
    );
}

/// The second UI scene of a session goes down the tab-swap path, where the
/// seeding order matters: a root spawned before the swap leaves with the
/// scene the swap puts away.
#[test]
fn a_second_new_ui_scene_in_a_session_still_arrives_with_its_root() {
    let mut app = authoring_app();
    run_finished(&mut app, "scene.new ui=true");
    let first = ui_roots(app.world_mut())[0];

    run_finished(&mut app, "scene.new ui=true");

    let roots = ui_roots(app.world_mut());
    assert_eq!(
        roots.len(),
        1,
        "the previous scene's root is still live after the swap; got {roots:?}"
    );
    assert_ne!(roots[0], first, "the second scene reused the first's root");
    assert!(
        app.world()
            .resource::<SceneBsnAst>()
            .ast_for(roots[0])
            .is_some(),
        "the second scene's root is in its own document"
    );
    assert_eq!(
        app.world().resource::<Selection>().primary(),
        Some(roots[0]),
        "the second scene's root is selected, like the first's"
    );
}

// --- Widgets, bindings, reparenting ---------------------------------------

/// The widget vocabulary ships as its own extension, which an on-disk
/// config may leave off. Force it on so `widget.add` has definitions to
/// name, the way `tests/ui_palette.rs` does.
fn widget_app() -> App {
    let mut app = authoring_app();
    jackdaw_api_internal::lifecycle::enable_extension(app.world_mut(), "jackdaw.ui_palette");
    app.update();
    app
}

#[track_caller]
fn named(app: &mut App, wanted: &str) -> Entity {
    let found: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &Name)>()
        .iter(app.world())
        .filter(|(_, name)| name.as_str() == wanted)
        .map(|(entity, _)| entity)
        .collect();
    assert_eq!(found.len(), 1, "one entity named {wanted}, got {found:?}");
    found[0]
}

/// The bindings the scene document holds for `entity`, as authored
/// values. What survives a save, as opposed to what the ECS carries.
fn authored_bindings(app: &App, entity: Entity) -> Vec<BsnValue> {
    let ast = app.world().resource::<SceneBsnAst>();
    let node = ast.ast_for(entity).expect("the entity is in the document");
    let patch = ast
        .find_patch_by_type_path(node, "jackdaw_bind::types::Bindings")
        .and_then(|patch| ast.get_patch(patch))
        .expect("the document holds a Bindings patch");
    let jackdaw_bsn::BsnPatch::TupleStruct(data) = patch else {
        panic!("Bindings authors as a newtype patch, got {patch:?}");
    };
    match data.values.first() {
        Some(BsnValue::List(items)) => items.clone(),
        other => panic!("Bindings holds a list of bindings, got {other:?}"),
    }
}

fn bindings_of(app: &App, entity: Entity) -> Vec<Binding> {
    app.world()
        .get::<Bindings>(entity)
        .map(|bindings| bindings.0.clone())
        .unwrap_or_default()
}

/// Put an empty `Bindings` on `entity` the way the Add Component picker
/// does, since that is what a binding clause requires to be there first.
#[track_caller]
fn give_bindings(app: &mut App, name: &str) {
    run_finished(
        app,
        &format!("component.add name={name} type_path=jackdaw_bind::types::Bindings"),
    );
}

#[test]
fn a_widget_clause_lands_a_widget_in_the_open_ui_scene() {
    let mut app = widget_app();
    run_finished(&mut app, "scene.new ui=true");
    let root = ui_roots(app.world_mut())[0];
    let before = undo_depth(&app);

    run_finished(&mut app, "widget.add name=ui.button");

    let button = named(&mut app, "Button");
    assert_eq!(
        app.world().get::<ChildOf>(button).map(ChildOf::parent),
        Some(root),
        "the widget did not land in the open UI scene"
    );
    assert!(
        app.world()
            .resource::<SceneBsnAst>()
            .ast_for(button)
            .is_some(),
        "the widget is not in the document, so a save would lose it"
    );
    assert_eq!(
        app.world().resource::<Selection>().primary(),
        Some(button),
        "the new widget is not selected, so the next clause cannot build on it"
    );
    assert_eq!(
        undo_depth(&app),
        before + 1,
        "one widget clause is one undo entry"
    );

    run_finished(&mut app, "history.undo");
    assert!(
        app.world().get_entity(button).is_err(),
        "undo left the widget behind"
    );
    assert!(
        app.world()
            .resource::<SceneBsnAst>()
            .ast_for(button)
            .is_none(),
        "undo left the widget's node in the document"
    );
}

#[test]
fn a_widget_takes_the_parent_the_clause_names() {
    let mut app = widget_app();
    run_finished(&mut app, "scene.new ui=true");
    run_finished(&mut app, "widget.add name=ui.panel");
    let panel = named(&mut app, "Panel");
    let root = ui_roots(app.world_mut())[0];
    jackdaw::selection::select_only(app.world_mut(), root);
    app.update();

    run_finished(&mut app, "widget.add name=ui.button parent=Panel");

    let button = named(&mut app, "Button");
    assert_eq!(
        app.world().get::<ChildOf>(button).map(ChildOf::parent),
        Some(panel),
        "`parent=` lost to the selection"
    );
}

/// The palette's own rule, whichever way the parent arrived: a node that
/// is not part of the UI scene cannot adopt a widget, so the scene root
/// does. Pinned here because a clause can name a parent a click cannot.
#[test]
fn a_parent_outside_the_ui_scene_hands_the_widget_to_the_root() {
    let mut app = widget_app();
    run_finished(&mut app, "scene.new ui=true");
    let root = ui_roots(app.world_mut())[0];
    let elsewhere = app
        .world_mut()
        .spawn((Name::new("Cube"), Transform::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), elsewhere);
    app.update();

    run_finished(&mut app, "widget.add name=ui.button parent=Cube");

    let button = named(&mut app, "Button");
    assert_eq!(
        app.world().get::<ChildOf>(button).map(ChildOf::parent),
        Some(root),
        "a widget was parented outside the UI scene"
    );
}

/// Three scripted adds make three siblings, the same as three presses of the
/// palette row.
///
/// A bare `widget.add` names no parent, so the widget goes beside the
/// selection; the selection after each add is the widget it made. Filling
/// `parent` in from the selection would turn each clause into the adopting
/// form and build a chain instead.
#[test]
fn three_widget_clauses_make_three_siblings() {
    let mut app = widget_app();
    run_finished(&mut app, "scene.new ui=true");
    let root = ui_roots(app.world_mut())[0];

    for _ in 0..3 {
        run_finished(&mut app, "widget.add name=ui.button");
    }

    let names: Vec<String> = app
        .world()
        .get::<Children>(root)
        .map(|children| {
            children
                .iter()
                .filter_map(|child| app.world().get::<Name>(child))
                .map(|name| name.as_str().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(names, vec!["Button", "Button2", "Button3"]);
}

#[test]
fn a_field_binding_is_authored_from_a_clause() {
    let mut app = widget_app();
    run_finished(&mut app, "scene.new ui=true");
    run_finished(&mut app, "widget.add name=ui.button");
    give_bindings(&mut app, "Button");
    let button = named(&mut app, "Button");
    let before = undo_depth(&app);

    run_finished(
        &mut app,
        "binding.add entity=Button kind=field read=operators::authoring_ops::AuthoringHealth.current \
         write=bevy_ui::ui_node::Node.width as_percent=true",
    );

    assert_eq!(
        bindings_of(&app, button),
        vec![Binding::Field {
            read: vec![BindPath::new(
                "operators::authoring_ops::AuthoringHealth.current"
            )],
            via: None,
            write: BindPath::new("bevy_ui::ui_node::Node.width"),
            as_percent: true,
        }],
        "the clause authored a different binding than the card would have"
    );
    assert_eq!(
        authored_bindings(&app, button).len(),
        1,
        "the binding is on the entity but not in the document"
    );
    assert_eq!(
        undo_depth(&app),
        before + 1,
        "one binding clause is one undo entry"
    );

    run_finished(&mut app, "history.undo");
    assert_eq!(
        bindings_of(&app, button),
        Vec::new(),
        "undo did not take the binding back"
    );
}

#[test]
fn a_binding_clause_refuses_an_entity_that_carries_no_bindings() {
    let mut app = widget_app();
    run_finished(&mut app, "scene.new ui=true");
    run_finished(&mut app, "widget.add name=ui.button");
    let button = named(&mut app, "Button");

    let result = run(&mut app, "binding.add entity=Button kind=value");

    assert_eq!(
        result,
        OperatorResult::Cancelled,
        "a binding clause reported success on an entity with no Bindings"
    );
    assert!(
        app.world().get::<Bindings>(button).is_none(),
        "the clause added the component itself instead of saying to"
    );
}

#[test]
fn a_binding_clause_edits_one_binding_by_index() {
    let mut app = widget_app();
    run_finished(&mut app, "scene.new ui=true");
    run_finished(&mut app, "widget.add name=ui.slider");
    give_bindings(&mut app, "Slider");
    let slider = named(&mut app, "Slider");
    run_finished(&mut app, "binding.add entity=Slider kind=value");
    run_finished(&mut app, "binding.add entity=Slider kind=visible");

    run_finished(
        &mut app,
        "binding.set entity=Slider index=0 read=operators::authoring_ops::AuthoringHealth.current \
         two_way=true",
    );

    assert_eq!(
        bindings_of(&app, slider),
        vec![
            Binding::Value {
                with: BindPath::new("operators::authoring_ops::AuthoringHealth.current"),
                two_way: true,
            },
            Binding::Visible {
                read: BindPath::default(),
                via: None,
            },
        ],
        "binding.set edited the wrong binding, or more than one"
    );
}

/// Every shape the card's own Add Binding menu offers, spelled as a
/// clause. These five plus the index edits above are the whole clause
/// vocabulary for authoring a bound scene; anything finer stays a pointer
/// gesture.
#[test]
fn every_binding_shape_the_card_offers_is_reachable_from_a_clause() {
    let mut app = widget_app();
    run_finished(&mut app, "scene.new ui=true");
    run_finished(&mut app, "widget.add name=ui.button");
    give_bindings(&mut app, "Button");
    let button = named(&mut app, "Button");

    run_finished(
        &mut app,
        "binding.add entity=Button kind=field read=operators::authoring_ops::AuthoringHealth.current \
         via=jackdaw_bind::clamp01 write=bevy_ui::ui_node::Node.width as_percent=true",
    );
    run_finished(
        &mut app,
        "binding.add entity=Button kind=field read=operators::authoring_ops::AuthoringHealth.alive \
         write=operators::authoring_ops::AuthoringMarker",
    );
    run_finished(
        &mut app,
        "binding.add entity=Button kind=text format={}/100 \
         read=operators::authoring_ops::AuthoringHealth.current",
    );
    run_finished(
        &mut app,
        "binding.add entity=Button kind=value with=operators::authoring_ops::AuthoringHealth.current \
         two_way=true",
    );
    run_finished(
        &mut app,
        "binding.add entity=Button kind=action event=operators::authoring_ops::Hit \
         map=amount:operators::authoring_ops::AuthoringHealth.current",
    );

    assert_eq!(
        bindings_of(&app, button),
        vec![
            Binding::Field {
                read: vec![BindPath::new(
                    "operators::authoring_ops::AuthoringHealth.current"
                )],
                via: Some("jackdaw_bind::clamp01".to_string()),
                write: BindPath::new("bevy_ui::ui_node::Node.width"),
                as_percent: true,
            },
            Binding::Field {
                read: vec![BindPath::new(
                    "operators::authoring_ops::AuthoringHealth.alive"
                )],
                via: None,
                write: BindPath::new("operators::authoring_ops::AuthoringMarker"),
                as_percent: false,
            },
            Binding::Text {
                format: "{}/100".to_string(),
                args: vec![BindPath::new(
                    "operators::authoring_ops::AuthoringHealth.current"
                )],
            },
            Binding::Value {
                with: BindPath::new("operators::authoring_ops::AuthoringHealth.current"),
                two_way: true,
            },
            Binding::Action {
                event: "operators::authoring_ops::Hit".to_string(),
                fields: vec![(
                    "amount".to_string(),
                    BindPath::new("operators::authoring_ops::AuthoringHealth.current"),
                )],
            },
        ],
        "a clause authored a shape the card would not have"
    );
}

#[test]
fn a_reparent_clause_moves_a_child_and_undo_puts_it_back() {
    let mut app = authoring_app();
    run_finished(&mut app, "scene.new");
    let first = authored_node(&mut app, "First");
    let second = authored_node(&mut app, "Second");
    let child = authored_node(&mut app, "Child");
    app.world_mut().entity_mut(child).insert(ChildOf(first));
    app.update();
    let before = undo_depth(&app);

    run_finished(&mut app, "entity.reparent child=Child parent=Second");

    assert_eq!(
        app.world().get::<ChildOf>(child).map(ChildOf::parent),
        Some(second),
        "the child did not move"
    );
    assert_eq!(
        undo_depth(&app),
        before + 1,
        "one reparent clause is one undo entry"
    );

    run_finished(&mut app, "history.undo");
    assert_eq!(
        app.world().get::<ChildOf>(child).map(ChildOf::parent),
        Some(first),
        "undo did not put the child back where it was"
    );
}

/// The selection may only fill in a sole target, so with two entity
/// parameters both are named and the selection is left alone.
#[test]
fn a_reparent_clause_names_both_targets_and_takes_neither_from_the_selection() {
    let mut app = authoring_app();
    run_finished(&mut app, "scene.new");
    let child = authored_node(&mut app, "Child");
    let parent = authored_node(&mut app, "Parent");
    let elsewhere = authored_node(&mut app, "Elsewhere");
    jackdaw::selection::select_only(app.world_mut(), elsewhere);
    app.update();

    let outcomes = resolve(&mut app, "entity.reparent child=Child parent=Parent");

    assert_eq!(
        outcomes,
        vec![
            EntityParam::Named {
                param: "child",
                name: "Child".to_string(),
                entity: child,
                entity_name: "Child".to_string(),
            },
            EntityParam::Named {
                param: "parent",
                name: "Parent".to_string(),
                entity: parent,
                entity_name: "Parent".to_string(),
            },
        ]
    );
    assert_eq!(
        app.world().resource::<Selection>().primary(),
        Some(elsewhere),
        "a two-target clause moved the selection to one of its targets"
    );

    assert_eq!(
        resolve(&mut app, "entity.reparent child=Child"),
        vec![
            EntityParam::Named {
                param: "child",
                name: "Child".to_string(),
                entity: child,
                entity_name: "Child".to_string(),
            },
            EntityParam::NeedsAName { param: "parent" },
        ],
        "the missing half was guessed from the selection"
    );
    assert_eq!(
        run(&mut app, "entity.reparent child=Child"),
        OperatorResult::Cancelled
    );
}

/// A clause can name a parent no drag could reach. A subtree adopting its
/// own ancestor is not a hierarchy, so the clause refuses instead.
#[test]
fn a_reparent_clause_refuses_to_put_a_parent_under_its_own_child() {
    let mut app = authoring_app();
    run_finished(&mut app, "scene.new");
    let outer = authored_node(&mut app, "Outer");
    let inner = authored_node(&mut app, "Inner");
    app.world_mut().entity_mut(inner).insert(ChildOf(outer));
    app.update();

    let result = run(&mut app, "entity.reparent child=Outer parent=Inner");

    assert_eq!(
        result,
        OperatorResult::Cancelled,
        "the clause built a parent cycle"
    );
    assert_eq!(
        app.world().get::<ChildOf>(inner).map(ChildOf::parent),
        Some(outer),
        "the refused clause moved something anyway"
    );
    assert!(app.world().get::<ChildOf>(outer).is_none());
}

/// A bound two-widget scene authored purely by clauses loads and behaves
/// like the same scene authored by hand.
///
/// "By hand" means the palette and document APIs the editor's own pointer
/// gestures call, with no operator involved. Both scenes are saved, loaded
/// back into fresh editors, and compared on the components they carry and
/// the value the binding evaluates to.
#[test]
fn a_scripted_scene_loads_and_evaluates_like_a_hand_authored_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scripted_path = dir.path().join("scripted.bsn");
    let hand_path = dir.path().join("hand.bsn");

    let mut scripted = widget_app();
    run_finished(
        &mut scripted,
        &format!("scene.new ui=true path={}", scripted_path.display()),
    );
    run_finished(&mut scripted, "widget.add name=ui.panel");
    run_finished(&mut scripted, "widget.add name=ui.label parent=Panel");
    give_bindings(&mut scripted, "Panel");
    run_finished(
        &mut scripted,
        "binding.add entity=Panel kind=field read=operators::authoring_ops::AuthoringHealth.current \
         write=bevy_ui::ui_node::Node.width as_percent=true",
    );
    run_finished(
        &mut scripted,
        &format!("field.set name=Label type_path={NODE} field=height value={{\"Px\":24.0}}"),
    );
    run_finished(&mut scripted, "scene.save");

    let mut hand = widget_app();
    hand_author(&mut hand, &hand_path);

    let scripted_load = load_and_evaluate(&scripted_path);
    let hand_load = load_and_evaluate(&hand_path);
    assert_eq!(
        scripted_load, hand_load,
        "the scripted scene loads differently from the hand-authored one"
    );
    assert_eq!(
        scripted_load.width,
        Val::Percent(50.0),
        "the binding did not drive the width on either scene"
    );
    assert_eq!(scripted_load.height, Val::Px(24.0));
}

/// The same scene as the scripted arm, built through the palette and document
/// APIs a pointer gesture drives. The authoring is operator-free: making the
/// tab and pointing it at a file are still `scene.new` and the tab's own path,
/// which is what the File menu does on either arm.
fn hand_author(app: &mut App, path: &std::path::Path) {
    run_finished(app, "scene.new ui=true");
    let world = app.world_mut();
    let panel = jackdaw::ui_palette::instantiate_widget(world, "ui.panel").expect("a panel");
    // Naming the parent, as the scripted arm's `parent=Panel` does: a widget
    // added with no parent named is the selection's sibling, not its child.
    jackdaw::ui_palette::instantiate_widget_under(world, "ui.label", Some(panel))
        .expect("a label under the panel");
    let bindings = Bindings(vec![Binding::Field {
        read: vec![BindPath::new(
            "operators::authoring_ops::AuthoringHealth.current",
        )],
        via: None,
        write: BindPath::new("bevy_ui::ui_node::Node.width"),
        as_percent: true,
    }]);
    world.entity_mut(panel).insert(bindings.clone());
    jackdaw::commands::sync_component_to_ast(
        world,
        panel,
        "jackdaw_bind::types::Bindings",
        &bindings,
    );
    let label = named(app, "Label");
    let mut node = app
        .world_mut()
        .get_mut::<Node>(label)
        .expect("the label has a Node");
    node.height = Val::Px(24.0);
    let node = node.clone();
    jackdaw::commands::sync_component_to_ast(app.world_mut(), label, NODE, &node);
    // The active tab is what a save reads for its target. An untitled one
    // sends `save_scene` to the native Save As dialog, which a headless
    // run must not open.
    let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
    let active = scenes.active;
    scenes.tabs[active].path = Some(path.to_path_buf());
    assert!(
        jackdaw::scene_io::save_scene(app.world_mut()),
        "the hand-authored scene did not reach disk"
    );
    app.update();
}

/// What a loaded scene is, for the purposes of the diff: the components
/// each authored node carries and what the binding does once it runs.
#[derive(Debug, PartialEq)]
struct LoadedScene {
    panel_components: Vec<String>,
    label_components: Vec<String>,
    bindings: Vec<Binding>,
    width: Val,
    height: Val,
}

fn load_and_evaluate(path: &std::path::Path) -> LoadedScene {
    let mut app = widget_app();
    run_finished(&mut app, &format!("scene.open path={}", path.display()));
    let panel = named(&mut app, "Panel");
    let label = named(&mut app, "Label");

    // The subject the binding reads from, named the way a game names it.
    let subject = app
        .world_mut()
        .spawn((
            Name::new("Subject"),
            AuthoringHealth {
                current: 0.5,
                alive: true,
            },
        ))
        .id();
    app.world_mut()
        .entity_mut(panel)
        .insert(jackdaw_bind::BindContext(subject));
    app.update();
    app.world_mut()
        .run_system_cached(jackdaw_bind::evaluate_bindings)
        .expect("the evaluator runs");

    LoadedScene {
        panel_components: component_paths(&app, panel),
        label_components: component_paths(&app, label),
        bindings: bindings_of(&app, panel),
        width: app.world().get::<Node>(panel).expect("a panel Node").width,
        height: app.world().get::<Node>(label).expect("a label Node").height,
    }
}

/// The authored component type paths on one entity, sorted. `BindContext`
/// is dropped: the test puts it there, the scene does not.
fn component_paths(app: &App, entity: Entity) -> Vec<String> {
    let mut paths: Vec<String> = app
        .world()
        .inspect_entity(entity)
        .expect("the entity is live")
        .filter_map(|info| info.type_id().map(|_| info.name().to_string()))
        .filter(|name| !name.starts_with("jackdaw_bind::types::BindContext"))
        .collect();
    paths.sort();
    paths
}

/// How many entities the open document holds. A refused clause is one
/// the document never heard about; counting live entities instead would
/// count the editor's own chrome, which a frame tick spawns and drops
/// whatever the clause did.
fn document_size(app: &App) -> usize {
    app.world().resource::<SceneBsnAst>().ecs_to_ast.len()
}

/// A name the registry does not answer to gets a refusal, not a
/// `Finished` with a warning nobody reads. The clause did not happen, and
/// the run's own record has to say so.
#[test]
fn a_widget_name_that_names_no_definition_is_refused() {
    let mut app = widget_app();
    run_finished(&mut app, "scene.new ui=true");
    let before = document_size(&app);

    let result = run(&mut app, "widget.add name=ui.nope");

    assert_eq!(result, OperatorResult::Cancelled);
    assert_eq!(
        document_size(&app),
        before,
        "a refused widget clause authored something anyway"
    );
}

#[test]
fn a_widget_clause_with_no_ui_scene_to_put_it_in_is_refused() {
    let mut app = widget_app();
    run_finished(&mut app, "scene.new");
    let before = document_size(&app);

    let result = run(&mut app, "widget.add name=ui.button");

    assert_eq!(result, OperatorResult::Cancelled);
    assert_eq!(
        document_size(&app),
        before,
        "a widget landed in a document with no UI scene"
    );
}

/// An index the clause spelled but nothing can read is a typo, and a typo
/// that edits binding zero is worse than one that edits nothing.
#[test]
fn a_binding_index_that_is_no_position_is_refused() {
    let mut app = widget_app();
    run_finished(&mut app, "scene.new ui=true");
    run_finished(&mut app, "widget.add name=ui.slider");
    give_bindings(&mut app, "Slider");
    let slider = named(&mut app, "Slider");
    run_finished(&mut app, "binding.add entity=Slider kind=value");
    let before = bindings_of(&app, slider);

    for clause in [
        "binding.set entity=Slider index=-1 two_way=true",
        "binding.set entity=Slider index=notanumber two_way=true",
    ] {
        assert_eq!(
            run(&mut app, clause),
            OperatorResult::Cancelled,
            "{clause} was accepted"
        );
        assert_eq!(
            bindings_of(&app, slider),
            before,
            "{clause} edited a binding anyway"
        );
    }
}

#[test]
fn a_reparent_clause_refuses_to_put_an_entity_under_itself() {
    let mut app = authoring_app();
    run_finished(&mut app, "scene.new");
    let node = authored_node(&mut app, "Only");

    let result = run(&mut app, "entity.reparent child=Only parent=Only");

    assert_eq!(result, OperatorResult::Cancelled);
    assert!(
        app.world().get::<ChildOf>(node).is_none(),
        "an entity became its own child"
    );
}
