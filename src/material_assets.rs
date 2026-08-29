//! Saved material assets: `assets/materials/<name>.material.bsn`.
//!
//! A saved material is one reflected `StandardMaterial` in its own `.bsn`
//! file, named by the file stem. Every editor surface that shows or edits
//! materials reads the same [`MaterialRegistry`] and dispatches the same
//! operators from this module; nothing here depends on a particular panel.
//!
//! # Saved vs unsaved
//!
//! Detected texture sets and freshly created materials are *unsaved*: they
//! live in the running editor's `Assets<StandardMaterial>` and in the shared
//! [`crate::asset_catalog::AssetCatalog`] (so `@Name` references resolve and
//! scene saves emit them), but nothing is written for them. `material.save`
//! writes the file and promotes the entry. Detected sets are reproducible
//! from the same texture files on the next open; a saved material takes over its
//! detected base name on rescan so the two never sit side by side.
//!
//! # The catalog file
//!
//! `assets/materials/` *is* the material index: the file stems are the
//! `@Name`s. `assets/catalog.bsn` holds only what has no file of its own:
//! catalog entries of other asset types, and inline material entries. Inline
//! material entries load normally and are rewritten as `.material.bsn` files
//! on the next save, which removes them from `catalog.bsn`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use bevy::asset::{UntypedAssetId, UntypedHandle};
use bevy::image::ImageLoaderSettings;
use bevy::prelude::*;
use jackdaw_bsn::{BsnPatch, BsnValue, CatalogAssetRef, SceneBsnAst};

use crate::asset_catalog::AssetCatalog;
use crate::prelude::*;
use crate::project::ProjectRoot;

/// Directory under `assets/` holding saved material files.
pub const MATERIALS_DIR: &str = "materials";

/// Suffix identifying a saved material file. The name is the stem before it.
pub const MATERIAL_FILE_SUFFIX: &str = ".material.bsn";

const STANDARD_MATERIAL: &str = "bevy_pbr::pbr_material::StandardMaterial";

/// `StandardMaterial` texture slots holding linear (non-color) data. These
/// must be loaded with `is_srgb = false` before anything else resolves their
/// paths, since the asset server keys images by path and hands out whichever
/// decode was requested first.
const LINEAR_SLOTS: [&str; 4] = [
    "normal_map_texture",
    "metallic_roughness_texture",
    "occlusion_texture",
    "depth_map",
];

/// Material names that are durable: backed by a `.material.bsn` file, or by an
/// inline `catalog.bsn` entry awaiting migration.
///
/// Outlives registry rebuilds, so a rescan keeps a detected set ephemeral and a
/// saved material of the same name saved.
#[derive(Resource, Default)]
pub struct SavedMaterials(pub HashSet<String>);

/// The materials editor surfaces browse, in display order.
///
/// Entries are keyed by `name`; the catalog spells the same identity `@name`.
#[derive(Resource, Default)]
pub struct MaterialRegistry {
    pub entries: Vec<MaterialRegistryEntry>,
}

pub struct MaterialRegistryEntry {
    pub name: String,
    pub handle: Handle<StandardMaterial>,
    /// Whether a `.material.bsn` file backs this entry. Unsaved entries are
    /// usable while the editor runs but are not written by [`persist_materials`].
    pub saved: bool,
}

impl MaterialRegistry {
    pub fn get_by_name(&self, name: &str) -> Option<&MaterialRegistryEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    pub fn name_of(&self, handle: &Handle<StandardMaterial>) -> Option<&str> {
        self.entries
            .iter()
            .find(|e| e.handle == *handle)
            .map(|e| e.name.as_str())
    }

    pub fn is_saved(&self, name: &str) -> bool {
        self.get_by_name(name).is_some_and(|e| e.saved)
    }

    /// Add an unsaved entry (a detected set or a freshly created material).
    pub fn add(&mut self, name: String, handle: Handle<StandardMaterial>) {
        self.entries.push(MaterialRegistryEntry {
            name,
            handle,
            saved: false,
        });
    }

    /// Add an entry backed by a material file (or by an inline catalog entry
    /// awaiting migration).
    pub fn add_saved(&mut self, name: String, handle: Handle<StandardMaterial>) {
        self.entries.push(MaterialRegistryEntry {
            name,
            handle,
            saved: true,
        });
    }

    /// Insert a "None" entry at the top of the list if one isn't already present.
    pub fn ensure_none_entry(&mut self) {
        if !self.entries.iter().any(|e| e.handle == Handle::default()) {
            self.entries.insert(
                0,
                MaterialRegistryEntry {
                    name: "None".to_string(),
                    handle: Handle::default(),
                    saved: true,
                },
            );
        }
    }

    /// Entries that can be named by a durable reference: backed by a file, and
    /// not the "None" placeholder.
    pub fn saved_entries(&self) -> impl Iterator<Item = &MaterialRegistryEntry> {
        self.entries
            .iter()
            .filter(|e| e.saved && e.handle != Handle::default())
    }

    /// The first free `Material_N` name.
    pub fn next_created_name(&self) -> String {
        let mut idx = 1u32;
        loop {
            let candidate = format!("Material_{idx}");
            if self.get_by_name(&candidate).is_none() {
                return candidate;
            }
            idx += 1;
        }
    }
}

/// Strip path separators and other characters that cannot appear in a file
/// stem, so a material name always maps to exactly one file.
pub fn sanitize_material_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "material".to_string()
    } else {
        cleaned
    }
}

/// `<project>/assets/materials`.
pub fn materials_dir(project: &ProjectRoot) -> PathBuf {
    project.assets_dir().join(MATERIALS_DIR)
}

/// The file a material of this name saves to.
pub fn material_file_path(project: &ProjectRoot, name: &str) -> PathBuf {
    materials_dir(project).join(format!(
        "{}{MATERIAL_FILE_SUFFIX}",
        sanitize_material_name(name)
    ))
}

/// Reflect one material out of its `Assets` store as a single-entry `.bsn`
/// document. Texture slots emit as project-relative asset paths.
pub fn material_to_bsn(world: &World, name: &str, asset_id: UntypedAssetId) -> String {
    jackdaw_bsn::serialize_assets_to_bsn(
        world,
        &[CatalogAssetRef {
            name: sanitize_material_name(name),
            type_id: std::any::TypeId::of::<StandardMaterial>(),
            asset_id,
        }],
    )
}

