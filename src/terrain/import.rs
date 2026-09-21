//! Importing a terrain from images: the `terrain.import` operator, the
//! Terrain panel's picked heightmap, and the file dialog behind it.
//!
//! Sculpting and generation make a terrain here. This takes one that already
//! exists somewhere else -- a painted heightmap, a scan, a terrain exported
//! from another tool -- and lays it over the selected terrain's grid, with
//! one greyscale image per material slot optionally painting what draws
//! where, one per scatter mask saying where things stand, and one per
//! detail layer saying where its grass grows.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
use jackdaw_api::prelude::*;
use jackdaw_commands::{CommandGroup, EditorCommand};
use jackdaw_scene_types::{TerrainChannel, TerrainChannelElement};
use jackdaw_terrain::import::{
    ChannelPaint, GreyImage, HeightRange, SlotWeights, heights_from_image,
};
use path_slash::PathExt as _;

use super::TerrainDataStore;
use super::detail_ops::AddTerrainChannel;
use super::ops::{FRESH_TERRAIN_REGIONS, has_selected_terrain};
use super::paint::{SetTerrainChannel, SetTerrainControl};
use super::sculpt::SetTerrainHeights;
use super::shape_ops::{SetTerrainShape, clamp_cell_size};
use crate::commands::CommandHistory;
use crate::project::ProjectRoot;
use crate::selection::Selection;

/// What one undo entry covering a whole import is called.
const LABEL: &str = "Import Terrain";

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<TerrainImportState>().add_systems(
        Update,
        poll_heightmap_pick.run_if(resource_exists::<HeightmapPick>),
    );
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TerrainImportPickOp>()
        .register_operator::<TerrainImportOp>();
}

/// The heightmap the Terrain panel has picked and the world heights its
/// black and white ends stand for.
///
/// Persistent like [`super::panel::TerrainGenerateState`] beside it, so the
/// range survives a panel rebuild and a second import of the same file
/// costs one click.
#[derive(Resource)]
pub struct TerrainImportState {
    /// The picked heightmap, as a path under the project's assets. Empty
    /// until something is picked.
    pub heightmap: String,
    /// World height the heightmap's black end stands at.
    pub min: f32,
    /// World height its white end stands at.
    pub max: f32,
}

impl Default for TerrainImportState {
    fn default() -> Self {
        Self {
            heightmap: String::new(),
            min: 0.0,
            max: 100.0,
        }
    }
}

/// Shallowest and deepest a height range may be scrubbed to in the panel.
/// A range past this is still reachable through the operator.
pub const MIN_IMPORT_HEIGHT: f32 = -500.0;
pub const MAX_IMPORT_HEIGHT: f32 = 500.0;

/// The running file dialog, so a second press does not open a second one.
#[derive(Resource)]
struct HeightmapPick(Task<Option<rfd::FileHandle>>);

/// Choose the heightmap the next import reads.
///
/// Opens the project's file dialog. A file outside the project's assets is
/// refused: the import reads it through the asset path a scene can name.
#[operator(
    id = "terrain.import.pick",
    label = "Pick Heightmap",
    description = "Choose the image the next terrain import reads its heights from.",
    is_available = has_selected_terrain,
    allows_undo = false
)]
pub(crate) fn terrain_import_pick(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(open_heightmap_picker);
    OperatorResult::Finished
}

fn open_heightmap_picker(world: &mut World) {
    if world.contains_resource::<HeightmapPick>() {
        return;
    }
    let dialog =
        crate::native_dialog::file_dialog(world, crate::native_dialog::DialogPurpose::Image)
            .set_title("Select heightmap")
            .add_filter("Greyscale images", &["png"]);
    let task = AsyncComputeTaskPool::get().spawn(async move { dialog.pick_file().await });
    world.insert_resource(HeightmapPick(task));
}

fn poll_heightmap_pick(world: &mut World) {
    let Some(mut pick) = world.get_resource_mut::<HeightmapPick>() else {
        return;
    };
    let Some(result) = future::block_on(future::poll_once(&mut pick.0)) else {
        return;
    };
    world.remove_resource::<HeightmapPick>();
    let Some(chosen) = result else {
        return;
    };
    let file = chosen.path().to_path_buf();
    crate::native_dialog::remember_pick(world, crate::native_dialog::DialogPurpose::Image, &file);
    let Some(relative) = crate::asset_index::indexed_path(world, &file) else {
        crate::status_bar::notify_error(
            world,
            "that heightmap is outside this project's assets".to_string(),
        );
        return;
    };
    world.resource_mut::<TerrainImportState>().heightmap = relative.to_slash_lossy().into_owned();
}

