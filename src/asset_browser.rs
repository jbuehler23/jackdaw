use std::path::{Path, PathBuf};
use std::sync::{Mutex, mpsc};

use bevy::{
    asset::RenderAssetUsages,
    feathers::cursor::{EntityCursor, OverrideCursor},
    feathers::theme::ThemedText,
    image::{CompressedImageFormats, ImageSampler, ImageType},
    picking::hover::Hovered,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureSampleType},
    tasks::{AsyncComputeTaskPool, Task, futures_lite::future},
    window::{PrimaryWindow, RawHandleWrapper, SystemCursorIcon},
};
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_feathers::text_edit::TextEditValue;
use jackdaw_feathers::tooltip::Tooltip;
use jackdaw_feathers::{file_browser, icons, icons::EditorFont, icons::IconFont, tokens};
use jackdaw_widgets::file_browser::{FileBrowserItem, FileItemDoubleClicked};
use rfd::AsyncFileDialog;

use crate::{
    EditorEntity,
    brush::{Brush, BrushEditMode, BrushSelection, EditMode, LastUsedMaterial},
    material_browser::MaterialRegistry,
    model_thumbnail::ModelThumbnailSlot,
    prelude::*,
    selection::Selection,
};

/// Returns true if the KTX2 file is NOT a simple 2D texture (cubemap or array texture).
pub fn is_ktx2_non_2d(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    use std::io::Read;
    let mut header = [0u8; 40];
    if file.read_exact(&mut header).is_err() {
        return false;
    }
    let pixel_depth = u32::from_le_bytes([header[28], header[29], header[30], header[31]]);
    let layer_count = u32::from_le_bytes([header[32], header[33], header[34], header[35]]);
    let face_count = u32::from_le_bytes([header[36], header[37], header[38], header[39]]);
    pixel_depth > 0 || layer_count > 1 || face_count > 1
}

/// Watches the asset root directory for filesystem changes using the `notify` crate.
#[derive(Resource)]
struct DirectoryWatcher {
    _watcher: notify::RecommendedWatcher,
    receiver: Mutex<mpsc::Receiver<()>>,
}

fn setup_directory_watcher(root: &Path, commands: &mut Commands) {
    let (tx, rx) = mpsc::channel();
    let watcher = notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
        if let Ok(event) = res {
            use notify::EventKind;
            if matches!(
                event.kind,
                EventKind::Create(_)
                    | EventKind::Remove(_)
                    | EventKind::Modify(notify::event::ModifyKind::Name(_))
            ) {
                let _ = tx.send(());
            }
        }
    });
    match watcher {
        Ok(mut w) => {
            use notify::Watcher;
            if w.watch(root, notify::RecursiveMode::Recursive).is_ok() {
                commands.insert_resource(DirectoryWatcher {
                    _watcher: w,
                    receiver: Mutex::new(rx),
                });
            } else {
                warn!("Failed to watch directory: {:?}", root);
            }
        }
        Err(e) => {
            warn!("Failed to create directory watcher: {}", e);
        }
    }
}

/// Path of the asset currently being dragged from the asset browser. Set by a
/// `Pointer<DragStart>` observer attached to each prefab entry, read by the
/// viewport's drop handler, and cleared after the drop (or by `DragEnd` if no
/// drop happened). Resource-based so the consumer doesn't need to dig the
/// payload out of pointer event metadata.
#[derive(Resource, Default)]
pub struct ActiveAssetDrag {
    pub path: Option<PathBuf>,
    /// Set when the drag started on a 2D image thumbnail. Dropping it
    /// on the viewport spawns a reference image plane at the drop
    /// point; image thumbnails aren't `FileBrowserItem` rows, so the
    /// drop handler can't recover the path from the dropped entity.
    pub image: Option<PathBuf>,
    /// The floating drag-ghost entity that follows the cursor while a drag
    /// is in flight. Spawned and despawned by `manage_asset_drag_ghost`,
    /// keyed off `path`/`image`.
    pub ghost: Option<Entity>,
}

/// Marker on the floating drag-ghost node (a dimmed icon + label that
/// follows the cursor during an asset-browser drag).
#[derive(Component)]
struct AssetDragGhost;

pub struct AssetBrowserPlugin;

impl Plugin for AssetBrowserPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AssetBrowserState>()
            .init_resource::<AssetPreviewState>()
            .init_resource::<ActiveAssetDrag>()
            .add_systems(OnEnter(crate::AppState::Editor), setup_initial_directory)
            .add_systems(
                Update,
                (
                    refresh_browser_on_change,
                    poll_asset_browser_folder,
                    extract_array_layers,
                    update_preview_panel,
                    check_watcher_events,
                    remove_incompatible_image_nodes,
                    update_asset_browser_filter,
                    toggle_prefabs_only_chip,
                    update_prefabs_only_chip_style,
                    manage_asset_drag_ghost,
                )
                    .run_if(in_state(crate::AppState::Editor)),
            )
            .add_observer(handle_file_double_click)
            .add_observer(handle_select_asset_preview)
            .add_observer(on_asset_browser_context_action);
    }
}

