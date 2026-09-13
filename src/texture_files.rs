//! What an image file on disk is, and what the editor can do with one: read
//! its layers and faces, load a thumbnail for it, step through an array
//! texture, and apply it to the selection as a material.

use std::path::{Path, PathBuf};

use bevy::{
    asset::RenderAssetUsages,
    image::{CompressedImageFormats, ImageSampler, ImageType},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureSampleType},
};
use jackdaw_api::prelude::*;
use path_slash::PathExt as _;

use crate::{
    brush::{Brush, BrushEditMode, BrushSelection, EditMode, LastUsedMaterial},
    material_browser::MaterialRegistry,
    selection::Selection,
};

/// The extensions the editor shows a picture for.
const IMAGE_EXTENSIONS: [&str; 7] = ["png", "jpg", "jpeg", "bmp", "tga", "webp", "ktx2"];

/// Whether a path names an image file the editor can show.
pub fn is_image_file_path(path: &Path) -> bool {
    let Some(extension) = path.extension() else {
        return false;
    };
    let extension = extension.to_string_lossy().to_lowercase();
    IMAGE_EXTENSIONS.contains(&extension.as_str())
}

/// Whether a KTX2 file holds something other than a plain 2D texture: a
/// cubemap, an array, or a volume.
pub fn is_ktx2_non_2d(path: &Path) -> bool {
    let Some(header) = read_ktx2_header(path) else {
        return false;
    };
    let pixel_depth = u32::from_le_bytes([header[28], header[29], header[30], header[31]]);
    let layer_count = u32::from_le_bytes([header[32], header[33], header[34], header[35]]);
    let face_count = u32::from_le_bytes([header[36], header[37], header[38], header[39]]);
    pixel_depth > 0 || layer_count > 1 || face_count > 1
}

/// The layer and face counts a KTX2 header reports.
fn read_ktx2_info(path: &Path) -> (u32, u32) {
    let Some(header) = read_ktx2_header(path) else {
        return (1, 1);
    };
    let layer_count = u32::from_le_bytes([header[32], header[33], header[34], header[35]]);
    let face_count = u32::from_le_bytes([header[36], header[37], header[38], header[39]]);
    (layer_count, face_count)
}

fn read_ktx2_header(path: &Path) -> Option<[u8; 40]> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut header = [0u8; 40];
    file.read_exact(&mut header).ok()?;
    Some(header)
}

/// What an image file holds, as its header reports it.
#[derive(Clone, Debug)]
pub struct TextureInfo {
    pub image_handle: Option<Handle<Image>>,
    pub is_cubemap: bool,
    pub is_array: bool,
    pub layer_count: u32,
    pub face_count: u32,
}

impl TextureInfo {
    /// What the file at `path` holds, with a thumbnail handle for the plain
    /// 2D textures the UI can draw.
    pub fn read(path: &Path, asset_server: &AssetServer) -> Self {
        let is_ktx2 = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("ktx2"));
        if !is_ktx2 {
            return Self {
                image_handle: load_thumbnail(path, asset_server),
                is_cubemap: false,
                is_array: false,
                layer_count: 1,
                face_count: 1,
            };
        }
        let (layer_count, face_count) = read_ktx2_info(path);
        let plain_2d = layer_count <= 1 && face_count <= 1;
        Self {
            image_handle: plain_2d
                .then(|| load_thumbnail(path, asset_server))
                .flatten(),
            is_cubemap: face_count > 1,
            is_array: layer_count > 1,
            layer_count,
            face_count,
        }
    }

    /// Whether the texture can be applied to a surface as it stands.
    pub fn is_plain_2d(&self) -> bool {
        !self.is_cubemap && !self.is_array
    }

    /// The line the card reads the texture as.
    pub fn description(&self) -> String {
        if self.is_cubemap {
            format!("Cubemap ({} faces)", self.face_count)
        } else if self.is_array {
            format!("{} layers", self.layer_count)
        } else {
            "2D Texture".to_string()
        }
    }
}

/// The handle the asset server gives a file under the project's assets.
pub fn load_thumbnail(path: &Path, asset_server: &AssetServer) -> Option<Handle<Image>> {
    let fs_path = path.to_slash_lossy();
    let asset_path = crate::entity_ops::to_asset_path(&fs_path);
    Some(asset_server.load(asset_path))
}

