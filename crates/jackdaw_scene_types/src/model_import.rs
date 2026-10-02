//! A model's import settings, kept in the `.meta` Bevy reads beside it, as
//! Unity keeps a model importer's settings in its `.meta` and Godot in its
//! `.import`.
//!
//! A model whose meta names [`ModelLoader`] loads exactly as through Bevy's
//! `GltfLoader`, and its settings may also hold the model's levels of detail.
//! A placed model with levels draws them live without a [`LodGroup`]: its
//! entity gets a derived [`ModelLevels`] from the settings and the
//! placement's [`LodOverride`].

use std::sync::Arc;

use bevy::asset::meta::{AssetAction, AssetMeta, AssetMetaDyn};
use bevy::asset::{AssetLoader, AssetPath, LoadContext, io::Reader};
use bevy::gltf::extensions::GltfExtensionHandlers;
use bevy::gltf::{DefaultGltfImageSampler, Gltf, GltfError, GltfLoader, GltfLoaderSettings};
use bevy::image::{CompressedImageFormatSupport, CompressedImageFormats};
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::tasks::{IoTaskPool, Task, block_on, futures_lite::future};
use serde::{Deserialize, Serialize};

use crate::model_parts::{nodes_key, source_path};
use crate::{GltfSource, LodFade, LodGroup, LodLevel, LodOverride};

/// The levels of detail a model imports with.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ModelLod {
    /// The layout these settings were written in.
    #[serde(default = "first_version")]
    pub version: u32,
    /// How the levels were found, so a reimport looks the same way again.
    #[serde(default)]
    pub source: LodImportSource,
    /// The model's size along its largest side, in its own units, measured
    /// from its first level; 0 measures it once that level has loaded.
    #[serde(default)]
    pub size: f32,
    /// How each level hands over to the next.
    #[serde(default)]
    pub fade: LodFade,
    /// One entry per level, from the most detailed down. The last level's
    /// screen height is where the model stops drawing.
    pub levels: Vec<ModelLodLevel>,
}

fn first_version() -> u32 {
    ModelLod::VERSION
}

impl ModelLod {
    /// The layout this build writes.
    pub const VERSION: u32 = 1;
}

/// Where an import found a model's levels.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LodImportSource {
    /// Listed by hand, or carried over from a scene's LOD groups.
    #[default]
    Authored,
    /// Files named `<model>_LOD1`, `<model>_LOD2`, ... beside the model.
    SiblingFiles,
    /// Nodes inside the model named with a `_LOD0`, `_LOD1`, ... suffix.
    NodeSuffixes,
}

/// One level of a model.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ModelLodLevel {
    /// What the level draws.
    pub show: LevelShow,
    /// The share of the screen's height the model has to cover for this level
    /// or a more detailed one to show.
    pub screen_height: f32,
}

/// What a level of a model draws.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum LevelShow {
    /// The model itself.
    Model,
    /// The first scene of another model file, relative to this one's folder.
    File(String),
    /// Only these nodes of the model, each with everything under it.
    Nodes(Vec<String>),
}

/// The settings of a model whose meta names [`ModelLoader`].
#[derive(Serialize, Deserialize, Default)]
pub struct ModelSettings {
    /// What Bevy's glTF loader reads from the file.
    #[serde(default)]
    pub gltf: GltfLoaderSettings,
    /// The model's levels of detail, when it has any.
    #[serde(default)]
    pub lod: Option<ModelLod>,
}

impl ModelSettings {
    /// Settings that load the model as every drawn model loads, with `lod`.
    pub fn drawn(lod: Option<ModelLod>) -> Self {
        let mut gltf = GltfLoaderSettings::default();
        crate::render_assets::model_settings(&mut gltf);
        Self { gltf, lod }
    }
}

/// Loads a model through Bevy's [`GltfLoader`] with the settings its meta
/// holds. Only a meta can choose it: it claims no file extension, so a model
/// without one loads through `GltfLoader` as in any Bevy app.
#[derive(TypePath)]
pub struct ModelLoader {
    /// The loader every model's file goes through.
    pub gltf: GltfLoader,
}

impl AssetLoader for ModelLoader {
    type Asset = Gltf;
    type Settings = ModelSettings;
    type Error = GltfError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        settings: &ModelSettings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Gltf, GltfError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        GltfLoader::load_gltf(&self.gltf, &bytes, load_context, &settings.gltf).await
    }

    fn extensions(&self) -> &[&str] {
        &[]
    }
}

