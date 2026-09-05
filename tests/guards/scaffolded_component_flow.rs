//! End-to-end coverage for the scaffolded user flow: a custom
//! component reaches the picker, `component.add` attaches it to
//! an authored entity, and the scene document records the addition
//! so a save/load round-trip would persist it.

use crate::util;

use std::collections::HashSet;

use bevy::prelude::*;
use jackdaw::commands::{EditorCommand, SetBsnField};
use jackdaw::inspector::component_picker::{PickerDenylist, enumerate_pickable_components};
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_bsn::SceneBsnAst;
use jackdaw_runtime::{EditorCategory, EditorHidden};

#[derive(Component, Reflect)]
#[reflect(Component, @EditorCategory::new("Gameplay"))]
struct SpinningCube {
    speed: f32,
    enabled: bool,
}

#[derive(Component, Reflect)]
#[reflect(Component, @EditorCategory::new("Actor"))]
struct PlayerSpawn;

/// Mirrors a plugin author marking a helper Component as editor-hidden, through
/// the same public API a user's crate would use.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default, @EditorHidden)]
struct PluginInternalSupport;

fn app_with_user_components() -> App {
    let mut app = util::editor_test_app();
    app.register_type::<SpinningCube>();
    app.register_type::<PlayerSpawn>();
    app
}

/// Spawn an authored entity, register it in the scene document so
/// component edits persist, and make it the primary selection.
fn spawn_authored_entity(app: &mut App) -> Entity {
    let entity = app.world_mut().spawn(Name::new("authored")).id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    app.world_mut().resource_mut::<Selection>().entities = vec![entity];
    app.update();
    entity
}

#[test]
fn scaffolded_user_components_reach_picker() {
    let app = app_with_user_components();
    let registry = app
        .world()
        .resource::<bevy::ecs::reflect::AppTypeRegistry>()
        .read();
    let pickables =
        enumerate_pickable_components(&registry, &HashSet::new(), &PickerDenylist::default());

    let spinning = pickables
        .iter()
        .find(|p| p.short_name == "SpinningCube")
        .expect("SpinningCube must appear in the picker");
    assert_eq!(spinning.category, "Gameplay");

    let player = pickables
        .iter()
        .find(|p| p.short_name == "PlayerSpawn")
        .expect("PlayerSpawn must appear in the picker");
    assert_eq!(player.category, "Actor");
}

#[test]
fn editor_hidden_marker_hides_component_in_real_app() {
    // Registered through the path a user's crate would use, then the full editor
    // type registry is walked to check the picker filters it out.
    let mut app = util::editor_test_app();
    app.register_type::<PluginInternalSupport>();
    app.register_type::<SpinningCube>();
    app.update();

    let registry = app
        .world()
        .resource::<bevy::ecs::reflect::AppTypeRegistry>()
        .read();
    let pickables =
        enumerate_pickable_components(&registry, &HashSet::new(), &PickerDenylist::default());
    let names: Vec<&str> = pickables.iter().map(|p| p.short_name.as_str()).collect();

    assert!(
        names.contains(&"SpinningCube"),
        "control: unmarked component must still appear in the picker; got {names:?}",
    );
    assert!(
        !names.contains(&"PluginInternalSupport"),
        "@EditorHidden must hide a Component from the picker when \
         registered through the same App lifecycle a user/extension would use; got {names:?}",
    );
}

#[test]
fn add_component_lands_on_entity_and_in_ast() {
    let mut app = app_with_user_components();
    let entity = spawn_authored_entity(&mut app);

    let result = app
        .world_mut()
        .operator("component.add")
        .param("entity", entity)
        .param(
            "type_path",
            "guards::scaffolded_component_flow::SpinningCube".to_string(),
        )
        .call()
        .expect("dispatch resolves");
    assert_eq!(result, OperatorResult::Finished);

    app.update();

    let cube = app
        .world()
        .entity(entity)
        .get::<SpinningCube>()
        .expect("SpinningCube must land on the entity");
    assert_eq!(cube.speed, 0.0, "default-constructed value");
    assert!(!cube.enabled);

    let ast = app.world().resource::<SceneBsnAst>();
    let node = ast
        .ast_for(entity)
        .expect("authored entity must be tracked in the document");
    assert!(
        ast.component_type_paths(node)
            .iter()
            .any(|tp| tp == "guards::scaffolded_component_flow::SpinningCube"),
        "AddComponent must record the component in the document so \
         scene save preserves it; node has: {:?}",
        ast.component_type_paths(node),
    );
}