/// Write `assets/materials/<name>.material.bsn` for a live material.
pub fn write_material_file(
    world: &World,
    name: &str,
    handle: &Handle<StandardMaterial>,
) -> std::io::Result<PathBuf> {
    let project = world
        .get_resource::<ProjectRoot>()
        .ok_or_else(|| std::io::Error::other("no project root"))?;
    let path = material_file_path(project, name);
    let text = material_to_bsn(world, name, handle.id().untyped());
    // Skip an identical rewrite: the asset watcher reloads on mtime, and a material apply
    // can flag the catalog dirty many times over.
    if std::fs::read_to_string(&path).is_ok_and(|existing| existing == text) {
        return Ok(path);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::scene_io::save::write_atomic(&path, text.as_bytes())?;
    Ok(path)
}

/// Delete the file backing a material name, if there is one.
pub fn remove_material_file(world: &World, name: &str) {
    let Some(project) = world.get_resource::<ProjectRoot>() else {
        return;
    };
    let path = material_file_path(project, name);
    match std::fs::remove_file(&path) {
        Ok(()) => info!("Removed {}", path.display()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => warn!("Failed to remove {}: {err}", path.display()),
    }
}

/// Every `assets/materials/*.material.bsn` as `(name, path)`, sorted by path
/// so scan order is stable.
pub fn material_files(world: &World) -> Vec<(String, PathBuf)> {
    let Some(project) = world.get_resource::<ProjectRoot>() else {
        return Vec::new();
    };
    let Ok(read_dir) = std::fs::read_dir(materials_dir(project)) else {
        return Vec::new();
    };
    let mut files: Vec<(String, PathBuf)> = read_dir
        .flatten()
        .map(|e| e.path())
        .filter_map(|path| Some((material_name_from_file(&path)?, path)))
        .collect();
    files.sort_by(|a, b| a.1.cmp(&b.1));
    files
}

/// Load one material file. A file that fails to read or parse is reported and
/// skipped; a missing texture path still produces a handle (the asset server
/// surfaces the missing file), so one dead texture never drops the material.
pub fn load_material_file(world: &mut World, path: &Path) -> Option<UntypedHandle> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            warn!("Failed to read {}: {err}", path.display());
            return None;
        }
    };
    let handle = load_material_bsn(world, &text);
    if handle.is_none() {
        warn!("No material found in {}", path.display());
    }
    handle
}

/// Load every `assets/materials/*.material.bsn`, returning `(name, handle)`
/// pairs keyed by file stem.
pub fn load_material_files(world: &mut World) -> Vec<(String, UntypedHandle)> {
    material_files(world)
        .into_iter()
        .filter_map(|(name, path)| Some((name, load_material_file(world, &path)?)))
        .collect()
}

/// What a rescan of `assets/materials` found.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct MaterialRescan {
    /// Names whose file appeared since the last scan. Loaded and marked saved.
    pub added: Vec<String>,
    /// Names whose file has gone. Demoted to unsaved; see
    /// [`rescan_material_files`] for what that keeps and what it drops.
    pub demoted: Vec<String>,
}

/// Rescan `assets/materials` while the editor is up. Files that appeared since
/// the last scan load and register as they would at project open.
///
/// A file that has disappeared demotes its material to *unsaved*:
///
/// - **Memory**: the material is kept. Faces and terrain slots reference it
///   by name, and dropping it would orphan them over a file that may be
///   moving rather than gone.
/// - **Disk**: it is not recreated. Leaving it saved would have the next
///   `persist_materials` write the file back, undoing the user's deletion. A
///   scene that references it embeds it inline, staying self-contained.
/// - **Marker**: it lists as unsaved wherever materials are shown.
///   `material.save` promotes it again and writes a fresh file.
///
/// The material file has no asset-server handle behind it, so a file changed
/// on disk is not reloaded here; its images are, through the asset server's
/// own watcher.
pub fn rescan_material_files(world: &mut World) -> MaterialRescan {
    let files = material_files(world);
    let known = world.resource::<SavedMaterials>().0.clone();
    let on_disk: HashSet<&String> = files.iter().map(|(name, _)| name).collect();

    let mut scan = MaterialRescan::default();
    for gone in known.iter().filter(|name| !on_disk.contains(name)) {
        info!("Material file for '{gone}' is gone; keeping it as an unsaved material");
        world.resource_mut::<SavedMaterials>().0.remove(gone);
        scan.demoted.push(gone.clone());
    }
    scan.demoted.sort();

    for (name, path) in files {
        if known.contains(&name) {
            continue;
        }
        let Some(handle) = load_material_file(world, &path) else {
            continue;
        };
        world
            .resource_mut::<AssetCatalog>()
            .insert(format!("@{name}"), handle);
        world
            .resource_mut::<SavedMaterials>()
            .0
            .insert(name.clone());
        scan.added.push(name);
    }
    if !scan.added.is_empty() {
        info!("Loaded {} new saved materials", scan.added.len());
    }
    scan
}

/// The material name a file path denotes, or `None` if it is not a material
/// file.
pub fn material_name_from_file(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(MATERIAL_FILE_SUFFIX))
        .map(str::to_owned)
}

/// Build a material from `.bsn` text, rehydrating its texture slots through
/// the asset server.
///
/// A material file holds exactly one `StandardMaterial`; anything else in the
/// document is reported and ignored.
pub fn load_material_bsn(world: &mut World, text: &str) -> Option<UntypedHandle> {
    if text.trim().is_empty() {
        return None;
    }
    // Claim the linear slots' images as non-sRGB first; the generic applier below resolves
    // the same paths and gets these handles.
    let _linear = preload_linear_textures(world, text);
    let entries = match jackdaw_bsn::load_bsn_assets(world, text) {
        Ok(entries) => entries,
        Err(err) => {
            warn!("Failed to parse material: {err}");
            return None;
        }
    };
    if entries.len() > 1 {
        warn!(
            "material document holds {} assets; only the first is used",
            entries.len()
        );
    }
    let entry = entries.into_iter().next()?;
    if entry.handle.type_id() != std::any::TypeId::of::<StandardMaterial>() {
        warn!(
            "material document '{}' is not a StandardMaterial",
            entry.name
        );
        return None;
    }
    Some(entry.handle)
}

/// Pre-load the linear-space textures a material's `.bsn` text references with
/// `is_srgb = false`. The returned handles keep the images alive until the
/// material takes its own strong references.
pub(crate) fn preload_linear_textures(world: &mut World, text: &str) -> Vec<UntypedHandle> {
    let Ok(ast) = jackdaw_bsn::parse_bsn_text(text) else {
        return Vec::new();
    };
    let paths = linear_texture_paths(&ast);
    if paths.is_empty() {
        return Vec::new();
    }
    let asset_server = world.resource::<AssetServer>().clone();
    paths
        .into_iter()
        .map(|path| {
            asset_server
                .load_builder()
                .with_settings(|s: &mut ImageLoaderSettings| s.is_srgb = false)
                .load::<Image>(&path)
                .untyped()
        })
        .collect()
}

