use std::path::{Path, PathBuf};

use bevy::{
    asset::RenderAssetUsages,
    feathers::theme::ThemedText,
    image::{CompressedImageFormats, ImageSampler, ImageType},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension},
    tasks::{AsyncComputeTaskPool, Task, futures_lite::future},
    ui_widgets::observe,
    window::{PrimaryWindow, RawHandleWrapper},
};
use jackdaw_feathers::{file_browser, icons, icons::IconFont, popover, tokens};
use jackdaw_widgets::file_browser::{FileBrowserItem, FileItemDoubleClicked};
use rfd::AsyncFileDialog;

use crate::{
    EditorEntity,
    brush::{Brush, BrushEditMode, BrushSelection, EditMode, LastUsedTexture, SetBrush},
    commands::CommandHistory,
    material_definition::is_ktx2_non_2d,
    selection::Selection,
};

pub struct AssetBrowserPlugin;

impl Plugin for AssetBrowserPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AssetBrowserState>()
            .init_resource::<AssetPreviewState>()
            .init_resource::<ActiveTooltip>()
            .init_resource::<PendingThumbnailLoads>()
            .add_systems(OnEnter(crate::AppState::Editor), setup_initial_directory)
            .add_systems(
                Update,
                (
                    refresh_browser_on_change,
                    poll_asset_browser_folder,
                    load_pending_thumbnails,
                    extract_array_layers,
                    update_preview_panel,
                )
                    .run_if(in_state(crate::AppState::Editor)),
            )
            .add_observer(handle_file_double_click)
            .add_observer(handle_apply_texture)
            .add_observer(handle_select_asset_preview);
    }
}

// ── Events (absorbed from texture_browser) ──────────────────────────────────

/// Apply a texture to currently selected brush faces.
#[derive(Event, Debug, Clone)]
pub struct ApplyTextureToFaces {
    pub path: String,
}

/// Clear texture from currently selected brush faces.
#[derive(Event, Debug, Clone)]
pub struct ClearTextureFromFaces;

// ── Texture info ────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct TextureInfo {
    pub image_handle: Option<Handle<Image>>,
    pub is_cubemap: bool,
    pub is_array: bool,
    pub layer_count: u32,
    pub face_count: u32,
}

// ── Browser state ───────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BrowserViewMode {
    #[default]
    Grid,
    List,
}

#[derive(Resource)]
pub struct AssetBrowserState {
    pub current_directory: PathBuf,
    pub root_directory: PathBuf,
    pub filter: String,
    pub view_mode: BrowserViewMode,
    pub needs_refresh: bool,
    pub entries: Vec<DirEntry>,
}

impl Default for AssetBrowserState {
    fn default() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            current_directory: cwd.clone(),
            root_directory: cwd,
            filter: String::new(),
            view_mode: BrowserViewMode::Grid,
            needs_refresh: true,
            entries: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct DirEntry {
    pub path: PathBuf,
    pub file_name: String,
    pub is_directory: bool,
    pub texture_info: Option<TextureInfo>,
}

// ── Preview state ───────────────────────────────────────────────────────────

#[derive(Resource, Default)]
pub struct AssetPreviewState {
    pub selected_path: Option<PathBuf>,
    pub selected_info: Option<TextureInfo>,
    pub current_layer: u32,
    pub layer_images: Vec<Handle<Image>>,
}

#[derive(Resource, Default)]
pub struct ActiveTooltip(pub Option<Entity>);

#[derive(Event, Debug, Clone)]
struct SelectAssetPreview {
    path: PathBuf,
    info: TextureInfo,
}

// ── Components ──────────────────────────────────────────────────────────────

#[derive(Component)]
pub struct AssetBrowserPanel;

#[derive(Component)]
pub struct AssetBrowserContent;

#[derive(Component)]
pub struct AssetBrowserBreadcrumb;

#[derive(Component)]
struct AssetBrowserRootLabel;

#[derive(Component)]
struct PreviewPanelContainer;

#[derive(Resource)]
struct AssetBrowserFolderTask(Task<Option<rfd::FileHandle>>);

#[derive(Resource, Default)]
struct PendingThumbnailLoads(Vec<(PathBuf, Handle<Image>)>);

// ── Helpers (absorbed from texture_browser) ─────────────────────────────────

fn is_image_file_path(path: &Path) -> bool {
    let Some(ext) = path.extension() else {
        return false;
    };
    let ext = ext.to_string_lossy().to_lowercase();
    matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "bmp" | "tga" | "webp" | "ktx2"
    )
}