/// The `.meta` of a model with import settings.
pub type ModelMeta = AssetMeta<ModelLoader, ()>;

/// The meta bytes that have a model load through [`ModelLoader`] with
/// `settings`.
pub fn model_meta(settings: ModelSettings) -> Vec<u8> {
    let meta = ModelMeta::new(AssetAction::Load {
        loader: ModelLoader::type_path().to_string(),
        settings,
    });
    AssetMetaDyn::serialize(&meta)
}

/// The settings a model's meta holds, when the meta names [`ModelLoader`].
pub fn read_model_meta(bytes: &[u8]) -> Option<ModelSettings> {
    let meta = ModelMeta::deserialize(bytes).ok()?;
    let AssetAction::Load { loader, settings } = meta.asset else {
        return None;
    };
    (loader == ModelLoader::type_path()).then_some(settings)
}

/// The `.meta` file beside a model.
pub fn meta_path(model: &std::path::Path) -> std::path::PathBuf {
    let mut name = model.as_os_str().to_owned();
    name.push(".meta");
    std::path::PathBuf::from(name)
}

/// The model level `show` draws for a model at `model`, as [`ModelParts`]
/// keys it.
///
/// [`ModelParts`]: crate::model_parts::ModelParts
pub fn level_key(model: &str, show: &LevelShow) -> String {
    match show {
        LevelShow::Model => model.to_string(),
        LevelShow::File(file) => sibling_path(model, file),
        LevelShow::Nodes(nodes) => nodes_key(model, nodes),
    }
}

/// `file`, relative to the folder of the model at `model`, as an asset path.
pub fn sibling_path(model: &str, file: &str) -> String {
    AssetPath::parse(model)
        .resolve_embed_str(file)
        .map_or_else(|_| file.to_string(), |path| path.to_string())
}

/// The levels of detail a placed model draws: its model's settings with the
/// placement's [`LodOverride`] laid over them. Derived, never saved.
#[derive(Component, Clone, Debug, PartialEq)]
pub struct ModelLevels {
    /// The screen heights, size and fade the levels switch by.
    pub group: LodGroup,
    /// What each level draws, as [`ModelParts`] keys it.
    ///
    /// [`ModelParts`]: crate::model_parts::ModelParts
    pub models: Vec<String>,
}

impl ModelLevels {
    /// The levels the model at `model` draws with `lod`, overridden by
    /// `placement`.
    pub fn new(model: &str, lod: &ModelLod, placement: Option<&LodOverride>) -> Self {
        let heights = placement.and_then(|placement| placement.screen_heights.as_ref());
        let levels = lod
            .levels
            .iter()
            .enumerate()
            .map(|(index, level)| LodLevel {
                screen_height: heights
                    .and_then(|heights| heights.get(index).copied())
                    .unwrap_or(level.screen_height),
            })
            .collect();
        let fade = placement
            .and_then(|placement| placement.fade)
            .unwrap_or(lod.fade);
        Self {
            group: LodGroup {
                levels,
                size: lod.size.max(0.0),
                fade: fade.width(),
            },
            models: lod
                .levels
                .iter()
                .map(|level| level_key(model, &level.show))
                .collect(),
        }
    }
}

#[derive(Clone, Debug)]
enum Entry {
    /// Asked for, with the read that will answer it.
    Reading(u32),
    /// Read: whether the meta names [`ModelLoader`], and the model's levels,
    /// or `None` when it has none.
    Known {
        imported: bool,
        lod: Option<Arc<ModelLod>>,
    },
}

/// The levels of detail of every placed model, by asset path, read from each
/// model's `.meta` the first time a placement names it.
#[derive(Resource, Default)]
pub struct ModelLodIndex {
    entries: HashMap<String, Entry>,
    unread: Vec<String>,
    reads: Vec<Task<(String, u32, Option<ModelSettings>)>>,
    next_read: u32,
    placed: HashMap<String, HashSet<Entity>>,
    stale: HashSet<Entity>,
}

impl ModelLodIndex {
    /// Read the meta of the model at `path` unless it is already known.
    pub fn request(&mut self, path: &str) {
        if !self.entries.contains_key(path) {
            self.read(path);
        }
    }

    /// Read the meta of the model at `path` again, for a meta that changed on
    /// disk.
    pub fn reread(&mut self, path: &str) {
        self.read(path);
    }

    fn read(&mut self, path: &str) {
        self.next_read = self.next_read.wrapping_add(1);
        self.entries
            .insert(path.to_string(), Entry::Reading(self.next_read));
        self.unread.push(path.to_string());
    }

