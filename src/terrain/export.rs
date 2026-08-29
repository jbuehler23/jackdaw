//! Headless CLI export: a `.bsn` scene's `Terrain` component and sidecar
//! data become a directory of PNG heightmaps/channels plus a JSON manifest
//! and a placements list, for an importer in another repo to consume.
//!
//! [`build_export`] is a pure function over plain data, with no scene and
//! no `World`, so its byte-for-byte output can be tested without a `.bsn`
//! fixture. [`run_export_terrain_cli`] is the shell that loads a scene
//! headless, reads its terrain sidecar, and writes what [`build_export`]
//! returns to disk.
//!
//! The output layout is a cross-repo contract: field names, key order, sort
//! order, and PNG pixel convention must not drift without updating that
//! contract.

use std::cmp::Ordering;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bevy::app::App;
use bevy::ecs::query::{With, Without};
use bevy::ecs::reflect::{AppTypeRegistry, ReflectComponent};
use bevy::ecs::world::World;
use bevy::prelude::{GlobalTransform, MinimalPlugins, Name, Transform};
use bevy::reflect::TypePath;
use bevy::reflect::serde::TypedReflectSerializer;
use image::{ImageBuffer, Luma};
use serde::Serialize;

use jackdaw_scene_types::{GltfSource, Terrain, TerrainChannelElement};

// --- Pure core: plain data in, file bytes out ---

/// Storage width of one exported channel's pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportChannelElement {
    U8,
    U16,
}

