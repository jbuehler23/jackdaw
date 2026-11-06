//! A scene opened with its project spawns before the walk that indexes the
//! project's assets lands, and its brush faces still get their material files.

use crate::util;

use bevy::prelude::*;
use jackdaw::project::{ProjectConfig, ProjectRoot};
use jackdaw_scene_types::types::Brush;

const MATERIAL: &str = "\
// jackdaw asset bevy_pbr::pbr_material::StandardMaterial
#soil
bevy_pbr::pbr_material::StandardMaterial {
    perceptual_roughness: 0.25,
}
";

const SCENE: &str = "\
#Block
bevy_transform::components::transform::Transform
jackdaw_scene_types::types::Brush {
    faces: [
        jackdaw_geometry::BrushFaceData {
            plane: jackdaw_geometry::BrushPlane {
                normal: glam::Vec3 { x: 0.0, y: 1.0, z: 0.0 },
                distance: 1.5,
            },
            material: \"materials/soil.bsn\",
        },
    ],
}
";

#[test]
fn the_first_scene_opened_finds_the_materials_it_names() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let materials = tmp.path().join("assets/materials");
    std::fs::create_dir_all(&materials).expect("the folder is made");
    std::fs::write(materials.join("soil.bsn"), MATERIAL).expect("the material is written");
    let scene = tmp.path().join("assets/scene.bsn");
    std::fs::write(&scene, SCENE).expect("the scene is written");

    let mut app = util::editor_test_app_reading(&tmp.path().join("assets"));
    app.insert_resource(ProjectRoot::new(
        tmp.path().to_path_buf(),
        ProjectConfig::default(),
    ));

    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene);

    let mut brushes = app.world_mut().query::<&Brush>();
    let material = brushes
        .iter(app.world())
        .next()
        .expect("the brush spawned")
        .faces[0]
        .material
        .clone();
    let stored = app
        .world()
        .resource::<Assets<StandardMaterial>>()
        .get(&material)
        .expect("the face holds the file's material, not a load that cannot finish");
    assert_eq!(stored.perceptual_roughness, 0.25);
    assert!(
        material.path().is_none(),
        "no typed load of a document path, which has no loader"
    );
}