/// Asset paths bound to a `StandardMaterial`'s linear texture slots anywhere
/// in the document.
fn linear_texture_paths(ast: &SceneBsnAst) -> Vec<String> {
    let mut paths = Vec::new();
    for &root in &ast.roots {
        let Some(patches) = ast.get_patches(root) else {
            continue;
        };
        for &pe in &patches.0 {
            let Some(BsnPatch::Struct(data)) = ast.get_patch(pe) else {
                continue;
            };
            if data.type_path != STANDARD_MATERIAL {
                continue;
            }
            for field in &data.fields.0 {
                if LINEAR_SLOTS.contains(&field.name.as_str())
                    && let BsnValue::String(path) = &field.value
                    && !path.is_empty()
                    && !path.starts_with('@')
                    && !path.starts_with('#')
                {
                    paths.push(path.clone());
                }
            }
        }
    }
    paths
}

/// Write a `.material.bsn` for every saved registry entry. Unsaved entries are
/// skipped; they stay ephemeral until `material.save` runs.
///
/// Returns the asset ids that have files, so the catalog writer knows which
/// entries it need not hold inline. Keyed by id, not name, so an entry of
/// another type sharing a material's name is never mistaken for it.
///
/// A name that would change under [`sanitize_material_name`] cannot become a
/// file stem without changing the `@Name` scenes reference, so it is reported
/// and left inline.
pub fn persist_materials(world: &mut World) -> Vec<UntypedAssetId> {
    let saved: Vec<(String, Handle<StandardMaterial>)> = world
        .resource::<MaterialRegistry>()
        .entries
        .iter()
        .filter(|e| e.saved && e.handle != Handle::default())
        .map(|e| (e.name.clone(), e.handle.clone()))
        .collect();

    let mut written = Vec::new();
    for (name, handle) in saved {
        if sanitize_material_name(&name) != name {
            warn!("Material '{name}' has no valid file name; keeping it in the catalog");
            continue;
        }
        match write_material_file(world, &name, &handle) {
            Ok(_) => written.push(handle.id().untyped()),
            Err(err) => warn!("Failed to write material '{name}': {err}"),
        }
    }
    written
}

/// Asset ids of materials with no file behind them. A scene that references
/// one must embed it inline: an `@Name` reference would resolve to nothing
/// outside this editor run.
pub fn ephemeral_material_ids(world: &World) -> std::collections::HashSet<UntypedAssetId> {
    world
        .get_resource::<MaterialRegistry>()
        .map(|registry| {
            registry
                .entries
                .iter()
                .filter(|entry| !entry.saved && entry.handle != Handle::default())
                .map(|entry| entry.handle.id().untyped())
                .collect()
        })
        .unwrap_or_default()
}

// -- Browsing grid ----------------------------------------------------------

/// The image a material browses as: its base colour texture.
pub fn material_thumbnail(
    materials: &Assets<StandardMaterial>,
    handle: &Handle<StandardMaterial>,
) -> Option<Handle<Image>> {
    materials
        .get(handle)
        .and_then(|m| m.base_color_texture.clone())
}

/// One material's tile in a browsing grid, shared by the Materials panel and
/// the terrain Textures tab.
pub struct MaterialTile {
    pub name: String,
    pub thumbnail: Option<Handle<Image>>,
    pub saved: bool,
    pub selected: bool,
    /// Font unsaved names render in.
    pub italic_font: Handle<Font>,
}

/// Longest name a tile shows before eliding; the full name goes in a tooltip.
const TILE_NAME_LIMIT: usize = 10;

/// Spawn a material tile under `parent` and return it, leaving the caller to
/// attach its own click handling.
pub fn spawn_material_tile(commands: &mut Commands, parent: Entity, tile: MaterialTile) -> Entity {
    use bevy::picking::hover::Hovered;
    use bevy::text::FontSource;
    use jackdaw_feathers::{tokens, tooltip::Tooltip};

    let cell = commands
        .spawn((
            Node {
                width: px(tokens::THUMB_CELL_WIDTH),
                height: px(tokens::THUMB_CELL_HEIGHT),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                padding: UiRect::all(px(2.0)),
                border: UiRect::all(px(1.0)),
                border_radius: BorderRadius::all(px(4.0)),
                ..default()
            },
            BorderColor::all(if tile.selected {
                tokens::ACCENT_BLUE
            } else {
                Color::NONE
            }),
            BackgroundColor(Color::NONE),
            ChildOf(parent),
        ))
        .id();

    let mut swatch = commands.spawn((
        Node {
            width: px(tokens::THUMB_IMAGE_SIZE),
            height: px(tokens::THUMB_IMAGE_SIZE),
            ..default()
        },
        BackgroundColor(tokens::INPUT_BG),
        ChildOf(cell),
    ));
    if let Some(image) = tile.thumbnail {
        swatch.insert(ImageNode::new(image));
    }

    let elided = tile.name.chars().count() > TILE_NAME_LIMIT;
    let shown = if elided {
        format!("{}...", tile.name.chars().take(8).collect::<String>())
    } else {
        tile.name.clone()
    };
    let mut label = commands.spawn((
        Text::new(shown),
        TextFont {
            font: if tile.saved {
                FontSource::default()
            } else {
                FontSource::Handle(tile.italic_font)
            },
            font_size: tokens::TEXT_SIZE_XS,
            ..default()
        },
        TextColor(if tile.selected {
            tokens::TEXT_BODY_COLOR.into()
        } else {
            tokens::TEXT_SECONDARY
        }),
        Node {
            max_width: px(tokens::THUMB_NAME_MAX_WIDTH),
            overflow: Overflow::clip(),
            ..default()
        },
        ChildOf(cell),
    ));
    if !tile.saved {
        label.insert((
            Hovered::default(),
            Tooltip::title(format!("{} (unsaved)", tile.name)),
        ));
    } else if elided {
        label.insert((Hovered::default(), Tooltip::title(tile.name)));
    }

    let selected = tile.selected;
    commands.entity(cell).observe(
        move |hover: On<Pointer<Over>>, mut borders: Query<&mut BorderColor>| {
            if let Ok(mut border) = borders.get_mut(hover.event_target()) {
                *border = BorderColor::all(tokens::SELECTED_BORDER);
            }
        },
    );
    commands.entity(cell).observe(
        move |out: On<Pointer<Out>>, mut borders: Query<&mut BorderColor>| {
            if let Ok(mut border) = borders.get_mut(out.event_target()) {
                *border = BorderColor::all(if selected {
                    tokens::ACCENT_BLUE
                } else {
                    Color::NONE
                });
            }
        },
    );

    cell
}

// -- Operators --------------------------------------------------------------

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<MaterialSaveOp>()
        .register_operator::<MaterialDeleteOp>();
}

pub(crate) fn plugin(app: &mut App) {
    // The retag that makes 16-bit maps bindable comes from the runtime, so the editor and
    // the built game agree on it.
    app.add_plugins(jackdaw_runtime::MaterialTextureFormatPlugin)
        .init_resource::<PendingMaterialDelete>()
        .add_observer(on_delete_dialog_opened)
        .add_observer(on_delete_dialog_closed)
        .add_observer(on_material_delete_confirmed);
}

