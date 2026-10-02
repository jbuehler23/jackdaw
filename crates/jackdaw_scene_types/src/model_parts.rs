//! A glTF model as the parts it draws: each mesh with its material, at the
//! transform the file's node graph puts it.
//!
//! Spawning a glTF scene builds one entity per node and per primitive. Code
//! that only needs the meshes -- to draw many copies, to measure the model, or
//! to read its geometry -- flattens the file once instead and reads the parts.

use std::sync::Arc;

use bevy::asset::LoadState;
use bevy::camera::primitives::{Aabb, MeshAabb};
use bevy::gltf::{Gltf, GltfMaterial, GltfMesh, GltfNode};
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;

/// One drawable part of a model.
#[derive(Clone, Debug)]
pub struct ModelPart {
    pub mesh: Handle<Mesh>,
    pub material: Handle<StandardMaterial>,
    /// The glTF's name for the material, which material overrides are keyed by.
    pub material_name: Option<String>,
    /// Where this part sat inside the glTF, flattened through the node graph
    /// above it.
    pub local: Transform,
}

/// A glTF flattened into the parts it draws.
#[derive(Clone, Debug)]
pub struct FlatModel {
    pub parts: Vec<ModelPart>,
    /// Bounds of every part together, in the model's own space.
    pub bounds: Aabb,
    /// Whether the model moves or holds more than one scene: skins,
    /// animations and morph targets play only on a spawned instance.
    pub needs_instance: bool,
}

/// Every drawable part of a glTF, with the transform of the node it hung under
/// folded in.
///
/// `None` while any part is still loading: a half-resolved model would have to
/// be redone when the rest arrived.
pub fn flatten_gltf(
    gltf: &Gltf,
    nodes: &Assets<GltfNode>,
    meshes: &Assets<GltfMesh>,
    mesh_assets: &Assets<Mesh>,
    server: &AssetServer,
) -> Option<FlatModel> {
    flatten_gltf_nodes(gltf, nodes, meshes, mesh_assets, server, None)
}

/// The parts [`flatten_gltf`] finds, kept to those under the nodes named in
/// `only`, each node with everything below it, when `only` is given.
pub fn flatten_gltf_nodes(
    gltf: &Gltf,
    nodes: &Assets<GltfNode>,
    meshes: &Assets<GltfMesh>,
    mesh_assets: &Assets<Mesh>,
    server: &AssetServer,
    only: Option<&[String]>,
) -> Option<FlatModel> {
    let mut child_ids = HashSet::new();
    let mut needs_instance = !gltf.skins.is_empty() || gltf.scenes.len() > 1;
    for handle in &gltf.nodes {
        let node = nodes.get(handle)?;
        needs_instance |= node.skin.is_some() || node.is_animation_root;
        for child in &node.children {
            child_ids.insert(child.id());
        }
    }

    let material_names: HashMap<AssetId<GltfMaterial>, String> = gltf
        .named_materials
        .iter()
        .map(|(name, handle)| (handle.id(), name.to_string()))
        .collect();
    let mut parts = Vec::new();
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    let mut pending: Vec<(Handle<GltfNode>, Transform, bool)> = gltf
        .nodes
        .iter()
        .filter(|handle| !child_ids.contains(&handle.id()))
        .map(|handle| (handle.clone(), Transform::IDENTITY, only.is_none()))
        .collect();

    while let Some((handle, parent, above)) = pending.pop() {
        let node = nodes.get(&handle)?;
        let local = parent * node.transform;
        let kept = above || only.is_some_and(|only| only.contains(&node.name));
        for child in &node.children {
            pending.push((child.clone(), local, kept));
        }
        if !kept {
            continue;
        }
        let Some(mesh_handle) = &node.mesh else {
            continue;
        };
        let mesh = meshes.get(mesh_handle)?;
        for primitive in &mesh.primitives {
            let material = primitive
                .material
                .as_ref()
                .and_then(|handle| standard_material(server, handle))
                .unwrap_or_default();
            let mesh_asset = mesh_assets.get(&primitive.mesh)?;
            needs_instance |= mesh_asset.has_morph_targets();
            let Some(bounds) = mesh_asset.compute_aabb() else {
                warn!(
                    "a primitive of {:?} has no positions and draws nothing",
                    gltf.default_scene
                );
                continue;
            };
            let affine = local.compute_affine();
            let centre = affine.transform_point3(Vec3::from(bounds.center));
            let radius = affine
                .matrix3
                .abs()
                .mul_vec3(Vec3::from(bounds.half_extents));
            min = min.min(centre - radius);
            max = max.max(centre + radius);
            let material_name = primitive
                .material
                .as_ref()
                .and_then(|handle| material_names.get(&handle.id()).cloned());
            parts.push(ModelPart {
                mesh: primitive.mesh.clone(),
                material,
                material_name,
                local,
            });
        }
    }

    if parts.is_empty() {
        min = Vec3::ZERO;
        max = Vec3::ZERO;
    }
    Some(FlatModel {
        parts,
        bounds: Aabb::from_min_max(min, max),
        needs_instance,
    })
}