/// Drive the drag-ghost and the grab cursor from the current
/// [`ActiveAssetDrag`]: while a drag is live, force a "grabbing" cursor and
/// float a dimmed icon + filename card under the pointer; tear both down
/// when the drag ends. One system, no per-entry observers.
fn manage_asset_drag_ghost(
    mut commands: Commands,
    mut drag: ResMut<ActiveAssetDrag>,
    mut cursor: ResMut<OverrideCursor>,
    windows: Query<&Window, With<PrimaryWindow>>,
    editor_font: Res<EditorFont>,
    icon_font: Res<IconFont>,
    mut ghost_nodes: Query<&mut Node, With<AssetDragGhost>>,
) {
    let active = drag.path.as_ref().or(drag.image.as_ref()).cloned();
    let cursor_pos = windows
        .single()
        .ok()
        .and_then(bevy::prelude::Window::cursor_position);

    // Grab cursor: set while dragging (without clobbering another tool's
    // override); clear our own when the drag ends.
    let grabbing = Some(EntityCursor::System(SystemCursorIcon::Grabbing));
    if active.is_some() {
        if cursor.0.is_none() {
            cursor.0 = grabbing;
        }
    } else if cursor.0 == grabbing {
        cursor.0 = None;
    }

    match (active, drag.ghost) {
        (Some(path), None) => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let icon = file_browser::file_icon(&name);
            let pos = cursor_pos.unwrap_or(Vec2::ZERO);
            let ghost = spawn_asset_drag_ghost(
                &mut commands,
                &editor_font.0,
                &icon_font.0,
                icon,
                &name,
                pos,
            );
            drag.ghost = Some(ghost);
        }
        (Some(_), Some(ghost)) => {
            if let Some(pos) = cursor_pos
                && let Ok(mut node) = ghost_nodes.get_mut(ghost)
            {
                node.left = Val::Px(pos.x + 14.0);
                node.top = Val::Px(pos.y + 8.0);
            }
        }
        (None, Some(ghost)) => {
            commands.entity(ghost).try_despawn();
            drag.ghost = None;
        }
        (None, None) => {}
    }
}

/// Spawn the dimmed drag-ghost card at `pos`. `Pickable::IGNORE` so it never
/// intercepts the drop target under the cursor.
fn spawn_asset_drag_ghost(
    commands: &mut Commands,
    font: &Handle<Font>,
    icon_font: &Handle<Font>,
    icon: icons::Icon,
    name: &str,
    pos: Vec2,
) -> Entity {
    commands
        .spawn((
            AssetDragGhost,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(pos.x + 14.0),
                top: Val::Px(pos.y + 8.0),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(6.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(5.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                max_width: Val::Px(220.0),
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(tokens::PANEL_BG.with_alpha(0.85)),
            BorderColor::all(tokens::BORDER_SUBTLE.with_alpha(0.8)),
            GlobalZIndex(10_000),
            Pickable::IGNORE,
            children![
                (
                    Text::new(String::from(icon.unicode())),
                    TextFont {
                        font: icon_font.clone().into(),
                        font_size: tokens::ICON_SM,
                        ..default()
                    },
                    TextColor(tokens::TEXT_SECONDARY.with_alpha(0.75)),
                    Pickable::IGNORE,
                ),
                (
                    Text::new(name.to_string()),
                    TextFont {
                        font: font.clone().into(),
                        font_size: tokens::TEXT_SIZE_SM,
                        ..default()
                    },
                    TextColor(tokens::TEXT_PRIMARY.with_alpha(0.75)),
                    Pickable::IGNORE,
                ),
            ],
        ))
        .id()
}

fn on_asset_browser_context_action(
    event: On<jackdaw_widgets::context_menu::ContextMenuAction>,
    mut commands: Commands,
    state: Res<AssetBrowserState>,
    mut menu_state: ResMut<jackdaw_widgets::context_menu::ContextMenuState>,
) {
    if event.action != "asset_browser.delete" {
        return;
    }
    let Some(path) = state.selected_file.clone() else {
        return;
    };
    commands.operator("file.delete").param("path", path).call();
    if let Some(menu) = menu_state.menu_entity.take()
        && let Ok(mut ec) = commands.get_entity(menu)
    {
        ec.despawn();
    }
}

// -- Texture info ------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct TextureInfo {
    pub image_handle: Option<Handle<Image>>,
    pub is_cubemap: bool,
    pub is_array: bool,
    pub layer_count: u32,
    pub face_count: u32,
}

// -- Browser state -----------------------------------------------------------

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
    /// Currently selected file path (shown in breadcrumb, highlighted in grid).
    pub selected_file: Option<String>,
    /// Timestamp of last click for double-click detection.
    pub last_click_time: f64,
    /// When true, only scene files that contain a `Prefab` component are
    /// shown in the grid.
    pub prefabs_only: bool,
    /// Per-path memo of whether a scene file is a prefab. Keyed by
    /// absolute path, valued by `(mtime, is_prefab)`; invalidated when
    /// the file's mtime changes.
    prefab_cache: PrefabCheckCache,
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
            selected_file: None,
            last_click_time: 0.0,
            prefabs_only: false,
            prefab_cache: PrefabCheckCache::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct DirEntry {
    pub path: PathBuf,
    pub file_name: String,
    pub is_directory: bool,
    pub texture_info: Option<TextureInfo>,
    pub is_prefab: bool,
}

/// Returns true if `path` is a prefab: a scene document whose root entity
/// carries a `jackdaw::prefab::components::Prefab` component.
///
/// Both formats the editor can hold a prefab in are recognised. `.bsn` is
/// what the editor writes today (`crate::prefab::operators::write_prefab_doc`
/// redirects even a `.jsn` target to a `.bsn` file); `.jsn` is the legacy
/// form. Anything else, and any I/O or parse error, is not a prefab.
fn read_is_prefab(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("bsn") => read_is_bsn_prefab(path),
        Some(ext) if ext.eq_ignore_ascii_case("jsn") => read_is_jsn_prefab(path),
        _ => false,
    }
}

/// A `.bsn` document is a prefab when one of its roots carries the `Prefab`
/// marker.
///
/// Deliberately *not* routed through `crate::prefab::save_load::read_prefab_ast`:
/// that calls `normalize_as_prefab_source`, which wraps any plain scene into
/// an instanceable prefab, so every `.bsn` in the project would answer yes.
/// Detection has to see the file as authored.
fn read_is_bsn_prefab(path: &Path) -> bool {
    use crate::prefab::resolver_bsn::PREFAB_TYPE;
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    // Cheap reject before the parser runs: the marker's type path appears
    // verbatim in BSN text, so a document that never mentions it cannot be a
    // prefab. Worth having when a directory holds hundreds of scenes.
    if !text.contains(PREFAB_TYPE) {
        return false;
    }
    let Ok(ast) = jackdaw_bsn::parse_bsn_text(&text) else {
        return false;
    };
    ast.roots
        .iter()
        .any(|&root| ast.find_patch_by_type_path(root, PREFAB_TYPE).is_some())
}

