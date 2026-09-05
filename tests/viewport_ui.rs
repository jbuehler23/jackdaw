mod util;

use bevy::prelude::*;
use jackdaw::{
    ui_authoring::UiAuthoring,
    ui_projection::UiProjection,
    viewport_ui::{ViewportUiHost, attach_viewport_ui},
};
use jackdaw_api::WidgetInstantiateContext;
use jackdaw_scene_types::SceneNodeId;

#[test]
fn viewport_projects_authored_ui_to_its_own_camera_and_cleans_up() {
    let mut app = util::editor_test_app();
    let canvas = UiAuthoring::instantiate(
        app.world_mut(),
        "layout.canvas",
        WidgetInstantiateContext::default(),
    )
    .unwrap();
    let button = UiAuthoring::instantiate(
        app.world_mut(),
        "feathers.button",
        WidgetInstantiateContext {
            parent: Some(canvas),
        },
    )
    .unwrap();
    let button_id = *app.world().get::<SceneNodeId>(button).unwrap();

    let camera = app.world_mut().spawn(Camera3d::default()).id();
    let host = app.world_mut().spawn_empty().id();
    attach_viewport_ui(app.world_mut(), host, camera);
    app.update();

    let state = app.world().get::<ViewportUiHost>(host).unwrap();
    assert_eq!(state.camera, camera);
    assert_eq!(state.projections.len(), 1);
    assert_eq!(state.projections[0].canvas, canvas);
    let handle = state.projections[0].handle;
    assert!(
        UiProjection::projected_entity(app.world(), handle, button_id).is_some(),
        "the 3D viewport should contain an editable projection of the UI"
    );
    let root = UiProjection::root(app.world(), handle).unwrap();

    app.world_mut().entity_mut(host).despawn();
    app.update();
    assert!(app.world().get_entity(root).is_err());
    assert!(app.world().get_entity(camera).is_ok());
}
