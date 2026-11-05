//! A game reads scene text in its compact form, in the form older builds wrote,
//! and in the binary twin, through its own asset loader.

use std::time::Duration;

use bevy::prelude::*;
use jackdaw_runtime::{JackdawPlugin, JackdawScene, JackdawSceneRoot};
use jackdaw_scene_types::Brush;

const COMPACT: &str = "\
#Block
jackdaw_scene_types::types::Brush {
faces: [
jackdaw_geometry::BrushFaceData { plane: jackdaw_geometry::BrushPlane { normal: glam::Vec3 { y: 0.1 }, distance: 1.5 } },
jackdaw_geometry::BrushFaceData,
],
}
";

const OLDER: &str = "\
#Block
jackdaw_scene_types::types::Brush {
    faces: [
        jackdaw_geometry::BrushFaceData {
            plane: jackdaw_geometry::BrushPlane {
                normal: glam::Vec3 {
                    x: 0.0,
                    y: 0.10000000149011612,
                    z: 0.0,
                },
                distance: 1.5,
            },
            uv_offset: glam::Vec2 {
                x: 0.0,
                y: 0.0,
            },
        },
        jackdaw_geometry::BrushFaceData {
            uv_rotation: 0.0,
        },
    ],
}
";

fn game(dir: &std::path::Path) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(bevy::asset::AssetPlugin {
        file_path: dir.to_string_lossy().into_owned(),
        ..Default::default()
    });
    app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    app.add_plugins(JackdawPlugin);
    app
}

/// The faces of every brush spawned once `roots` scene roots at `file` have
/// spawned.
fn loaded_brushes(file: &str, contents: &[u8], roots: usize) -> Vec<Vec<(Vec3, f32)>> {
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::write(dir.path().join(file), contents).expect("the scene");
    let mut app = game(dir.path());
    let handle: Handle<JackdawScene> = app.world().resource::<AssetServer>().load(file.to_string());
    for _ in 0..roots {
        app.world_mut().spawn(JackdawSceneRoot(handle.clone()));
    }
    for _ in 0..500 {
        app.update();
        std::thread::sleep(Duration::from_millis(2));
        let mut brushes = app.world_mut().query::<&Brush>();
        let found: Vec<Vec<(Vec3, f32)>> = brushes
            .iter(app.world())
            .map(|brush| {
                brush
                    .faces
                    .iter()
                    .map(|face| (face.plane.normal, face.plane.distance))
                    .collect()
            })
            .collect();
        if found.len() >= roots {
            return found;
        }
    }
    panic!("{file} did not spawn {roots} brushes");
}

fn assert_expected(faces: &[(Vec3, f32)]) {
    assert_eq!(faces.len(), 2);
    assert_eq!(faces[0].0.y.to_bits(), 0.1_f32.to_bits());
    assert_eq!(faces[0].0.x.to_bits(), 0.0_f32.to_bits());
    assert_eq!(faces[0].1, 1.5);
    assert_eq!(faces[1], (Vec3::ZERO, 0.0));
}

#[test]
fn compact_text_spawns_through_the_loader() {
    let brushes = loaded_brushes("scene.bsn", COMPACT.as_bytes(), 1);
    assert_expected(&brushes[0]);
}

#[test]
fn older_text_spawns_the_same_through_the_loader() {
    let brushes = loaded_brushes("scene.bsn", OLDER.as_bytes(), 1);
    assert_expected(&brushes[0]);
}

#[test]
fn a_binary_scene_spawns_through_the_loader() {
    let ast = jackdaw_bsn::parse_bsn_text(COMPACT).expect("the text parses");
    let bytes = jackdaw_bsn::binary::encode(&ast, None);
    let brushes = loaded_brushes("scene.bsb", &bytes, 1);
    assert_expected(&brushes[0]);
}

#[test]
fn every_root_of_one_scene_gets_its_own_spawn() {
    let brushes = loaded_brushes("scene.bsn", COMPACT.as_bytes(), 2);
    assert_eq!(brushes.len(), 2);
    assert_expected(&brushes[0]);
    assert_expected(&brushes[1]);

    let ast = jackdaw_bsn::parse_bsn_text(COMPACT).expect("the text parses");
    let bytes = jackdaw_bsn::binary::encode(&ast, None);
    let brushes = loaded_brushes("scene.bsb", &bytes, 2);
    assert_eq!(brushes.len(), 2);
    assert_expected(&brushes[1]);
}

#[test]
fn a_binary_scene_stamped_by_an_older_bevy_spawns_migrated() {
    let body = "\
#Panel
bevy_ui::ui_node::Node { border_radius: bevy_ui::ui_node::BorderRadius { top_left: bevy_ui::geometry::Val::Px(6.0) } }
";
    let ast = jackdaw_bsn::parse_bsn_text(body).expect("the text parses");
    let bytes = jackdaw_bsn::binary::encode(&ast, Some("// jackdaw 0.19.0 | bevy 0.19\n"));

    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::write(dir.path().join("old.bsb"), bytes).expect("the scene");
    let mut app = game(dir.path());
    app.register_type::<Node>();
    let handle: Handle<JackdawScene> = app.world().resource::<AssetServer>().load("old.bsb");
    app.world_mut().spawn(JackdawSceneRoot(handle));
    for _ in 0..500 {
        app.update();
        std::thread::sleep(Duration::from_millis(2));
        let mut nodes = app.world_mut().query_filtered::<&Node, With<Name>>();
        if let Some(node) = nodes.iter(app.world()).next() {
            assert_eq!(node.border_radius.top_left.x, Val::Px(6.0));
            return;
        }
    }
    panic!("old.bsb did not spawn its node");
}
