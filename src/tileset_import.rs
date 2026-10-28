use std::path::{Component, Path, PathBuf};

use bevy::prelude::*;
use image::ImageEncoder as _;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lucide_icons::Icon;
use jackdaw_tilemap::{
    GridLayout, RgbaImage, TilesetSetup, auto_setup_on_grid, detect_grid_shared,
};
use jackdaw_tileset::{Tileset, TilesetSheet};
use path_slash::PathExt as _;

use crate::asset_index::{AssetValue, assets_dir, index_written, indexed_path};
use crate::definition_assets::{asset_text, names_a_file};
use crate::scene_io::save::write_atomic;
use crate::scenes::operators::resolve_scene_path;

/// The kind a tileset file holds.
pub const TILESET_KIND: &str = "tileset";

const TILESET_SUFFIX: &str = ".tileset.bsn";

/// Layers an array texture may hold on native backends.
const MAX_LAYERS: u32 = 2048;

/// Layers an array texture may hold on WebGL2.
const WEBGL2_MAX_LAYERS: u32 = 256;

pub(crate) fn plugin(app: &mut App) {
    app.add_plugins(jackdaw_tileset::TilesetPlugin);
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_asset_kind(
        AssetKind::extension(TILESET_KIND, "Tileset", Tileset::type_path())
            .with_icon(Icon::Grid2x2),
    );
    ctx.register_operator::<TilesetImportOp>();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SetupChoice {
    Auto,
    Manual,
    None,
}

/// Make a tileset from tile sheets: a packed copy of their tiles that loads as
/// an array texture, and the tileset asset over it.
///
/// The packed copy holds one layer per tile and at most 2048 layers, the most
/// an array texture holds; WebGL2 loads at most 256. Nothing is written when
/// any step fails, and an import never replaces an existing file.
#[operator(
    id = "tileset.import",
    label = "Import Tileset",
    description = "Make a tileset from one or more tile sheets of one tile size: a packed copy \
                   of their tiles with a .meta that loads it as an array texture in any Bevy \
                   app, and the tileset asset beside it. The sheets are left as they are. At \
                   most 2048 tiles; WebGL2 loads at most 256.",
    allows_undo = false,
    params(
        paths(
            String,
            doc = "Comma-separated image paths under the project's assets."
        ),
        out(
            String,
            doc = "Tileset file, or folder to write it in. A name not ending in .tileset.bsn \
                   is a stem. Defaults to <first sheet>.tileset.bsn beside the first sheet; \
                   the packed copy <stem>.tiles.png goes beside the tileset."
        ),
        setup(
            String,
            default = "auto",
            doc = "\"auto\" finds terrains, terrain sets and objects, \"manual\" leaves them \
                   to be set by hand, \"none\" makes a plain tile atlas."
        ),
    )
)]
pub fn tileset_import(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let paths: Vec<PathBuf> = (params.as_str("paths").unwrap_or_default().split(','))
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .collect();
    if paths.is_empty() {
        warn!("tileset.import: no paths given");
        return OperatorResult::Cancelled;
    }
    let choice = match params.as_str("setup").unwrap_or("auto") {
        "auto" => SetupChoice::Auto,
        "manual" => SetupChoice::Manual,
        "none" => SetupChoice::None,
        other => {
            warn!("tileset.import: setup is auto, manual or none, not `{other}`");
            return OperatorResult::Cancelled;
        }
    };
    let out = (params.as_str("out"))
        .filter(|out| !out.trim().is_empty())
        .map(PathBuf::from);
    commands.queue(
        move |world: &mut World| match import(world, &paths, out.as_deref(), choice) {
            Ok(message) => report_to_caller(world, message),
            Err(message) => warn_caller(world, format!("tileset.import: {message}")),
        },
    );
    OperatorResult::Finished
}

pub(crate) struct Sheet {
    file: PathBuf,
    pub(crate) name: String,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Sheet {
    pub(crate) fn image(&self) -> Result<RgbaImage<'_>, String> {
        RgbaImage::new(self.width, self.height, &self.pixels)
            .ok_or_else(|| format!("{} decodes to the wrong size", self.name))
    }
}

