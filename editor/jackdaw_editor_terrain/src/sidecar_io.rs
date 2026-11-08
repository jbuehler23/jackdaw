//! Each terrain's bulk data, kept in a sidecar file beside the scene that
//! holds it: written just ahead of the scene on save, and read back into the
//! [`TerrainDataStore`] when a scene opens or its tab comes to the front.

use std::any::Any;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw_editor_core::scene_io::SidecarImport;
use jackdaw_editor_core::scene_io::hooks::{
    ChangedFile, DocumentIoHook, DocumentIoHooks, PlayCopies, PlayDocument, SidecarWrite,
    data_paths_in, play_copy_stamp,
};
use jackdaw_editor_core::scenes::Scenes;

use crate::TerrainDataStore;

/// Registers the terrain sidecars with the document save and load.
pub fn plugin(app: &mut App) {
    app.world_mut()
        .get_resource_or_init::<DocumentIoHooks>()
        .register(TerrainSidecars);
}

/// Terrain sidecars, as the document save and load reach them.
struct TerrainSidecars;

impl DocumentIoHook for TerrainSidecars {
    fn begin_save(&self, _world: &mut World, _path: &str) -> Box<dyn Any + Send> {
        Box::new(Vec::<EncodedTerrain>::new())
    }

    fn encode_sidecars(
        &self,
        world: &mut World,
        path: &str,
        state: &mut (dyn Any + Send),
    ) -> Result<Vec<SidecarWrite>, BevyError> {
        let encoded = state
            .downcast_mut::<Vec<EncodedTerrain>>()
            .ok_or("terrain save state of the wrong type")?;
        let (terrains, writes) = encode_terrain_sidecars(world, path)?;
        *encoded = terrains;
        Ok(writes)
    }

    fn finish_save(&self, world: &mut World, state: Box<dyn Any + Send>, written: bool) {
        if written && let Ok(terrains) = state.downcast::<Vec<EncodedTerrain>>() {
            note_written_terrains(world, &terrains);
        }
    }

    fn import_sidecars(&self, world: &mut World, path: &str, mode: SidecarImport) {
        import_terrain_sidecars(world, path, mode);
    }

    fn forget_project(&self, world: &mut World) {
        world.insert_resource(TerrainDataStore::default());
    }

    fn export_play_sidecars(
        &self,
        world: &World,
        document: &PlayDocument,
        copies: &mut PlayCopies,
    ) -> Result<(), BevyError> {
        export_play_terrain(world, document, copies)
    }

    fn has_unsaved_edits(&self, world: &World, tab: usize) -> bool {
        let Some(store) = store_of_tab(world, Some(tab)) else {
            return false;
        };
        terrain_paths_of_tab(world, tab)
            .iter()
            .any(|path| store.has_unsaved_edits(path) && !store.is_load_failed(path))
    }

    fn changed_on_disk(&self, world: &World, tab: usize) -> Vec<PathBuf> {
        let Some(store) = store_of_tab(world, Some(tab)) else {
            return Vec::new();
        };
        changed_terrain_files(world, tab)
            .into_iter()
            .filter(|(path, _)| store.has_unsaved_edits(path))
            .map(|(_, file)| file)
            .collect()
    }

    fn keep_over_changed(&self, world: &mut World, tab: usize, files: &[ChangedFile]) {
        let kept: Vec<(String, std::time::SystemTime)> = terrain_files_of_tab(world, tab)
            .into_iter()
            .filter_map(|(path, file)| {
                let asked = files.iter().find(|asked| asked.path == file)?;
                Some((path, asked.modified?))
            })
            .collect();
        let Some(mut store) = store_of_tab_mut(world, tab) else {
            return;
        };
        for (path, mtime) in kept {
            store.note_kept_over(&path, mtime);
        }
    }
}

