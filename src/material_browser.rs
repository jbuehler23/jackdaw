use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bevy::{
    feathers::theme::ThemedText,
    image::ImageLoaderSettings,
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures_lite::future},
    ui_widgets::observe,
    window::{PrimaryWindow, RawHandleWrapper},
};
use jackdaw_feathers::{
    icons,
    text_edit::{self, TextEditCommitEvent, TextEditDragging, TextEditProps, TextEditValue},
    tokens,
};
use rfd::AsyncFileDialog;

use crate::{
    EditorEntity,
    asset_browser::attach_tooltip,
    brush::{Brush, BrushEditMode, BrushSelection, EditMode, SetBrush},
    commands::CommandHistory,
    material_preview::MaterialPreviewState,
    selection::Selection,
};

pub struct MaterialBrowserPlugin;

impl Plugin for MaterialBrowserPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MaterialBrowserState>()
            .init_resource::<MaterialPreviewState>()
            .init_resource::<MaterialRegistry>()
            .add_systems(
                OnEnter(crate::AppState::Editor),
                (
                    |world: &mut World| crate::asset_catalog::load_catalog(world),
                    scan_material_definitions,
                    |world: &mut World| crate::asset_catalog::save_catalog(world),
                    crate::material_preview::setup_material_preview_scene,
                )
                    .chain(),
            )
            .add_systems(
                Update,
                (
                    rescan_material_definitions,
                    save_catalog_if_dirty,
                    apply_material_filter,
                    update_material_browser_ui,
                    update_preview_area,
                    poll_material_browser_folder,
                    poll_texture_slot_pick,
                    crate::material_preview::update_preview_camera_transform,
                    crate::material_preview::update_active_preview_material,
                )
                    .run_if(in_state(crate::AppState::Editor)),
            )
            .add_observer(handle_apply_material)
            .add_observer(handle_select_material_preview)
            .add_observer(on_material_param_commit)
            .add_observer(handle_create_new_material)
            .add_observer(handle_browse_texture_slot)
            .add_observer(handle_clear_texture_slot);
    }
}

/// Simple registry of named materials for browsing.
#[derive(Resource, Default)]
pub struct MaterialRegistry {
    pub entries: Vec<MaterialRegistryEntry>,
}

pub struct MaterialRegistryEntry {
    pub name: String,
    pub handle: Handle<StandardMaterial>,
}

impl MaterialRegistry {
    pub fn get_by_name(&self, name: &str) -> Option<&MaterialRegistryEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    pub fn add(&mut self, name: String, handle: Handle<StandardMaterial>) {
        self.entries.push(MaterialRegistryEntry { name, handle });
    }
}

#[derive(Resource, Default)]
pub struct MaterialBrowserState {
    pub filter: String,
    pub needs_rescan: bool,
    pub scan_directory: PathBuf,
}

#[derive(Event, Clone)]
pub struct ApplyMaterialDefToFaces {
    pub material: Handle<StandardMaterial>,
}

#[derive(Event, Clone)]
struct SelectMaterialPreview {
    handle: Handle<StandardMaterial>,
}

#[derive(Component)]
pub struct MaterialBrowserPanel;

#[derive(Component)]
pub struct MaterialBrowserGrid;

#[derive(Component)]
pub struct MaterialBrowserFilter;

#[derive(Component)]
struct MaterialBrowserRootLabel;

#[derive(Resource)]
struct MaterialBrowserFolderTask(Task<Option<rfd::FileHandle>>);

/// Container for the interactive preview area (shown when a material is selected).
#[derive(Component)]
struct PreviewAreaContainer;

/// The ImageNode displaying the render-to-texture preview.
#[derive(Component)]
struct PreviewAreaImage;

/// Text label showing the selected material name in the preview area.
#[derive(Component)]
struct PreviewAreaLabel;

/// Identifies which material parameter a numeric input controls.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum MaterialParamInput {
    ParallaxDepthScale,
    MaxParallaxLayers,
    PerceptualRoughness,
    Metallic,
    Reflectance,
}

/// Identifies a texture slot on `StandardMaterial`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TextureSlot {
    BaseColorTexture,
    NormalMapTexture,
    MetallicRoughnessTexture,
    EmissiveTexture,
    OcclusionTexture,
    DepthMap,
}

impl TextureSlot {
    const ALL: [TextureSlot; 6] = [
        TextureSlot::BaseColorTexture,
        TextureSlot::NormalMapTexture,
        TextureSlot::MetallicRoughnessTexture,
        TextureSlot::EmissiveTexture,
        TextureSlot::OcclusionTexture,
        TextureSlot::DepthMap,
    ];

    fn label(self) -> &'static str {
        match self {
            TextureSlot::BaseColorTexture => "base_color_texture",
            TextureSlot::NormalMapTexture => "normal_map_texture",
            TextureSlot::MetallicRoughnessTexture => "metallic_roughness_texture",
            TextureSlot::EmissiveTexture => "emissive_texture",
            TextureSlot::OcclusionTexture => "occlusion_texture",
            TextureSlot::DepthMap => "depth_map",
        }
    }

    fn is_srgb(self) -> bool {
        matches!(
            self,
            TextureSlot::BaseColorTexture | TextureSlot::EmissiveTexture
        )
    }

    fn get_from(self, mat: &StandardMaterial) -> Option<Handle<Image>> {
        match self {
            TextureSlot::BaseColorTexture => mat.base_color_texture.clone(),
            TextureSlot::NormalMapTexture => mat.normal_map_texture.clone(),
            TextureSlot::MetallicRoughnessTexture => mat.metallic_roughness_texture.clone(),
            TextureSlot::EmissiveTexture => mat.emissive_texture.clone(),
            TextureSlot::OcclusionTexture => mat.occlusion_texture.clone(),
            TextureSlot::DepthMap => mat.depth_map.clone(),
        }
    }

    fn set_on(self, mat: &mut StandardMaterial, handle: Option<Handle<Image>>) {
        match self {
            TextureSlot::BaseColorTexture => mat.base_color_texture = handle,
            TextureSlot::NormalMapTexture => mat.normal_map_texture = handle,
            TextureSlot::MetallicRoughnessTexture => {
                mat.metallic_roughness_texture = handle;
                if mat.metallic_roughness_texture.is_some() {
                    // When a metallic/roughness texture is present, scalars multiply the
                    // texture values. Default both to 1.0 so the texture is used as-is.
                    mat.metallic = 1.0;
                    mat.perceptual_roughness = 1.0;
                }
            }
            TextureSlot::EmissiveTexture => mat.emissive_texture = handle,
            TextureSlot::OcclusionTexture => mat.occlusion_texture = handle,
            TextureSlot::DepthMap => {
                let has_depth = handle.is_some();
                mat.depth_map = handle;
                if has_depth {
                    if mat.parallax_depth_scale == 0.0 {
                        mat.parallax_depth_scale = 0.05;
                    }
                    if mat.max_parallax_layer_count == 0.0 {
                        mat.max_parallax_layer_count = 32.0;
                    }
                    mat.parallax_mapping_method = bevy::pbr::ParallaxMappingMethod::Occlusion;
                } else {
                    mat.parallax_depth_scale = 0.0;
                    mat.max_parallax_layer_count = 0.0;
                }
            }
        }
    }
}