#[test]
fn add_marker_component_round_trips_through_ast() {
    let mut app = app_with_user_components();
    let entity = spawn_authored_entity(&mut app);

    let result = app
        .world_mut()
        .operator("component.add")
        .param("entity", entity)
        .param(
            "type_path",
            "guards::scaffolded_component_flow::PlayerSpawn".to_string(),
        )
        .call()
        .expect("dispatch resolves");
    assert_eq!(result, OperatorResult::Finished);

    app.update();

    assert!(app.world().entity(entity).contains::<PlayerSpawn>());

    let ast = app.world().resource::<SceneBsnAst>();
    let node = ast.ast_for(entity).expect("tracked");
    assert!(
        ast.component_type_paths(node)
            .iter()
            .any(|tp| tp == "guards::scaffolded_component_flow::PlayerSpawn"),
        "marker component must round-trip through the document too",
    );
}

#[test]
fn inspector_field_edit_updates_ecs_and_ast() {
    // Inspector field commits dispatch `SetBsnField`, which must mutate both the
    // document (so a save persists) and the ECS component (so play sees it).
    let mut app = app_with_user_components();
    let entity = spawn_authored_entity(&mut app);

    let result = app
        .world_mut()
        .operator("component.add")
        .param("entity", entity)
        .param(
            "type_path",
            "guards::scaffolded_component_flow::SpinningCube".to_string(),
        )
        .call()
        .expect("dispatch resolves");
    assert_eq!(result, OperatorResult::Finished);
    app.update();

    let cube = app
        .world()
        .entity(entity)
        .get::<SpinningCube>()
        .expect("SpinningCube on entity");
    assert_eq!(cube.speed, 0.0);

    let mut cmd: Box<dyn EditorCommand> = Box::new(SetBsnField {
        entity,
        type_path: "guards::scaffolded_component_flow::SpinningCube".to_string(),
        field_path: "speed".to_string(),
        old_value: Some(jackdaw_bsn::BsnValue::Float(0.0)),
        new_value: jackdaw_bsn::BsnValue::Float(1.5),
        was_derived: false,
    });
    cmd.execute(app.world_mut());
    app.update();

    let cube = app
        .world()
        .entity(entity)
        .get::<SpinningCube>()
        .expect("SpinningCube on entity");
    assert!(
        (cube.speed - 1.5).abs() < f32::EPSILON,
        "ECS field must update; got speed = {}",
        cube.speed,
    );

    let ast = app.world().resource::<SceneBsnAst>();
    let node = ast.ast_for(entity).expect("tracked");
    let value = jackdaw_bsn::get_bsn_field(
        ast,
        node,
        "guards::scaffolded_component_flow::SpinningCube",
        "speed",
    )
    .expect("document must store the edited field");
    assert!(
        matches!(value, jackdaw_bsn::BsnValue::Float(speed) if (speed - 1.5).abs() < 1e-6),
        "document field must update to 1.5",
    );
}

#[test]
fn inspector_field_edit_undoes_back_to_original() {
    // Inspector edits go through the undo stack, so undo must restore the value.
    let mut app = app_with_user_components();
    let entity = spawn_authored_entity(&mut app);

    let result = app
        .world_mut()
        .operator("component.add")
        .param("entity", entity)
        .param(
            "type_path",
            "guards::scaffolded_component_flow::SpinningCube".to_string(),
        )
        .call()
        .expect("dispatch resolves");
    assert_eq!(result, OperatorResult::Finished);
    app.update();

    let mut cmd: Box<dyn EditorCommand> = Box::new(SetBsnField {
        entity,
        type_path: "guards::scaffolded_component_flow::SpinningCube".to_string(),
        field_path: "speed".to_string(),
        old_value: Some(jackdaw_bsn::BsnValue::Float(0.0)),
        new_value: jackdaw_bsn::BsnValue::Float(1.5),
        was_derived: false,
    });
    cmd.execute(app.world_mut());
    cmd.undo(app.world_mut());
    app.update();

    let cube = app.world().entity(entity).get::<SpinningCube>().unwrap();
    assert!(
        (cube.speed - 0.0).abs() < f32::EPSILON,
        "undo must restore ECS speed to 0; got {}",
        cube.speed,
    );
}

