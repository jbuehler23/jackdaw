use std::path::{Path, PathBuf};

use bevy::{
    feathers::theme::ThemedText,
    image::ImageLoaderSettings,
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures_lite::future},
    window::{PrimaryWindow, RawHandleWrapper},
};
use jackdaw_feathers::{
    button::{ButtonOperatorCall, ButtonVariant, IconButtonProps, icon_button},
    icons::{self, Icon},
    panel_card::PanelCardCollapseState,
    text_edit::{self, TextEditProps, TextEditValue},
    tokens,
};
use rfd::AsyncFileDialog;

use crate::brush::LastUsedMaterial;
use crate::material_ui::{
    ActionHeaderProps, HeaderAction, MaterialSection, TextureSlot, fill_surface_rows,
    fill_texture_rows, library_actions, spawn_action_header, spawn_preview, spawn_section,
};
use crate::{
    EditorEntity,
    brush::{Brush, BrushEditMode, BrushSelection, EditMode, SetBrush},
    commands::CommandHistory,
    material_preview::MaterialPreviewState,
    prelude::*,
    selection::Selection,
};
use jackdaw_commands::{CommandGroup, EditorCommand};

pub struct MaterialBrowserPlugin;

impl Plugin for MaterialBrowserPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(crate::material_assets::plugin)
            .init_resource::<MaterialBrowserState>()
            .init_resource::<MaterialPreviewState>()
            .init_resource::<MaterialRegistry>()
            .init_resource::<crate::material_assets::SavedMaterials>()
            .add_systems(
                OnEnter(crate::AppState::Editor),
                (
                    |world: &mut World| crate::asset_catalog::load_catalog(world),
                    rebuild_material_registry,
                    |world: &mut World| crate::asset_catalog::save_catalog(world),
                )
                    .chain(),
            )
            .add_systems(
                Update,
                (
                    rescan_material_definitions,
                    save_catalog_if_dirty,
                    apply_material_filter,
                    update_material_browser_ui.after(rescan_material_definitions),
                    update_preview_area,
                    poll_material_browser_folder,
                    poll_texture_slot_pick,
                )
                    .run_if(in_state(crate::AppState::Editor)),
            )
            .add_observer(on_material_grid_added)
            .add_observer(handle_apply_material)
            .add_observer(handle_select_material_preview)
            .add_observer(handle_browse_texture_slot)
            .add_observer(handle_clear_texture_slot);
    }
}

pub use crate::material_assets::{MaterialRegistry, MaterialRegistryEntry};

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

/// Fixed bar under the panel title holding the material action header. Outside the
/// scrolling body, so the actions stay put.
#[derive(Component)]
struct MaterialActionBar;

/// Container for the editing sections shown for the selected material.
#[derive(Component)]
struct PreviewAreaContainer;

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

/// Load a texture path into an `Image` handle for the given role, choosing the
/// color space from `role.is_srgb()`.
fn load_role_image(
    role: jackdaw_material::TextureRole,
    fs_path: &str,
    asset_server: &AssetServer,
) -> Handle<Image> {
    // `group_texture_sets` returns the absolute filesystem path it was given;
    // derive the asset-relative path for AssetServer loads so we stay inside
    // Bevy's approved-path set.
    let asset_path = crate::entity_ops::to_asset_path(fs_path);
    if role.is_srgb() {
        asset_server.load::<Image>(asset_path)
    } else {
        asset_server
            .load_builder()
            .with_settings(|s: &mut ImageLoaderSettings| s.is_srgb = false)
            .load::<Image>(asset_path)
    }
}

/// Scan a directory for PBR texture sets. `group_texture_sets` already returns
/// them sorted by base name.
fn detect_material_sets(dir: &Path) -> Vec<jackdaw_material::MaterialSet> {
    let mut paths = Vec::new();
    collect_texture_paths(dir, &mut paths);
    jackdaw_material::group_texture_sets(&paths)
}