fn a_material_is_selected(
    preview: Option<Res<crate::material_preview::MaterialPreviewState>>,
) -> bool {
    preview.is_some_and(|p| {
        p.active_material
            .as_ref()
            .is_some_and(|h| *h != Handle::default())
    })
}

/// Write the target material to `assets/materials` and promote it to a saved
/// asset. Both parameters are optional so a surface can dispatch this with no
/// arguments for the previewed material.
///
/// Registry key, file stem and `@Name` are the same string, so a call naming a
/// material another entry already answers to is refused.
#[operator(
    id = "material.save",
    label = "Save Material",
    description = "Write the material to assets/materials as a reusable asset.",
    allows_undo = false,
    is_available = a_material_is_selected,
    params(
        material(String, doc = "Name of the material to save. Defaults to the previewed one."),
        name(String, doc = "Name to save under. Defaults to the material's current name.")
    )
)]
pub fn material_save(
    params: In<OperatorParameters>,
    registry: Res<MaterialRegistry>,
    preview: Option<Res<crate::material_preview::MaterialPreviewState>>,
    mut commands: Commands,
) -> OperatorResult {
    let handle = match params.as_str("material") {
        Some(name) => registry.get_by_name(name).map(|e| e.handle.clone()),
        None => preview.and_then(|p| p.active_material.clone()),
    };
    let Some(handle) = handle.filter(|h| *h != Handle::default()) else {
        warn!("material.save: no material to save");
        return OperatorResult::Cancelled;
    };

    let current = registry.name_of(&handle).map(str::to_owned);
    let name = sanitize_material_name(
        params
            .as_str("name")
            .or(current.as_deref())
            .unwrap_or("material"),
    );

    if let Some(owner) = name_owner(&registry, &handle, &name) {
        warn!("material.save: '{name}' already belongs to material '{owner}'");
        return OperatorResult::Cancelled;
    }

    commands.queue(move |world: &mut World| {
        write_and_promote(world, &handle, current.as_deref(), &name);
    });
    OperatorResult::Finished
}

/// The material already answering to `name`, if it is not `handle` itself.
///
/// Compared case-insensitively, as a case-insensitive file system would, so two
/// display names cannot race for one file stem.
fn name_owner(
    registry: &MaterialRegistry,
    handle: &Handle<StandardMaterial>,
    name: &str,
) -> Option<String> {
    registry
        .entries
        .iter()
        .find(|e| e.handle != *handle && e.name.eq_ignore_ascii_case(name))
        .map(|e| e.name.clone())
}

/// Write the material's file and make `name` its identity everywhere.
///
/// A rename removes the file the old name held and keeps `@old` resolving to
/// the same handle for the rest of this editor run, so open scenes do not lose
/// their material between the rename and their next save.
fn write_and_promote(
    world: &mut World,
    handle: &Handle<StandardMaterial>,
    current: Option<&str>,
    name: &str,
) {
    // The dispatching check ran before this command was queued, so a second save aimed at
    // the same name may have landed in between; re-check at the point of claiming it.
    if let Some(owner) = name_owner(world.resource::<MaterialRegistry>(), handle, name) {
        warn!("material.save: '{name}' already belongs to material '{owner}'");
        return;
    }

    if let Err(err) = write_material_file(world, name, handle) {
        warn!("material.save: failed to write '{name}': {err}");
        return;
    }

    let renamed = current.is_some_and(|old| old != name);
    if renamed && let Some(old) = current {
        remove_material_file(world, old);
        world.resource_mut::<SavedMaterials>().0.remove(old);
    }

    let mut registry = world.resource_mut::<MaterialRegistry>();
    if let Some(entry) = registry.entries.iter_mut().find(|e| e.handle == *handle) {
        entry.name = name.to_owned();
        entry.saved = true;
    } else {
        registry.add_saved(name.to_owned(), handle.clone());
    }

    // `insert` repoints `id_to_name` at the new name; the old key stays in `handles` alone,
    // as a lookup alias with no claim on save output.
    world
        .resource_mut::<AssetCatalog>()
        .insert(format!("@{name}"), handle.clone().untyped());
    world.resource_mut::<AssetCatalog>().dirty = true;
    world
        .resource_mut::<SavedMaterials>()
        .0
        .insert(name.to_owned());
    info!("Saved material '{name}'");
}

/// The material a confirmed delete will remove, and the dialog asking about it.
///
/// Every dialog in the editor raises the same action event, so the entity
/// distinguishes this delete's confirmation from another dialog's.
#[derive(Resource, Default)]
pub struct PendingMaterialDelete {
    pub name: Option<String>,
    /// The dialog entity, once it has spawned. Only an action from *this*
    /// entity may act on `name`.
    pub dialog: Option<Entity>,
}

impl PendingMaterialDelete {
    fn disarm(&mut self) {
        self.name = None;
        self.dialog = None;
    }
}

/// Delete a saved material: its file, its registry entry and its catalog name
/// all go together.
///
/// Guarded by the shared confirmation dialog rather than by reference counting.
/// Faces hold material *handles*, not names, so they keep drawing what they
/// were given for the rest of this run; a terrain slot naming this material
/// reports it as missing and keeps its texture id, so nothing painted moves.
///
/// Refused while another dialog is open: the confirmation infrastructure shows
/// one at a time, so arming against a dialog that never appears would leave
/// this pointing at whatever opened next.
#[operator(
    id = "material.delete",
    label = "Delete Material",
    description = "Delete a saved material's file and remove it from this project.",
    allows_undo = false,
    is_available = a_material_is_selected,
    params(material(
        String,
        doc = "Name of the material to delete. Defaults to the previewed one."
    ))
)]
pub fn material_delete(
    params: In<OperatorParameters>,
    registry: Res<MaterialRegistry>,
    preview: Option<Res<crate::material_preview::MaterialPreviewState>>,
    open_dialogs: Query<(), With<jackdaw_feathers::dialog::EditorDialog>>,
    mut pending: ResMut<PendingMaterialDelete>,
    mut commands: Commands,
) -> OperatorResult {
    if !open_dialogs.is_empty() {
        warn!("material.delete: another dialog is already open");
        return OperatorResult::Cancelled;
    }
    let name = match params.as_str("material") {
        Some(name) => Some(name.to_string()),
        None => preview
            .and_then(|p| p.active_material.clone())
            .filter(|h| *h != Handle::default())
            .and_then(|handle| registry.name_of(&handle).map(str::to_owned)),
    };
    let Some(name) = name else {
        warn!("material.delete: no material to delete");
        return OperatorResult::Cancelled;
    };
    if !registry.is_saved(&name) {
        warn!("material.delete: '{name}' has no file to delete");
        return OperatorResult::Cancelled;
    }

    pending.name = Some(name.clone());
    pending.dialog = None;
    commands.trigger(
        jackdaw_feathers::dialog::OpenConfirmationDialogEvent::new("Delete material", "Delete")
            .with_description(format!(
                "Delete the material '{name}'? Terrains and faces that reference it \
                 lose it."
            )),
    );
    OperatorResult::Finished
}