/// A legacy `.jsn` document is a prefab when its first scene entity carries
/// the `Prefab` component. Parsed as plain JSON.
fn read_is_jsn_prefab(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    value
        .get("scene")
        .and_then(|v| v.get(0))
        .and_then(|e| e.get("components"))
        .and_then(|c| c.get(crate::prefab::resolver_bsn::PREFAB_TYPE))
        .is_some()
}

#[derive(Default)]
struct PrefabCheckCache {
    entries: std::collections::HashMap<PathBuf, (std::time::SystemTime, bool)>,
}

impl PrefabCheckCache {
    fn check(&mut self, path: &Path) -> bool {
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        if let Some(mtime) = mtime
            && let Some((cached_mtime, cached)) = self.entries.get(path)
            && *cached_mtime == mtime
        {
            return *cached;
        }
        let is_prefab = read_is_prefab(path);
        if let Some(mtime) = mtime {
            self.entries.insert(path.to_path_buf(), (mtime, is_prefab));
        }
        is_prefab
    }
}

// -- Preview state -----------------------------------------------------------

#[derive(Resource, Default)]
pub struct AssetPreviewState {
    pub selected_path: Option<PathBuf>,
    pub selected_info: Option<TextureInfo>,
    pub current_layer: u32,
    pub layer_images: Vec<Handle<Image>>,
}

#[derive(Event, Debug, Clone)]
struct SelectAssetPreview {
    path: PathBuf,
    info: TextureInfo,
}

// -- Components --------------------------------------------------------------

#[derive(Component)]
pub struct AssetBrowserPanel;

#[derive(Component)]
pub struct AssetBrowserContent;

#[derive(Component)]
pub struct AssetBrowserBreadcrumb;

#[derive(Component)]
pub struct AssetBrowserFilter;

#[derive(Component)]
struct PrefabsOnlyChip;

#[derive(Component)]
struct PreviewPanelContainer;

#[derive(Resource)]
struct AssetBrowserFolderTask(Task<Option<rfd::FileHandle>>);

// -- Helpers (absorbed from texture_browser) ---------------------------------

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

// -- Systems -----------------------------------------------------------------

fn setup_initial_directory(
    mut state: ResMut<AssetBrowserState>,
    mut commands: Commands,
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

    setup_directory_watcher(&state.root_directory, &mut commands);
}