/// The `data_path` of each terrain in tab `tab`'s document: the live one for the
/// tab in front, the snapshot for any other, or the cache entry for a prefab
/// behind. Terrains deleted from it are left out, though the store keeps their
/// data for an undo.
fn terrain_paths_of_tab(world: &World, tab: usize) -> Vec<String> {
    use jackdaw_editor_core::scenes::TabContent;

    let Some(scenes) = world.get_resource::<Scenes>() else {
        return Vec::new();
    };
    if scenes.active == tab {
        let Some(mut query) = world.try_query::<&jackdaw_scene_types::Terrain>() else {
            return Vec::new();
        };
        let mut paths: Vec<String> = query
            .iter(world)
            .map(|terrain| terrain.data_path.clone())
            .filter(|path| !path.is_empty())
            .collect();
        paths.sort();
        paths.dedup();
        return paths;
    }
    let type_path = <jackdaw_scene_types::Terrain as bevy::reflect::TypePath>::type_path();
    match scenes.tabs.get(tab).map(|tab| &tab.content) {
        Some(TabContent::Scene(Some(document))) => data_paths_in(document, type_path),
        Some(TabContent::Prefab(path)) => world
            .get_resource::<jackdaw_editor_core::prefab::PrefabAstCache>()
            .and_then(|cache| cache.get(path.as_path()))
            .map(|document| data_paths_in(document, type_path))
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Each terrain in tab `tab`'s document with the file its sidecar is, beside the
/// tab's file, in path order.
fn terrain_files_of_tab(world: &World, tab: usize) -> Vec<(String, PathBuf)> {
    let Some(dir) = world.get_resource::<Scenes>().and_then(|scenes| {
        scenes
            .tabs
            .get(tab)?
            .path
            .as_deref()?
            .parent()
            .map(Path::to_path_buf)
    }) else {
        return Vec::new();
    };
    terrain_paths_of_tab(world, tab)
        .into_iter()
        .filter_map(|path| {
            let file = sidecar_file(&dir, &path)?;
            Some((path, file))
        })
        .collect()
}

/// The sidecar `data_path` names beside a document in `dir`, as the store
/// records the file it read or wrote.
fn sidecar_file(dir: &Path, data_path: &str) -> Option<PathBuf> {
    let file = jackdaw_terrain::sidecar::resolve_path(dir, data_path).ok()?;
    Some(dunce::canonicalize(&file).unwrap_or(file))
}

/// Each terrain of tab `tab` whose sidecar something else wrote after the store
/// last read or wrote it, with that file, in path order. A terrain whose file
/// failed to load is left out, since a save never writes it.
fn changed_terrain_files(world: &World, tab: usize) -> Vec<(String, PathBuf)> {
    let Some(store) = store_of_tab(world, Some(tab)) else {
        return Vec::new();
    };
    terrain_files_of_tab(world, tab)
        .into_iter()
        .filter(|(path, file)| {
            store.contains(path)
                && !store.is_load_failed(path)
                && store.sidecar_changed_since_read(path, file)
        })
        .collect()
}

/// [`store_of_tab`], writable.
fn store_of_tab_mut(world: &mut World, tab: usize) -> Option<Mut<'_, TerrainDataStore>> {
    let active = world.get_resource::<Scenes>()?.active;
    if active == tab {
        return world.get_resource_mut::<TerrainDataStore>();
    }
    let scenes = world.get_resource_mut::<Scenes>()?;
    if tab >= scenes.tabs.len() {
        return None;
    }
    Some(scenes.map_unchanged(|scenes| &mut scenes.tabs[tab].terrain_data_store))
}

/// The terrain store of tab `tab`: the world's for the tab in front.
fn store_of_tab(world: &World, tab: Option<usize>) -> Option<&TerrainDataStore> {
    let tab = tab?;
    let scenes = world.get_resource::<Scenes>()?;
    if scenes.active == tab {
        return world.get_resource::<TerrainDataStore>();
    }
    Some(&scenes.tabs.get(tab)?.terrain_data_store)
}

/// A store holding unsaved edits to the sidecar `data_path` names in `dir`: the tab in
/// front's first, then any other open document in that folder, where the same path
/// names the same file. How a prefab no tab holds gets edits made on its instances.
fn store_with_edits<'a>(
    world: &'a World,
    dir: &Path,
    data_path: &str,
) -> Option<&'a TerrainDataStore> {
    let scenes = world.get_resource::<Scenes>()?;
    let wanted = std::fs::canonicalize(dir).ok()?;
    let beside = |tab: usize| {
        let folder = scenes.tabs.get(tab)?.path.as_deref()?.parent()?;
        (std::fs::canonicalize(folder).ok()? == wanted).then_some(tab)
    };
    std::iter::once(scenes.active)
        .chain((0..scenes.tabs.len()).filter(|tab| *tab != scenes.active))
        .filter_map(beside)
        .filter_map(|tab| store_of_tab(world, Some(tab)))
        .find(|store| store.has_unsaved_edits(data_path) && !store.is_load_failed(data_path))
}

