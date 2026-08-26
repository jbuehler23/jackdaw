//! The splat material: terrain shaded from its control map against a
//! texture set, with height-blended transitions between the two ids a
//! control word names.
//!
//! Behind the `render` feature. The data model, sidecar and mesher below
//! it stay buildable with no GPU crate in the graph.
//!
//! A terrain's material names become the three texture arrays the shader
//! binds in two steps: [`resolve_with`] maps each slot onto a
//! [`TextureSetEntry`], and [`splat_images`] stacks the entries' images
//! into arrays.
//!
//! Nothing here loads assets or knows of a name store. Finding the
//! material behind a name is the host's job: the editor looks in its
//! material registry, a game in its project catalog, and each hands
//! [`resolve_with`] a closure that does the looking. What may not differ
//! between them lives here: which material slot is albedo, that a vacated
//! id keeps its place, and that a missing name is reported rather than
//! compacted away.
//!
//! The blend is height-weighted rather than a cross-fade: a layer's
//! weight is `pow(corner_weight + layer_weight + height, sharpness)`,
//! normalized across every contributing layer, so the taller of two
//! textures wins the contested band and the transition interlocks
//! instead of fading. See `shaders/terrain_splat.wgsl`.

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor,
    TextureViewDimension,
};
use bevy::shader::ShaderRef;

use crate::heightmap::Heightmap;
use crate::sidecar::{AutoTerrainSettings, TerrainMaterialSlot};
use crate::splat::ControlTexels;
use crate::texture_set::{
    MAX_TEXTURES, TextureSet, TextureSetEntry, TextureSetError, check_layer_sizes,
};

const SHADER_PATH: &str = "embedded://jackdaw_terrain/render/shaders/terrain_splat.wgsl";

/// Blend sharpness a terrain uses unless it says otherwise: the middle
/// of the dial. The shader remaps `0..1` onto a `4..64` power exponent.
pub const DEFAULT_BLEND_SHARPNESS: f32 = 0.5;

/// Tangent-space normal of a flat surface, as an RGBA8 texel.
const FLAT_NORMAL_TEXEL: [u8; 4] = [128, 128, 255, 255];
/// Height of a layer with no height map: the top of the range, so a
/// heightless layer competes on even terms rather than losing every band.
const FLAT_HEIGHT_TEXEL: [u8; 4] = [255, 255, 255, 255];
/// Albedo of a slot whose material has no base colour texture, or none
/// this project can resolve. Neutral grey rather than a missing layer, so
/// the slot keeps its texture id and the ids painted after it do not
/// shift.
const FALLBACK_ALBEDO_TEXEL: [u8; 4] = [128, 128, 128, 255];

/// The loaded image handles for a [`TextureSet`], parallel to its entries.
///
/// Held per entry rather than pre-stacked because stacking needs the
/// decoded images, which do not exist until the asset server finishes.
/// Every slot is `Option`: a material may be missing, or may not bind
/// that texture.
#[derive(Clone, Debug, Default)]
pub struct TextureSetImages {
    /// Albedo handle per entry, in id order; `None` where the material has
    /// no base colour texture or could not be resolved.
    pub albedo: Vec<Option<Handle<Image>>>,
    /// Normal handle per entry, `None` where the entry has none.
    pub normal: Vec<Option<Handle<Image>>>,
    /// Height handle per entry, `None` where the entry has none.
    pub height: Vec<Option<Handle<Image>>>,
}

impl TextureSetImages {
    /// Every image handle, in no particular order. What a change watcher
    /// compares an `AssetEvent` against.
    pub fn handles(&self) -> impl Iterator<Item = &Handle<Image>> {
        self.albedo
            .iter()
            .flatten()
            .chain(self.normal.iter().flatten())
            .chain(self.height.iter().flatten())
    }
}

/// A terrain's material slots, resolved into what [`splat_images`] reads.
#[derive(Clone, Debug, Default)]
pub struct ResolvedSlots {
    /// One entry per slot, in id order.
    pub set: TextureSet,
    /// Image handles parallel to [`Self::set`].
    pub images: TextureSetImages,
    /// Names of slots whose material the host could not find, in slot
    /// order. Those slots still hold their ids and draw the fallback
    /// layer; this is what a panel or a log names them by.
    pub missing: Vec<String>,
}

/// Turn a terrain's material names into texture entries.
///
/// `lookup` answers what material a name addresses: the editor asks its
/// material registry, a game its project catalog.
///
/// Index-preserving: entry `i` is slot `i` is texture id `i`, whether the
/// slot resolved, went missing, or was vacated. The control map is
/// addressed by these indices, so compacting the list would renumber
/// every id painted above the dropped one.
///
/// The slot mapping is fixed: albedo is `base_color_texture`, normal is
/// `normal_map_texture`, height is `depth_map`. A material with no depth
/// map gets [`splat_images`]'s flat-height fallback, and one flagged
/// `flip_normal_map_y` carries that flag through so the array builder can
/// flip the green channel before the shader samples it.
///
/// Tiling and detiling come from the slot, never from the material: one
/// material is shared across surfaces and tiles differently on each.
pub fn resolve_with<'m>(
    slots: &[TerrainMaterialSlot],
    lookup: impl Fn(&str) -> Option<&'m StandardMaterial>,
    assets: &AssetServer,
) -> ResolvedSlots {
    let path_of = |handle: &Option<Handle<Image>>| {
        handle
            .as_ref()
            .and_then(|handle| assets.get_path(handle.id()))
            .map(|path| path.path().to_string_lossy().replace('\\', "/"))
    };

    let mut resolved = ResolvedSlots::default();
    for slot in slots {
        // A vacated id draws the fallback and is not reported as missing.
        if slot.is_tombstone() {
            resolved.push(TextureSetEntry::vacant(), None, None, None);
            continue;
        }
        let Some(material) = lookup(&slot.material) else {
            resolved.missing.push(slot.material.clone());
            resolved.push(
                TextureSetEntry {
                    detile: slot.detile,
                    ..TextureSetEntry::unresolved(&slot.material, slot.uv_scale)
                },
                None,
                None,
                None,
            );
            continue;
        };

        resolved.push(
            TextureSetEntry {
                material: slot.material.clone(),
                albedo: path_of(&material.base_color_texture),
                normal: path_of(&material.normal_map_texture),
                flip_normal_y: material.flip_normal_map_y,
                height: path_of(&material.depth_map),
                uv_scale: slot.uv_scale,
                detile: slot.detile,
            },
            material.base_color_texture.clone(),
            material.normal_map_texture.clone(),
            material.depth_map.clone(),
        );
    }
    resolved
}

impl ResolvedSlots {
    /// Append one slot's entry and its three handles together, so the
    /// entries and the handle lists cannot fall out of step.
    fn push(
        &mut self,
        entry: TextureSetEntry,
        albedo: Option<Handle<Image>>,
        normal: Option<Handle<Image>>,
        height: Option<Handle<Image>>,
    ) {
        self.set.entries.push(entry);
        self.images.albedo.push(albedo);
        self.images.normal.push(normal);
        self.images.height.push(height);
    }
}

/// The three texture arrays a splat material binds.
#[derive(Clone, Debug)]
pub struct SplatImages {
    pub albedo: Image,
    pub normal: Image,
    pub height: Image,
}

/// Why a texture set could not be stacked into arrays yet, or at all.
#[derive(Clone, Debug, PartialEq)]
pub enum SplatBuildError {
    /// At least one of the set's images has not finished loading. Not a
    /// failure: the caller tries again next frame.
    NotReady,
    /// The set cannot be used, and retrying will not help.
    Invalid(TextureSetError),
    /// An image decoded, but into a format neither bevy's conversion nor
    /// the narrowing below can turn into the array's.
    Unconvertible {
        from: TextureFormat,
        to: TextureFormat,
    },
}