/// `model` drawing its generated level `level`: each part's mesh swapped for
/// the one the model's loader generated from it. A part with no generated
/// mesh keeps its own.
fn generated_parts(mut model: FlatModel, level: usize, server: &AssetServer) -> FlatModel {
    for part in &mut model.parts {
        let Some(path) = part.mesh.path() else {
            continue;
        };
        let Some(label) = path.label() else {
            continue;
        };
        let generated = path
            .clone_owned()
            .with_label(crate::model_import::generated_label(level, label));
        if let Some(handle) = server.get_handle(&generated) {
            part.mesh = handle;
        }
    }
    model
}

/// The `StandardMaterial` the glTF loader wrote beside a primitive's
/// `GltfMaterial`, under the same asset path with a `/std` label.
///
/// `None` for a primitive whose material has no path to hang that label on,
/// which the caller draws with the default material.
pub fn standard_material(
    server: &AssetServer,
    material: &Handle<GltfMaterial>,
) -> Option<Handle<StandardMaterial>> {
    let path = material.path()?;
    let label = path.label()?;
    Some(server.load(path.clone_owned().with_label(format!("{label}/std"))))
}

/// The asset path of the model a [`GltfSource`](crate::GltfSource) names, as
/// [`ModelParts`] keys it.
pub fn source_path(source: &crate::GltfSource) -> String {
    crate::to_asset_path(&source.path, None)
}

/// How [`ModelParts`] keys generated level `level` of the model at `path`.
pub fn generated_key(path: &str, level: usize) -> String {
    format!("{path}#{GENERATED}{level}")
}

/// The generated level a [`ModelParts`] key names, when it names one.
pub fn split_generated_key(key: &str) -> Option<(&str, usize)> {
    let (path, rest) = key.split_once('#')?;
    Some((path, rest.strip_prefix(GENERATED)?.parse().ok()?))
}

const GENERATED: &str = "generated:";

/// How [`ModelParts`] keys the parts of only `nodes` of the model at `path`.
pub fn nodes_key(path: &str, nodes: &[String]) -> String {
    format!("{path}#{}", nodes.join("|"))
}

/// The model file a [`ModelParts`] key names, and the nodes it keeps when it
/// keeps only some.
pub fn split_nodes_key(key: &str) -> (&str, Option<Vec<String>>) {
    if let Some((path, _)) = split_generated_key(key) {
        return (path, None);
    }
    match key.split_once('#') {
        Some((path, nodes)) => (path, Some(nodes.split('|').map(str::to_string).collect())),
        None => (key, None),
    }
}

/// Flattens models on request, once each, keyed by asset path.
pub struct ModelPartsPlugin;

impl Plugin for ModelPartsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ModelParts>()
            .add_systems(PreUpdate, resolve_model_parts);
    }
}

/// Add [`ModelPartsPlugin`] unless something else already has.
pub fn add_model_parts(app: &mut App) {
    if !app.is_plugin_added::<ModelPartsPlugin>() {
        app.add_plugins(ModelPartsPlugin);
    }
}

#[derive(Clone, Debug)]
enum ModelEntry {
    /// Asked for, and not yet handed to the asset server.
    Wanted,
    Loading(Handle<Gltf>),
    Ready(Arc<FlatModel>),
    Failed,
}

