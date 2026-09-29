//! Pixels of the images a terrain stacks into its texture arrays.
//!
//! A drawn texture keeps its pixels only on the GPU, so the arrays read their
//! layers from the files instead, and hold them only while a terrain needs them.

use bevy::asset::{AssetPath, AssetServerMode, RenderAssetUsages};
use bevy::image::{CompressedImageFormats, ImageFormat, ImageSampler, ImageType};
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, poll_once};

use super::TextureSetImages;

/// The first bytes of a Basis Universal file. An imported texture is served
/// as one whatever its extension says.
const BASIS_SIGNATURE: [u8; 2] = *b"sB";

/// Layer pixels read from their files, by the image they stand in for.
#[derive(Resource, Default)]
pub struct LayerTexels {
    reads: HashMap<AssetId<Image>, LayerRead>,
}

enum LayerRead {
    Reading(Task<Result<Image, String>>),
    Ready(Image),
    Failed(String),
}

impl LayerTexels {
    /// Start reading every loaded image in `set` that holds no pixels.
    pub fn request(
        &mut self,
        set: &TextureSetImages,
        images: &Assets<Image>,
        server: &AssetServer,
    ) {
        for (handle, srgb) in layer_handles(set) {
            if self.reads.contains_key(&handle.id())
                || images.get(handle).is_none_or(|image| image.data.is_some())
            {
                continue;
            }
            let Some(path) = server.get_path(handle.id()) else {
                continue;
            };
            let task = AsyncComputeTaskPool::get().spawn(read_layer(
                server.clone(),
                path.into_owned(),
                srgb,
            ));
            self.reads.insert(handle.id(), LayerRead::Reading(task));
        }
    }

    /// Drop every read that none of `sets` names.
    pub fn keep_only<'a>(&mut self, sets: impl IntoIterator<Item = &'a TextureSetImages>) {
        let wanted: HashSet<AssetId<Image>> = sets
            .into_iter()
            .flat_map(TextureSetImages::handles)
            .map(Handle::id)
            .collect();
        self.reads.retain(|id, _| wanted.contains(id));
    }

    /// Why the first unreadable file in `set` could not be read.
    pub fn failed(&self, set: &TextureSetImages) -> Option<&str> {
        set.handles()
            .find_map(|handle| match self.reads.get(&handle.id()) {
                Some(LayerRead::Failed(reason)) => Some(reason.as_str()),
                _ => None,
            })
    }

    fn get(&self, id: AssetId<Image>) -> Option<&Image> {
        match self.reads.get(&id) {
            Some(LayerRead::Ready(image)) => Some(image),
            _ => None,
        }
    }

    fn poll(&mut self) {
        for read in self.reads.values_mut() {
            let LayerRead::Reading(task) = read else {
                continue;
            };
            let Some(result) = block_on(poll_once(task)) else {
                continue;
            };
            *read = match result {
                Ok(image) => LayerRead::Ready(image),
                Err(reason) => LayerRead::Failed(reason),
            };
        }
    }
}

/// Where the array builder finds a layer's pixels.
pub trait LayerPixels {
    /// The image holding the pixels for `handle`, once they are in memory.
    fn layer(&self, handle: &Handle<Image>) -> Option<&Image>;
}

impl LayerPixels for Assets<Image> {
    fn layer(&self, handle: &Handle<Image>) -> Option<&Image> {
        self.get(handle)
    }
}

/// Loaded images that still hold their pixels, and file reads for the rest.
pub struct LayerImages<'a> {
    /// The loaded images.
    pub images: &'a Assets<Image>,
    /// Pixels read from the files of the images that hold none.
    pub texels: &'a LayerTexels,
}

impl LayerPixels for LayerImages<'_> {
    fn layer(&self, handle: &Handle<Image>) -> Option<&Image> {
        match self.images.get(handle) {
            Some(image) if image.data.is_some() => Some(image),
            _ => self.texels.get(handle.id()),
        }
    }
}

