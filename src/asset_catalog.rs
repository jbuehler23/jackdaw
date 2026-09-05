use std::collections::{HashMap, HashSet};

use bevy::asset::{UntypedAssetId, UntypedHandle};
use bevy::prelude::*;
use jackdaw_jsn::format::JsnCatalog;

/// Project-level asset catalog for cross-scene deduplication.
///
/// Assets in the catalog are referenced with `@Name` prefix in scene files,
/// while scene-local inline assets use `#Name`. When multiple scenes reference
/// the same `@Name`, they share the same handle (zero duplication).
///
/// The map holds every named asset the editor has loaded, saved or not, so
/// `@Name` references resolve while it runs. What *persists* is decided at save
/// time: materials go to their own files (see [`crate::material_assets`]),
/// everything else to `catalog.bsn`.
#[derive(Resource, Default)]
pub struct AssetCatalog {
    /// `@Name` -> loaded `UntypedHandle` (populated at project open).
    pub handles: HashMap<String, UntypedHandle>,
    /// Reverse lookup: asset ID -> `@Name` (used during save to emit catalog refs).
    pub id_to_name: HashMap<UntypedAssetId, String>,
    /// Whether the catalog has unsaved changes.
    pub dirty: bool,
    /// Set when `catalog.bsn` existed but could not be read or parsed.
    ///
    /// The loaded map is then an unknown fraction of the file, so writing it
    /// back would destroy whatever failed to load. While this is set, saving
    /// does not touch the catalog file.
    pub load_failed: bool,
    /// Whether the quarantine refusal has already been reported this run.
    quarantine_reported: bool,
}

/// What a catalog file with no entries holds. A blank file is indistinguishable
/// from a truncated one, and deleting the file would un-shadow the older catalog
/// locations and resurrect their contents.
const EMPTY_CATALOG: &str = "// no catalog entries\n";

impl AssetCatalog {
    /// Insert a runtime handle into the catalog. Does not mark dirty; the
    /// caller sets `dirty` when the change should persist.
    pub fn insert(&mut self, name: String, handle: UntypedHandle) {
        self.id_to_name.insert(handle.id(), name.clone());
        self.handles.insert(name, handle);
    }

    /// Check if a name is already in the catalog.
    pub fn contains_name(&self, name: &str) -> bool {
        self.handles.contains_key(name)
    }

    /// Refuse further writes to the catalog file: what is in memory may not be
    /// the whole of what is on disk.
    pub fn quarantine(&mut self) {
        self.load_failed = true;
    }
}

/// Whether catalog text carries no entries: only comments and whitespace. Such
/// a file loads as an empty catalog rather than as a parse failure.
fn is_empty_catalog_text(text: &str) -> bool {
    text.lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .all(|line| line.trim().is_empty())
}

/// Populate [`AssetCatalog`] for the open project: the `assets/materials`
/// files first, then whatever `catalog.bsn` holds.
///
/// Inline material entries in `catalog.bsn` load normally and are flagged dirty,
/// so the next save writes them out as `.material.bsn` files and drops them from
/// the catalog file.
pub fn load_catalog(world: &mut World) {
    // Materials load first so their linear-space textures claim their paths before the
    // catalog file's generic applier resolves the same paths as sRGB, and so a saved
    // material wins its name.
    let materials = crate::material_assets::load_material_files(world);
    let count = materials.len();
    for (name, handle) in materials {
        world
            .resource_mut::<AssetCatalog>()
            .insert(format!("@{name}"), handle);
        world
            .resource_mut::<crate::material_assets::SavedMaterials>()
            .0
            .insert(name);
    }
    if count > 0 {
        info!("Loaded {count} saved materials");
    }

    load_catalog_file(world);
}

/// Whether a catalog entry is a `StandardMaterial`, and so belongs in
/// `assets/materials` rather than the catalog file.
fn is_material(handle: &UntypedHandle) -> bool {
    handle.type_id() == std::any::TypeId::of::<StandardMaterial>()
}

