use std::path::PathBuf;

use bevy::{
    feathers::theme::ThemedText,
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures_lite::future},
    ui_widgets::observe,
    window::{PrimaryWindow, RawHandleWrapper},
};
use jackdaw_feathers::{
    icons,
    text_edit::{self, TextEditProps, TextEditValue},
    tokens,
};
use rfd::AsyncFileDialog;

use crate::{
    EditorEntity,
    asset_browser::attach_tooltip,
    brush::{Brush, BrushEditMode, BrushSelection, EditMode, SetBrush},
    commands::CommandHistory,
    material_definition::{
        MaterialDefinitionCache, MaterialLibrary, build_material_from_slots, detect_material_sets,
        is_ktx2_non_2d, parse_texture_filename, pbr_filename_regex,
    },
    material_preview::MaterialPreviewState,
    selection::Selection,
};
use jackdaw_feathers::icons::IconFont;

pub struct MaterialBrowserPlugin;

impl Plugin for MaterialBrowserPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MaterialBrowserState>()
            .init_resource::<MaterialPreviewState>()
            .init_resource::<SlotPickerContext>()
            .add_systems(
                OnEnter(crate::AppState::Editor),
                (
                    scan_material_definitions,
                    crate::material_preview::setup_material_preview_scene,
                ),
            )
            .add_systems(
                Update,
                (
                    rescan_material_definitions,
                    apply_material_filter,
                    update_material_browser_ui,
                    update_preview_area,
                    poll_material_browser_folder,
                    poll_material_files_picker,
                    poll_slot_file_picker,
                    crate::material_preview::update_preview_camera_transform,
                    crate::material_preview::update_active_preview_material,
                )
                    .run_if(in_state(crate::AppState::Editor)),
            )
            .add_observer(handle_apply_material)
            .add_observer(handle_select_material_preview)
            .add_observer(handle_pick_slot_texture);
    }
}

#[derive(Resource, Default)]
pub struct MaterialBrowserState {
    pub filter: String,
    pub needs_rescan: bool,
    pub scan_directory: PathBuf,
}

#[derive(Event, Debug, Clone)]
pub struct ApplyMaterialDefToFaces {
    pub name: String,
}

#[derive(Event, Debug, Clone)]
struct SelectMaterialPreview {
    name: String,
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

#[derive(Resource)]
struct MaterialFilesPickerTask(Task<Option<Vec<rfd::FileHandle>>>);

#[derive(Resource, Default)]
struct SlotPickerContext {
    material_name: Option<String>,
    slot_name: Option<String>,
}

#[derive(Resource)]
struct SlotFilePickerTask(Task<Option<rfd::FileHandle>>);

#[derive(Event, Debug, Clone)]
struct PickSlotTexture {
    material_name: String,
    slot_name: String,
}

/// Container for the interactive preview area (shown when a material is selected).
#[derive(Component)]
struct PreviewAreaContainer;

/// The ImageNode displaying the render-to-texture preview.
#[derive(Component)]
struct PreviewAreaImage;

/// Text label showing the selected material name in the preview area.
#[derive(Component)]
struct PreviewAreaLabel;

fn scan_material_definitions(
    mut state: ResMut<MaterialBrowserState>,
    mut library: ResMut<MaterialLibrary>,
    project_root: Option<Res<crate::project::ProjectRoot>>,
) {
    let assets_dir = project_root
        .map(|p| p.assets_dir())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default().join("assets"));
    state.scan_directory = assets_dir.clone();

    let detected = detect_material_sets(&state.scan_directory, &assets_dir);
    for def in detected {
        if library.get_by_name(&def.name).is_none() {
            library.add(def);
        }
    }
}