pub(crate) fn follow_layer_reads(
    mut texels: ResMut<LayerTexels>,
    mut image_events: MessageReader<AssetEvent<Image>>,
) {
    for event in image_events.read() {
        if let AssetEvent::Modified { id }
        | AssetEvent::Removed { id }
        | AssetEvent::Unused { id } = event
        {
            texels.reads.remove(id);
        }
    }
    texels.poll();
}

fn layer_handles(set: &TextureSetImages) -> impl Iterator<Item = (&Handle<Image>, bool)> {
    let colour = set.albedo.iter().flatten().map(|handle| (handle, true));
    let linear = set
        .normal
        .iter()
        .chain(&set.height)
        .chain(&set.occlusion)
        .chain(&set.roughness)
        .flatten()
        .map(|handle| (handle, false));
    colour.chain(linear)
}

async fn read_layer(
    server: AssetServer,
    path: AssetPath<'static>,
    srgb: bool,
) -> Result<Image, String> {
    if path.label().is_some() {
        return Err(format!(
            "{path}: a texture inside another file cannot be read as a layer"
        ));
    }
    let source = server
        .get_source(path.source().clone_owned())
        .map_err(|err| err.to_string())?;
    let reader = match server.mode() {
        AssetServerMode::Processed => source.processed_reader().map_err(|err| err.to_string())?,
        AssetServerMode::Unprocessed => source.reader(),
    };
    let mut reader = reader
        .read(path.path())
        .await
        .map_err(|err| format!("{path}: {err}"))?;
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .await
        .map_err(|err| format!("{path}: {err}"))?;
    decode_layer(&bytes, &path, srgb)
}

/// Decode a layer file to uncompressed texels, keeping only its top mip.
fn decode_layer(bytes: &[u8], path: &AssetPath, srgb: bool) -> Result<Image, String> {
    let extension = path
        .path()
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let basis = bytes
        .starts_with(&BASIS_SIGNATURE)
        .then(|| ImageFormat::from_extension("basis"))
        .flatten();
    let image_type = match basis {
        Some(basis) => ImageType::Format(basis),
        None => ImageType::Extension(&extension),
    };
    let mut image = Image::from_buffer(
        bytes,
        image_type,
        CompressedImageFormats::NONE,
        srgb,
        ImageSampler::Default,
        RenderAssetUsages::MAIN_WORLD,
    )
    .map_err(|err| format!("{path}: {err}"))?;
    keep_top_mip(&mut image);
    Ok(image)
}