fn is_image_file(path: &str) -> bool {
    is_image_file_path(Path::new(path))
}

/// Read KTX2 header to get layer/face counts.
fn read_ktx2_info(path: &Path) -> (u32, u32) {
    let Ok(mut file) = std::fs::File::open(path) else {
        return (1, 1);
    };
    use std::io::Read;
    let mut header = [0u8; 40];
    if file.read_exact(&mut header).is_err() {
        return (1, 1);
    }
    let layer_count = u32::from_le_bytes([header[32], header[33], header[34], header[35]]);
    let face_count = u32::from_le_bytes([header[36], header[37], header[38], header[39]]);
    (layer_count, face_count)
}

fn decode_image_from_disk(path: &Path) -> Option<Image> {
    let bytes = std::fs::read(path).ok()?;
    let ext = path.extension()?.to_str()?;
    Image::from_buffer(
        &bytes,
        ImageType::Extension(ext),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::default(),
        RenderAssetUsages::default(),
    )
    .ok()
}

/// Convert an absolute filesystem path to an asset-relative path.
pub fn to_asset_relative_path(absolute: &str) -> Option<String> {
    let abs_path = Path::new(absolute);

    if let Some(project_dir) = crate::project::read_last_project() {
        let assets_dir = project_dir.join("assets");
        if let Ok(relative) = abs_path.strip_prefix(&assets_dir) {
            return Some(relative.to_string_lossy().replace('\\', "/"));
        }
    }

    let assets_dir = std::env::current_dir().ok()?.join("assets");
    let relative = abs_path
        .strip_prefix(&assets_dir)
        .ok()?
        .to_string_lossy()
        .replace('\\', "/");
    Some(relative)
}

// ── Systems ─────────────────────────────────────────────────────────────────

fn setup_initial_directory(
    mut state: ResMut<AssetBrowserState>,
    project_root: Option<Res<crate::project::ProjectRoot>>,
) {
    if let Some(project) = project_root {
        let assets_dir = project.assets_dir();
        state.root_directory = assets_dir.clone();
        state.current_directory = assets_dir;
    } else {
        let assets_dir = state.root_directory.join("assets");
        if assets_dir.is_dir() {
            state.current_directory = assets_dir.clone();
            state.root_directory = assets_dir;
        }
    }
    state.needs_refresh = true;
}

