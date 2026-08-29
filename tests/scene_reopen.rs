//! Opening a second scene over a first one.
//!
//! The File > Open dialog, a row in Open Recent and `scene.open path=` from a
//! script all land on the same `request_open_with_conversion` entry, so one
//! test covers them.

mod util;

use bevy::prelude::*;
use jackdaw::migrate_dialog::request_open_with_conversion;

/// A one-entity scene whose only distinguishing mark is the entity's name.
fn write_scene(dir: &std::path::Path, file: &str, name: &str) -> std::path::PathBuf {
    let path = dir.join(file);
    std::fs::write(
        &path,
        format!(
            "// jackdaw 0.19.0 | bevy 0.19\n\
             bevy_ecs::hierarchy::Children [\n    \
             #{name}\n    \
             bevy_transform::components::transform::Transform\n    \
             bevy_camera::visibility::Visibility::Inherited\n]\n"
        ),
    )
    .expect("write scene");
    path
}

fn holds(app: &mut App, name: &str) -> bool {
    let mut query = app.world_mut().query::<&Name>();
    query.iter(app.world()).any(|held| held.as_str() == name)
}

/// A second open replaces the first scene rather than wedging the editor.
///
/// The loader clears the world and spawns a document, over a world it has
/// already filled. A frame is ticked after each open, so this fails on a load
/// that never finishes as well as on one that finishes wrong.
#[test]
fn a_second_open_replaces_the_first_scene() {
    let mut app = util::editor_test_app();
    let tmp = tempfile::tempdir().expect("tempdir");
    let first = write_scene(tmp.path(), "first.bsn", "Alpha");
    let second = write_scene(tmp.path(), "second.bsn", "Beta");

    request_open_with_conversion(app.world_mut(), &first);
    app.update();
    assert!(holds(&mut app, "Alpha"), "the first scene must load");

    request_open_with_conversion(app.world_mut(), &second);
    app.update();

    assert!(holds(&mut app, "Beta"), "the second scene must load");
    assert!(
        !holds(&mut app, "Alpha"),
        "the first scene must be gone once the second is open",
    );
}