    /// Whether the meta of the model at `path` has been read.
    pub fn is_known(&self, path: &str) -> bool {
        matches!(self.entries.get(path), Some(Entry::Known { .. }))
    }

    /// Whether the meta of the model at `path` names [`ModelLoader`].
    pub fn is_imported(&self, path: &str) -> bool {
        matches!(
            self.entries.get(path),
            Some(Entry::Known { imported: true, .. })
        )
    }

    /// The levels of the model at `path`, once read, when it has any.
    pub fn get(&self, path: &str) -> Option<&Arc<ModelLod>> {
        match self.entries.get(path) {
            Some(Entry::Known { lod, .. }) => lod.as_ref(),
            _ => None,
        }
    }

    /// Give the model at `path` these levels, or none, in place of what its
    /// meta says, until it is read again. Its placements follow.
    pub fn set(&mut self, path: &str, lod: Option<ModelLod>) {
        let imported = self.is_imported(path);
        self.entries.insert(
            path.to_string(),
            Entry::Known {
                imported,
                lod: lod.map(Arc::new),
            },
        );
        self.restale(path);
    }

    /// Whether any meta is still being read.
    pub fn is_reading(&self) -> bool {
        !self.unread.is_empty() || !self.reads.is_empty()
    }

    fn restale(&mut self, path: &str) {
        if let Some(placed) = self.placed.get(path) {
            self.stale.extend(placed.iter().copied());
        }
    }
}

/// Reads model import settings and keeps every placed model's
/// [`ModelLevels`] in step with them.
pub struct ModelImportPlugin;

impl Plugin for ModelImportPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ModelLodIndex>()
            .add_observer(index_placed_model)
            .add_observer(unindex_placed_model)
            .add_systems(PreUpdate, (read_model_lods, resolve_model_levels).chain());
    }

    fn finish(&self, app: &mut App) {
        if let Some(gltf) = gltf_loader(app) {
            app.register_asset_loader(ModelLoader { gltf })
                .add_systems(PreUpdate, reread_reloaded_models.before(read_model_lods));
        }
    }
}

/// Add [`ModelImportPlugin`] unless something else already has.
pub fn add_model_import(app: &mut App) {
    if !app.is_plugin_added::<ModelImportPlugin>() {
        app.add_plugins(ModelImportPlugin);
    }
}

/// A glTF loader built as Bevy's `GltfPlugin` builds its own, or `None` in an
/// app without one.
fn gltf_loader(app: &App) -> Option<GltfLoader> {
    let plugin = app
        .get_added_plugins::<bevy::gltf::GltfPlugin>()
        .into_iter()
        .next()?;
    let world = app.world();
    Some(GltfLoader {
        supported_compressed_formats: world
            .get_resource::<CompressedImageFormatSupport>()
            .map_or(CompressedImageFormats::NONE, |support| support.0),
        custom_vertex_attributes: plugin.custom_vertex_attributes.clone(),
        default_sampler: world.get_resource::<DefaultGltfImageSampler>().map_or_else(
            || DefaultGltfImageSampler::new(&plugin.default_sampler).get_internal(),
            DefaultGltfImageSampler::get_internal,
        ),
        default_convert_coordinates: plugin.convert_coordinates,
        extensions: world.get_resource::<GltfExtensionHandlers>()?.0.clone(),
        default_skinned_mesh_bounds_policy: plugin.skinned_mesh_bounds_policy,
    })
}

fn index_placed_model(
    insert: On<Insert, GltfSource>,
    sources: Query<&GltfSource>,
    mut index: ResMut<ModelLodIndex>,
) {
    let Ok(source) = sources.get(insert.entity) else {
        return;
    };
    let path = source_path(source);
    index.request(&path);
    index.placed.entry(path).or_default().insert(insert.entity);
    index.stale.insert(insert.entity);
}

fn unindex_placed_model(
    replace: On<Discard, GltfSource>,
    sources: Query<&GltfSource>,
    mut index: ResMut<ModelLodIndex>,
) {
    let Ok(source) = sources.get(replace.entity) else {
        return;
    };
    let path = source_path(source);
    if let Some(placed) = index.placed.get_mut(&path) {
        placed.remove(&replace.entity);
        if placed.is_empty() {
            index.placed.remove(&path);
        }
    }
    index.stale.insert(replace.entity);
}

