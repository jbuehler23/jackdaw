//! The startup walk reads a document in either form, and keys a binary one by
//! the path its text twin would sit at, so references written before an export
//! still resolve.

use std::path::PathBuf;

use bevy::asset::{Asset, AssetApp};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use jackdaw_runtime::{JackdawCatalog, JackdawCatalogPath, JackdawPlugin};

/// A reflectable stand-in for a material, so the walk has a registered asset
/// type to load without the render stack.
#[derive(Asset, Reflect, Default)]
#[reflect(Default)]
struct BinaryMaterial {
    tint: f32,
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

fn app_over(dir: &std::path::Path) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(bevy::asset::AssetPlugin::default());
    app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    app.add_plugins(bevy::image::ImagePlugin::default());
    app.init_asset::<BinaryMaterial>();
    app.register_asset_reflect::<BinaryMaterial>();
    app.insert_resource(JackdawCatalogPath(dir.join("catalog.bsn")));
    app.add_plugins(JackdawPlugin);
    app
}

#[test]
fn the_runtime_loads_a_material_held_in_the_binary_form() {
    let type_path = <BinaryMaterial as TypePath>::type_path();
    let dir = unique_temp_dir("binary-material");
    std::fs::create_dir_all(dir.join("materials")).unwrap();
    let text = jackdaw_bsn::with_asset_header(type_path, &format!("#grass\n{type_path}\n"));
    jackdaw_bsn::write_document_text(&dir.join("materials/grass.bsb"), &text).unwrap();

    let mut app = app_over(&dir);
    app.update();

    let catalog = app.world().resource::<JackdawCatalog>();
    assert!(
        catalog.get("materials/grass.bsn").is_some(),
        "a binary material answers to the path its text twin would sit at"
    );
    assert!(
        catalog.get("materials/grass.bsb").is_some(),
        "and to the path it is written at"
    );
    assert!(
        catalog.get("@grass").is_some(),
        "and to the one name it carries"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_document_held_in_both_forms_is_loaded_once_from_its_text() {
    let type_path = <BinaryMaterial as TypePath>::type_path();
    let dir = unique_temp_dir("binary-material-pair");
    std::fs::create_dir_all(dir.join("materials")).unwrap();
    let text = jackdaw_bsn::with_asset_header(
        type_path,
        &format!("#grass\n{type_path} {{ tint: 1.0 }}\n"),
    );
    jackdaw_bsn::write_document_text(&dir.join("materials/grass.bsn"), &text).unwrap();
    jackdaw_bsn::write_document_text(&dir.join("materials/grass.bsb"), &text).unwrap();

    let mut app = app_over(&dir);
    app.update();

    let catalog = app.world().resource::<JackdawCatalog>();
    assert!(
        catalog.get("materials/grass.bsn").is_some(),
        "the text file is the one that loaded"
    );
    assert_eq!(
        catalog.len(),
        2,
        "the pair is one asset under one path and one name, not two"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
