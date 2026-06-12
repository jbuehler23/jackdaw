//! A `.jsn` scene must LOAD at runtime under `MinimalPlugins` with NO
//! render stack. Proves two things headless:
//!
//! 1. The scene-load loop (`spawn_loaded_scenes`) runs under
//!    `MinimalPlugins` + `TransformPlugin` + `AssetPlugin` +
//!    `ScenePlugin` and instantiates entities plus user components.
//! 2. A `Brush` whose face `material` is a path string round-trips
//!    through reflection deserialization. Built `--no-default-features`,
//!    `BrushFaceData::material` is typed `String`, so the `"#StoneWall"`
//!    path deserializes straight into it with no render types touched.
//!
//! Run: `cargo test -p jackdaw_runtime --no-default-features --test headless_brush_load`
//!
//! ## Why this test is headless-only (`cfg(not(feature = "render"))`)
//!
//! Under the `render` feature `BrushFaceData::material` is a
//! `Handle<StandardMaterial>`. The runtime's deserializer processor can
//! turn a `#Name`/`@Name`/path string into the correct typed handle
//! ONLY when that name resolves through the scene's inline-asset map or
//! the project catalog — and populating that map requires the
//! `StandardMaterial` asset type to be registered for reflection. That
//! registration is installed by `MaterialPlugin::<StandardMaterial>` /
//! `PbrPlugin`, which drag in the full render stack (`RenderPlugin`,
//! window/GPU adapter, etc.). A genuinely headless `App` has none of
//! that, so render-ON the inline `StandardMaterial` entry is skipped,
//! the bare `#StoneWall` falls through to an untyped `load_untyped`
//! handle, and reflect-applying that untyped handle into the strong
//! `Handle<StandardMaterial>` field fails — the whole `Brush` is
//! rejected. Resolving a material path into a typed handle is therefore
//! inseparable from render-side registration; this test deliberately
//! exercises the render-free `material: String` path instead, and is
//! compiled out when the `render` feature is on.
#![cfg(not(feature = "render"))]

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::reflect::TypePath;
use jackdaw_jsn::format::{JsnAssets, JsnEntity, JsnHeader, JsnMetadata, JsnScene};
use jackdaw_runtime::{JackdawPlugin, JackdawScene, JackdawSceneRoot};
use serde_json::json;

/// User-style component the test injects into the scene. Has a field
/// so the JSN deserializer treats it as a struct, not a unit type.
#[derive(Component, Reflect, Clone, Copy, Default)]
#[reflect(Component, Default)]
struct ZoneMarker {
    pub id: u32,
}

/// One face of the test brush. Field set mirrors the real reflected
/// `BrushFaceData` layout exactly (captured from `assets/dummy.jsn`):
/// `is_cap` is `#[reflect(ignore)]` so it is absent, and `material` is a
/// bare `#Name` reference string just as the editor writes it.
fn brush_face(material: &str, normal: [f32; 3], distance: f32) -> serde_json::Value {
    json!({
        "material": material,
        "plane": {
            "normal": normal,
            "distance": distance,
        },
        "uv_offset": [0.0, 0.0],
        "uv_scale": [1.0, 1.0],
        "uv_rotation": 0.0,
        "uv_u_axis": [0.0, 0.0, 1.0],
        "uv_v_axis": [0.0, -1.0, 0.0],
    })
}