fn rescan_material_definitions(
    mut state: ResMut<MaterialBrowserState>,
    mut library: ResMut<MaterialLibrary>,
    mut cache: ResMut<MaterialDefinitionCache>,
    project_root: Option<Res<crate::project::ProjectRoot>>,
) {
    if !state.needs_rescan {
        return;
    }
    state.needs_rescan = false;

    let assets_dir = project_root
        .map(|p| p.assets_dir())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default().join("assets"));

    library.materials.clear();
    cache.entries.clear();

    let detected = detect_material_sets(&state.scan_directory, &assets_dir);
    for def in detected {
        library.add(def);
    }
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
) {
    if *edit_mode == EditMode::BrushEdit(BrushEditMode::Face) && !brush_selection.faces.is_empty() {
        if let Some(entity) = brush_selection.entity {
            if let Ok(mut brush) = brushes.get_mut(entity) {
                let old = brush.clone();
                for &face_idx in &brush_selection.faces {
                    if face_idx < brush.faces.len() {
                        brush.faces[face_idx].material_name = Some(event.name.clone());
                        brush.faces[face_idx].texture_path = None;
                    }
                }
                let cmd = SetBrush {
                    entity,
                    old,
                    new: brush.clone(),
                    label: "Apply material".into(),
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
                    face.material_name = Some(event.name.clone());
                    face.texture_path = None;
                }
                let cmd = SetBrush {
                    entity,
                    old,
                    new: brush.clone(),
                    label: "Apply material".into(),
                };
                history.undo_stack.push(Box::new(cmd));
                history.redo_stack.clear();
            }
        }
    }
}

fn handle_select_material_preview(
    event: On<SelectMaterialPreview>,
    mut preview_state: ResMut<MaterialPreviewState>,
) {
    if preview_state.active_material.as_deref() == Some(&event.name) {
        preview_state.active_material = None;
    } else {
        preview_state.active_material = Some(event.name.clone());
        preview_state.orbit_yaw = 0.5;
        preview_state.orbit_pitch = -0.3;
        preview_state.zoom_distance = 3.0;
    }
}