/// Replace the selected terrain's heights from a greyscale image, and
/// optionally its paint, its scatter masks and its detail masks from one
/// image each.
///
/// Leaves one undo entry for the whole import, heights, paint, masks and
/// shape together: a half-undone import is a terrain whose ground and
/// paint disagree.
///
/// `allows_undo = false` because the entry is pushed here. A `size` writes
/// the cell size onto the component, which the framework's snapshot diff
/// would otherwise record a second time.
#[operator(
    id = "terrain.import",
    label = "Import Terrain",
    description = "Replace the selected terrain's heights, and optionally its paint and masks, \
                   from images.",
    is_available = has_selected_terrain,
    allows_undo = false,
    params(
        heightmap(
            String,
            doc = "Greyscale heightmap to read, as a path under the project's assets. \
                   Defaults to the one the Terrain panel has picked."
        ),
        height_range(
            String,
            doc = "World heights the black and white ends stand at, as \"min,max\"."
        ),
        size(
            f64,
            doc = "World metres along each edge. Left out, the terrain keeps the ground \
                   it covers and the image is resampled onto it."
        ),
        weights(
            String,
            doc = "Greyscale weight image per material slot, as \
                   \"0:ground/grass.png,1:ground/gravel.png\"."
        ),
        channels(
            String,
            doc = "Greyscale image per scatter mask, as \"rocks:masks/rocks.png\". \
                   Black is unpainted; anywhere else takes the mask's value, ready \
                   for terrain.scatter. A mask this terrain does not declare is added."
        ),
        details(
            String,
            doc = "Greyscale image per detail layer, as \"0:masks/grass.png\", naming \
                   the layer by index or by name. The image is the layer's whole \
                   mask: black is bare ground and white is full cover."
        ),
    )
)]
pub(crate) fn terrain_import(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    terrains: Query<&jackdaw_scene_types::Terrain>,
    store: Res<TerrainDataStore>,
    state: Res<TerrainImportState>,
    project: Option<Res<ProjectRoot>>,
    mut commands: Commands,
) -> OperatorResult {
    let entity = selection.primary()?;
    let terrain = terrains.get(entity)?.clone();

    match read_images(
        &params.0,
        &terrain,
        &store,
        &state,
        project.as_deref().map(ProjectRoot::assets_dir),
    ) {
        Ok(import) => {
            commands.queue(move |world: &mut World| apply(world, entity, terrain, import));
            OperatorResult::Finished
        }
        Err(message) => {
            warn!("{message}");
            commands.queue(move |world: &mut World| {
                crate::terrain::toast_terrain_notice(world, &message);
            });
            OperatorResult::Cancelled
        }
    }
}

/// The images an import reads and how to lay them down, all decoded before
/// anything is written so a refusal leaves the terrain as it was.
struct DecodedImport {
    heightmap: GreyImage,
    range: HeightRange,
    size: Option<f32>,
    weights: Vec<(u8, GreyImage)>,
    /// Scatter masks by name, whether or not the terrain declares them yet.
    channels: Vec<(String, GreyImage)>,
    /// Detail masks by the index of the channel their layer grows from.
    masks: Vec<(usize, GreyImage)>,
}

/// One channel's values before and after an import paints them.
struct ChannelWrite {
    index: usize,
    old: Vec<u16>,
    new: Vec<u16>,
}

/// The value a scatter mask's image writes: the first value its palette
/// offers that is not the unset zero, or full cover on a channel carrying
/// no palette at all, which is what a detail layer's density is.
fn paint_value(channel: &TerrainChannel) -> u16 {
    channel
        .palette
        .iter()
        .map(|entry| entry.value)
        .find(|value| *value != 0)
        .unwrap_or_else(|| channel.element.max_value())
}

/// The channel an import mints for a name the terrain does not declare:
/// what `terrain.channel.add` would have made.
fn fresh_channel(name: &str) -> TerrainChannel {
    TerrainChannel {
        name: name.to_string(),
        element: TerrainChannelElement::U8,
        palette: super::channel_ops::seeded_palette(),
    }
}

/// Lay the decoded images over the terrain and leave one undo entry.
///
/// Reads the grid, lays ground down where there is none and writes the
/// result in one world access: a terrain that has just been given its
/// regions must not be measured in one frame and written in another, or the
/// write lands on a grid that is no longer the one it was sized for.
fn apply(
    world: &mut World,
    entity: Entity,
    terrain: jackdaw_scene_types::Terrain,
    import: DecodedImport,
) {
    let mut store = world.resource_mut::<TerrainDataStore>();
    let Some(mut data) = store.entry_for(&terrain) else {
        return;
    };
    // An unsculpted terrain holds no regions, so an import lays the same
    // footprint down that a generate does before writing over it.
    if data.document().grid_resolution() == 0
        && let Err(err) =
            data.ensure_extent(FRESH_TERRAIN_REGIONS * jackdaw_terrain::RegionSize::DEFAULT.get())
    {
        let message = err.to_string();
        warn!("{message}");
        crate::terrain::toast_terrain_notice(world, &message);
        return;
    }
    let resolution = data.document().grid_resolution();

    let mut new_heights = heights_from_image(&import.heightmap, resolution, import.range);
    // Snapped before the array is recorded, so the terrain is never briefly
    // off-lattice and undo restores no unsnapped intermediate.
    if let Some(step) = terrain.quantization.active_height_step() {
        jackdaw_terrain::quantize_heights(&mut new_heights, step);
    }
    let old_heights = data.heights().to_vec();

    // Read before anything writes, so a channel this import is about to
    // add reads as the zeros it is about to be given.
    let mut minted: Vec<String> = Vec::new();
    let mut writes: Vec<ChannelWrite> = Vec::new();
    for (name, image) in &import.channels {
        let declared = terrain.channels.iter().position(|c| &c.name == name);
        let (index, value) = match declared {
            Some(index) => (index, paint_value(&terrain.channels[index])),
            None => {
                let at = minted
                    .iter()
                    .position(|already| already == name)
                    .unwrap_or_else(|| {
                        minted.push(name.clone());
                        minted.len() - 1
                    });
                (
                    terrain.channels.len() + at,
                    paint_value(&fresh_channel(name)),
                )
            }
        };
        let old = data.channel_values(index);
        let mut new = old.clone();
        // Whole texels, so the edge of a drawn shape stays where it was
        // drawn rather than fading a blend of two values outward.
        jackdaw_terrain::import::paint_channel(
            &mut new,
            &ChannelPaint {
                value,
                samples: image.resample_nearest(resolution),
            },
        );
        writes.push(ChannelWrite { index, old, new });
    }
    for (index, image) in &import.masks {
        let ceiling = terrain
            .channels
            .get(*index)
            .map_or(u16::from(u8::MAX), |channel| channel.element.max_value());
        let old = data.channel_values(*index);
        let mut new = old.clone();
        jackdaw_terrain::import::paint_mask(&mut new, &image.resample(resolution), ceiling);
        writes.push(ChannelWrite {
            index: *index,
            old,
            new,
        });
    }

    let control = (!import.weights.is_empty()).then(|| {
        let old = store.control(&terrain.data_path).to_vec();
        let mut new = old.clone();
        let layers: Vec<SlotWeights> = import
            .weights
            .iter()
            .map(|(slot, image)| SlotWeights {
                slot: *slot,
                weights: image.resample(resolution),
            })
            .collect();
        jackdaw_terrain::import::paint_weights(&mut new, &layers);
        (old, new)
    });

    let mut steps: Vec<Box<dyn EditorCommand>> = Vec::new();
    if let Some(size) = import.size {
        let spacing = clamp_cell_size(size / (resolution.max(2) - 1) as f32);
        if spacing != terrain.cell_size {
            steps.push(Box::new(SetTerrainShape::new(
                entity,
                terrain.data_path.clone(),
                terrain.cell_size,
                spacing,
                LABEL.to_string(),
            )));
        }
    }
    steps.push(Box::new(SetTerrainHeights::whole(
        entity,
        old_heights,
        new_heights,
        LABEL.to_string(),
    )));
    if let Some((old, new)) = control {
        steps.push(Box::new(SetTerrainControl::whole(
            entity,
            old,
            new,
            LABEL.to_string(),
        )));
    }
    // The descriptors first: a channel's values have nowhere to land until
    // the terrain declares it. Undo runs the group backwards, so the values
    // go back before the descriptor they belong to leaves.
    for name in minted {
        steps.push(Box::new(AddTerrainChannel {
            entity,
            name,
            palette: super::channel_ops::seeded_palette(),
        }));
    }
    for write in writes {
        steps.push(Box::new(SetTerrainChannel {
            entity,
            channel: write.index,
            old_values: write.old,
            new_values: write.new,
            label: LABEL.to_string(),
        }));
    }
    world.resource_scope(|world, mut history: Mut<CommandHistory>| {
        history.execute(
            Box::new(CommandGroup {
                commands: steps,
                label: LABEL.to_string(),
            }),
            world,
        );
    });
}

