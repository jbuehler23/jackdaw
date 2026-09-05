use std::collections::HashSet;
use std::path::Path;

use bevy::{
    ecs::reflect::AppTypeRegistry,
    prelude::*,
    tasks::{Task, futures_lite::future},
};
use rfd::FileHandle;
use serde::de::DeserializeSeed;

use crate::EditorEntity;

use super::registration::register_entities_in_ast;
use super::save::save_scene_inner;
use super::{SceneDirtyState, SceneFilePath};

#[derive(Resource)]
pub(super) enum SceneDialogTask {
    Save(Task<Option<FileHandle>>),
}

pub fn load_scene_from_file(world: &mut World, chosen: &std::path::Path) {
    finish_load_scene(world, chosen);
}

fn finish_load_scene(world: &mut World, chosen: &std::path::Path) {
    let mut path = chosen.to_string_lossy().to_string();

    let json = match std::fs::read_to_string(&path) {
        Ok(json) => json,
        Err(err) => {
            warn!("Failed to read scene file '{path}': {err}");
            return;
        }
    };

    // Only update `last_directory` once the file has been successfully read
    // and we're committed to the load. A failed read must NOT leak a stale
    // path into the dialog state.
    world.resource_mut::<SceneFilePath>().last_directory =
        chosen.parent().map(std::path::Path::to_path_buf);

    if path.ends_with(".scene.json") {
        // Legacy format: raw DynamicWorld JSON
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();

        use bevy::world_serialization::serde::WorldDeserializer;
        let mut asset_server = world.resource_mut::<AssetServer>();
        let scene_deserializer = WorldDeserializer {
            type_registry: &registry,
            load_from_path: &mut *asset_server,
        };
        let mut json_de = serde_json::Deserializer::from_str(&json);
        let scene = match scene_deserializer.deserialize(&mut json_de) {
            Ok(scene) => scene,
            Err(err) => {
                warn!("Failed to deserialize legacy scene: {err}");
                return;
            }
        };

        drop(registry);
        clear_scene_entities(world);
        match scene.write_to_world(world, &mut Default::default()) {
            Ok(_) => info!("Scene loaded from {path} (legacy format)"),
            Err(err) => warn!("Failed to write scene to world: {err}"),
        }
    } else {
        let parent_path = Path::new(&path)
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();

        // Scenes load through the BSN document path. Legacy `.jsn` files are
        // not imported directly: they convert ON DISK first (writing the
        // `.bsn` sibling, keeping the original as `.jsn.bak`), and the editor
        // opens the converted file. Interactive open paths confirm with the
        // user before reaching here; direct calls apply the conversion tool.
        // The legacy metadata and camera framing carry over below.
        let (bsn_text, legacy_jsn) = if path.ends_with(".bsn") {
            (json, None)
        } else {
            let jsn = match jackdaw_jsn::format::parse_scene(&json) {
                Ok((jsn, version)) => {
                    if version[0] < 2 {
                        warn!(
                            "JSN format version {version:?} is not supported. Please re-save with the latest editor.",
                        );
                        return;
                    }
                    if version[0] < 3 {
                        info!("Migrating JSN v2 scene to v3 format");
                    }
                    jsn
                }
                Err(err) => {
                    warn!("Failed to parse JSN file: {err}");
                    return;
                }
            };
            let (bsn_path, _report) =
                match crate::jsn_to_bsn::convert_scene_file(world, Path::new(&path)) {
                    Ok(converted) => converted,
                    Err(err) => {
                        warn!("Failed to convert legacy scene '{path}': {err}");
                        return;
                    }
                };
            let bsn_text = match std::fs::read_to_string(&bsn_path) {
                Ok(text) => text,
                Err(err) => {
                    warn!(
                        "Failed to read converted scene '{}': {err}",
                        bsn_path.display()
                    );
                    return;
                }
            };
            info!(
                "Converted legacy scene to {}; original kept as .jsn.bak",
                bsn_path.display()
            );
            path = bsn_path.to_string_lossy().into_owned();
            (bsn_text, Some(jsn))
        };

        // Migrate reflect type-paths for scenes written under an older Bevy,
        // keyed by the version the save stamped in. A no-op at the current
        // baseline and for unstamped (hand-authored) scenes; the stamp is a
        // BSN comment, so it does not affect parsing either way.
        let bsn_text = match crate::scene_io::stamp::read_stamp(&bsn_text) {
            Some(stamp) => {
                crate::scene_io::stamp::migrate_type_paths(&bsn_text, &stamp.bevy).into_owned()
            }
            None => bsn_text,
        };

        clear_scene_entities(world);

        // Populate the prefab cache from the document's IsA references, then
        // resolve instances so the spawn produces complete entities. A
        // resolution failure (e.g. cycle) falls back to the authored text so
        // the editor stays usable. Worlds without a prefab cache (headless
        // harnesses) spawn the authored text directly.
        let resolved_text = match jackdaw_bsn::parse_bsn_text(&bsn_text) {
            Ok(authored) if world.contains_resource::<crate::prefab::PrefabAstCache>() => {
                {
                    let mut cache = world.resource_mut::<crate::prefab::PrefabAstCache>();
                    crate::prefab::save_load::populate_cache_for_scene_bsn(
                        &authored,
                        &mut cache,
                        &parent_path,
                    );
                }
                let cache = world.resource::<crate::prefab::PrefabAstCache>();
                let get_prefab = |p: &Path| cache.get(p);
                match crate::prefab::resolver_bsn::resolve_scene(&authored, &get_prefab) {
                    Ok(resolved) => jackdaw_bsn::emit_scene(&resolved),
                    Err(e) => {
                        warn!("prefab resolution failed: {e}; spawning unresolved scene");
                        bsn_text.clone()
                    }
                }
            }
            Ok(_) => bsn_text.clone(),
            Err(err) => {
                warn!("Failed to parse BSN scene '{path}': {err}");
                return;
            }
        };

        match jackdaw_bsn::load_bsn_scene(world, &resolved_text) {
            Ok(loaded) => {
                // Fill the JSN AST so the remaining mirror readers keep
                // working; the loaded entities already carry their BSN
                // document links.
                register_entities_in_ast(world, &loaded.entities);
                info!(
                    "Scene loaded from {path} ({} entities, {} embedded assets)",
                    loaded.entities.len(),
                    loaded.assets.len()
                );
            }
            Err(err) => {
                warn!("Failed to load BSN scene '{path}': {err}");
                return;
            }
        }

        if let Some(jsn) = legacy_jsn {
            // Conversion persisted any re-minted node ids into the written
            // `.bsn`, so no dirty flag is needed for id healing.

            // Restore the saved camera framing if present.
            if let Some(camera) = jsn.editor.as_ref().and_then(|e| e.camera.as_ref()) {
                let restored: Transform = camera.clone().into();
                let mut q = world
                    .query_filtered::<&mut Transform, With<crate::viewport::MainViewportCamera>>();
                for mut tf in q.iter_mut(world) {
                    *tf = restored;
                }
            }

            // Restore metadata
            let mut scene_path = world.resource_mut::<SceneFilePath>();
            scene_path.metadata = jsn.metadata.into();
        }
    }

    // Terrain bulk data lives beside the scene rather than in it. An
    // explicit load means disk is the truth, so this overwrites whatever
    // the store held for these paths.
    import_terrain_sidecars(world, &path, SidecarImport::Reload);
    // The navmesh baked from that ground lives beside it; reading it back lets the options
    // bar distinguish a never-baked terrain from a baked one.
    crate::terrain::navmesh_bake::import_beside_scene(world, &path);

    world.resource_mut::<SceneFilePath>().path = Some(path);

    // Stacks were cleared by clear_scene_entities, so dirty baseline is 0
    world.resource_mut::<SceneDirtyState>().undo_len_at_save = 0;
}