impl ExportChannelElement {
    fn bit_depth(self) -> u8 {
        match self {
            Self::U8 => 8,
            Self::U16 => 16,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::U8 => "u8",
            Self::U16 => "u16",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExportPaletteEntry {
    pub value: u16,
    pub label: String,
    /// Lowercase `#rrggbb`.
    pub color_hex: String,
}

#[derive(Clone, Debug)]
pub struct ExportChannel {
    pub name: String,
    pub element: ExportChannelElement,
    /// Row-major (z-major), `resolution^2` long, raw stored values.
    pub values: Vec<u16>,
    pub palette: Vec<ExportPaletteEntry>,
}

#[derive(Clone, Debug, Default)]
pub struct ExportPlacement {
    pub name: String,
    pub asset: Option<String>,
    pub translation: [f32; 3],
    /// `[x, y, z, w]`.
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    /// Keyed by short type path. Values are already reflect-serialized JSON.
    pub components: serde_json::Map<String, serde_json::Value>,
}

pub struct ExportInput<'a> {
    pub resolution: u32,
    /// Row-major (z-major), `resolution^2` long.
    pub heights: &'a [f32],
    pub channels: &'a [ExportChannel],
    /// `(size.x, size.y)`, the world-space XZ extent.
    pub size: (f32, f32),
    /// World-space XZ position of heightmap sample `(0, 0)`: the terrain
    /// entity's world translation plus the grid's anchor, since the mesh is
    /// built terrain-local (`jackdaw_terrain::mesh`) and a terrain's cells
    /// sit where its grid says rather than around its entity.
    pub terrain_origin_m: [f32; 2],
    pub max_height: f32,
    /// `Some((cell_size_m, elevation_step_m))` for a quantized export.
    pub quantization: Option<(f32, f32)>,
    pub source_scene: &'a str,
    pub placements: &'a [ExportPlacement],
    pub raw_heights: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExportError {
    HeightOutOfRange {
        height: f32,
        value: f64,
    },
    CellSizeMismatch {
        axis: &'static str,
        declared: f32,
        derived: f32,
    },
    ChannelNameCollision(String, String),
    DuplicateChannelName(String),
    Encode(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HeightOutOfRange { height, value } => write!(
                f,
                "height {height} quantizes to {value}, outside the representable 0..=65535 \
                 pixel range"
            ),
            Self::CellSizeMismatch {
                axis,
                declared,
                derived,
            } => write!(
                f,
                "declared cell size {declared} does not match {axis} / (resolution - 1) = \
                 {derived}"
            ),
            Self::ChannelNameCollision(a, b) => write!(
                f,
                "channel names {a:?} and {b:?} both sanitize to the same export filename"
            ),
            Self::DuplicateChannelName(name) => write!(
                f,
                "channel name {name:?} is used by more than one channel; \
                 rename one before exporting"
            ),
            Self::Encode(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ExportError {}

/// One exported file, path relative to the export's `--out` directory.
#[derive(Debug)]
pub struct ExportedFile {
    pub relative_path: PathBuf,
    pub bytes: Vec<u8>,
}

/// Build every file a terrain export produces, from plain in-memory data.
///
/// No filesystem and no `World`. The same input produces byte-identical
/// output: every collection feeding an output file is sorted before it is
/// written.
pub fn build_export(input: &ExportInput) -> Result<Vec<ExportedFile>, ExportError> {
    if let Some((cell_size, _)) = input.quantization {
        validate_cell_size(input.size, input.resolution, cell_size)?;
    }

    let mut snapped_heights = input.heights.to_vec();
    let step_m = match input.quantization {
        Some((_, elevation_step)) => {
            jackdaw_terrain::quantize_heights(&mut snapped_heights, elevation_step);
            elevation_step
        }
        // `max_height` is a configured ceiling rather than the authored
        // span, which sculpted heights can exceed or run below, so a step
        // sized off it can overflow the u16 range. The step comes from the
        // min..max span instead.
        None => {
            let min = snapped_heights
                .iter()
                .copied()
                .fold(f32::INFINITY, f32::min);
            let max = snapped_heights
                .iter()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max);
            let span = if min.is_finite() && max.is_finite() {
                max - min
            } else {
                0.0
            };
            if span > 0.0 { span / 65535.0 } else { 1.0 }
        }
    };

    let min_height = snapped_heights
        .iter()
        .copied()
        .fold(f32::INFINITY, f32::min);
    let min_height = if min_height.is_finite() {
        min_height
    } else {
        0.0
    };
    let base_m = (min_height / step_m).floor() * step_m;

    let mut height_pixels = Vec::with_capacity(snapped_heights.len());
    for &h in &snapped_heights {
        let raw = (f64::from(h) - f64::from(base_m)) / f64::from(step_m);
        let value = raw.round();
        if !(0.0..=65535.0).contains(&value) {
            return Err(ExportError::HeightOutOfRange { height: h, value });
        }
        height_pixels.push(value as u16);
    }
    let heightmap_bytes = encode_png_u16(input.resolution, input.resolution, &height_pixels)?;

    let mut files = Vec::new();
    files.push(ExportedFile {
        relative_path: PathBuf::from("heightmap.png"),
        bytes: heightmap_bytes,
    });

    let mut ordered_channels: Vec<&ExportChannel> = input.channels.iter().collect();
    ordered_channels.sort_by(|a, b| a.name.cmp(&b.name));

    let mut filename_owners: Vec<(String, String)> = Vec::new();
    let mut manifest_channels = Vec::with_capacity(ordered_channels.len());
    for channel in &ordered_channels {
        let filename = sanitize_channel_filename(&channel.name);
        if let Some((_, owner)) = filename_owners.iter().find(|(f, _)| *f == filename) {
            if owner == &channel.name {
                // Two channels with the same name write one output file,
                // and last-wins would drop one.
                return Err(ExportError::DuplicateChannelName(channel.name.clone()));
            }
            return Err(ExportError::ChannelNameCollision(
                owner.clone(),
                channel.name.clone(),
            ));
        }
        filename_owners.push((filename.clone(), channel.name.clone()));

        let relative_path = format!("channels/{filename}.png");
        let bytes = match channel.element {
            ExportChannelElement::U16 => {
                encode_png_u16(input.resolution, input.resolution, &channel.values)?
            }
            ExportChannelElement::U8 => {
                // Saturate rather than wrap, matching sidecar.rs's U8
                // encode: wrapping would turn 256 into palette index 0.
                let values8: Vec<u8> = channel
                    .values
                    .iter()
                    .map(|&v| v.min(u16::from(u8::MAX)) as u8)
                    .collect();
                encode_png_u8(input.resolution, input.resolution, &values8)?
            }
        };
        files.push(ExportedFile {
            relative_path: PathBuf::from(&relative_path),
            bytes,
        });

        let mut palette = channel.palette.clone();
        palette.sort_by_key(|entry| entry.value);
        manifest_channels.push(ManifestChannel {
            name: channel.name.clone(),
            file: relative_path,
            bit_depth: channel.element.bit_depth(),
            element: channel.element.label(),
            palette: palette
                .into_iter()
                .map(|entry| ManifestPaletteEntry {
                    value: entry.value,
                    label: entry.label,
                    color: entry.color_hex,
                })
                .collect(),
        });
    }

    let manifest = Manifest {
        format: "jackdaw-terrain-export",
        format_version: 2,
        source_scene: input.source_scene.to_string(),
        terrain: ManifestTerrain {
            cells_per_edge: input.resolution,
            world_size_m: [input.size.0, input.size.1],
            max_height_m: input.max_height,
            terrain_origin_m: input.terrain_origin_m,
        },
        quantization: input.quantization.map(|(cell_size_m, elevation_step_m)| {
            ManifestQuantization {
                cell_size_m,
                elevation_step_m,
            }
        }),
        heightmap: ManifestHeightmap {
            file: "heightmap.png",
            bit_depth: 16,
            encoding: "unsigned-steps-from-base",
            base_m,
            step_m,
        },
        channels: manifest_channels,
        placements_file: "placements.json",
    };
    files.push(ExportedFile {
        relative_path: PathBuf::from("manifest.json"),
        bytes: to_json_bytes(&manifest).map_err(|e| ExportError::Encode(e.to_string()))?,
    });

    let mut ordered_placements: Vec<&ExportPlacement> = input.placements.iter().collect();
    ordered_placements.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.asset.cmp(&b.asset))
            .then_with(|| cmp_f32_array(&a.translation, &b.translation))
    });
    let placements_doc = PlacementsFile {
        format_version: 1,
        placements: ordered_placements
            .into_iter()
            .map(|p| PlacementEntry {
                name: p.name.clone(),
                asset: p.asset.clone(),
                translation_m: p.translation,
                rotation_quat: p.rotation,
                scale: p.scale,
                components: p.components.clone(),
            })
            .collect(),
    };
    files.push(ExportedFile {
        relative_path: PathBuf::from("placements.json"),
        bytes: to_json_bytes(&placements_doc).map_err(|e| ExportError::Encode(e.to_string()))?,
    });

    if input.raw_heights {
        let mut buf = Vec::with_capacity(input.heights.len() * 4);
        for h in input.heights {
            buf.extend_from_slice(&h.to_le_bytes());
        }
        files.push(ExportedFile {
            relative_path: PathBuf::from("heights.f32"),
            bytes: buf,
        });
    }

    Ok(files)
}

fn validate_cell_size(size: (f32, f32), resolution: u32, declared: f32) -> Result<(), ExportError> {
    if resolution < 2 {
        return Ok(());
    }
    let denom = (resolution - 1) as f32;
    let derived_x = size.0 / denom;
    let derived_y = size.1 / denom;
    if (derived_x - declared).abs() > 1e-6 {
        return Err(ExportError::CellSizeMismatch {
            axis: "size.x",
            declared,
            derived: derived_x,
        });
    }
    if (derived_y - declared).abs() > 1e-6 {
        return Err(ExportError::CellSizeMismatch {
            axis: "size.y",
            declared,
            derived: derived_y,
        });
    }
    Ok(())
}

/// Any character outside `[A-Za-z0-9_-]` becomes `_`.
fn sanitize_channel_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn cmp_f32_array(a: &[f32; 3], b: &[f32; 3]) -> Ordering {
    for i in 0..3 {
        let ord = a[i].total_cmp(&b[i]);
        if ord != Ordering::Equal {
            return ord;
        }
    }
    Ordering::Equal
}

fn encode_png_u16(width: u32, height: u32, pixels: &[u16]) -> Result<Vec<u8>, ExportError> {
    let buffer: ImageBuffer<Luma<u16>, Vec<u16>> =
        ImageBuffer::from_raw(width, height, pixels.to_vec())
            .ok_or_else(|| ExportError::Encode("pixel buffer size mismatch".to_string()))?;
    let mut out = Cursor::new(Vec::new());
    buffer
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| ExportError::Encode(e.to_string()))?;
    Ok(out.into_inner())
}

fn encode_png_u8(width: u32, height: u32, pixels: &[u8]) -> Result<Vec<u8>, ExportError> {
    let buffer: ImageBuffer<Luma<u8>, Vec<u8>> =
        ImageBuffer::from_raw(width, height, pixels.to_vec())
            .ok_or_else(|| ExportError::Encode("pixel buffer size mismatch".to_string()))?;
    let mut out = Cursor::new(Vec::new());
    buffer
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| ExportError::Encode(e.to_string()))?;
    Ok(out.into_inner())
}