/// Bind a detected set's files to a fresh `StandardMaterial`.
fn material_from_set(
    set: &jackdaw_material::MaterialSet,
    asset_server: &AssetServer,
    materials: &mut Assets<StandardMaterial>,
) -> Handle<StandardMaterial> {
    use jackdaw_material::TextureRole;

    let base_color_texture = set
        .base_color
        .as_deref()
        .map(|p| load_role_image(TextureRole::BaseColor, p, asset_server));
    let normal_map_texture = set
        .normal
        .as_deref()
        .map(|p| load_role_image(TextureRole::Normal, p, asset_server));
    let metallic_roughness_texture = set
        .metallic_roughness
        .as_deref()
        .map(|p| load_role_image(TextureRole::MetallicRoughness, p, asset_server));
    let emissive_texture = set
        .emissive
        .as_deref()
        .map(|p| load_role_image(TextureRole::Emissive, p, asset_server));
    let occlusion_texture = set
        .occlusion
        .as_deref()
        .map(|p| load_role_image(TextureRole::Occlusion, p, asset_server));
    // A 16-bit height map binds like any other: the material asset layer retags the `Uint`
    // decode as its filterable `Unorm` twin before anything reaches a bind group.
    let depth_map = set
        .depth
        .as_deref()
        .map(|p| load_role_image(TextureRole::Depth, p, asset_server));

    let scalars = set.recommended_scalars();
    materials.add(StandardMaterial {
        base_color_texture,
        normal_map_texture,
        metallic_roughness_texture,
        emissive_texture,
        occlusion_texture,
        depth_map,
        metallic: scalars.metallic,
        perceptual_roughness: scalars.perceptual_roughness,
        parallax_depth_scale: scalars.parallax_depth_scale,
        parallax_mapping_method: bevy::pbr::ParallaxMappingMethod::Occlusion,
        max_parallax_layer_count: scalars.max_parallax_layer_count,
        ..default()
    })
}

/// Recursively walk a directory, collecting candidate texture file paths as
/// forward-slash strings. The crate's regex is the authority on which of these
/// actually parse into material sets; this walk only filters out non-2D KTX2
/// files (cubemaps, texture arrays) that can't bind to a `StandardMaterial`
/// texture slot.
fn collect_texture_paths(dir: &Path, paths: &mut Vec<String>) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_texture_paths(&path, paths);
        } else {
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("ktx2"))
                && crate::asset_browser::is_ktx2_non_2d(&path)
            {
                continue;
            }

            paths.push(path.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// Rebuild [`MaterialRegistry`] from the catalog plus a fresh scan of both
/// `assets/materials` and the texture sets under `assets/`.
///
/// Saved materials are listed first and win their base name: a detected set
/// whose name already belongs to a saved material is skipped. Detected sets
/// enter the in-memory catalog (so `@Name` face references resolve and scene
/// saves emit them) but stay unsaved until `material.save` writes a file.
fn rebuild_material_registry(world: &mut World) {
    let assets_dir = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(super::project::ProjectRoot::assets_dir)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default().join("assets"));
    world.resource_mut::<MaterialBrowserState>().scan_directory = assets_dir.clone();
    world.resource_mut::<MaterialRegistry>().entries.clear();

    // Material files written since the last scan (by another tool, by hand, or by a second
    // editor) become saved materials here, without reopening the project.
    crate::material_assets::rescan_material_files(world);

    let durable = world
        .resource::<crate::material_assets::SavedMaterials>()
        .0
        .clone();
    let mut saved: Vec<(String, Handle<StandardMaterial>)> = world
        .resource::<crate::asset_catalog::AssetCatalog>()
        .handles
        .iter()
        .filter(|(name, handle)| {
            handle.type_id() == std::any::TypeId::of::<StandardMaterial>()
                && durable.contains(name.trim_start_matches(['@', '#']))
        })
        .map(|(name, handle)| {
            (
                name.trim_start_matches(['@', '#']).to_string(),
                handle.clone().typed::<StandardMaterial>(),
            )
        })
        .collect();
    saved.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, handle) in saved {
        world
            .resource_mut::<MaterialRegistry>()
            .add_saved(name, handle);
    }

    let sets = detect_material_sets(&assets_dir);
    info!(
        "Material scan: {} detected sets in {}",
        sets.len(),
        assets_dir.display()
    );
    for set in sets {
        // The registry key, the file stem a save would use and the `@Name` scenes reference
        // are one string, so detection commits to the file-safe spelling up front.
        let name = crate::material_assets::sanitize_material_name(&set.base_name);
        if world
            .resource::<MaterialRegistry>()
            .get_by_name(&name)
            .is_some()
        {
            continue;
        }
        let catalog_name = format!("@{name}");
        // Reuse the handle a previous scan published under this name, so a rescan does not
        // orphan the material on faces that reference it.
        let existing = world
            .resource::<crate::asset_catalog::AssetCatalog>()
            .handles
            .get(&catalog_name)
            .filter(|handle| handle.type_id() == std::any::TypeId::of::<StandardMaterial>())
            .map(|handle| handle.clone().typed::<StandardMaterial>());
        let handle = match existing {
            Some(handle) => handle,
            None => {
                let handle = {
                    let asset_server = world.resource::<AssetServer>().clone();
                    let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
                    material_from_set(&set, &asset_server, &mut materials)
                };
                world
                    .resource_mut::<crate::asset_catalog::AssetCatalog>()
                    .insert(catalog_name, handle.clone().untyped());
                handle
            }
        };
        world.resource_mut::<MaterialRegistry>().add(name, handle);
    }

    // Whatever the catalog holds that neither pass claimed: a material created this run and
    // never saved, or one whose file is gone. It is in memory and references to it resolve,
    // so it is listed as unsaved.
    let mut orphans: Vec<(String, Handle<StandardMaterial>)> = world
        .resource::<crate::asset_catalog::AssetCatalog>()
        .handles
        .iter()
        .filter(|(_, handle)| handle.type_id() == std::any::TypeId::of::<StandardMaterial>())
        .map(|(name, handle)| {
            (
                name.trim_start_matches(['@', '#']).to_string(),
                handle.clone().typed::<StandardMaterial>(),
            )
        })
        .collect();
    orphans.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, handle) in orphans {
        if world
            .resource::<MaterialRegistry>()
            .get_by_name(&name)
            .is_some()
        {
            continue;
        }
        world.resource_mut::<MaterialRegistry>().add(name, handle);
    }

    world.resource_mut::<MaterialRegistry>().ensure_none_entry();
}

