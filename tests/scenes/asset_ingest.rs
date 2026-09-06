use jackdaw::asset_ingest::{AssetKind, classify, import_to_assets, write_to_assets};
use std::fs;

#[test]
fn classify_resolves_images_else_other() {
    assert_eq!(classify("png"), AssetKind::Image);
    assert_eq!(classify("JPG"), AssetKind::Image);
    assert_eq!(classify("gltf"), AssetKind::Other);
    assert_eq!(classify(""), AssetKind::Other);
}

#[test]
fn import_copies_into_assets_and_dedupes_collisions() {
    let tmp = std::env::temp_dir().join(format!("jd_ingest_import_{}", std::process::id()));
    let assets = tmp.join("assets");
    fs::create_dir_all(&assets).unwrap();
    let src = tmp.join("character_reference.png");
    fs::write(&src, b"fake-png-bytes").unwrap();

    let rel = import_to_assets(&assets, &src).unwrap();
    assert_eq!(rel, "character_reference.png");
    assert!(assets.join("character_reference.png").exists());

    let rel2 = import_to_assets(&assets, &src).unwrap();
    assert_eq!(rel2, "character_reference-1.png");
    assert!(assets.join("character_reference-1.png").exists());

    fs::remove_dir_all(&tmp).ok();
}

#[test]
fn write_bytes_dedupes_collisions() {
    let tmp = std::env::temp_dir().join(format!("jd_ingest_write_{}", std::process::id()));
    let assets = tmp.join("assets");
    fs::create_dir_all(&assets).unwrap();

    let a = write_to_assets(&assets, "pasted", "png", b"a").unwrap();
    let b = write_to_assets(&assets, "pasted", "png", b"b").unwrap();
    assert_eq!(a, "pasted.png");
    assert_eq!(b, "pasted-1.png");

    fs::remove_dir_all(&tmp).ok();
}