/// Claim the dialog this delete armed against, as it spawns.
///
/// The open event is a command, so the entity does not exist when the operator
/// returns. The operator refuses while another dialog is open, so the next
/// dialog to appear is this one.
fn on_delete_dialog_opened(
    event: On<Add, jackdaw_feathers::dialog::EditorDialog>,
    mut pending: ResMut<PendingMaterialDelete>,
) {
    if pending.name.is_some() && pending.dialog.is_none() {
        pending.dialog = Some(event.entity);
    }
}

/// Disarm when the dialog goes away for any reason.
///
/// Cancel, the close button, the backdrop and Escape all despawn it without an
/// action event. A confirmation despawns it too, but triggers the action event
/// first, which takes the name before this runs.
fn on_delete_dialog_closed(
    event: On<Remove, jackdaw_feathers::dialog::EditorDialog>,
    mut pending: ResMut<PendingMaterialDelete>,
) {
    if pending.dialog == Some(event.entity) {
        pending.disarm();
    }
}

fn on_material_delete_confirmed(
    event: On<jackdaw_feathers::dialog::DialogActionEvent>,
    mut pending: ResMut<PendingMaterialDelete>,
    mut commands: Commands,
) {
    if pending.dialog != Some(event.entity) {
        return;
    }
    let Some(name) = pending.name.take() else {
        return;
    };
    pending.disarm();
    commands.queue(move |world: &mut World| delete_material(world, &name));
}