fn refresh_browser_on_change(
    mut state: ResMut<AssetBrowserState>,
    mut commands: Commands,
    icon_font: Res<IconFont>,
    asset_server: Res<AssetServer>,
    content_query: Query<(Entity, Option<&Children>), With<AssetBrowserContent>>,
    breadcrumb_query: Query<(Entity, Option<&Children>), With<AssetBrowserBreadcrumb>>,
) {
    if !state.needs_refresh {
        return;
    }
    state.needs_refresh = false;

    // Scan directory
    state.entries.clear();
    let filter = state.filter.clone();
    if let Ok(read_dir) = std::fs::read_dir(&state.current_directory) {
        let mut entries: Vec<DirEntry> = read_dir
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let file_name = entry.file_name().to_string_lossy().to_string();
                if file_name.starts_with('.') {
                    return None;
                }
                if !filter.is_empty() && !file_name.to_lowercase().contains(&filter.to_lowercase())
                {
                    return None;
                }
                let path = entry.path();
                // `DirEntry::file_type` reports the link itself, so a
                // symlinked directory used to classify as a file and render
                // as an unopenable row. Ask the path, which follows the
                // link: symlinking a shared art kit into a project is a
                // normal way to avoid copying gigabytes of models.
                let is_directory = path.is_dir();

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
                                load_thumbnail(&path, &asset_server)
                            },
                            is_cubemap: face_count > 1,
                            is_array: layer_count > 1,
                            layer_count,
                            face_count,
                        })
                    } else {
                        Some(TextureInfo {
                            image_handle: load_thumbnail(&path, &asset_server),
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
                    is_prefab: false,
                })
            })
            .collect();

        // Compute is_prefab for scene documents (cached by mtime), then
        // apply the prefabs_only filter if it's enabled.
        for entry in entries.iter_mut() {
            if !entry.is_directory
                && entry
                    .path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("jsn") || e.eq_ignore_ascii_case("bsn"))
            {
                entry.is_prefab = state.prefab_cache.check(&entry.path);
            }
        }
        if state.prefabs_only {
            entries.retain(|e| e.is_directory || e.is_prefab);
        }

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
                        width: Val::Px(tokens::THUMB_CELL_WIDTH),
                        height: Val::Px(tokens::THUMB_CELL_HEIGHT),
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
                        width: Val::Px(tokens::THUMB_IMAGE_SIZE),
                        height: Val::Px(tokens::THUMB_IMAGE_SIZE),
                        ..Default::default()
                    },
                    ChildOf(thumb_entity),
                ));
            } else {
                // Non-2D: gray placeholder with badge
                let placeholder = commands
                    .spawn((
                        Node {
                            width: Val::Px(tokens::THUMB_IMAGE_SIZE),
                            height: Val::Px(tokens::THUMB_IMAGE_SIZE),
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
                        font_size: tokens::TEXT_SIZE_XS,
                        ..Default::default()
                    },
                    TextColor(Color::srgb(0.8, 0.8, 0.8)),
                    ChildOf(placeholder),
                ));
            }

            // File name label. Truncates inline when it overflows the
            // cell; when truncated, attach a generic `Tooltip` so the
            // user can hover to read the full name. Direct attach
            // (no source-component bridge); the data is already in
            // hand at the call site.
            let is_truncated = entry.file_name.len() > 10;
            let display_name = if is_truncated {
                format!("{}...", &entry.file_name[..8])
            } else {
                entry.file_name.clone()
            };
            let mut name_label = commands.spawn((
                Text::new(display_name),
                TextFont {
                    font_size: tokens::TEXT_SIZE_XS,
                    ..default()
                },
                TextColor(tokens::TEXT_SECONDARY),
                Node {
                    max_width: px(tokens::THUMB_NAME_MAX_WIDTH),
                    overflow: Overflow::clip(),
                    ..default()
                },
                ChildOf(thumb_entity),
            ));
            if is_truncated {
                name_label.insert((Hovered::default(), Tooltip::title(entry.file_name.clone())));
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

            // Click: 2D textures -> apply directly; non-2D -> select for preview
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
                        // 2D texture: apply to faces via operator.
                        // User-facing click so history opt-in is
                        // explicit (the default is `false`).
                        commands
                            .operator("material.apply_texture")
                            .param("path", click_path.clone())
                            .settings(CallOperatorSettings {
                                creates_history_entry: true,
                                ..default()
                            })
                            .call();
                    }
                },
            );

            // 2D textures: track drag start so the viewport's drop
            // handler can spawn a reference image plane at the drop
            // point. Clear on DragEnd if nothing consumed it.
            if !tex_info.is_cubemap && !tex_info.is_array {
                let drag_path = entry.path.clone();
                commands.entity(thumb_entity).observe(
                    move |_: On<Pointer<DragStart>>, mut drag: ResMut<ActiveAssetDrag>| {
                        drag.image = Some(drag_path.clone());
                    },
                );
                commands.entity(thumb_entity).observe(
                    |_: On<Pointer<DragEnd>>, mut drag: ResMut<ActiveAssetDrag>| {
                        drag.image = None;
                    },
                );
            }
        } else {
            // Non-image file or directory: use standard file browser item
            let item = FileBrowserItem {
                path: entry.path.to_string_lossy().to_string(),
                is_directory: entry.is_directory,
                file_name: entry.file_name.clone(),
            };

            let icon_override = if entry.is_prefab {
                Some(icons::Icon::Package)
            } else {
                None
            };

            let item_entity = match state.view_mode {
                // A `.glb`/`.gltf` grid tile gets a thumbnail slot in place
                // of the plain glyph. List mode keeps the glyph: a 16px row
                // is too small for a rendered model to say anything.
                BrowserViewMode::Grid
                    if crate::model_thumbnail::is_model_path(&entry.path)
                        && !entry.is_directory =>
                {
                    commands
                        .spawn((
                            model_grid_tile(&item, &icon_font, entry.path.clone()),
                            ChildOf(content_entity),
                        ))
                        .id()
                }
                BrowserViewMode::Grid => commands
                    .spawn((
                        file_browser::file_browser_item_with_icon(&item, &icon_font, icon_override),
                        ChildOf(content_entity),
                    ))
                    .id(),
                BrowserViewMode::List => commands
                    .spawn((
                        file_browser::file_browser_list_item_with_icon(
                            &item,
                            &icon_font,
                            icon_override,
                        ),
                        ChildOf(content_entity),
                    ))
                    .id(),
            };

            // Apply selected highlight if this item is the selected file
            let is_selected =
                state.selected_file.as_deref() == Some(entry.path.to_string_lossy().as_ref());
            if is_selected {
                commands
                    .entity(item_entity)
                    .insert(BackgroundColor(tokens::ELEVATED_BG));
            }

            commands
                .entity(item_entity)
                .observe(highlight_on_hover)
                .observe(unhighlight_on_out)
                .observe(
                    move |click: On<Pointer<Click>>,
                          mut state: ResMut<AssetBrowserState>,
                          mut commands: Commands,
                          time: Res<Time>| {
                        // Right-click is handled by the context menu observer
                        // below; let it through here.
                        if click.event().button != PointerButton::Primary {
                            return;
                        }
                        let now = time.elapsed_secs_f64();
                        let is_double = state.selected_file.as_deref() == Some(&path_for_click)
                            && (now - state.last_click_time) < 0.4;

                        if is_double {
                            commands.trigger(FileItemDoubleClicked {
                                path: path_for_click.clone(),
                                is_directory: is_dir,
                            });
                        } else {
                            // Single-click: select
                            state.selected_file = Some(path_for_click.clone());
                            state.last_click_time = now;
                            state.needs_refresh = true;
                        }
                    },
                );

            // Right-click: open context menu with a Delete entry. The
            // click also selects the row so the breadcrumb shows the
            // target and the Delete dispatch can pick it up.
            let rmb_path = entry.path.clone();
            commands.entity(item_entity).observe(
                move |click: On<Pointer<Click>>,
                      mut commands: Commands,
                      mut state: ResMut<AssetBrowserState>,
                      windows: Query<&Window>,
                      mut menu_state: ResMut<jackdaw_widgets::context_menu::ContextMenuState>| {
                    if click.event().button != PointerButton::Secondary {
                        return;
                    }
                    state.selected_file = Some(rmb_path.to_string_lossy().into_owned());
                    state.needs_refresh = true;

                    let cursor_pos = windows
                        .single()
                        .ok()
                        .and_then(bevy::prelude::Window::cursor_position)
                        .unwrap_or_default();
                    if let Some(existing) = menu_state.menu_entity.take()
                        && let Ok(mut ec) = commands.get_entity(existing)
                    {
                        ec.despawn();
                    }
                    let menu = jackdaw_feathers::context_menu::spawn_context_menu(
                        &mut commands,
                        cursor_pos,
                        None,
                        &[("asset_browser.delete", "Delete")],
                    );
                    menu_state.menu_entity = Some(menu);
                },
            );

            // Prefab entries and hand-authored `.bsn` scenes: track drag
            // start so the viewport's drop handler can pull the source path
            // out of `ActiveAssetDrag` and route through `spawn_instance`,
            // which normalizes a plain scene into an instanceable prefab.
            // Clear on DragEnd if nothing consumed it.
            let is_bsn_scene = entry.path.extension().is_some_and(|e| e == "bsn");
            if entry.is_prefab || is_bsn_scene {
                let drag_path = entry.path.clone();
                commands.entity(item_entity).observe(
                    move |_: On<Pointer<DragStart>>, mut drag: ResMut<ActiveAssetDrag>| {
                        drag.path = Some(drag_path.clone());
                    },
                );
                commands.entity(item_entity).observe(
                    |_: On<Pointer<DragEnd>>, mut drag: ResMut<ActiveAssetDrag>| {
                        drag.path = None;
                    },
                );
            }
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

    // Each path component is a clickable button that navigates to that directory.
    commands
        .spawn((
            Node {
                width: percent(100),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::FlexStart,
                column_gap: Val::Px(2.0),
                ..default()
            },
            ChildOf(breadcrumb_entity),
        ))
        .with_children(|parent| {
            let mut ancestors: Vec<_> = state.current_directory.ancestors().collect();
            ancestors.reverse();

            for (i, path) in ancestors.iter().enumerate() {
                let component = path
                    .file_name()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|| {
                        // Ok so we don't have a filename. This *should* only happen for the root path (e.g. /, E:\).
                        if i != 0 {
                            return "?".to_owned();
                        }

                        // Use the components() API to do a decent job.
                        match path.components().next() {
                            // Unix-style root path.
                            // Replace with a unique string so that it's nicely clickable.
                            Some(std::path::Component::RootDir) => "Root".to_owned(),
                            // Anything else. Unless something went wrong, this should only be Windows prefixes (E:, \\foo\bar, etc).
                            // We format it like this to avoid the backslash after the prefix (which `path` has).
                            Some(comp) => comp.as_os_str().to_string_lossy().into_owned(),
                            None => "?".to_owned(),
                        }
                    });

                let path = path.to_string_lossy().into_owned();

                // Separator (skip before first)
                if i > 0 {
                    parent.spawn((
                        Text::new(" / "),
                        TextFont {
                            font_size: tokens::TEXT_SIZE,
                            ..Default::default()
                        },
                        TextColor(tokens::TEXT_SECONDARY),
                    ));
                }

                // Clickable path segment
                parent
                    .spawn((
                        Button,
                        Text::new(component),
                        Node {
                            border_radius: BorderRadius::all(Val::Px(3.0)),
                            padding: UiRect::axes(Val::Px(2.0), Val::Px(1.0)),
                            ..default()
                        },
                        TextFont {
                            font_size: tokens::TEXT_SIZE,
                            ..Default::default()
                        },
                        TextColor(tokens::TEXT_TERTIARY),
                    ))
                    .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
                        commands.trigger(FileItemDoubleClicked {
                            path: path.clone(),
                            is_directory: true,
                        });
                    })
                    .observe(highlight_on_hover)
                    .observe(unhighlight_on_out);
            }

            // If a file is selected, show its name at the end of the breadcrumb
            if let Some(ref selected) = state.selected_file {
                let file_name = Path::new(selected)
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_default();
                if !file_name.is_empty() {
                    parent.spawn((
                        Text::new(" / "),
                        TextFont {
                            font_size: tokens::TEXT_SIZE,
                            ..Default::default()
                        },
                        TextColor(tokens::TEXT_SECONDARY),
                    ));
                    parent.spawn((
                        Text::new(file_name),
                        TextFont {
                            font_size: tokens::TEXT_SIZE,
                            ..Default::default()
                        },
                        TextColor(tokens::TEXT_PRIMARY),
                    ));
                }
            }
        });
}

