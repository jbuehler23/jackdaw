//! `tileset.import`: the packed copy and its array meta, the tileset asset
//! for each setup choice, refusals, and the asset index opening the result.

use crate::util;
use crate::util::OperatorResultExt as _;

use std::path::{Path, PathBuf};
use std::time::Duration;

use bevy::asset::meta::AssetAction;
use bevy::image::{ImageArrayLayout, ImageSampler};
use bevy::prelude::*;
use jackdaw::asset_index::{AssetIndex, AssetValue};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{OperatorReports, OperatorWarnings};
use jackdaw_runtime::texture_import::TextureImportMeta;
use jackdaw_scene_types::PropertyValue;
use jackdaw_tilemap::{RgbaImage, TilesetSetup, auto_setup};
use jackdaw_tileset::{TerrainMatch, Tileset};

pub(crate) const SHEET: &str = "overworld.png";
const SPACED_SHEET: &str = "overworld_spaced.png";

/// An editor on a temp project whose assets hold the overworld sheet, packed
/// and spaced.
pub(crate) fn project_app() -> (App, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let assets = dir.path().join("assets");
    std::fs::create_dir_all(&assets).expect("assets");
    std::fs::copy(util::OVERWORLD_SHEET, assets.join(SHEET)).expect("copy the sheet");
    util::write_spaced_overworld_sheet(&assets.join(SPACED_SHEET));
    let app = editor_on(&dir);
    (app, dir)
}

fn editor_on(dir: &tempfile::TempDir) -> App {
    let mut app = util::editor_test_app_reading(&dir.path().join("assets"));
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: dir.path().to_path_buf(),
            config: default(),
        });
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    for _ in 0..4 {
        app.update();
    }
    app
}

/// The sheet down to its last row of art: the tiles an import packs.
fn art_rows() -> image::RgbaImage {
    let sheet = decoded(util::OVERWORLD_SHEET);
    image::imageops::crop_imm(&sheet, 0, 0, 432, 38 * 16).to_image()
}

