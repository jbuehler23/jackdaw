//! Tilemap cells in the editor: a store keyed by data path, kept per scene
//! tab, and read from and written to `.jdtiles` sidecars beside the document
//! that owns each map.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use bevy::prelude::*;
use bevy::reflect::TypePath;
use jackdaw_api_internal::document::SceneResource;
use jackdaw_scene_types::{LayerTerrainMatch, Tilemap, TilemapLayer};
use jackdaw_tilemap::{DataPathError, TerrainSet, TilemapData};
use jackdaw_tileset::{TerrainMatch, Tileset};

use crate::prefab::{IsA, PrefabEntityId};
use crate::scene_io::{SceneFilePath, SidecarImport};

pub fn plugin(app: &mut App) {
    app.init_resource::<TilemapDataStore>().add_systems(
        Update,
        ensure_tilemap_data_path.run_if(in_state(crate::AppState::Editor)),
    );
    app.world_mut()
        .spawn(SceneResource::of::<TilemapDataStore>());
}

/// Decoded tilemap sidecars of the scene in front, keyed by data path.
#[derive(Resource, Default)]
pub struct TilemapDataStore {
    entries: HashMap<String, TilemapData>,
    /// Paths whose file exists but could not be read, with the reason. Their
    /// files are never written and their names never minted again.
    load_failed: HashMap<String, String>,
    read_mtimes: HashMap<String, SystemTime>,
    /// Paths edited since they were last read or written.
    changed: HashSet<String>,
}

impl TilemapDataStore {
    pub fn get(&self, data_path: &str) -> Option<&TilemapData> {
        self.entries.get(data_path)
    }

    /// The map for editing; the next save writes it.
    pub fn get_mut(&mut self, data_path: &str) -> Option<&mut TilemapData> {
        self.changed.insert(data_path.to_string());
        self.entries.get_mut(data_path)
    }

    pub fn contains(&self, data_path: &str) -> bool {
        self.entries.contains_key(data_path)
    }

    /// Puts an edited map in place; the next save writes it.
    pub fn insert(&mut self, data_path: impl Into<String>, data: TilemapData) {
        let data_path = data_path.into();
        self.changed.insert(data_path.clone());
        self.entries.insert(data_path, data);
    }

    pub fn remove(&mut self, data_path: &str) -> Option<TilemapData> {
        self.read_mtimes.remove(data_path);
        self.load_failed.remove(data_path);
        self.changed.remove(data_path);
        self.entries.remove(data_path)
    }

    pub fn load_failed_reason(&self, data_path: &str) -> Option<&str> {
        self.load_failed.get(data_path).map(String::as_str)
    }

    /// Lifts the quarantine on a path, for a read that decoded.
    pub fn clear_load_failed(&mut self, data_path: &str) {
        self.load_failed.remove(data_path);
    }

    fn quarantine(&mut self, data_path: &str, reason: String) {
        self.entries.remove(data_path);
        self.changed.remove(data_path);
        self.load_failed.insert(data_path.to_string(), reason);
    }

    fn note_read(&mut self, data_path: &str, full: &Path) {
        match std::fs::metadata(full).and_then(|meta| meta.modified()) {
            Ok(mtime) => self.read_mtimes.insert(data_path.to_string(), mtime),
            Err(_) => self.read_mtimes.remove(data_path),
        };
    }

    /// Whether the file has changed since the store read it, or appeared
    /// after the store found it missing.
    fn is_stale(&self, data_path: &str, full: &Path) -> bool {
        let Ok(mtime) = std::fs::metadata(full).and_then(|meta| meta.modified()) else {
            return false;
        };
        self.read_mtimes
            .get(data_path)
            .is_none_or(|read| *read != mtime)
    }
}

/// The solver set terrain layer `layer` resolves with, or a message saying
/// what the user has to fix.
pub fn layer_terrain_set(tileset: &Tileset, layer: &TilemapLayer) -> Result<TerrainSet, String> {
    let mode = match layer.terrain_match {
        LayerTerrainMatch::Auto => None,
        LayerTerrainMatch::Corners => Some(TerrainMatch::Corners),
        LayerTerrainMatch::Sides => Some(TerrainMatch::Sides),
        LayerTerrainMatch::CornersAndSides => Some(TerrainMatch::CornersAndSides),
    };
    tileset
        .terrain_set(mode)
        .map_err(|err| format!("Tilemap layer {:?}: {err}", layer.name))
}

fn parent_dir(path: &Path) -> PathBuf {
    path.parent().map(Path::to_path_buf).unwrap_or_default()
}