/// Load `assets/catalog.bsn` (or a legacy location) into [`AssetCatalog`].
fn load_catalog_file(world: &mut World) {
    let catalog_path = catalog_file_path(world);
    let Some(catalog_path) = catalog_path else {
        info!("No project root, skipping catalog load");
        return;
    };

    if !catalog_path.exists() {
        info!("No asset catalog found, starting with empty catalog");
        return;
    }

    let json = match std::fs::read_to_string(&catalog_path) {
        Ok(json) => json,
        Err(err) => {
            warn!("Failed to read {}: {err}", catalog_path.display());
            world.resource_mut::<AssetCatalog>().quarantine();
            return;
        }
    };

    if catalog_path.extension().is_some_and(|e| e == "bsn") {
        if is_empty_catalog_text(&json) {
            info!("Asset catalog holds no entries");
            return;
        }
        // Claim the inline materials' linear-space textures before the generic
        // applier resolves the same paths as sRGB.
        let _linear = crate::material_assets::preload_linear_textures(world, &json);
        match jackdaw_bsn::load_bsn_assets(world, &json) {
            Ok(entries) => {
                let count = entries.len();
                let mut migratable = 0;
                for entry in entries {
                    // Scenes reference catalog assets as `@Name`.
                    let name = format!("@{}", entry.name);
                    if world.resource::<AssetCatalog>().handles.contains_key(&name) {
                        info!("'{name}' already has a material file; ignoring the inline entry");
                        continue;
                    }
                    if is_material(&entry.handle) {
                        // Migrating a name that is not a legal file stem would change the
                        // `@Name` scenes reference, so the entry stays inline.
                        if crate::material_assets::sanitize_material_name(&entry.name) == entry.name
                        {
                            migratable += 1;
                            world
                                .resource_mut::<crate::material_assets::SavedMaterials>()
                                .0
                                .insert(entry.name.clone());
                        } else {
                            warn!(
                                "Material '{}' has no valid file name; it stays in the catalog",
                                entry.name
                            );
                        }
                    }
                    let mut catalog = world.resource_mut::<AssetCatalog>();
                    catalog.id_to_name.insert(entry.handle.id(), name.clone());
                    catalog.handles.insert(name, entry.handle);
                }
                // Migrate the inline materials out on the next save.
                world.resource_mut::<AssetCatalog>().dirty = migratable > 0;
                info!("Loaded asset catalog with {count} entries");
            }
            Err(err) => {
                warn!("Failed to parse {}: {err}", catalog_path.display());
                world.resource_mut::<AssetCatalog>().quarantine();
            }
        }
        return;
    }

    let jsn_catalog: JsnCatalog = match serde_json::from_str(&json) {
        Ok(c) => c,
        Err(err) => {
            warn!("Failed to parse asset catalog: {err}");
            world.resource_mut::<AssetCatalog>().quarantine();
            return;
        }
    };

    // Resolve relative asset paths from the assets directory, not the catalog file location
    let assets_dir = world.resource::<crate::project::ProjectRoot>().assets_dir();

    // Use the same load_inline_assets function scenes use
    let loaded = crate::scene_io::load_inline_assets(world, &jsn_catalog.assets, &assets_dir);

    // Populate the catalog resource
    let mut catalog = world.resource_mut::<AssetCatalog>();
    for (name, handle) in loaded {
        catalog.id_to_name.insert(handle.id(), name.clone());
        catalog.handles.insert(name, handle);
    }
    catalog.dirty = false;

    info!(
        "Loaded asset catalog with {} entries",
        catalog.handles.len()
    );
}