fn update_asset_browser_filter(
    mut state: ResMut<AssetBrowserState>,
    filters: Query<&TextEditValue, (With<AssetBrowserFilter>, Changed<TextEditValue>)>,
) {
    for filter in filters {
        state.filter = filter.0.clone();
        state.needs_refresh = true;
    }
}

fn highlight_on_hover(hover: On<Pointer<Over>>, mut bg: Query<&mut BackgroundColor>) {
    if let Ok(mut bg) = bg.get_mut(hover.event_target()) {
        bg.0 = tokens::HOVER_BG;
    }
}

fn unhighlight_on_out(out: On<Pointer<Out>>, mut bg: Query<&mut BackgroundColor>) {
    if let Ok(mut bg) = bg.get_mut(out.event_target()) {
        bg.0 = Color::NONE;
    }
}

/// Grid tile for a `.glb` / `.gltf` entry.
///
/// Same shape as [`file_browser::file_browser_item_with_icon`] -- and it
/// carries the same `FileBrowserItem`, so click, double-click, right-click
/// and drag-to-viewport all keep working through the observers the caller
/// attaches. The one difference is that the icon sits inside a fixed square
/// tagged [`ModelThumbnailSlot`], which [`crate::model_thumbnail`] swaps for
/// the rendered picture once there is one. The glyph is the fallback, so a
/// pending or failed thumbnail looks exactly like the browser did before.
fn model_grid_tile(item: &FileBrowserItem, icon_font: &IconFont, path: PathBuf) -> impl Bundle {
    let slot_size = crate::model_thumbnail::THUMBNAIL_DISPLAY_SIZE;
    (
        item.clone(),
        Node {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            padding: UiRect::all(Val::Px(6.0)),
            width: Val::Px(80.0),
            border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
            ..default()
        },
        BackgroundColor(Color::NONE),
        children![
            (
                ModelThumbnailSlot::new(path),
                Node {
                    width: Val::Px(slot_size),
                    height: Val::Px(slot_size),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                },
                children![(
                    Text::new(String::from(
                        file_browser::file_icon(&item.file_name).unicode()
                    )),
                    TextFont {
                        font: icon_font.0.clone().into(),
                        font_size: tokens::ICON_LG,
                        ..default()
                    },
                    TextColor(tokens::FILE_ICON_COLOR),
                )],
            ),
            (
                Text::new(truncate_tile_name(&item.file_name, 12)),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..default()
                },
                ThemedText,
            ),
        ],
    )
}

/// Shorten a file name to fit a grid tile, cutting on a character boundary
/// so a non-ASCII name cannot panic the slice.
fn truncate_tile_name(name: &str, max_len: usize) -> String {
    if name.chars().count() <= max_len {
        return name.to_string();
    }
    let head: String = name.chars().take(max_len.saturating_sub(3)).collect();
    format!("{head}...")
}

fn load_thumbnail(path: &Path, asset_server: &AssetServer) -> Option<Handle<Image>> {
    let fs_path = path.to_string_lossy().replace('\\', "/");
    let asset_path = crate::entity_ops::to_asset_path(&fs_path);
    Some(asset_server.load(asset_path))
}