#[derive(Event)]
struct CreateNewMaterial;

#[derive(Event)]
struct BrowseTextureSlot {
    slot: TextureSlot,
    material_handle: Handle<StandardMaterial>,
}

#[derive(Event)]
struct ClearTextureSlot {
    slot: TextureSlot,
    material_handle: Handle<StandardMaterial>,
}

#[derive(Resource)]
struct TextureSlotPickTask {
    task: Task<Option<rfd::FileHandle>>,
    slot: TextureSlot,
    material_handle: Handle<StandardMaterial>,
}

/// PBR filename regex pattern.
pub(crate) fn pbr_filename_regex() -> Option<regex::Regex> {
    let pattern = r"(?i)^(.+?)[_\-\.\s](diffuse|diff|albedo|base|col|color|basecolor|metallic|metalness|metal|mtl|roughness|rough|rgh|normal|normaldx|normalgl|nor|nrm|nrml|norm|orm|emission|emissive|emit|ao|ambient|occlusion|ambientocclusion|displacement|displace|disp|dsp|height|heightmap|alpha|opacity|specularity|specular|spec|spc|gloss|glossy|glossiness|bump|bmp|b|n)\.(png|jpg|jpeg|ktx2|bmp|tga|webp)$";
    regex::Regex::new(pattern).ok()
}

/// Returns `true` if the PNG file uses 16-bit (or higher) bit depth.
///
/// Bevy decodes such PNGs as `R16Uint` which is incompatible with
/// `StandardMaterial`'s float-filterable `depth_map` slot.
fn is_16bit_png(path: &Path) -> bool {
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("png"))
    {
        return false;
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    use std::io::Read;
    // PNG layout: 8-byte signature, then IHDR chunk (4 len + 4 type + 13 data).
    // Byte 24 (offset 24) is the bit depth field inside IHDR.
    let mut header = [0u8; 25];
    if file.read_exact(&mut header).is_err() {
        return false;
    }
    header[24] >= 16
}

/// Scan a directory for PBR texture sets and create `StandardMaterial` assets.
fn detect_and_create_materials(
    dir: &Path,
    asset_server: &AssetServer,
    materials: &mut Assets<StandardMaterial>,
) -> Vec<(String, Handle<StandardMaterial>)> {
    let re = match pbr_filename_regex() {
        Some(r) => r,
        None => return Vec::new(),
    };

    let mut groups: HashMap<String, Vec<(String, String)>> = HashMap::new();
    scan_dir_recursive(dir, &re, &mut groups);

    let mut results = Vec::new();
    for (base_name, slots) in &groups {
        let mut base_color_texture = None;
        let mut normal_map_texture = None;
        let mut metallic_roughness_texture = None;
        let mut emissive_texture = None;
        let mut occlusion_texture = None;
        let mut depth_map = None;

        for (tag, asset_path) in slots {
            let tag_lower = tag.to_lowercase();
            match tag_lower.as_str() {
                "diffuse" | "diff" | "albedo" | "base" | "col" | "color" | "basecolor" | "b" => {
                    base_color_texture = Some(asset_server.load::<Image>(asset_path.clone()));
                }
                "normalgl" | "nor" | "nrm" | "nrml" | "norm" | "bump" | "bmp" | "n" | "normal" => {
                    normal_map_texture = Some(
                        asset_server.load_with_settings::<Image, ImageLoaderSettings>(
                            asset_path.clone(),
                            |s: &mut ImageLoaderSettings| s.is_srgb = false,
                        ),
                    );
                }
                "orm" => {
                    let img = asset_server.load_with_settings::<Image, ImageLoaderSettings>(
                        asset_path.clone(),
                        |s: &mut ImageLoaderSettings| s.is_srgb = false,
                    );
                    if metallic_roughness_texture.is_none() {
                        metallic_roughness_texture = Some(img.clone());
                    }
                    if occlusion_texture.is_none() {
                        occlusion_texture = Some(img);
                    }
                }
                "metallic" | "metalness" | "metal" | "mtl" | "roughness" | "rough" | "rgh"
                    if metallic_roughness_texture.is_none() => {
                        metallic_roughness_texture = Some(
                            asset_server.load_with_settings::<Image, ImageLoaderSettings>(
                                asset_path.clone(),
                                |s: &mut ImageLoaderSettings| s.is_srgb = false,
                            ),
                        );
                    }
                "emission" | "emissive" | "emit" => {
                    emissive_texture = Some(asset_server.load::<Image>(asset_path.clone()));
                }
                "ao" | "ambient" | "occlusion" | "ambientocclusion" => {
                    occlusion_texture = Some(
                        asset_server.load_with_settings::<Image, ImageLoaderSettings>(
                            asset_path.clone(),
                            |s: &mut ImageLoaderSettings| s.is_srgb = false,
                        ),
                    );
                }
                "displacement" | "displace" | "disp" | "dsp" | "height" | "heightmap"
                    // Skip 16-bit integer PNGs. Bevy decodes them as R16Uint which
                    // is incompatible with StandardMaterial's float-filterable depth_map slot.
                    if !is_16bit_png(Path::new(asset_path)) => {
                        depth_map = Some(
                            asset_server.load_with_settings::<Image, ImageLoaderSettings>(
                                asset_path.clone(),
                                |s: &mut ImageLoaderSettings| s.is_srgb = false,
                            ),
                        );
                    }
                _ => {}
            }
        }

        // Only create if at least one texture slot is populated
        if base_color_texture.is_none()
            && normal_map_texture.is_none()
            && metallic_roughness_texture.is_none()
            && emissive_texture.is_none()
            && occlusion_texture.is_none()
            && depth_map.is_none()
        {
            continue;
        }

        let has_depth = depth_map.is_some();
        let has_mr = metallic_roughness_texture.is_some();
        let handle = materials.add(StandardMaterial {
            base_color_texture,
            normal_map_texture,
            metallic_roughness_texture,
            emissive_texture,
            occlusion_texture,
            depth_map,
            metallic: if has_mr { 1.0 } else { 0.0 },
            perceptual_roughness: if has_mr { 1.0 } else { 0.5 },
            parallax_depth_scale: if has_depth { 0.05 } else { 0.0 },
            parallax_mapping_method: bevy::pbr::ParallaxMappingMethod::Occlusion,
            max_parallax_layer_count: if has_depth { 32.0 } else { 0.0 },
            ..default()
        });

        results.push((base_name.clone(), handle));
    }

    results.sort_by(|a, b| a.0.cmp(&b.0));
    results
}