impl core::fmt::Display for SplatBuildError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotReady => write!(f, "texture set images are still loading"),
            Self::Invalid(reason) => write!(f, "{reason}"),
            Self::Unconvertible { from, to } => write!(
                f,
                "a texture decoded as {from:?}, which cannot be reinterpreted as the \
                 array's {to:?}"
            ),
        }
    }
}

impl core::error::Error for SplatBuildError {}

/// Stack a resolved texture set into albedo, normal and height arrays.
///
/// Returns [`SplatBuildError::NotReady`] until every image the set names
/// has decoded, and [`SplatBuildError::Invalid`], naming the material and
/// path, when the layers disagree on size, which a texture array cannot
/// accept.
///
/// Entries with no albedo, normal or height map get a filled layer rather
/// than a missing one, so optionality is per entry and a slot never loses
/// its texture id. When no entry has a given map, that array is built
/// 1x1: sampling a one-texel layer returns the constant anywhere, so the
/// shader needs no flag to tell the two cases apart.
///
/// A normal map flagged [`TextureSetEntry::flip_normal_y`] has its green
/// channel inverted here, once, on the way into the array: the shader
/// samples one convention, and a map that reached it unflipped would
/// light every slope backwards.
pub fn splat_images(
    set: &TextureSet,
    handles: &TextureSetImages,
    images: &Assets<Image>,
) -> Result<SplatImages, SplatBuildError> {
    if let Err(reason) = set.validate() {
        return Err(SplatBuildError::Invalid(reason));
    }
    // The handle lists are parallel to the entries, and every array below
    // declares `entry_count.max(1)` layers. A caller that passes no
    // handles at all would declare one layer over an empty buffer, which
    // is a texture wgpu cannot make.
    if handles.albedo.is_empty() {
        return Err(SplatBuildError::Invalid(TextureSetError::Empty));
    }

    let albedo_layers = decode_layers(&handles.albedo, set, images, |entry| {
        entry.albedo.as_deref().unwrap_or_default()
    })?;
    // The layer size is whatever the resolved albedos agree on. A set
    // whose materials all went missing has none to measure, and builds
    // one texel per layer.
    let (width, height) = match albedo_layers.sizes.first() {
        Some(_) => check_layer_sizes(&albedo_layers.sizes).map_err(SplatBuildError::Invalid)?,
        None => (1, 1),
    };

    let albedo = stack(
        &albedo_layers,
        handles.albedo.len(),
        (width, height),
        TextureFormat::Rgba8UnormSrgb,
        FALLBACK_ALBEDO_TEXEL,
        &[],
    )?;

    let normal_flips: Vec<bool> = set.entries.iter().map(|e| e.flip_normal_y).collect();
    let normal = stack_optional(
        &handles.normal,
        set,
        images,
        (width, height),
        FLAT_NORMAL_TEXEL,
        &normal_flips,
        |entry| entry.normal.as_deref().unwrap_or_default(),
    )?;
    let height_array = stack_optional(
        &handles.height,
        set,
        images,
        (width, height),
        FLAT_HEIGHT_TEXEL,
        &[],
        |entry| entry.height.as_deref().unwrap_or_default(),
    )?;

    Ok(SplatImages {
        albedo,
        normal,
        height: height_array,
    })
}

/// Decoded images for one per-entry handle list, with the sizes of the
/// ones that resolved.
struct DecodedLayers<'a> {
    decoded: Vec<Option<&'a Image>>,
    sizes: Vec<(usize, &'a str, &'a str, (u32, u32))>,
}

fn decode_layers<'a>(
    handles: &[Option<Handle<Image>>],
    set: &'a TextureSet,
    images: &'a Assets<Image>,
    path_of: impl Fn(&TextureSetEntry) -> &str,
) -> Result<DecodedLayers<'a>, SplatBuildError> {
    let mut decoded = Vec::with_capacity(handles.len());
    let mut sizes = Vec::new();
    for (index, handle) in handles.iter().enumerate() {
        let Some(handle) = handle else {
            decoded.push(None);
            continue;
        };
        let image = images.get(handle).ok_or(SplatBuildError::NotReady)?;
        let size = image.size();
        if let Some(entry) = set.entries.get(index) {
            sizes.push((
                index,
                entry.material.as_str(),
                path_of(entry),
                (size.x, size.y),
            ));
        }
        decoded.push(Some(image));
    }
    Ok(DecodedLayers { decoded, sizes })
}

/// Build one array from a per-entry list of optional handles.
///
/// A layer with no handle is filled with `fill`. If no entry has one at
/// all, the whole array collapses to 1x1; the shader reads the same
/// constant either way.
fn stack_optional(
    handles: &[Option<Handle<Image>>],
    set: &TextureSet,
    images: &Assets<Image>,
    size: (u32, u32),
    fill: [u8; 4],
    flip_green: &[bool],
    path_of: impl Fn(&TextureSetEntry) -> &str,
) -> Result<Image, SplatBuildError> {
    if handles.iter().all(Option::is_none) {
        return Ok(fill_array(1, 1, handles.len().max(1) as u32, fill));
    }
    let layers = decode_layers(handles, set, images, path_of)?;
    stack(
        &layers,
        handles.len(),
        size,
        TextureFormat::Rgba8Unorm,
        fill,
        flip_green,
    )
}

/// Lay decoded layers out as one array image, filling the entries that
/// resolved to nothing and refusing any that disagrees on size.
fn stack(
    layers: &DecodedLayers<'_>,
    entry_count: usize,
    (width, height): (u32, u32),
    format: TextureFormat,
    fill: [u8; 4],
    flip_green: &[bool],
) -> Result<Image, SplatBuildError> {
    // Measured against the albedo size, not against each other: every
    // array in a set shares one layer size.
    for (entry, material, path, size) in layers.sizes.iter().copied() {
        if size != (width, height) {
            return Err(SplatBuildError::Invalid(TextureSetError::MismatchedSize {
                entry,
                material: material.to_string(),
                path: path.to_string(),
                found: size,
                expected: (width, height),
            }));
        }
    }

    let texel_count = (width as usize) * (height as usize);
    let mut data = Vec::with_capacity(texel_count * 4 * entry_count.max(1));
    for (index, image) in layers.decoded.iter().enumerate() {
        match image {
            Some(image) => {
                let mut texels = converted(image, format)?;
                if flip_green.get(index).copied().unwrap_or(false) {
                    for texel in texels.chunks_exact_mut(4) {
                        texel[1] = 255 - texel[1];
                    }
                }
                data.extend_from_slice(&texels);
            }
            None => data.extend(fill.iter().copied().cycle().take(texel_count * 4)),
        }
    }
    Ok(array_image(
        width,
        height,
        entry_count.max(1) as u32,
        format,
        data,
    ))
}

/// An image's texels in `format`, converting if it decoded as something
/// else. The arrays are 8 bit per channel, and a texture pack may hand the
/// loader a 16-bit file for any slot; converting here keeps such a pack
/// usable without a pre-converted copy.
fn converted(image: &Image, format: TextureFormat) -> Result<Vec<u8>, SplatBuildError> {
    if image.texture_descriptor.format == format {
        return image.data.clone().ok_or(SplatBuildError::NotReady);
    }
    // Image data is stripped for the render world, so a decoded image
    // with none is unavailable; one that will not convert is an error,
    // and reporting it as still loading would retry forever.
    let Some(data) = image.data.as_deref() else {
        return Err(SplatBuildError::NotReady);
    };
    let from = image.texture_descriptor.format;
    if let Some(texels) = image.convert(format).and_then(|converted| converted.data) {
        return Ok(texels);
    }
    narrowed_to_rgba8(data, from, format).ok_or(SplatBuildError::Unconvertible { from, to: format })
}