/// Trim a single-layer, uncompressed image to its top mip.
fn keep_top_mip(image: &mut Image) {
    let descriptor = &mut image.texture_descriptor;
    if descriptor.mip_level_count <= 1
        || descriptor.size.depth_or_array_layers != 1
        || descriptor.format.block_dimensions() != (1, 1)
    {
        return;
    }
    let Some(texel_bytes) = descriptor.format.block_copy_size(None) else {
        return;
    };
    let top =
        descriptor.size.width as usize * descriptor.size.height as usize * texel_bytes as usize;
    descriptor.mip_level_count = 1;
    if let Some(data) = image.data.as_mut() {
        data.truncate(top);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

    const TWO_BY_TWO_PNG: [u8; 74] = [
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 2, 8, 6,
        0, 0, 0, 114, 182, 13, 36, 0, 0, 0, 17, 73, 68, 65, 84, 120, 156, 99, 224, 18, 145, 251,
        15, 194, 12, 48, 6, 0, 38, 140, 4, 237, 162, 200, 71, 131, 0, 0, 0, 0, 73, 69, 78, 68, 174,
        66, 96, 130,
    ];

    fn texel_image(data: Option<Vec<u8>>, usage: RenderAssetUsages) -> Image {
        let mut image = Image::new(
            Extent3d {
                width: 2,
                height: 2,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            vec![1; 16],
            TextureFormat::Rgba8UnormSrgb,
            usage,
        );
        image.data = data;
        image
    }

    #[test]
    fn a_png_layer_decodes_to_texels() {
        let decoded =
            decode_layer(&TWO_BY_TWO_PNG, &AssetPath::from("ground/grass.png"), true).unwrap();
        assert_eq!(
            decoded.texture_descriptor.format,
            TextureFormat::Rgba8UnormSrgb
        );
        assert_eq!(&decoded.data.unwrap()[..4], &[10, 20, 30, 255]);
    }

    #[test]
    fn a_layer_with_mips_keeps_only_its_top_level() {
        let mut image = texel_image(Some(vec![7; 16 + 4]), RenderAssetUsages::MAIN_WORLD);
        image.texture_descriptor.mip_level_count = 2;
        keep_top_mip(&mut image);
        assert_eq!(image.texture_descriptor.mip_level_count, 1);
        assert_eq!(image.data.as_ref().map(Vec::len), Some(16));
    }

    #[test]
    fn a_drawn_image_without_pixels_is_answered_from_its_file_read() {
        let mut images = Assets::<Image>::default();
        let drawn = images.add(texel_image(None, RenderAssetUsages::RENDER_WORLD));
        let kept = images.add(texel_image(Some(vec![3; 16]), RenderAssetUsages::default()));
        let mut texels = LayerTexels::default();

        let pixels = LayerImages {
            images: &images,
            texels: &texels,
        };
        assert!(pixels.layer(&drawn).is_none());
        assert!(pixels.layer(&kept).is_some());

        texels.reads.insert(
            drawn.id(),
            LayerRead::Ready(texel_image(
                Some(vec![9; 16]),
                RenderAssetUsages::MAIN_WORLD,
            )),
        );
        let pixels = LayerImages {
            images: &images,
            texels: &texels,
        };
        assert_eq!(
            pixels.layer(&drawn).and_then(|image| image.data.as_deref()),
            Some(&[9; 16][..])
        );

        let set = TextureSetImages {
            albedo: vec![Some(drawn.clone())],
            ..default()
        };
        texels.keep_only([&set]);
        assert_eq!(texels.reads.len(), 1, "a read a live set names is kept");
        texels.keep_only([]);
        assert!(texels.reads.is_empty(), "and dropped once no set names it");
    }

    #[test]
    fn a_layer_is_read_from_its_file_once_the_loaded_image_has_let_its_pixels_go() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("grass.png"), TWO_BY_TWO_PNG).unwrap();
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            AssetPlugin {
                file_path: dir.path().to_string_lossy().into_owned(),
                ..default()
            },
            bevy::image::ImagePlugin::default(),
        ));
        app.register_asset_loader(bevy::image::ImageLoader::new(CompressedImageFormats::NONE));
        app.init_resource::<LayerTexels>()
            .add_systems(PreUpdate, follow_layer_reads);
        let handle: Handle<Image> = app.world().resource::<AssetServer>().load("grass.png");
        for _ in 0..200 {
            app.update();
            if app.world().resource::<Assets<Image>>().contains(&handle) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if let Some(image) = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .get_mut_untracked(&handle)
        {
            image.data = None;
        }
        let set = TextureSetImages {
            albedo: vec![Some(handle.clone())],
            ..default()
        };

        let mut read = None;
        for _ in 0..200 {
            let world = app.world_mut();
            world.resource_scope(|world, mut texels: Mut<LayerTexels>| {
                texels.request(
                    &set,
                    world.resource::<Assets<Image>>(),
                    world.resource::<AssetServer>(),
                );
            });
            app.update();
            let world = app.world();
            let layers = LayerImages {
                images: world.resource::<Assets<Image>>(),
                texels: world.resource::<LayerTexels>(),
            };
            read = layers.layer(&handle).and_then(|image| image.data.clone());
            if read.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            read.as_deref().map(|data| &data[..4]),
            Some(&[10, 20, 30, 255][..])
        );
    }
}
