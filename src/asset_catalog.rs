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
/// The map holds every named asset the editor has loaded, so `@Name`
/// references resolve while it runs. Nothing here is written back:
/// `catalog.bsn` is read for what older projects put in it, and
/// `project.migrate_asset_references` writes those entries out as files of
/// their own.
#[derive(Resource, Default)]
pub struct AssetCatalog {
    /// `@Name` -> loaded `UntypedHandle` (populated at project open).
    pub handles: HashMap<String, UntypedHandle>,
    /// Reverse lookup: asset ID -> `@Name`, for the names a scene save falls
    /// back to when no file holds the asset.
    pub id_to_name: HashMap<UntypedAssetId, String>,
    /// Names of material entries the catalog file still holds inline, which
    /// the migration writes out as files of their own.
    pub inline_materials: HashSet<String>,
}

/// What `catalog.bsn` held when the project opened.
#[derive(Resource, Default)]
pub struct CatalogImport {
    /// How many entries the file holds, all of them without a file of their own.
    pub entries: usize,
    /// Whether the entries have already been reported this run.
    reported: bool,
}

impl AssetCatalog {
    /// Insert a runtime handle into the catalog.
    pub fn insert(&mut self, name: String, handle: UntypedHandle) {
        self.id_to_name.insert(handle.id(), name.clone());
        self.handles.insert(name, handle);
    }

    /// Check if a name is already in the catalog.
    pub fn contains_name(&self, name: &str) -> bool {
        self.handles.contains_key(name)
    }
}

/// Say once that the catalog file holds entries that belong in files of their
/// own, and how to move them.
fn report_catalog_entries(world: &mut World, count: usize) {
    let mut import = world.get_resource_or_init::<CatalogImport>();
    import.entries = count;
    if count == 0 || import.reported {
        return;
    }
    import.reported = true;
    warn!(
        "catalog.bsn holds {count} entries, which is how assets were kept before each had a \
         file of its own; run project.migrate_asset_references to write them out"
    );
}

/// Whether a catalog entry is a `StandardMaterial`, and so belongs in a file
/// of its own rather than in the catalog file.
fn is_material(handle: &UntypedHandle) -> bool {
    handle.type_id() == std::any::TypeId::of::<StandardMaterial>()
}

/// Whether catalog text carries no entries: only comments and whitespace. Such
/// a file loads as an empty catalog rather than as a parse failure.
fn is_empty_catalog_text(text: &str) -> bool {
    text.lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .all(|line| line.trim().is_empty())
}

/// Populate [`AssetCatalog`] from whatever `catalog.bsn` holds.
///
/// The project's material files are indexed and named before this runs, so
/// their linear-space textures have claimed their paths and a filed material
/// wins its name over an inline entry of the same name.
///
/// Inline material entries load normally; `project.migrate_asset_references`
/// writes them out as files of their own.
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

    if jackdaw_bsn::is_document_path(&catalog_path) {
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
                            world
                                .resource_mut::<AssetCatalog>()
                                .inline_materials
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
                info!("Loaded asset catalog with {count} entries");
                report_catalog_entries(world, count);
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

    let mut catalog = world.resource_mut::<AssetCatalog>();
    for (name, handle) in loaded {
        catalog.id_to_name.insert(handle.id(), name.clone());
        catalog.handles.insert(name, handle);
    }
    let count = catalog.handles.len();

    info!("Loaded asset catalog with {count} entries");
    report_catalog_entries(world, count);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material_assets::MaterialRegistry;
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
        std::fs::create_dir_all(tmp.path().join("assets")).expect("assets dir");
        (app, tmp)
    }

    fn catalog_file(tmp: &tempfile::TempDir) -> std::path::PathBuf {
        tmp.path().join("assets/catalog.bsn")
    }

    /// The catalog file is an import, not a store: the editor reads what an
    /// older project put there and never writes it, so what is on disk stays
    /// as the project committed it until the migration moves it.
    #[test]
    fn nothing_the_editor_does_writes_the_catalog_file() {
        let (mut app, tmp) = catalog_app();
        let path = catalog_file(&tmp);
        let held = "#slate\nbevy_pbr::pbr_material::StandardMaterial {}\n";
        std::fs::write(&path, held).expect("write");

        load_catalog(app.world_mut());
        app.update();
        app.update();

        assert_eq!(
            std::fs::read_to_string(&path).expect("still there"),
            held,
            "the file must survive an editor run untouched"
        );
    }

    /// The entries belong in files of their own, and the user is told once
    /// where to put them.
    #[test]
    fn opening_a_project_whose_catalog_holds_entries_reports_them_once() {
        let (mut app, tmp) = catalog_app();
        std::fs::write(
            catalog_file(&tmp),
            "#slate\nbevy_pbr::pbr_material::StandardMaterial {}\n",
        )
        .expect("write");

        load_catalog(app.world_mut());

        let import = app.world().resource::<CatalogImport>();
        assert_eq!(import.entries, 1);
        assert!(import.reported, "the count is said once");

        load_catalog(app.world_mut());
        assert_eq!(
            app.world().resource::<CatalogImport>().entries,
            1,
            "a second read says the same thing and no more"
        );
    }

    #[test]
    fn comment_only_and_blank_catalogs_load_as_empty_not_as_failures() {
        for text in ["", "   \n", "// no catalog entries\n"] {
            let (mut app, tmp) = catalog_app();
            std::fs::write(catalog_file(&tmp), text).expect("write");
            load_catalog(app.world_mut());
            assert!(app.world().resource::<AssetCatalog>().handles.is_empty());
            assert_eq!(
                app.world()
                    .get_resource::<CatalogImport>()
                    .map_or(0, |import| import.entries),
                0,
                "{text:?} holds nothing to migrate"
            );
        }
    }
}