fn scan_dir_recursive(
    dir: &Path,
    re: &regex::Regex,
    groups: &mut HashMap<String, Vec<(String, String)>>,
) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_dir_recursive(&path, re, groups);
        } else {
            let file_name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            // Skip non-2D KTX2 files (cubemaps, texture arrays). They can't
            // be used as regular 2D textures in StandardMaterial.
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("ktx2"))
                && crate::asset_browser::is_ktx2_non_2d(&path)
            {
                continue;
            }

            if let Some(caps) = re.captures(&file_name) {
                let base_name = caps[1].to_string();
                let tag = caps[2].to_string();

                let asset_path = path.to_string_lossy().replace('\\', "/");

                groups
                    .entry(base_name.to_lowercase())
                    .or_default()
                    .push((tag, asset_path));
            }
        }
    }
}

fn scan_material_definitions(world: &mut World) {
    let assets_dir = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(|p| p.assets_dir())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default().join("assets"));
    world.resource_mut::<MaterialBrowserState>().scan_directory = assets_dir.clone();

    let detected = {
        let asset_server = world.resource::<AssetServer>().clone();
        let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
        detect_and_create_materials(&assets_dir, &asset_server, &mut materials)
    };

    for (name, handle) in detected {
        let already_registered = world
            .resource::<MaterialRegistry>()
            .get_by_name(&name)
            .is_some();
        if already_registered {
            continue;
        }

        let catalog_name = format!("@{name}");
        let already_in_catalog = world
            .resource::<crate::asset_catalog::AssetCatalog>()
            .contains_name(&catalog_name);
        if already_in_catalog {
            // Use the catalog's existing handle so scene saves find it in id_to_name
            let catalog_handle = world
                .resource::<crate::asset_catalog::AssetCatalog>()
                .handles
                .get(&catalog_name)
                .cloned();
            if let Some(h) = catalog_handle {
                world
                    .resource_mut::<MaterialRegistry>()
                    .add(name, h.typed::<StandardMaterial>());
            }
        } else {
            // Serialize the material + nested textures into catalog assets
            let mut catalog_assets = world
                .resource::<crate::asset_catalog::AssetCatalog>()
                .assets
                .clone();
            crate::scene_io::serialize_asset_into(
                world,
                handle.clone().untyped(),
                &catalog_name,
                &assets_dir,
                &mut catalog_assets,
            );
            let mut catalog = world.resource_mut::<crate::asset_catalog::AssetCatalog>();
            catalog.assets = catalog_assets;
            catalog.insert(catalog_name, handle.clone().untyped());
            catalog.dirty = true;

            world.resource_mut::<MaterialRegistry>().add(name, handle);
        }
    }
}

fn rescan_material_definitions(world: &mut World) {
    let needs_rescan = world.resource::<MaterialBrowserState>().needs_rescan;
    if !needs_rescan {
        return;
    }

    let assets_dir = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(|p| p.assets_dir())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default().join("assets"));

    {
        let mut state = world.resource_mut::<MaterialBrowserState>();
        state.needs_rescan = false;
        state.scan_directory = assets_dir.clone();
    }

    world.resource_mut::<MaterialRegistry>().entries.clear();

    let detected = {
        let asset_server = world.resource::<AssetServer>().clone();
        let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
        detect_and_create_materials(&assets_dir, &asset_server, &mut materials)
    };

    for (name, handle) in detected {
        let catalog_name = format!("@{name}");
        let already_in_catalog = world
            .resource::<crate::asset_catalog::AssetCatalog>()
            .contains_name(&catalog_name);
        if already_in_catalog {
            // Use the catalog's existing handle so scene saves find it in id_to_name
            let catalog_handle = world
                .resource::<crate::asset_catalog::AssetCatalog>()
                .handles
                .get(&catalog_name)
                .cloned();
            if let Some(h) = catalog_handle {
                world
                    .resource_mut::<MaterialRegistry>()
                    .add(name, h.typed::<StandardMaterial>());
            }
        } else {
            let mut catalog_assets = world
                .resource::<crate::asset_catalog::AssetCatalog>()
                .assets
                .clone();
            crate::scene_io::serialize_asset_into(
                world,
                handle.clone().untyped(),
                &catalog_name,
                &assets_dir,
                &mut catalog_assets,
            );
            let mut catalog = world.resource_mut::<crate::asset_catalog::AssetCatalog>();
            catalog.assets = catalog_assets;
            catalog.insert(catalog_name, handle.clone().untyped());
            catalog.dirty = true;

            world.resource_mut::<MaterialRegistry>().add(name, handle);
        }
    }
}