/// Read a model's settings again when Bevy reloads the model, as it does when
/// the meta beside it changes.
fn reread_reloaded_models(
    mut events: MessageReader<AssetEvent<Gltf>>,
    server: Res<AssetServer>,
    mut index: ResMut<ModelLodIndex>,
) {
    for event in events.read() {
        if let AssetEvent::Modified { id } = event
            && let Some(path) = server.get_path(*id)
        {
            let path = path.without_label().to_string();
            if index.entries.contains_key(&path) {
                index.reread(&path);
            }
        }
    }
}

fn read_model_lods(server: Option<Res<AssetServer>>, mut index: ResMut<ModelLodIndex>) {
    if !index.is_reading() {
        return;
    }
    let index = index.as_mut();
    for path in std::mem::take(&mut index.unread) {
        let Some(Entry::Reading(read)) = index.entries.get(&path).cloned() else {
            continue;
        };
        let Some(server) = server.as_deref().cloned() else {
            index.entries.insert(
                path.clone(),
                Entry::Known {
                    imported: false,
                    lod: None,
                },
            );
            index.restale(&path);
            continue;
        };
        index.reads.push(IoTaskPool::get().spawn(async move {
            let settings = read_settings(&server, &path).await;
            (path, read, settings)
        }));
    }
    let mut done = Vec::new();
    index
        .reads
        .retain_mut(|task| match block_on(future::poll_once(task)) {
            Some(result) => {
                done.push(result);
                false
            }
            None => true,
        });
    for (path, read, settings) in done {
        if matches!(index.entries.get(&path), Some(Entry::Reading(current)) if *current == read) {
            let entry = Entry::Known {
                imported: settings.is_some(),
                lod: settings.and_then(|settings| settings.lod).map(Arc::new),
            };
            index.entries.insert(path.clone(), entry);
            index.restale(&path);
        }
    }
}

async fn read_settings(server: &AssetServer, path: &str) -> Option<ModelSettings> {
    let asset_path = AssetPath::parse(path);
    let source = server.get_source(asset_path.source()).ok()?;
    let bytes = source
        .reader()
        .read_meta_bytes(asset_path.path())
        .await
        .ok()?;
    read_model_meta(&bytes)
}

type Placements<'w, 's> = Query<
    'w,
    's,
    (
        &'static GltfSource,
        Option<&'static LodOverride>,
        Option<&'static ModelLevels>,
        Option<&'static ChildOf>,
    ),
    Without<LodGroup>,
>;

/// Give each placed model whose settings or override changed the levels they
/// now make, and take them off a model that has none or that a [`LodGroup`]
/// draws.
fn resolve_model_levels(
    mut commands: Commands,
    mut index: ResMut<ModelLodIndex>,
    overridden: Query<Entity, Or<(Changed<LodOverride>, Changed<ChildOf>)>>,
    mut removed: RemovedComponents<LodOverride>,
    regrouped: Query<&Children, Changed<LodGroup>>,
    mut ungrouped: RemovedComponents<LodGroup>,
    children: Query<&Children>,
    grouped: Query<(&LodGroup, &Children)>,
    placements: Placements,
    held: Query<(), With<ModelLevels>>,
) {
    let mut due: HashSet<Entity> = std::mem::take(&mut index.bypass_change_detection().stale);
    due.extend(&overridden);
    due.extend(removed.read());
    due.extend(regrouped.iter().flat_map(RelationshipTarget::iter));
    for group in ungrouped.read() {
        due.insert(group);
        due.extend(
            children
                .get(group)
                .into_iter()
                .flat_map(RelationshipTarget::iter),
        );
    }
    for entity in due {
        let wanted = match placements.get(entity) {
            Ok((source, placement, _, parent)) => {
                let level_of_group = parent
                    .and_then(|parent| grouped.get(parent.parent()).ok())
                    .is_some_and(|(group, children)| {
                        children
                            .iter()
                            .take(group.levels.len())
                            .any(|level| level == entity)
                    });
                let path = source_path(source);
                index
                    .get(&path)
                    .filter(|_| !level_of_group)
                    .map(|lod| ModelLevels::new(&path, lod, placement))
            }
            Err(_) => None,
        };
        let current = placements.get(entity).ok().and_then(|(.., held, _)| held);
        match wanted {
            Some(wanted) => {
                if current != Some(&wanted) {
                    commands.entity(entity).try_insert(wanted);
                }
            }
            None => {
                if held.contains(entity) {
                    commands.entity(entity).try_remove::<ModelLevels>();
                }
            }
        }
    }
}
