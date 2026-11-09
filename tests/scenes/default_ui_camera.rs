//! A scene may mark its own camera as the default UI camera. The editor keeps
//! that mark in the document and keeps drawing its own UI to the shell camera.

use crate::util;

use bevy::prelude::*;
use bevy::ui::{ComputedUiTargetCamera, IsDefaultUiCamera};
use jackdaw::EditorEntity;
use jackdaw::migrate_dialog::request_open_with_conversion;

const SCENE: &str = "\
#GameCamera
bevy_transform::components::transform::Transform
bevy_camera::components::Camera2d
bevy_camera::camera::Camera { order: 5 }
bevy_ui::ui_node::IsDefaultUiCamera
";

fn shell_camera(app: &mut App) -> Entity {
    let existing = app
        .world_mut()
        .query_filtered::<Entity, (With<Camera2d>, With<IsDefaultUiCamera>, With<EditorEntity>)>()
        .iter(app.world())
        .next();
    existing.unwrap_or_else(|| {
        app.world_mut()
            .spawn((Camera2d, IsDefaultUiCamera, EditorEntity))
            .id()
    })
}

#[test]
fn a_scene_marking_its_camera_leaves_the_editor_ui_on_the_shell_camera() {
    let mut app = util::editor_test_app();
    let shell = shell_camera(&mut app);
    app.update();

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("scene.bsn");
    std::fs::write(&path, SCENE).expect("write scene");
    request_open_with_conversion(app.world_mut(), &path);
    app.update();

    let game_camera = app
        .world_mut()
        .query::<(Entity, &Name)>()
        .iter(app.world())
        .find(|(_, name)| name.as_str() == "GameCamera")
        .map(|(entity, _)| entity)
        .expect("the scene's camera spawned");
    assert_eq!(
        app.world()
            .get::<Camera>(game_camera)
            .map(|camera| camera.order),
        Some(5),
        "the scene's camera outranks the shell camera in a scan"
    );

    let editor_ui = app.world_mut().spawn((EditorEntity, Node::default())).id();
    for _ in 0..3 {
        app.update();
    }

    assert_eq!(
        app.world()
            .get::<ComputedUiTargetCamera>(editor_ui)
            .and_then(ComputedUiTargetCamera::get),
        Some(shell),
        "the editor's UI draws to the shell camera"
    );

    let text = jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), dir.path())
        .expect("the scene emits");
    assert!(
        text.contains("IsDefaultUiCamera"),
        "the save keeps the scene's mark:\n{text}"
    );
}
