use std::path::{Path, PathBuf};

use bevy::asset::meta::{AssetAction, AssetMetaDyn};
use bevy::asset::{AssetPath, RenderAssetUsages, io::AssetSourceId};
use bevy::image::{ImageArrayLayout, ImageLoader, ImageLoaderSettings, ImageSampler};
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::sprite_render::SpriteMeshMaterial;
use jackdaw_api::prelude::*;
pub use jackdaw_runtime::Sampling;
use jackdaw_runtime::texture_import::{TextureImportMeta, TextureImportProcessor};
use jackdaw_runtime::{SAMPLING_KEY, game_settings_file};

use crate::inspector::PendingInspectorRebuild;
use crate::inspector::sprite_card::SpriteSamplingCombo;
use crate::project::ProjectRoot;
use crate::project_settings::{Section, store_section_at};
use crate::texture_import::meta_path;

/// The sampler label of an image's own sampling, written by
/// `image.set_sampling`. Any sampler an image's meta holds is its own choice;
/// the label only says which tool wrote it.
pub const OVERRIDE_LABEL: &str = "sampling override";

/// The open project's 2D sampling. The game reads it at startup from
/// `assets/jackdaw_game.json` through `jackdaw_runtime::image_plugin`; the
/// editor applies it to the images registered in [`Images2d`].
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProjectSampling2d(pub Sampling);

/// The images drawn in 2D. The editor gives each the project's 2D sampling in
/// memory, unless the image's meta holds its own; the editor's default sampler,
/// which 3D textures and UI use, stays as it is. Sprites register their images;
/// other 2D content registers its own through [`Images2d::register`].
#[derive(Resource, Default)]
pub struct Images2d {
    images: HashMap<AssetId<Image>, Image2d>,
    dirty: HashSet<AssetId<Image>>,
    /// Images whose sampler this pass changed, so the `Modified` that change
    /// raises is not taken for a reload.
    changed_here: HashSet<AssetId<Image>>,
    /// Images reloaded for a sampler their GPU copy could not take in place,
    /// with that sampler.
    reloading: HashMap<AssetId<Image>, ImageSampler>,
    warned: bool,
}

struct Image2d {
    path: Option<AssetPath<'static>>,
    /// The sampler the image's meta gives it, `None` until read.
    own: Option<Option<ImageSampler>>,
}

impl Images2d {
    /// Take `image` as a 2D image. A new one is given its sampling on the next
    /// pass, and again whenever it loads or reloads.
    pub fn register(&mut self, image: &Handle<Image>) {
        if !self.images.contains_key(&image.id()) {
            let path = image.path().map(AssetPath::clone_owned);
            self.images.insert(image.id(), Image2d { path, own: None });
            self.dirty.insert(image.id());
        }
    }

    /// Read `image`'s meta again and give it its sampling.
    fn reread(&mut self, image: AssetId<Image>) {
        if let Some(entry) = self.images.get_mut(&image) {
            entry.own = None;
            self.dirty.insert(image);
        }
    }

    fn refresh_all(&mut self) {
        self.dirty.extend(self.images.keys().copied());
    }