/// Removes `ImageNode` from entities whose loaded image uses an incompatible
/// texture format (e.g. `R16Uint`) that would crash Bevy's UI renderer.
fn remove_incompatible_image_nodes(
    mut commands: Commands,
    image_nodes: Query<(Entity, &ImageNode)>,
    images: Res<Assets<Image>>,
) {
    use bevy::render::render_resource::TextureSampleType;
    for (entity, image_node) in &image_nodes {
        if let Some(image) = images.get(&image_node.image) {
            let sample = image.texture_descriptor.format.sample_type(None, None);
            if !matches!(sample, Some(TextureSampleType::Float { .. })) {
                commands.entity(entity).remove::<ImageNode>();
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
        state.selected_file = None; // Clear selection when navigating
        state.needs_refresh = true;
        return;
    }

    let path_lower = event.path.to_lowercase();
    if path_lower.ends_with(".jsn") || path_lower.ends_with(".bsn") {
        let path_owned = std::path::PathBuf::from(&event.path);
        commands.queue(move |world: &mut World| {
            crate::scenes::operators::scene_open_system(world, &path_owned);
        });
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
            // User-facing click: explicit history opt-in.
            commands
                .operator("material.apply_texture")
                .param("path", event.path.clone())
                .settings(CallOperatorSettings {
                    creates_history_entry: true,
                    ..default()
                })
                .call();
        }
    }
}

/// If the texture filename matches a PBR naming convention, look up the
/// base name in the material registry and return the catalog handle.
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

/// Apply a texture material to the current face selection (in
/// brush-edit face mode) or to every face of every selected brush
/// (expanding selected non-brush parents into their child brushes).
///
/// Parameter: `path`; the asset path of the texture to apply.
///
/// The operator framework captures a scene-AST snapshot before/after
/// this runs and pushes it onto `CommandHistory`, so there's no
/// manual `SetBrush` push. The explicit `sync_brush_to_ast` queue at
/// the end is what makes that work: the outer `save_history` runs
/// immediately after this system returns and captures the after-
/// snapshot from the AST, so the mutation has to land in the AST
/// before that; `BrushPlugin`'s auto-sync fires next `Update`,
/// which is too late here.
#[operator(
    id = "material.apply_texture",
    label = "Apply Texture",
    description = "Apply a texture material to the selected faces or brushes"
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
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("ktx2");
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

    // Reject non-float formats (e.g. R16Uint) incompatible with UI ImageNode rendering
    let sample = image.texture_descriptor.format.sample_type(None, None);
    if !matches!(sample, Some(TextureSampleType::Float { .. })) {
        return;
    }

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
                width: Val::Px(tokens::PREVIEW_IMAGE_SIZE),
                height: Val::Px(tokens::PREVIEW_IMAGE_SIZE),
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
                width: Val::Px(tokens::PREVIEW_IMAGE_SIZE),
                height: Val::Px(tokens::PREVIEW_IMAGE_SIZE),
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
            font_size: tokens::TEXT_SIZE_SM,
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
            font_size: tokens::TEXT_SIZE_SM,
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

        // Previous button. `Hovered + ButtonOperatorCall` are
        // tooltip data sources only; the click below dispatches the
        // operator manually because this is a raw Node spawn (not a
        // feathers `button()`), so it doesn't fire `ButtonClickEvent`.
        let prev_btn = commands
            .spawn((
                Node {
                    padding: UiRect::axes(Val::Px(tokens::SPACING_SM), Val::Px(2.0)),
                    border_radius: BorderRadius::all(Val::Px(3.0)),
                    ..Default::default()
                },
                BackgroundColor(tokens::INPUT_BG),
                Hovered::default(),
                ButtonOperatorCall::new(AssetCycleArrayLayerOp::ID).with_param("direction", -1i64),
                ChildOf(nav_row),
            ))
            .id();
        commands.spawn((
            Text::new("<"),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_PRIMARY),
            ChildOf(prev_btn),
        ));
        commands
            .entity(prev_btn)
            .observe(|_: On<Pointer<Click>>, mut commands: Commands| {
                commands
                    .operator(AssetCycleArrayLayerOp::ID)
                    .param("direction", -1i64)
                    .call();
            });

        commands.spawn((
            Text::new(layer_text),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(nav_row),
        ));

        // Next button. See `prev_btn` above; same pattern.
        let next_btn = commands
            .spawn((
                Node {
                    padding: UiRect::axes(Val::Px(tokens::SPACING_SM), Val::Px(2.0)),
                    border_radius: BorderRadius::all(Val::Px(3.0)),
                    ..Default::default()
                },
                BackgroundColor(tokens::INPUT_BG),
                Hovered::default(),
                ButtonOperatorCall::new(AssetCycleArrayLayerOp::ID).with_param("direction", 1i64),
                ChildOf(nav_row),
            ))
            .id();
        commands.spawn((
            Text::new(">"),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_PRIMARY),
            ChildOf(next_btn),
        ));
        commands
            .entity(next_btn)
            .observe(|_: On<Pointer<Click>>, mut commands: Commands| {
                commands
                    .operator(AssetCycleArrayLayerOp::ID)
                    .param("direction", 1i64)
                    .call();
            });
    }

    // Apply button (only for 2D textures). See `prev_btn` for the
    // ButtonOperatorCall-as-tooltip-data pattern.
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
                Hovered::default(),
                ButtonOperatorCall::new("material.apply_texture")
                    .with_param("path", path_str.clone()),
                ChildOf(container),
            ))
            .id();
        commands.spawn((
            Text::new("Apply"),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_PRIMARY),
            ChildOf(apply_btn),
        ));
        commands
            .entity(apply_btn)
            .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
                // User-facing click: explicit history opt-in.
                commands
                    .operator("material.apply_texture")
                    .param("path", path_str.clone())
                    .settings(CallOperatorSettings {
                        creates_history_entry: true,
                        ..default()
                    })
                    .call();
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
    }
}