fn save_catalog_if_dirty(world: &mut World) {
    let is_dirty = world
        .get_resource::<crate::asset_catalog::AssetCatalog>()
        .is_some_and(|c| c.dirty);
    if !is_dirty {
        return;
    }

    // Re-serialize all registry materials so in-memory edits persist.
    let assets_dir = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(|p| p.assets_dir())
        .unwrap_or_default();
    let entries: Vec<(String, UntypedHandle)> = world
        .resource::<MaterialRegistry>()
        .entries
        .iter()
        .map(|e| (format!("@{}", e.name), e.handle.clone().untyped()))
        .collect();

    let mut catalog_assets = world
        .resource::<crate::asset_catalog::AssetCatalog>()
        .assets
        .clone();
    for (catalog_name, handle) in &entries {
        crate::scene_io::serialize_asset_into(
            world,
            handle.clone(),
            catalog_name,
            &assets_dir,
            &mut catalog_assets,
        );
    }
    world
        .resource_mut::<crate::asset_catalog::AssetCatalog>()
        .assets = catalog_assets;

    crate::asset_catalog::save_catalog(world);
}

fn apply_material_filter(
    filter_input: Query<&TextEditValue, (With<MaterialBrowserFilter>, Changed<TextEditValue>)>,
    mut state: ResMut<MaterialBrowserState>,
) {
    for input in &filter_input {
        if state.filter != input.0 {
            state.filter = input.0.clone();
        }
    }
}

fn handle_apply_material(
    event: On<ApplyMaterialDefToFaces>,
    brush_selection: Res<BrushSelection>,
    edit_mode: Res<EditMode>,
    selection: Res<Selection>,
    mut brushes: Query<&mut Brush>,
    mut history: ResMut<CommandHistory>,
    brush_groups: Query<(), With<jackdaw_jsn::types::BrushGroup>>,
    children_query: Query<&Children>,
    mut commands: Commands,
) {
    if *edit_mode == EditMode::BrushEdit(BrushEditMode::Face) && !brush_selection.faces.is_empty() {
        if let Some(entity) = brush_selection.entity {
            if let Ok(mut brush) = brushes.get_mut(entity) {
                let old = brush.clone();
                for &face_idx in &brush_selection.faces {
                    if face_idx < brush.faces.len() {
                        brush.faces[face_idx].material = event.material.clone();
                    }
                }
                let new_brush = brush.clone();
                let cmd = SetBrush {
                    entity,
                    old,
                    new: new_brush.clone(),
                    label: "Apply material".into(),
                };
                history.undo_stack.push(Box::new(cmd));
                history.redo_stack.clear();
                // Deferred AST sync (SetBrush was pushed without execute)
                commands.queue(move |world: &mut World| {
                    crate::brush::sync_brush_to_ast(world, entity, &new_brush);
                });
                commands
                    .entity(entity)
                    .insert(crate::inspector::InspectorDirty);
            }
        }
    } else {
        // Collect targets, expanding BrushGroups into their child brushes
        let targets: Vec<Entity> = selection
            .entities
            .iter()
            .flat_map(|&e| {
                if brush_groups.contains(e) {
                    children_query
                        .get(e)
                        .map(|c| c.iter().collect::<Vec<_>>())
                        .unwrap_or_default()
                } else {
                    vec![e]
                }
            })
            .collect();
        for entity in targets {
            if let Ok(mut brush) = brushes.get_mut(entity) {
                let old = brush.clone();
                for face in brush.faces.iter_mut() {
                    face.material = event.material.clone();
                }
                let new_brush = brush.clone();
                let cmd = SetBrush {
                    entity,
                    old,
                    new: new_brush.clone(),
                    label: "Apply material".into(),
                };
                history.undo_stack.push(Box::new(cmd));
                history.redo_stack.clear();
                // Deferred AST sync (SetBrush was pushed without execute)
                commands.queue(move |world: &mut World| {
                    crate::brush::sync_brush_to_ast(world, entity, &new_brush);
                });
                commands
                    .entity(entity)
                    .insert(crate::inspector::InspectorDirty);
            }
        }
    }
}

fn handle_select_material_preview(
    event: On<SelectMaterialPreview>,
    mut preview_state: ResMut<MaterialPreviewState>,
) {
    if preview_state.active_material.as_ref() == Some(&event.handle) {
        preview_state.active_material = None;
    } else {
        preview_state.active_material = Some(event.handle.clone());
        preview_state.orbit_yaw = 0.5;
        preview_state.orbit_pitch = -0.3;
        preview_state.zoom_distance = 3.0;
    }
}