/// Build the packed copy, its meta and the tileset, then write all three, and
/// say what was made.
fn import(
    world: &mut World,
    paths: &[PathBuf],
    out: Option<&Path>,
    choice: SetupChoice,
) -> Result<String, String> {
    let assets = assets_dir(world).ok_or("no project is open")?;
    let mut names: Vec<String> = Vec::with_capacity(paths.len());
    for path in paths {
        let name = sheet_name(world, path)?;
        if names.contains(&name) {
            return Err(format!("{name} is named more than once"));
        }
        names.push(name);
    }
    let sheets = (names.into_iter())
        .map(|name| decode_sheet(&assets, name))
        .collect::<Result<Vec<_>, _>>()?;
    let images = (sheets.iter().map(Sheet::image)).collect::<Result<Vec<_>, _>>()?;
    let detected = detect_grid_shared(&images).ok_or("no tile grid found in the sheets")?;
    let layouts = (sheets.iter().zip(&detected.layouts))
        .map(|(sheet, layout)| {
            (layout.map(|(layout, _)| layout))
                .ok_or_else(|| format!("{} has no grid of {} tiles", sheet.name, detected.tile))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let out_name = tileset_path(world, out, &sheets[0])?;
    let out = assets.join(&out_name);
    if out.exists() {
        return Err(format!("{} is already there", out_name.to_slash_lossy()));
    }
    let file_name = out_name.file_name().unwrap_or_default().to_string_lossy();
    let stem = file_name.strip_suffix(TILESET_SUFFIX).unwrap_or(&file_name);
    let packed_name = out_name.with_file_name(format!("{stem}.tiles.png"));
    let packed_slash = packed_name.to_slash_lossy().into_owned();
    if sheets.iter().any(|sheet| sheet.name == packed_slash) {
        return Err(format!(
            "the packed copy {packed_slash} would replace a sheet; choose another out"
        ));
    }
    let packed_file = assets.join(&packed_name);
    let meta_file = crate::texture_import::meta_path(&packed_file);
    if packed_file.exists() || meta_file.exists() {
        return Err(format!(
            "{packed_slash} or its meta is already there; remove it or choose another out"
        ));
    }

    let (grid, first_layers) = pack_layout(&layouts)?;
    let png = packed_png(&images, &layouts, &first_layers, grid, detected.tile)?;
    let meta = crate::image_sampling::tile_array_meta(None, detected.tile)
        .ok_or("no array meta could be made")?;
    let mut tileset = Tileset {
        image: packed_slash.clone(),
        tile_size: detected.tile,
        grid,
        sheets: (sheets.iter().zip(&layouts).zip(&first_layers))
            .map(|((sheet, layout), &first_layer)| TilesetSheet {
                source: sheet.name.clone(),
                margin: layout.margin,
                spacing: layout.spacing,
                columns: layout.columns,
                rows: layout.rows,
                first_layer,
            })
            .collect(),
        ..default()
    };
    let tiles: u32 = layouts.iter().map(GridLayout::tile_count).sum();
    let mut summary = format!(
        "{}: {tiles} tiles of {}x{} packed into {packed_slash}",
        out_name.to_slash_lossy(),
        detected.tile.x,
        detected.tile.y,
    );
    if grid.x * grid.y > WEBGL2_MAX_LAYERS {
        summary.push_str(&format!(
            "; more than {WEBGL2_MAX_LAYERS} layers, so WebGL2 cannot load it"
        ));
    }
    match choice {
        SetupChoice::Auto => {
            let inputs: Vec<(&str, RgbaImage)> =
                (sheets.iter().map(|s| s.name.as_str()).zip(images)).collect();
            let fresh =
                auto_setup_on_grid(&inputs, &detected, &[]).map_err(|err| err.to_string())?;
            (tileset.set_setup(&TilesetSetup::from_auto(&fresh))).map_err(|e| e.to_string())?;
            summary = format!("{summary}\n{}", fresh.report);
        }
        SetupChoice::Manual => tileset.setup_sheets = (0..sheets.len() as u32).collect(),
        SetupChoice::None => {}
    }

    let kind = (world
        .resource::<AssetKinds>()
        .by_name(TILESET_KIND)
        .cloned())
    .ok_or("the tileset kind is not registered")?;
    let handle = world.resource_mut::<Assets<Tileset>>().add(tileset);
    let value = AssetValue::Handle(handle.untyped());
    let text = asset_text(world, stem, &value).ok_or("the tileset has no text")?;
    jackdaw_bsn::parse_emitted(&text).map_err(|err| err.to_string())?;
    let document = jackdaw_bsn::document_bytes(&out, &text).map_err(|err| err.to_string())?;

    write_all(&[(&meta_file, &meta), (&packed_file, &png), (&out, &document)])
        .map_err(|err| format!("nothing was kept: {err}"))?;
    index_written(world, &out, &kind, value);
    if let Some(mut state) = world.get_resource_mut::<crate::project_window::ProjectWindowState>() {
        state.rebuild();
    }
    Ok(summary)
}

/// `path` as the index keys it, when it lies under the project's assets.
fn under_assets(world: &World, path: &Path) -> Result<PathBuf, String> {
    let canonical = assets_dir(world).and_then(|assets| dunce::canonicalize(assets).ok());
    let relative = (canonical.and_then(|assets| path.strip_prefix(assets).ok()))
        .map(Path::to_path_buf)
        .or_else(|| indexed_path(world, path));
    relative
        .filter(|relative| !relative.components().any(|c| c == Component::ParentDir))
        .ok_or_else(|| format!("{} is outside this project's assets", path.display()))
}

/// The asset path of the sheet `path` names.
fn sheet_name(world: &World, path: &Path) -> Result<String, String> {
    let file = resolve_scene_path(world, path)?;
    if !file.is_file() {
        return Err(format!("nothing at {}", path.display()));
    }
    let name = under_assets(world, &file)?.to_slash_lossy().into_owned();
    if !crate::texture_files::is_image_file_path(&file) {
        return Err(format!("{name} is not an image"));
    }
    Ok(name)
}

pub(crate) fn decode_sheet(assets: &Path, name: String) -> Result<Sheet, String> {
    let file = assets.join(&name);
    let decoded = image::open(&file).map_err(|err| format!("{name} is not an image: {err}"))?;
    let decoded = decoded.to_rgba8();
    Ok(Sheet {
        file,
        name,
        width: decoded.width(),
        height: decoded.height(),
        pixels: decoded.into_raw(),
    })
}

/// The tileset file `out` names, relative to the assets: a folder gets
/// `<first sheet>.tileset.bsn`, and any other name is a stem, less a trailing
/// `.bsn` or `.tileset`.
fn tileset_path(world: &World, out: Option<&Path>, first: &Sheet) -> Result<PathBuf, String> {
    let stem = first.file.file_stem().unwrap_or_default().to_string_lossy();
    let default_name = format!("{stem}{TILESET_SUFFIX}");
    let Some(out) = out else {
        return under_assets(world, &first.file.with_file_name(default_name));
    };
    let asks_for_folder = out.to_string_lossy().ends_with(['/', '\\']);
    let resolved = resolve_scene_path(world, out)?;
    let file = if asks_for_folder || (!names_a_file(&resolved) && resolved.is_dir()) {
        resolved.join(default_name)
    } else {
        let name = resolved.file_name().unwrap_or_default().to_string_lossy();
        let stem = name.strip_suffix(".bsn").unwrap_or(&name);
        let stem = stem.strip_suffix(".tileset").unwrap_or(stem);
        resolved.with_file_name(format!("{stem}{TILESET_SUFFIX}"))
    };
    under_assets(world, &file)
}

/// The packed grid, as wide as the widest sheet with each sheet starting a
/// row and at least two cells, and the layer of each sheet's first tile.
fn pack_layout(layouts: &[GridLayout]) -> Result<(UVec2, Vec<u32>), String> {
    let columns = layouts.iter().map(|l| l.columns).max().unwrap_or(1).max(1);
    let mut first_layers = Vec::with_capacity(layouts.len());
    let mut rows: u32 = 0;
    for layout in layouts {
        first_layers.push(rows.saturating_mul(columns));
        rows = rows.saturating_add(layout.tile_count().div_ceil(columns));
    }
    if rows.saturating_mul(columns) < 2 {
        rows += 1;
    }
    let layers = rows.saturating_mul(columns);
    if layers > MAX_LAYERS {
        return Err(format!(
            "the packed copy would need {layers} layers; an array texture holds at most \
             {MAX_LAYERS}, and WebGL2 at most {WEBGL2_MAX_LAYERS}"
        ));
    }
    Ok((UVec2::new(columns, rows), first_layers))
}

/// The tiles of `images` with no margin or spacing at their packed layers,
/// encoded as a PNG.
fn packed_png(
    images: &[RgbaImage],
    layouts: &[GridLayout],
    first_layers: &[u32],
    grid: UVec2,
    tile: UVec2,
) -> Result<Vec<u8>, String> {
    let size = grid * tile;
    let width = size.x as usize;
    let mut pixels = vec![0; width * size.y as usize * 4];
    let row_bytes = tile.x as usize * 4;
    for ((image, layout), first) in images.iter().zip(layouts).zip(first_layers) {
        for index in 0..layout.tile_count() {
            let (origin, _) = layout.tile_rect(index);
            let layer = first + index;
            let cell = UVec2::new(layer % grid.x, layer / grid.x) * tile;
            for y in 0..tile.y {
                let from = ((origin.y + y) * image.width + origin.x) as usize * 4;
                let to = ((cell.y + y) as usize * width + cell.x as usize) * 4;
                let source = (image.pixels.get(from..from + row_bytes))
                    .ok_or("a tile lies outside its sheet")?;
                let target = (pixels.get_mut(to..to + row_bytes))
                    .ok_or("a tile lies outside the packed copy")?;
                target.copy_from_slice(source);
            }
        }
    }
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&pixels, size.x, size.y, image::ExtendedColorType::Rgba8)
        .map_err(|err| err.to_string())?;
    Ok(png)
}

/// Write `files` in order. When one fails, the files already written and the
/// folders made for them are removed again.
fn write_all(files: &[(&Path, &[u8])]) -> std::io::Result<()> {
    let mut made: Vec<PathBuf> = Vec::new();
    let mut written: Vec<&Path> = Vec::new();
    for &(file, bytes) in files {
        let result = make_parent(file, &mut made).and_then(|()| write_atomic(file, bytes));
        if let Err(err) = result {
            for file in written.iter().rev() {
                let _ = std::fs::remove_file(file);
            }
            for folder in made.iter().rev() {
                let _ = std::fs::remove_dir(folder);
            }
            return Err(err);
        }
        written.push(file);
    }
    Ok(())
}

/// Make the folder `file` goes in, noting each folder made, outermost first.
fn make_parent(file: &Path, made: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let Some(parent) = file.parent() else {
        return Ok(());
    };
    let missing: Vec<PathBuf> = (parent.ancestors())
        .take_while(|folder| !folder.as_os_str().is_empty() && !folder.exists())
        .map(Path::to_path_buf)
        .collect();
    std::fs::create_dir_all(parent)?;
    made.extend(missing.into_iter().rev());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(columns: u32, rows: u32) -> GridLayout {
        GridLayout {
            tile: UVec2::splat(8),
            margin: UVec2::ZERO,
            spacing: UVec2::ZERO,
            columns,
            rows,
        }
    }

    #[test]
    fn sheets_start_rows_and_a_single_tile_still_makes_two_layers() {
        let (grid, firsts) = pack_layout(&[layout(4, 3), layout(3, 2)]).expect("fits");
        assert_eq!((grid, firsts), (UVec2::new(4, 5), vec![0, 12]));
        assert_eq!(
            pack_layout(&[layout(1, 1)]).expect("fits").0,
            UVec2::new(1, 2)
        );
    }

    #[test]
    fn more_layers_than_an_array_texture_holds_are_refused() {
        assert!(pack_layout(&[layout(32, 64)]).is_ok());
        let refused = pack_layout(&[layout(32, 64), layout(1, 1)]).expect_err("2080 layers");
        assert!(refused.contains("2048") && refused.contains("WebGL2 at most 256"));
    }

    #[test]
    fn a_failed_write_removes_what_was_written_before_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"a file, not a folder").expect("write");
        let first = dir.path().join("made/deeper/first.meta");
        let second = blocker.join("second.png");

        let result = write_all(&[(&first, b"1"), (&second, b"2")]);
        assert!(result.is_err());
        assert!(!dir.path().join("made").exists(), "its folders go too");
        assert!(blocker.is_file());
    }
}
