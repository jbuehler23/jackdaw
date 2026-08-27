//! Legacy `.jsn` conversion is committed to disk only once the converted
//! document has been read back and accepted.
//!
//! Writing the `.bsn` sibling and renaming the original to `.jsn.bak` before
//! anything has looked at the result would leave a user whose document the
//! editor then refuses with a converted file they never asked for and no
//! `.jsn` to go back to.

use bevy::prelude::*;
use jackdaw::jsn_to_bsn::{commit_conversion, convert_scene_file_pending};

const LEGACY: &str = r#"{
  "jsn": {"format_version": [3, 0, 0], "editor_version": "0.1.0", "bevy_version": "0.19"},
  "metadata": {"name": "", "description": "", "author": "", "created": "", "modified": ""},
  "assets": {},
  "scene": [
    {"id": 1, "components": {"bevy_ecs::name::Name": "Hero"}}
  ]
}"#;

#[test]
fn converting_a_legacy_scene_leaves_the_disk_alone_until_it_is_committed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let jsn_path = dir.path().join("scene.jsn");
    std::fs::write(&jsn_path, LEGACY).unwrap();
    let bsn_path = dir.path().join("scene.bsn");
    let backup_path = dir.path().join("scene.jsn.bak");

    let mut app = headless_app();
    let pending =
        convert_scene_file_pending(app.world_mut(), &jsn_path).expect("the legacy scene converts");

    assert!(
        pending.scene_bsn.contains("Hero"),
        "the conversion produced the document in memory: {}",
        pending.scene_bsn
    );
    assert!(
        !bsn_path.exists(),
        "nothing is written until the converted document has been accepted"
    );
    assert!(
        jsn_path.exists() && !backup_path.exists(),
        "the original stays where the user left it until then"
    );

    commit_conversion(app.world_mut(), pending).expect("the commit writes");

    assert!(bsn_path.exists(), "the accepted document reaches disk");
    assert!(
        backup_path.exists() && !jsn_path.exists(),
        "and the original is kept as the backup"
    );
}

fn headless_app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()));
    app.add_plugins(jackdaw_scene_types::SceneTypesPlugin {
        runtime_mesh_rebuild: false,
    });
    app.add_plugins(jackdaw_bsn::JackdawBsnPlugin);
    app
}