/// Update the interactive preview area visibility and content.
fn update_preview_area(
    mut commands: Commands,
    preview_state: Res<MaterialPreviewState>,
    registry: Res<MaterialRegistry>,
    materials: Res<Assets<StandardMaterial>>,
    container_query: Query<(Entity, Option<&Children>), With<PreviewAreaContainer>>,
    dragging_query: Query<(), With<TextEditDragging>>,
    all_children_query: Query<&Children>,
    icon_font: Res<icons::IconFont>,
) {
    let icon_font = icon_font.0.clone();
    if !preview_state.is_changed() {
        return;
    }

    let Ok((container, children)) = container_query.single() else {
        return;
    };

    // Don't rebuild while a slider drag is in progress. The drag system
    // has in-flight commands targeting child entities.
    if let Some(children) = children {
        for child in children.iter() {
            // TextEditDragging lives on the wrapper entity (grandchild of container)
            if dragging_query.contains(child) {
                return;
            }
            if let Ok(grandchildren) = all_children_query.get(child) {
                if grandchildren.iter().any(|gc| dragging_query.contains(gc)) {
                    return;
                }
            }
        }
    }

    // Clear existing children
    if let Some(children) = children {
        for child in children.iter() {
            commands.entity(child).despawn();
        }
    }

    let Some(ref active_handle) = preview_state.active_material else {
        return;
    };

    // Show the preview image
    let preview_img = preview_state.preview_image.clone();
    commands.spawn((
        PreviewAreaImage,
        ImageNode::new(preview_img),
        Node {
            width: Val::Px(128.0),
            height: Val::Px(128.0),
            align_self: AlignSelf::Center,
            ..Default::default()
        },
        ChildOf(container),
    ));

    // Material name
    let active_name = registry
        .entries
        .iter()
        .find(|e| e.handle == *active_handle)
        .map(|e| e.name.clone())
        .unwrap_or_else(|| format!("{:?}", active_handle.id()));
    commands.spawn((
        PreviewAreaLabel,
        Text::new(active_name),
        TextFont {
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        Node {
            align_self: AlignSelf::Center,
            margin: UiRect::vertical(Val::Px(tokens::SPACING_XS)),
            ..Default::default()
        },
        ChildOf(container),
    ));

    // Apply button
    let handle_for_apply = active_handle.clone();
    let apply_btn = commands
        .spawn((
            Node {
                padding: UiRect::axes(Val::Px(tokens::SPACING_MD), Val::Px(tokens::SPACING_XS)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_SM)),
                align_self: AlignSelf::Center,
                margin: UiRect::top(Val::Px(tokens::SPACING_XS)),
                ..Default::default()
            },
            BackgroundColor(tokens::INPUT_BG),
            ChildOf(container),
        ))
        .id();
    commands.spawn((
        Text::new("Apply"),
        TextFont {
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        ChildOf(apply_btn),
    ));
    commands
        .entity(apply_btn)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            commands.trigger(ApplyMaterialDefToFaces {
                material: handle_for_apply.clone(),
            });
        });
    commands.entity(apply_btn).observe(
        |hover: On<Pointer<Over>>, mut bg: Query<&mut BackgroundColor>| {
            if let Ok(mut bg) = bg.get_mut(hover.event_target()) {
                bg.0 = tokens::HOVER_BG;
            }
        },
    );
    commands.entity(apply_btn).observe(
        |out: On<Pointer<Out>>, mut bg: Query<&mut BackgroundColor>| {
            if let Ok(mut bg) = bg.get_mut(out.event_target()) {
                bg.0 = tokens::INPUT_BG;
            }
        },
    );

    // Material parameter sliders
    let Some(mat) = materials.get(active_handle) else {
        return;
    };

    let has_depth = mat.depth_map.is_some();

    // --- Texture slots section ---
    // Section header
    commands.spawn((
        Text::new("Textures"),
        TextFont {
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Node {
            margin: UiRect::top(Val::Px(tokens::SPACING_MD)),
            ..Default::default()
        },
        ChildOf(container),
    ));

    for slot in TextureSlot::ALL {
        let tex_handle = slot.get_from(mat);
        let has_tex = tex_handle.is_some();

        let row = commands
            .spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(tokens::SPACING_XS),
                    width: Val::Percent(100.0),
                    margin: UiRect::top(Val::Px(tokens::SPACING_XS)),
                    ..Default::default()
                },
                ChildOf(container),
            ))
            .id();

        // Slot label
        commands.spawn((
            Text::new(slot.label()),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            Node {
                min_width: Val::Px(140.0),
                flex_shrink: 0.0,
                ..Default::default()
            },
            ChildOf(row),
        ));

        // Thumbnail (24x24)
        if let Some(ref img) = tex_handle {
            commands.spawn((
                ImageNode::new(img.clone()),
                Node {
                    width: Val::Px(24.0),
                    height: Val::Px(24.0),
                    flex_shrink: 0.0,
                    ..Default::default()
                },
                ChildOf(row),
            ));
        } else {
            commands.spawn((
                Node {
                    width: Val::Px(24.0),
                    height: Val::Px(24.0),
                    flex_shrink: 0.0,
                    ..Default::default()
                },
                BackgroundColor(Color::srgb(0.25, 0.25, 0.25)),
                ChildOf(row),
            ));
        }

        // Texture filename
        let path_text = tex_handle
            .as_ref()
            .and_then(|h| h.path())
            .and_then(|p| {
                p.path()
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
            })
            .unwrap_or_else(|| "(none)".to_string());
        let path_color = tokens::TEXT_SECONDARY;
        commands.spawn((
            Text::new(path_text),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(path_color),
            Node {
                flex_grow: 1.0,
                ..Default::default()
            },
            ChildOf(row),
        ));

        // Browse button
        let browse_handle = active_handle.clone();
        let browse_btn = commands
            .spawn((
                Node {
                    padding: UiRect::all(Val::Px(2.0)),
                    border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_SM)),
                    ..Default::default()
                },
                icons::icon_colored(
                    icons::Icon::FolderOpen,
                    tokens::FONT_SM,
                    icon_font.clone(),
                    tokens::TEXT_SECONDARY,
                ),
                ChildOf(row),
            ))
            .id();
        commands.entity(browse_btn).observe(
            move |_: On<Pointer<Click>>, mut commands: Commands| {
                commands.trigger(BrowseTextureSlot {
                    slot,
                    material_handle: browse_handle.clone(),
                });
            },
        );

        // Clear button (only if texture exists)
        if has_tex {
            let clear_handle = active_handle.clone();
            let clear_btn = commands
                .spawn((
                    Node {
                        padding: UiRect::all(Val::Px(2.0)),
                        border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_SM)),
                        ..Default::default()
                    },
                    icons::icon_colored(
                        icons::Icon::X,
                        tokens::FONT_SM,
                        icon_font.clone(),
                        tokens::TEXT_SECONDARY,
                    ),
                    ChildOf(row),
                ))
                .id();
            commands.entity(clear_btn).observe(
                move |_: On<Pointer<Click>>, mut commands: Commands| {
                    commands.trigger(ClearTextureSlot {
                        slot,
                        material_handle: clear_handle.clone(),
                    });
                },
            );
        }
    }

    // --- Parameters section ---
    commands.spawn((
        Text::new("Parameters"),
        TextFont {
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Node {
            margin: UiRect::top(Val::Px(tokens::SPACING_MD)),
            ..Default::default()
        },
        ChildOf(container),
    ));

    // Helper: spawn a label + numeric input row
    let spawn_param_row = |commands: &mut Commands,
                           parent: Entity,
                           label: &str,
                           value: f32,
                           min: f64,
                           max: f64,
                           param: MaterialParamInput| {
        let row = commands
            .spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(tokens::SPACING_XS),
                    width: Val::Percent(100.0),
                    margin: UiRect::top(Val::Px(tokens::SPACING_XS)),
                    ..Default::default()
                },
                ChildOf(parent),
            ))
            .id();

        commands.spawn((
            Text::new(label),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            Node {
                min_width: Val::Px(140.0),
                flex_shrink: 0.0,
                ..Default::default()
            },
            ChildOf(row),
        ));

        commands.spawn((
            text_edit::text_edit(
                TextEditProps::default()
                    .numeric_f32()
                    .grow()
                    .with_min(min)
                    .with_max(max)
                    .with_default_value(format!("{value:.3}")),
            ),
            param,
            ChildOf(row),
        ));
    };

    if has_depth {
        spawn_param_row(
            &mut commands,
            container,
            "parallax_depth_scale",
            mat.parallax_depth_scale,
            0.0,
            0.3,
            MaterialParamInput::ParallaxDepthScale,
        );
        spawn_param_row(
            &mut commands,
            container,
            "max_parallax_layer_count",
            mat.max_parallax_layer_count,
            4.0,
            64.0,
            MaterialParamInput::MaxParallaxLayers,
        );
    }

    spawn_param_row(
        &mut commands,
        container,
        "perceptual_roughness",
        mat.perceptual_roughness,
        0.0,
        2.0,
        MaterialParamInput::PerceptualRoughness,
    );
    spawn_param_row(
        &mut commands,
        container,
        "metallic",
        mat.metallic,
        0.0,
        2.0,
        MaterialParamInput::Metallic,
    );
    spawn_param_row(
        &mut commands,
        container,
        "reflectance",
        mat.reflectance,
        0.0,
        1.0,
        MaterialParamInput::Reflectance,
    );
}