/// Whether a sidecar import may overwrite data the store already holds.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SidecarImport {
    /// Disk wins. Used when the user explicitly loads or reloads a scene.
    Reload,
    /// Only fill paths the store has never heard of. Used on tab
    /// activation, where the store may be holding unsaved sculpting that
    /// the file on disk does not have yet.
    FillMissing,
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

    if world
        .get_resource::<crate::terrain::TerrainDataStore>()
        .is_none()
    {
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
    if mode == SidecarImport::FillMissing {
        let store = world.resource::<crate::terrain::TerrainDataStore>();
        wanted.retain(|(path, _)| !store.contains(path));
    }
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
            // `load` takes either format version: a pre-region sidecar migrates on the way
            // in, and the next save writes it back as the current version.
            Ok(bytes) => match sidecar::load(&bytes) {
                Ok(data) => {
                    world
                        .resource_mut::<crate::terrain::TerrainDataStore>()
                        .insert(data_path.clone(), data);
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
                        .resource_mut::<crate::terrain::TerrainDataStore>()
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

    if !world.contains_resource::<crate::terrain::TerrainDataStore>() {
        return;
    }
    let defaults = jackdaw_scene_types::Terrain::default();
    let mut query = world.query::<(Entity, &jackdaw_scene_types::Terrain, Option<&Name>)>();
    let pending: Vec<Settling> = query
        .iter(world)
        .map(|(entity, terrain, name)| {
            let stored = world
                .resource::<crate::terrain::TerrainDataStore>()
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
            crate::terrain::toast_terrain_notice(world, &message);
        }
        if !settling.data_path.is_empty() {
            world
                .resource_mut::<crate::terrain::TerrainDataStore>()
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

/// Collect `roots` and their full descendant subtrees into a set,
/// walking the `Children` relation. Each root is included; the returned
/// set dedups the walk so a shared descendant is visited only once.
fn collect_subtree(world: &World, roots: impl IntoIterator<Item = Entity>) -> HashSet<Entity> {
    let mut set = HashSet::new();
    let mut stack: Vec<Entity> = roots.into_iter().collect();
    while let Some(entity) = stack.pop() {
        if !set.insert(entity) {
            continue;
        }
        if let Some(children) = world.get::<Children>(entity) {
            stack.extend(children.iter());
        }
    }
    set
}

/// Collect every editor entity: each `EditorEntity` root and its
/// full descendant subtree. Used to exclude editor-internal trees
/// (panels, gizmos, picker overlays) when despawning scene entities.
fn collect_editor_entities(
    world: &mut World,
    roots_query: &mut QueryState<Entity, With<EditorEntity>>,
) -> HashSet<Entity> {
    let roots: Vec<Entity> = roots_query.iter(world).collect();
    collect_subtree(world, roots)
}

/// Remove scene entities from the world (named non-editor entities + their descendants).
pub(crate) fn clear_scene_entities(world: &mut World) {
    if world.contains_resource::<jackdaw_bsn::SceneBsnAst>() {
        world.insert_resource(jackdaw_bsn::SceneBsnAst::default());
    }

    // The baked navmesh belongs to the scene being cleared, not to the tab it was in. A tab
    // switch has stashed it by the time this runs (`capture_active_tab`), so only a bake
    // whose scene is going away is dropped here.
    crate::terrain::navmesh_bake::forget_scene_navmesh(world);

    world
        .resource_mut::<crate::selection::Selection>()
        .entities
        .clear();

    if let Err(err) = world.run_system_cached(crate::hierarchy::clear_all_tree_rows) {
        error!("Failed to clear tree rows: {err}");
    }

    // Clear undo/redo stacks; they hold entity references that become
    // stale when the scene is dropped. Callers who want to preserve
    // history (e.g. undo/redo itself) use `despawn_scene_entities`
    // directly.
    let mut history = world.resource_mut::<jackdaw_commands::CommandHistory>();
    history.undo_stack.clear();
    history.redo_stack.clear();

    if let Err(err) = despawn_scene_entities(world) {
        error!("clear_scene_entities failed: {err}");
    }
}

/// Despawn every non-editor scene entity, leaving editor infrastructure
/// (cameras, grids, gizmos) and the undo/redo stacks intact. Used by
/// snapshot apply during undo/redo.
///
/// `bevy_enhanced_input`'s `Action<A>` component auto-inserts a
/// `Name` component (see its `#[require(Name::new(any::type_name::<A>()), ...)]`),
/// so BEI action entities are otherwise indistinguishable from
/// scene roots. They also carry the non-generic `ActionSettings`
/// marker, so excluding those keeps every operator's input routing
/// alive across an `apply_ast_to_world` pass; without action
/// entities in `Actions<CoreExtensionInputContext>`, BEI emits no
/// `Fire` events and every editor keybind goes silent.
pub(crate) fn despawn_scene_entities(world: &mut World) -> Result<(), BevyError> {
    let editor_set = world.run_system_cached(collect_editor_entities)?;

    let roots: Vec<Entity> = world
        .query_filtered::<Entity, (
            With<Name>,
            Without<bevy_enhanced_input::prelude::ActionSettings>,
        )>()
        .iter(world)
        .filter(|e| !editor_set.contains(e))
        .collect();

    let scene_set = collect_subtree(world, roots);

    for entity in scene_set {
        if let Ok(entity_mut) = world.get_entity_mut(entity) {
            entity_mut.despawn();
        }
    }

    // Sweep any leftover chunk mesh children. Despawning a parent brush
    // does not always cascade through `ChildOf` in time; orphan chunk
    // meshes would otherwise survive, keep their `Transform` and
    // `MeshMaterial3d`, and render as a ghost box at world origin in
    // the next scene.
    let orphan_chunks: Vec<Entity> = world
        .query_filtered::<Entity, With<crate::brush::BrushMeshChunk>>()
        .iter(world)
        .collect();
    for entity in orphan_chunks {
        if let Ok(entity_mut) = world.get_entity_mut(entity) {
            entity_mut.despawn();
        }
    }

    Ok(())
}

pub(super) fn poll_scene_dialog(world: &mut World) {
    let Some(mut task) = world.remove_resource::<SceneDialogTask>() else {
        return;
    };

    match &mut task {
        SceneDialogTask::Save(t) => {
            let Some(result) = future::block_on(future::poll_once(t)) else {
                world.insert_resource(task); // Not ready, put it back
                return;
            };
            if let Some(file) = result {
                let path = file.path().to_path_buf();
                let last_dir = path.parent().map(std::path::Path::to_path_buf);

                // Bind the picked path onto the active scene tab so
                // subsequent swaps/saves go to the right file, and the
                // dirty-state and display name reflect "saved scene"
                // instead of "untitled-N". One function moves everything that follows a
                // scene to a new name, so a rename cannot take part of it.
                crate::scene_io::retarget_active_scene(world, &path.to_string_lossy());
                world.resource_mut::<SceneFilePath>().last_directory = last_dir;

                match save_scene_inner(world) {
                    Ok(()) => {}
                    Err(err) => error!("scene save (after Save As dialog) failed: {err}"),
                }
            }
        }
    }
}

/// Tests for reading terrain sidecars back into the store.
///
/// The mode distinction is the substance here. A terrain reaches the
/// world by two routes -- `finish_load_scene` for an explicit open, and
/// `scenes::swap::activate_tab` for a tab pushed straight onto the strip
/// by `scene_open_system` -- and only the first is an instruction to take
/// disk as the truth. Wiring the second as a reload would silently throw
/// away unsaved sculpting on every tab switch.
#[cfg(test)]
mod terrain_sidecar_import_tests {
    use std::path::PathBuf;

    use bevy::prelude::*;
    use jackdaw_terrain::{RegionTerrainData, TerrainData, sidecar};

    use super::{SidecarImport, import_terrain_sidecars};
    use crate::terrain::TerrainDataStore;

    fn unique_tmp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jd_terrin_{}_{label}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn on_disk() -> TerrainData {
        TerrainData {
            resolution: 4,
            heights: (0..16).map(|i| i as f32).collect(),
            channels: vec![],
        }
    }

    fn document(data: &TerrainData) -> RegionTerrainData {
        RegionTerrainData::from_legacy_v1(data).expect("a power-of-two resolution migrates")
    }

    /// A world with one terrain naming `data_path`, and a sidecar for it written beside
    /// `zone.bsn` in a fresh temp dir. The sidecar uses the pre-region format, which opening
    /// migrates.
    fn world_and_scene(label: &str, data_path: &str) -> (World, PathBuf) {
        let tmp = unique_tmp_dir(label);
        let bytes = sidecar::encode(&on_disk()).expect("encodes");
        std::fs::write(tmp.join(data_path), bytes).expect("write sidecar");

        let mut world = World::new();
        world.insert_resource(TerrainDataStore::default());
        world.spawn(jackdaw_scene_types::Terrain {
            resolution: 4,
            data_path: data_path.to_string(),
            ..default()
        });
        (world, tmp.join("zone.bsn"))
    }

    #[test]
    fn a_terrain_with_no_stored_data_is_hydrated_from_its_sidecar() {
        let (mut world, scene) = world_and_scene("fill", "zone.terrain-0.jdterrain");
        import_terrain_sidecars(
            &mut world,
            &scene.to_string_lossy(),
            SidecarImport::FillMissing,
        );
        assert_eq!(
            world
                .resource::<TerrainDataStore>()
                .heights("zone.terrain-0.jdterrain"),
            on_disk().heights.as_slice(),
        );
        let _ = std::fs::remove_dir_all(scene.parent().expect("temp dir"));
    }

    /// The regression this mode exists for: sculpt, switch tabs without
    /// saving, switch back. The store is the truth, not the older file.
    #[test]
    fn fill_missing_leaves_unsaved_edits_alone() {
        let (mut world, scene) = world_and_scene("unsaved", "zone.terrain-0.jdterrain");
        let unsaved = TerrainData {
            resolution: 4,
            heights: vec![99.0; 16],
            channels: vec![],
        };
        world
            .resource_mut::<TerrainDataStore>()
            .insert("zone.terrain-0.jdterrain".to_string(), document(&unsaved));

        import_terrain_sidecars(
            &mut world,
            &scene.to_string_lossy(),
            SidecarImport::FillMissing,
        );
        assert_eq!(
            world
                .resource::<TerrainDataStore>()
                .heights("zone.terrain-0.jdterrain"),
            vec![99.0; 16].as_slice(),
            "a tab swap must not re-read over unsaved sculpting",
        );
        let _ = std::fs::remove_dir_all(scene.parent().expect("temp dir"));
    }

    /// An explicit open, by contrast, is exactly a request for what is on
    /// disk.
    #[test]
    fn reload_overwrites_what_the_store_was_holding() {
        let (mut world, scene) = world_and_scene("reload", "zone.terrain-0.jdterrain");
        world.resource_mut::<TerrainDataStore>().insert(
            "zone.terrain-0.jdterrain".to_string(),
            document(&TerrainData {
                resolution: 4,
                heights: vec![99.0; 16],
                channels: vec![],
            }),
        );

        import_terrain_sidecars(&mut world, &scene.to_string_lossy(), SidecarImport::Reload);
        assert_eq!(
            world
                .resource::<TerrainDataStore>()
                .heights("zone.terrain-0.jdterrain"),
            on_disk().heights.as_slice(),
        );
        let _ = std::fs::remove_dir_all(scene.parent().expect("temp dir"));
    }

    /// A scene whose sidecar was never copied alongside it opens flat with
    /// a warning rather than failing.
    #[test]
    fn a_missing_sidecar_loads_flat_rather_than_erroring() {
        let tmp = unique_tmp_dir("missing");
        let mut world = World::new();
        world.insert_resource(TerrainDataStore::default());
        world.spawn(jackdaw_scene_types::Terrain {
            resolution: 4,
            data_path: "gone.jdterrain".to_string(),
            ..default()
        });

        import_terrain_sidecars(
            &mut world,
            &tmp.join("zone.bsn").to_string_lossy(),
            SidecarImport::Reload,
        );
        assert!(
            world
                .resource::<TerrainDataStore>()
                .heights("gone.jdterrain")
                .is_empty(),
            "a missing sidecar leaves the terrain flat",
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

/// Settling a loaded terrain onto the geometry its cells are drawn at: the
/// sidecar's own geometry where it states one, and the declared rectangle where
/// it does not.
#[cfg(test)]
mod grid_settling_tests {
    use bevy::prelude::*;
    use jackdaw_terrain::{
        RegionTerrainData,
        region::{RegionCoord, RegionSize, TerrainRegions},
    };

    use super::{SidecarImport, import_terrain_sidecars, settle_terrain_grids};
    use crate::terrain::TerrainDataStore;

    /// Regions covering `span` regions per axis from the origin, at 256 cells per region.
    fn regions_spanning(span: i32) -> TerrainRegions {
        let mut regions = TerrainRegions::new(RegionSize::DEFAULT);
        for rz in 0..span {
            for rx in 0..span {
                regions.ensure_region(RegionCoord::new(rx, rz));
            }
        }
        regions
    }

    /// A document whose regions cover `span` regions per axis.
    fn document(span: i32) -> RegionTerrainData {
        RegionTerrainData {
            regions: regions_spanning(span),
            ..default()
        }
    }

    const DATA_PATH: &str = "scene.terrain-1.jdterrain";

    /// A terrain whose sidecar states no geometry, so the declared rectangle places its
    /// cells.
    fn world_with_legacy_terrain() -> World {
        world_with(256, 4, DATA_PATH)
    }

    fn world_with(resolution: u32, span: i32, data_path: &str) -> World {
        let mut world = World::new();
        let mut store = TerrainDataStore::default();
        // No stored geometry, so the declared rectangle places these cells.
        store.insert(data_path.to_string(), legacy_document(span));
        world.insert_resource(store);
        world.spawn(jackdaw_scene_types::Terrain {
            resolution,
            size: Vec2::splat(100.0),
            data_path: data_path.to_string(),
            ..default()
        });
        world
    }

    /// Where a terrain declaring a `size` by `resolution` rectangle draws the vertex at grid
    /// `(x, z)`, in entity-local space.
    ///
    /// The rectangle is centred on the entity and its `resolution` counts vertices, so the
    /// first sits at `-size/2` and the last on the far edge. The migration reproduces this
    /// mapping; it is spelled out here independently of the code under test.
    fn declared_rect_vertex(size: Vec2, resolution: u32, x: u32, z: u32) -> Vec2 {
        let spacing = size / (resolution.max(2) - 1) as f32;
        -size / 2.0 + Vec2::new(x as f32, z as f32) * spacing
    }

    /// A document as a sidecar without geometry hands it over: cells, and nothing saying
    /// where they sit.
    fn legacy_document(span: i32) -> RegionTerrainData {
        RegionTerrainData {
            grid: None,
            ..document(span)
        }
    }

    /// Where the settled geometry puts the vertex at grid `(x, z)`.
    fn settled_vertex(world: &mut World, x: u32, z: u32) -> Vec2 {
        let grid = world
            .resource::<TerrainDataStore>()
            .grid(DATA_PATH)
            .expect("the load settles a geometry onto every terrain");
        grid.anchor + Vec2::new(x as f32, z as f32) * grid.cell_size
    }

    /// The migration re-describes rather than moves: whatever rectangle a scene declared,
    /// every stored cell comes out of the load at the world position it had.
    ///
    /// Both forms are covered: a scene that elided the pair and refilled it from the
    /// component's defaults, and one that wrote it out explicitly.
    #[test]
    fn a_declared_rects_ground_stays_where_it_was_through_the_migration() {
        for (size, resolution) in [
            // Elided: refilled from the component defaults.
            (Vec2::splat(100.0), 256u32),
            // Explicit, the shape the shape panel offers.
            (Vec2::splat(1024.0), 1024),
            // A 2^k+1 grid, which lands on a whole spacing.
            (Vec2::splat(128.0), 129),
        ] {
            let mut world = World::new();
            let mut store = TerrainDataStore::default();
            store.insert(DATA_PATH.to_string(), legacy_document(4));
            world.insert_resource(store);
            world.spawn(jackdaw_scene_types::Terrain {
                resolution,
                size,
                data_path: DATA_PATH.to_string(),
                ..default()
            });

            settle_terrain_grids(&mut world);

            for (x, z) in [
                (0u32, 0u32),
                (1, 0),
                (0, 1),
                (resolution - 1, resolution - 1),
            ] {
                assert_eq!(
                    settled_vertex(&mut world, x, z),
                    declared_rect_vertex(size, resolution, x, z),
                    "vertex ({x}, {z}) of a {size:?} by {resolution} terrain moved",
                );
            }
        }
    }

    /// A cell is square, so a rectangle asking for two spacings cannot be re-described
    /// exactly. X wins and Z is respaced, which moves ground, so the settling warns.
    #[test]
    fn a_non_square_rect_settles_on_its_x_spacing_and_says_so() {
        let mut world = World::new();
        let mut store = TerrainDataStore::default();
        store.insert(DATA_PATH.to_string(), legacy_document(4));
        world.insert_resource(store);
        world.spawn(jackdaw_scene_types::Terrain {
            resolution: 1024,
            size: Vec2::new(2000.0, 500.0),
            data_path: DATA_PATH.to_string(),
            ..default()
        });

        settle_terrain_grids(&mut world);

        let mut query = world.query::<&jackdaw_scene_types::Terrain>();
        assert_eq!(
            query.single(&world).expect("one terrain").cell_size,
            2000.0 / 1023.0,
            "the X axis spacing is the one a scalar cell size keeps",
        );
        assert_eq!(
            jackdaw_terrain::sidecar::declared_rect_respacing(Vec2::new(2000.0, 500.0), 1024),
            Some((2000.0 / 1023.0, 500.0 / 1023.0)),
            "both spacings are available to name in the warning",
        );
    }

    /// The inlets are read once and emptied, so a saved scene carries the derived cell size
    /// and no rectangle for a later load to re-derive from.
    #[test]
    fn settling_a_terrain_fills_its_cell_size_and_empties_the_inlets() {
        let mut world = world_with(256, 4, DATA_PATH);
        settle_terrain_grids(&mut world);

        let defaults = jackdaw_scene_types::Terrain::default();
        let mut query = world.query::<&jackdaw_scene_types::Terrain>();
        let terrain = query.single(&world).expect("one terrain");
        assert_eq!(terrain.cell_size, 100.0 / 255.0);
        assert_eq!(terrain.size, defaults.size);
        assert_eq!(terrain.resolution, defaults.resolution);
    }

    /// A sidecar that states its own geometry wins over a scene text that declares a
    /// rectangle, the state a save interrupted between its two files leaves behind.
    #[test]
    fn a_sidecar_that_states_its_geometry_outranks_stale_scene_text() {
        use jackdaw_terrain::sidecar::GridGeometry;

        let stated = GridGeometry {
            cell_size: 2.5,
            anchor: Vec2::new(7.0, -3.0),
        };
        let mut world = world_with(256, 4, DATA_PATH);
        world
            .resource_mut::<TerrainDataStore>()
            .set_grid(DATA_PATH, stated);

        settle_terrain_grids(&mut world);

        assert_eq!(
            world.resource::<TerrainDataStore>().grid(DATA_PATH),
            Some(stated)
        );
        let mut query = world.query::<&jackdaw_scene_types::Terrain>();
        assert_eq!(query.single(&world).expect("one terrain").cell_size, 2.5);
    }

    /// A scene opened a second time, with the store warm, reads no sidecar again.
    #[test]
    fn reopening_a_warm_store_reads_no_sidecar_again() {
        use jackdaw_terrain::{RegionTerrainData, sidecar};

        let tmp = std::env::temp_dir().join(format!("jd_fb_warm_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let data_path = "zone.terrain-0.jdterrain";
        std::fs::write(
            tmp.join(data_path),
            sidecar::save(&RegionTerrainData {
                regions: regions_spanning(4),
                ..default()
            })
            .expect("encodes"),
        )
        .expect("write sidecar");

        let mut world = World::new();
        world.insert_resource(TerrainDataStore::default());
        world.spawn(jackdaw_scene_types::Terrain {
            resolution: 256,
            size: Vec2::splat(100.0),
            data_path: data_path.to_string(),
            ..default()
        });
        let scene = tmp.join("zone.bsn").to_string_lossy().to_string();

        let first = import_terrain_sidecars(&mut world, &scene, SidecarImport::FillMissing);
        assert_eq!(first.len(), 1, "the first load reads the sidecar in");

        let second = import_terrain_sidecars(&mut world, &scene, SidecarImport::FillMissing);
        assert!(
            second.is_empty(),
            "the store already holds it, so nothing is read again: {second:?}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A tab switch re-runs the import to fill anything missing. With every sidecar already
    /// in the store it imports nothing, costing no file read and moving no stored data.
    #[test]
    fn an_import_that_finds_everything_already_loaded_reads_nothing() {
        let mut world = world_with_legacy_terrain();
        let scene = std::path::Path::new("/nonexistent/zone.bsn");

        // A store holding every sidecar has been through a load, so its terrains are
        // settled onto their geometry.
        settle_terrain_grids(&mut world);

        // Nothing is missing, so FillMissing has nothing to read.
        let before = world.resource::<TerrainDataStore>().get(DATA_PATH).cloned();
        import_terrain_sidecars(
            &mut world,
            &scene.to_string_lossy(),
            SidecarImport::FillMissing,
        );

        assert_eq!(
            world.resource::<TerrainDataStore>().get(DATA_PATH).cloned(),
            before,
            "a no-op import leaves the store alone"
        );
    }
}