fn on_material_grid_added(
    _trigger: On<Add, MaterialBrowserGrid>,
    mut state: ResMut<MaterialBrowserState>,
) {
    info!("MaterialBrowserGrid added, triggering rescan");
    state.needs_rescan = true;
}

fn rescan_material_definitions(world: &mut World) {
    if !world.resource::<MaterialBrowserState>().needs_rescan {
        return;
    }
    world.resource_mut::<MaterialBrowserState>().needs_rescan = false;
    rebuild_material_registry(world);
}

fn save_catalog_if_dirty(world: &mut World) {
    let is_dirty = world
        .get_resource::<crate::asset_catalog::AssetCatalog>()
        .is_some_and(|c| c.dirty);
    if !is_dirty {
        return;
    }

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
    children_query: Query<&Children>,
    mut last_material: ResMut<LastUsedMaterial>,
    mut catalog: ResMut<crate::asset_catalog::AssetCatalog>,
    mut commands: Commands,
) {
    last_material.material = Some(event.material.clone());
    // The assignment is durable in the scene, so the catalog is rewritten; an unsaved
    // material stays flagged as needing its own save.
    catalog.dirty = true;

    let active_faces: Vec<usize> = brush_selection
        .active_sub()
        .map(|s| s.faces.clone())
        .unwrap_or_default();
    if *edit_mode == EditMode::BrushEdit(BrushEditMode::Face) && !active_faces.is_empty() {
        if let Some(entity) = brush_selection.active_brush
            && let Ok(mut brush) = brushes.get_mut(entity)
        {
            let old = brush.clone();
            for &face_idx in &active_faces {
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
            history.push_executed(Box::new(cmd));
            // Deferred AST sync (SetBrush was pushed without execute)
            commands.queue(move |world: &mut World| {
                crate::brush::sync_brush_to_ast(world, entity, &new_brush);
            });
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
        let mut group_commands: Vec<Box<dyn EditorCommand>> = Vec::new();
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
                group_commands.push(Box::new(cmd));
                // Deferred AST sync (SetBrush was pushed without execute)
                commands.queue(move |world: &mut World| {
                    crate::brush::sync_brush_to_ast(world, entity, &new_brush);
                });
            }
        }
        if !group_commands.is_empty() {
            history.push_executed(Box::new(CommandGroup {
                commands: group_commands,
                label: "Apply material".into(),
            }));
        }
    }

    // The inspector only ever shows the primary selection, and applying a
    // material only changes the assigned handle (not the component set), so
    // refresh all material card bodies at once rather than tearing down and
    // rebuilding the whole panel per applied brush. The full rebuild is reserved
    // for archetype changes; the targeted refresh falls back to it if no
    // material card is currently mounted.
    if let Some(primary) = selection.primary() {
        commands.trigger(
            crate::inspector::material_card_routing::RefreshInspectorCardBody {
                source: primary,
                type_path: "material_card::".to_string(),
            },
        );
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

/// What the editing sections are built from. A rebuild happens when one of
/// these changes and at no other time.
///
/// No camera state appears here: orbiting or zooming writes
/// `MaterialPreviewState` every frame of the gesture, and the preview's drag
/// observer lives inside this subtree, so a rebuild driven by that state would
/// despawn the observer mid-drag. The preview renders camera state through
/// `material_preview`'s own systems, which touch no UI.
///
/// Values are absent too: structure is rebuilt from this signature, values are
/// refreshed in place by `material_ui::refresh_material_rows` off
/// `AssetEvent::Modified`. A row open on two surfaces resyncs through its
/// binding rather than through a rebuild, which would tear the section down
/// under the pointer on every drag of the colour picker.
#[derive(PartialEq, Clone, Debug, Default)]
struct PreviewAreaBuild {
    material: Option<AssetId<StandardMaterial>>,
    name: String,
    saved: bool,
    /// Which slots are bound. The rows differ by it (swatch, file name, and
    /// whether a clear button exists), so binding or clearing a texture rebuilds.
    slots: Vec<Option<AssetId<Image>>>,
}

/// Rebuild the action header and the editing sections for the previewed material.
///
/// The header rebuilds even with nothing selected, so the actions never move;
/// Save and Delete disable themselves through their operators' availability.
fn update_preview_area(
    mut commands: Commands,
    preview_state: Res<MaterialPreviewState>,
    registry: Res<MaterialRegistry>,
    materials: Res<Assets<StandardMaterial>>,
    collapse: Res<PanelCardCollapseState>,
    bar_query: Query<(Entity, Option<&Children>), With<MaterialActionBar>>,
    container_query: Query<(Entity, Option<&Children>), With<PreviewAreaContainer>>,
    icon_font: Res<icons::IconFont>,
    italic_font: Res<icons::EditorFontItalic>,
    mut last_material: ResMut<LastUsedMaterial>,
    mut built: Local<Option<PreviewAreaBuild>>,
) {
    let active = preview_state
        .active_material
        .clone()
        .filter(|handle| *handle != Handle::default());
    let entry = active
        .as_ref()
        .and_then(|handle| registry.entries.iter().find(|e| e.handle == *handle));
    let (name, saved) = entry
        .map(|entry| (entry.name.clone(), entry.saved))
        .unwrap_or_else(|| ("No material selected".to_string(), true));

    let material = active.as_ref().and_then(|handle| materials.get(handle));
    let wanted = preview_area_build(active.as_ref(), &name, saved, material);
    if built.as_ref() == Some(&wanted) || container_query.is_empty() {
        return;
    }
    *built = Some(wanted);

    let icon_font = icon_font.0.clone();
    let italic_font = italic_font.0.clone();

    for (bar, children) in &bar_query {
        despawn_children(&mut commands, children);
        spawn_action_header(
            &mut commands,
            bar,
            ActionHeaderProps {
                name: name.clone(),
                saved,
                italic_font: &italic_font,
                icon_font: &icon_font,
                actions: browser_actions(),
            },
        );
    }

    for (container, children) in &container_query {
        despawn_children(&mut commands, children);
        let Some(handle) = active.clone() else {
            continue;
        };
        // Selecting arms the material for the next brush drawn, leaving placed brushes
        // untouched.
        last_material.material = Some(handle.clone());

        let preview = spawn_section(
            &mut commands,
            container,
            PREVIEW_SECTION,
            &icon_font,
            &collapse,
        );
        spawn_preview(
            &mut commands,
            preview.body,
            preview_state.preview_image.clone(),
        );

        let Some(material) = material else {
            continue;
        };
        let surface = spawn_section(
            &mut commands,
            container,
            SURFACE_SECTION,
            &icon_font,
            &collapse,
        );
        fill_surface_rows(&mut commands, surface.body, material, &handle);

        let textures = spawn_section(
            &mut commands,
            container,
            TEXTURES_SECTION,
            &icon_font,
            &collapse,
        );
        fill_texture_rows(&mut commands, textures.body, material, &handle, &icon_font);
    }
}

/// The signature the sections would be built from.
///
/// Takes no camera state, so an orbit cannot tear down the observer driving it.
fn preview_area_build(
    active: Option<&Handle<StandardMaterial>>,
    name: &str,
    saved: bool,
    material: Option<&StandardMaterial>,
) -> PreviewAreaBuild {
    PreviewAreaBuild {
        material: active.map(Handle::id),
        name: name.to_string(),
        saved,
        slots: material
            .map(|material| {
                TextureSlot::ALL
                    .iter()
                    .map(|slot| slot.get_from(material).as_ref().map(Handle::id))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// The library surface opens the preview and the surface values, and leaves the
/// per-slot texture bindings collapsed.
const PREVIEW_SECTION: MaterialSection =
    MaterialSection::new("Preview", Icon::Eye, "materials.window.preview", false);
const SURFACE_SECTION: MaterialSection =
    MaterialSection::new("Surface", Icon::Palette, "materials.window.surface", false);
const TEXTURES_SECTION: MaterialSection =
    MaterialSection::new("Textures", Icon::Image, "materials.window.textures", true);

fn despawn_children(commands: &mut Commands, children: Option<&Children>) {
    let Some(children) = children else {
        return;
    };
    for child in children.iter() {
        commands.entity(child).despawn();
    }
}

/// The library actions, plus applying the previewed material to the selection.
fn browser_actions() -> Vec<HeaderAction> {
    let mut actions = library_actions();
    actions.push(HeaderAction::new(
        Icon::PaintBucket,
        "Apply Material",
        "Apply this material to the selected faces or brushes.",
        ButtonOperatorCall::new(MaterialApplyOp::ID),
    ));
    actions
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
        .set_title(format!("Select image for {}", slot.field()))
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
    let fs_path = path.to_string_lossy().replace('\\', "/");
    let asset_path = crate::entity_ops::to_asset_path(&fs_path);
    let asset_server = world.resource::<AssetServer>().clone();
    let image_handle = if slot.is_srgb() {
        asset_server.load::<Image>(asset_path)
    } else {
        asset_server
            .load_builder()
            .with_settings(|s: &mut ImageLoaderSettings| s.is_srgb = false)
            .load::<Image>(asset_path)
    };

    let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
    if let Some(mut mat) = materials.get_mut(&material_handle) {
        slot.set_on(&mut mat, Some(image_handle));
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
    if let Some(mut mat) = materials.get_mut(&event.material_handle) {
        event.slot.set_on(&mut mat, None);
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
    italic_font: Res<icons::EditorFontItalic>,
    grid_query: Query<(Entity, Option<&Children>), With<MaterialBrowserGrid>>,
    mut root_label_query: Query<&mut Text, With<MaterialBrowserRootLabel>>,
) {
    let italic_font = italic_font.0.clone();
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
        info!(
            "update_material_browser_ui: no MaterialBrowserGrid entity found, {} entries in registry",
            registry.entries.len()
        );
        return;
    };
    info!(
        "update_material_browser_ui: rebuilding with {} entries",
        registry.entries.len()
    );

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

        let handle = entry.handle.clone();
        let tile = crate::material_assets::spawn_material_tile(
            &mut commands,
            grid_entity,
            crate::material_assets::MaterialTile {
                name: entry.name.clone(),
                thumbnail: crate::material_assets::material_thumbnail(&materials, &handle),
                saved: entry.saved,
                selected: false,
                italic_font: italic_font.clone(),
            },
        );

        // Single-click: select for preview
        commands
            .entity(tile)
            .observe(move |click: On<Pointer<Click>>, mut commands: Commands| {
                if click.event().button == PointerButton::Primary {
                    commands.trigger(SelectMaterialPreview {
                        handle: handle.clone(),
                    });
                }
            });
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
                                    font_size: tokens::TEXT_SIZE,
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
                                Text::default(),
                                TextFont {
                                    font_size: tokens::TEXT_SIZE_SM,
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
                            material_folder_button(icon_font.clone()),
                            rescan_button(icon_font),
                        ],
                    ),
                ],
            ),
            // Action header.
            (
                MaterialActionBar,
                EditorEntity,
                Node {
                    flex_direction: FlexDirection::Column,
                    width: Val::Percent(100.0),
                    padding: UiRect::axes(Val::Px(tokens::SPACING_MD), Val::Px(tokens::SPACING_XS)),
                    flex_shrink: 0.0,
                    ..Default::default()
                },
            ),
            // Editing sections for the previewed material.
            (
                PreviewAreaContainer,
                EditorEntity,
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(tokens::SPACING_XS),
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

fn material_folder_button(icon_font: Handle<Font>) -> impl Bundle {
    (
        icon_button(
            IconButtonProps::new(Icon::FolderOpen).variant(ButtonVariant::Ghost),
            &icon_font,
        ),
        ButtonOperatorCall::new(MaterialSelectFolderOp::ID),
    )
}

fn rescan_button(icon_font: Handle<Font>) -> impl Bundle {
    (
        icon_button(
            IconButtonProps::new(Icon::RefreshCw).variant(ButtonVariant::Ghost),
            &icon_font,
        ),
        ButtonOperatorCall::new(MaterialRescanOp::ID),
    )
}

// -- Operators --------------------------------------------------------------

/// Resource set by the texture-slot button click before dispatching
/// `material.browse_texture_slot` / `material.clear_texture_slot`. The
/// operator reads this and clears it. `Handle<StandardMaterial>` doesn't
/// fit `PropertyValue`, so we route it through a sidecar resource the
/// same way `hierarchy.rename_begin` uses `PendingRenameTarget`.
#[derive(Resource, Default)]
pub(crate) struct PendingTextureSlot {
    pub(crate) slot: Option<TextureSlot>,
    pub(crate) material_handle: Option<Handle<StandardMaterial>>,
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.init_resource::<PendingTextureSlot>()
        .register_operator::<MaterialCreateOp>()
        .register_operator::<MaterialSelectOp>()
        .register_operator::<MaterialApplyOp>()
        .register_operator::<MaterialRescanOp>()
        .register_operator::<MaterialSelectFolderOp>()
        .register_operator::<MaterialBrowseTextureSlotOp>()
        .register_operator::<MaterialClearTextureSlotOp>();
}

fn pending_texture_slot_set(pending: Res<PendingTextureSlot>) -> bool {
    pending.slot.is_some() && pending.material_handle.is_some()
}

/// Create a fresh empty material and select it for preview. It stays unsaved
/// until `material.save` writes a file for it.
#[operator(
    id = "material.create",
    label = "New Material",
    description = "Create a fresh empty material."
)]
pub(crate) fn material_create(
    _: In<OperatorParameters>,
    mut registry: ResMut<MaterialRegistry>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut catalog: ResMut<crate::asset_catalog::AssetCatalog>,
    mut preview_state: ResMut<MaterialPreviewState>,
) -> OperatorResult {
    let name = registry.next_created_name();
    let handle = materials.add(StandardMaterial::default());
    catalog.insert(format!("@{name}"), handle.clone().untyped());
    registry.add(name, handle.clone());
    preview_state.active_material = Some(handle);
    preview_state.orbit_yaw = 0.5;
    preview_state.orbit_pitch = -0.3;
    preview_state.zoom_distance = 3.0;
    OperatorResult::Finished
}

/// Load a named material into the shared preview.
///
/// Each surface's action header aims its Save and Delete at the preview, so this
/// also picks which material those act on.
#[operator(
    id = "material.select",
    label = "Select Material",
    description = "Load a material into the material preview.",
    allows_undo = false,
    params(material(String, doc = "Name of the material to preview."))
)]
pub(crate) fn material_select(
    params: In<OperatorParameters>,
    registry: Res<MaterialRegistry>,
    mut preview_state: ResMut<MaterialPreviewState>,
) -> OperatorResult {
    let name = params.as_str("material")?;
    let Some(entry) = registry.get_by_name(name) else {
        warn!("material.select: no material named '{name}'");
        return OperatorResult::Cancelled;
    };
    preview_state.active_material = Some(entry.handle.clone());
    preview_state.orbit_yaw = 0.5;
    preview_state.orbit_pitch = -0.3;
    preview_state.zoom_distance = 3.0;
    OperatorResult::Finished
}

/// Apply a material to the selected faces, or to every face of the selected
/// brushes. The apply records a history entry per brush, so this operator adds
/// none of its own.
#[operator(
    id = "material.apply",
    label = "Apply Material",
    description = "Apply a material to the selected faces or brushes.",
    allows_undo = false,
    params(material(
        String,
        doc = "Name of the material to apply. Defaults to the previewed one."
    ))
)]
pub(crate) fn material_apply(
    params: In<OperatorParameters>,
    registry: Res<MaterialRegistry>,
    preview_state: Res<MaterialPreviewState>,
    mut commands: Commands,
) -> OperatorResult {
    let handle = match params.as_str("material") {
        Some(name) => registry.get_by_name(name).map(|e| e.handle.clone()),
        None => preview_state.active_material.clone(),
    };
    let Some(material) = handle.filter(|h| *h != Handle::default()) else {
        warn!("material.apply: no material to apply");
        return OperatorResult::Cancelled;
    };
    commands.trigger(ApplyMaterialDefToFaces { material });
    OperatorResult::Finished
}