fn refresh_browser_on_change(
    mut state: ResMut<AssetBrowserState>,
    mut commands: Commands,
    icon_font: Res<IconFont>,
    asset_server: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
    mut pending: ResMut<PendingThumbnailLoads>,
    content_query: Query<(Entity, Option<&Children>), With<AssetBrowserContent>>,
    breadcrumb_query: Query<(Entity, Option<&Children>), With<AssetBrowserBreadcrumb>>,
    mut root_label_query: Query<&mut Text, With<AssetBrowserRootLabel>>,
) {
    if !state.needs_refresh {
        return;
    }
    state.needs_refresh = false;

    // Scan directory
    state.entries.clear();
    if let Ok(read_dir) = std::fs::read_dir(&state.current_directory) {
        let mut entries: Vec<DirEntry> = read_dir
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let file_name = entry.file_name().to_string_lossy().to_string();
                if file_name.starts_with('.') {
                    return None;
                }
                if !state.filter.is_empty()
                    && !file_name
                        .to_lowercase()
                        .contains(&state.filter.to_lowercase())
                {
                    return None;
                }
                let path = entry.path();
                let is_directory = entry.file_type().ok()?.is_dir();

                // Build texture info for image files
                let texture_info = if !is_directory && is_image_file_path(&path) {
                    let ext = path
                        .extension()
                        .map(|e| e.to_string_lossy().to_lowercase())
                        .unwrap_or_default();

                    if ext == "ktx2" {
                        let (layer_count, face_count) = read_ktx2_info(&path);
                        let is_non_2d = layer_count > 1 || face_count > 1;
                        Some(TextureInfo {
                            image_handle: if is_non_2d {
                                None
                            } else {
                                // Load via asset server if inside asset root
                                load_thumbnail(
                                    &path,
                                    &state.root_directory,
                                    &asset_server,
                                    &mut images,
                                    &mut pending.0,
                                )
                            },
                            is_cubemap: face_count > 1,
                            is_array: layer_count > 1,
                            layer_count,
                            face_count,
                        })
                    } else {
                        Some(TextureInfo {
                            image_handle: load_thumbnail(
                                &path,
                                &state.root_directory,
                                &asset_server,
                                &mut images,
                                &mut pending.0,
                            ),
                            is_cubemap: false,
                            is_array: false,
                            layer_count: 1,
                            face_count: 1,
                        })
                    }
                } else {
                    None
                };

                Some(DirEntry {
                    path,
                    file_name,
                    is_directory,
                    texture_info,
                })
            })
            .collect();

        entries.sort_by(|a, b| {
            b.is_directory
                .cmp(&a.is_directory)
                .then_with(|| a.file_name.to_lowercase().cmp(&b.file_name.to_lowercase()))
        });

        state.entries = entries;
    }

    // Clear content area
    let Ok((content_entity, content_children)) = content_query.single() else {
        return;
    };
    if let Some(children) = content_children {
        for child in children.iter() {
            commands.entity(child).despawn();
        }
    }

    // Spawn items
    for entry in &state.entries {
        let path_for_click = entry.path.to_string_lossy().to_string();
        let is_dir = entry.is_directory;

        if let Some(ref tex_info) = entry.texture_info {
            // Image file: render as thumbnail tile
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
                    ChildOf(content_entity),
                ))
                .id();

            if let Some(ref img) = tex_info.image_handle {
                // 2D texture thumbnail
                commands.spawn((
                    ImageNode::new(img.clone()),
                    Node {
                        width: Val::Px(56.0),
                        height: Val::Px(56.0),
                        ..Default::default()
                    },
                    ChildOf(thumb_entity),
                ));
            } else {
                // Non-2D: gray placeholder with badge
                let placeholder = commands
                    .spawn((
                        Node {
                            width: Val::Px(56.0),
                            height: Val::Px(56.0),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            ..Default::default()
                        },
                        BackgroundColor(Color::srgb(0.25, 0.25, 0.25)),
                        ChildOf(thumb_entity),
                    ))
                    .id();

                let badge_text = if tex_info.is_cubemap {
                    "Cubemap".to_string()
                } else {
                    format!("{} layers", tex_info.layer_count)
                };

                commands.spawn((
                    Text::new(badge_text),
                    TextFont {
                        font_size: 8.0,
                        ..Default::default()
                    },
                    TextColor(Color::srgb(0.8, 0.8, 0.8)),
                    ChildOf(placeholder),
                ));
            }

            // File name label
            let is_truncated = entry.file_name.len() > 10;
            let display_name = if is_truncated {
                format!("{}...", &entry.file_name[..8])
            } else {
                entry.file_name.clone()
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
                attach_tooltip(&mut commands, name_entity, entry.file_name.clone());
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

            // Click: 2D textures → apply directly; non-2D → select for preview
            let tex_info_clone = tex_info.clone();
            let entry_path = entry.path.clone();
            let click_path = path_for_click.clone();
            commands.entity(thumb_entity).observe(
                move |_: On<Pointer<Click>>, mut commands: Commands| {
                    if tex_info_clone.is_cubemap || tex_info_clone.is_array {
                        commands.trigger(SelectAssetPreview {
                            path: entry_path.clone(),
                            info: tex_info_clone.clone(),
                        });
                    } else {
                        // 2D texture: apply to faces
                        if let Some(relative) = to_asset_relative_path(&click_path) {
                            commands.trigger(ApplyTextureToFaces { path: relative });
                        }
                    }
                },
            );
        } else {
            // Non-image file or directory: use standard file browser item
            let item = FileBrowserItem {
                path: entry.path.to_string_lossy().to_string(),
                is_directory: entry.is_directory,
                file_name: entry.file_name.clone(),
            };

            let item_entity = match state.view_mode {
                BrowserViewMode::Grid => commands
                    .spawn((
                        file_browser::file_browser_item(&item, &icon_font),
                        ChildOf(content_entity),
                    ))
                    .id(),
                BrowserViewMode::List => commands
                    .spawn((
                        file_browser::file_browser_list_item(&item, &icon_font),
                        ChildOf(content_entity),
                    ))
                    .id(),
            };

            commands.entity(item_entity).observe(
                |hover: On<Pointer<Over>>, mut bg: Query<&mut BackgroundColor>| {
                    if let Ok(mut bg) = bg.get_mut(hover.event_target()) {
                        bg.0 = tokens::HOVER_BG;
                    }
                },
            );
            commands.entity(item_entity).observe(
                |out: On<Pointer<Out>>, mut bg: Query<&mut BackgroundColor>| {
                    if let Ok(mut bg) = bg.get_mut(out.event_target()) {
                        bg.0 = Color::NONE;
                    }
                },
            );
            commands.entity(item_entity).observe(
                move |_: On<Pointer<Click>>, mut commands: Commands| {
                    commands.trigger(FileItemDoubleClicked {
                        path: path_for_click.clone(),
                        is_directory: is_dir,
                    });
                },
            );
        }
    }

    // Update breadcrumb
    let Ok((breadcrumb_entity, breadcrumb_children)) = breadcrumb_query.single() else {
        return;
    };
    if let Some(children) = breadcrumb_children {
        for child in children.iter() {
            commands.entity(child).despawn();
        }
    }

    let relative = state
        .current_directory
        .strip_prefix(&state.root_directory)
        .unwrap_or(&state.current_directory);
    let path_str = relative.to_string_lossy().to_string();

    commands.spawn((
        Text::new(if path_str.is_empty() {
            "/".to_string()
        } else {
            format!("/ {}", path_str.replace(std::path::MAIN_SEPARATOR, " / "))
        }),
        TextFont {
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        ThemedText,
        ChildOf(breadcrumb_entity),
    ));

    for mut text in root_label_query.iter_mut() {
        **text = state.root_directory.to_string_lossy().to_string();
    }
}