fn active_scene_dir(world: &World) -> Option<PathBuf> {
    let path = world.get_resource::<SceneFilePath>()?.path.as_ref()?;
    Some(parent_dir(Path::new(path)))
}

/// The prefab instance an inherited entity came from, if it is one.
fn owning_instance(world: &World, entity: Entity) -> Option<&IsA> {
    world.get::<PrefabEntityId>(entity)?;
    let mut current = Some(entity);
    while let Some(e) = current {
        if let Some(isa) = world.get::<IsA>(e) {
            return Some(isa);
        }
        current = world.get::<ChildOf>(e).map(ChildOf::parent);
    }
    None
}

/// The folder of the document that owns `entity`'s map: the prefab file an
/// inherited entity came from, else the scene.
fn owning_dir(world: &World, entity: Entity, scene_dir: &Path) -> PathBuf {
    let Some(isa) = owning_instance(world, entity) else {
        return scene_dir.to_path_buf();
    };
    let root = crate::prefab::save_load::source_root(world, scene_dir);
    parent_dir(&crate::prefab::save_load::resolve_source_path(
        &isa.source,
        &root,
        scene_dir,
    ))
}

/// Each live map's data path and where its sidecar sits.
fn live_sidecars(
    world: &mut World,
    scene_path: &str,
) -> Vec<(String, Result<PathBuf, DataPathError>)> {
    let scene_dir = parent_dir(Path::new(scene_path));
    let mut query = world.query::<(Entity, &Tilemap)>();
    let world: &World = world;
    let mut sidecars: Vec<(String, Result<PathBuf, DataPathError>)> = Vec::new();
    for (entity, tilemap) in query.iter(world) {
        if tilemap.data_path.is_empty() || sidecars.iter().any(|(p, _)| *p == tilemap.data_path) {
            continue;
        }
        let dir = owning_dir(world, entity, &scene_dir);
        let full = jackdaw_tilemap::resolve_path(&dir, &tilemap.data_path);
        sidecars.push((tilemap.data_path.clone(), full));
    }
    sidecars
}

/// A data path beside the scene in front that no map, store entry,
/// quarantined path or file already takes.
pub fn mint_data_path(world: &mut World) -> String {
    let stem = crate::terrain::store::active_scene_stem(world);
    let dir = active_scene_dir(world);
    let mut query = world.query::<&Tilemap>();
    let live: HashSet<String> = (query.iter(world)).map(|t| t.data_path.clone()).collect();
    let store = world.resource::<TilemapDataStore>();
    jackdaw_tilemap::mint_data_path(&stem, |path| {
        store.contains(path)
            || store.load_failed.contains_key(path)
            || live.contains(path)
            || dir.as_ref().is_some_and(|dir| dir.join(path).exists())
    })
}

/// Gives every scene-owned tilemap a data path of its own. A map with none
/// gets a fresh one; a copy sharing another's path, from a duplicate or a
/// paste, is moved to a fresh one holding a clone of the original's cells.
/// Maps inherited from a prefab share their prefab's sidecar and are left.
pub fn ensure_tilemap_data_path(world: &mut World) {
    let mut query = world.query::<(Entity, &Tilemap)>();
    let mut seen: HashSet<String> = HashSet::new();
    let mut needs: Vec<(Entity, Option<String>)> = Vec::new();
    {
        let world: &World = world;
        for (entity, tilemap) in query.iter(world) {
            if owning_instance(world, entity).is_some() {
                continue;
            }
            if tilemap.data_path.is_empty() {
                needs.push((entity, None));
            } else if !seen.insert(tilemap.data_path.clone()) {
                needs.push((entity, Some(tilemap.data_path.clone())));
            }
        }
    }
    for (entity, aliased) in needs {
        let minted = mint_data_path(world);
        let mut store = world.resource_mut::<TilemapDataStore>();
        let data = aliased.and_then(|from| store.get(&from).cloned());
        store.insert(minted.clone(), data.unwrap_or_default());
        let Some(mut tilemap) = world.get_mut::<Tilemap>(entity) else {
            continue;
        };
        tilemap.data_path = minted;
        let tilemap = tilemap.clone();
        crate::commands::sync_component_to_ast(world, entity, Tilemap::type_path(), &tilemap);
    }
}

