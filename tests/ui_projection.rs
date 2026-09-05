use bevy::{feathers::controls::ButtonVariant, prelude::*, ui::UiTargetCamera};
use jackdaw::ui_projection::{UiProjection, UiProjectionSpec};
use jackdaw_scene_types::SceneNodeId;
use jackdaw_ui::{JackdawUiPlugin, UiButton, UiCanvas};

#[test]
fn one_canvas_projects_into_two_independent_targets() {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        JackdawUiPlugin::marked_only(),
    ));

    let canvas = app
        .world_mut()
        .spawn((UiCanvas::default(), SceneNodeId(100)))
        .id();
    let authored_button = app
        .world_mut()
        .spawn((
            UiButton {
                label: "Create".into(),
                variant: ButtonVariant::Primary,
                disabled: false,
            },
            SceneNodeId(101),
            ChildOf(canvas),
        ))
        .id();
    app.update();

    let camera_a = app.world_mut().spawn(Camera2d).id();
    let camera_b = app.world_mut().spawn(Camera2d).id();
    let projection_a = UiProjection::open(
        app.world_mut(),
        UiProjectionSpec {
            canvas,
            target_camera: camera_a,
        },
    )
    .unwrap();
    let projection_b = UiProjection::open(
        app.world_mut(),
        UiProjectionSpec {
            canvas,
            target_camera: camera_b,
        },
    )
    .unwrap();

    let root_a = UiProjection::root(app.world(), projection_a).unwrap();
    let root_b = UiProjection::root(app.world(), projection_b).unwrap();
    assert_ne!(root_a, root_b);
    assert_eq!(
        app.world().get::<UiTargetCamera>(root_a),
        Some(&UiTargetCamera(camera_a))
    );
    assert_eq!(
        app.world().get::<UiTargetCamera>(root_b),
        Some(&UiTargetCamera(camera_b))
    );

    let projected_a =
        UiProjection::projected_entity(app.world(), projection_a, SceneNodeId(101)).unwrap();
    let projected_b =
        UiProjection::projected_entity(app.world(), projection_b, SceneNodeId(101)).unwrap();
    assert_ne!(projected_a, projected_b);
    assert_eq!(
        UiProjection::authored_node(app.world(), projected_a),
        Some(SceneNodeId(101))
    );

    app.world_mut()
        .get_mut::<UiButton>(authored_button)
        .unwrap()
        .label = "Save".into();
    app.update();
    UiProjection::refresh(app.world_mut(), projection_a).unwrap();
    UiProjection::refresh(app.world_mut(), projection_b).unwrap();

    for projection in [projection_a, projection_b] {
        let button =
            UiProjection::projected_entity(app.world(), projection, SceneNodeId(101)).unwrap();
        assert_eq!(
            app.world().get::<UiButton>(button).unwrap().label,
            "Save",
            "refresh must copy the latest authored state"
        );
    }
    let refreshed_projected_b =
        UiProjection::projected_entity(app.world(), projection_b, SceneNodeId(101)).unwrap();

    let surviving_root = UiProjection::root(app.world(), projection_b).unwrap();
    assert!(UiProjection::close(app.world_mut(), projection_a));
    assert!(app.world().get_entity(root_a).is_err());
    assert!(app.world().get_entity(surviving_root).is_ok());
    assert_eq!(
        UiProjection::authored_node(app.world(), refreshed_projected_b),
        Some(SceneNodeId(101))
    );
}