fn load_thumbnail(
    path: &Path,
    asset_root: &Path,
    asset_server: &AssetServer,
    images: &mut Assets<Image>,
    pending: &mut Vec<(PathBuf, Handle<Image>)>,
) -> Option<Handle<Image>> {
    if let Ok(relative) = path.strip_prefix(asset_root) {
        let relative = relative.to_string_lossy().replace('\\', "/");
        Some(asset_server.load(relative))
    } else {
        let handle = images.add(Image::default());
        pending.push((path.to_path_buf(), handle.clone()));
        Some(handle)
    }
}

fn load_pending_thumbnails(
    mut pending: ResMut<PendingThumbnailLoads>,
    mut images: ResMut<Assets<Image>>,
) {
    if pending.0.is_empty() {
        return;
    }
    for _ in 0..2 {
        let Some((path, handle)) = pending.0.pop() else {
            return;
        };
        if let Some(loaded) = decode_image_from_disk(&path) {
            if let Some(img) = images.get_mut(handle.id()) {
                *img = loaded;
            }
        }
    }
}

fn handle_file_double_click(
    event: On<FileItemDoubleClicked>,
    mut state: ResMut<AssetBrowserState>,
    mut commands: Commands,
) {
    if event.is_directory {
        state.current_directory = PathBuf::from(&event.path);
        state.needs_refresh = true;
        return;
    }

    if is_image_file(&event.path) {
        // Only apply 2D textures on double-click
        let p = Path::new(&event.path);
        let is_non_2d = p
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("ktx2"))
            && is_ktx2_non_2d(p);
        if !is_non_2d {
            if let Some(relative) = to_asset_relative_path(&event.path) {
                commands.trigger(ApplyTextureToFaces { path: relative });
            }
        }
    }
}

fn handle_apply_texture(
    event: On<ApplyTextureToFaces>,
    brush_selection: Res<BrushSelection>,
    edit_mode: Res<EditMode>,
    selection: Res<Selection>,
    mut brushes: Query<&mut Brush>,
    mut history: ResMut<CommandHistory>,
    mut last_texture: ResMut<LastUsedTexture>,
) {
    if *edit_mode == EditMode::BrushEdit(BrushEditMode::Face) && !brush_selection.faces.is_empty() {
        if let Some(entity) = brush_selection.entity {
            if let Ok(mut brush) = brushes.get_mut(entity) {
                let old = brush.clone();
                for &face_idx in &brush_selection.faces {
                    if face_idx < brush.faces.len() {
                        brush.faces[face_idx].texture_path = Some(event.path.clone());
                    }
                }
                let cmd = SetBrush {
                    entity,
                    old,
                    new: brush.clone(),
                    label: "Apply texture".into(),
                };
                history.undo_stack.push(Box::new(cmd));
                history.redo_stack.clear();
            }
        }
    } else {
        for &entity in &selection.entities {
            if let Ok(mut brush) = brushes.get_mut(entity) {
                let old = brush.clone();
                for face in brush.faces.iter_mut() {
                    face.texture_path = Some(event.path.clone());
                }
                let cmd = SetBrush {
                    entity,
                    old,
                    new: brush.clone(),
                    label: "Apply texture".into(),
                };
                history.undo_stack.push(Box::new(cmd));
                history.redo_stack.clear();
            }
        }
    }

    last_texture.texture_path = Some(event.path.clone());
}