/// Refresh the material browser from disk.
#[operator(
    id = "material.rescan",
    label = "Rescan Materials",
    description = "Refresh the material browser from disk."
)]
pub(crate) fn material_rescan(
    _: In<OperatorParameters>,
    mut state: ResMut<MaterialBrowserState>,
) -> OperatorResult {
    state.needs_rescan = true;
    OperatorResult::Finished
}

/// Choose a different folder as the materials directory.
#[operator(
    id = "material.select_folder",
    label = "Select Materials Folder",
    description = "Choose a different folder as the materials directory."
)]
pub fn material_select_folder(
    _: In<OperatorParameters>,
    mut commands: Commands,
    raw_handle: Query<&bevy::window::RawHandleWrapper, With<bevy::window::PrimaryWindow>>,
) -> OperatorResult {
    let mut dialog = AsyncFileDialog::new().set_title("Select materials directory");
    if let Ok(rh) = raw_handle.single() {
        // SAFETY: the primary window is open, so its `RawHandleWrapper`
        // points to a live OS handle. We use the returned wrapper only
        // to parent the modal dialog within this scope.
        let handle = unsafe { rh.get_handle() };
        dialog = dialog.set_parent(&handle);
    }
    let task =
        bevy::tasks::AsyncComputeTaskPool::get().spawn(async move { dialog.pick_folder().await });
    commands.insert_resource(MaterialBrowserFolderTask(task));
    OperatorResult::Finished
}