/// Settle a copy of each of `document`'s terrains with unsaved edits, or with no
/// file to read, from the store of the tab holding it. A clean terrain, however far
/// its store lags its file, and one that failed to load, get none. Edits that an
/// undo took back leave nothing to copy either, once compared with the file. A prefab
/// no tab holds takes only the edits an open document in its folder holds, and a
/// scene no tab holds takes none.
fn export_play_terrain(
    world: &World,
    document: &PlayDocument,
    copies: &mut PlayCopies,
) -> Result<(), BevyError> {
    use jackdaw_terrain::sidecar;

    let type_path = <jackdaw_scene_types::Terrain as bevy::reflect::TypePath>::type_path();
    let assets = jackdaw_editor_core::asset_index::assets_dir(world);
    for data_path in document.data_paths(type_path) {
        let Ok(copy) = sidecar::resolve_path(document.overlay_dir, &data_path) else {
            warn!("Skipping invalid terrain data path {data_path:?}");
            continue;
        };
        let store = match document.tab {
            Some(tab) => store_of_tab(world, Some(tab)),
            None if document.instanced => document
                .file_dir()
                .and_then(|dir| store_with_edits(world, dir, &data_path)),
            None => None,
        };
        let Some((store, data)) = store.and_then(|store| Some((store, store.get(&data_path)?)))
        else {
            continue;
        };
        if store.is_load_failed(&data_path) {
            continue;
        }
        let saved = document
            .file_dir()
            .and_then(|dir| sidecar::resolve_path(dir, &data_path).ok());
        let on_disk = saved.as_deref().is_some_and(Path::exists);
        if on_disk && !store.has_unsaved_edits(&data_path) {
            continue;
        }
        let stamp = play_copy_stamp(
            store.edit_revision(&data_path).unwrap_or_default(),
            saved.as_deref(),
        );
        copies.settle(&copy, stamp, || {
            let holds = saved
                .as_deref()
                .is_some_and(|file| sidecar_holds(file, data, assets.as_deref()));
            if holds {
                return Ok(None);
            }
            sidecar::save(data).map(Some).map_err(|err| {
                format!("terrain data {data_path:?} cannot be encoded: {err}").into()
            })
        })?;
    }
    Ok(())
}

/// Whether the sidecar at `file` decodes to `data`. An older format on disk does
/// not read as a change, nor does a file stating no geometry when a load settled
/// the store onto one.
fn sidecar_holds(
    file: &Path,
    data: &jackdaw_terrain::RegionTerrainData,
    assets: Option<&Path>,
) -> bool {
    use jackdaw_terrain::sidecar;

    let Ok(held) = std::fs::read(file) else {
        return false;
    };
    sidecar::load_from(&held, assets).is_ok_and(|loaded| {
        let mut held = loaded.data;
        held.grid = held.grid.or(data.grid);
        held == *data
    })
}

/// A terrain whose sidecar a save has encoded, for when the save is over.
struct EncodedTerrain {
    data_path: String,
    file: PathBuf,
    /// Its latest edit as of the encoding, so a later one stays unsaved.
    revision: Option<u64>,
    /// [`jackdaw_terrain::store::sidecar_digest`] of the bytes encoded.
    digest: u64,
}