    fn has_work(&self) -> bool {
        !self.dirty.is_empty() || !self.reloading.is_empty()
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<ProjectSampling2d>()
        .init_resource::<Images2d>()
        .add_systems(
            Update,
            (
                load_project_sampling,
                register_sprite_images,
                follow_image_loads,
                apply_2d_sampling.run_if(|images: Res<Images2d>| images.has_work()),
            )
                .chain(),
        );
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ProjectSet2dSamplingOp>()
        .register_operator::<ImageSetSamplingOp>();
}

/// Read the open project's 2D sampling, once per project opened.
fn load_project_sampling(
    project: Option<Res<ProjectRoot>>,
    mut sampling: ResMut<ProjectSampling2d>,
    mut images: ResMut<Images2d>,
    mut loaded_root: Local<Option<PathBuf>>,
) {
    let Some(project) = project else {
        return;
    };
    if loaded_root.as_ref() == Some(&project.root) {
        return;
    }
    *loaded_root = Some(project.root.clone());
    sampling.0 = Sampling::read(&game_settings_file(&project.root));
    images.refresh_all();
}

/// Register the image of every sprite that is new or changed, by whatever
/// route: a spawn, the inspector, an undo or a remote edit. An image already
/// registered is left as it is.
fn register_sprite_images(
    sprites: Query<&Sprite, (Changed<Sprite>, Without<crate::EditorEntity>)>,
    mut images: ResMut<Images2d>,
) {
    for sprite in &sprites {
        images.register(&sprite.image);
    }
}

/// Give a registered image its sampling again whenever it loads or reloads,
/// whatever the reason: a first load, one after a failed load, a re-saved
/// file, or a meta written or removed. Forget one that nothing holds any more.
fn follow_image_loads(mut events: MessageReader<AssetEvent<Image>>, mut images: ResMut<Images2d>) {
    for event in events.read() {
        match event {
            AssetEvent::Modified { id } if images.changed_here.remove(id) => {}
            AssetEvent::LoadedWithDependencies { id } | AssetEvent::Modified { id } => {
                images.reread(*id);
            }
            AssetEvent::Removed { id } | AssetEvent::Unused { id } => {
                images.images.remove(id);
                images.reloading.remove(id);
            }
            AssetEvent::Added { .. } => {}
        }
    }
}

/// Give each refreshed 2D image its sampling: its meta's own, else the
/// project's. Sprite materials drawing a changed image are marked modified,
/// because a material keeps the sampler it was built with until it is itself
/// modified; other material types drawing it are not refreshed.
///
/// An image held only on the GPU, as an imported texture is, has no pixels
/// left to send again with a new sampler, so it is reloaded and given the
/// sampler once it is back.
fn apply_2d_sampling(
    mut registered: ResMut<Images2d>,
    sampling: Res<ProjectSampling2d>,
    project: Option<Res<ProjectRoot>>,
    asset_server: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
    mut materials: Option<ResMut<Assets<SpriteMeshMaterial>>>,
) {
    let registered = &mut *registered;
    let assets = project.map(|project| project.assets_dir());
    let project_sampler = match sampling.0 {
        Sampling::Linear => ImageSampler::Default,
        Sampling::Nearest => ImageSampler::Descriptor(Sampling::Nearest.descriptor()),
    };
    let mut wanted: Vec<(AssetId<Image>, ImageSampler)> = Vec::new();
    registered.reloading.retain(|id, sampler| {
        let back = images.get(*id).is_some_and(|image| image.data.is_some());
        if back {
            wanted.push((*id, sampler.clone()));
        }
        !back
    });
    for id in std::mem::take(&mut registered.dirty) {
        let Some(entry) = registered.images.get_mut(&id) else {
            continue;
        };
        let own = entry.own.get_or_insert_with(|| {
            entry
                .path
                .as_ref()
                .zip(assets.as_deref())
                .and_then(|(path, assets)| sampling_target(assets, path))
                .and_then(|target| target.own)
        });
        wanted.push((id, own.clone().unwrap_or_else(|| project_sampler.clone())));
    }
    let mut changed = HashSet::new();
    for (id, sampler) in wanted {
        let Some(image) = images.get(id) else {
            continue;
        };
        if image.sampler == sampler {
            continue;
        }
        let gpu_only =
            image.data.is_none() && !image.asset_usage.contains(RenderAssetUsages::MAIN_WORLD);
        if gpu_only {
            match registered
                .images
                .get(&id)
                .and_then(|entry| entry.path.clone())
            {
                Some(path) => {
                    asset_server.reload(path);
                    registered.reloading.insert(id, sampler);
                }
                None if !registered.warned => {
                    registered.warned = true;
                    warn!(
                        "a 2D image held only on the GPU, with no file to reload, keeps its sampler"
                    );
                }
                None => {}
            }
            continue;
        }
        if let Some(mut image) = images.get_mut(id) {
            image.sampler = sampler;
            changed.insert(id);
        }
    }
    if let Some(materials) = materials.as_deref_mut()
        && !changed.is_empty()
    {
        let drawing: Vec<_> = materials
            .iter()
            .filter(|(_, material)| changed.contains(&material.image.id()))
            .map(|(material, _)| material)
            .collect();
        for material in drawing {
            if let Some(material) = materials.get_mut(material) {
                material.into_inner();
            }
        }
    }
    registered.changed_here.extend(changed);
}

/// The meta, when `bytes` is an image meta: a plain load or a texture import.
fn image_meta(bytes: &[u8]) -> Option<TextureImportMeta> {
    let meta = TextureImportMeta::deserialize(bytes).ok()?;
    let ours = match &meta.asset {
        AssetAction::Load { loader, .. } => {
            loader == ImageLoader::type_path() || loader == ImageLoader::short_type_path()
        }
        AssetAction::Process { processor, .. } => {
            processor == TextureImportProcessor::type_path()
                || processor == TextureImportProcessor::short_type_path()
        }
        AssetAction::Ignore => false,
    };
    ours.then_some(meta)
}

fn loader_settings(meta: &mut TextureImportMeta) -> Option<&mut ImageLoaderSettings> {
    match &mut meta.asset {
        AssetAction::Load { settings, .. } => Some(settings),
        AssetAction::Process { settings, .. } => Some(&mut settings.loader_settings),
        AssetAction::Ignore => None,
    }
}

/// How the image meta `bytes` cuts its image into array layers, if it does.
pub fn array_layout(bytes: &[u8]) -> Option<ImageArrayLayout> {
    loader_settings(&mut image_meta(bytes)?)?.array_layout
}

/// The meta of an image loaded with the default settings, as if it had none.
/// A plain load, as the editor and the game template load assets unprocessed.
fn plain_meta() -> TextureImportMeta {
    TextureImportMeta::new(AssetAction::Load {
        loader: ImageLoader::type_path().to_string(),
        settings: ImageLoaderSettings::default(),
    })
}

/// The meta of an image loaded with the default settings, serialised.
pub fn plain_meta_bytes() -> Vec<u8> {
    AssetMetaDyn::serialize(&plain_meta())
}

/// What to do with an image's meta file.
#[derive(Debug, PartialEq, Eq)]
pub enum MetaWrite {
    /// It already says this.
    Keep,
    /// Write these bytes.
    Write(Vec<u8>),
    /// Delete it: nothing in it differs from having none.
    Remove,
}

/// The meta of an image whose meta reads `held` (`None` without one) with its
/// own sampling set to `own`, or cleared when `own` is `None`. Only the filters
/// and the label of a sampler move; every other setting is carried over.
/// `None` when `held` is not an image meta.
pub fn overridden_meta(held: Option<&[u8]>, own: Option<Sampling>) -> Option<MetaWrite> {
    let mut meta = match held {
        Some(bytes) => image_meta(bytes)?,
        None => plain_meta(),
    };
    let settings = loader_settings(&mut meta)?;
    let next = match own {
        Some(mode) => {
            let mut descriptor = match &settings.sampler {
                ImageSampler::Descriptor(descriptor) => descriptor.clone(),
                ImageSampler::Default => mode.descriptor(),
            };
            descriptor.set_filter(mode.filter());
            if mode == Sampling::Nearest {
                descriptor.anisotropy_clamp = 1;
            }
            descriptor.label = Some(OVERRIDE_LABEL.to_string());
            ImageSampler::Descriptor(descriptor)
        }
        None => ImageSampler::Default,
    };
    if next == settings.sampler {
        return Some(MetaWrite::Keep);
    }
    settings.sampler = next;
    let bytes = AssetMetaDyn::serialize(&meta);
    Some(if bytes == plain_meta_bytes() {
        MetaWrite::Remove
    } else {
        MetaWrite::Write(bytes)
    })
}

/// The meta of an image whose meta reads `held` (`None` without one) loading
/// it as an array of `tile`-sized layers, row by row, sampled nearest. Every
/// other setting is carried over. `None` when `held` is not an image meta.
pub fn tile_array_meta(held: Option<&[u8]>, tile: UVec2) -> Option<Vec<u8>> {
    let mut meta = match held {
        Some(bytes) => image_meta(bytes)?,
        None => plain_meta(),
    };
    loader_settings(&mut meta)?.array_layout = Some(ImageArrayLayout::GridSize {
        tile_width_pixels: tile.x,
        tile_height_pixels: tile.y,
    });
    let bytes = AssetMetaDyn::serialize(&meta);
    match overridden_meta(Some(&bytes), Some(Sampling::Nearest))? {
        MetaWrite::Write(sampled) => Some(sampled),
        MetaWrite::Keep | MetaWrite::Remove => Some(bytes),
    }
}

/// `fresh`, an image meta about to replace `held`, carrying `held`'s sampler
/// and array layout, so a rewrite of the meta for another reason keeps the
/// image's sampling and keeps a tile sheet an array texture.
pub fn keep_own_settings(fresh: Vec<u8>, held: Option<&[u8]>) -> Vec<u8> {
    let Some(mut held) = held.and_then(image_meta) else {
        return fresh;
    };
    let Some(own) = loader_settings(&mut held) else {
        return fresh;
    };
    let Some(mut meta) = image_meta(&fresh) else {
        return fresh;
    };
    let Some(settings) = loader_settings(&mut meta) else {
        return fresh;
    };
    settings.sampler = own.sampler.clone();
    settings.array_layout = own.array_layout;
    AssetMetaDyn::serialize(&meta)
}

/// An image whose sampling can be set.
pub struct SamplingTarget {
    /// The image file.
    pub file: PathBuf,
    /// The sampler its meta gives it, when it gives one.
    pub own: Option<ImageSampler>,
}

impl SamplingTarget {
    /// The sampling the image's meta gives it, when it gives one.
    pub fn own_sampling(&self) -> Option<Sampling> {
        let Some(ImageSampler::Descriptor(descriptor)) = &self.own else {
            return None;
        };
        Some(if descriptor.mag_filter == Sampling::Nearest.filter() {
            Sampling::Nearest
        } else {
            Sampling::Linear
        })
    }
}

/// The image at `path`, when its sampling can be set: a whole file (no label)
/// on the default source, inside `assets` by `crate::project::path_within`,
/// whose meta is absent or an image meta. Reads the meta once.
pub fn sampling_target(assets: &Path, path: &AssetPath) -> Option<SamplingTarget> {
    if path.label().is_some() || !matches!(path.source(), AssetSourceId::Default) {
        return None;
    }
    let file = crate::project::path_within(assets, path.path())
        .ok()
        .filter(|file| file.is_file())?;
    let own = match std::fs::read(meta_path(&file)) {
        Ok(bytes) => {
            let mut meta = image_meta(&bytes)?;
            Some(loader_settings(&mut meta)?.sampler.clone())
                .filter(|sampler| matches!(sampler, ImageSampler::Descriptor(_)))
        }
        Err(_) => None,
    };
    Some(SamplingTarget { file, own })
}

/// Ask the sampling card, if one is showing, to read the image's sampling again.
fn refresh_card(cards: &Query<&SpriteSamplingCombo>, rebuild: &mut PendingInspectorRebuild) {
    if let Some(card) = cards.iter().next() {
        rebuild.0 = Some(card.source);
    }
}

/// Set how the project's 2D sprites sample their images. The editor applies it
/// at once, to 2D images only. The game reads it at startup from the project's
/// `assets/jackdaw_game.json` when its `main.rs` sets
/// `DefaultPlugins.set(jackdaw_runtime::image_plugin())`, and applies it to
/// every image, 3D textures included; a game that sets its own `ImagePlugin`
/// keeps its own sampler. A project setting, so no history entry.
#[operator(
    id = "project.set_2d_sampling",
    label = "Set 2D Sampling",
    description = "Draw the project's 2D sprites smooth (linear) or crisp (nearest, for pixel art). \
                   The game applies it when its main.rs sets \
                   `DefaultPlugins.set(jackdaw_runtime::image_plugin())`.",
    allows_undo = false,
    params(mode(
        String,
        doc = "`linear` (smooth, the default) or `nearest` (crisp pixel art)."
    ))
)]
pub(crate) fn project_set_2d_sampling(
    params: In<OperatorParameters>,
    mut sampling: ResMut<ProjectSampling2d>,
    mut images: ResMut<Images2d>,
    project: Option<Res<ProjectRoot>>,
    cards: Query<&SpriteSamplingCombo>,
    mut rebuild: ResMut<PendingInspectorRebuild>,
) -> OperatorResult {
    let Some(mode) = params.as_str("mode").and_then(Sampling::parse) else {
        warn!("project.set_2d_sampling: 'mode' must be `linear` or `nearest`");
        return OperatorResult::Cancelled;
    };
    if let Some(project) = project {
        let file = game_settings_file(&project.root);
        store_section_at(&file, Section::Key(SAMPLING_KEY), &mode);
    }
    sampling.0 = mode;
    images.refresh_all();
    refresh_card(&cards, &mut rebuild);
    OperatorResult::Finished
}