/// Handle TextEditCommitEvent for material parameter inputs.
fn on_material_param_commit(
    event: On<TextEditCommitEvent>,
    param_query: Query<&MaterialParamInput>,
    child_of_query: Query<&ChildOf>,
    preview_state: Res<MaterialPreviewState>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    registry: Res<MaterialRegistry>,
    mut catalog: ResMut<crate::asset_catalog::AssetCatalog>,
) {
    // Walk up the hierarchy to find a MaterialParamInput marker
    let mut current = event.entity;
    let mut param = None;
    for _ in 0..4 {
        let Ok(child_of) = child_of_query.get(current) else {
            break;
        };
        if let Ok(p) = param_query.get(child_of.parent()) {
            param = Some(*p);
            break;
        }
        current = child_of.parent();
    }

    let Some(param) = param else { return };
    let Some(ref active_handle) = preview_state.active_material else {
        return;
    };
    let value: f32 = event.text.parse().unwrap_or(0.0);

    let Some(mat) = materials.get_mut(active_handle) else {
        return;
    };
    match param {
        MaterialParamInput::ParallaxDepthScale => mat.parallax_depth_scale = value,
        MaterialParamInput::MaxParallaxLayers => mat.max_parallax_layer_count = value,
        MaterialParamInput::PerceptualRoughness => mat.perceptual_roughness = value,
        MaterialParamInput::Metallic => mat.metallic = value,
        MaterialParamInput::Reflectance => mat.reflectance = value,
    }

    // Persist to catalog
    let catalog_name = registry
        .entries
        .iter()
        .find(|e| e.handle == *active_handle)
        .map(|e| format!("@{}", e.name));
    if let Some(name) = catalog_name {
        if catalog.contains_name(&name) {
            catalog.dirty = true;
        }
    }
}

fn spawn_material_folder_dialog(
    _: On<Pointer<Click>>,
    mut commands: Commands,
    raw_handle: Query<&RawHandleWrapper, With<PrimaryWindow>>,
) {
    let mut dialog = AsyncFileDialog::new().set_title("Select materials directory");
    if let Ok(rh) = raw_handle.single() {
        let handle = unsafe { rh.get_handle() };
        dialog = dialog.set_parent(&handle);
    }
    let task = AsyncComputeTaskPool::get().spawn(async move { dialog.pick_folder().await });
    commands.insert_resource(MaterialBrowserFolderTask(task));
}

fn poll_material_browser_folder(world: &mut World) {
    let Some(mut task_res) = world.get_resource_mut::<MaterialBrowserFolderTask>() else {
        return;
    };
    let Some(result) = future::block_on(future::poll_once(&mut task_res.0)) else {
        return;
    };
    world.remove_resource::<MaterialBrowserFolderTask>();

    if let Some(handle) = result {
        let path = handle.path().to_path_buf();
        let mut state = world.resource_mut::<MaterialBrowserState>();
        state.scan_directory = path.clone();
        state.needs_rescan = true;

        let mut label_query = world.query_filtered::<&mut Text, With<MaterialBrowserRootLabel>>();
        for mut text in label_query.iter_mut(world) {
            **text = path.to_string_lossy().to_string();
        }
    }
}

fn handle_create_new_material(
    _: On<CreateNewMaterial>,
    mut registry: ResMut<MaterialRegistry>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut catalog: ResMut<crate::asset_catalog::AssetCatalog>,
    mut preview_state: ResMut<MaterialPreviewState>,
) {
    // Generate a unique name
    let mut idx = 1u32;
    let name = loop {
        let candidate = format!("Material_{idx}");
        if registry.get_by_name(&candidate).is_none() {
            break candidate;
        }
        idx += 1;
    };

    let handle = materials.add(StandardMaterial::default());
    let catalog_name = format!("@{name}");
    catalog.insert(catalog_name, handle.clone().untyped());
    catalog.dirty = true;
    registry.add(name, handle.clone());
    preview_state.active_material = Some(handle);
    preview_state.orbit_yaw = 0.5;
    preview_state.orbit_pitch = -0.3;
    preview_state.zoom_distance = 3.0;
}