fn poll_asset_browser_folder(world: &mut World) {
    let Some(mut task_res) = world.get_resource_mut::<AssetBrowserFolderTask>() else {
        return;
    };
    let Some(result) = future::block_on(future::poll_once(&mut task_res.0)) else {
        return;
    };
    world.remove_resource::<AssetBrowserFolderTask>();

    if let Some(file_handle) = result {
        let path = file_handle.path().to_path_buf();
        let mut state = world.resource_mut::<AssetBrowserState>();
        state.root_directory = path.clone();
        state.current_directory = path.clone();
        state.needs_refresh = true;

        let mut commands = world.commands();
        setup_directory_watcher(&path, &mut commands);

        // Breadcrumb will be rebuilt on next refresh
    }
}

// -- Panel layout ------------------------------------------------------------

pub fn asset_browser_panel(icon_font: Handle<Font>) -> impl Bundle {
    let folder_icon_font = icon_font.clone();
    let chip_icon_font = icon_font.clone();
    // NOTE: the 30px window-selector sidebar that used to live here
    // is now owned by `layout::bottom_panels` (the dock container),
    // because it's about picking WHICH tool window is shown in the
    // bottom panel, not about the asset browser itself. Adding more
    // windows (e.g. Timeline) means adding an icon there, not
    // touching this function.
    let _ = icon_font;
    (
        AssetBrowserPanel,
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
            // Main asset browser content
            (
                Node {
                    flex_direction: FlexDirection::Column,
                    flex_grow: 1.0,
                    min_width: Val::Px(0.0),
                    padding: UiRect::all(Val::Px(tokens::SPACING_SM)),
                    ..Default::default()
                },
                children![
                    // Breadcrumb bar: path on left, search + folder button on right
                    (
                        Node {
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            justify_content: JustifyContent::SpaceBetween,
                            width: Val::Percent(100.0),
                            height: Val::Px(34.0),
                            padding: UiRect::axes(
                                Val::Px(tokens::SPACING_MD),
                                Val::Px(tokens::SPACING_SM)
                            ),
                            flex_shrink: 0.0,
                            ..Default::default()
                        },
                        children![
                            // Left: breadcrumb path
                            (
                                Node {
                                    flex_direction: FlexDirection::Row,
                                    align_items: AlignItems::Center,
                                    overflow: Overflow::clip(),
                                    flex_shrink: 1.0,
                                    flex_grow: 1.0,
                                    ..Default::default()
                                },
                                children![(
                                    AssetBrowserBreadcrumb,
                                    EditorEntity,
                                    Node {
                                        flex_direction: FlexDirection::Row,
                                        align_items: AlignItems::Center,
                                        ..Default::default()
                                    },
                                ),],
                            ),
                            // Right: Search input + folder button
                            (
                                Node {
                                    flex_direction: FlexDirection::Row,
                                    align_items: AlignItems::Center,
                                    column_gap: Val::Px(tokens::SPACING_SM),
                                    flex_shrink: 0.0,
                                    ..Default::default()
                                },
                                children![
                                    // Search... input (matching Figma: 200px width)
                                    (
                                        Node {
                                            width: Val::Px(200.0),
                                            ..Default::default()
                                        },
                                        children![(AssetBrowserFilter, jackdaw_feathers::text_edit::text_edit(
                                            jackdaw_feathers::text_edit::TextEditProps::default()
                                                .with_placeholder("Search...")
                                                .allow_empty()
                                        ),)],
                                    ),
                                    prefabs_only_chip(chip_icon_font),
                                    asset_folder_button(folder_icon_font),
                                ],
                            ),
                        ],
                    ),
                    // Main row: content grid + preview panel (separator at top per Figma)
                    (
                        EditorEntity,
                        Node {
                            flex_direction: FlexDirection::Row,
                            width: Val::Percent(100.0),
                            flex_grow: 1.0,
                            min_height: Val::Px(0.0),
                            border: UiRect::top(Val::Px(1.0)),
                            ..Default::default()
                        },
                        BorderColor::all(tokens::BORDER_SUBTLE),
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
            ), // close main content container
        ],
    )
}

fn asset_folder_button(icon_font: Handle<Font>) -> impl Bundle {
    (
        jackdaw_feathers::button::icon_button(
            jackdaw_feathers::button::IconButtonProps::new(icons::Icon::FolderOpen)
                .variant(jackdaw_feathers::button::ButtonVariant::Ghost),
            &icon_font,
        ),
        ButtonOperatorCall::new(AssetSelectFolderOp::ID),
    )
}

/// Compact toolbar chip that toggles the asset browser's "Prefabs only"
/// filter. The chip's background is updated by
/// `update_prefabs_only_chip_style` to reflect whether the filter is on,
/// and `toggle_prefabs_only_chip` flips `state.prefabs_only` on press.
fn prefabs_only_chip(icon_font: Handle<Font>) -> impl Bundle {
    (
        PrefabsOnlyChip,
        Interaction::default(),
        Hovered::default(),
        Tooltip::title("Prefabs only")
            .with_description("Show only scene files that define a prefab."),
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(tokens::SPACING_XS),
            padding: UiRect::axes(Val::Px(tokens::SPACING_SM), Val::Px(2.0)),
            border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_SM)),
            ..Default::default()
        },
        BackgroundColor(Color::NONE),
        children![
            (
                Text::new(String::from(icons::Icon::Package.unicode())),
                TextFont {
                    font: icon_font.into(),
                    font_size: tokens::TEXT_SIZE,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
            ),
            (
                Text::new("Prefabs"),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
            ),
        ],
    )
}

/// Flip `state.prefabs_only` and request a refresh whenever the chip is
/// pressed.
fn toggle_prefabs_only_chip(
    mut state: ResMut<AssetBrowserState>,
    interactions: Query<&Interaction, (Changed<Interaction>, With<PrefabsOnlyChip>)>,
) {
    for interaction in &interactions {
        if *interaction == Interaction::Pressed {
            state.prefabs_only = !state.prefabs_only;
            state.needs_refresh = true;
        }
    }
}