/// Models flattened into their parts, by asset path.
#[derive(Resource, Default)]
pub struct ModelParts {
    entries: HashMap<String, ModelEntry>,
    loading: Vec<String>,
    /// Paths that finished, loaded or failed, on the last resolve.
    settled: Vec<String>,
}

impl ModelParts {
    /// Ask for the model at `path` unless it is already known. It starts
    /// loading on the next resolve, in the order it was asked for, once the
    /// app can load glTF files.
    pub fn request(&mut self, path: &str) {
        if self.entries.contains_key(path) {
            return;
        }
        self.entries.insert(path.to_string(), ModelEntry::Wanted);
        self.loading.push(path.to_string());
    }

    /// The model at `path`, once it has loaded and flattened.
    pub fn get(&self, path: &str) -> Option<&Arc<FlatModel>> {
        match self.entries.get(path) {
            Some(ModelEntry::Ready(model)) => Some(model),
            _ => None,
        }
    }

    /// Whether the model at `path` was asked for and is not done loading.
    pub fn is_loading(&self, path: &str) -> bool {
        matches!(
            self.entries.get(path),
            Some(ModelEntry::Wanted | ModelEntry::Loading(_))
        )
    }

    /// Whether the model at `path` failed to load.
    pub fn failed(&self, path: &str) -> bool {
        matches!(self.entries.get(path), Some(ModelEntry::Failed))
    }

    /// How many requested models are still loading.
    pub fn loading_count(&self) -> usize {
        self.loading.len()
    }

    /// How many models have been asked for.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The paths that finished loading, or failed, on the last resolve.
    pub fn settled(&self) -> &[String] {
        &self.settled
    }

    /// Put a flattened model in place under `path`, as a loaded one would be.
    pub fn insert(&mut self, path: &str, model: FlatModel) {
        self.loading.retain(|loading| loading != path);
        self.entries
            .insert(path.to_string(), ModelEntry::Ready(Arc::new(model)));
        self.settled.push(path.to_string());
    }
}

fn resolve_model_parts(
    parts: ResMut<ModelParts>,
    server: Option<Res<AssetServer>>,
    gltfs: Option<Res<Assets<Gltf>>>,
    nodes: Option<Res<Assets<GltfNode>>>,
    meshes: Option<Res<Assets<GltfMesh>>>,
    mesh_assets: Option<Res<Assets<Mesh>>>,
    imports: Option<Res<crate::model_import::ModelLodIndex>>,
) {
    let (Some(server), Some(gltfs), Some(nodes), Some(meshes), Some(mesh_assets)) =
        (server, gltfs, nodes, meshes, mesh_assets)
    else {
        return;
    };
    if parts.loading.is_empty() {
        if !parts.settled.is_empty() {
            parts.into_inner().settled.clear();
        }
        return;
    }
    let parts = parts.into_inner();
    parts.settled.clear();
    let loading = std::mem::take(&mut parts.loading);
    for path in loading {
        let handle = match parts.entries.get(&path) {
            Some(ModelEntry::Loading(handle)) => handle.clone(),
            Some(ModelEntry::Wanted) => {
                let handle: Handle<Gltf> = crate::render_assets::load_model(
                    &server,
                    imports.as_deref(),
                    split_nodes_key(&path).0.to_string(),
                );
                parts
                    .entries
                    .insert(path.clone(), ModelEntry::Loading(handle));
                parts.loading.push(path);
                continue;
            }
            _ => continue,
        };
        if matches!(server.load_state(handle.id()), LoadState::Failed(_)) {
            warn!("{path} failed to load; nothing is drawn from it");
            parts.entries.insert(path.clone(), ModelEntry::Failed);
            parts.settled.push(path);
            continue;
        }
        let only = split_nodes_key(&path).1;
        let model = gltfs
            .get(&handle)
            .and_then(|gltf| {
                flatten_gltf_nodes(
                    gltf,
                    &nodes,
                    &meshes,
                    &mesh_assets,
                    &server,
                    only.as_deref(),
                )
            })
            .map(|model| match split_generated_key(&path) {
                Some((_, level)) => generated_parts(model, level, &server),
                None => model,
            });
        match model {
            Some(model) => {
                parts
                    .entries
                    .insert(path.clone(), ModelEntry::Ready(Arc::new(model)));
                parts.settled.push(path);
            }
            None => parts.loading.push(path),
        }
    }
}
