//! `JackdawPlugin` loads `catalog.bsn` at `Startup` and exposes its named
//! entries via the `JackdawCatalog` resource, keyed as `@Name`. Without this,
//! scene fields like `material: "@bricks"` silently fall back to defaults at
//! runtime.

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