/// Remove every trace of a material name from this project.
fn delete_material(world: &mut World, name: &str) {
    remove_material_file(world, name);
    world.resource_mut::<SavedMaterials>().0.remove(name);

    let mut registry = world.resource_mut::<MaterialRegistry>();
    let removed = registry
        .entries
        .iter()
        .position(|entry| entry.name == name)
        .map(|at| registry.entries.remove(at));

    let mut catalog = world.resource_mut::<AssetCatalog>();
    // The name and the handle are removed separately: a rename leaves the old name as a
    // lookup alias on the same handle, and both keys have to go or `@old` keeps resolving
    // to a material with no file.
    if let Some(handle) = catalog.handles.remove(&format!("@{name}")) {
        catalog.id_to_name.remove(&handle.id());
    }
    if let Some(entry) = &removed {
        let id = entry.handle.id().untyped();
        catalog.id_to_name.remove(&id);
        catalog.handles.retain(|_, handle| handle.id() != id);
    }
    catalog.dirty = true;

    if let Some(entry) = removed
        && let Some(mut preview) =
            world.get_resource_mut::<crate::material_preview::MaterialPreviewState>()
        && preview.active_material.as_ref() == Some(&entry.handle)
    {
        preview.active_material = None;
    }
    info!("Deleted material '{name}'");
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::AssetPlugin;

    fn material_app() -> App {
        let mut app = App::new();
        app.add_plugins((bevy::app::TaskPoolPlugin::default(), AssetPlugin::default()));
        app.init_asset::<Image>();
        app.init_asset::<StandardMaterial>();
        app.register_asset_reflect::<Image>();
        app.register_asset_reflect::<StandardMaterial>();
        app.register_type::<StandardMaterial>();
        app
    }

    fn textured(app: &mut App, path: &str, srgb: bool) -> Handle<Image> {
        let server = app.world().resource::<AssetServer>().clone();
        let path = path.to_owned();
        if srgb {
            server.load::<Image>(path)
        } else {
            server
                .load_builder()
                .with_settings(|s: &mut ImageLoaderSettings| s.is_srgb = false)
                .load::<Image>(path)
        }
    }

    fn slot_path(app: &App, handle: Option<&Handle<Image>>) -> Option<String> {
        let server = app.world().resource::<AssetServer>();
        handle
            .and_then(|h| server.get_path(h.id()))
            .map(|p| p.to_string().replace('\\', "/"))
    }

    fn round_trip(app: &mut App, material: StandardMaterial) -> StandardMaterial {
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(material);
        let text = material_to_bsn(app.world(), "probe", handle.id().untyped());
        let loaded = load_material_bsn(app.world_mut(), &text).expect("material reloads");
        app.world()
            .resource::<Assets<StandardMaterial>>()
            .get(&loaded.typed::<StandardMaterial>())
            .expect("loaded material")
            .clone()
    }

    #[test]
    fn all_six_texture_slots_and_scalars_survive_a_round_trip() {
        let mut app = material_app();
        let source = StandardMaterial {
            base_color_texture: Some(textured(&mut app, "t/base.png", true)),
            normal_map_texture: Some(textured(&mut app, "t/normal.png", false)),
            metallic_roughness_texture: Some(textured(&mut app, "t/rough.png", false)),
            emissive_texture: Some(textured(&mut app, "t/emit.png", true)),
            occlusion_texture: Some(textured(&mut app, "t/ao.png", false)),
            depth_map: Some(textured(&mut app, "t/height.png", false)),
            metallic: 0.25,
            perceptual_roughness: 0.75,
            reflectance: 0.125,
            parallax_depth_scale: 0.05,
            max_parallax_layer_count: 32.0,
            ..default()
        };

        let text = {
            let handle = app
                .world_mut()
                .resource_mut::<Assets<StandardMaterial>>()
                .add(source.clone());
            material_to_bsn(app.world(), "probe", handle.id().untyped())
        };
        for path in [
            "t/base.png",
            "t/normal.png",
            "t/rough.png",
            "t/emit.png",
            "t/ao.png",
            "t/height.png",
        ] {
            assert!(
                text.contains(path),
                "slot must serialize as the project-relative path {path}, got:\n{text}"
            );
        }

        let loaded = round_trip(&mut app, source);

        assert_eq!(
            slot_path(&app, loaded.base_color_texture.as_ref()).as_deref(),
            Some("t/base.png")
        );
        assert_eq!(
            slot_path(&app, loaded.normal_map_texture.as_ref()).as_deref(),
            Some("t/normal.png")
        );
        assert_eq!(
            slot_path(&app, loaded.metallic_roughness_texture.as_ref()).as_deref(),
            Some("t/rough.png")
        );
        assert_eq!(
            slot_path(&app, loaded.emissive_texture.as_ref()).as_deref(),
            Some("t/emit.png")
        );
        assert_eq!(
            slot_path(&app, loaded.occlusion_texture.as_ref()).as_deref(),
            Some("t/ao.png")
        );
        assert_eq!(
            slot_path(&app, loaded.depth_map.as_ref()).as_deref(),
            Some("t/height.png")
        );

        assert!((loaded.metallic - 0.25).abs() < f32::EPSILON);
        assert!((loaded.perceptual_roughness - 0.75).abs() < f32::EPSILON);
        assert!((loaded.reflectance - 0.125).abs() < f32::EPSILON);
        assert!((loaded.parallax_depth_scale - 0.05).abs() < f32::EPSILON);
        assert!((loaded.max_parallax_layer_count - 32.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_material_with_no_textures_survives_a_round_trip() {
        let mut app = material_app();
        let loaded = round_trip(
            &mut app,
            StandardMaterial {
                perceptual_roughness: 0.4,
                ..default()
            },
        );
        assert!(loaded.base_color_texture.is_none());
        assert!(loaded.normal_map_texture.is_none());
        assert!(loaded.depth_map.is_none());
        assert!((loaded.perceptual_roughness - 0.4).abs() < f32::EPSILON);
    }

    #[test]
    fn a_missing_texture_file_loads_without_panicking_and_keeps_its_path() {
        let mut app = material_app();
        let text = "#probe\nbevy_pbr::pbr_material::StandardMaterial {\n\
                    base_color_texture: \"t/does_not_exist.png\",\n}\n";
        let handle = load_material_bsn(app.world_mut(), text).expect("material still loads");
        let loaded = app
            .world()
            .resource::<Assets<StandardMaterial>>()
            .get(&handle.typed::<StandardMaterial>())
            .expect("loaded material")
            .clone();
        assert_eq!(
            slot_path(&app, loaded.base_color_texture.as_ref()).as_deref(),
            Some("t/does_not_exist.png"),
            "the unresolved path must stay visible on the material"
        );
    }

    #[test]
    fn malformed_material_text_returns_none() {
        let mut app = material_app();
        assert!(load_material_bsn(app.world_mut(), "not $$ bsn {{{").is_none());
        assert!(load_material_bsn(app.world_mut(), "   ").is_none());
    }

    #[test]
    fn names_sanitize_to_one_file_each() {
        assert_eq!(sanitize_material_name("grass_05"), "grass_05");
        assert_eq!(sanitize_material_name("a/b"), "a_b");
        assert_eq!(sanitize_material_name("  "), "material");
        assert_eq!(sanitize_material_name("../escape"), ".._escape");
    }

    #[test]
    fn material_file_names_round_trip_through_their_stem() {
        let path = PathBuf::from("/p/assets/materials/grass_05.material.bsn");
        assert_eq!(material_name_from_file(&path).as_deref(), Some("grass_05"));
        assert_eq!(
            material_name_from_file(Path::new("/p/assets/catalog.bsn")),
            None
        );
    }

    fn params(pairs: &[(&str, &str)]) -> OperatorParameters {
        let mut params = OperatorParameters::default();
        for (key, value) in pairs {
            params.insert(
                (*key).to_string(),
                jackdaw_api::scene::PropertyValue::String((*value).to_string().into()),
            );
        }
        params
    }

    fn project_app() -> (App, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut app = material_app();
        app.insert_resource(ProjectRoot {
            root: tmp.path().to_path_buf(),
            config: crate::project::ProjectConfig::default(),
        });
        app.init_resource::<MaterialRegistry>();
        app.init_resource::<SavedMaterials>();
        app.init_resource::<AssetCatalog>();
        (app, tmp)
    }

    #[test]
    fn saving_a_detected_material_writes_a_file_and_promotes_it() {
        let (mut app, tmp) = project_app();
        let handle = {
            let base = textured(&mut app, "t/grass_basecolor.png", true);
            app.world_mut()
                .resource_mut::<Assets<StandardMaterial>>()
                .add(StandardMaterial {
                    base_color_texture: Some(base),
                    ..default()
                })
        };
        app.world_mut()
            .resource_mut::<MaterialRegistry>()
            .add("grass".into(), handle.clone());

        write_and_promote(app.world_mut(), &handle, Some("grass"), "grass");

        let path = tmp.path().join("assets/materials/grass.material.bsn");
        assert!(path.is_file(), "the material file must exist at {path:?}");
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("t/grass_basecolor.png")
        );
        assert!(app.world().resource::<MaterialRegistry>().is_saved("grass"));
        assert!(
            app.world().resource::<SavedMaterials>().0.contains("grass"),
            "the name must stay durable across registry rebuilds"
        );
        assert!(
            app.world()
                .resource::<AssetCatalog>()
                .contains_name("@grass"),
            "scene face references must keep resolving by name"
        );
    }

    #[test]
    fn saving_over_another_materials_name_is_refused() {
        let (mut app, tmp) = project_app();
        let (detected, fresh) = {
            let mut materials = app.world_mut().resource_mut::<Assets<StandardMaterial>>();
            (
                materials.add(StandardMaterial::default()),
                materials.add(StandardMaterial::default()),
            )
        };
        {
            let mut registry = app.world_mut().resource_mut::<MaterialRegistry>();
            registry.add("grass".into(), detected);
            registry.add("Material_1".into(), fresh);
        }

        let result = app
            .world_mut()
            .run_system_cached_with(
                material_save,
                params(&[("material", "Material_1"), ("name", "GRASS")]),
            )
            .expect("operator runs");

        assert!(
            matches!(result, OperatorResult::Cancelled),
            "a name another material answers to must not be taken"
        );
        assert!(
            !tmp.path()
                .join("assets/materials/GRASS.material.bsn")
                .exists(),
            "a refused save must not leave a file behind"
        );
        assert!(!app.world().resource::<MaterialRegistry>().is_saved("grass"));
    }

    #[test]
    fn a_queued_save_re_checks_the_name_it_was_cleared_for() {
        let (mut app, _tmp) = project_app();
        let (first, second) = {
            let mut materials = app.world_mut().resource_mut::<Assets<StandardMaterial>>();
            (
                materials.add(StandardMaterial::default()),
                materials.add(StandardMaterial::default()),
            )
        };
        {
            let mut registry = app.world_mut().resource_mut::<MaterialRegistry>();
            registry.add("Material_1".into(), first.clone());
            registry.add("Material_2".into(), second.clone());
        }

        // Both dispatches cleared the name against the same registry; only the first may
        // take it.
        write_and_promote(app.world_mut(), &first, Some("Material_1"), "grass");
        write_and_promote(app.world_mut(), &second, Some("Material_2"), "grass");

        let registry = app.world().resource::<MaterialRegistry>();
        assert_eq!(
            registry
                .entries
                .iter()
                .filter(|e| e.name == "grass")
                .count(),
            1,
            "one name must never end up on two entries"
        );
        assert_eq!(registry.name_of(&first), Some("grass"));
        assert_eq!(registry.name_of(&second), Some("Material_2"));
        assert!(!registry.is_saved("Material_2"));
    }

    #[test]
    fn renaming_a_material_moves_its_file_and_keeps_the_old_name_resolving() {
        let (mut app, tmp) = project_app();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        app.world_mut()
            .resource_mut::<MaterialRegistry>()
            .add("grass".into(), handle.clone());

        write_and_promote(app.world_mut(), &handle, Some("grass"), "grass");
        assert!(
            tmp.path()
                .join("assets/materials/grass.material.bsn")
                .is_file()
        );

        write_and_promote(app.world_mut(), &handle, Some("grass"), "meadow");

        assert!(
            !tmp.path()
                .join("assets/materials/grass.material.bsn")
                .exists(),
            "the file the old name held must go with the name"
        );
        assert!(
            tmp.path()
                .join("assets/materials/meadow.material.bsn")
                .is_file()
        );

        let catalog = app.world().resource::<AssetCatalog>();
        assert_eq!(
            catalog
                .handles
                .get("@grass")
                .map(bevy::prelude::UntypedHandle::id),
            Some(handle.id().untyped()),
            "open scenes must keep resolving the name they were saved with"
        );
        assert_eq!(
            catalog
                .id_to_name
                .get(&handle.id().untyped())
                .map(String::as_str),
            Some("@meadow"),
            "new saves must emit the new name"
        );
        let saved = app.world().resource::<SavedMaterials>();
        assert!(saved.0.contains("meadow"));
        assert!(!saved.0.contains("grass"));
    }

    #[test]
    fn a_name_that_is_not_a_legal_file_stem_stays_in_the_catalog() {
        let (mut app, tmp) = project_app();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        app.world_mut()
            .resource_mut::<MaterialRegistry>()
            .add_saved("my material".into(), handle);

        assert!(
            persist_materials(app.world_mut()).is_empty(),
            "migrating would silently rename the material"
        );
        assert!(!tmp.path().join("assets/materials").exists());
    }

    #[test]
    fn ephemeral_ids_name_exactly_the_unsaved_materials() {
        let (mut app, _tmp) = project_app();
        let (unsaved, saved) = {
            let mut materials = app.world_mut().resource_mut::<Assets<StandardMaterial>>();
            (
                materials.add(StandardMaterial::default()),
                materials.add(StandardMaterial::default()),
            )
        };
        {
            let mut registry = app.world_mut().resource_mut::<MaterialRegistry>();
            registry.add("detected".into(), unsaved.clone());
            registry.add_saved("promoted".into(), saved.clone());
            registry.ensure_none_entry();
        }

        let ids = ephemeral_material_ids(app.world());
        assert!(ids.contains(&unsaved.id().untyped()));
        assert!(!ids.contains(&saved.id().untyped()));
        assert_eq!(ids.len(), 1, "the None entry is not a material to embed");
    }

    #[test]
    fn saved_material_files_reload_by_their_stem() {
        let (mut app, tmp) = project_app();
        let handle = {
            let normal = textured(&mut app, "t/rock_normal.png", false);
            app.world_mut()
                .resource_mut::<Assets<StandardMaterial>>()
                .add(StandardMaterial {
                    normal_map_texture: Some(normal),
                    perceptual_roughness: 0.31,
                    ..default()
                })
        };
        write_material_file(app.world(), "rock", &handle).expect("write");

        let loaded = load_material_files(app.world_mut());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].0, "rock");
        let material = app
            .world()
            .resource::<Assets<StandardMaterial>>()
            .get(&loaded[0].1.clone().typed::<StandardMaterial>())
            .expect("reloaded material")
            .clone();
        assert!((material.perceptual_roughness - 0.31).abs() < f32::EPSILON);
        assert_eq!(
            slot_path(&app, material.normal_map_texture.as_ref()).as_deref(),
            Some("t/rock_normal.png")
        );
        assert!(tmp.path().join("assets/materials").is_dir());
    }

    #[test]
    fn unsaved_materials_are_not_written() {
        let (mut app, tmp) = project_app();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        app.world_mut()
            .resource_mut::<MaterialRegistry>()
            .add("detected".into(), handle);

        assert!(persist_materials(app.world_mut()).is_empty());
        assert!(
            !tmp.path()
                .join("assets/materials/detected.material.bsn")
                .exists()
        );
    }

    /// Without a rescan a file written while the editor is up stays invisible until the
    /// project is reopened.
    #[test]
    fn a_file_that_appeared_after_load_registers_on_rescan() {
        let (mut app, _tmp) = project_app();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                perceptual_roughness: 0.42,
                ..default()
            });
        write_material_file(app.world(), "slate", &handle).expect("write");
        assert!(
            !app.world().resource::<SavedMaterials>().0.contains("slate"),
            "nothing has scanned yet",
        );

        let scan = rescan_material_files(app.world_mut());

        assert_eq!(scan.added, vec!["slate".to_string()]);
        assert!(scan.demoted.is_empty());
        assert!(app.world().resource::<SavedMaterials>().0.contains("slate"));
        let loaded = app
            .world()
            .resource::<AssetCatalog>()
            .handles
            .get("@slate")
            .expect("catalog entry")
            .clone()
            .typed::<StandardMaterial>();
        let roughness = app
            .world()
            .resource::<Assets<StandardMaterial>>()
            .get(&loaded)
            .expect("loaded material")
            .perceptual_roughness;
        assert!((roughness - 0.42).abs() < f32::EPSILON);

        assert_eq!(
            rescan_material_files(app.world_mut()),
            MaterialRescan::default(),
            "a second rescan re-registers nothing",
        );
    }

    /// Write a material, scan it in, then delete its file behind the editor's back.
    fn deleted_behind_our_back(name: &str) -> (App, tempfile::TempDir) {
        let (mut app, tmp) = project_app();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        write_material_file(app.world(), name, &handle).expect("write");
        rescan_material_files(app.world_mut());
        app.world_mut()
            .resource_mut::<MaterialRegistry>()
            .add_saved(name.to_string(), handle);

        std::fs::remove_file(
            tmp.path()
                .join(format!("assets/materials/{name}{MATERIAL_FILE_SUFFIX}")),
        )
        .expect("remove");
        (app, tmp)
    }

    /// Faces and terrain slots reference a material by name, so a file deleted out from
    /// under a running editor must not take the loaded material with it.
    #[test]
    fn a_deleted_file_demotes_its_material_instead_of_dropping_it() {
        let (mut app, _tmp) = deleted_behind_our_back("slate");

        let scan = rescan_material_files(app.world_mut());

        assert_eq!(scan.demoted, vec!["slate".to_string()]);
        assert!(scan.added.is_empty());
        assert!(
            app.world()
                .resource::<AssetCatalog>()
                .handles
                .contains_key("@slate"),
            "the loaded material outlives its file",
        );
        assert!(
            !app.world().resource::<SavedMaterials>().0.contains("slate"),
            "nothing on disk backs it any more",
        );
    }

    /// Leaving a vanished material marked saved would have the next persist write its file
    /// back, undoing the user's deletion.
    #[test]
    fn a_deleted_file_is_not_written_back_by_the_next_persist() {
        let (mut app, tmp) = deleted_behind_our_back("slate");
        rescan_material_files(app.world_mut());
        // The browser rebuilds the registry from `SavedMaterials`; stand in for that here.
        app.world_mut().resource_mut::<MaterialRegistry>().entries = Vec::new();
        let handle = app
            .world()
            .resource::<AssetCatalog>()
            .handles
            .get("@slate")
            .expect("catalog entry")
            .clone()
            .typed::<StandardMaterial>();
        app.world_mut()
            .resource_mut::<MaterialRegistry>()
            .add("slate".to_string(), handle.clone());

        assert!(persist_materials(app.world_mut()).is_empty());
        assert!(
            !tmp.path()
                .join("assets/materials/slate.material.bsn")
                .exists(),
            "the deletion stands",
        );

        // An explicit save writes the file again.
        write_and_promote(app.world_mut(), &handle, Some("slate"), "slate");
        assert!(app.world().resource::<SavedMaterials>().0.contains("slate"));
        assert!(
            tmp.path()
                .join("assets/materials/slate.material.bsn")
                .exists()
        );
    }

    /// Deleting leaves nothing that could still resolve: the file, the durable-name set, the
    /// registry entry and both catalog keys all go together.
    #[test]
    fn deleting_a_material_removes_its_file_and_every_name_that_resolved_to_it() {
        let (mut app, tmp) = project_app();
        app.init_resource::<PendingMaterialDelete>();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        app.world_mut()
            .resource_mut::<MaterialRegistry>()
            .add("grass".into(), handle.clone());
        write_and_promote(app.world_mut(), &handle, Some("grass"), "grass");
        // A rename leaves the old name behind as an alias; both keys have to go.
        write_and_promote(app.world_mut(), &handle, Some("grass"), "meadow");

        delete_material(app.world_mut(), "meadow");

        assert!(
            !tmp.path()
                .join("assets/materials/meadow.material.bsn")
                .exists()
        );
        assert!(
            !app.world()
                .resource::<SavedMaterials>()
                .0
                .contains("meadow")
        );
        assert!(
            app.world()
                .resource::<MaterialRegistry>()
                .get_by_name("meadow")
                .is_none()
        );
        let catalog = app.world().resource::<AssetCatalog>();
        assert!(!catalog.contains_name("@meadow"));
        assert!(
            !catalog.handles.contains_key("@grass"),
            "the alias must not outlive the material it aliased"
        );
        assert!(!catalog.id_to_name.contains_key(&handle.id().untyped()));
    }

    /// The observer is global, so without the entity check a confirmation from any later
    /// dialog would spend this armed delete.
    #[test]
    fn a_confirmation_from_another_dialog_leaves_the_armed_delete_alone() {
        let (mut app, _tmp) = project_app();
        app.init_resource::<PendingMaterialDelete>();
        app.add_observer(on_material_delete_confirmed);

        let ours = app.world_mut().spawn_empty().id();
        let theirs = app.world_mut().spawn_empty().id();
        {
            let mut pending = app.world_mut().resource_mut::<PendingMaterialDelete>();
            pending.name = Some("grass".to_string());
            pending.dialog = Some(ours);
        }

        app.world_mut()
            .trigger(jackdaw_feathers::dialog::DialogActionEvent { entity: theirs });
        assert_eq!(
            app.world()
                .resource::<PendingMaterialDelete>()
                .name
                .as_deref(),
            Some("grass"),
            "an unrelated confirmation must not consume this delete",
        );

        app.world_mut()
            .trigger(jackdaw_feathers::dialog::DialogActionEvent { entity: ours });
        assert!(
            app.world()
                .resource::<PendingMaterialDelete>()
                .name
                .is_none(),
            "its own confirmation does consume it",
        );
    }

    /// Cancelling, closing, clicking away and pressing Escape all despawn the dialog without
    /// an action event, and each has to disarm.
    #[test]
    fn dismissing_the_dialog_without_confirming_disarms_the_delete() {
        let (mut app, _tmp) = project_app();
        app.init_resource::<PendingMaterialDelete>();
        app.add_observer(on_delete_dialog_closed);

        let dialog = app
            .world_mut()
            .spawn(jackdaw_feathers::dialog::EditorDialog)
            .id();
        {
            let mut pending = app.world_mut().resource_mut::<PendingMaterialDelete>();
            pending.name = Some("grass".to_string());
            pending.dialog = Some(dialog);
        }

        app.world_mut().entity_mut(dialog).despawn();

        let pending = app.world().resource::<PendingMaterialDelete>();
        assert!(
            pending.name.is_none(),
            "a declined delete must not stay armed"
        );
        assert!(pending.dialog.is_none());
    }

    /// An unsaved material has no file to delete and no durable name to clean up.
    #[test]
    fn deleting_an_unsaved_material_is_refused() {
        let (mut app, _tmp) = project_app();
        app.init_resource::<PendingMaterialDelete>();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        app.world_mut()
            .resource_mut::<MaterialRegistry>()
            .add("detected".into(), handle);

        let result = app
            .world_mut()
            .run_system_cached_with(material_delete, params(&[("material", "detected")]))
            .expect("operator runs");

        assert!(matches!(result, OperatorResult::Cancelled));
        assert!(
            app.world()
                .resource::<PendingMaterialDelete>()
                .name
                .is_none()
        );
        assert!(
            app.world()
                .resource::<MaterialRegistry>()
                .get_by_name("detected")
                .is_some()
        );
    }

    #[test]
    fn saved_entries_skip_the_none_placeholder_and_unsaved_materials() {
        let mut assets = Assets::<StandardMaterial>::default();
        let mut registry = MaterialRegistry::default();
        registry.ensure_none_entry();
        registry.add("detected".into(), assets.add(StandardMaterial::default()));
        registry.add_saved("promoted".into(), assets.add(StandardMaterial::default()));
        let names: Vec<&str> = registry.saved_entries().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["promoted"]);
    }

    #[test]
    fn created_names_skip_taken_ones() {
        let mut registry = MaterialRegistry::default();
        assert_eq!(registry.next_created_name(), "Material_1");
        registry.add("Material_1".into(), Handle::default());
        assert_eq!(registry.next_created_name(), "Material_2");
    }

    #[test]
    fn saved_flag_separates_durable_entries_from_ephemeral_ones() {
        let mut registry = MaterialRegistry::default();
        registry.add("detected".into(), Handle::default());
        registry.add_saved("promoted".into(), Handle::default());
        assert!(!registry.is_saved("detected"));
        assert!(registry.is_saved("promoted"));
    }

    /// Without the runtime's retag, a pack of 16-bit maps binds in the game and not in the
    /// editor.
    #[test]
    fn the_editor_plugin_registers_the_runtime_texture_format_retag() {
        use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

        let mut app = material_app();
        app.add_plugins(plugin);
        let image = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::new(
                Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                vec![0x00, 0x80],
                TextureFormat::R16Uint,
                bevy::asset::RenderAssetUsages::default(),
            ));
        let _material = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                depth_map: Some(image.clone()),
                ..default()
            });

        app.update();

        assert_eq!(
            app.world()
                .resource::<Assets<Image>>()
                .get(&image)
                .expect("image")
                .texture_descriptor
                .format,
            TextureFormat::R16Unorm,
        );
    }
}
