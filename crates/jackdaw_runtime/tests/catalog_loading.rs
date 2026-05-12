//! `JackdawPlugin` loads `.jsn/catalog.jsn` at `Startup` and
//! exposes its `@Name` entries via the `JackdawCatalog` resource.
//! Without this, scene fields like `"material": "@bricks"`
//! silently fall back to defaults at runtime.

use std::path::PathBuf;

use bevy::prelude::*;
use jackdaw_runtime::{JackdawCatalog, JackdawCatalogPath, JackdawPlugin};

#[test]
fn project_catalog_populates_resource() {
    // Catalogs reference textures by path. We use a fake path —
    // the AssetServer returns a handle either way; we only check
    // that `@brick` is keyed into the catalog.
    let catalog_json = r##"{
        "jsn": {
            "format_version": [3, 0, 0],
            "editor_version": "0.4.1",
            "bevy_version": "0.18"
        },
        "assets": {
            "bevy_image::image::Image": {
                "@brick": "does-not-need-to-exist.png",
                "#Image0": "also-fake.png"
            }
        }
    }"##;

    let dir = unique_temp_dir("catalog-loading-resource");
    std::fs::create_dir_all(&dir).unwrap();
    let catalog_path = dir.join("catalog.jsn");
    std::fs::write(&catalog_path, catalog_json).unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(bevy::asset::AssetPlugin::default());
    app.add_plugins(bevy::scene::ScenePlugin);
    app.add_plugins(bevy::image::ImagePlugin::default());
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
    app.add_plugins(bevy::scene::ScenePlugin);
    app.insert_resource(JackdawCatalogPath(PathBuf::from(
        "/definitely/does/not/exist/catalog.jsn",
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