fn to_json_bytes<T: Serialize>(value: &T) -> serde_json::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

// --- Manifest / placements document shapes (exact key order matters) ---

#[derive(Serialize)]
struct Manifest {
    format: &'static str,
    format_version: u32,
    source_scene: String,
    terrain: ManifestTerrain,
    quantization: Option<ManifestQuantization>,
    heightmap: ManifestHeightmap,
    channels: Vec<ManifestChannel>,
    placements_file: &'static str,
}

#[derive(Serialize)]
struct ManifestTerrain {
    /// Cells per edge of the grid this export wrote. A terrain declares no
    /// resolution; this is how many cells it stores.
    cells_per_edge: u32,
    world_size_m: [f32; 2],
    max_height_m: f32,
    terrain_origin_m: [f32; 2],
}

#[derive(Serialize)]
struct ManifestQuantization {
    cell_size_m: f32,
    elevation_step_m: f32,
}

#[derive(Serialize)]
struct ManifestHeightmap {
    file: &'static str,
    bit_depth: u8,
    encoding: &'static str,
    base_m: f32,
    step_m: f32,
}

#[derive(Serialize)]
struct ManifestChannel {
    name: String,
    file: String,
    bit_depth: u8,
    element: &'static str,
    palette: Vec<ManifestPaletteEntry>,
}

