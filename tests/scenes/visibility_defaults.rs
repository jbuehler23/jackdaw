//! `Visibility::Inherited` is what a load gives a placed entity with none, so a
//! save leaves it out and writes only the visibility an author chose.

use crate::util;

use bevy::prelude::*;
use jackdaw::migrate_dialog::request_open_with_conversion;

const SCENE: &str = "\
bevy_ecs::hierarchy::Children [
    #Parent
    bevy_transform::components::transform::Transform
    bevy_ecs::hierarchy::Children [
        #Kid
        bevy_transform::components::transform::Transform
        bevy_camera::visibility::Visibility::Hidden
    ]
    --
    #Shown
    bevy_transform::components::transform::Transform
    bevy_camera::visibility::Visibility::Visible
    --
    #Written
    bevy_transform::components::transform::Transform
    bevy_camera::visibility::Visibility::Inherited
]
";

fn visibility(app: &mut App, name: &str) -> Option<Visibility> {
    let mut query = app.world_mut().query::<(&Name, Option<&Visibility>)>();
    query
        .iter(app.world())
        .find(|(held, _)| held.as_str() == name)
        .unwrap_or_else(|| panic!("{name} loaded"))
        .1
        .copied()
}

fn opened() -> (App, tempfile::TempDir) {
    let mut app = util::editor_test_app();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("scene.bsn");
    std::fs::write(&path, SCENE).expect("write scene");
    request_open_with_conversion(app.world_mut(), &path);
    app.update();
    (app, dir)
}

#[test]
fn a_placed_entity_saved_without_visibility_loads_inheriting_it() {
    let (mut app, _dir) = opened();
    assert_eq!(visibility(&mut app, "Parent"), Some(Visibility::Inherited));
    assert_eq!(visibility(&mut app, "Kid"), Some(Visibility::Hidden));
    assert_eq!(visibility(&mut app, "Shown"), Some(Visibility::Visible));
    assert_eq!(visibility(&mut app, "Written"), Some(Visibility::Inherited));
}

#[test]
fn a_save_writes_only_the_visibility_an_author_chose() {
    let (mut app, dir) = opened();
    let added = app
        .world_mut()
        .spawn((
            Name::new("Added"),
            Transform::default(),
            Visibility::Inherited,
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), added);

    let text = jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), dir.path())
        .expect("the scene emits");

    assert!(
        !text.contains("Visibility::Inherited"),
        "the save wrote the default visibility:\n{text}"
    );
    assert!(text.contains("Visibility::Hidden"), "{text}");
    assert!(text.contains("Visibility::Visible"), "{text}");
}
