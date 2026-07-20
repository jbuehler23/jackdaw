use std::collections::HashMap;

use bevy::asset::UntypedAssetId;
use bevy::prelude::*;
use jackdaw_jsn::format::JsnCatalog;

/// Project-level asset catalog for cross-scene deduplication.
///
/// Assets in the catalog are referenced with `@Name` prefix in scene files,
/// while scene-local inline assets use `#Name`. When multiple scenes reference
/// the same `@Name`, they share the same handle (zero duplication).
#[derive(Resource, Default)]
pub struct AssetCatalog {
    /// `@Name` -> loaded `UntypedHandle` (populated at project open).
    pub handles: HashMap<String, UntypedHandle>,
    /// Reverse lookup: asset ID -> `@Name` (used during save to emit catalog refs).
    pub id_to_name: HashMap<UntypedAssetId, String>,
    /// Whether the catalog has unsaved changes.
    pub dirty: bool,
}

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
}

/// Load the project catalog from `.jsn/catalog.jsn` (or legacy `assets/catalog.jsn`) if it exists.
/// Populates `AssetCatalog` handles using the same `load_inline_assets` logic as scenes.
pub fn load_catalog(world: &mut World) {
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
            return;
        }
    };

    if catalog_path.extension().is_some_and(|e| e == "bsn") {
        match jackdaw_bsn::load_bsn_assets(world, &json) {
            Ok(entries) => {
                let count = entries.len();
                let mut catalog = world.resource_mut::<AssetCatalog>();
                for entry in entries {
                    // Scenes reference catalog assets as `@Name`.
                    let name = format!("@{}", entry.name);
                    catalog.id_to_name.insert(entry.handle.id(), name.clone());
                    catalog.handles.insert(name, entry.handle);
                }
                catalog.dirty = false;
                info!("Loaded asset catalog with {count} entries");
            }
            Err(err) => warn!("Failed to parse {}: {err}", catalog_path.display()),
        }
        return;
    }

    let jsn_catalog: JsnCatalog = match serde_json::from_str(&json) {
        Ok(c) => c,
        Err(err) => {
            warn!("Failed to parse asset catalog: {err}");
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

/// Save the catalog to `.jsn/catalog.jsn`.
pub fn save_catalog(world: &mut World) {
    let Some(catalog_path) = catalog_save_path(world) else {
        return;
    };

    let catalog = world.resource::<AssetCatalog>();
    if !catalog.dirty {
        return;
    }

    // The catalog persists as `.bsn`, reflected from the live asset stores;
    // the cached JSON values only feed legacy tooling until deletion.
    let refs: Vec<jackdaw_bsn::CatalogAssetRef> = catalog
        .id_to_name
        .iter()
        .map(|(&asset_id, name)| jackdaw_bsn::CatalogAssetRef {
            name: name.trim_start_matches(['@', '#']).to_string(),
            type_id: asset_id.type_id(),
            asset_id,
        })
        .collect();
    let json = jackdaw_bsn::serialize_assets_to_bsn(world, &refs);

    // Ensure parent directory exists
    if let Some(parent) = catalog_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    match std::fs::write(&catalog_path, &json) {
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
/// not in the gitignored `.jackdaw/`.
fn catalog_save_path(world: &World) -> Option<std::path::PathBuf> {
    let project = world.get_resource::<crate::project::ProjectRoot>()?;
    Some(project.assets_dir().join("catalog.bsn"))
}