/// Mirror `state.prefabs_only` onto the chip's background/text colors so
/// the active state is visually obvious.
fn update_prefabs_only_chip_style(
    state: Res<AssetBrowserState>,
    mut chips: Query<(&mut BackgroundColor, Option<&Children>), With<PrefabsOnlyChip>>,
    mut text_colors: Query<&mut TextColor>,
) {
    if !state.is_changed() {
        return;
    }
    let active = state.prefabs_only;
    for (mut bg, children) in &mut chips {
        bg.0 = if active {
            tokens::ELEVATED_BG
        } else {
            Color::NONE
        };
        let target_color = if active {
            tokens::TEXT_PRIMARY
        } else {
            tokens::TEXT_SECONDARY
        };
        let Some(children) = children else { continue };
        for child in children.iter() {
            if let Ok(mut color) = text_colors.get_mut(child) {
                color.0 = target_color;
            }
        }
    }
}

/// Checks for filesystem events from the `notify` watcher and triggers browser refreshes.
fn check_watcher_events(
    watcher: Option<Res<DirectoryWatcher>>,
    mut browser: ResMut<AssetBrowserState>,
    mut material_browser: ResMut<crate::material_browser::MaterialBrowserState>,
) {
    let Some(watcher) = watcher else { return };
    let Ok(rx) = watcher.receiver.lock() else {
        return;
    };
    let mut changed = false;
    while rx.try_recv().is_ok() {
        changed = true;
    }
    if changed {
        browser.needs_refresh = true;
        material_browser.needs_rescan = true;
    }
}

// -- Operators --------------------------------------------------------------

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<AssetCycleArrayLayerOp>()
        .register_operator::<AssetSelectFolderOp>();
}

fn has_array_preview(preview: Res<AssetPreviewState>) -> bool {
    preview
        .selected_info
        .as_ref()
        .is_some_and(|info| info.layer_count > 0)
}

/// Step the asset preview's selected layer by `direction`, wrapping at
/// the texture's layer count.
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

/// Choose a different folder as the assets directory.
#[operator(
    id = "asset.select_folder",
    label = "Select Assets Folder",
    description = "Choose a different folder as the assets directory."
)]
pub fn asset_select_folder(
    _: In<OperatorParameters>,
    mut commands: Commands,
    raw_handle: Query<&RawHandleWrapper, With<PrimaryWindow>>,
) -> OperatorResult {
    let mut dialog = AsyncFileDialog::new().set_title("Select assets directory");
    if let Ok(rh) = raw_handle.single() {
        // SAFETY: the primary window is open, so its `RawHandleWrapper`
        // points to a live OS handle. We use the returned wrapper only
        // to parent the modal dialog within this scope.
        let handle = unsafe { rh.get_handle() };
        dialog = dialog.set_parent(&handle);
    }
    let task = AsyncComputeTaskPool::get().spawn(async move { dialog.pick_folder().await });
    commands.insert_resource(AssetBrowserFolderTask(task));
    OperatorResult::Finished
}

#[cfg(test)]
mod tests {
    use super::read_is_prefab;

    #[test]
    fn read_is_prefab_detects_prefab_component() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.jsn");
        let body = r#"{
            "jsn": { "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" },
            "metadata": { "name": "p", "created": "", "modified": "" },
            "assets": {},
            "scene": [{
                "components": {
                    "jackdaw::prefab::components::Prefab": null,
                    "bevy_ecs::name::Name": "p"
                }
            }]
        }"#;
        std::fs::write(&path, body).unwrap();
        assert!(read_is_prefab(&path), "prefab JSON is detected");
    }

    #[test]
    fn read_is_prefab_rejects_regular_scene() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s.jsn");
        let body = r#"{
            "jsn": { "format_version": [3,0,0], "editor_version": "0", "bevy_version": "0.18" },
            "metadata": { "name": "s", "created": "", "modified": "" },
            "assets": {},
            "scene": [{
                "components": {
                    "bevy_ecs::name::Name": "root"
                }
            }]
        }"#;
        std::fs::write(&path, body).unwrap();
        assert!(!read_is_prefab(&path), "regular scene JSON is not a prefab");
    }

    #[test]
    fn read_is_prefab_returns_false_on_garbage() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("g.jsn");
        std::fs::write(&path, "not json at all").unwrap();
        assert!(!read_is_prefab(&path), "invalid JSON returns false");
    }

    /// `.bsn` is the format the editor actually writes prefabs in, so it is
    /// the case that matters most; detection used to parse every candidate
    /// as JSON and missed all of them.
    #[test]
    fn read_is_prefab_detects_a_bsn_prefab() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("Cube1.bsn");
        let body = "jackdaw::prefab::components::Prefab\n\
                    jackdaw::prefab::components::PrefabEntityId(0)\n\
                    #Cube1\n\
                    bevy_camera::visibility::Visibility::Inherited\n";
        std::fs::write(&path, body).unwrap();
        assert!(read_is_prefab(&path), "a .bsn prefab is detected");
    }

    #[test]
    fn read_is_prefab_rejects_a_plain_bsn_scene() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("scene.bsn");
        let body = "#Root\n\
                    bevy_transform::components::transform::Transform\n\
                    bevy_camera::visibility::Visibility::Inherited\n";
        std::fs::write(&path, body).unwrap();
        assert!(
            !read_is_prefab(&path),
            "a hand-authored scene is not a prefab"
        );
    }

    /// The marker has to be on a root. A document that only mentions the
    /// type path in passing must not pass the filter.
    #[test]
    fn read_is_prefab_rejects_a_bsn_that_only_mentions_the_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("named.bsn");
        let body = "#\"jackdaw::prefab::components::Prefab\"\n\
                    bevy_camera::visibility::Visibility::Inherited\n";
        std::fs::write(&path, body).unwrap();
        assert!(
            !read_is_prefab(&path),
            "the marker as a name is not the marker as a component"
        );
    }

    #[test]
    fn read_is_prefab_returns_false_on_unparseable_bsn() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("broken.bsn");
        std::fs::write(&path, "jackdaw::prefab::components::Prefab {{{{").unwrap();
        assert!(!read_is_prefab(&path), "a parse failure is not a prefab");
    }
}