/// Narrow 16-bit-per-channel texels to RGBA8, keeping the high byte of each
/// channel.
///
/// `Image::convert` routes through `DynamicImage`, which has no path out of
/// the formats bevy decodes deep files into: 16-bit grayscale lands on
/// `R16Uint`, grayscale+alpha on `Rg16Uint`, colour on `Rgba16Unorm`. A
/// single channel replicates across RGB, so a 16-bit height or roughness
/// map stacks as grey rather than red.
fn narrowed_to_rgba8(data: &[u8], from: TextureFormat, to: TextureFormat) -> Option<Vec<u8>> {
    if !matches!(
        to,
        TextureFormat::Rgba8Unorm | TextureFormat::Rgba8UnormSrgb
    ) {
        return None;
    }
    match from {
        TextureFormat::Rgba16Unorm | TextureFormat::Rgba16Uint => Some(
            data.chunks_exact(8)
                .flat_map(|t| [t[1], t[3], t[5], t[7]])
                .collect(),
        ),
        TextureFormat::Rg16Unorm | TextureFormat::Rg16Uint => Some(
            data.chunks_exact(4)
                .flat_map(|t| [t[1], t[1], t[1], t[3]])
                .collect(),
        ),
        TextureFormat::R16Unorm | TextureFormat::R16Uint => Some(
            data.chunks_exact(2)
                .flat_map(|t| [t[1], t[1], t[1], 255])
                .collect(),
        ),
        _ => None,
    }
}

fn fill_array(width: u32, height: u32, layers: u32, fill: [u8; 4]) -> Image {
    let texels = (width as usize) * (height as usize) * (layers as usize);
    let data: Vec<u8> = fill.iter().copied().cycle().take(texels * 4).collect();
    array_image(width, height, layers, TextureFormat::Rgba8Unorm, data)
}

fn array_image(
    width: u32,
    height: u32,
    layers: u32,
    format: TextureFormat,
    data: Vec<u8>,
) -> Image {
    let mut image = Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: layers,
        },
        TextureDimension::D2,
        data,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    // A one-layer array still has to present as an array to the shader,
    // which wgpu will not infer from `depth_or_array_layers == 1`.
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::D2Array),
        ..default()
    });
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

/// The control map as an uploadable image: `R32Uint`, one texel per grid
/// point, never filtered.
pub fn control_image(texels: &ControlTexels) -> Image {
    control_image_from_bytes(&texels.to_bytes(), texels.resolution)
}

/// [`control_image`] from a texel buffer the caller already holds.
///
/// An editor keeping the CPU-side mirror of the uploaded texture patches
/// the rows a brush touched and hands the whole buffer here, instead of
/// walking the terrain's control words back into bytes for every frame of
/// a stroke.
pub fn control_image_from_bytes(bytes: &[u8], resolution: u32) -> Image {
    let side = resolution.max(1);
    let mut data = bytes.to_vec();
    data.resize((side as usize) * (side as usize) * 4, 0);
    let mut image = Image::new(
        Extent3d {
            width: side,
            height: side,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::R32Uint,
        RenderAssetUsages::RENDER_WORLD,
    );
    // `textureLoad` ignores the sampler, but wgpu still validates one
    // against the format, and `R32Uint` is unfilterable.
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        mag_filter: ImageFilterMode::Nearest,
        min_filter: ImageFilterMode::Nearest,
        mipmap_filter: ImageFilterMode::Nearest,
        ..default()
    });
    image
}

/// A heightmap's slope field as an uploadable image: `R32Float`, one
/// texel per grid point, laid out exactly like the control map so a
/// fragment reads both with one grid coordinate.
///
/// The shader shades autoterrain from this rather than from the surface
/// it is drawing. The surface is a clipmap: the same ground is meshed at
/// several step sizes, each smoothing the relief by a different amount,
/// so a slope read off the geometry changes as a level hands over and the
/// ground re-textures itself under a moving camera. These values are per
/// grid point and the same from every distance.
pub fn slope_image(map: &Heightmap) -> Image {
    let side = map.resolution.max(1);
    let slopes = map.slope_field();
    let mut data = Vec::with_capacity((side as usize) * (side as usize) * 4);
    for texel in &slopes {
        data.extend_from_slice(&texel.to_le_bytes());
    }
    data.resize((side as usize) * (side as usize) * 4, 0);
    let mut image = Image::new(
        Extent3d {
            width: side,
            height: side,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::R32Float,
        RenderAssetUsages::RENDER_WORLD,
    );
    // `textureLoad` ignores the sampler, but wgpu still validates one
    // against the format, and `R32Float` is unfilterable.
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        mag_filter: ImageFilterMode::Nearest,
        min_filter: ImageFilterMode::Nearest,
        mipmap_filter: ImageFilterMode::Nearest,
        ..default()
    });
    image
}

/// Terrain shaded from a control map against a texture set.
///
/// One material per terrain: the control map and terrain size are the two
/// things that differ between them, and both live here rather than
/// per chunk, so every chunk of a terrain shares one material and one
/// upload.
#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
pub struct TerrainSplatMaterial {
    /// Per-id UV scales, four to a `Vec4`. The shader unpacks by id.
    #[uniform(0)]
    pub uv_scales: [Vec4; MAX_TEXTURES / 4],
    /// Per-id detiling strengths, packed the same way. 0 leaves a layer
    /// sampled exactly where its UV scale puts it.
    #[uniform(0)]
    pub detile_strengths: [Vec4; MAX_TEXTURES / 4],
    /// Terrain XZ extent in world units.
    #[uniform(0)]
    pub terrain_size: Vec2,
    /// `0..1`, remapped by the shader onto a `4..64` power exponent. Low is
    /// a soft cross-fade, high a near-binary height cutout.
    #[uniform(0)]
    pub blend_sharpness: f32,
    #[uniform(0)]
    pub perceptual_roughness: f32,
    /// Grid points per terrain edge; the control map's side length.
    #[uniform(0)]
    pub control_resolution: u32,
    /// Texture ids the bound set defines. Ids past it clamp to the last.
    #[uniform(0)]
    pub layer_count: u32,
    /// Nonzero where this terrain textures the cells no hand has claimed
    /// from their slope. 0 leaves every cell to its own control word.
    #[uniform(0)]
    pub autoterrain_enabled: u32,
    /// Texture id flat ground draws where autoterrain is on.
    #[uniform(0)]
    pub autoterrain_base_slot: u32,
    /// Texture id steep ground draws where autoterrain is on.
    #[uniform(0)]
    pub autoterrain_slope_slot: u32,
    /// Slope at which the base texture starts giving way, in radians.
    /// Converted from the authored degrees here so the shader carries no
    /// conversion of its own.
    #[uniform(0)]
    pub autoterrain_slope_start: f32,
    /// Slope at which the slope texture has fully taken over, in radians.
    #[uniform(0)]
    pub autoterrain_slope_end: f32,
    #[texture(1, dimension = "2d_array")]
    #[sampler(2)]
    pub albedo: Handle<Image>,
    #[texture(3, dimension = "2d_array")]
    pub normal: Handle<Image>,
    #[texture(4, dimension = "2d_array")]
    pub height: Handle<Image>,
    #[texture(5, sample_type = "u_int")]
    pub control: Handle<Image>,
    /// Slope per grid point, in radians. Only autoterrain reads it.
    #[texture(6, sample_type = "float", filterable = false)]
    pub slope: Handle<Image>,
}