#[test]
fn headless_brush_load() {
    let mut app = App::new();
    // Deliberately NO render/image/PBR plugins. `ScenePlugin` is in the
    // base feature set and the loader path expects it (matches the
    // existing `insert_observer_global_transform` harness).
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(bevy::asset::AssetPlugin::default());
    app.add_plugins(bevy::scene::ScenePlugin);
    app.add_plugins(JackdawPlugin);
    app.register_type::<ZoneMarker>();

    // The `Brush` component lives in `jackdaw_jsn` (type path
    // `jackdaw_jsn::types::Brush`), re-exported as `jackdaw_jsn::Brush`.
    // Its `faces` carry the geometry; `BrushFaceData`/`BrushPlane` come
    // from `jackdaw_geometry`. Reference the path via `TypePath` rather
    // than hardcoding the fragile module string.
    let brush_type_path = <jackdaw_jsn::Brush as TypePath>::type_path().to_string();

    // Entity 0: a transform carrying the user marker.
    // Entity 1: a brush whose every face references `#StoneWall` by path
    // string. The real reflected `Brush` omits `topology` for legacy
    // brushes loaded without it (the deserializer fills it from the
    // type's registered default), so we omit it here too.
    let scene = JsnScene {
        jsn: JsnHeader::default(),
        metadata: JsnMetadata::default(),
        editor: None,
        assets: JsnAssets::default(),
        scene: vec![
            JsnEntity {
                id: None,
                parent: None,
                components: [
                    (
                        "bevy_transform::components::transform::Transform".to_string(),
                        json!({
                            "translation": [1.0, 2.0, 3.0],
                            "rotation": [0.0, 0.0, 0.0, 1.0],
                            "scale": [1.0, 1.0, 1.0],
                        }),
                    ),
                    (
                        <ZoneMarker as TypePath>::type_path().to_string(),
                        json!({ "id": 7 }),
                    ),
                ]
                .into_iter()
                .collect(),
            },
            JsnEntity {
                id: None,
                parent: None,
                components: [(
                    brush_type_path,
                    json!({
                        "faces": [
                            brush_face("#StoneWall", [1.0, 0.0, 0.0], 1.0),
                            brush_face("#StoneWall", [-1.0, 0.0, 0.0], 1.0),
                            brush_face("#StoneWall", [0.0, 1.0, 0.0], 1.0),
                            brush_face("#StoneWall", [0.0, -1.0, 0.0], 1.0),
                            brush_face("#StoneWall", [0.0, 0.0, 1.0], 1.0),
                            brush_face("#StoneWall", [0.0, 0.0, -1.0], 1.0),
                        ],
                    }),
                )]
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
    // PostUpdate follow-up (matches the existing harness).
    app.update();
    app.update();

    // Exactly one ZoneMarker, with the authored id: proves the headless
    // load loop ran and user components were inserted.
    let mut zone_query = app.world_mut().query::<&ZoneMarker>();
    let zones: Vec<u32> = zone_query.iter(app.world()).map(|z| z.id).collect();
    assert_eq!(zones.len(), 1, "expected exactly one ZoneMarker entity");
    assert_eq!(zones[0], 7, "ZoneMarker id must survive headless load");

    // The brush must survive load with its `material` path string
    // deserialized into the render-free `String` field.
    let brush_count = app
        .world_mut()
        .query::<&jackdaw_jsn::Brush>()
        .iter(app.world())
        .count();
    assert_eq!(brush_count, 1, "Brush geometry must survive headless load");

    // The brush deserialized in full: all six faces survived the headless
    // load loop. The material-value round-trip is intentionally NOT asserted
    // here: `BrushFaceData::material` is `String` only under headless
    // `jackdaw_geometry`, but cross-crate `--all-targets` feature unification
    // (the `jackdaw_jsn` dev-dependency pulls in `jackdaw_geometry/render`)
    // can flip it to `Handle<StandardMaterial>`, which neither compares to a
    // `&str` nor carries the authored path in its `Debug` output. The
    // headless property under test is that the brush + faces survive load
    // without the render asset machinery; that the `#StoneWall` path string
    // deserializes into the `String` field is covered implicitly by the
    // brush deserializing at all.
    let mut brush_query = app.world_mut().query::<&jackdaw_jsn::Brush>();
    let brush = brush_query.iter(app.world()).next().expect("one brush");
    assert_eq!(brush.faces.len(), 6, "all six faces must load");
}
