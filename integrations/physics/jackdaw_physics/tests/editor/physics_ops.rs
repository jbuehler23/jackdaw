//! Enabling physics authors a static body, the way a new brush carries one,
//! a collider with no body stands still until it is given one, and disabling
//! physics takes off everything physical the entity authored, as one undo
//! step either way.

use crate::util;

use avian3d::prelude::{ColliderConstructor, Position, RigidBody};
use bevy::prelude::*;
use jackdaw::commands::CommandHistory;
use jackdaw::entity_ops::GltfSource;
use jackdaw::selection::{Selected, Selection};
use jackdaw_api::prelude::*;
use jackdaw_avian_integration::{AvianCollider, ImpliedStaticBody};
use jackdaw_bsn::SceneBsnAst;
use jackdaw_scene_types::Brush;

const RIGID_BODY: &str = "avian3d::dynamics::rigid_body::RigidBody";

fn call(app: &mut App, id: &'static str, entity: Option<Entity>) {
    let mut call = app.world_mut().operator(id).settings(CallOperatorSettings {
        execution_context: ExecutionContext::Invoke,
        creates_history_entry: true,
    });
    if let Some(entity) = entity {
        call = call.param("entity", entity);
    }
    let result = call
        .call()
        .unwrap_or_else(|err| panic!("{id}: dispatch errored: {err}"));
    assert_eq!(result, OperatorResult::Finished, "{id} reported {result:?}");
    for _ in 0..3 {
        app.update();
    }
}

fn placed_model(app: &mut App) -> Entity {
    let model = app
        .world_mut()
        .spawn((
            Name::new("Rock"),
            Transform::default(),
            GltfSource {
                path: "rock.gltf".to_string(),
                scene_index: 0,
            },
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), model);
    app.world_mut().resource_mut::<Selection>().entities = vec![model];
    app.update();
    model
}

fn authors_body(app: &App, entity: Entity) -> bool {
    let doc = app.world().resource::<SceneBsnAst>();
    doc.ast_for(entity)
        .is_some_and(|node| doc.find_patch_by_type_path(node, RIGID_BODY).is_some())
}

#[test]
fn enabling_physics_on_a_placed_model_authors_a_static_body() {
    let mut app = util::editor_app();
    let model = placed_model(&mut app);

    call(&mut app, "physics.enable", Some(model));

    assert_eq!(
        app.world().get::<RigidBody>(model),
        Some(&RigidBody::Static)
    );
    assert!(app.world().get::<AvianCollider>(model).is_some());
    assert!(
        authors_body(&app, model),
        "the body is saved with the scene"
    );
    assert!(app.world().get::<ImpliedStaticBody>(model).is_none());
}

#[test]
fn a_collider_added_alone_stands_as_a_static_body_that_is_not_saved() {
    let mut app = util::editor_app();
    let model = placed_model(&mut app);

    app.world_mut()
        .entity_mut(model)
        .insert(AvianCollider::default());
    for _ in 0..3 {
        app.update();
    }

    assert_eq!(
        app.world().get::<RigidBody>(model),
        Some(&RigidBody::Static)
    );
    assert!(!authors_body(&app, model));
}

#[test]
fn a_new_brush_keeps_its_authored_static_body() {
    let mut app = util::editor_app();

    call(&mut app, "entity.add.cube", None);

    let brush = app
        .world_mut()
        .query_filtered::<Entity, With<Brush>>()
        .single(app.world())
        .expect("one brush");
    assert_eq!(
        app.world().get::<RigidBody>(brush),
        Some(&RigidBody::Static)
    );
    assert!(authors_body(&app, brush));
    assert!(app.world().get::<ImpliedStaticBody>(brush).is_none());
}

/// A named entity in the scene document, made the primary selection.
fn spawn_authored_selected_target(app: &mut App) -> Entity {
    let entity = app.world_mut().spawn(Name::new("op-param-target")).id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    app.world_mut().resource_mut::<Selection>().entities = vec![entity];
    app.update();
    entity
}