fn handle_select_asset_preview(
    event: On<SelectAssetPreview>,
    mut preview_state: ResMut<AssetPreviewState>,
) {
    if preview_state.selected_path.as_ref() == Some(&event.path) {
        // Toggle off
        preview_state.selected_path = None;
        preview_state.selected_info = None;
        preview_state.layer_images.clear();
        preview_state.current_layer = 0;
    } else {
        preview_state.selected_path = Some(event.path.clone());
        preview_state.selected_info = Some(event.info.clone());
        preview_state.current_layer = 0;
        preview_state.layer_images.clear();
    }
}

pub fn attach_tooltip(commands: &mut Commands, entity: Entity, text: String) {
    commands.entity(entity).observe(
        move |trigger: On<Pointer<Over>>,
              mut commands: Commands,
              mut tooltip: ResMut<ActiveTooltip>| {
            if let Some(old) = tooltip.0.take() {
                commands.entity(old).try_despawn();
            }
            let anchor = trigger.event_target();
            let tip = commands
                .spawn(popover::popover(
                    popover::PopoverProps::new(anchor)
                        .with_placement(popover::PopoverPlacement::Bottom)
                        .with_padding(4.0)
                        .with_z_index(300),
                ))
                .id();
            commands.spawn((
                Text::new(text.clone()),
                TextFont {
                    font_size: tokens::FONT_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_PRIMARY),
                ChildOf(tip),
            ));
            tooltip.0 = Some(tip);
        },
    );
    commands.entity(entity).observe(
        |_: On<Pointer<Out>>, mut commands: Commands, mut tooltip: ResMut<ActiveTooltip>| {
            if let Some(old) = tooltip.0.take() {
                commands.entity(old).try_despawn();
            }
        },
    );
}