/// Encode every terrain's bulk data for its sidecar beside the scene. Writes nothing.
///
/// A scene with no terrain is a clean no-op. An invalid path or encoding failure is
/// returned to the save boundary so the tab stays dirty. One file per terrain is
/// named by the scene-relative `data_path` the `Terrain` component carries, so a
/// scene and its terrain data move together.
///
/// A path with no store entry (never loaded -- its sidecar was missing --
/// and never edited) is silently skipped: there is nothing to write and
/// nothing lost. A path marked load-failed (a sidecar existed but could
/// not decode) is also skipped, with a warning: writing zeroed data over
/// a real, if damaged, file would be worse than leaving it alone. Neither
/// case fails the save.
///
/// Only terrains that are actually in the scene are written. The store
/// can outlive them -- an undo can bring one back, and other tabs keep
/// their own entries -- so writing the whole store would scatter files
/// for terrains this scene does not own.
fn encode_terrain_sidecars(
    world: &mut World,
    scene_path: &str,
) -> Result<(Vec<EncodedTerrain>, Vec<SidecarWrite>), BevyError> {
    use jackdaw_terrain::sidecar;

    if world.get_resource::<TerrainDataStore>().is_none() {
        return Ok(Default::default());
    }
    let scene_dir = Path::new(scene_path)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();

    let mut paths: Vec<String> = Vec::new();
    let mut query = world.query::<&jackdaw_scene_types::Terrain>();
    for terrain in query.iter(world) {
        if !terrain.data_path.is_empty() && !paths.contains(&terrain.data_path) {
            paths.push(terrain.data_path.clone());
        }
    }

    settle_changed_files_before_save(world, scene_path, &scene_dir, &paths)?;

    let store = world.resource::<TerrainDataStore>();
    let mut terrains = Vec::new();
    let mut writes = Vec::new();
    for data_path in paths {
        let path = match sidecar::resolve_path(&scene_dir, &data_path) {
            Ok(path) => path,
            Err(err) => {
                return Err(BevyError::from(format!(
                    "invalid terrain data path {data_path:?}: {err}"
                )));
            }
        };
        if store.is_load_failed(&data_path) {
            warn!(
                "Not saving terrain data {data_path:?}: it failed to load, so writing \
                 would overwrite the original file with empty data. Fix or replace the \
                 sidecar and reload the scene."
            );
            continue;
        }
        // No entry means this path was never loaded (its sidecar was
        // missing, and the load stayed lenient) and never edited since:
        // nothing to write, and nothing lost by skipping it.
        let Some(data) = store.get(&data_path) else {
            continue;
        };
        let bytes = sidecar::save(data).map_err(|err| {
            BevyError::from(format!(
                "terrain sidecar {data_path:?} cannot be written: {err}"
            ))
        })?;
        terrains.push(EncodedTerrain {
            revision: store.edit_revision(&data_path),
            digest: jackdaw_terrain::store::sidecar_digest(&bytes),
            data_path,
            file: path.clone(),
        });
        writes.push((path, bytes));
    }
    Ok((terrains, writes))
}

/// Before a save writes the terrains `paths` beside `scene_path`, read again
/// each file something else changed since it was read that holds no unsaved
/// edits here. Refuse the save when such a file does hold unsaved edits, and ask
/// the user whether to reload or keep them; once kept, the next save writes. A
/// save to another folder, as a save-as can make, writes files this store never
/// read and checks none.
fn settle_changed_files_before_save(
    world: &mut World,
    scene_path: &str,
    scene_dir: &Path,
    paths: &[String],
) -> Result<(), BevyError> {
    let store = world.resource::<TerrainDataStore>();
    let mut conflicts: Vec<String> = Vec::new();
    let mut stale = false;
    for path in paths {
        let Some(file) = sidecar_file(scene_dir, path) else {
            continue;
        };
        if !store.contains(path)
            || store.is_load_failed(path)
            || !store.sidecar_changed_since_read(path, &file)
        {
            continue;
        }
        if store.has_unsaved_edits(path) {
            conflicts.push(file.display().to_string());
        } else {
            stale = true;
        }
    }
    if stale {
        import_terrain_sidecars(world, scene_path, SidecarImport::RefreshChanged);
    }
    if conflicts.is_empty() {
        return Ok(());
    }
    let tab = world.resource::<Scenes>().active;
    jackdaw_editor_core::scenes::external_watch::ask_about_changed_files(world, tab);
    Err(BevyError::from(format!(
        "{} changed on disk since it was read. Reload it or keep your edits, \
         answering with scene.external_change where no dialog shows, then save again",
        conflicts.join(", ")
    )))
}

