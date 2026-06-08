//! A dedicated server compiles with the `render` feature (for the scene types)
//! but adds none of the rendering plugins. Under `render`, a brush face's
//! `material` is a `Handle<StandardMaterial>`, and an unassigned face serializes
//! as `material: null`. The runtime must still load such a brush headless:
//!
//! 1. `JackdawPlugin` registers `StandardMaterial`/`Image` asset reflection, so
//!    the deserializer's handle processor turns `null` into a default handle
//!    instead of failing (which would drop the whole brush).
//! 2. The `On<Insert, Brush>` mesh-rebuild observer no-ops when the mesh/material
//!    asset stores are absent, so loading the brush does not panic on the missing
//!    `Assets<Mesh>`.
//!
//! Run: `cargo test -p jackdaw_runtime --test headless_render_brush_load`
#![cfg(feature = "render")]

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::reflect::TypePath;
use jackdaw_jsn::format::{JsnAssets, JsnEntity, JsnHeader, JsnMetadata, JsnScene};
use jackdaw_runtime::{JackdawPlugin, JackdawScene, JackdawSceneRoot};
use serde_json::json;

/// One face with an UNASSIGNED material (`null`), exactly as the editor
/// serializes a face that has no material applied.
fn unassigned_face(normal: [f32; 3], distance: f32) -> serde_json::Value {
    json!({
        "material": null,
        "plane": { "normal": normal, "distance": distance },
        "uv_offset": [0.0, 0.0],
        "uv_scale": [1.0, 1.0],
        "uv_rotation": 0.0,
        "uv_u_axis": [0.0, 0.0, 1.0],
        "uv_v_axis": [0.0, -1.0, 0.0],
    })
}

#[test]
fn null_material_brush_loads_headless_under_render_feature() {
    let mut app = App::new();
    // `render` feature is ON (so `material` is `Handle<StandardMaterial>`), but
    // NO rendering plugins are added -- this is the dedicated-server shape.
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(bevy::asset::AssetPlugin::default());
    app.add_plugins(bevy::scene::ScenePlugin);
    app.add_plugins(JackdawPlugin);

    let brush_type_path = <jackdaw_jsn::Brush as TypePath>::type_path().to_string();

    let scene = JsnScene {
        jsn: JsnHeader::default(),
        metadata: JsnMetadata::default(),
        editor: None,
        assets: JsnAssets::default(),
        scene: vec![JsnEntity {
            id: None,
            parent: None,
            components: [(
                brush_type_path,
                json!({
                    "faces": [
                        unassigned_face([1.0, 0.0, 0.0], 1.0),
                        unassigned_face([-1.0, 0.0, 0.0], 1.0),
                        unassigned_face([0.0, 1.0, 0.0], 1.0),
                        unassigned_face([0.0, -1.0, 0.0], 1.0),
                        unassigned_face([0.0, 0.0, 1.0], 1.0),
                        unassigned_face([0.0, 0.0, -1.0], 1.0),
                    ],
                }),
            )]
            .into_iter()
            .collect(),
        }],
    };

    let scene_handle = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(scene, PathBuf::new()));
    app.world_mut().spawn(JackdawSceneRoot(scene_handle));

    // First update runs the scene-load loop; the second covers the insert
    // observer firing. Neither must panic on the absent render asset stores.
    app.update();
    app.update();

    let mut brush_query = app.world_mut().query::<&jackdaw_jsn::Brush>();
    let brushes: Vec<_> = brush_query.iter(app.world()).collect();
    assert_eq!(
        brushes.len(),
        1,
        "a brush with null face materials must survive a headless render-feature load",
    );
    assert_eq!(brushes[0].faces.len(), 6, "all six faces must load");
}

/// When the rendering plugins are already present (a windowed client, the editor)
/// they own the `StandardMaterial` asset machinery. Re-running `init_asset` over it
/// orphans existing handles and corrupts the asset storage, which is what crashed
/// the client. The plugin must detect the existing registration and leave it alone.
#[test]
fn existing_material_storage_is_left_intact() {
    use bevy::pbr::StandardMaterial;

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::asset::AssetPlugin::default());
    app.add_plugins(bevy::scene::ScenePlugin);

    // Stand in for the render plugins owning the material asset, and mint a handle
    // against that storage before the jackdaw plugins are added.
    app.init_asset::<StandardMaterial>();
    let handle = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());

    app.add_plugins(JackdawPlugin);

    assert!(
        app.world()
            .resource::<Assets<StandardMaterial>>()
            .get(&handle)
            .is_some(),
        "the pre-existing material storage must survive plugin init, not be re-registered",
    );
}