fn handle_browse_texture_slot(
    event: On<BrowseTextureSlot>,
    mut commands: Commands,
    existing_task: Option<Res<TextureSlotPickTask>>,
    raw_handle: Query<&RawHandleWrapper, With<PrimaryWindow>>,
) {
    if existing_task.is_some() {
        return;
    }
    let slot = event.slot;
    let material_handle = event.material_handle.clone();
    let mut dialog = AsyncFileDialog::new()
        .set_title(format!("Select image for {}", slot.label()))
        .add_filter(
            "Images",
            &["png", "jpg", "jpeg", "ktx2", "bmp", "tga", "webp"],
        );
    if let Ok(rh) = raw_handle.single() {
        let handle = unsafe { rh.get_handle() };
        dialog = dialog.set_parent(&handle);
    }
    let task = AsyncComputeTaskPool::get().spawn(async move { dialog.pick_file().await });
    commands.insert_resource(TextureSlotPickTask {
        task,
        slot,
        material_handle,
    });
}

fn poll_texture_slot_pick(world: &mut World) {
    let Some(mut task_res) = world.get_resource_mut::<TextureSlotPickTask>() else {
        return;
    };
    let Some(result) = future::block_on(future::poll_once(&mut task_res.task)) else {
        return;
    };
    let slot = task_res.slot;
    let material_handle = task_res.material_handle.clone();
    world.remove_resource::<TextureSlotPickTask>();

    let Some(file_handle) = result else {
        return;
    };
    let path = file_handle.path().to_path_buf();

    // Skip 16-bit PNGs for DepthMap
    if slot == TextureSlot::DepthMap && is_16bit_png(&path) {
        return;
    }

    let asset_path = path.to_string_lossy().replace('\\', "/");
    let asset_server = world.resource::<AssetServer>().clone();
    let image_handle = if slot.is_srgb() {
        asset_server.load::<Image>(asset_path)
    } else {
        asset_server.load_with_settings::<Image, ImageLoaderSettings>(
            asset_path,
            |s: &mut ImageLoaderSettings| s.is_srgb = false,
        )
    };

    let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
    if let Some(mat) = materials.get_mut(&material_handle) {
        slot.set_on(mat, Some(image_handle));
    }

    world
        .resource_mut::<crate::asset_catalog::AssetCatalog>()
        .dirty = true;
    world.resource_mut::<MaterialPreviewState>().set_changed();
}

fn handle_clear_texture_slot(
    event: On<ClearTextureSlot>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut catalog: ResMut<crate::asset_catalog::AssetCatalog>,
    mut preview_state: ResMut<MaterialPreviewState>,
) {
    if let Some(mat) = materials.get_mut(&event.material_handle) {
        event.slot.set_on(mat, None);
    }
    catalog.dirty = true;
    preview_state.set_changed();
}