/// Give one image its own sampling in its `.meta`, or have it follow the
/// project again. The meta is a project file, so no history entry.
#[operator(
    id = "image.set_sampling",
    label = "Set Image Sampling",
    description = "Override the project's 2D sampling for one image, or follow it again.",
    allows_undo = false,
    params(
        path(String, doc = "The image, as a path under the project's assets."),
        mode(
            String,
            doc = "`linear`, `nearest`, or `project` to follow the project's 2D sampling: \
                   this clears the override, and removes the meta when nothing else is in it."
        )
    )
)]
pub(crate) fn image_set_sampling(
    params: In<OperatorParameters>,
    mut images: ResMut<Images2d>,
    project: Option<Res<ProjectRoot>>,
    asset_server: Res<AssetServer>,
    cards: Query<&SpriteSamplingCombo>,
    mut rebuild: ResMut<PendingInspectorRebuild>,
) -> OperatorResult {
    let Some(project) = project else {
        warn!("image.set_sampling: no project is open");
        return OperatorResult::Cancelled;
    };
    let own = match params.as_str("mode") {
        Some("project") => None,
        Some(mode) if Sampling::parse(mode).is_some() => Sampling::parse(mode),
        _ => {
            warn!("image.set_sampling: 'mode' must be `linear`, `nearest` or `project`");
            return OperatorResult::Cancelled;
        }
    };
    let path = params
        .as_str("path")
        .and_then(|path| AssetPath::try_parse(path).ok())
        .map(AssetPath::into_owned);
    let target = path
        .as_ref()
        .and_then(|path| sampling_target(&project.assets_dir(), path));
    let (Some(path), Some(target)) = (path, target) else {
        warn!(
            "image.set_sampling: 'path' must name an image file under the project's assets \
             whose meta, if any, is an image meta"
        );
        return OperatorResult::Cancelled;
    };
    let meta = meta_path(&target.file);
    let held = std::fs::read(&meta).ok();
    let written = match overridden_meta(held.as_deref(), own) {
        Some(MetaWrite::Write(bytes)) => std::fs::write(&meta, bytes),
        Some(MetaWrite::Remove) => std::fs::remove_file(&meta),
        Some(MetaWrite::Keep) | None => Ok(()),
    };
    if let Err(error) = written {
        warn!("image.set_sampling: {error}");
        return OperatorResult::Cancelled;
    }
    if let Some(handle) = asset_server.get_handle::<Image>(path) {
        images.reread(handle.id());
    }
    refresh_card(&cards, &mut rebuild);
    OperatorResult::Finished
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSamplerDescriptor};
    use jackdaw_runtime::texture_import::{read_texture_import_meta, texture_import_meta};

    fn written(held: Option<&[u8]>, own: Option<Sampling>) -> Vec<u8> {
        match overridden_meta(held, own) {
            Some(MetaWrite::Write(bytes)) => bytes,
            other => panic!("expected a write, got {other:?}"),
        }
    }

    fn sampler(bytes: &[u8]) -> ImageSampler {
        let mut meta = image_meta(bytes).expect("an image meta");
        loader_settings(&mut meta)
            .expect("loader settings")
            .sampler
            .clone()
    }

    #[test]
    fn an_override_is_labelled_and_project_removes_a_sampler_only_meta() {
        let own = written(None, Some(Sampling::Nearest));
        let ImageSampler::Descriptor(descriptor) = sampler(&own) else {
            panic!("a descriptor");
        };
        assert_eq!(descriptor.mag_filter, ImageFilterMode::Nearest);
        assert_eq!(descriptor.label.as_deref(), Some(OVERRIDE_LABEL));
        assert_eq!(
            overridden_meta(Some(&own), Some(Sampling::Nearest)),
            Some(MetaWrite::Keep)
        );
        assert_eq!(overridden_meta(Some(&own), None), Some(MetaWrite::Remove));
        assert_eq!(overridden_meta(None, None), Some(MetaWrite::Keep));
    }

    #[test]
    fn project_keeps_an_import_and_its_settings() {
        let own = written(None, Some(Sampling::Nearest));
        let import = keep_own_settings(texture_import_meta(false), Some(&own));
        assert!(matches!(sampler(&import), ImageSampler::Descriptor(_)));
        let cleared = written(Some(&import), None);
        let loader = read_texture_import_meta(&cleared).expect("still an import meta");
        assert!(!loader.is_srgb, "the import's colour space is kept");
        assert_eq!(loader.sampler, ImageSampler::Default);
    }

    #[test]
    fn an_override_keeps_the_rest_of_a_hand_written_sampler() {
        let mut meta = plain_meta();
        loader_settings(&mut meta).expect("loader settings").sampler =
            ImageSampler::Descriptor(ImageSamplerDescriptor {
                address_mode_u: ImageAddressMode::MirrorRepeat,
                anisotropy_clamp: 8,
                ..ImageSamplerDescriptor::linear()
            });
        let held = AssetMetaDyn::serialize(&meta);
        let ImageSampler::Descriptor(descriptor) =
            sampler(&written(Some(&held), Some(Sampling::Nearest)))
        else {
            panic!("a descriptor");
        };
        assert_eq!(descriptor.address_mode_u, ImageAddressMode::MirrorRepeat);
        assert_eq!(
            descriptor.anisotropy_clamp, 1,
            "nearest cannot be anisotropic"
        );
    }

    #[test]
    fn a_meta_for_another_loader_is_not_touched() {
        let other = br#"(
    meta_format_version: "1.0",
    asset: Load(
        loader: "some::OtherLoader",
        settings: (),
    ),
)"#;
        assert_eq!(overridden_meta(Some(other), Some(Sampling::Nearest)), None);
    }
}