/// Read the arguments and decode every image they name, refusing anything
/// the import could not act on.
fn read_images(
    params: &OperatorParameters,
    terrain: &jackdaw_scene_types::Terrain,
    store: &TerrainDataStore,
    state: &TerrainImportState,
    assets: Option<PathBuf>,
) -> Result<DecodedImport, String> {
    let named = params.as_str("heightmap").unwrap_or(&state.heightmap);
    if named.is_empty() {
        return Err("choose a heightmap to import".to_string());
    }
    let range = match params.as_str("height_range") {
        Some(text) => parse_height_range(text)?,
        None => HeightRange::new(state.min, state.max),
    };
    let heightmap = decode_grey(&project_file(assets.as_deref(), named)?, named)?;

    let slots = store.materials(&terrain.data_path);
    let named_weights = match params.as_str("weights") {
        Some(text) => parse_weights(text, slots)?,
        None => Vec::new(),
    };
    let mut weights = Vec::with_capacity(named_weights.len());
    for (slot, named) in &named_weights {
        let file = project_file(assets.as_deref(), named)?;
        weights.push((*slot, decode_grey(&file, named)?));
    }

    let named_channels = match params.as_str("channels") {
        Some(text) => parse_pairs(text, "channels", "mask:path", "rocks:masks/rocks.png")?,
        None => Vec::new(),
    };
    let mut channels = Vec::with_capacity(named_channels.len());
    for (mask, named) in &named_channels {
        let file = project_file(assets.as_deref(), named)?;
        channels.push((mask.clone(), decode_grey(&file, named)?));
    }

    let named_masks = match params.as_str("details") {
        Some(text) => parse_details(text, terrain)?,
        None => Vec::new(),
    };
    let mut masks = Vec::with_capacity(named_masks.len());
    for (index, named) in &named_masks {
        let file = project_file(assets.as_deref(), named)?;
        masks.push((*index, decode_grey(&file, named)?));
    }

    Ok(DecodedImport {
        heightmap,
        range,
        size: number(params, "size").map(|size| size as f32),
        weights,
        channels,
        masks,
    })
}

/// A float parameter, however it was spelled.
fn number(params: &OperatorParameters, key: &str) -> Option<f64> {
    params
        .as_float(key)
        .or_else(|| params.as_int(key).map(|value| value as f64))
}

/// `"min,max"` as the two world heights it names.
fn parse_height_range(text: &str) -> Result<HeightRange, String> {
    let bad = || format!("height_range wants two numbers, as \"0,60\", not \"{text}\"");
    let (min, max) = text.split_once(',').ok_or_else(bad)?;
    let min: f32 = min.trim().parse().map_err(|_| bad())?;
    let max: f32 = max.trim().parse().map_err(|_| bad())?;
    if !min.is_finite() || !max.is_finite() {
        return Err(bad());
    }
    Ok(HeightRange::new(min, max))
}

/// `"a:one.png,b:two.png"` as the pairs it names, each side trimmed.
///
/// `shape` and `example` are how the refusal describes the pair the
/// argument wanted, since each argument pairs a different thing with a
/// path.
fn parse_pairs(
    text: &str,
    key: &str,
    shape: &str,
    example: &str,
) -> Result<Vec<(String, String)>, String> {
    let mut pairs = Vec::new();
    for field in text.split(',').map(str::trim).filter(|f| !f.is_empty()) {
        let bad = || format!("{key} wants {shape} pairs, as \"{example}\", not \"{field}\"");
        let (left, path) = field.split_once(':').ok_or_else(bad)?;
        let (left, path) = (left.trim(), path.trim());
        if left.is_empty() || path.is_empty() {
            return Err(bad());
        }
        pairs.push((left.to_string(), path.to_string()));
    }
    Ok(pairs)
}

