use bevy::prelude::*;
use jackdaw::{
    ui_canvas::{UI_CANVAS_WINDOW_ID, UiCanvasPanelHost},
    ui_projection::UiProjection,
};
use jackdaw_api_internal::lifecycle::enable_extension;
use jackdaw_panels::WindowRegistry;
use jackdaw_scene_types::SceneNodeId;
use jackdaw_ui::UiCanvas;

mod util;

#[test]
fn ui_canvas_is_a_multi_instance_window_with_isolated_lifecycle() {
    let mut app = util::editor_test_app();
    let _ = enable_extension(app.world_mut(), "jackdaw.ui_editor");
    app.update();

    let build = app
        .world()
        .resource::<WindowRegistry>()
        .get(UI_CANVAS_WINDOW_ID)
        .expect("UI Canvas window registered")
        .build
        .clone();
    let canvas = app
        .world_mut()
        .spawn((Name::new("Main UI"), UiCanvas::default(), SceneNodeId(900)))
        .id();
    let host_a = app.world_mut().spawn(Node::default()).id();
    let host_b = app.world_mut().spawn(Node::default()).id();
    build(&mut ChildSpawner::new(app.world_mut(), host_a));
    build(&mut ChildSpawner::new(app.world_mut(), host_b));
    app.update();

    let invalid_visibility_parents = {
        let world = app.world_mut();
        let mut query = world.query_filtered::<(Entity, &ChildOf), With<InheritedVisibility>>();
        query
            .iter(world)
            .filter(|(_, parent)| world.get::<InheritedVisibility>(parent.parent()).is_none())
            .map(|(entity, parent)| (entity, parent.parent()))
            .collect::<Vec<_>>()
    };
    assert!(
        invalid_visibility_parents.is_empty(),
        "visibility hierarchy violations: {invalid_visibility_parents:?}"
    );

    let panel_a = *app
        .world()
        .get::<UiCanvasPanelHost>(host_a)
        .expect("first panel host");
    let panel_b = *app
        .world()
        .get::<UiCanvasPanelHost>(host_b)
        .expect("second panel host");
    assert_eq!(panel_a.canvas, Some(canvas));
    assert_eq!(panel_b.canvas, Some(canvas));
    assert_ne!(panel_a.camera, panel_b.camera);
    assert_ne!(panel_a.projection, panel_b.projection);

    let root_a = UiProjection::root(app.world(), panel_a.projection.unwrap()).unwrap();
    let root_b = UiProjection::root(app.world(), panel_b.projection.unwrap()).unwrap();
    for root in [root_a, root_b] {
        assert!(
            app.world().entity(root).contains::<InheritedVisibility>(),
            "projected UI roots participate in Bevy visibility propagation"
        );
    }
    app.world_mut().entity_mut(host_a).despawn();
    app.update();

    assert!(app.world().get_entity(panel_a.camera).is_err());
    assert!(app.world().get_entity(root_a).is_err());
    assert!(app.world().get_entity(panel_b.camera).is_ok());
    assert!(app.world().get_entity(root_b).is_ok());
}