const PROJECT_MARKER: &str = "mygame::world::PatrolPoint";
const PROJECT_STRUCT: &str = "mygame::world::Health";
const PROJECT_ENUM: &str = "mygame::world::Team";

/// A schema shaped like one the project build extracts: a field-less
/// marker, a struct, and an enum, none of which the editor has a Rust
/// type for.
fn project_schema() -> jackdaw_schema::ProjectSchema {
    let entry = |type_path: &str,
                 kind: jackdaw_schema::TypeKind,
                 fields: Vec<jackdaw_schema::FieldSchema>| {
        jackdaw_schema::TypeSchema {
            type_path: type_path.to_string(),
            short_name: type_path.rsplit("::").next().unwrap_or("").to_string(),
            module_path: String::new(),
            category: String::new(),
            description: String::new(),
            hidden: false,
            default_constructible: true,
            fields,
            kind,
            default: None,
            variants: Vec::new(),
            entity_fields: Vec::new(),
            fills_gaps: true,
        }
    };
    jackdaw_schema::ProjectSchema {
        components: vec![
            entry(PROJECT_MARKER, jackdaw_schema::TypeKind::Struct, Vec::new()),
            entry(
                PROJECT_STRUCT,
                jackdaw_schema::TypeKind::Struct,
                vec![jackdaw_schema::FieldSchema {
                    name: "current".to_string(),
                    type_path: "f32".to_string(),
                }],
            ),
            entry(PROJECT_ENUM, jackdaw_schema::TypeKind::Enum, Vec::new()),
        ],
        resources: Vec::new(),
        events: Vec::new(),
        functions: Vec::new(),
    }
}

fn app_with_project_schema() -> App {
    let mut app = util::editor_test_app();
    {
        let native = jackdaw::project_types::native_type_paths(
            &app.world().resource::<AppTypeRegistry>().read(),
        );
        app.world_mut()
            .resource_mut::<jackdaw::project_types::ProjectTypes>()
            .update(&project_schema(), &native);
    }
    jackdaw::project_types::publish_document_only_types(app.world_mut());
    app
}

fn write_scene(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).expect("scene dir");
    let path = dir.join("scene.bsn");
    std::fs::write(&path, body).expect("write scene");
    path
}