/// `"0:a.png,1:b.png"` as the slot and path pairs it names, refusing a slot
/// with no material behind it.
fn parse_weights(
    text: &str,
    slots: &[jackdaw_terrain::sidecar::TerrainMaterialSlot],
) -> Result<Vec<(u8, String)>, String> {
    let mut pairs = Vec::new();
    for (slot, path) in parse_pairs(text, "weights", "slot:path", "0:ground/grass.png")? {
        let index: u8 = slot
            .parse()
            .map_err(|_| format!("\"{slot}\" is not a texture slot"))?;
        match slots.get(index as usize) {
            Some(entry) if !entry.is_tombstone() => {}
            _ => return Err(format!("this terrain has no material in slot {index}")),
        }
        pairs.push((index, path));
    }
    Ok(pairs)
}

/// `"0:a.png,grass:b.png"` as the channel each named detail layer grows
/// from and the image painting its mask, refusing a layer this terrain does
/// not have.
///
/// A layer is named by index or by the name it carries, an index first so
/// that one still reaches a layer whose name is a number.
fn parse_details(
    text: &str,
    terrain: &jackdaw_scene_types::Terrain,
) -> Result<Vec<(usize, String)>, String> {
    let mut pairs = Vec::new();
    for (named, path) in parse_pairs(text, "details", "layer:path", "0:ground/grass.png")? {
        let layer = named
            .parse::<usize>()
            .ok()
            .filter(|index| *index < terrain.detail.len())
            .or_else(|| terrain.detail.iter().position(|layer| layer.name == named))
            .and_then(|index| terrain.detail.get(index))
            .ok_or_else(|| format!("this terrain has no detail layer {named}"))?;
        let index = terrain
            .channels
            .iter()
            .position(|channel| channel.name == layer.density_channel)
            .ok_or_else(|| {
                format!(
                    "the {} detail layer grows from no mask this terrain declares",
                    layer.name
                )
            })?;
        pairs.push((index, path));
    }
    Ok(pairs)
}

/// An asset path as the file it names, refusing one that walks out of the
/// project.
fn project_file(assets: Option<&Path>, named: &str) -> Result<PathBuf, String> {
    let outside = || format!("\"{named}\" is outside this project's assets");
    let path = Path::new(named);
    if path.is_absolute() || path.components().any(|part| part.as_os_str() == "..") {
        return Err(outside());
    }
    let assets = assets.ok_or_else(outside)?;
    Ok(assets.join(path))
}

