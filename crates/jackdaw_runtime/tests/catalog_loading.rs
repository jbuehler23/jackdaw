//! `JackdawPlugin` reads the project's asset files at `Startup` and exposes
//! them through the `JackdawCatalog` resource, keyed by the path each file sits
//! at and by the `@Name` the references written before paths spell. Without it,
//! a scene field naming a material silently falls back to a default at runtime.

use std::path::PathBuf;

use bevy::asset::{Asset, AssetApp};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use jackdaw_runtime::{JackdawCatalog, JackdawCatalogPath, JackdawPlugin};

/// A minimal reflectable asset stood up so the catalog has a concrete type to
/// load. Real catalogs hold `StandardMaterial` entries, which need the render
/// stack; this exercises the `@Name` resolution path without it.
#[derive(Asset, Reflect, Default)]
#[reflect(Default)]
struct CatalogMaterial {
    tint: f32,
}

#[test]
fn project_catalog_populates_resource() {
    // A single named catalog entry. `load_bsn_assets` reads the `#Name`, builds
    // the asset from its default, and the runtime keys it as `@brick`.
    let type_path = <CatalogMaterial as TypePath>::type_path();
    let catalog_bsn = format!("#brick\n{type_path}\n");

    let dir = unique_temp_dir("catalog-loading-resource");
    std::fs::create_dir_all(&dir).unwrap();
    let catalog_path = dir.join("catalog.bsn");
    std::fs::write(&catalog_path, catalog_bsn).unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(bevy::asset::AssetPlugin::default());
    app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    app.add_plugins(bevy::image::ImagePlugin::default());
    app.init_asset::<CatalogMaterial>();
    app.register_asset_reflect::<CatalogMaterial>();
    app.insert_resource(JackdawCatalogPath(catalog_path.clone()));
    app.add_plugins(JackdawPlugin);

    // First update fires `Startup`, which loads the catalog.
    app.update();

    let catalog = app.world().resource::<JackdawCatalog>();
    assert!(
        catalog.get("@brick").is_some(),
        "expected @brick in JackdawCatalog after Startup; entries = {}",
        catalog.len()
    );
    assert!(
        catalog.get("#Image0").is_none(),
        "#Image0 is a scene-local inline name; catalog should only keep @-prefixed entries"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_catalog_leaves_resource_empty() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(bevy::asset::AssetPlugin::default());
    app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    app.insert_resource(JackdawCatalogPath(PathBuf::from(
        "/definitely/does/not/exist/catalog.bsn",
    )));
    app.add_plugins(JackdawPlugin);

    app.update();

    assert!(app.world().resource::<JackdawCatalog>().is_empty());
}

fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "jackdaw-runtime-{label}-{}-{nanos}",
        std::process::id()
    ))
}

/// A component holding one material, so a scene can spell a reference without
/// a mesh in the way.
#[derive(Component, Reflect, Clone, Default)]
#[reflect(Component, Default)]
struct Painted {
    material: Handle<StandardMaterial>,
}

#[test]
fn a_scene_reaches_a_material_in_any_folder_by_the_path_of_its_file() {
    let dir = unique_temp_dir("catalog-loading-path");
    std::fs::create_dir_all(dir.join("content/props")).unwrap();
    std::fs::write(
        dir.join("content/props/grass.bsn"),
        "#grass\nbevy_pbr::pbr_material::StandardMaterial {\n    perceptual_roughness: 0.25,\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("scene.bsn"),
        format!(
            "{} {{ material: \"content/props/grass.bsn\" }}\n",
            <Painted as TypePath>::type_path()
        ),
    )
    .unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(bevy::asset::AssetPlugin {
        file_path: dir.to_string_lossy().into_owned(),
        ..Default::default()
    });
    app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    app.add_plugins(bevy::image::ImagePlugin::default());
    app.init_asset::<StandardMaterial>();
    app.register_asset_reflect::<StandardMaterial>();
    app.register_type::<Painted>();
    app.add_plugins(JackdawPlugin);

    let handle: Handle<jackdaw_runtime::JackdawScene> =
        app.world().resource::<AssetServer>().load("scene.bsn");
    app.world_mut()
        .spawn(jackdaw_runtime::JackdawSceneRoot(handle));

    let mut painted = None;
    for _ in 0..200 {
        app.update();
        let mut query = app.world_mut().query::<&Painted>();
        if let Some(found) = query.iter(app.world()).next() {
            painted = Some(found.material.clone());
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let painted = painted.expect("the scene spawned its painted entity");

    let catalog = app.world().resource::<JackdawCatalog>();
    assert_eq!(
        Some(painted.id().untyped()),
        catalog
            .get("content/props/grass.bsn")
            .map(bevy::asset::UntypedHandle::id),
        "the path names the material the catalog loaded, not a fresh load of the document"
    );
    assert_eq!(
        Some(painted.id().untyped()),
        catalog.get("@grass").map(bevy::asset::UntypedHandle::id),
        "the name the file was spelled by before paths reaches the same material"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