/// Without noting the write, the store keeps the mtime it loaded from and every
/// later activation re-imports the whole terrain.
fn note_written_terrains(world: &mut World, terrains: &[EncodedTerrain]) {
    let Some(mut store) = world.get_resource_mut::<TerrainDataStore>() else {
        return;
    };
    for terrain in terrains {
        let mtime = std::fs::metadata(&terrain.file)
            .and_then(|meta| meta.modified())
            .ok();
        let file = dunce::canonicalize(&terrain.file).unwrap_or_else(|_| terrain.file.clone());
        store.note_written(
            &terrain.data_path,
            file,
            mtime,
            terrain.revision,
            terrain.digest,
        );
    }
}

/// The whole of what a save does with the sidecars, in one go.
#[cfg(test)]
pub(crate) fn export_terrain_sidecars(
    world: &mut World,
    scene_path: &str,
) -> Result<(), BevyError> {
    let (terrains, writes) = encode_terrain_sidecars(world, scene_path)?;
    jackdaw_editor_core::scene_io::hooks::write_sidecars(&writes, false)?;
    note_written_terrains(world, &terrains);
    Ok(())
}

/// Read each terrain's binary sidecar into the store.
///
/// A missing or unreadable sidecar warns and leaves the terrain flat
/// rather than failing the load: a scene whose data file was not copied
/// alongside it should still open, so the user can see what happened and
/// fix it. A legacy scene carrying inline `heights` has no sidecar and is
/// left alone here -- `ensure_terrain_data_path` drains the inline values
/// into the store, and the next save writes them out properly.
///
/// Called from two places, because there are two ways a terrain reaches
/// the world: `finish_load_scene` for an explicit open, and
/// `scenes::swap::activate_tab` for a tab that was opened by pushing a
/// parsed document straight onto the tab strip. Wiring only the first
/// leaves every scene opened from the tab strip flat.
///
/// Returns the sidecar paths this call read in, distinguishing a first load from
/// a tab switch that found everything already in the store.
pub(crate) fn import_terrain_sidecars(
    world: &mut World,
    scene_path: &str,
    mode: SidecarImport,
) -> Vec<String> {
    use jackdaw_terrain::sidecar;

    if world.get_resource::<TerrainDataStore>().is_none() {
        return Vec::new();
    }
    let scene_dir = std::path::Path::new(scene_path)
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();

    let mut wanted: Vec<(String, bool)> = Vec::new();
    let mut query = world.query::<&jackdaw_scene_types::Terrain>();
    for terrain in query.iter(world) {
        if terrain.data_path.is_empty() {
            continue;
        }
        if wanted.iter().any(|(path, _)| path == &terrain.data_path) {
            continue;
        }
        wanted.push((terrain.data_path.clone(), terrain.heights.is_empty()));
    }
    match mode {
        SidecarImport::Reload => {}
        SidecarImport::FillMissing => {
            let store = world.resource::<TerrainDataStore>();
            wanted.retain(|(path, _)| !store.contains(path));
        }
        // A terrain holding unsaved edits keeps them however far its file has
        // moved on; the scene asks the user about that file.
        SidecarImport::RefreshChanged => {
            let store = world.resource::<TerrainDataStore>();
            wanted.retain(|(path, _)| {
                !store.contains(path)
                    || (sidecar::resolve_path(&scene_dir, path)
                        .is_ok_and(|full| store.sidecar_is_stale(path, &full))
                        && !store.has_unsaved_edits(path))
            });
        }
    }
    let assets = jackdaw_editor_core::asset_index::assets_dir(world);
    let mut imported: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (data_path, no_inline_heights) in wanted {
        let full = match sidecar::resolve_path(&scene_dir, &data_path) {
            Ok(path) => path,
            Err(err) => {
                warn!("Skipping invalid terrain data path {data_path:?}: {err}");
                continue;
            }
        };
        match std::fs::read(&full) {
            // A sidecar written before regions existed, or before slots held
            // paths, migrates on the way in.
            Ok(bytes) => match sidecar::load_from(&bytes, assets.as_deref()) {
                Ok(loaded) => {
                    for message in loaded.warnings() {
                        warn!("{message}");
                    }
                    let data = loaded.data;
                    let mtime = std::fs::metadata(&full)
                        .and_then(|meta| meta.modified())
                        .ok();
                    let file = dunce::canonicalize(&full).unwrap_or_else(|_| full.clone());
                    let mut store = world.resource_mut::<TerrainDataStore>();
                    store.insert(data_path.clone(), data);
                    store.note_read(&data_path, mtime);
                    store.note_held(
                        &data_path,
                        file,
                        jackdaw_terrain::store::sidecar_digest(&bytes),
                    );
                    imported.insert(data_path);
                }
                Err(err) => {
                    warn!(
                        "Terrain data {} is unreadable ({err}); edits to this terrain \
                         are refused and save will not overwrite the file until it is \
                         fixed and reloaded",
                        full.display()
                    );
                    world
                        .resource_mut::<TerrainDataStore>()
                        .mark_load_failed(data_path, err.to_string());
                }
            },
            // A legacy scene names no sidecar it ever wrote, so only a
            // terrain that expected one is worth warning about.
            Err(err) if no_inline_heights => {
                warn!(
                    "Terrain data {} is missing ({err}); loading a flat terrain",
                    full.display()
                );
            }
            Err(_) => {}
        }
    }

    settle_terrain_grids(world);
    // Terrains are placed by now, which a group saved beside one is moved
    // into the space of.
    crate::scatter::migrate_legacy_scatter_groups(world);
    imported.into_iter().collect()
}