/// The image the inspector is showing a card for, and the layers it has split
/// out of an array texture.
#[derive(Resource, Default)]
pub struct AssetPreviewState {
    pub selected_path: Option<PathBuf>,
    pub selected_info: Option<TextureInfo>,
    pub current_layer: u32,
    pub layer_images: Vec<Handle<Image>>,
}

impl AssetPreviewState {
    /// Show `path`, forgetting whatever was shown before.
    pub fn show(&mut self, path: PathBuf, info: TextureInfo) {
        self.selected_path = Some(path);
        self.selected_info = Some(info);
        self.current_layer = 0;
        self.layer_images.clear();
    }

    pub fn clear(&mut self) {
        self.selected_path = None;
        self.selected_info = None;
        self.current_layer = 0;
        self.layer_images.clear();
    }
}

pub struct TextureFilesPlugin;

impl Plugin for TextureFilesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AssetPreviewState>().add_systems(
            Update,
            (extract_array_layers, remove_incompatible_image_nodes)
                .run_if(in_state(crate::AppState::Editor)),
        );
    }
}

/// Drop `ImageNode` from entities whose loaded image uses a format the UI
/// renderer cannot sample, which would otherwise bring the renderer down.
fn remove_incompatible_image_nodes(
    mut commands: Commands,
    image_nodes: Query<(Entity, &ImageNode)>,
    images: Res<Assets<Image>>,
) {
    for (entity, image_node) in &image_nodes {
        if let Some(image) = images.get(&image_node.image) {
            let sample = image.texture_descriptor.format.sample_type(None, None);
            if !matches!(sample, Some(TextureSampleType::Float { .. })) {
                commands.entity(entity).remove::<ImageNode>();
            }
        }
    }
}

/// Split the previewed array texture into one image per layer, so the card
/// can step through them.
fn extract_array_layers(
    mut preview_state: ResMut<AssetPreviewState>,
    mut images: ResMut<Assets<Image>>,
) {
    let wanted = preview_state
        .selected_info
        .as_ref()
        .is_some_and(|info| info.is_array)
        && preview_state.layer_images.is_empty()
        && preview_state.selected_path.is_some();
    if !wanted {
        return;
    }

    let Some(path) = preview_state.selected_path.clone() else {
        return;
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("ktx2");
    let Ok(image) = Image::from_buffer(
        &bytes,
        ImageType::Extension(extension),
        CompressedImageFormats::all(),
        true,
        ImageSampler::default(),
        RenderAssetUsages::default(),
    ) else {
        return;
    };

    let sample = image.texture_descriptor.format.sample_type(None, None);
    if !matches!(sample, Some(TextureSampleType::Float { .. })) {
        return;
    }

    let Some(layer_count) = preview_state
        .selected_info
        .as_ref()
        .map(|info| info.layer_count)
        .filter(|count| *count > 0)
    else {
        return;
    };
    let Some(ref data) = image.data else {
        return;
    };
    let total_size = data.len();
    let layer_size = total_size / layer_count as usize;
    if layer_size == 0 || total_size % layer_count as usize != 0 {
        return;
    }

    let descriptor = &image.texture_descriptor;
    for layer in 0..layer_count {
        let start = layer as usize * layer_size;
        let end = start + layer_size;
        let mut layer_image = Image::new(
            Extent3d {
                width: descriptor.size.width,
                height: descriptor.size.height,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            data[start..end].to_vec(),
            descriptor.format,
            image.asset_usage,
        );
        layer_image.sampler = image.sampler.clone();
        preview_state.layer_images.push(images.add(layer_image));
    }
}

/// If the texture filename matches a PBR naming convention, look the base name
/// up in the material registry and return the catalog handle.
fn try_find_registry_material(
    path: &str,
    registry: &MaterialRegistry,
) -> Option<Handle<StandardMaterial>> {
    let re = jackdaw_material::pbr_filename_regex()?;
    let filename = Path::new(path).file_name()?.to_str()?;
    let caps = re.captures(filename)?;
    let base_name = caps.get(1)?.as_str().to_lowercase();
    registry.get_by_name(&base_name).map(|e| e.handle.clone())
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<AssetCycleArrayLayerOp>();
}

fn has_array_preview(preview: Res<AssetPreviewState>) -> bool {
    preview
        .selected_info
        .as_ref()
        .is_some_and(|info| info.layer_count > 0)
}

/// Step the previewed image's selected layer by `direction`, wrapping at the
/// texture's layer count.
#[operator(
    id = "asset.cycle_array_layer",
    label = "Cycle Array Layer",
    description = "Step the previewed array texture by one layer.",
    is_available = has_array_preview,
    params(direction(i64, default = 1, doc = "How many layers to advance, can be negative.")),
)]
pub(crate) fn asset_cycle_array_layer(
    params: In<OperatorParameters>,
    mut preview: ResMut<AssetPreviewState>,
) -> OperatorResult {
    let info = preview.selected_info.as_ref()?;
    if info.layer_count == 0 {
        return OperatorResult::Cancelled;
    }
    let direction = params.as_int("direction").unwrap_or(1);
    let count = info.layer_count as i64;
    let next = ((preview.current_layer as i64) + direction).rem_euclid(count);
    preview.current_layer = next as u32;
    OperatorResult::Finished
}