/// Pick an image from disk for the targeted material's texture slot.
///
/// The slot and target material are routed through the
/// [`PendingTextureSlot`] resource by the inspector button before
/// dispatch.
#[operator(
    id = "material.browse_texture_slot",
    label = "Browse Texture",
    description = "Pick an image to assign to this material's texture slot.",
    is_available = pending_texture_slot_set
)]
pub(crate) fn material_browse_texture_slot(
    _: In<OperatorParameters>,
    mut pending: ResMut<PendingTextureSlot>,
    mut commands: Commands,
) -> OperatorResult {
    let slot = pending.slot.take()?;
    let material_handle = pending.material_handle.take()?;
    commands.trigger(BrowseTextureSlot {
        slot,
        material_handle,
    });
    OperatorResult::Finished
}

/// Remove the image from the targeted material's texture slot.
///
/// The slot and target material are routed through the
/// [`PendingTextureSlot`] resource by the inspector button before
/// dispatch.
#[operator(
    id = "material.clear_texture_slot",
    label = "Clear Texture",
    description = "Remove the image from this material's texture slot.",
    is_available = pending_texture_slot_set
)]
pub(crate) fn material_clear_texture_slot(
    _: In<OperatorParameters>,
    mut pending: ResMut<PendingTextureSlot>,
    mut commands: Commands,
) -> OperatorResult {
    let slot = pending.slot.take()?;
    let material_handle = pending.material_handle.take()?;
    commands.trigger(ClearTextureSlot {
        slot,
        material_handle,
    });
    OperatorResult::Finished
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::AssetPlugin;

    fn browser_app() -> App {
        let mut app = App::new();
        app.add_plugins((bevy::app::TaskPoolPlugin::default(), AssetPlugin::default()));
        app.init_asset::<Image>();
        app.init_asset::<StandardMaterial>();
        app
    }

    fn detected(paths: &[&str]) -> jackdaw_material::MaterialSet {
        let owned: Vec<String> = paths.iter().map(|p| (*p).to_string()).collect();
        jackdaw_material::group_texture_sets(&owned)
            .into_iter()
            .next()
            .expect("one set")
    }

    fn built(app: &mut App, set: &jackdaw_material::MaterialSet) -> StandardMaterial {
        let asset_server = app.world().resource::<AssetServer>().clone();
        let handle = {
            let mut materials = app.world_mut().resource_mut::<Assets<StandardMaterial>>();
            material_from_set(set, &asset_server, &mut materials)
        };
        app.world()
            .resource::<Assets<StandardMaterial>>()
            .get(&handle)
            .expect("built material")
            .clone()
    }

    /// The parallax scalars apply only with a height map bound, so a detected height map has
    /// to reach the slot they read.
    #[test]
    fn a_detected_height_map_binds_beside_the_parallax_scalars_it_implies() {
        let mut app = browser_app();
        let set = detected(&["pack/rock_albedo.png", "pack/rock_height.png"]);
        let scalars = set.recommended_scalars();
        let material = built(&mut app, &set);

        assert!(material.depth_map.is_some());
        assert_eq!(material.parallax_depth_scale, scalars.parallax_depth_scale);
        assert_eq!(
            material.max_parallax_layer_count,
            scalars.max_parallax_layer_count
        );
        assert!(material.parallax_depth_scale > 0.0);
    }

    /// The preview's drag observer lives inside the subtree a rebuild despawns, and the
    /// colour picker writes `base_color` on every frame of a drag.
    #[test]
    fn a_scalar_edit_does_not_ask_for_a_rebuild() {
        let mut app = browser_app();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());

        let before = {
            let materials = app.world().resource::<Assets<StandardMaterial>>();
            preview_area_build(Some(&handle), "rock", true, materials.get(&handle))
        };
        {
            let mut materials = app.world_mut().resource_mut::<Assets<StandardMaterial>>();
            let mut material = materials.get_mut(&handle).expect("material");
            material.base_color = Color::srgb(0.1, 0.2, 0.3);
            material.metallic = 0.75;
        }
        let after = {
            let materials = app.world().resource::<Assets<StandardMaterial>>();
            preview_area_build(Some(&handle), "rock", true, materials.get(&handle))
        };

        assert_eq!(
            before, after,
            "a value edit changes no row's structure, so nothing is rebuilt",
        );
    }

    /// Binding a texture changes the rows: the swatch, the file name and whether a clear
    /// button exists.
    #[test]
    fn binding_a_texture_does_ask_for_a_rebuild() {
        let mut app = browser_app();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let image = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .reserve_handle();

        let before = {
            let materials = app.world().resource::<Assets<StandardMaterial>>();
            preview_area_build(Some(&handle), "rock", true, materials.get(&handle))
        };
        {
            let mut materials = app.world_mut().resource_mut::<Assets<StandardMaterial>>();
            materials
                .get_mut(&handle)
                .expect("material")
                .base_color_texture = Some(image);
        }
        let after = {
            let materials = app.world().resource::<Assets<StandardMaterial>>();
            preview_area_build(Some(&handle), "rock", true, materials.get(&handle))
        };

        assert_ne!(before, after);
    }

    /// Saving changes the marker beside the name, which the header draws.
    #[test]
    fn saving_a_material_asks_for_a_rebuild() {
        let mut app = browser_app();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let materials = app.world().resource::<Assets<StandardMaterial>>();
        assert_ne!(
            preview_area_build(Some(&handle), "rock", false, materials.get(&handle)),
            preview_area_build(Some(&handle), "rock", true, materials.get(&handle)),
        );
    }

    #[test]
    fn a_detected_set_with_no_height_map_leaves_parallax_off() {
        let mut app = browser_app();
        let material = built(&mut app, &detected(&["pack/rock_albedo.png"]));

        assert!(material.depth_map.is_none());
        assert_eq!(material.parallax_depth_scale, 0.0);
        assert_eq!(material.max_parallax_layer_count, 0.0);
    }

    fn project_browser_app() -> (App, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut app = browser_app();
        // A material file is read and reflected back rather than loaded through the asset
        // server, so the scan needs the reflection registrations.
        app.register_asset_reflect::<Image>();
        app.register_asset_reflect::<StandardMaterial>();
        app.register_type::<StandardMaterial>();
        app.insert_resource(crate::project::ProjectRoot {
            root: tmp.path().to_path_buf(),
            config: crate::project::ProjectConfig::default(),
        });
        app.init_resource::<MaterialRegistry>();
        app.init_resource::<crate::material_assets::SavedMaterials>();
        app.init_resource::<crate::asset_catalog::AssetCatalog>();
        app.init_resource::<MaterialBrowserState>();
        (app, tmp)
    }

    /// A material whose file went missing stays in the list so its name resolves, and lists
    /// as unsaved so nothing writes the file back and a scene using it embeds it inline.
    #[test]
    fn a_material_whose_file_vanished_is_listed_as_unsaved() {
        let (mut app, tmp) = project_browser_app();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        crate::material_assets::write_material_file(app.world(), "slate", &handle).expect("write");

        rebuild_material_registry(app.world_mut());
        assert!(
            app.world().resource::<MaterialRegistry>().is_saved("slate"),
            "a material with a file behind it is saved",
        );

        std::fs::remove_file(tmp.path().join("assets/materials/slate.material.bsn"))
            .expect("remove");
        rebuild_material_registry(app.world_mut());

        let registry = app.world().resource::<MaterialRegistry>();
        let entry = registry.get_by_name("slate").expect("still listed");
        assert!(!entry.saved, "with no file behind it, it reads as unsaved");
        assert!(
            registry.saved_entries().all(|e| e.name != "slate"),
            "nothing durable may name it any more",
        );
    }
}