#[test]
fn enabling_physics_is_one_undo_step() {
    let mut app = util::editor_app();
    let entity = spawn_authored_selected_target(&mut app);
    let undo_depth = app.world().resource::<CommandHistory>().undo_stack.len();

    call(&mut app, "physics.enable", Some(entity));

    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        undo_depth + 1,
        "the body and the collider arrive as one undo step"
    );
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    app.update();
    let entity_ref = app.world().entity(entity);
    assert!(
        !entity_ref.contains::<RigidBody>(),
        "undo takes the body off"
    );
    assert!(
        !entity_ref.contains::<AvianCollider>(),
        "and the collider with it"
    );
}

#[test]
fn physics_enable_with_entity_param_attaches_components() {
    let mut app = util::editor_app();
    let entity = spawn_authored_selected_target(&mut app);

    let result = app
        .world_mut()
        .operator("physics.enable")
        .param("entity", entity)
        .call()
        .expect("dispatch resolves");
    assert_eq!(result, OperatorResult::Finished);

    app.update();
    let entity_ref = app.world().entity(entity);
    assert!(
        entity_ref.contains::<RigidBody>(),
        "physics.enable should attach RigidBody"
    );
    assert!(
        entity_ref.contains::<AvianCollider>(),
        "physics.enable should attach AvianCollider"
    );
}

#[test]
fn physics_disable_with_entity_param_detaches_components() {
    let mut app = util::editor_app();
    let entity = spawn_authored_selected_target(&mut app);
    call(&mut app, "physics.enable", Some(entity));
    app.world_mut()
        .entity_mut(entity)
        .insert(AvianCollider(ColliderConstructor::Cuboid {
            x_length: 1.0,
            y_length: 1.0,
            z_length: 1.0,
        }));
    let undo_depth = app.world().resource::<CommandHistory>().undo_stack.len();

    call(&mut app, "physics.disable", Some(entity));

    let entity_ref = app.world().entity(entity);
    assert!(
        !entity_ref.contains::<RigidBody>(),
        "physics.disable should remove RigidBody"
    );
    assert!(
        !entity_ref.contains::<AvianCollider>(),
        "physics.disable should remove AvianCollider"
    );
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        undo_depth + 1,
        "the disable is one undo step"
    );

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    app.update();
    let entity_ref = app.world().entity(entity);
    assert!(
        entity_ref.contains::<RigidBody>(),
        "undo puts the body back"
    );
    assert!(entity_ref.contains::<AvianCollider>(), "and the collider");
}

#[test]
fn add_cube_authors_static_physics_by_default() {
    let mut app = util::editor_app();
    let result = app
        .world_mut()
        .operator("entity.add.cube")
        .call()
        .expect("dispatch resolves");
    assert_eq!(result, OperatorResult::Finished);
    app.update();

    let entity = app
        .world_mut()
        .query_filtered::<Entity, With<Brush>>()
        .iter(app.world())
        .next()
        .expect("entity.add.cube spawned a brush");
    let entity_ref = app.world().entity(entity);
    assert_eq!(
        entity_ref.get::<RigidBody>().copied(),
        Some(RigidBody::Static),
        "new brushes should spawn as static bodies"
    );
    assert!(
        entity_ref.contains::<AvianCollider>(),
        "new brushes should spawn with AvianCollider"
    );

    let ast = app.world().resource::<SceneBsnAst>();
    let node = ast
        .ast_for(entity)
        .expect("cube is tracked in the document");
    let paths = ast.component_type_paths(node);
    assert!(
        paths
            .iter()
            .any(|p| p.starts_with("avian3d::dynamics::rigid_body::RigidBody")),
        "RigidBody should be authored, not only live ECS: {paths:?}"
    );
    assert!(
        paths
            .iter()
            .any(|p| p == "jackdaw_avian_integration::AvianCollider"),
        "AvianCollider should be authored, not only live ECS: {paths:?}"
    );
    assert!(
        !paths.iter().any(|p| p.ends_with("::Position")),
        "avian require companions must stay off the document: {paths:?}"
    );
}