/// Project open restores every persisted tab in the same exclusive run that
/// enters the editor, so the first scene of a session loads before the schema
/// watcher has ever ticked and the types have to be on hand by then.
#[test]
fn the_first_scene_of_a_session_loads_before_any_watcher_tick() {
    let mut app = util::editor_test_app();
    let root = std::env::temp_dir().join(format!("jd_project_open_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let jackdaw_dir = root.join(".jackdaw");
    std::fs::create_dir_all(&jackdaw_dir).expect("project dir");
    std::fs::write(
        jackdaw_schema::schema_path(&jackdaw_dir),
        serde_json::to_vec(&project_schema()).expect("serialize schema"),
    )
    .expect("write schema.json");
    let path = write_scene(
        &root.join("assets"),
        &format!("bevy_ecs::hierarchy::Children [\n    #Patrol\n    {PROJECT_MARKER}\n]\n"),
    );

    let config = jackdaw::project::create_default_project(&root);
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot::new(root.clone(), config));
    assert!(
        app.world()
            .resource::<jackdaw::project_types::ProjectTypes>()
            .is_empty(),
        "no tick has run, so nothing can be known yet",
    );

    // What project open does before it restores tabs.
    jackdaw::pie::refresh_project_types(app.world_mut());
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &path);

    let unresolved = app
        .world()
        .resource::<jackdaw_bsn::UnresolvedTypes>()
        .types();
    assert!(
        unresolved.is_empty(),
        "the schema is on disk and current; nothing here needs a rebuild, \
         got unresolved: {unresolved:?}",
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// A scene that names the project's own components opens with them authored
/// rather than unknown.
#[test]
fn a_scene_naming_project_components_loads_without_unresolved_types() {
    let mut app = app_with_project_schema();
    let dir = std::env::temp_dir().join(format!("jd_project_types_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = write_scene(
        &dir,
        &format!(
            "bevy_ecs::hierarchy::Children [\n    #Patrol\n    {PROJECT_MARKER}\n    \
             {PROJECT_STRUCT} {{ current: 12.0 }}\n]\n"
        ),
    );

    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &path);
    app.update();

    let unresolved = app
        .world()
        .resource::<jackdaw_bsn::UnresolvedTypes>()
        .types();
    assert!(
        unresolved.is_empty(),
        "project components are authored, not unknown; got unresolved: {unresolved:?}",
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The document is where a project component lives, so a load followed by
/// a save has to hand back what it was given.
#[test]
fn project_components_survive_an_open_and_resave() {
    let mut app = app_with_project_schema();
    let dir = std::env::temp_dir().join(format!("jd_project_resave_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = write_scene(
        &dir,
        &format!(
            "bevy_ecs::hierarchy::Children [\n    #Patrol\n    {PROJECT_MARKER}\n    \
             {PROJECT_STRUCT} {{ current: 12.0 }}\n]\n"
        ),
    );

    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &path);
    app.update();

    // The editor has no Rust type to insert, so the document node is what every
    // editor surface reads the component off.
    let entity = app
        .world_mut()
        .query_filtered::<Entity, With<jackdaw_bsn::AstNodeRef>>()
        .iter(app.world())
        .find(|&e| {
            app.world()
                .get::<Name>(e)
                .is_some_and(|n| n.as_str() == "Patrol")
        })
        .expect("the authored entity spawned");
    let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    let node = ast.ast_for(entity).expect("entity is tracked");
    for type_path in [PROJECT_MARKER, PROJECT_STRUCT] {
        assert!(
            ast.find_patch_by_type_path(node, type_path).is_some(),
            "{type_path} must be on the loaded entity's node",
        );
    }

    let emitted = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(app.world_mut(), &dir);
    for type_path in [PROJECT_MARKER, PROJECT_STRUCT] {
        assert!(
            emitted.contains(type_path),
            "re-save dropped {type_path}; got:\n{emitted}",
        );
    }
    assert!(
        emitted.contains("12.0"),
        "re-save dropped the authored field value; got:\n{emitted}",
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A project enum is authored one variant at a time, and no schema lists
/// `Team::Red`, only `Team`. The owning type has to answer for it.
#[test]
fn an_authored_variant_of_a_project_enum_is_not_unresolved() {
    let mut app = app_with_project_schema();
    let dir = std::env::temp_dir().join(format!("jd_project_enum_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = write_scene(
        &dir,
        &format!("bevy_ecs::hierarchy::Children [\n    #Squad\n    {PROJECT_ENUM}::Red\n]\n"),
    );

    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &path);
    app.update();

    let unresolved = app
        .world()
        .resource::<jackdaw_bsn::UnresolvedTypes>()
        .types();
    assert!(
        unresolved.is_empty(),
        "a variant of a reported project enum is authored, not unknown; got: {unresolved:?}",
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A type the schema does not know either is a real gap, and stays reported.
#[test]
fn a_type_absent_from_the_schema_is_still_reported() {
    let mut app = app_with_project_schema();
    let dir = std::env::temp_dir().join(format!("jd_project_stale_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = write_scene(
        &dir,
        "bevy_ecs::hierarchy::Children [\n    #Ghost\n    mygame::world::NotBuiltYet\n]\n",
    );

    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &path);
    app.update();

    let unresolved = app
        .world()
        .resource::<jackdaw_bsn::UnresolvedTypes>()
        .types();
    assert!(
        unresolved.contains("mygame::world::NotBuiltYet"),
        "a type in neither the registry nor the schema must be reported; got: {unresolved:?}",
    );

    let _ = std::fs::remove_dir_all(&dir);
}