/// Reads the sidecar of every tilemap in the world into the store, and
/// returns a warning per map that loaded empty or not at all.
///
/// A missing file loads an empty map. A file that cannot be read or decoded
/// leaves no entry and is quarantined, so a save keeps the file instead of
/// writing over it; it is read again only once its file changes.
pub(crate) fn import_tilemap_sidecars(
    world: &mut World,
    scene_path: &str,
    mode: SidecarImport,
) -> Vec<String> {
    if !world.contains_resource::<TilemapDataStore>() {
        return Vec::new();
    }
    let mut warnings = Vec::new();
    for (data_path, full) in live_sidecars(world, scene_path) {
        let full = match full {
            Ok(full) => full,
            Err(err) => {
                warnings.push(format!("Tilemap data path {data_path:?} is invalid: {err}"));
                continue;
            }
        };
        let store = world.resource::<TilemapDataStore>();
        let stale = store.is_stale(&data_path, &full);
        let wanted = match mode {
            SidecarImport::Reload => true,
            SidecarImport::FillMissing => !store.contains(&data_path),
            SidecarImport::RefreshChanged => !store.contains(&data_path) || stale,
        };
        let quarantined = store.load_failed_reason(&data_path).is_some();
        if !wanted || (quarantined && !stale) {
            continue;
        }
        let read = std::fs::read(&full);
        let mut store = world.resource_mut::<TilemapDataStore>();
        match read.map(|bytes| jackdaw_tilemap::decode(&bytes)) {
            Ok(Ok(data)) => {
                store.entries.insert(data_path.clone(), data);
                store.changed.remove(&data_path);
                store.clear_load_failed(&data_path);
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                store
                    .entries
                    .insert(data_path.clone(), TilemapData::default());
                warnings.push(format!(
                    "Tilemap data {} is missing; loading an empty map",
                    full.display()
                ));
            }
            Ok(Err(err)) => {
                store.quarantine(&data_path, err.to_string());
                warnings.push(format!(
                    "Tilemap data {} is unreadable ({err}); saving keeps the file \
                     until it is fixed",
                    full.display()
                ));
            }
            Err(err) => {
                store.quarantine(&data_path, err.to_string());
                warnings.push(format!(
                    "Tilemap data {} cannot be read ({err}); saving keeps the file \
                     until it is fixed",
                    full.display()
                ));
            }
        }
        store.note_read(&data_path, &full);
    }
    for warning in &warnings {
        warn!("{warning}");
    }
    if let Some(first) = warnings.first() {
        crate::status_bar::notify_warn(world, first.clone());
    }
    warnings
}

/// Writes each live map the store holds whose cells changed since they were
/// read or written, or whose file is missing.
///
/// An invalid path and a quarantined path are skipped with a warning and never
/// fail the save; the quarantined file is kept as it is.
pub(crate) fn export_tilemap_sidecars(
    world: &mut World,
    scene_path: &str,
) -> Result<(), BevyError> {
    if !world.contains_resource::<TilemapDataStore>() {
        return Ok(());
    }
    let mut writes = Vec::new();
    for (data_path, full) in live_sidecars(world, scene_path) {
        let full = match full {
            Ok(full) => full,
            Err(err) => {
                warn!("Not saving tilemap data {data_path:?}: the path is invalid ({err})");
                continue;
            }
        };
        let store = world.resource::<TilemapDataStore>();
        if let Some(reason) = store.load_failed_reason(&data_path) {
            warn!(
                "Not saving tilemap data {data_path:?}: it could not be loaded ({reason}), so \
                 the file on disk is kept"
            );
            continue;
        }
        let unchanged = !store.changed.contains(&data_path) && full.exists();
        if let Some(data) = store.get(&data_path).filter(|_| !unchanged) {
            writes.push((data_path, full, jackdaw_tilemap::encode(data)));
        }
    }
    for (data_path, full, bytes) in writes {
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("cannot make {}: {err}", parent.display()))?;
        }
        jackdaw_api_internal::document::write_atomic(&full, &bytes)
            .map_err(|err| format!("failed to write tilemap data {}: {err}", full.display()))?;
        let mut store = world.resource_mut::<TilemapDataStore>();
        store.changed.remove(&data_path);
        store.note_read(&data_path, &full);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jackdaw_tileset::TilesetTerrainSet;

    #[test]
    fn a_layer_on_a_mixed_mode_tileset_is_told_to_pick_a_mode() {
        let set = |inner, mode| TilesetTerrainSet {
            inner,
            outer: 1,
            mode,
            ..Default::default()
        };
        let tileset = Tileset {
            terrain_sets: vec![set(2, TerrainMatch::Sides), set(3, TerrainMatch::Corners)],
            ..Default::default()
        };
        let mut layer = TilemapLayer {
            name: "Ground".into(),
            ..Default::default()
        };
        let warning = layer_terrain_set(&tileset, &layer).expect_err("ambiguous");
        assert!(warning.contains("Ground") && warning.contains("pick one match mode"));
        layer.terrain_match = LayerTerrainMatch::Sides;
        assert!(layer_terrain_set(&tileset, &layer).is_ok());
    }
}
