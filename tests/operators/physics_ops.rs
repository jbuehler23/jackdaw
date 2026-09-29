//! Enabling physics authors a static body, the way a new brush carries one,
//! and a collider with no body stands still until it is given one.

use crate::util;

use avian3d::prelude::RigidBody;
use bevy::prelude::*;
use jackdaw::entity_ops::GltfSource;
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
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
    let mut app = util::editor_test_app();
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
    let mut app = util::editor_test_app();
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
    let mut app = util::editor_test_app();

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