#[test]
fn component_remove_drops_authored_physics_from_the_document() {
    let mut app = util::editor_app();
    let result = app
        .world_mut()
        .operator("entity.add.cube")
        .call()
        .expect("dispatch resolves");
    assert_eq!(result, OperatorResult::Finished);
    app.update();

    let entity = app
        .world_mut()
        .query_filtered::<Entity, With<Brush>>()
        .iter(app.world())
        .next()
        .expect("entity.add.cube spawned a brush");
    app.world_mut().resource_mut::<Selection>().entities = vec![entity];
    app.update();

    for type_path in [
        "avian3d::dynamics::rigid_body::RigidBody",
        "jackdaw_avian_integration::AvianCollider",
    ] {
        let result = app
            .world_mut()
            .operator("component.remove")
            .param("entity", entity)
            .param("type_path", type_path.to_string())
            .call()
            .expect("dispatch resolves");
        assert_eq!(result, OperatorResult::Finished);
        app.update();
    }

    let entity_ref = app.world().entity(entity);
    assert!(
        !entity_ref.contains::<RigidBody>(),
        "component.remove should take RigidBody off the brush"
    );
    assert!(
        !entity_ref.contains::<AvianCollider>(),
        "component.remove should take AvianCollider off the brush"
    );
    assert!(
        !entity_ref.contains::<avian3d::prelude::Position>(),
        "component.remove should take RigidBody's #[require] companions off the brush"
    );
    assert!(
        !entity_ref.contains::<avian3d::prelude::Rotation>(),
        "component.remove should take RigidBody's #[require] companions off the brush"
    );
    assert!(
        !entity_ref.contains::<avian3d::prelude::Collider>(),
        "component.remove should take the built Collider off the brush"
    );
    assert!(
        !entity_ref.contains::<avian3d::collider_tree::ColliderTreeProxyKey>(),
        "unreflected collider requires must leave with AvianCollider"
    );

    let ast = app.world().resource::<SceneBsnAst>();
    let node = ast
        .ast_for(entity)
        .expect("cube is tracked in the document");
    let paths = ast.component_type_paths(node);
    assert!(
        !paths
            .iter()
            .any(|p| p.starts_with("avian3d::dynamics::rigid_body::RigidBody")),
        "RigidBody should leave the document: {paths:?}"
    );
    assert!(
        !paths
            .iter()
            .any(|p| p == "jackdaw_avian_integration::AvianCollider"),
        "AvianCollider should leave the document: {paths:?}"
    );
}

#[test]
fn component_remove_keeps_entity_identity_and_undo_restores_requires() {
    let mut app = util::editor_app();
    let result = app
        .world_mut()
        .operator("entity.add.cube")
        .call()
        .expect("dispatch resolves");
    assert_eq!(result, OperatorResult::Finished);
    app.update();

    let entity = app
        .world_mut()
        .query_filtered::<Entity, With<Brush>>()
        .iter(app.world())
        .next()
        .expect("entity.add.cube spawned a brush");
    app.world_mut().entity_mut(entity).insert(Selected);
    app.world_mut().resource_mut::<Selection>().entities = vec![entity];
    app.update();

    assert!(
        app.world().entity(entity).contains::<Position>(),
        "precondition: RigidBody's require companion is live"
    );

    let result = app
        .world_mut()
        .operator("component.remove")
        .param("entity", entity)
        .param(
            "type_path",
            "avian3d::dynamics::rigid_body::RigidBody".to_string(),
        )
        .call()
        .expect("dispatch resolves");
    assert_eq!(result, OperatorResult::Finished);
    app.update();

    let entity_ref = app.world().entity(entity);
    assert!(
        entity_ref.contains::<ImpliedStaticBody>(),
        "the authored RigidBody should leave ECS, leaving the collider an implied static body"
    );
    assert!(
        entity_ref.contains::<AvianCollider>(),
        "AvianCollider was not removed and should survive"
    );
    assert!(
        entity_ref.contains::<Selected>(),
        "resync must keep the same entity selected"
    );
    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![entity],
        "selection resource must still name the same entity"
    );

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    app.update();

    let entity_ref = app.world().entity(entity);
    assert!(
        entity_ref.contains::<RigidBody>(),
        "undo should put RigidBody back"
    );
    assert!(
        entity_ref.contains::<Position>(),
        "undo resync should restore RigidBody's require companions"
    );
    assert!(
        !entity_ref.contains::<ImpliedStaticBody>(),
        "undo puts back the authored body in place of the implied one"
    );
    assert!(
        entity_ref.contains::<Selected>(),
        "undo resync must keep selection"
    );
}