fn extract_array_layers(
    mut preview_state: ResMut<AssetPreviewState>,
    mut images: ResMut<Assets<Image>>,
) {
    let dominated = preview_state
        .selected_info
        .as_ref()
        .is_some_and(|i| i.is_array)
        && preview_state.layer_images.is_empty()
        && preview_state.selected_path.is_some();
    if !dominated {
        return;
    }

    let path = preview_state.selected_path.as_ref().unwrap();
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("ktx2");
    let Ok(image) = Image::from_buffer(
        &bytes,
        ImageType::Extension(ext),
        CompressedImageFormats::all(),
        true,
        ImageSampler::default(),
        RenderAssetUsages::default(),
    ) else {
        return;
    };

    let layer_count = preview_state.selected_info.as_ref().unwrap().layer_count;
    let Some(ref data) = image.data else {
        return;
    };
    let total_size = data.len();
    let layer_size = total_size / layer_count as usize;

    if layer_size == 0 || total_size % layer_count as usize != 0 {
        return;
    }

    let desc = &image.texture_descriptor;
    for i in 0..layer_count {
        let start = i as usize * layer_size;
        let end = start + layer_size;
        let mut layer_img = Image::new(
            Extent3d {
                width: desc.size.width,
                height: desc.size.height,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            data[start..end].to_vec(),
            desc.format,
            image.asset_usage,
        );
        layer_img.sampler = image.sampler.clone();
        preview_state.layer_images.push(images.add(layer_img));
    }
}

fn update_preview_panel(
    mut commands: Commands,
    preview_state: Res<AssetPreviewState>,
    container_query: Query<(Entity, Option<&Children>), With<PreviewPanelContainer>>,
) {
    if !preview_state.is_changed() {
        return;
    }

    let Ok((container, children)) = container_query.single() else {
        return;
    };

    if let Some(children) = children {
        for child in children.iter() {
            commands.entity(child).despawn();
        }
    }

    let Some(ref path) = preview_state.selected_path else {
        return;
    };
    let Some(ref info) = preview_state.selected_info else {
        return;
    };

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    // Preview image (for 2D textures or if we have layer images)
    if let Some(ref img) = info.image_handle {
        commands.spawn((
            ImageNode::new(img.clone()),
            Node {
                width: Val::Px(128.0),
                height: Val::Px(128.0),
                align_self: AlignSelf::Center,
                ..Default::default()
            },
            ChildOf(container),
        ));
    } else if !preview_state.layer_images.is_empty() {
        let idx = (preview_state.current_layer as usize).min(preview_state.layer_images.len() - 1);
        commands.spawn((
            ImageNode::new(preview_state.layer_images[idx].clone()),
            Node {
                width: Val::Px(128.0),
                height: Val::Px(128.0),
                align_self: AlignSelf::Center,
                ..Default::default()
            },
            ChildOf(container),
        ));
    }

    // Filename
    commands.spawn((
        Text::new(file_name),
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

    // Type info
    let type_text = if info.is_cubemap {
        format!("Cubemap ({} faces)", info.face_count)
    } else if info.is_array {
        format!("{} layers", info.layer_count)
    } else {
        "2D Texture".to_string()
    };
    commands.spawn((
        Text::new(type_text),
        TextFont {
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Node {
            align_self: AlignSelf::Center,
            ..Default::default()
        },
        ChildOf(container),
    ));

    // Layer cycling buttons for arrays
    if info.is_array && !preview_state.layer_images.is_empty() {
        let layer_text = format!(
            "Layer {} of {}",
            preview_state.current_layer + 1,
            info.layer_count
        );

        let nav_row = commands
            .spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    align_self: AlignSelf::Center,
                    column_gap: Val::Px(tokens::SPACING_SM),
                    margin: UiRect::top(Val::Px(tokens::SPACING_XS)),
                    ..Default::default()
                },
                ChildOf(container),
            ))
            .id();

        // Previous button
        let prev_btn = commands
            .spawn((
                Node {
                    padding: UiRect::axes(Val::Px(tokens::SPACING_SM), Val::Px(2.0)),
                    border_radius: BorderRadius::all(Val::Px(3.0)),
                    ..Default::default()
                },
                BackgroundColor(tokens::INPUT_BG),
                ChildOf(nav_row),
            ))
            .id();
        commands.spawn((
            Text::new("<"),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_PRIMARY),
            ChildOf(prev_btn),
        ));
        let layer_count = info.layer_count;
        commands.entity(prev_btn).observe(
            move |_: On<Pointer<Click>>, mut ps: ResMut<AssetPreviewState>| {
                if ps.current_layer > 0 {
                    ps.current_layer -= 1;
                } else {
                    ps.current_layer = layer_count.saturating_sub(1);
                }
            },
        );

        commands.spawn((
            Text::new(layer_text),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(nav_row),
        ));

        // Next button
        let next_btn = commands
            .spawn((
                Node {
                    padding: UiRect::axes(Val::Px(tokens::SPACING_SM), Val::Px(2.0)),
                    border_radius: BorderRadius::all(Val::Px(3.0)),
                    ..Default::default()
                },
                BackgroundColor(tokens::INPUT_BG),
                ChildOf(nav_row),
            ))
            .id();
        commands.spawn((
            Text::new(">"),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_PRIMARY),
            ChildOf(next_btn),
        ));
        let layer_count2 = info.layer_count;
        commands.entity(next_btn).observe(
            move |_: On<Pointer<Click>>, mut ps: ResMut<AssetPreviewState>| {
                ps.current_layer = (ps.current_layer + 1) % layer_count2;
            },
        );
    }

    // Apply button (only for 2D textures)
    if !info.is_cubemap && !info.is_array {
        let path_str = path.to_string_lossy().to_string();
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
        commands.entity(apply_btn).observe(
            move |_: On<Pointer<Click>>, mut commands: Commands| {
                if let Some(relative) = to_asset_relative_path(&path_str) {
                    commands.trigger(ApplyTextureToFaces { path: relative });
                }
            },
        );
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
    }
}

fn spawn_asset_folder_dialog(
    _: On<Pointer<Click>>,
    mut commands: Commands,
    raw_handle: Query<&RawHandleWrapper, With<PrimaryWindow>>,
) {
    let mut dialog = AsyncFileDialog::new().set_title("Select assets directory");
    if let Ok(rh) = raw_handle.single() {
        let handle = unsafe { rh.get_handle() };
        dialog = dialog.set_parent(&handle);
    }
    let task = AsyncComputeTaskPool::get().spawn(async move { dialog.pick_folder().await });
    commands.insert_resource(AssetBrowserFolderTask(task));
}