impl Material for TerrainSplatMaterial {
    fn fragment_shader() -> ShaderRef {
        SHADER_PATH.into()
    }
}

impl TerrainSplatMaterial {
    /// Build a material for a terrain from its set, its arrays, its
    /// control map and how it textures the cells no hand has claimed.
    pub fn new(
        set: &TextureSet,
        arrays: SplatArrayHandles,
        control: Handle<Image>,
        slope: Handle<Image>,
        terrain_size: Vec2,
        control_resolution: u32,
        autoterrain: AutoTerrainSettings,
    ) -> Self {
        let scales = set.uv_scales();
        let mut uv_scales = [Vec4::ZERO; MAX_TEXTURES / 4];
        for (i, scale) in scales.iter().enumerate() {
            uv_scales[i / 4][i % 4] = *scale;
        }
        let mut detile_strengths = [Vec4::ZERO; MAX_TEXTURES / 4];
        for (i, strength) in set.detile_strengths().iter().enumerate() {
            detile_strengths[i / 4][i % 4] = *strength;
        }
        let mut material = Self {
            uv_scales,
            detile_strengths,
            terrain_size,
            blend_sharpness: DEFAULT_BLEND_SHARPNESS,
            perceptual_roughness: 0.9,
            control_resolution,
            layer_count: set.len().max(1) as u32,
            autoterrain_enabled: 0,
            autoterrain_base_slot: 0,
            autoterrain_slope_slot: 0,
            autoterrain_slope_start: 0.0,
            autoterrain_slope_end: 0.0,
            albedo: arrays.albedo,
            normal: arrays.normal,
            height: arrays.height,
            control,
            slope,
        };
        material.set_autoterrain(autoterrain);
        material
    }

    /// Point the autoterrain half of the uniform at `settings`.
    ///
    /// Separate from [`Self::new`] because changing these is a uniform
    /// write: the control map, the arrays and the mesh all stand, so an
    /// editor moving a slider re-uploads one buffer rather than
    /// rebuilding a terrain.
    ///
    /// The settings are sanitized on the way in. They reach a
    /// `smoothstep` the fragment cannot guard, and a caller that read
    /// them from anywhere but a decoded sidecar has not been through that
    /// check.
    pub fn set_autoterrain(&mut self, settings: AutoTerrainSettings) {
        let settings = settings.sanitized();
        self.autoterrain_enabled = u32::from(settings.enabled);
        self.autoterrain_base_slot = settings.base_slot as u32;
        self.autoterrain_slope_slot = settings.slope_slot as u32;
        self.autoterrain_slope_start = settings.slope_start_deg.to_radians();
        self.autoterrain_slope_end = settings.slope_end_deg.to_radians();
    }
}

/// The three array images once they are in `Assets<Image>`.
#[derive(Clone, Debug)]
pub struct SplatArrayHandles {
    pub albedo: Handle<Image>,
    pub normal: Handle<Image>,
    pub height: Handle<Image>,
}

/// Registers the texture-set asset, its loader, the splat material and its
/// shader.
pub struct TerrainRenderPlugin;

impl Plugin for TerrainRenderPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "shaders/terrain_splat.wgsl");
        app.add_plugins(MaterialPlugin::<TerrainSplatMaterial>::default());
    }
}

/// Resolution: turning material names into entries, through a lookup
/// closure standing in for the host's name store.
#[cfg(test)]
mod resolve_tests {
    use std::collections::HashMap;

    use bevy::app::App;
    use bevy::asset::{AssetPlugin, AssetServer, Assets};
    use bevy::prelude::*;

    use super::{ResolvedSlots, resolve_with};
    use crate::sidecar::TerrainMaterialSlot;

    /// A host's name store, minus everything resolution does not use.
    #[derive(Resource, Default)]
    struct Saved(HashMap<String, Handle<StandardMaterial>>);

    fn resolve_app() -> App {
        let mut app = App::new();
        app.add_plugins((bevy::app::TaskPoolPlugin::default(), AssetPlugin::default()));
        app.init_asset::<Image>();
        app.init_asset::<StandardMaterial>();
        app.init_resource::<Saved>();
        app
    }