#[track_caller]
pub(crate) fn import(app: &mut App, params: &[(&'static str, &str)]) {
    assert_eq!(call_import(app, params), OperatorResult::Finished);
}

fn call_import(app: &mut App, params: &[(&'static str, &str)]) -> OperatorResult {
    app.world_mut()
        .get_resource_or_init::<OperatorReports>()
        .0
        .clear();
    app.world_mut()
        .get_resource_or_init::<OperatorWarnings>()
        .0
        .clear();
    let mut call = app.world_mut().operator("tileset.import");
    for (key, value) in params {
        call = call.param(*key, PropertyValue::from(value.to_string()));
    }
    let result = call.call().expect("tileset.import dispatches");
    app.update();
    result
}

pub(crate) fn reports(app: &App) -> Vec<String> {
    app.world().resource::<OperatorReports>().0.clone()
}

pub(crate) fn warnings(app: &App) -> Vec<String> {
    app.world().resource::<OperatorWarnings>().0.clone()
}

pub(crate) fn asset_file(dir: &tempfile::TempDir, relative: &str) -> PathBuf {
    dir.path().join("assets").join(relative)
}

pub(crate) fn read_tileset(path: &Path) -> Tileset {
    jackdaw_bsn::read_asset_file::<Tileset>(path).expect("the tileset reads back")
}

fn decoded(path: impl AsRef<Path>) -> image::RgbaImage {
    image::open(path).expect("decodes").to_rgba8()
}

fn layers_once_loaded(app: &mut App, path: &'static str) -> (UVec2, u32) {
    let handle: Handle<Image> = app.world().resource::<AssetServer>().load(path);
    util::update_until(app, path, Duration::from_secs(20), |app| {
        app.world()
            .resource::<Assets<Image>>()
            .get(&handle)
            .is_some()
    });
    let images = app.world().resource::<Assets<Image>>();
    let image = images.get(&handle).expect("loaded");
    let size = image.texture_descriptor.size;
    (
        UVec2::new(size.width, size.height),
        size.depth_or_array_layers,
    )
}

#[test]
fn auto_import_packs_the_sheet_and_finds_its_terrains() {
    let (mut app, dir) = project_app();
    import(&mut app, &[("paths", SHEET), ("setup", "auto")]);
    assert!(warnings(&app).is_empty(), "{:?}", warnings(&app));

    let packed = asset_file(&dir, "overworld.tiles.png");
    assert_eq!(decoded(&packed), art_rows());

    let meta = std::fs::read(asset_file(&dir, "overworld.tiles.png.meta")).expect("a meta");
    let meta = TextureImportMeta::deserialize(&meta).expect("an image meta");
    let AssetAction::Load { settings, .. } = &meta.asset else {
        panic!("the packed copy loads unprocessed");
    };
    assert!(
        matches!(
            settings.array_layout,
            Some(ImageArrayLayout::GridSize {
                tile_width_pixels: 16,
                tile_height_pixels: 16
            })
        ),
        "{:?}",
        settings.array_layout
    );
    let ImageSampler::Descriptor(sampler) = &settings.sampler else {
        panic!("the packed copy names its sampler");
    };
    assert_eq!(sampler.mag_filter, bevy::image::ImageFilterMode::Nearest);

    let tileset = read_tileset(&asset_file(&dir, "overworld.tileset.bsn"));
    assert_eq!(tileset.image, "overworld.tiles.png");
    assert_eq!(tileset.tile_size, UVec2::splat(16));
    assert_eq!(tileset.grid, UVec2::new(27, 38));
    assert_eq!(tileset.setup_sheets, vec![0]);
    assert_eq!(tileset.terrains.len(), 8);
    let background: Vec<u8> = (tileset.terrains.iter())
        .filter(|t| t.background)
        .map(|t| t.id)
        .collect();
    // Grass has pure tiles, so the background is the dark ground the cliff
    // set sits on.
    assert_eq!(background, vec![6]);
    let sets: Vec<(u8, u8, TerrainMatch)> = (tileset.terrain_sets.iter())
        .map(|s| (s.inner, s.outer, s.mode))
        .collect();
    assert_eq!(
        sets,
        [
            (2, 1, TerrainMatch::CornersAndSides),
            (5, 6, TerrainMatch::Corners),
            (9, 2, TerrainMatch::Corners),
            (11, 1, TerrainMatch::CornersAndSides),
            (11, 9, TerrainMatch::Corners),
            (12, 11, TerrainMatch::Corners),
        ]
    );
    assert_eq!(tileset.objects.len(), 34);
    let sheet = decoded(util::OVERWORLD_SHEET);
    let image = RgbaImage::new(sheet.width(), sheet.height(), sheet.as_raw()).expect("sized");
    let fresh = auto_setup(&[(SHEET, image)], &[]).expect("auto setup runs");
    assert_eq!(
        tileset.setup().expect("converts"),
        TilesetSetup::from_auto(&fresh),
        "the file holds the whole setup"
    );
    let report = reports(&app).join("\n");
    assert_eq!(
        report.matches("Corners, 14/16 shapes").count(),
        3,
        "the report covers the sea sets: {report}"
    );

    assert_eq!(
        layers_once_loaded(&mut app, "overworld.tiles.png"),
        (UVec2::splat(16), 1026)
    );
    assert!(!asset_file(&dir, "overworld.png.meta").exists());
    assert_eq!(
        layers_once_loaded(&mut app, SHEET),
        (UVec2::new(432, 1040), 1),
        "a sprite of the source sheet still draws it whole"
    );
}

#[test]
fn a_spaced_sheet_packs_to_the_same_tiles() {
    let (mut app, dir) = project_app();
    import(&mut app, &[("paths", SPACED_SHEET), ("setup", "none")]);
    assert!(warnings(&app).is_empty(), "{:?}", warnings(&app));

    assert_eq!(
        decoded(asset_file(&dir, "overworld_spaced.tiles.png")),
        art_rows()
    );
    let tileset = read_tileset(&asset_file(&dir, "overworld_spaced.tileset.bsn"));
    assert_eq!(tileset.grid, UVec2::new(27, 38));
    let sheet = &tileset.sheets[0];
    assert_eq!(sheet.source, SPACED_SHEET);
    assert_eq!(sheet.spacing, UVec2::ONE);
    assert_eq!((sheet.columns, sheet.rows), (27, 38));
    assert!(
        tileset.setup_sheets.is_empty(),
        "none leaves out terrain setup"
    );
    assert!(tileset.terrains.is_empty() && tileset.tiles.is_empty());
}

#[test]
fn manual_setup_covers_the_sheets_with_no_terrains() {
    let (mut app, dir) = project_app();
    import(
        &mut app,
        &[
            ("paths", SHEET),
            ("setup", "manual"),
            ("out", "farm/ground.tileset.bsn"),
        ],
    );
    assert!(warnings(&app).is_empty(), "{:?}", warnings(&app));

    let tileset = read_tileset(&asset_file(&dir, "farm/ground.tileset.bsn"));
    assert_eq!(tileset.image, "farm/ground.tiles.png");
    assert!(asset_file(&dir, "farm/ground.tiles.png.meta").exists());
    assert_eq!(tileset.setup_sheets, vec![0]);
    assert!(tileset.terrains.is_empty() && tileset.terrain_sets.is_empty());
    let setup = tileset.setup().expect("converts");
    assert_eq!(setup.sheets, vec![SHEET.to_string()]);
}

#[test]
fn several_sheets_each_start_a_row_of_the_packed_copy() {
    let (mut app, dir) = project_app();
    // Both copies of the sheet would need 2052 layers, past the array limit.
    import(
        &mut app,
        &[
            ("paths", "overworld.png, overworld_spaced.png"),
            ("setup", "none"),
        ],
    );
    assert!(
        warnings(&app).join("\n").contains("2052 layers"),
        "{:?}",
        warnings(&app)
    );
    // The sheet's first four rows as a second, shorter sheet.
    let rows = image::imageops::crop_imm(&art_rows(), 0, 0, 432, 64).to_image();
    rows.save(asset_file(&dir, "rows.png")).expect("save");
    import(
        &mut app,
        &[("paths", "overworld.png, rows.png"), ("setup", "none")],
    );
    assert!(warnings(&app).is_empty(), "{:?}", warnings(&app));

    let tileset = read_tileset(&asset_file(&dir, "overworld.tileset.bsn"));
    assert_eq!(tileset.grid, UVec2::new(27, 42));
    let firsts: Vec<u32> = tileset.sheets.iter().map(|s| s.first_layer).collect();
    assert_eq!(firsts, vec![0, 1026]);
    assert_eq!(tileset.layer(1, 0), Some(1026));
    let packed = decoded(asset_file(&dir, "overworld.tiles.png"));
    assert_eq!(packed.dimensions(), (432, 672));
}

/// Names of the files under the assets that an import wrote.
fn tile_files(dir: &tempfile::TempDir) -> Vec<String> {
    let mut found: Vec<String> = jackdaw_bsn::walk_files_with_extensions(
        &dir.path().join("assets"),
        &["bsn", "png", "meta"],
    )
    .into_iter()
    .filter_map(|file| file.file_name()?.to_str().map(str::to_owned))
    .filter(|name| name.contains(".tile"))
    .collect();
    found.sort();
    found
}

#[test]
fn imports_it_cannot_make_are_refused_and_leave_nothing_behind() {
    let (mut app, dir) = project_app();
    std::fs::copy(util::OVERWORLD_SHEET, dir.path().join("outside.png")).expect("copy");
    std::fs::write(asset_file(&dir, "notes.txt"), "not an image").expect("write");
    image::RgbaImage::from_pixel(40, 24, image::Rgba([10, 200, 30, 255]))
        .save(asset_file(&dir, "flat.png"))
        .expect("save");

    for (paths, setup, says) in [
        ("outside.png", "auto", "outside"),
        ("notes.txt", "auto", "not an image"),
        ("missing.png", "auto", "nothing at"),
        ("flat.png", "auto", "no tile grid"),
        ("overworld.png, ./overworld.png", "auto", "more than once"),
        (
            "overworld.png, assets/overworld.png",
            "manual",
            "more than once",
        ),
        (
            "overworld_spaced.png,overworld_spaced.png",
            "none",
            "more than once",
        ),
    ] {
        import(&mut app, &[("paths", paths), ("setup", setup)]);
        let told = warnings(&app).join("\n");
        assert!(told.contains(says), "{paths}: {told}");
        assert!(
            tile_files(&dir).is_empty(),
            "{paths}: {:?}",
            tile_files(&dir)
        );
    }

    import(&mut app, &[("paths", SHEET)]);
    assert!(warnings(&app).is_empty(), "{:?}", warnings(&app));
    import(&mut app, &[("paths", SHEET)]);
    assert!(
        warnings(&app).join("\n").contains("already there"),
        "a second import does not overwrite the first"
    );
    std::fs::remove_file(asset_file(&dir, "overworld.tileset.bsn")).expect("remove");
    import(&mut app, &[("paths", SHEET)]);
    assert!(
        warnings(&app)
            .join("\n")
            .contains("overworld.tiles.png or its meta"),
        "a packed copy left behind is not replaced: {:?}",
        warnings(&app)
    );

    assert_eq!(
        call_import(&mut app, &[("paths", SHEET), ("setup", "sometimes")]),
        OperatorResult::Cancelled
    );
    assert_eq!(
        tile_files(&dir),
        vec!["overworld.tiles.png", "overworld.tiles.png.meta"]
    );
}

#[test]
fn packed_copies_are_named_by_the_whole_tileset_stem() {
    let (mut app, dir) = project_app();
    for name in ["grass.png", "grass.winter.png", "x.tiles.png"] {
        std::fs::copy(util::OVERWORLD_SHEET, asset_file(&dir, name)).expect("copy");
    }
    import(&mut app, &[("paths", "grass.png"), ("setup", "none")]);
    import(
        &mut app,
        &[("paths", "grass.winter.png"), ("setup", "none")],
    );
    assert!(warnings(&app).is_empty(), "{:?}", warnings(&app));
    let image_of = |file: &str| read_tileset(&asset_file(&dir, file)).image;
    assert_eq!(image_of("grass.tileset.bsn"), "grass.tiles.png");
    assert_eq!(
        image_of("grass.winter.tileset.bsn"),
        "grass.winter.tiles.png"
    );

    import(&mut app, &[("paths", "x.tiles.png"), ("out", "x")]);
    assert!(
        warnings(&app).join("\n").contains("would replace a sheet"),
        "{:?}",
        warnings(&app)
    );
    assert_eq!(
        decoded(asset_file(&dir, "x.tiles.png")),
        decoded(util::OVERWORLD_SHEET)
    );
    assert!(!asset_file(&dir, "x.tiles.png.meta").exists());
    import(&mut app, &[("paths", "x.tiles.png"), ("setup", "none")]);
    assert_eq!(image_of("x.tiles.tileset.bsn"), "x.tiles.tiles.png");
}

#[test]
fn out_names_a_file_a_folder_or_a_stem() {
    let (mut app, dir) = project_app();
    std::fs::create_dir_all(asset_file(&dir, "farm")).expect("folder");
    for (out, written) in [
        ("", "overworld.tileset.bsn"),
        ("farm", "farm/overworld.tileset.bsn"),
        ("meadow/", "meadow/overworld.tileset.bsn"),
        ("ground.tileset", "ground.tileset.bsn"),
        ("hills", "hills.tileset.bsn"),
        ("assets/x.tileset.bsn", "x.tileset.bsn"),
    ] {
        import(
            &mut app,
            &[("paths", SHEET), ("setup", "none"), ("out", out)],
        );
        assert!(warnings(&app).is_empty(), "{out}: {:?}", warnings(&app));
        assert!(asset_file(&dir, written).is_file(), "{out}: {written}");
    }
    assert!(asset_file(&dir, "ground.tiles.png").is_file());
    assert!(!dir.path().join("assets/assets").exists());
}

#[test]
fn texture_import_keeps_a_packed_copy_an_array() {
    let (mut app, dir) = project_app();
    import(&mut app, &[("paths", SHEET), ("setup", "none")]);
    let packed = "overworld.tiles.png";
    let layout = |dir: &tempfile::TempDir| {
        let bytes = std::fs::read(asset_file(dir, &format!("{packed}.meta"))).expect("a meta");
        let meta = TextureImportMeta::deserialize(&bytes).expect("an image meta");
        let settings = match &meta.asset {
            AssetAction::Load { settings, .. } => settings.clone(),
            AssetAction::Process { settings, .. } => settings.loader_settings.clone(),
            AssetAction::Ignore => panic!("the packed copy is read"),
        };
        matches!(
            settings.array_layout,
            Some(ImageArrayLayout::GridSize {
                tile_width_pixels: 16,
                tile_height_pixels: 16
            })
        )
    };
    for compression in ["vram_compressed", "none"] {
        app.world_mut()
            .operator("texture.import")
            .param("path", PropertyValue::from(packed))
            .param("compression", PropertyValue::from(compression))
            .call()
            .expect("texture.import dispatches")
            .assert_finished();
        app.update();
        assert!(layout(&dir), "{compression}");
    }
}

#[test]
fn the_imported_tileset_is_listed_and_opens_from_the_index() {
    let (mut app, dir) = project_app();
    import(&mut app, &[("paths", SHEET)]);
    let indexed = Path::new("overworld.tileset.bsn");
    let entry = app.world().resource::<AssetIndex>().get(indexed).cloned();
    assert!(matches!(
        entry.map(|e| e.value),
        Some(AssetValue::Handle(_))
    ));

    app.world_mut()
        .get_resource_or_init::<OperatorReports>()
        .0
        .clear();
    app.world_mut()
        .operator("asset.list")
        .param("type", PropertyValue::from("tileset"))
        .call()
        .expect("asset.list dispatches")
        .assert_finished();
    app.update();
    assert_eq!(reports(&app), vec!["tileset: overworld.tileset.bsn"]);
    drop(app);

    let mut reopened = editor_on(&dir);
    util::update_until(
        &mut reopened,
        "the walk to index the tileset",
        Duration::from_secs(20),
        |app| app.world().resource::<AssetIndex>().get(indexed).is_some(),
    );
    let entry = (reopened
        .world()
        .resource::<AssetIndex>()
        .get(indexed)
        .cloned())
    .expect("indexed");
    let AssetValue::Handle(handle) = entry.value else {
        panic!("a walk loads the tileset rather than leaving it to a panel");
    };
    let tileset = (reopened.world().resource::<Assets<Tileset>>())
        .get(handle.id().typed::<Tileset>())
        .cloned()
        .expect("in the store");
    assert_eq!(tileset.terrains.len(), 8);
    assert!(jackdaw::definition_assets::open_definition_file(
        reopened.world_mut(),
        &asset_file(&dir, "overworld.tileset.bsn"),
    ));
}
