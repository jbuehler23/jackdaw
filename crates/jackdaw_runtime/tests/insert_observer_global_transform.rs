//! `On<Insert, UserType>` observers see the entity's final
//! `GlobalTransform` during JSN scene load, not the
//! pre-propagation identity. Verified against an actual
//! `JackdawScene` load.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use bevy::reflect::TypePath;
use jackdaw_jsn::format::{JsnAssets, JsnEntity, JsnHeader, JsnMetadata, JsnScene};
use jackdaw_runtime::{JackdawPlugin, JackdawScene, JackdawSceneRoot};
use serde_json::json;

/// User-style component the test injects into the scene. Has a
/// field so the JSN deserializer treats it as a struct, not a
/// unit type.
#[derive(Component, Reflect, Clone, Copy, Default)]
#[reflect(Component, Default)]
struct PlayerSpawn {
    pub variant: u32,
}

/// Captures `GlobalTransform.translation` values from the
/// `On<Insert, PlayerSpawn>` observer.
#[derive(Resource, Default, Clone)]
struct InsertObservation(Arc<Mutex<Vec<Vec3>>>);

#[test]
fn on_insert_observer_sees_propagated_global_transform() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(bevy::asset::AssetPlugin::default());
    app.add_plugins(bevy::scene::ScenePlugin);
    app.add_plugins(JackdawPlugin);
    app.register_type::<PlayerSpawn>();

    let observation = InsertObservation::default();
    app.insert_resource(observation.clone());
    app.add_observer(
        |trigger: On<Insert, PlayerSpawn>,
         transforms: Query<&GlobalTransform>,
         observation: Res<InsertObservation>| {
            if let Ok(gt) = transforms.get(trigger.entity)
                && let Ok(mut log) = observation.0.lock()
            {
                log.push(gt.translation());
            }
        },
    );

    // Parent at (10, 0, 0), child at local (0, 5, 0) carrying
    // PlayerSpawn. Final world translation for the child is
    // (10, 5, 0); without the fix it would be (0, 0, 0).
    let scene = JsnScene {
        jsn: JsnHeader::default(),
        metadata: JsnMetadata::default(),
        editor: None,
        assets: JsnAssets::default(),
        scene: vec![
            JsnEntity {
                parent: None,
                components: [(
                    "bevy_transform::components::transform::Transform".to_string(),
                    json!({
                        "translation": [10.0, 0.0, 0.0],
                        "rotation": [0.0, 0.0, 0.0, 1.0],
                        "scale": [1.0, 1.0, 1.0],
                    }),
                )]
                .into_iter()
                .collect(),
            },
            JsnEntity {
                parent: Some(0),
                components: [
                    (
                        "bevy_transform::components::transform::Transform".to_string(),
                        json!({
                            "translation": [0.0, 5.0, 0.0],
                            "rotation": [0.0, 0.0, 0.0, 1.0],
                            "scale": [1.0, 1.0, 1.0],
                        }),
                    ),
                    (
                        <PlayerSpawn as TypePath>::type_path().to_string(),
                        json!({ "variant": 0 }),
                    ),
                ]
                .into_iter()
                .collect(),
            },
        ],
    };

    let scene_handle = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(scene, PathBuf::new()));

    app.world_mut().spawn(JackdawSceneRoot(scene_handle));

    // First update runs `spawn_loaded_scenes`; second covers any
    // normal-PostUpdate propagation.
    app.update();
    app.update();

    let log = observation.0.lock().unwrap();
    assert_eq!(log.len(), 1, "expected one PlayerSpawn insert observation");
    let translation = log[0];
    assert!(
        (translation.x - 10.0).abs() < 1e-4 && (translation.y - 5.0).abs() < 1e-4,
        "On<Insert, PlayerSpawn> observer must see GlobalTransform = parent * child = (10, 5, 0); got {translation:?}",
    );
}