/// Update the interactive preview area visibility and content.
fn update_preview_area(
    mut commands: Commands,
    preview_state: Res<MaterialPreviewState>,
    library: Res<MaterialLibrary>,
    icon_font: Res<IconFont>,
    container_query: Query<(Entity, Option<&Children>), With<PreviewAreaContainer>>,
) {
    if !preview_state.is_changed() {
        return;
    }

    let Ok((container, children)) = container_query.single() else {
        return;
    };

    // Clear existing children
    if let Some(children) = children {
        for child in children.iter() {
            commands.entity(child).despawn();
        }
    }

    let Some(ref active_name) = preview_state.active_material else {
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
    commands.spawn((
        PreviewAreaLabel,
        Text::new(active_name.clone()),
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

    // Slot details — show all 6 slots, with full paths and picker buttons
    if let Some(def) = library.get_by_name(active_name) {
        let slots: &[(&str, &str, &Option<String>)] = &[
            ("Base Color", "base_color", &def.base_color_texture),
            ("Normal", "normal", &def.normal_map_texture),
            ("Metal/Rough", "metallic_roughness", &def.metallic_roughness_texture),
            ("Emissive", "emissive", &def.emissive_texture),
            ("Occlusion", "occlusion", &def.occlusion_texture),
            ("Depth", "depth", &def.depth_texture),
        ];
        for &(label, slot_id, tex) in slots {
            let display = tex
                .as_deref()
                .unwrap_or("None")
                .to_string();

            let row = commands
                .spawn((
                    Node {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(tokens::SPACING_XS),
                        align_items: AlignItems::Center,
                        flex_wrap: FlexWrap::Wrap,
                        ..Default::default()
                    },
                    ChildOf(container),
                ))
                .id();
            commands.spawn((
                Text::new(format!("{label}:")),
                TextFont {
                    font_size: tokens::FONT_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
                Node {
                    flex_shrink: 0.0,
                    ..Default::default()
                },
                ChildOf(row),
            ));
            let text_color = if tex.is_some() {
                tokens::TEXT_PRIMARY
            } else {
                tokens::TEXT_SECONDARY
            };
            let text_entity = commands
                .spawn((
                    Text::new(display.clone()),
                    TextFont {
                        font_size: tokens::FONT_SM,
                        ..Default::default()
                    },
                    TextColor(text_color),
                    Node {
                        flex_shrink: 1.0,
                        ..Default::default()
                    },
                    ChildOf(row),
                ))
                .id();
            // Tooltip for full path in case it gets clipped
            if tex.is_some() {
                attach_tooltip(&mut commands, text_entity, display);
            }

            // Folder picker button for this slot
            let mat_name = active_name.clone();
            let slot_name = slot_id.to_string();
            let picker_btn = commands
                .spawn((
                    Node {
                        padding: UiRect::all(Val::Px(2.0)),
                        flex_shrink: 0.0,
                        ..Default::default()
                    },
                    icons::icon_colored(
                        icons::Icon::FolderOpen,
                        tokens::FONT_SM,
                        icon_font.0.clone(),
                        tokens::TEXT_SECONDARY,
                    ),
                    ChildOf(row),
                ))
                .id();
            commands.entity(picker_btn).observe(
                move |_: On<Pointer<Click>>, mut commands: Commands| {
                    commands.trigger(PickSlotTexture {
                        material_name: mat_name.clone(),
                        slot_name: slot_name.clone(),
                    });
                },
            );
        }
    }

    // Apply button
    let name_for_apply = active_name.clone();
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
                name: name_for_apply.clone(),
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
}

fn handle_pick_slot_texture(
    event: On<PickSlotTexture>,
    mut commands: Commands,
    mut slot_ctx: ResMut<SlotPickerContext>,
    raw_handle: Query<&RawHandleWrapper, With<PrimaryWindow>>,
) {
    slot_ctx.material_name = Some(event.material_name.clone());
    slot_ctx.slot_name = Some(event.slot_name.clone());

    let mut dialog = AsyncFileDialog::new()
        .set_title(format!("Select texture for {} slot", event.slot_name))
        .add_filter(
            "Images",
            &["png", "jpg", "jpeg", "ktx2", "bmp", "tga", "webp"],
        );
    if let Ok(rh) = raw_handle.single() {
        let handle = unsafe { rh.get_handle() };
        dialog = dialog.set_parent(&handle);
    }
    let task = AsyncComputeTaskPool::get().spawn(async move { dialog.pick_file().await });
    commands.insert_resource(SlotFilePickerTask(task));
}

fn poll_slot_file_picker(world: &mut World) {
    let Some(mut task_res) = world.get_resource_mut::<SlotFilePickerTask>() else {
        return;
    };
    let Some(result) = future::block_on(future::poll_once(&mut task_res.0)) else {
        return;
    };
    world.remove_resource::<SlotFilePickerTask>();

    let Some(handle) = result else {
        return;
    };

    let file_path = handle.path().to_path_buf();

    let slot_ctx = world.resource::<SlotPickerContext>();
    let Some(material_name) = slot_ctx.material_name.clone() else {
        return;
    };
    let Some(slot_name) = slot_ctx.slot_name.clone() else {
        return;
    };

    // Convert to asset-relative path
    let project_dir = crate::project::read_last_project();
    let assets_dir = project_dir
        .map(|p| p.join("assets"))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default().join("assets"));

    let asset_path = file_path
        .strip_prefix(&assets_dir)
        .map(|r| r.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| file_path.to_string_lossy().replace('\\', "/"));

    let mut library = world.resource_mut::<MaterialLibrary>();
    if let Some(def) = library.get_by_name_mut(&material_name) {
        match slot_name.as_str() {
            "base_color" => def.base_color_texture = Some(asset_path),
            "normal" => def.normal_map_texture = Some(asset_path),
            "metallic_roughness" => def.metallic_roughness_texture = Some(asset_path),
            "emissive" => def.emissive_texture = Some(asset_path),
            "occlusion" => def.occlusion_texture = Some(asset_path),
            "depth" => def.depth_texture = Some(asset_path),
            _ => {}
        }
    }

    // Clear cached material to force rebuild
    let mut cache = world.resource_mut::<MaterialDefinitionCache>();
    cache.entries.remove(&material_name);

    // Trigger preview rebuild
    let mut preview_state = world.resource_mut::<MaterialPreviewState>();
    preview_state.active_material = Some(material_name);
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

fn update_material_browser_ui(
    mut commands: Commands,
    library: Res<MaterialLibrary>,
    state: Res<MaterialBrowserState>,
    mat_cache: Res<MaterialDefinitionCache>,
    grid_query: Query<(Entity, Option<&Children>), With<MaterialBrowserGrid>>,
    mut root_label_query: Query<&mut Text, With<MaterialBrowserRootLabel>>,
) {
    let needs_rebuild = library.is_changed() || state.is_changed() || mat_cache.is_changed();
    if !needs_rebuild {
        return;
    }

    for mut text in root_label_query.iter_mut() {
        **text = state.scan_directory.to_string_lossy().to_string();
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

    for def in &library.materials {
        if !filter_lower.is_empty() && !def.name.to_lowercase().contains(&filter_lower) {
            continue;
        }

        let name = def.name.clone();

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

        // Use preview_image → base_color_image → gray placeholder
        let thumbnail = mat_cache.entries.get(&name).and_then(|e| {
            e.preview_image
                .clone()
                .or_else(|| e.base_color_image.clone())
        });

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
        let name_for_select = name.clone();
        commands.entity(thumb_entity).observe(
            move |click: On<Pointer<Click>>, mut commands: Commands| {
                if click.event().button == PointerButton::Primary {
                    commands.trigger(SelectMaterialPreview {
                        name: name_for_select.clone(),
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
                    height: Val::Px(tokens::ROW_HEIGHT),
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
                            add_material_button(icon_font.clone()),
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
                    flex_shrink: 0.0,
                    ..Default::default()
                },
            ),
            // Filter input
            (
                Node {
                    padding: UiRect::axes(
                        Val::Px(tokens::SPACING_SM),
                        Val::Px(tokens::SPACING_XS),
                    ),
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

fn add_material_button(icon_font: Handle<Font>) -> impl Bundle {
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
        observe(spawn_material_files_dialog),
    )
}

fn spawn_material_files_dialog(
    _: On<Pointer<Click>>,
    mut commands: Commands,
    raw_handle: Query<&RawHandleWrapper, With<PrimaryWindow>>,
) {
    let mut dialog = AsyncFileDialog::new()
        .set_title("Select texture files for material")
        .add_filter(
            "Images",
            &["png", "jpg", "jpeg", "ktx2", "bmp", "tga", "webp"],
        );
    if let Ok(rh) = raw_handle.single() {
        let handle = unsafe { rh.get_handle() };
        dialog = dialog.set_parent(&handle);
    }
    let task = AsyncComputeTaskPool::get().spawn(async move { dialog.pick_files().await });
    commands.insert_resource(MaterialFilesPickerTask(task));
}

fn poll_material_files_picker(world: &mut World) {
    let Some(mut task_res) = world.get_resource_mut::<MaterialFilesPickerTask>() else {
        return;
    };
    let Some(result) = future::block_on(future::poll_once(&mut task_res.0)) else {
        return;
    };
    world.remove_resource::<MaterialFilesPickerTask>();

    let Some(handles) = result else {
        return;
    };

    let Some(re) = pbr_filename_regex() else {
        return;
    };

    let project_dir = crate::project::read_last_project();
    let assets_dir = project_dir
        .map(|p| p.join("assets"))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default().join("assets"));

    let mut slots: Vec<(String, String)> = Vec::new();
    let mut base_names: Vec<String> = Vec::new();

    for handle in &handles {
        let path = handle.path().to_path_buf();

        // Skip non-2D KTX2
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("ktx2"))
            && is_ktx2_non_2d(&path)
        {
            continue;
        }

        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        if let Some((base_name, tag)) = parse_texture_filename(&file_name, &re) {
            let asset_path = path
                .strip_prefix(&assets_dir)
                .map(|r| r.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"));
            base_names.push(base_name);
            slots.push((tag, asset_path));
        }
    }

    if slots.is_empty() {
        return;
    }

    // Use most common base name
    let mut name_counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for name in &base_names {
        *name_counts.entry(name.as_str()).or_default() += 1;
    }
    let material_name = name_counts
        .into_iter()
        .max_by_key(|&(_, count)| count)
        .map(|(name, _)| name.to_lowercase())
        .unwrap_or_else(|| "new_material".to_string());

    let def = build_material_from_slots(material_name.clone(), &slots);

    // Only add if at least one texture slot is populated
    if def.base_color_texture.is_none()
        && def.normal_map_texture.is_none()
        && def.metallic_roughness_texture.is_none()
        && def.emissive_texture.is_none()
        && def.occlusion_texture.is_none()
        && def.depth_texture.is_none()
    {
        return;
    }

    let mut library = world.resource_mut::<MaterialLibrary>();
    library.upsert(def);

    let mut preview_state = world.resource_mut::<MaterialPreviewState>();
    preview_state.active_material = Some(material_name);
}