fn poll_asset_browser_folder(world: &mut World) {
    let Some(mut task_res) = world.get_resource_mut::<AssetBrowserFolderTask>() else {
        return;
    };
    let Some(result) = future::block_on(future::poll_once(&mut task_res.0)) else {
        return;
    };
    world.remove_resource::<AssetBrowserFolderTask>();

    if let Some(handle) = result {
        let path = handle.path().to_path_buf();
        let mut state = world.resource_mut::<AssetBrowserState>();
        state.root_directory = path.clone();
        state.current_directory = path.clone();
        state.needs_refresh = true;

        let mut label_query = world.query_filtered::<&mut Text, With<AssetBrowserRootLabel>>();
        for mut text in label_query.iter_mut(world) {
            **text = path.to_string_lossy().to_string();
        }
    }
}

// ── Panel layout ────────────────────────────────────────────────────────────

pub fn asset_browser_panel(icon_font: Handle<Font>) -> impl Bundle {
    let folder_icon_font = icon_font;
    (
        AssetBrowserPanel,
        EditorEntity,
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            padding: UiRect::all(Val::Px(tokens::SPACING_SM)),
            ..Default::default()
        },
        BackgroundColor(tokens::PANEL_BG),
        children![
            // Root directory header
            (
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::SpaceBetween,
                    width: Val::Percent(100.0),
                    height: Val::Px(tokens::ROW_HEIGHT),
                    padding: UiRect::horizontal(Val::Px(tokens::SPACING_MD)),
                    flex_shrink: 0.0,
                    ..Default::default()
                },
                BackgroundColor(tokens::PANEL_HEADER_BG),
                children![
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
                                Text::new("Assets"),
                                TextFont {
                                    font_size: tokens::FONT_MD,
                                    ..Default::default()
                                },
                                ThemedText,
                            ),
                            (
                                AssetBrowserRootLabel,
                                Text::new(""),
                                TextFont {
                                    font_size: tokens::FONT_SM,
                                    ..Default::default()
                                },
                                TextColor(tokens::TEXT_SECONDARY),
                            ),
                        ],
                    ),
                    asset_folder_button(folder_icon_font),
                ],
            ),
            // Breadcrumb bar
            (
                AssetBrowserBreadcrumb,
                EditorEntity,
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    padding: UiRect::axes(Val::Px(tokens::SPACING_MD), Val::Px(tokens::SPACING_SM)),
                    width: Val::Percent(100.0),
                    height: Val::Px(tokens::HEADER_HEIGHT),
                    flex_shrink: 0.0,
                    ..Default::default()
                },
                BackgroundColor(tokens::TOOLBAR_BG),
            ),
            // Main row: content grid + preview panel
            (
                EditorEntity,
                Node {
                    flex_direction: FlexDirection::Row,
                    width: Val::Percent(100.0),
                    flex_grow: 1.0,
                    min_height: Val::Px(0.0),
                    ..Default::default()
                },
                children![
                    // Content area (grid of files)
                    (
                        AssetBrowserContent,
                        EditorEntity,
                        Node {
                            flex_direction: FlexDirection::Row,
                            flex_wrap: FlexWrap::Wrap,
                            align_content: AlignContent::FlexStart,
                            flex_grow: 1.0,
                            min_width: Val::Px(0.0),
                            min_height: Val::Px(0.0),
                            overflow: Overflow::scroll_y(),
                            padding: UiRect::all(Val::Px(tokens::SPACING_SM)),
                            row_gap: Val::Px(tokens::SPACING_XS),
                            column_gap: Val::Px(tokens::SPACING_XS),
                            ..Default::default()
                        },
                    ),
                    // Preview panel (right side, populated dynamically)
                    (
                        PreviewPanelContainer,
                        EditorEntity,
                        Node {
                            flex_direction: FlexDirection::Column,
                            width: Val::Px(160.0),
                            flex_shrink: 0.0,
                            padding: UiRect::all(Val::Px(tokens::SPACING_SM)),
                            border: UiRect::left(Val::Px(1.0)),
                            overflow: Overflow::scroll_y(),
                            ..Default::default()
                        },
                        BorderColor::all(tokens::PANEL_HEADER_BG),
                    ),
                ],
            )
        ],
    )
}

fn asset_folder_button(icon_font: Handle<Font>) -> impl Bundle {
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
        observe(spawn_asset_folder_dialog),
    )
}
