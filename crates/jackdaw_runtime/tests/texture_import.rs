//! A texture whose `.meta` asks for an import loads as Bevy's processor would
//! write it, and every other file reads exactly as it does without the import
//! reader.

#![cfg(feature = "render")]

use std::path::{Path, PathBuf};

use bevy::asset::io::file::FileAssetReader;
use bevy::asset::io::{AssetReader, AssetReaderError, AssetSourceBuilder, AssetSourceId};
use bevy::asset::{AssetApp, AssetPlugin, LoadState};
use bevy::image::{CompressedImageFormats, ImageLoader};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::tasks::block_on;
use jackdaw_runtime::texture_import::{
    TextureImportReader, texture_import_meta, with_texture_imports,
};

fn write_png(path: &Path, size: u32) {
    let texels = image::RgbaImage::from_fn(size, size, |x, y| {
        image::Rgba([(x * 16) as u8, (y * 16) as u8, 128, 255])
    });
    texels.save(path).expect("the png is written");
}

fn import(dir: &Path, file: &str, srgb: bool) {
    std::fs::write(dir.join(format!("{file}.meta")), texture_import_meta(srgb))
        .expect("the meta is written");
}

fn with_image_loading(mut app: App) -> App {
    app.init_asset::<Image>();
    app.register_asset_loader(ImageLoader::new(CompressedImageFormats::BC));
    app
}

fn reader_app(dir: &Path, cache: PathBuf) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.register_asset_source(
        AssetSourceId::Default,
        with_texture_imports(
            AssetSourceBuilder::platform_default(&dir.to_string_lossy(), None),
            cache,
        ),
    );
    app.add_plugins(AssetPlugin {
        file_path: dir.to_string_lossy().into_owned(),
        ..Default::default()
    });
    with_image_loading(app)
}

fn load_image(app: &mut App, path: &str) -> Image {
    let handle: Handle<Image> = app.world().resource::<AssetServer>().load(path.to_string());
    for _ in 0..2000 {
        app.update();
        if let Some(image) = app.world().resource::<Assets<Image>>().get(&handle) {
            return image.clone();
        }
        if let Some(LoadState::Failed(err)) = app
            .world()
            .resource::<AssetServer>()
            .get_load_state(&handle)
        {
            panic!("{path} failed to load: {err}");
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("{path} never loaded");
}

fn files_under(dir: &Path) -> usize {
    std::fs::read_dir(dir).map_or(0, Iterator::count)
}

#[test]
fn an_imported_texture_loads_block_compressed_with_every_mip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cache = dir.path().join("cache");
    write_png(&dir.path().join("rock.png"), 16);
    write_png(&dir.path().join("rock_n.png"), 16);
    import(dir.path(), "rock.png", true);
    import(dir.path(), "rock_n.png", false);

    let mut app = reader_app(dir.path(), cache.clone());
    let colour = load_image(&mut app, "rock.png");
    let normal = load_image(&mut app, "rock_n.png");

    assert_eq!(
        colour.texture_descriptor.format,
        TextureFormat::Bc7RgbaUnormSrgb
    );
    assert_eq!(
        normal.texture_descriptor.format,
        TextureFormat::Bc7RgbaUnorm
    );
    assert_eq!(colour.texture_descriptor.mip_level_count, 5);
    assert_eq!(colour.size(), UVec2::new(16, 16));
    assert_eq!(
        files_under(&cache),
        2,
        "each processed file is kept for the next load"
    );
}

#[test]
fn a_game_with_the_jackdaw_asset_source_loads_imported_textures() {
    let project = tempfile::tempdir().expect("tempdir");
    let assets = project.path().join("assets");
    std::fs::create_dir_all(&assets).expect("assets");
    write_png(&assets.join("rock.png"), 16);
    import(&assets, "rock.png", true);

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(jackdaw_runtime::JackdawAssetSourcePlugin {
        file_path: assets.to_string_lossy().into_owned(),
        ..Default::default()
    });
    app.add_plugins(AssetPlugin {
        file_path: assets.to_string_lossy().into_owned(),
        ..Default::default()
    });
    let mut app = with_image_loading(app);

    let image = load_image(&mut app, "rock.png");

    assert_eq!(
        image.texture_descriptor.format,
        TextureFormat::Bc7RgbaUnormSrgb
    );
    assert_eq!(files_under(&project.path().join(".jackdaw/imported")), 1);
}

async fn read_all(reader: &TextureImportReader, path: &Path, meta: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut read = if meta {
        Box::new(reader.read_meta(path).await.expect("the meta reads"))
            as Box<dyn bevy::asset::io::Reader>
    } else {
        Box::new(reader.read(path).await.expect("the file reads"))
    };
    read.read_to_end(&mut bytes).await.expect("the bytes read");
    bytes
}

#[test]
fn a_texture_with_no_import_meta_reads_byte_identically() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_png(&dir.path().join("plain.png"), 8);
    write_png(&dir.path().join("other.png"), 8);
    let foreign_meta = b"(meta_format_version: \"1.0\", asset: Ignore)".to_vec();
    std::fs::write(dir.path().join("other.png.meta"), &foreign_meta).expect("meta");
    let reader = TextureImportReader::new(
        Box::new(FileAssetReader::new(dir.path())),
        dir.path().join("cache"),
    );

    let plain = block_on(read_all(&reader, Path::new("plain.png"), false));
    assert_eq!(plain, std::fs::read(dir.path().join("plain.png")).unwrap());
    assert!(matches!(
        block_on(reader.read_meta(Path::new("plain.png"))),
        Err(AssetReaderError::NotFound(_))
    ));
    assert_eq!(
        block_on(read_all(&reader, Path::new("other.png"), true)),
        foreign_meta
    );
    assert_eq!(
        block_on(read_all(&reader, Path::new("other.png"), false)),
        std::fs::read(dir.path().join("other.png")).unwrap()
    );
    assert!(!dir.path().join("cache").exists());
}