/// Persist the catalog: saved materials to their own files, everything else to
/// `assets/catalog.bsn`.
///
/// A material written to `assets/materials` and an unsaved material (a detected
/// set or a fresh `material.create`) are both left out of the catalog file: the
/// first has its own file, the second is ephemeral and travels inline inside the
/// scenes that use it. Any other entry (another asset type, or a material whose
/// name is not a legal file stem) is written inline.
///
/// The file is never unlinked. With nothing left to write it holds an empty
/// document, which keeps it shadowing the other catalog locations
/// `catalog_file_path` reads.
pub fn save_catalog(world: &mut World) {
    let Some(catalog_path) = catalog_save_path(world) else {
        return;
    };

    if !world.resource::<AssetCatalog>().dirty {
        return;
    }

    if world.resource::<AssetCatalog>().load_failed {
        let mut catalog = world.resource_mut::<AssetCatalog>();
        if !catalog.quarantine_reported {
            catalog.quarantine_reported = true;
            warn!("Asset catalog failed to load; refusing to write over it");
        }
        return;
    }

    let in_files: HashSet<UntypedAssetId> = crate::material_assets::persist_materials(world)
        .into_iter()
        .collect();
    let ephemeral = crate::material_assets::ephemeral_material_ids(world);

    let catalog = world.resource::<AssetCatalog>();
    let refs: Vec<jackdaw_bsn::CatalogAssetRef> = catalog
        .id_to_name
        .iter()
        .filter(|(asset_id, _)| !in_files.contains(asset_id) && !ephemeral.contains(asset_id))
        .map(|(&asset_id, name)| jackdaw_bsn::CatalogAssetRef {
            name: name.trim_start_matches(['@', '#']).to_string(),
            type_id: asset_id.type_id(),
            asset_id,
        })
        .collect();

    // Emptiness is decided from the entries, not the emitted text: the serializer drops
    // entries it cannot express, which must not read as having nothing to keep.
    let text = if refs.is_empty() {
        EMPTY_CATALOG.to_string()
    } else {
        let (text, skipped) = jackdaw_bsn::serialize_assets_to_bsn_reporting(world, &refs);
        if !skipped.is_empty() {
            warn!(
                "Catalog entries could not be serialized and are missing from {}: {}",
                catalog_path.display(),
                skipped.join(", ")
            );
        }
        if is_empty_catalog_text(&text) {
            EMPTY_CATALOG.to_string()
        } else {
            text
        }
    };

    if let Some(parent) = catalog_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    match crate::scene_io::save::write_atomic(&catalog_path, text.as_bytes()) {
        Ok(()) => {
            info!("Catalog saved to {}", catalog_path.display());
            world.resource_mut::<AssetCatalog>().dirty = false;
        }
        Err(err) => warn!("Failed to write catalog: {err}"),
    }
}

