//! Opening a second scene over a first one.
//!
//! The File > Open dialog, a row in Open Recent and `scene.open path=` from a
//! script all land on the same `request_open_with_conversion` entry, so one
//! test covers them.

use crate::util;

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

/// A scene whose nodes, nested ones too, carry no node ids.
const UNNAMED_NESTED_SCENE: &str = "\
bevy_ecs::hierarchy::Children [
    #Root
    bevy_transform::components::transform::Transform
    bevy_ecs::hierarchy::Children [
        #Camera
        bevy_transform::components::transform::Transform
        --
        #Sun
        bevy_transform::components::transform::Transform
    ]
    --
    #Ground
    bevy_transform::components::transform::Transform
]
";

/// Every node of `text` by name, with its node id.
fn saved_ids(text: &str) -> std::collections::BTreeMap<String, Option<u64>> {
    let ast = jackdaw_bsn::parse_bsn_text(text).expect("the saved scene parses");
    let mut ids = std::collections::BTreeMap::new();
    let mut stack = ast.roots.clone();
    while let Some(node) = stack.pop() {
        let name = ast.get_name(node).unwrap_or_default().to_string();
        ids.insert(name, ast.stable_id_of(node));
        stack.extend(ast.get_children_ast(node));
    }
    ids
}

/// Every named scene entity, with its node id.
fn entity_ids(app: &mut App) -> std::collections::BTreeMap<String, Option<u64>> {
    let mut query = app
        .world_mut()
        .query::<(&Name, Option<&jackdaw_scene_types::SceneNodeId>)>();
    query
        .iter(app.world())
        .filter(|(name, _)| ["Root", "Camera", "Sun", "Ground"].contains(&name.as_str()))
        .map(|(name, id)| (name.to_string(), id.map(|id| id.0)))
        .collect()
}

fn save(app: &mut App, path: &std::path::Path) -> String {
    use crate::util::OperatorResultExt as _;
    use jackdaw_api::prelude::*;
    app.world_mut()
        .operator("scene.save")
        .param("path", path.to_string_lossy().into_owned())
        .call()
        .expect("scene.save dispatches")
        .assert_finished();
    app.update();
    std::fs::read_to_string(path).expect("the saved scene")
}

/// Opening a scene names every node it owns, and the save writes the ids the
/// editor holds, the same ones on every later save and open.
#[test]
fn opening_and_saving_names_every_node_once() {
    let mut app = util::editor_test_app();
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("nested.bsn");
    std::fs::write(&path, UNNAMED_NESTED_SCENE).expect("write scene");

    request_open_with_conversion(app.world_mut(), &path);
    app.update();
    let held = entity_ids(&mut app);
    assert_eq!(held.len(), 4, "every node spawned: {held:?}");
    assert!(held.values().all(Option::is_some), "{held:?}");

    let first = saved_ids(&save(&mut app, &path));
    assert_eq!(first, held, "the save writes the ids the editor holds");
    let unique: std::collections::HashSet<_> = first.values().collect();
    assert_eq!(unique.len(), first.len(), "no two nodes share an id");

    assert_eq!(saved_ids(&save(&mut app, &path)), first);

    let mut reopened = util::editor_test_app();
    request_open_with_conversion(reopened.world_mut(), &path);
    reopened.update();
    assert_eq!(
        entity_ids(&mut reopened),
        first,
        "a reopen keeps the saved ids"
    );
}

/// A named asset root spawns no entity, so it gets no node id.
#[test]
fn opening_a_scene_leaves_its_asset_roots_unnamed() {
    let mut app = util::editor_test_app();
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("assets.bsn");
    std::fs::write(
        &path,
        "\
bevy_ecs::hierarchy::Children [
    #Stone
    bevy_pbr::pbr_material::StandardMaterial
    --
    #Ground
    bevy_transform::components::transform::Transform
]
",
    )
    .expect("write scene");

    request_open_with_conversion(app.world_mut(), &path);
    app.update();

    let saved = save(&mut app, &path);
    let ids = saved_ids(&saved);
    assert!(ids["Ground"].is_some(), "{saved}");
    assert_eq!(ids["Stone"], None, "{saved}");
}

/// A scene whose Transforms are bare, or hold only a translation in the
/// older multi-line form.
const SPARSE_TRANSFORM_SCENE: &str = "\
bevy_ecs::hierarchy::Children [
    #Root
    bevy_transform::components::transform::Transform
    bevy_ecs::hierarchy::Children [
        #Camera
        bevy_transform::components::transform::Transform {
            translation: glam::Vec3 {
                x: 17.0,
                y: 13.0,
                z: 19.5,
            },
        }
    ]
]
";

/// Saving an opened scene with no edit keeps a sparse Transform sparse, and
/// the next save is byte-identical.
#[test]
fn saving_with_no_edit_keeps_sparse_transforms_sparse() {
    let mut app = util::editor_test_app();
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("sparse.bsn");
    std::fs::write(&path, SPARSE_TRANSFORM_SCENE).expect("write scene");

    request_open_with_conversion(app.world_mut(), &path);
    app.update();

    let first = save(&mut app, &path);
    assert!(
        first.contains("translation: glam::Vec3 { x: 17.0, y: 13.0, z: 19.5 }"),
        "{first}"
    );
    for expanded in ["scale", "rotation", "x: 0.0"] {
        assert!(!first.contains(expanded), "{expanded} appeared:\n{first}");
    }
    assert_eq!(save(&mut app, &path), first);
}