/// Settle every loaded terrain onto the geometry its cells are drawn at, and
/// empty the migration inlets it may have arrived with.
///
/// Two sidecar formats arrive here. One states its own geometry, and the
/// component takes its cell size from the file, so a scene whose text is older
/// than its sidecar still draws correctly. One predates that field and is placed
/// by the rectangle the component declares, turned into the spacing and corner
/// that rectangle drew with.
///
/// Nothing moves in either case: the derived geometry matches the one the
/// rectangle implied, so every stored cell keeps its world position. The settled
/// terrain states where its cells are rather than implying it, and can hold
/// cells the rectangle left unreachable.
///
/// The inlets are reset afterwards, so a saved scene carries no `size` or
/// `resolution`.
pub(crate) fn settle_terrain_grids(world: &mut World) {
    use jackdaw_terrain::sidecar;

    if !world.contains_resource::<TerrainDataStore>() {
        return;
    }
    let defaults = jackdaw_scene_types::Terrain::default();
    let mut query = world.query::<(Entity, &jackdaw_scene_types::Terrain, Option<&Name>)>();
    let pending: Vec<Settling> = query
        .iter(world)
        .map(|(entity, terrain, name)| {
            let stored = world
                .resource::<TerrainDataStore>()
                .grid(&terrain.data_path);
            Settling {
                entity,
                name: name
                    .map(std::string::ToString::to_string)
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| terrain.data_path.clone()),
                data_path: terrain.data_path.clone(),
                grid: sidecar::resolve_grid(stored, terrain.size, terrain.resolution),
                // Only a sidecar that states no geometry is placed by the declared
                // rectangle, so only that one can be respaced.
                respaced: stored.is_none().then_some(()).and_then(|()| {
                    sidecar::declared_rect_respacing(terrain.size, terrain.resolution)
                }),
            }
        })
        .collect();

    for settling in pending {
        if let Some((x, z)) = settling.respaced {
            let message = format!(
                "{}: this terrain was drawn {x} metres per cell across and {z} along, \
                 which one square cell cannot describe. Its grid is respaced to {x} \
                 on both axes, so its ground has moved along Z.",
                settling.name,
            );
            warn!("{message}");
            crate::toast_terrain_notice(world, &message);
        }
        if !settling.data_path.is_empty() {
            world
                .resource_mut::<TerrainDataStore>()
                .set_grid(&settling.data_path, settling.grid);
        }
        if let Some(mut terrain) = world.get_mut::<jackdaw_scene_types::Terrain>(settling.entity) {
            terrain.cell_size = settling.grid.cell_size;
            terrain.size = defaults.size;
            terrain.resolution = defaults.resolution;
        }
    }
}

/// One terrain moving from the rectangle it declared to the geometry its cells
/// are drawn at.
struct Settling {
    entity: Entity,
    /// What to call this terrain in a notice to the author.
    name: String,
    data_path: String,
    grid: jackdaw_terrain::sidecar::GridGeometry,
    /// The two spacings a non-square declared rectangle asked for, when this
    /// settling is respacing one.
    respaced: Option<(f32, f32)>,
}

#[cfg(test)]
mod load_tests;
#[cfg(test)]
mod save_tests;
