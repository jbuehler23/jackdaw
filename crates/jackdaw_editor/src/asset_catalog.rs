use std::collections::HashMap;

use bevy::asset::UntypedAssetId;
use bevy::prelude::*;
use jackdaw_jsn::format::{JsnAssets, JsnCatalog, JsnHeader};

/// Project-level asset catalog for cross-scene deduplication.
///
/// Assets in the catalog are referenced with `@Name` prefix in scene files,
/// while scene-local inline assets use `#Name`. When multiple scenes reference
/// the same `@Name`, they share the same handle (zero duplication).
#[derive(Resource, Default)]
pub struct AssetCatalog {
    /// `@Name` -> loaded `UntypedHandle` (populated at project open).
    pub handles: HashMap<String, UntypedHandle>,
    /// The raw `JsnAssets` data (for re-serialization / UI browsing).
    pub assets: JsnAssets,
    /// Reverse lookup: asset ID -> `@Name` (used during save to emit catalog refs).
    pub id_to_name: HashMap<UntypedAssetId, String>,
    /// Whether the catalog has unsaved changes.
    pub dirty: bool,
}

impl AssetCatalog {
    /// Insert a runtime handle into the catalog. Does not mark dirty; use
    /// [`add_to_catalog_assets`] to persist new serializable data.
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
        info!("No catalog.jsn found, starting with empty catalog");
        return;
    }

    let json = match std::fs::read_to_string(&catalog_path) {
        Ok(json) => json,
        Err(err) => {
            warn!("Failed to read catalog.jsn: {err}");
            return;
        }
    };

    let jsn_catalog: JsnCatalog = match serde_json::from_str(&json) {
        Ok(c) => c,
        Err(err) => {
            warn!("Failed to parse catalog.jsn: {err}");
            return;
        }
    };

    // Resolve relative asset paths from the assets directory, not the catalog file location
    let assets_dir = world.resource::<crate::project::ProjectRoot>().assets_dir();

    // Use the same load_inline_assets function scenes use
    let loaded = crate::scene_io::load_inline_assets(world, &jsn_catalog.assets, &assets_dir);

    // Populate the catalog resource
    let mut catalog = world.resource_mut::<AssetCatalog>();
    catalog.assets = jsn_catalog.assets;
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

    let jsn_catalog = JsnCatalog {
        jsn: JsnHeader::default(),
        assets: catalog.assets.clone(),
    };

    let json = match serde_json::to_string_pretty(&jsn_catalog) {
        Ok(json) => json,
        Err(err) => {
            warn!("Failed to serialize catalog: {err}");
            return;
        }
    };

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

/// Add a named entry to the catalog's `JsnAssets` data (for persistence).
pub fn add_to_catalog_assets(
    catalog: &mut AssetCatalog,
    type_path: &str,
    name: &str,
    value: serde_json::Value,
    handle: UntypedHandle,
) {
    catalog
        .assets
        .0
        .entry(type_path.to_string())
        .or_default()
        .insert(name.to_string(), value);
    catalog.id_to_name.insert(handle.id(), name.to_string());
    catalog.handles.insert(name.to_string(), handle);
    catalog.dirty = true;
}

/// Resolve the catalog file path for loading.
///
/// Prefers `.jsn/catalog.jsn`, falls back to legacy `assets/catalog.jsn`.
fn catalog_file_path(world: &World) -> Option<std::path::PathBuf> {
    let project = world.get_resource::<crate::project::ProjectRoot>()?;
    let new_path = project.jsn_dir().join("catalog.jsn");
    if new_path.is_file() {
        return Some(new_path);
    }
    let legacy_path = project.assets_dir().join("catalog.jsn");
    if legacy_path.is_file() {
        return Some(legacy_path);
    }
    // No catalog exists yet
    Some(new_path)
}

/// Always returns `.jsn/catalog.jsn`. Saves always go to the new location.
fn catalog_save_path(world: &World) -> Option<std::path::PathBuf> {
    let project = world.get_resource::<crate::project::ProjectRoot>()?;
    Some(project.jsn_dir().join("catalog.jsn"))
}