fn update_material_browser_ui(
    mut commands: Commands,
    registry: Res<MaterialRegistry>,
    state: Res<MaterialBrowserState>,
    materials: Res<Assets<StandardMaterial>>,
    project_root: Res<crate::project::ProjectRoot>,
    grid_query: Query<(Entity, Option<&Children>), With<MaterialBrowserGrid>>,
    mut root_label_query: Query<&mut Text, With<MaterialBrowserRootLabel>>,
) {
    let needs_rebuild = registry.is_changed() || state.is_changed();
    if !needs_rebuild {
        return;
    }

    for mut text in root_label_query.iter_mut() {
        **text = project_root
            .to_relative(&state.scan_directory)
            .to_string_lossy()
            .to_string();
    }

    let Ok((grid_entity, grid_children)) = grid_query.single() else {
        return;
    };

    if let Some(children) = grid_children {
        for child in children.iter() {
            commands.entity(child).despawn();
        }
    }

    let filter_lower = state.filter.to_lowercase();

    for entry in &registry.entries {
        if !filter_lower.is_empty() && !entry.name.to_lowercase().contains(&filter_lower) {
            continue;
        }

        let name = entry.name.clone();
        let handle = entry.handle.clone();

        let thumb_entity = commands
            .spawn((
                Node {
                    width: Val::Px(64.0),
                    height: Val::Px(80.0),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    padding: UiRect::all(Val::Px(2.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    border_radius: BorderRadius::all(Val::Px(4.0)),
                    ..Default::default()
                },
                BorderColor::all(Color::NONE),
                BackgroundColor(Color::NONE),
                ChildOf(grid_entity),
            ))
            .id();

        // Use base_color_texture as thumbnail if available
        let thumbnail = materials
            .get(&handle)
            .and_then(|m| m.base_color_texture.clone());

        if let Some(img) = thumbnail {
            commands.spawn((
                ImageNode::new(img),
                Node {
                    width: Val::Px(56.0),
                    height: Val::Px(56.0),
                    ..Default::default()
                },
                ChildOf(thumb_entity),
            ));
        } else {
            commands.spawn((
                Node {
                    width: Val::Px(56.0),
                    height: Val::Px(56.0),
                    ..Default::default()
                },
                BackgroundColor(Color::srgb(0.3, 0.3, 0.3)),
                ChildOf(thumb_entity),
            ));
        }

        let is_truncated = name.len() > 10;
        let display_name = if is_truncated {
            format!("{}...", &name[..8])
        } else {
            name.clone()
        };
        let name_entity = commands
            .spawn((
                Text::new(display_name),
                TextFont {
                    font_size: 9.0,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
                Node {
                    max_width: Val::Px(60.0),
                    overflow: Overflow::clip(),
                    ..Default::default()
                },
                ChildOf(thumb_entity),
            ))
            .id();
        if is_truncated {
            attach_tooltip(&mut commands, name_entity, name.clone());
        }

        // Hover
        commands.entity(thumb_entity).observe(
            |hover: On<Pointer<Over>>, mut borders: Query<&mut BorderColor>| {
                if let Ok(mut border) = borders.get_mut(hover.event_target()) {
                    *border = BorderColor::all(tokens::SELECTED_BORDER);
                }
            },
        );
        commands.entity(thumb_entity).observe(
            |out: On<Pointer<Out>>, mut borders: Query<&mut BorderColor>| {
                if let Ok(mut border) = borders.get_mut(out.event_target()) {
                    *border = BorderColor::all(Color::NONE);
                }
            },
        );

        // Single-click: select for preview
        let handle_for_select = handle.clone();
        commands.entity(thumb_entity).observe(
            move |click: On<Pointer<Click>>, mut commands: Commands| {
                if click.event().button == PointerButton::Primary {
                    commands.trigger(SelectMaterialPreview {
                        handle: handle_for_select.clone(),
                    });
                }
            },
        );
    }
}

pub fn material_browser_panel(icon_font: Handle<Font>) -> impl Bundle {
    (
        MaterialBrowserPanel,
        EditorEntity,
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_LG)),
            overflow: Overflow::clip(),
            ..Default::default()
        },
        BackgroundColor(tokens::PANEL_BG),
        children![
            // Header
            (
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::SpaceBetween,
                    width: Val::Percent(100.0),
                    min_height: Val::Px(tokens::ROW_HEIGHT),
                    padding: UiRect::horizontal(Val::Px(tokens::SPACING_MD)),
                    flex_shrink: 0.0,
                    ..Default::default()
                },
                BackgroundColor(tokens::PANEL_HEADER_BG),
                children![
                    // Left side: title + path
                    (
                        Node {
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            column_gap: Val::Px(tokens::SPACING_MD),
                            overflow: Overflow::clip(),
                            flex_shrink: 1.0,
                            ..Default::default()
                        },
                        children![
                            (
                                Text::new("Materials"),
                                TextFont {
                                    font_size: tokens::FONT_MD,
                                    ..Default::default()
                                },
                                ThemedText,
                            ),
                            (
                                MaterialBrowserRootLabel,
                                Node {
                                    margin: UiRect::vertical(Val::Px(tokens::SPACING_MD)),
                                    ..Default::default()
                                },
                                Text::new(""),
                                TextFont {
                                    font_size: tokens::FONT_SM,
                                    ..Default::default()
                                },
                                TextColor(tokens::TEXT_SECONDARY),
                            ),
                        ],
                    ),
                    // Right side: folder picker + rescan
                    (
                        Node {
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            column_gap: Val::Px(tokens::SPACING_XS),
                            ..Default::default()
                        },
                        children![
                            new_material_button(icon_font.clone()),
                            material_folder_button(icon_font.clone()),
                            rescan_button(icon_font),
                        ],
                    ),
                ],
            ),
            // Interactive preview area (content populated dynamically)
            (
                PreviewAreaContainer,
                EditorEntity,
                Node {
                    flex_direction: FlexDirection::Column,
                    width: Val::Percent(100.0),
                    padding: UiRect::all(Val::Px(tokens::SPACING_SM)),
                    flex_shrink: 1.0,
                    min_height: Val::Px(0.0),
                    overflow: Overflow::scroll_y(),
                    ..Default::default()
                },
            ),
            // Filter input
            (
                Node {
                    padding: UiRect::axes(Val::Px(tokens::SPACING_SM), Val::Px(tokens::SPACING_XS),),
                    flex_shrink: 0.0,
                    ..Default::default()
                },
                children![(
                    MaterialBrowserFilter,
                    text_edit::text_edit(
                        TextEditProps::default()
                            .with_placeholder("Filter materials")
                            .allow_empty()
                    )
                ),],
            ),
            // Grid
            (
                MaterialBrowserGrid,
                EditorEntity,
                Node {
                    flex_direction: FlexDirection::Row,
                    flex_wrap: FlexWrap::Wrap,
                    align_content: AlignContent::FlexStart,
                    width: Val::Percent(100.0),
                    flex_grow: 1.0,
                    min_height: Val::Px(0.0),
                    overflow: Overflow::scroll_y(),
                    padding: UiRect::all(Val::Px(tokens::SPACING_SM)),
                    row_gap: Val::Px(tokens::SPACING_XS),
                    column_gap: Val::Px(tokens::SPACING_XS),
                    ..Default::default()
                },
            ),
        ],
    )
}

fn new_material_button(icon_font: Handle<Font>) -> impl Bundle {
    (
        Node {
            padding: UiRect::all(Val::Px(tokens::SPACING_XS)),
            border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_SM)),
            ..Default::default()
        },
        icons::icon_colored(
            icons::Icon::Plus,
            tokens::FONT_MD,
            icon_font,
            tokens::TEXT_SECONDARY,
        ),
        observe(|_: On<Pointer<Click>>, mut commands: Commands| {
            commands.trigger(CreateNewMaterial);
        }),
    )
}

fn material_folder_button(icon_font: Handle<Font>) -> impl Bundle {
    (
        Node {
            padding: UiRect::all(Val::Px(tokens::SPACING_XS)),
            border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_SM)),
            ..Default::default()
        },
        icons::icon_colored(
            icons::Icon::FolderOpen,
            tokens::FONT_MD,
            icon_font,
            tokens::TEXT_SECONDARY,
        ),
        observe(spawn_material_folder_dialog),
    )
}

fn rescan_button(icon_font: Handle<Font>) -> impl Bundle {
    (
        Node {
            padding: UiRect::all(Val::Px(tokens::SPACING_XS)),
            border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_SM)),
            ..Default::default()
        },
        icons::icon_colored(
            icons::Icon::RefreshCw,
            tokens::FONT_MD,
            icon_font,
            tokens::TEXT_SECONDARY,
        ),
        observe(
            |_: On<Pointer<Click>>, mut state: ResMut<MaterialBrowserState>| {
                state.needs_rescan = true;
            },
        ),
    )
}