#[derive(Serialize)]
struct ManifestPaletteEntry {
    value: u16,
    label: String,
    color: String,
}

#[derive(Serialize)]
struct PlacementsFile {
    format_version: u32,
    placements: Vec<PlacementEntry>,
}

#[derive(Serialize)]
struct PlacementEntry {
    name: String,
    asset: Option<String>,
    translation_m: [f32; 3],
    rotation_quat: [f32; 4],
    scale: [f32; 3],
    components: serde_json::Map<String, serde_json::Value>,
}

// --- Shell: load a scene, read its sidecar, write the export to disk ---

/// `jd export-terrain <scene.bsn> --out <dir> [--cell-size <m>] \
/// [--elevation-step <m>] [--raw-heights]`
#[expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "CLI results are written to stdout and errors to stderr"
)]
pub fn run_export_terrain_cli(args: &[String]) -> ExitCode {
    match run(args) {
        Ok(summary) => {
            println!("{summary}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("jd export-terrain: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<String, String> {
    let scene_arg = first_positional(args)
        .ok_or("usage: jd export-terrain <scene.bsn> --out <dir>")?
        .clone();
    let scene_path = PathBuf::from(&scene_arg);
    if !scene_path.exists() {
        return Err(format!(
            "{}: scene file does not exist",
            scene_path.display()
        ));
    }

    let out_dir = flag_value(args, "--out")
        .map(PathBuf::from)
        .ok_or("--out <dir> is required")?;
    let cell_size_flag = parse_f32_flag(args, "--cell-size")?;
    let elevation_step_flag = parse_f32_flag(args, "--elevation-step")?;
    let raw_heights = args.iter().any(|a| a == "--raw-heights");

    let scene_text = std::fs::read_to_string(&scene_path)
        .map_err(|e| format!("{}: {e}", scene_path.display()))?;

    let mut app = load_export_app(&scene_text)
        .map_err(|error| format!("{}: {error}", scene_path.display()))?;

    let world = app.world_mut();
    let terrain_entity = world
        .query_filtered::<bevy::ecs::entity::Entity, With<Terrain>>()
        .iter(world)
        .next()
        .ok_or_else(|| format!("{}: scene has no Terrain component", scene_path.display()))?;
    let terrain = world
        .get::<Terrain>(terrain_entity)
        .expect("queried entity carries Terrain")
        .clone();
    let (quantization, quantization_source) =
        resolve_quantization(&terrain, cell_size_flag, elevation_step_flag)?;
    let quantization_report = match (quantization_source, quantization) {
        (QuantizationSource::Terrain, Some((cell_size, step))) => {
            format!("quantization: from terrain (cell_size {cell_size} m, step {step} m)")
        }
        (QuantizationSource::Flags, Some((cell_size, step))) => {
            format!("quantization: from flags (cell_size {cell_size} m, step {step} m)")
        }
        _ => "quantization: none".to_string(),
    };

    let scene_dir = scene_path.parent().unwrap_or_else(|| Path::new("."));
    let (terrain_data, grid) = load_terrain_data(&terrain, scene_dir)?;
    let cells = (terrain_data.resolution as usize) * (terrain_data.resolution as usize);
    if terrain_data.heights.len() != cells {
        return Err(format!(
            "{}: terrain data has {} height values, expected {cells} for the {} cells \
             per edge it stores",
            scene_path.display(),
            terrain_data.heights.len(),
            terrain_data.resolution,
        ));
    }

    let channels: Vec<ExportChannel> = terrain
        .channels
        .iter()
        .map(|descriptor| {
            let values = terrain_data
                .channels
                .iter()
                .find(|c| c.name == descriptor.name)
                .map(|c| c.values.clone())
                .unwrap_or_else(|| vec![0u16; cells]);
            ExportChannel {
                name: descriptor.name.clone(),
                element: match descriptor.element {
                    TerrainChannelElement::U8 => ExportChannelElement::U8,
                    TerrainChannelElement::U16 => ExportChannelElement::U16,
                },
                values,
                palette: descriptor
                    .palette
                    .iter()
                    .map(|entry| ExportPaletteEntry {
                        value: entry.value,
                        label: entry.label.clone(),
                        color_hex: color_to_hex(entry.color),
                    })
                    .collect(),
            }
        })
        .collect();

    // Extent is derived rather than declared: the cells the terrain stores,
    // at the size those cells are drawn. The origin is where its first cell
    // sits, the entity plus the grid's anchor.
    let world_size = (terrain_data.resolution.saturating_sub(1)) as f32 * grid.cell_size;
    let terrain_origin_m = terrain_origin_for(world, terrain_entity, grid.anchor)?;

    let placements = collect_placements(app.world_mut(), terrain_entity);
    let input = ExportInput {
        resolution: terrain_data.resolution,
        heights: &terrain_data.heights,
        channels: &channels,
        size: (world_size, world_size),
        terrain_origin_m,
        max_height: terrain.max_height,
        quantization,
        source_scene: &scene_arg,
        placements: &placements,
        raw_heights,
    };
    let files = build_export(&input).map_err(|e| e.to_string())?;

    std::fs::create_dir_all(&out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    for file in &files {
        let dest = out_dir.join(&file.relative_path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&dest, &file.bytes).map_err(|e| format!("{}: {e}", dest.display()))?;
    }

    Ok(format!(
        "{quantization_report}\nexported {} file(s) to {}",
        files.len(),
        out_dir.display()
    ))
}

/// Load a scene into the same headless app the CLI uses and bring derived
/// transforms up to date before any export query reads them.
fn load_export_app(scene_text: &str) -> Result<App, String> {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(bevy::asset::AssetPlugin::default());
    app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    app.add_plugins(jackdaw_runtime::JackdawPlugin);
    app.register_type::<Name>();
    app.register_type::<Transform>();

    jackdaw_bsn::load_bsn_scene(app.world_mut(), scene_text).map_err(|error| error.to_string())?;
    app.update();
    Ok(app)
}

/// Where a completed export's quantization setting came from, for the
/// CLI's stdout report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QuantizationSource {
    Terrain,
    Flags,
    None,
}

fn first_positional(args: &[String]) -> Option<&String> {
    const VALUE_FLAGS: [&str; 3] = ["--out", "--cell-size", "--elevation-step"];
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if VALUE_FLAGS.contains(&arg.as_str()) {
            skip_next = true;
            continue;
        }
        if !arg.starts_with('-') {
            return Some(arg);
        }
    }
    None
}

fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn parse_f32_flag(args: &[String], name: &str) -> Result<Option<f32>, String> {
    match flag_value(args, name) {
        Some(raw) => raw
            .parse::<f32>()
            .map(Some)
            .map_err(|_| format!("{name} expects a number, got {raw:?}")),
        None => Ok(None),
    }
}

/// Resolve the export's quantization and where it came from.
///
/// Precedence: the terrain's authored
/// [`TerrainQuantization`](jackdaw_scene_types::TerrainQuantization) wins
/// when enabled (both `cell_size` and `height_step` positive).
/// `--cell-size`/`--elevation-step` are the fallback. Giving exactly one
/// flag is rejected; giving both while the terrain also declares
/// quantization is accepted only when they agree (within 1e-6).
fn resolve_quantization(
    terrain: &Terrain,
    cell_size_flag: Option<f32>,
    elevation_step_flag: Option<f32>,
) -> Result<(Option<(f32, f32)>, QuantizationSource), String> {
    let terrain_quant = {
        let q = &terrain.quantization;
        if q.enabled
            && q.cell_size.is_finite()
            && q.cell_size > 0.0
            && q.height_step.is_finite()
            && q.height_step > 0.0
        {
            Some((q.cell_size, q.height_step))
        } else {
            None
        }
    };

    let flags = match (cell_size_flag, elevation_step_flag) {
        (Some(cell_size), Some(elevation_step)) => Some((cell_size, elevation_step)),
        (None, None) => None,
        _ => {
            return Err(
                "--cell-size and --elevation-step must be given together, or neither".to_string(),
            );
        }
    };

    match (terrain_quant, flags) {
        (Some(t), Some(f)) => {
            if (t.0 - f.0).abs() > 1e-6 || (t.1 - f.1).abs() > 1e-6 {
                return Err(format!(
                    "terrain declares quantization (cell_size {} m, elevation_step {} m) but \
                     --cell-size/--elevation-step declare ({} m, {} m); these disagree",
                    t.0, t.1, f.0, f.1
                ));
            }
            Ok((Some(t), QuantizationSource::Terrain))
        }
        (Some(t), None) => Ok((Some(t), QuantizationSource::Terrain)),
        (None, Some(f)) => Ok((Some(f), QuantizationSource::Flags)),
        (None, None) => Ok((None, QuantizationSource::None)),
    }
}

/// The dense grid this export writes, and the geometry it was drawn at.
///
/// The geometry travels with the data because a terrain's extent is the
/// cells it stores: how much ground the export covers is that count times
/// the cell size, and where it starts is the grid's anchor.
fn load_terrain_data(
    terrain: &Terrain,
    scene_dir: &Path,
) -> Result<(jackdaw_terrain::TerrainData, jackdaw_terrain::GridGeometry), String> {
    if !terrain.data_path.is_empty() {
        let sidecar_path = jackdaw_terrain::sidecar::resolve_path(scene_dir, &terrain.data_path)
            .map_err(|err| format!("invalid terrain data path {:?}: {err}", terrain.data_path))?;
        match std::fs::read(&sidecar_path) {
            Ok(bytes) => {
                // Either format version; a pre-region sidecar migrates on
                // the way in. The export pipeline is dense, so it reads the
                // document's editing window and writes no control or colour
                // layer.
                let document = jackdaw_terrain::sidecar::load(&bytes)
                    .map_err(|e| format!("{}: {e}", sidecar_path.display()))?;
                // What the dense pipeline cannot represent is an error
                // rather than a warning: exporting zeroes, or one region
                // out of several, ships a wrong artifact that reads as a
                // right one. Extent runs from the origin outwards, so a
                // region at a negative coordinate is ground this dense grid
                // has no index for; everything else is inside by
                // construction.
                let outside = document
                    .regions
                    .iter_sorted()
                    .any(|(coord, _)| coord.x < 0 || coord.z < 0);
                if outside {
                    return Err(format!(
                        "{}: terrain holds regions at negative coordinates and this export \
                         writes one dense grid from the origin; exporting them is not \
                         supported yet",
                        sidecar_path.display(),
                    ));
                }
                let resolution = document.grid_resolution();
                let channels = document
                    .channels
                    .iter()
                    .enumerate()
                    .map(|(index, descriptor)| jackdaw_terrain::ChannelData {
                        name: descriptor.name.clone(),
                        element: descriptor.element,
                        values: document.regions.read_grid_channel(index, resolution),
                    })
                    .collect();
                let mut data = jackdaw_terrain::TerrainData {
                    resolution,
                    heights: document.grid_heights(),
                    channels,
                };
                data.normalize();
                let grid = jackdaw_terrain::sidecar::resolve_grid(
                    document.grid,
                    terrain.size,
                    terrain.resolution,
                );
                return Ok((data, grid));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !terrain.heights.is_empty() => {
                // Legacy scene with no sidecar; falls through to the inline
                // heights below.
            }
            Err(e) => {
                return Err(format!(
                    "{}: terrain sidecar unreadable: {e}",
                    sidecar_path.display()
                ));
            }
        }
    }

    if terrain.heights.is_empty() {
        return Err("terrain has no data_path sidecar and no legacy inline heights".to_string());
    }
    // Legacy inline heights: their grid is the rectangle the component
    // declares, which is what the migration reads.
    Ok((
        jackdaw_terrain::TerrainData {
            resolution: terrain.resolution,
            heights: terrain.heights.clone(),
            channels: Vec::new(),
        },
        jackdaw_terrain::GridGeometry::for_declared_rect(terrain.size, terrain.resolution),
    ))
}

fn color_to_hex(color: bevy::color::Color) -> String {
    use bevy::color::ColorToPacked;
    let [r, g, b] = color.to_srgba().to_u8_array_no_alpha();
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// The transform an exported entity occupies in scene space.
///
/// Runtime scene loading computes `GlobalTransform` parent-first. A local
/// fallback keeps isolated worlds and older hand-built fixtures usable.
fn transform_for_export(local: Transform, global: Option<&GlobalTransform>) -> Transform {
    global
        .map(GlobalTransform::compute_transform)
        .unwrap_or(local)
}

fn resolved_transform(world: &World, entity: bevy::ecs::entity::Entity) -> Transform {
    transform_for_export(
        world.get::<Transform>(entity).copied().unwrap_or_default(),
        world.get::<GlobalTransform>(entity),
    )
}

/// World-space XZ origin for heightmap sample `(0, 0)`.
///
/// The export contract is an axis-aligned heightfield and carries no
/// terrain orientation or scale basis, so a transform it cannot represent
/// is rejected rather than written out as coordinates that appear valid.
fn terrain_origin_for(
    world: &World,
    entity: bevy::ecs::entity::Entity,
    anchor: bevy::math::Vec2,
) -> Result<[f32; 2], String> {
    const EPSILON: f32 = 1e-5;

    let transform = resolved_transform(world, entity);
    let identity_alignment = transform.rotation.dot(bevy::math::Quat::IDENTITY).abs();
    if 1.0 - identity_alignment > EPSILON {
        return Err(
            "terrain export cannot represent terrain rotation; apply the rotation before export"
                .to_string(),
        );
    }
    if transform
        .scale
        .to_array()
        .into_iter()
        .any(|axis| (axis - 1.0).abs() > EPSILON)
    {
        return Err(
            "terrain export cannot represent terrain scale; apply the scale before export"
                .to_string(),
        );
    }

    Ok([
        transform.translation.x + anchor.x,
        transform.translation.z + anchor.y,
    ])
}

fn collect_placements(
    world: &mut World,
    terrain_entity: bevy::ecs::entity::Entity,
) -> Vec<ExportPlacement> {
    let excluded = [
        Terrain::type_path(),
        Transform::type_path(),
        GlobalTransform::type_path(),
        Name::type_path(),
        GltfSource::type_path(),
    ];

    let registry = world.resource::<AppTypeRegistry>().clone();
    let reg = registry.read();

    let mut query = world.query_filtered::<(
        bevy::ecs::entity::Entity,
        &Transform,
        Option<&GlobalTransform>,
    ), Without<Terrain>>();
    let entities: Vec<(bevy::ecs::entity::Entity, Transform)> = query
        .iter(world)
        .map(|(entity, local, global)| (entity, transform_for_export(*local, global)))
        .collect();

    let components = world.components();
    let mut placements = Vec::new();
    for (entity, transform) in entities {
        if entity == terrain_entity {
            continue;
        }
        let name = world
            .get::<Name>(entity)
            .map(|n| n.as_str().to_string())
            .unwrap_or_default();
        let asset = world.get::<GltfSource>(entity).map(|g| g.path.clone());

        let mut other_components = serde_json::Map::new();
        if let Ok(entity_ref) = world.get_entity(entity) {
            for component_id in entity_ref.archetype().iter_components() {
                let Some(info) = components.get_info(component_id) else {
                    continue;
                };
                let Some(type_id) = info.type_id() else {
                    continue;
                };
                let Some(registration) = reg.get(type_id) else {
                    continue;
                };
                let full_path = registration.type_info().type_path_table().path();
                if excluded.contains(&full_path) {
                    continue;
                }
                let Some(reflect_component) = registration.data::<ReflectComponent>() else {
                    continue;
                };
                let Some(component_ref) = reflect_component.reflect(entity_ref) else {
                    continue;
                };
                let serializer = TypedReflectSerializer::new(component_ref, &reg);
                if let Ok(value) = serde_json::to_value(&serializer) {
                    let short = registration.type_info().type_path_table().short_path();
                    other_components.insert(short.to_string(), value);
                }
            }
        }

        placements.push(ExportPlacement {
            name,
            asset,
            translation: transform.translation.to_array(),
            rotation: transform.rotation.to_array(),
            scale: transform.scale.to_array(),
            components: other_components,
        });
    }
    placements
}

#[cfg(test)]
mod tests {
    use bevy::prelude::*;

    use super::*;

    fn terrain_with_quantization(enabled: bool, cell_size: f32, height_step: f32) -> Terrain {
        Terrain {
            quantization: jackdaw_scene_types::TerrainQuantization {
                enabled,
                cell_size,
                height_step,
            },
            ..Terrain::default()
        }
    }

    #[test]
    fn terrain_quantization_wins_when_enabled() {
        let terrain = terrain_with_quantization(true, 1.0, 0.25);
        let (quantization, source) = resolve_quantization(&terrain, None, None).unwrap();
        assert_eq!(quantization, Some((1.0, 0.25)));
        assert_eq!(source, QuantizationSource::Terrain);
    }

    #[test]
    fn flags_are_used_when_terrain_quantization_is_off() {
        let terrain = Terrain::default();
        let (quantization, source) = resolve_quantization(&terrain, Some(2.0), Some(0.5)).unwrap();
        assert_eq!(quantization, Some((2.0, 0.5)));
        assert_eq!(source, QuantizationSource::Flags);
    }

    #[test]
    fn neither_terrain_nor_flags_is_unquantized() {
        let terrain = Terrain::default();
        let (quantization, source) = resolve_quantization(&terrain, None, None).unwrap();
        assert_eq!(quantization, None);
        assert_eq!(source, QuantizationSource::None);
    }

    #[test]
    fn agreeing_terrain_and_flags_are_accepted_as_terrain_sourced() {
        let terrain = terrain_with_quantization(true, 1.0, 0.25);
        let (quantization, source) = resolve_quantization(&terrain, Some(1.0), Some(0.25)).unwrap();
        assert_eq!(quantization, Some((1.0, 0.25)));
        assert_eq!(source, QuantizationSource::Terrain);
    }

    #[test]
    fn disagreeing_terrain_and_flags_are_refused_naming_both() {
        let terrain = terrain_with_quantization(true, 1.0, 0.25);
        let error = resolve_quantization(&terrain, Some(2.0), Some(0.25)).unwrap_err();
        assert!(error.contains('1'), "names the terrain value: {error}");
        assert!(error.contains('2'), "names the flag value: {error}");
    }

    #[test]
    fn one_flag_without_the_other_is_rejected() {
        let terrain = Terrain::default();
        let error = resolve_quantization(&terrain, Some(1.0), None).unwrap_err();
        assert!(error.contains("together"));
    }

    #[test]
    fn terrain_quantization_disabled_falls_through_to_flags_even_with_stale_values() {
        // `enabled: false` means off whatever the other two fields hold.
        let terrain = terrain_with_quantization(false, 1.0, 0.25);
        let (quantization, source) = resolve_quantization(&terrain, Some(3.0), Some(0.75)).unwrap();
        assert_eq!(quantization, Some((3.0, 0.75)));
        assert_eq!(source, QuantizationSource::Flags);
    }

    fn assert_array_close(actual: [f32; 3], expected: [f32; 3]) {
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!((actual - expected).abs() < 1e-5, "{actual} != {expected}");
        }
    }

    #[test]
    fn placements_export_the_propagated_world_transform() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::transform::TransformPlugin));
        app.init_resource::<AppTypeRegistry>();
        app.register_type::<Name>();
        app.register_type::<Transform>();
        app.register_type::<GlobalTransform>();

        let parent = app
            .world_mut()
            .spawn((
                Transform {
                    translation: Vec3::new(10.0, 0.0, -2.0),
                    rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
                    scale: Vec3::splat(2.0),
                },
                GlobalTransform::default(),
            ))
            .id();
        let child = app
            .world_mut()
            .spawn((
                Name::new("child"),
                Transform {
                    translation: Vec3::new(1.0, 2.0, 3.0),
                    rotation: Quat::from_rotation_y(0.25),
                    scale: Vec3::splat(0.5),
                },
                GlobalTransform::default(),
                ChildOf(parent),
            ))
            .id();
        let terrain_entity = app.world_mut().spawn(Terrain::default()).id();

        app.update();
        let expected = app
            .world()
            .get::<GlobalTransform>(child)
            .expect("transform propagated")
            .compute_transform();
        let placements = collect_placements(app.world_mut(), terrain_entity);
        let placement = placements
            .iter()
            .find(|placement| placement.name == "child")
            .expect("child exported");

        assert_array_close(placement.translation, expected.translation.to_array());
        assert_array_close(placement.scale, expected.scale.to_array());
        for (actual, expected) in placement
            .rotation
            .into_iter()
            .zip(expected.rotation.to_array())
        {
            assert!((actual - expected).abs() < 1e-5, "{actual} != {expected}");
        }
        assert!(
            placement
                .components
                .keys()
                .all(|name| !name.contains("GlobalTransform")),
            "runtime world transform must not be exported as metadata",
        );
    }

    #[test]
    fn terrain_origin_uses_world_translation_and_rejects_unrepresentable_transforms() {
        let mut world = World::new();
        let entity = world
            .spawn((
                Transform::from_xyz(1.0, 0.0, 2.0),
                GlobalTransform::from(Transform::from_xyz(11.0, 4.0, -7.0)),
            ))
            .id();
        // Where the grid's first cell sits relative to the entity. A
        // migrated terrain carries `-size/2` here.
        let anchor = Vec2::new(-2.0, -3.0);

        assert_eq!(terrain_origin_for(&world, entity, anchor), Ok([9.0, -10.0]));

        world
            .entity_mut(entity)
            .insert(GlobalTransform::from(Transform {
                translation: Vec3::new(11.0, 4.0, -7.0),
                rotation: Quat::from_rotation_y(0.25),
                ..default()
            }));
        assert!(
            terrain_origin_for(&world, entity, anchor)
                .unwrap_err()
                .contains("rotation")
        );

        world
            .entity_mut(entity)
            .insert(GlobalTransform::from(Transform {
                translation: Vec3::new(11.0, 4.0, -7.0),
                scale: Vec3::new(2.0, 1.0, 1.0),
                ..default()
            }));
        assert!(
            terrain_origin_for(&world, entity, anchor)
                .unwrap_err()
                .contains("scale")
        );
    }

    #[test]
    fn loaded_scene_propagates_parent_rotation_before_export_reads_transforms() {
        let mut source = App::new();
        source
            .register_type::<Name>()
            .register_type::<Transform>()
            .register_type::<ChildOf>()
            .register_type::<Children>()
            .register_type::<Terrain>();

        let parent = source
            .world_mut()
            .spawn((
                Name::new("parent"),
                Transform::from_rotation(Quat::from_rotation_y(0.25)),
            ))
            .id();
        source.world_mut().spawn((
            Name::new("terrain"),
            Transform::from_xyz(3.0, 0.0, 5.0),
            Terrain::default(),
            ChildOf(parent),
        ));
        let scene = jackdaw_bsn::serialize_to_bsn(source.world());

        let loaded = load_export_app(&scene).expect("scene reloads through the export shell");
        let world = loaded.world();
        let terrain_entity = world
            .iter_entities()
            .find(bevy::prelude::EntityRef::contains::<Terrain>)
            .expect("terrain reloads")
            .id();
        assert!(
            terrain_origin_for(world, terrain_entity, Vec2::ZERO)
                .unwrap_err()
                .contains("rotation"),
            "the inherited rotation must be propagated before export",
        );
    }

    #[test]
    fn a_missing_global_transform_falls_back_to_local_space() {
        let local = Transform::from_xyz(3.0, 4.0, 5.0);

        assert_eq!(transform_for_export(local, None), local);
    }
}