/// Apply a texture material to the current face selection (in brush-edit face
/// mode) or to every face of every selected brush, expanding selected
/// non-brush parents into their child brushes.
///
/// The operator framework captures a scene-AST snapshot before and after this
/// runs and pushes it onto `CommandHistory`, so there is no manual `SetBrush`
/// push. The explicit `sync_brush_to_ast` queue at the end is what makes that
/// work: the outer `save_history` runs immediately after this system returns
/// and captures the after-snapshot from the AST, so the mutation has to land in
/// the AST before that; `BrushPlugin`'s auto-sync fires next `Update`, which is
/// too late here.
#[operator(
    id = "material.apply_texture",
    label = "Apply Texture",
    description = "Apply a texture material to the selected faces or brushes",
    params(path(
        String,
        doc = "Texture file to apply, as an asset path under the project's assets directory."
    ))
)]
pub fn apply_texture(
    In(params): In<OperatorParameters>,
    brush_selection: Res<BrushSelection>,
    edit_mode: Res<EditMode>,
    selection: Res<Selection>,
    mut brushes: Query<&mut Brush>,
    mut last_material: ResMut<LastUsedMaterial>,
    asset_server: Res<AssetServer>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    registry: Res<MaterialRegistry>,
    children_query: Query<&Children>,
    mut commands: Commands,
) -> OperatorResult {
    let path: String = match params.0.get("path") {
        Some(jackdaw_scene_types::PropertyValue::String(s)) => s.to_string(),
        _ => {
            warn!("material.apply_texture called without a String `path` parameter");
            return OperatorResult::Cancelled;
        }
    };

    let material = if let Some(handle) = try_find_registry_material(&path, &registry) {
        handle
    } else {
        let asset_path = crate::entity_ops::to_asset_path(&path);
        let image: Handle<Image> = asset_server.load(asset_path);
        materials.add(StandardMaterial {
            base_color_texture: Some(image),
            ..default()
        })
    };

    let mut modified: Vec<Entity> = Vec::new();

    let active_faces: Vec<usize> = brush_selection
        .active_sub()
        .map(|s| s.faces.clone())
        .unwrap_or_default();
    if *edit_mode == EditMode::BrushEdit(BrushEditMode::Face) && !active_faces.is_empty() {
        if let Some(entity) = brush_selection.active_brush
            && let Ok(mut brush) = brushes.get_mut(entity)
        {
            for &face_idx in &active_faces {
                if face_idx < brush.faces.len() {
                    brush.faces[face_idx].material = material.clone();
                }
            }
            modified.push(entity);
        }
    } else {
        let targets: Vec<Entity> = crate::brush::shown_edit_brushes(
            &selection.entities,
            |e| brushes.contains(e),
            |e| {
                children_query
                    .get(e)
                    .map(|c| c.iter().collect())
                    .unwrap_or_default()
            },
        );

        for entity in targets {
            if let Ok(mut brush) = brushes.get_mut(entity) {
                for face in brush.faces.iter_mut() {
                    face.material = material.clone();
                }
                modified.push(entity);
            }
        }
    }

    if modified.is_empty() {
        return OperatorResult::Cancelled;
    }

    last_material.material = Some(material);

    let to_sync = modified.clone();
    commands.queue(move |world: &mut World| {
        for entity in to_sync {
            if let Some(brush) = world.get::<Brush>(entity) {
                let brush = brush.clone();
                crate::brush::sync_brush_to_ast(world, entity, &brush);
            }
        }
    });

    for entity in modified {
        commands
            .entity(entity)
            .insert(crate::inspector::InspectorDirty);
    }

    OperatorResult::Finished
}