    /// Save a material under `name` with the three slots a terrain reads.
    fn saved_material(app: &mut App, name: &str, flip_y: bool) {
        let server = app.world().resource::<AssetServer>().clone();
        let base = server.load::<Image>(format!("t/{name}_base.png"));
        let normal = server.load::<Image>(format!("t/{name}_normal.png"));
        let depth = server.load::<Image>(format!("t/{name}_height.png"));
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                base_color_texture: Some(base),
                normal_map_texture: Some(normal),
                depth_map: Some(depth),
                flip_normal_map_y: flip_y,
                ..default()
            });
        app.world_mut()
            .resource_mut::<Saved>()
            .0
            .insert(name.to_string(), handle);
    }

    fn resolve_slots(app: &App, slots: &[TerrainMaterialSlot]) -> ResolvedSlots {
        let saved = app.world().resource::<Saved>();
        let materials = app.world().resource::<Assets<StandardMaterial>>();
        resolve_with(
            slots,
            |name| saved.0.get(name).and_then(|handle| materials.get(handle)),
            app.world().resource::<AssetServer>(),
        )
    }

    /// Base colour is albedo, the normal map is the normal, and the depth
    /// map is the height.
    #[test]
    fn a_materials_three_slots_become_albedo_normal_and_height() {
        let mut app = resolve_app();
        saved_material(&mut app, "grass", false);

        let resolved = resolve_slots(
            &app,
            &[TerrainMaterialSlot {
                material: "grass".to_string(),
                uv_scale: 0.3,
                detile: 0.6,
            }],
        );

        assert!(resolved.missing.is_empty());
        let entry = &resolved.set.entries[0];
        assert_eq!(entry.material, "grass");
        assert_eq!(entry.albedo.as_deref(), Some("t/grass_base.png"));
        assert_eq!(entry.normal.as_deref(), Some("t/grass_normal.png"));
        assert_eq!(entry.height.as_deref(), Some("t/grass_height.png"));
        assert_eq!(entry.uv_scale, 0.3, "tiling comes from the slot");
        assert_eq!(entry.detile, 0.6, "so does detiling");
        assert!(!entry.flip_normal_y);
        assert!(resolved.images.albedo[0].is_some());
        assert!(resolved.images.normal[0].is_some());
        assert!(resolved.images.height[0].is_some());
        assert_eq!(resolved.set.validate(), Ok(()));
    }

    /// A material whose normal map has green pointing down carries that
    /// flag to the array builder, the only place that can flip it before
    /// the shader samples it.
    #[test]
    fn a_materials_normal_convention_flag_reaches_the_texture_entry() {
        let mut app = resolve_app();
        saved_material(&mut app, "rock", true);

        let resolved = resolve_slots(&app, &[TerrainMaterialSlot::new("rock")]);

        assert!(resolved.set.entries[0].flip_normal_y);
    }

    /// A terrain referencing a material with no file keeps the slot:
    /// dropping it would renumber every id painted after it. The name is
    /// reported so the host can name the one that went.
    #[test]
    fn a_missing_material_keeps_its_slot_and_is_named() {
        let mut app = resolve_app();
        saved_material(&mut app, "grass", false);

        let resolved = resolve_slots(
            &app,
            &[
                TerrainMaterialSlot::new("grass"),
                TerrainMaterialSlot::new("deleted"),
                TerrainMaterialSlot::new("grass2"),
            ],
        );

        assert_eq!(resolved.set.entries.len(), 3, "ids keep their positions");
        assert_eq!(resolved.set.entries[1].material, "deleted");
        assert_eq!(resolved.set.entries[1].albedo, None);
        assert_eq!(resolved.images.albedo[1], None);
        assert_eq!(resolved.missing, vec!["deleted", "grass2"]);
    }

    /// A vacated id keeps its place and draws the fallback, and is not
    /// reported as missing.
    #[test]
    fn a_vacated_slot_holds_its_id_without_being_called_missing() {
        let mut app = resolve_app();
        saved_material(&mut app, "grass", false);

        let resolved = resolve_slots(
            &app,
            &[
                TerrainMaterialSlot::new("grass"),
                TerrainMaterialSlot::tombstone(),
                TerrainMaterialSlot::new("grass"),
            ],
        );

        assert_eq!(resolved.set.entries.len(), 3);
        assert!(resolved.set.entries[1].is_vacant());
        assert_eq!(resolved.images.albedo[1], None);
        assert!(resolved.missing.is_empty());
    }

    /// A material with no textures at all is not an error: it resolves to
    /// a slot that draws the fallback, the same as a missing one, but is
    /// not reported as missing because its name still resolves.
    #[test]
    fn a_material_with_no_textures_resolves_without_being_called_missing() {
        let mut app = resolve_app();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        app.world_mut()
            .resource_mut::<Saved>()
            .0
            .insert("plain".to_string(), handle);

        let resolved = resolve_slots(&app, &[TerrainMaterialSlot::new("plain")]);

        assert!(resolved.missing.is_empty());
        assert_eq!(resolved.set.entries[0].albedo, None);
        assert_eq!(resolved.set.validate(), Ok(()));
    }

    /// The entries and the three handle lists are one row per slot, so
    /// the array builder can read them by index.
    #[test]
    fn every_slot_contributes_one_entry_and_one_handle_row() {
        let mut app = resolve_app();
        saved_material(&mut app, "grass", false);
        let slots = [
            TerrainMaterialSlot::new("grass"),
            TerrainMaterialSlot::tombstone(),
            TerrainMaterialSlot::new("gone"),
        ];

        let resolved = resolve_slots(&app, &slots);

        assert_eq!(resolved.set.entries.len(), slots.len());
        assert_eq!(resolved.images.albedo.len(), slots.len());
        assert_eq!(resolved.images.normal.len(), slots.len());
        assert_eq!(resolved.images.height.len(), slots.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Control;
    use crate::texture_set::TextureSetEntry;

    fn solid(width: u32, height: u32, texel: [u8; 4], format: TextureFormat) -> Image {
        let data: Vec<u8> = texel
            .iter()
            .copied()
            .cycle()
            .take((width as usize) * (height as usize) * 4)
            .collect();
        Image::new(
            Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            data,
            format,
            RenderAssetUsages::RENDER_WORLD,
        )
    }

    /// A solid image whose texel is however many bytes `texel` is, for the
    /// formats bevy decodes deep files into.
    fn solid_texel(width: u32, height: u32, texel: &[u8], format: TextureFormat) -> Image {
        let data: Vec<u8> = texel
            .iter()
            .copied()
            .cycle()
            .take((width as usize) * (height as usize) * texel.len())
            .collect();
        Image::new(
            Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            data,
            format,
            RenderAssetUsages::RENDER_WORLD,
        )
    }

    /// A set of solid-colour entries, all loaded, one per requested size.
    fn loaded_set(sizes: &[(u32, u32)]) -> (Assets<Image>, TextureSet, TextureSetImages) {
        let mut images = Assets::<Image>::default();
        let mut albedo = Vec::new();
        let mut entries = Vec::new();
        for (i, (w, h)) in sizes.iter().enumerate() {
            albedo.push(Some(images.add(solid(
                *w,
                *h,
                [10, 20, 30, 255],
                TextureFormat::Rgba8UnormSrgb,
            ))));
            entries.push(TextureSetEntry::new(format!("m{i}"), format!("t{i}.png")));
        }
        let count = sizes.len();
        (
            images,
            TextureSet { entries },
            TextureSetImages {
                albedo,
                normal: vec![None; count],
                height: vec![None; count],
            },
        )
    }

    #[test]
    fn a_uniform_set_stacks_into_one_layer_per_entry() {
        let (images, set, handles) = loaded_set(&[(4, 4), (4, 4), (4, 4)]);
        let built = splat_images(&set, &handles, &images).expect("stacks");
        assert_eq!(built.albedo.texture_descriptor.size.width, 4);
        assert_eq!(
            built.albedo.texture_descriptor.size.depth_or_array_layers,
            3
        );
        assert_eq!(
            built
                .albedo
                .texture_view_descriptor
                .as_ref()
                .unwrap()
                .dimension,
            Some(TextureViewDimension::D2Array)
        );
    }

    #[test]
    fn a_single_entry_set_still_presents_as_an_array() {
        let (images, set, handles) = loaded_set(&[(2, 2)]);
        let built = splat_images(&set, &handles, &images).expect("stacks");
        assert_eq!(
            built.albedo.texture_descriptor.size.depth_or_array_layers,
            1
        );
        assert_eq!(
            built
                .albedo
                .texture_view_descriptor
                .as_ref()
                .unwrap()
                .dimension,
            Some(TextureViewDimension::D2Array)
        );
    }

    #[test]
    fn a_mismatched_albedo_names_the_entry_and_both_sizes() {
        let (images, set, handles) = loaded_set(&[(4, 4), (8, 4)]);
        let err = splat_images(&set, &handles, &images).expect_err("mismatched sizes");
        let SplatBuildError::Invalid(reason) = err else {
            panic!("expected an invalid-set error, got {err:?}");
        };
        let message = reason.to_string();
        assert!(message.contains("t1.png"), "{message}");
        assert!(message.contains("8x4"), "{message}");
        assert!(message.contains("4x4"), "{message}");
    }

    /// `splat_images` is public, so handles that do not line up with the
    /// entries are refused rather than turned into an undersized texture.
    #[test]
    fn handles_with_no_layers_at_all_are_refused_rather_than_built() {
        let images = Assets::<Image>::default();
        let set = TextureSet {
            entries: vec![TextureSetEntry::new("grass", "a.png")],
        };
        let err = splat_images(&set, &TextureSetImages::default(), &images)
            .expect_err("handles that name no layers are refused");
        assert_eq!(err, SplatBuildError::Invalid(TextureSetError::Empty));
    }

    #[test]
    fn an_image_that_has_not_decoded_yet_reports_not_ready_rather_than_failing() {
        let (_, set, handles) = loaded_set(&[(4, 4)]);
        let empty = Assets::<Image>::default();
        assert!(matches!(
            splat_images(&set, &handles, &empty),
            Err(SplatBuildError::NotReady)
        ));
    }

    #[test]
    fn a_set_with_no_normal_maps_gets_a_one_texel_flat_array() {
        let (images, set, handles) = loaded_set(&[(64, 64), (64, 64)]);
        let built = splat_images(&set, &handles, &images).expect("stacks");
        assert_eq!(built.normal.texture_descriptor.size.width, 1);
        assert_eq!(
            built.normal.texture_descriptor.size.depth_or_array_layers,
            2
        );
        assert_eq!(
            built.normal.data.as_ref().unwrap()[..4],
            FLAT_NORMAL_TEXEL,
            "a set with no normal maps must shade flat, not black"
        );
        assert_eq!(
            built.height.data.as_ref().unwrap()[..4],
            FLAT_HEIGHT_TEXEL,
            "a set with no height maps must blend evenly, not lose every band"
        );
    }

    /// Optionality is per entry: one entry with a height map and one
    /// without must both end up in a full-size array, the second filled.
    #[test]
    fn an_entry_without_a_height_map_gets_a_filled_layer_beside_one_that_has_it() {
        let mut images = Assets::<Image>::default();
        let a = images.add(solid(4, 4, [1, 2, 3, 255], TextureFormat::Rgba8UnormSrgb));
        let b = images.add(solid(4, 4, [4, 5, 6, 255], TextureFormat::Rgba8UnormSrgb));
        let h = images.add(solid(4, 4, [7, 7, 7, 255], TextureFormat::Rgba8Unorm));
        let set = TextureSet {
            entries: vec![
                TextureSetEntry {
                    height: Some("a_h.png".into()),
                    ..TextureSetEntry::new("grass", "a.png")
                },
                TextureSetEntry::new("rock", "b.png"),
            ],
        };
        let handles = TextureSetImages {
            albedo: vec![Some(a), Some(b)],
            normal: vec![None, None],
            height: vec![Some(h), None],
        };

        let built = splat_images(&set, &handles, &images).expect("stacks");
        assert_eq!(built.height.texture_descriptor.size.width, 4);
        assert_eq!(
            built.height.texture_descriptor.size.depth_or_array_layers,
            2
        );
        let data = built.height.data.as_ref().unwrap();
        assert_eq!(data[0], 7, "entry 0 keeps its own height map");
        assert_eq!(
            data[4 * 4 * 4],
            FLAT_HEIGHT_TEXEL[0],
            "entry 1 is filled, not left black"
        );
    }

    #[test]
    fn a_height_map_of_the_wrong_size_names_itself_not_the_albedo() {
        let mut images = Assets::<Image>::default();
        let a = images.add(solid(4, 4, [1, 2, 3, 255], TextureFormat::Rgba8UnormSrgb));
        let h = images.add(solid(8, 8, [7, 7, 7, 255], TextureFormat::Rgba8Unorm));
        let set = TextureSet {
            entries: vec![TextureSetEntry {
                height: Some("tall_h.png".into()),
                ..TextureSetEntry::new("grass", "a.png")
            }],
        };
        let handles = TextureSetImages {
            albedo: vec![Some(a)],
            normal: vec![None],
            height: vec![Some(h)],
        };
        let err = splat_images(&set, &handles, &images).expect_err("mismatched height map");
        let SplatBuildError::Invalid(reason) = err else {
            panic!("expected an invalid-set error, got {err:?}");
        };
        assert!(reason.to_string().contains("tall_h.png"), "{reason}");
        assert!(reason.to_string().contains("grass"), "{reason}");
    }

    /// A slot whose material has no file behind it must still occupy its
    /// own layer: dropping it would shift every id painted after it.
    #[test]
    fn a_slot_with_no_albedo_gets_a_fallback_layer_and_keeps_its_id() {
        let mut images = Assets::<Image>::default();
        let a = images.add(solid(4, 4, [1, 2, 3, 255], TextureFormat::Rgba8UnormSrgb));
        let c = images.add(solid(4, 4, [9, 9, 9, 255], TextureFormat::Rgba8UnormSrgb));
        let set = TextureSet {
            entries: vec![
                TextureSetEntry::new("grass", "a.png"),
                TextureSetEntry::unresolved("gone", 0.1),
                TextureSetEntry::new("rock", "c.png"),
            ],
        };
        let handles = TextureSetImages {
            albedo: vec![Some(a), None, Some(c)],
            normal: vec![None, None, None],
            height: vec![None, None, None],
        };

        let built = splat_images(&set, &handles, &images).expect("stacks");
        assert_eq!(
            built.albedo.texture_descriptor.size.depth_or_array_layers, 3,
            "the missing slot must keep its own layer",
        );
        let data = built.albedo.data.as_ref().unwrap();
        let layer = 4 * 4 * 4;
        assert_eq!(data[0], 1, "slot 0 still draws its own texture");
        assert_eq!(
            data[layer..layer + 4],
            FALLBACK_ALBEDO_TEXEL,
            "the missing slot draws the fallback rather than nothing",
        );
        assert_eq!(data[layer * 2], 9, "slot 2 kept its id");
    }

    /// Every material missing leaves nothing to measure, so the arrays
    /// collapse to one texel rather than failing to build at all, and the
    /// terrain still renders and stays paintable.
    #[test]
    fn a_set_whose_materials_all_went_missing_still_builds() {
        let images = Assets::<Image>::default();
        let set = TextureSet {
            entries: vec![
                TextureSetEntry::unresolved("gone", 0.1),
                TextureSetEntry::unresolved("also_gone", 0.1),
            ],
        };
        let handles = TextureSetImages {
            albedo: vec![None, None],
            normal: vec![None, None],
            height: vec![None, None],
        };
        let built = splat_images(&set, &handles, &images).expect("stacks");
        assert_eq!(built.albedo.texture_descriptor.size.width, 1);
        assert_eq!(
            built.albedo.texture_descriptor.size.depth_or_array_layers,
            2
        );
        assert_eq!(
            built.albedo.data.as_ref().unwrap()[..4],
            FALLBACK_ALBEDO_TEXEL
        );
    }

    /// A normal map authored with green pointing down is flipped once
    /// here, on the way into the array, so the shader never has to know
    /// which convention a material used.
    #[test]
    fn a_material_flagged_for_dx_normals_has_its_green_channel_flipped() {
        let mut images = Assets::<Image>::default();
        let a = images.add(solid(2, 2, [1, 2, 3, 255], TextureFormat::Rgba8UnormSrgb));
        let b = images.add(solid(2, 2, [4, 5, 6, 255], TextureFormat::Rgba8UnormSrgb));
        let dx = images.add(solid(2, 2, [128, 40, 255, 255], TextureFormat::Rgba8Unorm));
        let gl = images.add(solid(2, 2, [128, 40, 255, 255], TextureFormat::Rgba8Unorm));
        let set = TextureSet {
            entries: vec![
                TextureSetEntry {
                    normal: Some("dx_n.png".into()),
                    flip_normal_y: true,
                    ..TextureSetEntry::new("dx_rock", "a.png")
                },
                TextureSetEntry {
                    normal: Some("gl_n.png".into()),
                    ..TextureSetEntry::new("gl_rock", "b.png")
                },
            ],
        };
        let handles = TextureSetImages {
            albedo: vec![Some(a), Some(b)],
            normal: vec![Some(dx), Some(gl)],
            height: vec![None, None],
        };

        let built = splat_images(&set, &handles, &images).expect("stacks");
        let data = built.normal.data.as_ref().unwrap();
        let layer = 2 * 2 * 4;
        assert_eq!(data[1], 255 - 40, "the flagged layer's green is inverted");
        assert_eq!(data[0], 128, "red is untouched");
        assert_eq!(data[2], 255, "blue is untouched");
        assert_eq!(
            data[layer + 1],
            40,
            "an unflagged layer in the same set keeps its green",
        );
    }

    /// A downloaded pack ships 16-bit maps. They must stack from the
    /// originals, narrowed here, rather than needing a converted copy.
    #[test]
    fn a_sixteen_bit_set_stacks_from_the_high_byte_of_every_channel() {
        let mut images = Assets::<Image>::default();
        let albedo = images.add(solid_texel(
            2,
            2,
            &[0x00, 0x11, 0x00, 0x22, 0x00, 0x33, 0xff, 0xff],
            TextureFormat::Rgba16Unorm,
        ));
        let normal = images.add(solid_texel(
            2,
            2,
            &[0x00, 0x80, 0x00, 0x40, 0xff, 0xff, 0xff, 0xff],
            TextureFormat::Rgba16Unorm,
        ));
        let height = images.add(solid_texel(2, 2, &[0x00, 0x99], TextureFormat::R16Uint));
        let set = TextureSet {
            entries: vec![TextureSetEntry {
                normal: Some("n.png".into()),
                height: Some("h.png".into()),
                ..TextureSetEntry::new("rock", "a.png")
            }],
        };
        let handles = TextureSetImages {
            albedo: vec![Some(albedo)],
            normal: vec![Some(normal)],
            height: vec![Some(height)],
        };

        let built = splat_images(&set, &handles, &images).expect("stacks");
        assert_eq!(
            built.albedo.texture_descriptor.format,
            TextureFormat::Rgba8UnormSrgb
        );
        assert_eq!(built.albedo.data.as_ref().unwrap()[..4], [17, 34, 51, 255]);
        assert_eq!(
            built.normal.data.as_ref().unwrap()[..4],
            [128, 64, 255, 255]
        );
        assert_eq!(
            built.height.data.as_ref().unwrap()[..4],
            [153, 153, 153, 255],
            "a single-channel map greys across RGB rather than stacking as red",
        );
    }

    /// Narrowing happens on the way in, so the flip still lands on the
    /// green channel of a 16-bit normal map.
    #[test]
    fn a_sixteen_bit_normal_map_flagged_for_dx_still_flips_its_green() {
        let mut images = Assets::<Image>::default();
        let albedo = images.add(solid(2, 2, [1, 2, 3, 255], TextureFormat::Rgba8UnormSrgb));
        let normal = images.add(solid_texel(
            2,
            2,
            &[0x00, 0x80, 0x00, 0x40, 0xff, 0xff, 0xff, 0xff],
            TextureFormat::Rgba16Unorm,
        ));
        let set = TextureSet {
            entries: vec![TextureSetEntry {
                normal: Some("n.png".into()),
                flip_normal_y: true,
                ..TextureSetEntry::new("rock", "a.png")
            }],
        };
        let handles = TextureSetImages {
            albedo: vec![Some(albedo)],
            normal: vec![Some(normal)],
            height: vec![None],
        };

        let built = splat_images(&set, &handles, &images).expect("stacks");
        assert_eq!(built.normal.data.as_ref().unwrap()[1], 255 - 64);
    }

    /// Grayscale-plus-alpha lands on `Rg16Uint`: the grey channel spreads
    /// across RGB and the second channel stays the alpha it was.
    #[test]
    fn a_sixteen_bit_grayscale_alpha_height_map_keeps_its_alpha() {
        let mut images = Assets::<Image>::default();
        let albedo = images.add(solid(2, 2, [1, 2, 3, 255], TextureFormat::Rgba8UnormSrgb));
        let height = images.add(solid_texel(
            2,
            2,
            &[0x00, 0x40, 0x00, 0x80],
            TextureFormat::Rg16Uint,
        ));
        let set = TextureSet {
            entries: vec![TextureSetEntry {
                height: Some("h.png".into()),
                ..TextureSetEntry::new("rock", "a.png")
            }],
        };
        let handles = TextureSetImages {
            albedo: vec![Some(albedo)],
            normal: vec![None],
            height: vec![Some(height)],
        };

        let built = splat_images(&set, &handles, &images).expect("stacks");
        assert_eq!(built.height.data.as_ref().unwrap()[..4], [64, 64, 64, 128]);
    }

    /// A format with no narrowing path still fails, and the message names
    /// the two formats rather than blaming a colour space it cannot know.
    #[test]
    fn a_format_with_no_narrowing_path_names_both_formats() {
        let mut images = Assets::<Image>::default();
        let albedo = images.add(solid(2, 2, [1, 2, 3, 255], TextureFormat::Rgba8UnormSrgb));
        let normal = images.add(solid_texel(2, 2, &[0u8; 16], TextureFormat::Rgba32Float));
        let set = TextureSet {
            entries: vec![TextureSetEntry {
                normal: Some("n.exr".into()),
                ..TextureSetEntry::new("rock", "a.png")
            }],
        };
        let handles = TextureSetImages {
            albedo: vec![Some(albedo)],
            normal: vec![Some(normal)],
            height: vec![None],
        };

        let err = splat_images(&set, &handles, &images).expect_err("no narrowing path");
        assert_eq!(
            err,
            SplatBuildError::Unconvertible {
                from: TextureFormat::Rgba32Float,
                to: TextureFormat::Rgba8Unorm,
            }
        );
        let message = err.to_string();
        assert!(message.contains("Rgba32Float"), "{message}");
        assert!(message.contains("Rgba8Unorm"), "{message}");
        assert!(!message.contains("sRGB"), "{message}");
    }

    #[test]
    fn an_unusable_set_reports_invalid_rather_than_not_ready() {
        let (images, mut set, handles) = loaded_set(&[(4, 4)]);
        set.entries[0].uv_scale = 0.0;
        assert!(matches!(
            splat_images(&set, &handles, &images),
            Err(SplatBuildError::Invalid(TextureSetError::BadUvScale { .. }))
        ));
    }

    #[test]
    fn the_control_image_is_unfiltered_r32uint_at_the_terrain_resolution() {
        let control = vec![Control::default().with_base_id(3); 16];
        let texels = ControlTexels::from_control(&control, 4);
        let image = control_image(&texels);
        assert_eq!(image.texture_descriptor.format, TextureFormat::R32Uint);
        assert_eq!(image.texture_descriptor.size.width, 4);
        assert_eq!(image.texture_descriptor.size.height, 4);
        assert_eq!(image.data.as_ref().unwrap().len(), 4 * 4 * 4);
        assert_eq!(&image.data.as_ref().unwrap()[0..4], &3u32.to_le_bytes());
    }

    #[test]
    fn a_zero_resolution_terrain_still_produces_a_bindable_control_image() {
        let image = control_image(&ControlTexels::from_control(&[], 0));
        assert_eq!(image.texture_descriptor.size.width, 1);
        assert_eq!(image.data.as_ref().unwrap().len(), 4);
    }

    /// The control-word layout the shader unpacks by hand rather than
    /// through an accessor. A drift in the manual bit would silently
    /// shade painted cells as unclaimed ground; a drift in a shift or a
    /// mask would hand every cell the wrong texture id.
    #[test]
    fn the_shaders_control_layout_matches_the_control_words() {
        use crate::control::{
            BASE_MASK, BASE_SHIFT, BLEND_MASK, BLEND_SHIFT, MANUAL_BIT, OVERLAY_MASK, OVERLAY_SHIFT,
        };

        let pinned = |line: String| {
            assert!(
                SHADER_SOURCE.contains(&line),
                "the shader drifted from control.rs: expected `{line}`"
            );
        };
        pinned(format!("const BASE_SHIFT: u32 = {BASE_SHIFT}u;"));
        pinned(format!("const BASE_MASK: u32 = 0x{BASE_MASK:X}u;"));
        pinned(format!("const OVERLAY_SHIFT: u32 = {OVERLAY_SHIFT}u;"));
        pinned(format!("const OVERLAY_MASK: u32 = 0x{OVERLAY_MASK:X}u;"));
        pinned(format!("const BLEND_SHIFT: u32 = {BLEND_SHIFT}u;"));
        pinned(format!("const BLEND_MASK: u32 = 0x{BLEND_MASK:X}u;"));
        pinned(format!("const MANUAL_BIT: u32 = 0x{MANUAL_BIT:X}u;"));
    }

    fn test_material(autoterrain: AutoTerrainSettings) -> TerrainSplatMaterial {
        TerrainSplatMaterial::new(
            &TextureSet {
                entries: vec![TextureSetEntry::new("grass", "a.png")],
            },
            SplatArrayHandles {
                albedo: Handle::default(),
                normal: Handle::default(),
                height: Handle::default(),
            },
            Handle::default(),
            Handle::default(),
            Vec2::splat(100.0),
            256,
            autoterrain,
        )
    }

    /// Off is the default, and off means the uniform says nothing the
    /// shader's guard can act on.
    #[test]
    fn a_terrain_that_never_asked_for_autoterrain_uploads_it_disabled() {
        let material = test_material(AutoTerrainSettings::default());
        assert_eq!(material.autoterrain_enabled, 0);
    }

    /// The angles are authored in degrees and sampled in radians, so the
    /// conversion happens once, here.
    #[test]
    fn autoterrain_slots_and_angles_reach_the_uniform_in_radians() {
        let material = test_material(AutoTerrainSettings {
            enabled: true,
            base_slot: 1,
            slope_slot: 3,
            slope_start_deg: 30.0,
            slope_end_deg: 60.0,
        });

        assert_eq!(material.autoterrain_enabled, 1);
        assert_eq!(material.autoterrain_base_slot, 1);
        assert_eq!(material.autoterrain_slope_slot, 3);
        assert!((material.autoterrain_slope_start - core::f32::consts::FRAC_PI_6).abs() < 1e-6);
        assert!((material.autoterrain_slope_end - core::f32::consts::FRAC_PI_3).abs() < 1e-6);
    }

    /// Settings that never went through a sidecar decode still cannot
    /// reach the shader's `smoothstep` as a NaN or a backwards range.
    #[test]
    fn unsanitized_autoterrain_settings_are_pulled_back_before_they_are_uploaded() {
        let material = test_material(AutoTerrainSettings {
            enabled: true,
            base_slot: 0,
            slope_slot: 1,
            slope_start_deg: 80.0,
            slope_end_deg: f32::NAN,
        });

        assert!(material.autoterrain_slope_start.is_finite());
        assert!(material.autoterrain_slope_end.is_finite());
        assert!(
            material.autoterrain_slope_start <= material.autoterrain_slope_end,
            "the range must reach the shader the right way round",
        );
    }

    /// Changing the settings is a uniform write: nothing else about the
    /// material moves, so an editor moving a slider re-uploads one
    /// buffer rather than rebuilding the terrain.
    #[test]
    fn setting_autoterrain_again_touches_nothing_but_its_own_fields() {
        let mut material = test_material(AutoTerrainSettings::default());
        let before = material.clone();

        material.set_autoterrain(AutoTerrainSettings {
            enabled: true,
            base_slot: 2,
            slope_slot: 4,
            slope_start_deg: 10.0,
            slope_end_deg: 20.0,
        });

        assert_eq!(material.autoterrain_enabled, 1);
        assert_eq!(material.uv_scales, before.uv_scales);
        assert_eq!(material.detile_strengths, before.detile_strengths);
        assert_eq!(material.control_resolution, before.control_resolution);
        assert_eq!(material.layer_count, before.layer_count);
        assert_eq!(material.terrain_size, before.terrain_size);
        assert_eq!(material.control, before.control);
    }

    #[test]
    fn uv_scales_reach_the_uniform_in_id_order() {
        let set = TextureSet {
            entries: vec![
                TextureSetEntry {
                    uv_scale: 0.25,
                    ..TextureSetEntry::new("grass", "a.png")
                },
                TextureSetEntry {
                    uv_scale: 4.0,
                    ..TextureSetEntry::new("rock", "b.png")
                },
            ],
        };
        let arrays = SplatArrayHandles {
            albedo: Handle::default(),
            normal: Handle::default(),
            height: Handle::default(),
        };
        let material = TerrainSplatMaterial::new(
            &set,
            arrays,
            Handle::default(),
            Handle::default(),
            Vec2::splat(100.0),
            256,
            AutoTerrainSettings::default(),
        );
        assert_eq!(material.uv_scales[0].x, 0.25);
        assert_eq!(material.uv_scales[0].y, 4.0);
        assert_eq!(material.layer_count, 2);
        assert_eq!(material.control_resolution, 256);
        assert_eq!(material.terrain_size, Vec2::splat(100.0));
    }

    /// The shader source, for the binding checks below.
    ///
    /// The source cannot be compiled here: it is naga-oil input, not
    /// WGSL. It opens with `#import bevy_pbr::{...}` and declares
    /// `@group(#{MATERIAL_BIND_GROUP})`, neither of which naga parses, and
    /// composing it would mean pulling in every `bevy_pbr` shader it
    /// imports. The checks below are textual: they catch binding numbers
    /// drifting between the `AsBindGroup` derive and the shader that
    /// reads them.
    const SHADER_SOURCE: &str = include_str!("shaders/terrain_splat.wgsl");

    #[test]
    fn every_material_binding_is_declared_in_the_shader_at_its_derive_index() {
        for (binding, declaration) in [
            (0, "var<uniform> splat: SplatUniform"),
            (1, "var albedo_array: texture_2d_array<f32>"),
            (2, "var layer_sampler: sampler"),
            (3, "var normal_array: texture_2d_array<f32>"),
            (4, "var height_array: texture_2d_array<f32>"),
            (5, "var control_map: texture_2d<u32>"),
        ] {
            let expected =
                format!("@group(#{{MATERIAL_BIND_GROUP}}) @binding({binding}) {declaration};");
            assert!(
                SHADER_SOURCE.contains(&expected),
                "the AsBindGroup derive binds {binding} as `{declaration}`, \
                 but the shader does not declare it that way"
            );
        }
    }

    /// The uniform is one buffer built from the `#[uniform(0)]` fields in
    /// declaration order, so the WGSL struct has to list them in the same
    /// order or every field reads its neighbour's bytes.
    #[test]
    fn the_uniform_struct_matches_the_materials_field_order() {
        let start = SHADER_SOURCE
            .find("struct SplatUniform {")
            .expect("SplatUniform is declared");
        let body = &SHADER_SOURCE[start..];
        let end = body.find('}').expect("SplatUniform is closed");
        let body = &body[..end];

        let mut at = 0usize;
        for field in [
            "uv_scales: array<vec4<f32>, 4>",
            "detile_strengths: array<vec4<f32>, 4>",
            "terrain_size: vec2<f32>",
            "blend_sharpness: f32",
            "perceptual_roughness: f32",
            "control_resolution: u32",
            "layer_count: u32",
            "autoterrain_enabled: u32",
            "autoterrain_base_slot: u32",
            "autoterrain_slope_slot: u32",
            "autoterrain_slope_start: f32",
            "autoterrain_slope_end: f32",
        ] {
            let found = body[at..]
                .find(field)
                .unwrap_or_else(|| panic!("`{field}` is missing or out of order in SplatUniform"));
            at += found + field.len();
        }
    }

    /// `MAX_TEXTURES` sizes the UV-scale array on both sides.
    #[test]
    fn the_shaders_layer_ceiling_matches_the_rust_one() {
        assert_eq!(MAX_TEXTURES, 16);
        assert!(
            SHADER_SOURCE.contains("const MAX_LAYERS: u32 = 16u;"),
            "the shader's layer ceiling drifted from MAX_TEXTURES"
        );
        assert!(
            SHADER_SOURCE.contains("uv_scales: array<vec4<f32>, 4>"),
            "the UV-scale array must hold MAX_TEXTURES / 4 vec4s"
        );
    }
}