/// Resolve the catalog file path for loading.
///
/// Prefers `assets/catalog.bsn` (what saves write), then legacy `.jsn/`
/// and assets-dir `.jsn` catalogs for migration.
fn catalog_file_path(world: &World) -> Option<std::path::PathBuf> {
    let project = world.get_resource::<crate::project::ProjectRoot>()?;
    let legacy_dir = project.root.join(".jsn");
    let candidates = [
        project.assets_dir().join("catalog.bsn"),
        // Legacy locations, read for migration; the next save moves the
        // catalog to `assets/catalog.bsn`.
        legacy_dir.join("catalog.bsn"),
        legacy_dir.join("catalog.jsn"),
        project.assets_dir().join("catalog.jsn"),
    ];
    for candidate in candidates {
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    // No catalog exists yet
    Some(project.assets_dir().join("catalog.bsn"))
}

/// Always returns `assets/catalog.bsn`. The catalog is committed project
/// data (scenes reference its `@Name` entries), so it lives with the assets,
/// not in the gitignored `.jackdaw/`. Materials are not among those entries;
/// they live one per file under `assets/materials`.
fn catalog_save_path(world: &World) -> Option<std::path::PathBuf> {
    let project = world.get_resource::<crate::project::ProjectRoot>()?;
    Some(project.assets_dir().join("catalog.bsn"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material_assets::{MaterialRegistry, SavedMaterials};
    use crate::project::{ProjectConfig, ProjectRoot};
    use bevy::app::App;
    use bevy::asset::{AssetApp, AssetPlugin};

    fn catalog_app() -> (App, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut app = App::new();
        app.add_plugins((bevy::app::TaskPoolPlugin::default(), AssetPlugin::default()));
        app.init_asset::<Image>();
        app.init_asset::<StandardMaterial>();
        app.init_asset::<Mesh>();
        app.register_asset_reflect::<Image>();
        app.register_asset_reflect::<StandardMaterial>();
        app.register_asset_reflect::<Mesh>();
        app.insert_resource(ProjectRoot {
            root: tmp.path().to_path_buf(),
            config: ProjectConfig::default(),
        });
        app.init_resource::<AssetCatalog>();
        app.init_resource::<MaterialRegistry>();
        app.init_resource::<SavedMaterials>();
        std::fs::create_dir_all(tmp.path().join("assets")).expect("assets dir");
        (app, tmp)
    }

    fn catalog_file(tmp: &tempfile::TempDir) -> std::path::PathBuf {
        tmp.path().join("assets/catalog.bsn")
    }

    #[test]
    fn a_catalog_that_failed_to_load_is_never_written_over() {
        let (mut app, tmp) = catalog_app();
        let path = catalog_file(&tmp);
        std::fs::write(&path, "this is not $$ valid bsn {{{").expect("write");

        load_catalog(app.world_mut());
        assert!(
            app.world().resource::<AssetCatalog>().load_failed,
            "a parse failure must quarantine the catalog"
        );

        app.world_mut().resource_mut::<AssetCatalog>().dirty = true;
        save_catalog(app.world_mut());

        assert_eq!(
            std::fs::read_to_string(&path).expect("still there"),
            "this is not $$ valid bsn {{{",
            "the unreadable file must survive untouched"
        );
    }

    #[test]
    fn an_emptied_catalog_is_written_empty_rather_than_unlinked() {
        let (mut app, tmp) = catalog_app();
        let path = catalog_file(&tmp);
        std::fs::write(&path, "// placeholder\n").expect("write");

        app.world_mut().resource_mut::<AssetCatalog>().dirty = true;
        save_catalog(app.world_mut());

        assert!(
            path.is_file(),
            "unlinking would un-shadow the legacy catalog locations"
        );
        assert!(is_empty_catalog_text(
            &std::fs::read_to_string(&path).expect("read")
        ));
    }

    #[test]
    fn a_non_material_entry_sharing_a_materials_name_is_kept() {
        let (mut app, tmp) = catalog_app();
        let material = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let mesh = app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(Mesh::from(bevy::math::primitives::Cuboid::default()));
        {
            let mut catalog = app.world_mut().resource_mut::<AssetCatalog>();
            catalog.insert("@shared".into(), mesh.clone().untyped());
            catalog.dirty = true;
        }
        // The material carries the same display name but is ephemeral, so only its own id
        // is filtered out of the catalog file.
        app.world_mut()
            .resource_mut::<MaterialRegistry>()
            .add("shared".into(), material);

        save_catalog(app.world_mut());

        let text = std::fs::read_to_string(catalog_file(&tmp)).expect("read");
        assert!(
            text.contains("#shared"),
            "the mesh entry must survive a same-named material, got:\n{text}"
        );
    }

    #[test]
    fn a_material_that_could_not_be_written_stays_in_the_catalog() {
        let (mut app, tmp) = catalog_app();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        {
            let mut catalog = app.world_mut().resource_mut::<AssetCatalog>();
            catalog.insert("@blocked".into(), handle.clone().untyped());
            catalog.dirty = true;
        }
        app.world_mut()
            .resource_mut::<MaterialRegistry>()
            .add_saved("blocked".into(), handle);

        // A file where the materials directory belongs, so the write cannot land.
        std::fs::write(tmp.path().join("assets/materials"), "not a directory").expect("write");

        save_catalog(app.world_mut());

        let text = std::fs::read_to_string(catalog_file(&tmp)).expect("read");
        assert!(
            text.contains("#blocked"),
            "a failed migration must leave the entry inline, got:\n{text}"
        );
    }

    #[test]
    fn comment_only_and_blank_catalogs_load_as_empty_not_as_failures() {
        for text in ["", "   \n", EMPTY_CATALOG] {
            let (mut app, tmp) = catalog_app();
            std::fs::write(catalog_file(&tmp), text).expect("write");
            load_catalog(app.world_mut());
            let catalog = app.world().resource::<AssetCatalog>();
            assert!(!catalog.load_failed, "{text:?} must not quarantine");
            assert!(catalog.handles.is_empty());
        }
    }
}