/// A greyscale PNG as normalised samples.
///
/// Both bit depths land on the same `0..1`, so a terrain imported from an
/// 8-bit image and the same terrain at 16 bits stand at the same heights;
/// the deeper image only resolves finer steps between them.
fn decode_grey(file: &Path, named: &str) -> Result<GreyImage, String> {
    let decoded = image::ImageReader::open(file)
        .and_then(image::ImageReader::with_guessed_format)
        .map_err(|err| format!("could not read \"{named}\": {err}"))?
        .decode()
        .map_err(|err| format!("could not decode \"{named}\": {err}"))?;
    let (width, height) = (decoded.width(), decoded.height());
    let samples: Vec<f32> = match decoded {
        image::DynamicImage::ImageLuma8(image) => image
            .into_raw()
            .into_iter()
            .map(|value| value as f32 / u8::MAX as f32)
            .collect(),
        image::DynamicImage::ImageLuma16(image) => image
            .into_raw()
            .into_iter()
            .map(|value| value as f32 / u16::MAX as f32)
            .collect(),
        _ => return Err(format!("\"{named}\" is not a greyscale image")),
    };
    GreyImage::new(width, height, samples).map_err(|err| format!("\"{named}\": {err}"))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use jackdaw_scene_types::PropertyValue;
    use jackdaw_terrain::control::Control;
    use jackdaw_terrain::sidecar::TerrainMaterialSlot;

    use super::*;

    /// Importing end to end: images on disk in, heights and control words
    /// out, through the operator the panel and a script both dispatch.
    mod importing {
        use super::*;

        const PATH: &str = "zone.jdterrain";
        const RESOLUTION: u32 = 4;

        fn params(pairs: &[(&str, PropertyValue)]) -> OperatorParameters {
            let mut map = BTreeMap::new();
            for (key, value) in pairs {
                map.insert(key.to_string(), value.clone());
            }
            OperatorParameters(map)
        }

        fn text(value: &str) -> PropertyValue {
            PropertyValue::String(value.to_string().into())
        }

        /// Write a greyscale image into the project's assets, at either
        /// bit depth, from samples in `0..1`.
        fn write_grey(assets: &Path, name: &str, width: u32, samples: &[f32], deep: bool) {
            let file = assets.join(name);
            std::fs::create_dir_all(file.parent().expect("under assets")).expect("assets dir");
            let height = samples.len() as u32 / width;
            if deep {
                let raw: Vec<u16> = samples
                    .iter()
                    .map(|s| (s * u16::MAX as f32).round() as u16)
                    .collect();
                image::ImageBuffer::<image::Luma<u16>, _>::from_raw(width, height, raw)
                    .expect("fills the rectangle")
                    .save(&file)
                    .expect("writes");
            } else {
                let raw: Vec<u8> = samples
                    .iter()
                    .map(|s| (s * u8::MAX as f32).round() as u8)
                    .collect();
                image::ImageBuffer::<image::Luma<u8>, _>::from_raw(width, height, raw)
                    .expect("fills the rectangle")
                    .save(&file)
                    .expect("writes");
            }
        }

        /// A world holding one selected terrain over four cells a side, and
        /// a project whose assets the images are written into.
        fn world(root: &Path) -> World {
            let mut world = World::new();
            world.init_resource::<bevy::ecs::reflect::AppTypeRegistry>();
            world.init_resource::<Selection>();
            world.init_resource::<TerrainDataStore>();
            world.init_resource::<CommandHistory>();
            world.init_resource::<TerrainImportState>();
            world.insert_resource(ProjectRoot::new(
                root,
                crate::project::ProjectConfig::default(),
            ));

            let mut regions = jackdaw_terrain::TerrainRegions::new(
                jackdaw_terrain::RegionSize::new(RESOLUTION).expect("a power of two"),
            );
            regions.ensure_grid(RESOLUTION).expect("inside the cap");
            world.resource_mut::<TerrainDataStore>().insert(
                PATH.to_string(),
                jackdaw_terrain::RegionTerrainData {
                    regions,
                    ..default()
                },
            );

            let entity = world
                .spawn((
                    jackdaw_scene_types::Terrain {
                        resolution: RESOLUTION,
                        data_path: PATH.to_string(),
                        ..default()
                    },
                    crate::terrain::TerrainDirtyChunks::default(),
                ))
                .id();
            world.resource_mut::<Selection>().entities = vec![entity];
            world
        }

        /// A project directory with an `assets/` to write images into.
        fn project() -> (tempfile::TempDir, PathBuf) {
            let dir = tempfile::tempdir().expect("temp dir");
            let assets = dir.path().join("assets");
            std::fs::create_dir_all(&assets).expect("assets dir");
            (dir, assets)
        }

        fn import(world: &mut World, pairs: &[(&str, PropertyValue)]) -> OperatorResult {
            let result = world
                .run_system_cached_with(terrain_import, params(pairs))
                .expect("system runs");
            world.flush();
            result
        }

        fn heights(world: &World) -> Vec<f32> {
            world.resource::<TerrainDataStore>().heights(PATH).to_vec()
        }

        fn control(world: &World) -> Vec<Control> {
            world.resource::<TerrainDataStore>().control(PATH).to_vec()
        }

        /// One channel's values as the grid holds them, by the name the
        /// terrain declares it under.
        fn channel(world: &World, name: &str) -> Vec<u16> {
            let terrain = world
                .resource::<TerrainDataStore>()
                .get(PATH)
                .expect("a document");
            let index = world
                .iter_entities()
                .find_map(|entity| entity.get::<jackdaw_scene_types::Terrain>())
                .expect("a terrain")
                .channels
                .iter()
                .position(|channel| channel.name == name)
                .expect("a channel by that name");
            terrain
                .regions
                .read_grid_channel(index, terrain.grid_resolution())
        }

        /// Give the terrain a channel, and the store the zeroed plane that
        /// goes with it.
        fn declare_channel(world: &mut World, channel: TerrainChannel) {
            let entity = world.resource::<Selection>().entities[0];
            world
                .get_mut::<jackdaw_scene_types::Terrain>(entity)
                .expect("a terrain")
                .channels
                .push(channel);
            let terrain = world
                .get::<jackdaw_scene_types::Terrain>(entity)
                .expect("a terrain")
                .clone();
            world.resource_mut::<TerrainDataStore>().entry_for(&terrain);
        }

        /// A layer growing from a channel of continuous cover, as
        /// `terrain.detail.add` leaves one.
        fn declare_detail(world: &mut World, name: &str) {
            declare_channel(
                world,
                TerrainChannel {
                    palette: Vec::new(),
                    ..fresh_channel(name)
                },
            );
            let entity = world.resource::<Selection>().entities[0];
            world
                .get_mut::<jackdaw_scene_types::Terrain>(entity)
                .expect("a terrain")
                .detail
                .push(jackdaw_scene_types::DetailLayer {
                    name: name.to_string(),
                    density_channel: name.to_string(),
                    ..default()
                });
        }

        /// White down the right half of the grid and black down the left.
        fn right_half() -> Vec<f32> {
            (0..RESOLUTION * RESOLUTION)
                .map(|i| f32::from(i % RESOLUTION >= RESOLUTION / 2))
                .collect()
        }

        /// A ramp across the grid, black on the left and white on the right.
        fn ramp() -> Vec<f32> {
            (0..RESOLUTION * RESOLUTION)
                .map(|i| (i % RESOLUTION) as f32 / (RESOLUTION - 1) as f32)
                .collect()
        }

        /// Both depths address the same `0..1`, so the deeper image only
        /// resolves finer steps between the same two ends.
        #[test]
        fn an_8_bit_and_a_16_bit_heightmap_import_to_the_same_heights() {
            let (dir, assets) = project();
            write_grey(&assets, "ramp8.png", RESOLUTION, &ramp(), false);
            write_grey(&assets, "ramp16.png", RESOLUTION, &ramp(), true);

            let mut shallow = world(dir.path());
            assert_eq!(
                import(
                    &mut shallow,
                    &[
                        ("heightmap", text("ramp8.png")),
                        ("height_range", text("0,60")),
                    ],
                ),
                OperatorResult::Finished
            );
            let mut deep = world(dir.path());
            assert_eq!(
                import(
                    &mut deep,
                    &[
                        ("heightmap", text("ramp16.png")),
                        ("height_range", text("0,60")),
                    ],
                ),
                OperatorResult::Finished
            );

            let shallow = heights(&shallow);
            let deep = heights(&deep);
            assert_eq!(shallow.len(), (RESOLUTION * RESOLUTION) as usize);
            assert_eq!(shallow[0], 0.0, "black stands at the range's floor");
            assert_eq!(
                shallow[RESOLUTION as usize - 1],
                60.0,
                "white stands at its ceiling"
            );
            for (shallow, deep) in shallow.iter().zip(&deep) {
                assert!(
                    (shallow - deep).abs() < 0.25,
                    "{shallow} and {deep} are the same ground",
                );
            }
        }

        /// A terrain nothing has sculpted holds no regions, and an import
        /// has to lay ground down and write the heights onto the grid it
        /// just made rather than one measured a frame earlier.
        #[test]
        fn an_import_onto_a_terrain_with_no_ground_lays_it_down_and_writes_it() {
            let (dir, assets) = project();
            write_grey(&assets, "ramp8.png", RESOLUTION, &ramp(), false);

            let mut world = world(dir.path());
            world.resource_mut::<TerrainDataStore>().remove(PATH);
            assert!(heights(&world).is_empty(), "no ground to begin with");

            assert_eq!(
                import(
                    &mut world,
                    &[
                        ("heightmap", text("ramp8.png")),
                        ("height_range", text("0,60")),
                    ],
                ),
                OperatorResult::Finished
            );

            let heights = heights(&world);
            let resolution = FRESH_TERRAIN_REGIONS * jackdaw_terrain::RegionSize::DEFAULT.get();
            assert_eq!(heights.len(), (resolution * resolution) as usize);
            assert_eq!(heights[0], 0.0, "the image's black edge");
            assert_eq!(
                heights[resolution as usize - 1],
                60.0,
                "and its white one, across the ground just laid down"
            );
        }

        /// An image the grid's size is not stretched or cropped: it is
        /// resampled onto whatever grid the terrain holds.
        #[test]
        fn a_heightmap_of_another_resolution_resamples_to_the_terrains() {
            let (dir, assets) = project();
            write_grey(&assets, "small.png", 2, &[0.0, 1.0, 0.0, 1.0], false);

            let mut world = world(dir.path());
            assert_eq!(
                import(
                    &mut world,
                    &[
                        ("heightmap", text("small.png")),
                        ("height_range", text("0,30")),
                    ],
                ),
                OperatorResult::Finished
            );

            let heights = heights(&world);
            assert_eq!(heights.len(), (RESOLUTION * RESOLUTION) as usize);
            assert_eq!(heights[0], 0.0);
            assert_eq!(heights[RESOLUTION as usize - 1], 30.0);
            assert!(
                heights[1] > 0.0 && heights[1] < 30.0,
                "the two texels are interpolated across the grid, not repeated",
            );
        }

        /// Two weight images summing past one at every cell normalise
        /// against each other rather than clipping one out.
        #[test]
        fn weights_paint_the_named_slots_and_normalise() {
            let (dir, assets) = project();
            write_grey(&assets, "flat.png", RESOLUTION, &[0.5; 16], false);
            write_grey(&assets, "grass.png", RESOLUTION, &[1.0; 16], false);
            write_grey(&assets, "gravel.png", RESOLUTION, &[1.0; 16], false);

            let mut world = world(dir.path());
            world
                .resource_mut::<TerrainDataStore>()
                .set_materials(
                    PATH,
                    vec![
                        TerrainMaterialSlot::new("grass"),
                        TerrainMaterialSlot::new("gravel"),
                    ],
                )
                .expect("plain names are accepted");

            assert_eq!(
                import(
                    &mut world,
                    &[
                        ("heightmap", text("flat.png")),
                        ("height_range", text("0,10")),
                        ("weights", text("0:grass.png,1:gravel.png")),
                    ],
                ),
                OperatorResult::Finished
            );

            let control = control(&world);
            assert!(
                control.iter().all(|word| word.manual()),
                "every cell claimed"
            );
            assert_eq!(control[0].base_id(), 0);
            assert_eq!(control[0].overlay_id(), 1);
            assert_eq!(
                control[0].blend(),
                (0.5 * jackdaw_terrain::MAX_BLEND as f32).round() as u8,
                "two equal weights meet halfway",
            );
        }

        /// A refusal is a refusal: nothing of the import reaches the
        /// terrain, not the heights it could have written first.
        #[test]
        fn a_heightmap_outside_the_project_is_refused_and_the_terrain_is_unchanged() {
            let (dir, assets) = project();
            write_grey(&assets, "ramp8.png", RESOLUTION, &ramp(), false);

            let mut world = world(dir.path());
            let before = heights(&world);
            assert_eq!(
                import(
                    &mut world,
                    &[
                        ("heightmap", text("../ramp8.png")),
                        ("height_range", text("0,60")),
                    ],
                ),
                OperatorResult::Cancelled
            );
            assert_eq!(heights(&world), before);
        }

        /// The slot list is what the control map's ids mean, so a weight
        /// image naming an id with no material behind it is refused before
        /// anything is written.
        #[test]
        fn a_weight_naming_an_empty_slot_is_refused_and_the_terrain_is_unchanged() {
            let (dir, assets) = project();
            write_grey(&assets, "flat.png", RESOLUTION, &[0.5; 16], false);
            write_grey(&assets, "grass.png", RESOLUTION, &[1.0; 16], false);

            let mut world = world(dir.path());
            let before = heights(&world);
            assert_eq!(
                import(
                    &mut world,
                    &[
                        ("heightmap", text("flat.png")),
                        ("height_range", text("0,10")),
                        ("weights", text("3:grass.png")),
                    ],
                ),
                OperatorResult::Cancelled
            );
            assert_eq!(heights(&world), before);
            assert!(control(&world).iter().all(|word| !word.manual()));
        }

        /// A mask says where things stand, so an import lays a shape into
        /// it that `terrain.scatter` can place a group over.
        #[test]
        fn a_channel_image_paints_its_value_where_it_is_not_black() {
            let (dir, assets) = project();
            write_grey(&assets, "flat.png", RESOLUTION, &[0.5; 16], false);
            write_grey(&assets, "rocks.png", RESOLUTION, &right_half(), false);

            let mut world = world(dir.path());
            declare_channel(&mut world, fresh_channel("rocks"));

            assert_eq!(
                import(
                    &mut world,
                    &[
                        ("heightmap", text("flat.png")),
                        ("height_range", text("0,10")),
                        ("channels", text("rocks:rocks.png")),
                    ],
                ),
                OperatorResult::Finished
            );

            let painted = channel(&world, "rocks");
            assert_eq!(painted[0], 0, "black left the cell unpainted");
            assert_eq!(
                painted[RESOLUTION as usize - 1],
                1,
                "white wrote the mask's one paintable value",
            );
        }

        /// A terrain arriving from elsewhere brings masks this one has never
        /// been told about, and naming one is how it is told.
        #[test]
        fn a_channel_the_terrain_does_not_declare_is_added_by_the_import() {
            let (dir, assets) = project();
            write_grey(&assets, "flat.png", RESOLUTION, &[0.5; 16], false);
            write_grey(&assets, "rocks.png", RESOLUTION, &right_half(), false);

            let mut world = world(dir.path());
            assert_eq!(
                import(
                    &mut world,
                    &[
                        ("heightmap", text("flat.png")),
                        ("height_range", text("0,10")),
                        ("channels", text("rocks:rocks.png")),
                    ],
                ),
                OperatorResult::Finished
            );

            let entity = world.resource::<Selection>().entities[0];
            let declared = world
                .get::<jackdaw_scene_types::Terrain>(entity)
                .expect("a terrain")
                .channels
                .clone();
            assert_eq!(declared.len(), 1);
            assert_eq!(declared[0].name, "rocks");
            assert_eq!(
                channel(&world, "rocks")[RESOLUTION as usize - 1],
                1,
                "the mask it just minted is painted too",
            );
        }

        /// The image is the layer's whole mask: white is full cover and
        /// black is ground nothing grows on.
        #[test]
        fn a_detail_image_lays_the_layers_mask_down() {
            let (dir, assets) = project();
            write_grey(&assets, "flat.png", RESOLUTION, &[0.5; 16], false);
            write_grey(&assets, "grass.png", RESOLUTION, &right_half(), false);

            let mut world = world(dir.path());
            declare_detail(&mut world, "grass");

            assert_eq!(
                import(
                    &mut world,
                    &[
                        ("heightmap", text("flat.png")),
                        ("height_range", text("0,10")),
                        ("details", text("0:grass.png")),
                    ],
                ),
                OperatorResult::Finished
            );

            let mask = channel(&world, "grass");
            assert_eq!(mask[0], 0, "bare ground");
            assert_eq!(
                mask[RESOLUTION as usize - 1],
                u16::from(u8::MAX),
                "full cover at the channel's ceiling",
            );
        }

        /// A mask has nowhere to go without a layer to grow it, so the
        /// import refuses before it writes anything.
        #[test]
        fn a_detail_image_naming_no_layer_is_refused_and_the_terrain_is_unchanged() {
            let (dir, assets) = project();
            write_grey(&assets, "flat.png", RESOLUTION, &[0.5; 16], false);
            write_grey(&assets, "grass.png", RESOLUTION, &right_half(), false);

            let mut world = world(dir.path());
            let before = heights(&world);
            assert_eq!(
                import(
                    &mut world,
                    &[
                        ("heightmap", text("flat.png")),
                        ("height_range", text("0,10")),
                        ("details", text("2:grass.png")),
                    ],
                ),
                OperatorResult::Cancelled
            );
            assert_eq!(heights(&world), before);
        }

        /// One entry covers the whole import, so a terrain whose ground and
        /// paint arrived together goes back together.
        #[test]
        fn undo_restores_the_heights_and_the_paint() {
            let (dir, assets) = project();
            write_grey(&assets, "ramp8.png", RESOLUTION, &ramp(), false);
            write_grey(&assets, "grass.png", RESOLUTION, &[1.0; 16], false);

            let mut world = world(dir.path());
            world
                .resource_mut::<TerrainDataStore>()
                .set_materials(PATH, vec![TerrainMaterialSlot::new("grass")])
                .expect("a plain name is accepted");
            let heights_before = heights(&world);
            let control_before = control(&world);

            assert_eq!(
                import(
                    &mut world,
                    &[
                        ("heightmap", text("ramp8.png")),
                        ("height_range", text("0,60")),
                        ("weights", text("0:grass.png")),
                    ],
                ),
                OperatorResult::Finished
            );
            assert_ne!(heights(&world), heights_before);
            assert_ne!(control(&world), control_before);

            world.resource_scope(|world, mut history: Mut<CommandHistory>| {
                history.undo(world);
            });

            assert_eq!(heights(&world), heights_before);
            assert_eq!(control(&world), control_before);
        }

        /// Masks go back with the rest: a mask minted by the import leaves
        /// again, and one that was already painted keeps what it had.
        #[test]
        fn undo_restores_the_masks_and_drops_the_one_the_import_minted() {
            let (dir, assets) = project();
            write_grey(&assets, "flat.png", RESOLUTION, &[0.5; 16], false);
            write_grey(&assets, "shape.png", RESOLUTION, &right_half(), false);

            let mut world = world(dir.path());
            declare_detail(&mut world, "grass");

            assert_eq!(
                import(
                    &mut world,
                    &[
                        ("heightmap", text("flat.png")),
                        ("height_range", text("0,10")),
                        ("channels", text("rocks:shape.png")),
                        ("details", text("grass:shape.png")),
                    ],
                ),
                OperatorResult::Finished
            );
            assert_ne!(channel(&world, "grass"), vec![0; 16]);

            world.resource_scope(|world, mut history: Mut<CommandHistory>| {
                history.undo(world);
            });

            let entity = world.resource::<Selection>().entities[0];
            let declared = world
                .get::<jackdaw_scene_types::Terrain>(entity)
                .expect("a terrain")
                .channels
                .clone();
            assert_eq!(
                declared.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
                vec!["grass"],
                "the minted mask is gone again",
            );
            assert_eq!(
                channel(&world, "grass"),
                vec![0; 16],
                "and the layer's mask is back to bare ground",
            );
        }

        /// Heights, paint, masks and detail arrive as one terrain, so one
        /// press of undo is what puts them all back.
        #[test]
        fn heights_weights_channels_and_details_are_one_undo_step() {
            let (dir, assets) = project();
            write_grey(&assets, "ramp8.png", RESOLUTION, &ramp(), false);
            write_grey(&assets, "grass.png", RESOLUTION, &[1.0; 16], false);
            write_grey(&assets, "shape.png", RESOLUTION, &right_half(), false);

            let mut world = world(dir.path());
            world
                .resource_mut::<TerrainDataStore>()
                .set_materials(PATH, vec![TerrainMaterialSlot::new("grass")])
                .expect("a plain name is accepted");
            declare_detail(&mut world, "meadow");
            let heights_before = heights(&world);
            let control_before = control(&world);

            assert_eq!(
                import(
                    &mut world,
                    &[
                        ("heightmap", text("ramp8.png")),
                        ("height_range", text("0,60")),
                        ("weights", text("0:grass.png")),
                        ("channels", text("rocks:shape.png")),
                        ("details", text("meadow:shape.png")),
                    ],
                ),
                OperatorResult::Finished
            );

            world.resource_scope(|world, mut history: Mut<CommandHistory>| {
                history.undo(world);
            });

            assert_eq!(heights(&world), heights_before);
            assert_eq!(control(&world), control_before);
            assert_eq!(channel(&world, "meadow"), vec![0; 16]);
            let entity = world.resource::<Selection>().entities[0];
            assert_eq!(
                world
                    .get::<jackdaw_scene_types::Terrain>(entity)
                    .expect("a terrain")
                    .channels
                    .len(),
                1,
                "one press put every part of the import back",
            );
        }
    }

    fn slots() -> Vec<TerrainMaterialSlot> {
        vec![
            TerrainMaterialSlot::new("grass"),
            TerrainMaterialSlot::tombstone(),
        ]
    }

    #[test]
    fn a_height_range_is_two_numbers_and_anything_else_is_refused() {
        let range = parse_height_range(" 0 , 60 ").expect("two numbers");
        assert_eq!(range.min, 0.0);
        assert_eq!(range.max, 60.0);
        assert!(parse_height_range("60").is_err());
        assert!(parse_height_range("low,high").is_err());
    }

    #[test]
    fn a_weight_naming_a_slot_with_no_material_is_refused() {
        assert_eq!(
            parse_weights("0:ground/grass.png", &slots()).expect("slot 0 has a material"),
            vec![(0, "ground/grass.png".to_string())]
        );
        assert!(
            parse_weights("1:ground/gravel.png", &slots()).is_err(),
            "slot 1 is a tombstone"
        );
        assert!(parse_weights("4:ground/gravel.png", &slots()).is_err());
        assert!(parse_weights("ground/grass.png", &slots()).is_err());
    }

    #[test]
    fn a_pair_missing_one_of_its_sides_is_refused() {
        assert!(parse_pairs(":rocks.png", "channels", "mask:path", "a:b.png").is_err());
        assert!(parse_pairs("rocks:", "channels", "mask:path", "a:b.png").is_err());
        assert_eq!(
            parse_pairs(
                " rocks : masks/rocks.png ",
                "channels",
                "mask:path",
                "a:b.png"
            )
            .expect("a whole pair"),
            vec![("rocks".to_string(), "masks/rocks.png".to_string())]
        );
    }

    #[test]
    fn a_detail_image_naming_a_layer_this_terrain_has_not_got_is_refused() {
        let terrain = jackdaw_scene_types::Terrain {
            channels: vec![TerrainChannel {
                name: "grass".to_string(),
                element: TerrainChannelElement::U8,
                palette: Vec::new(),
            }],
            detail: vec![jackdaw_scene_types::DetailLayer {
                name: "meadow".to_string(),
                density_channel: "grass".to_string(),
                ..default()
            }],
            ..default()
        };
        let by_index = parse_details("0:masks/grass.png", &terrain).expect("the only layer");
        assert_eq!(by_index, vec![(0, "masks/grass.png".to_string())]);
        assert_eq!(
            parse_details("meadow:masks/grass.png", &terrain).expect("named"),
            by_index,
            "a name reaches the same layer as its index",
        );
        assert!(parse_details("1:masks/grass.png", &terrain).is_err());
        assert!(parse_details("heather:masks/grass.png", &terrain).is_err());
    }

    #[test]
    fn a_path_that_walks_out_of_the_project_is_refused() {
        let assets = Path::new("/project/assets");
        assert_eq!(
            project_file(Some(assets), "ground/hills.png").expect("stays under assets"),
            assets.join("ground/hills.png")
        );
        assert!(project_file(Some(assets), "../secret.png").is_err());
        assert!(project_file(Some(assets), "/etc/passwd").is_err());
    }
}
