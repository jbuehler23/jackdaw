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

use super::registration::{register_entities_in_ast, register_entity_in_ast};
use super::save::save_scene_inner;
use super::{SceneDirtyState, SceneFilePath};

fn prefab_cache_epoch(world: &World) -> Option<u64> {
    world
        .get_resource::<crate::prefab::PrefabAstCache>()
        .map(crate::prefab::PrefabAstCache::epoch)
}

/// Mark the prefab-cache bumps this load caused as answered, so a refused load
/// does not respawn the scene that is still open.
///
/// Reading a document's prefabs puts them in the cache, and a moved cache makes
/// the on-change driver re-resolve and respawn the active scene, discarding the
/// selection with it. Only this load's own bumps are taken as answered; one that
/// was already pending stays pending.
fn forget_prefab_cache_bump(world: &mut World, before: Option<u64>) {
    let (Some(before), Some(now)) = (before, prefab_cache_epoch(world)) else {
        return;
    };
    let ours = now.wrapping_sub(before);
    if ours == 0 {
        return;
    }
    if let Some(mut last) = world.get_resource_mut::<crate::prefab::sync::LastResolvedEpoch>() {
        last.0 = last.0.wrapping_add(ours);
        debug!(
            "refused load: took {ours} prefab cache bump(s) as answered, \
             leaving the driver at epoch {}",
            last.0
        );
    }
}

#[derive(Resource)]
pub(super) enum SceneDialogTask {
    Save(Task<Option<FileHandle>>),
}

/// Whether a load put its document in the world.
///
/// Every refusal is fail-soft: the scene already open is left standing. Callers
/// that offered the load, such as the external-edit reload prompt, report the
/// refusal to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadOutcome {
    Loaded,
    Refused(LoadRefusal),
}

/// Why a load did not happen, in a form the editor can display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadRefusal {
    pub category: RefusalCategory,
    /// The same sentence the warn log carries.
    pub message: String,
}

/// The kinds of refusal, so a caller can lead with what went wrong before
/// showing the log sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalCategory {
    /// The file could not be read.
    Unreadable,
    /// The text is not a document this editor reads.
    Unparsable,
    /// The document names a vocabulary the editor removed.
    Retired,
    /// A legacy conversion could not be written.
    NotConverted,
    /// The document was accepted but did not reach the world.
    NotSpawned,
}

impl RefusalCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unreadable => "the file could not be read",
            Self::Unparsable => "the file is not a scene this editor can read",
            Self::Retired => "the scene uses components this editor has removed",
            Self::NotConverted => "the legacy scene could not be converted",
            Self::NotSpawned => "the scene could not be spawned",
        }
    }
}

fn refuse(category: RefusalCategory, message: String) -> LoadOutcome {
    warn!("{message}");
    LoadOutcome::Refused(LoadRefusal { category, message })
}

pub fn load_scene_from_file(world: &mut World, chosen: &std::path::Path) {
    finish_load_scene(world, chosen);
}

/// [`load_scene_from_file`] for a caller that has to report a refusal.
pub fn load_scene_from_file_with_outcome(
    world: &mut World,
    chosen: &std::path::Path,
) -> LoadOutcome {
    finish_load_scene(world, chosen)
}

fn finish_load_scene(world: &mut World, chosen: &std::path::Path) -> LoadOutcome {
    let mut path = chosen.to_string_lossy().to_string();

    let json = match std::fs::read_to_string(&path) {
        Ok(json) => json,
        Err(err) => {
            return refuse(
                RefusalCategory::Unreadable,
                format!("failed to read scene file '{path}': {err}"),
            );
        }
    };

    // Only update `last_directory` once the file has been successfully read
    // and we're committed to the load. A failed read must NOT leak a stale
    // path into the dialog state.
    world.resource_mut::<SceneFilePath>().last_directory =
        chosen.parent().map(std::path::Path::to_path_buf);

    let mut loaded_hash: Option<u64> = None;

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
                return refuse(
                    RefusalCategory::Unparsable,
                    format!("failed to deserialize legacy scene: {err}"),
                );
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
        // not imported directly: they convert to a `.bsn` document, and the
        // editor opens that. The conversion is held in memory until the
        // document below is accepted, so a refused scene leaves neither a
        // converted file nor a renamed original behind. Interactive open paths
        // confirm with the user before reaching here. The legacy metadata and
        // camera framing carry over below.
        let (bsn_text, legacy_jsn, pending_conversion) = if path.ends_with(".bsn") {
            (json, None, None)
        } else {
            let jsn = match jackdaw_jsn::format::parse_scene(&json) {
                Ok((jsn, version)) => {
                    if version[0] < 2 {
                        return refuse(
                            RefusalCategory::Unparsable,
                            format!(
                                "JSN format version {version:?} is not supported. Please re-save with the latest editor."
                            ),
                        );
                    }
                    if version[0] < 3 {
                        info!("Migrating JSN v2 scene to v3 format");
                    }
                    jsn
                }
                Err(err) => {
                    return refuse(
                        RefusalCategory::Unparsable,
                        format!("failed to parse JSN file: {err}"),
                    );
                }
            };
            let pending =
                match crate::jsn_to_bsn::convert_scene_file_pending(world, Path::new(&path)) {
                    Ok(pending) => pending,
                    Err(err) => {
                        return refuse(
                            RefusalCategory::NotConverted,
                            format!("failed to convert legacy scene '{path}': {err}"),
                        );
                    }
                };
            (pending.scene_bsn.clone(), Some(jsn), Some(pending))
        };

        // Hash of the file this load becomes: the `.bsn` just read, or the one
        // the conversion below writes. Taken from what was read rather than
        // from a later look at disk, so an edit landing in between still
        // registers as an external edit.
        loaded_hash = Some(crate::scenes::external_watch::hash_bytes(
            bsn_text.as_bytes(),
        ));

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

        // Parse and validate before touching the open scene, so a rejected
        // document does not also wipe the scene already loaded.
        let mut authored = match jackdaw_bsn::parse_bsn_text(&bsn_text) {
            Ok(authored) => authored,
            Err(err) => {
                return refuse(
                    RefusalCategory::Unparsable,
                    format!("failed to parse BSN scene '{path}': {err}"),
                );
            }
        };

        // A saved scene names its prefabs relative to itself, so the file
        // travels. In memory they are absolute: the cache is keyed by path, and
        // readers would otherwise each need this scene's directory to resolve a
        // reference.
        jackdaw_prefab::absolutize_isa_sources(&mut authored, &parent_path);

        // A legacy scene's prefabs may be legacy too, and the cache reads
        // `.bsn` only. They convert here rather than with the rest of the
        // conversion below, because the resolve that needs them comes next and
        // the retarget after it looks the converted files up on disk.
        if let Some(pending) = &pending_conversion {
            crate::jsn_to_bsn::convert_prefab_dependencies(world, pending);
        }
        crate::prefab::save_load::retarget_isa_sources(&mut authored, &parent_path);

        // Populate the prefab cache from the document's IsA references, then
        // resolve instances so the spawn produces complete entities. A
        // resolution failure (e.g. cycle) falls back to the authored text so
        // the editor stays usable. Worlds without a prefab cache (headless
        // harnesses) spawn the authored text directly.
        //
        // This runs before the retired-component gate below, and before the
        // open scene is cleared: components merged in from a prefab are subject
        // to that gate too, and a refusal must leave the open scene standing.
        let epoch_before = prefab_cache_epoch(world);
        let resolved: Option<jackdaw_bsn::SceneBsnAst> =
            if world.contains_resource::<crate::prefab::PrefabAstCache>() {
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
                    Ok(resolved) => Some(resolved),
                    Err(e) => {
                        warn!("prefab resolution failed: {e}; spawning unresolved scene");
                        None
                    }
                }
            } else {
                None
            };

        if let Err(err) =
            jackdaw_bsn::reject_retired_ui_components(resolved.as_ref().unwrap_or(&authored))
        {
            forget_prefab_cache_bump(world, epoch_before);
            return refuse(
                RefusalCategory::Retired,
                format!("cannot load scene '{path}': {err}"),
            );
        }
        let scene_kind = declared_scene_kind(&authored);
        let resolved_text = match &resolved {
            Some(resolved) => jackdaw_bsn::emit_scene(resolved),
            None => bsn_text.clone(),
        };

        // The document parsed and passed the gate, so the conversion can be
        // written to disk.
        if let Some(pending) = pending_conversion {
            let bsn_path = pending.bsn_path.clone();
            if let Err(err) = crate::jsn_to_bsn::commit_conversion(world, pending) {
                forget_prefab_cache_bump(world, epoch_before);
                return refuse(
                    RefusalCategory::NotConverted,
                    format!(
                        "failed to write converted scene '{}': {err}",
                        bsn_path.display()
                    ),
                );
            }
            info!(
                "Converted legacy scene to {}; original kept as .jsn.bak",
                bsn_path.display()
            );
            path = bsn_path.to_string_lossy().into_owned();
        }

        clear_scene_entities(world);

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
                return refuse(
                    RefusalCategory::NotSpawned,
                    format!("failed to load BSN scene '{path}': {err}"),
                );
            }
        }

        // The document's own kind picks the mode, so a reopened flat scene
        // comes back on its canvas. Only a UI screen also fronts the panel:
        // an ordinary scene must not yank the viewport over whatever the user
        // was working in.
        let mode = crate::viewport_host::ViewportMode::for_scene_kind(scene_kind);
        if scene_kind == crate::scenes::operators::SceneKind::Ui {
            crate::viewport_host::focus_viewport(world, mode);
            // Without a fit, a 1920x1080 reference shown at 100% in a dock
            // leaf reveals only the scene's top-left corner.
            crate::viewport_2d::request_2d_fit(world);
        } else {
            crate::viewport_host::set_viewport_mode(world, mode, false);
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

    if let Some(hash) = loaded_hash {
        crate::scenes::external_watch::note_known_hash(world, Path::new(&path), hash);
    }
    world.resource_mut::<SceneFilePath>().path = Some(path);

    // A UI root authored before the root stated a size of its own shrinks to
    // fit its content, and every placement then resolves against that box
    // rather than against the canvas.
    crate::ui_palette::backfill_ui_root_size(world);

    // Stacks were cleared by clear_scene_entities, so dirty baseline is 0
    world.resource_mut::<SceneDirtyState>().undo_len_at_save = 0;

    LoadOutcome::Loaded
}

/// Type name of the UI scene root marker. Matched on the last path segment, so
/// a hand-authored document may name it short or fully qualified.
const UI_SCENE_ROOT_TYPE: &str = "UiSceneRoot";

/// [`UI_SCENE_ROOT_TYPE`] as the tail of a qualified path.
const UI_SCENE_ROOT_PATH_TAIL: &str = "::UiSceneRoot";

/// Does this document declare a UI scene?
///
/// Walks every patch component rather than following `Children` from the roots,
/// so a root nested inside a subtree is still found. Callers use this to decide
/// whether an open or a tab switch brings the viewport forward, and read the
/// mode to put it in from [`declared_scene_kind`]; live routing uses a
/// `UiSceneRoot` query on the spawned world instead.
pub fn declares_ui_scene_root(ast: &jackdaw_bsn::SceneBsnAst) -> bool {
    ast.all_patch_type_paths().any(is_ui_scene_root_type_path)
}

/// Does this component type path name the UI scene root marker?
///
/// Shared with [`declares_ui_scene_root`] so that a caller asking about a
/// single entity, such as the prefab importer, matches the document-wide walk.
pub fn is_ui_scene_root_type_path(path: &str) -> bool {
    path == UI_SCENE_ROOT_TYPE || path.ends_with(UI_SCENE_ROOT_PATH_TAIL)
}

/// Type name of the 2D world scene root marker, matched the same way as
/// [`UI_SCENE_ROOT_TYPE`].
const SCENE_2D_ROOT_TYPE: &str = "Scene2dRoot";

/// [`SCENE_2D_ROOT_TYPE`] as the tail of a qualified path.
const SCENE_2D_ROOT_PATH_TAIL: &str = "::Scene2dRoot";

/// Which kind of scene this document is, read from the root markers a save
/// writes.
///
/// The counterpart of the kind a `scene.new` was given: a reopened document
/// carries no operator parameter, so its kind has to be read back out of it.
/// Walks every patch component, like [`declares_ui_scene_root`], so a marker
/// nested inside a subtree still counts. A document carrying neither marker is
/// a 3D scene, which is what the editor has always assumed.
pub fn declared_scene_kind(ast: &jackdaw_bsn::SceneBsnAst) -> crate::scenes::operators::SceneKind {
    use crate::scenes::operators::SceneKind;

    let mut two_d = false;
    for path in ast.all_patch_type_paths() {
        if is_ui_scene_root_type_path(path) {
            // A UI screen is the more specific kind: a document declaring both
            // is authored as UI.
            return SceneKind::Ui;
        }
        two_d |= path == SCENE_2D_ROOT_TYPE || path.ends_with(SCENE_2D_ROOT_PATH_TAIL);
    }
    if two_d {
        SceneKind::TwoD
    } else {
        SceneKind::ThreeD
    }
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
    /// Fill paths the store has never heard of, and re-read the ones
    /// whose file has been written since the store last read it.
    ///
    /// Between [`Self::Reload`] and [`Self::FillMissing`]: the store's
    /// copy is the truth while it holds edits nobody has written, and the
    /// file is the truth once someone has written it. Used when returning
    /// to a tab with nothing unsaved in it, where a sidecar rewritten in
    /// the meantime is the newer of the two.
    RefreshChanged,
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
    match mode {
        SidecarImport::Reload => {}
        SidecarImport::FillMissing => {
            let store = world.resource::<crate::terrain::TerrainDataStore>();
            wanted.retain(|(path, _)| !store.contains(path));
        }
        SidecarImport::RefreshChanged => {
            let store = world.resource::<crate::terrain::TerrainDataStore>();
            wanted.retain(|(path, _)| {
                !store.contains(path)
                    || sidecar::resolve_path(&scene_dir, path)
                        .is_ok_and(|full| store.sidecar_is_stale(path, &full))
            });
        }
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
            // `load` takes either format version: a sidecar written before
            // regions existed migrates on the way in, and the next save
            // writes it back as the current version.
            Ok(bytes) => match sidecar::load(&bytes) {
                Ok(data) => {
                    let mtime = std::fs::metadata(&full)
                        .and_then(|meta| meta.modified())
                        .ok();
                    let mut store = world.resource_mut::<crate::terrain::TerrainDataStore>();
                    store.insert(data_path.clone(), data);
                    store.note_read(&data_path, mtime);
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
    // Terrains are placed by now, which is what a group saved beside one
    // has to be moved into the space of.
    crate::terrain::scatter::migrate_legacy_scatter_groups(world);
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

/// Spawn default lighting for a new / empty scene (Sun directional
/// light + no ambient). The ambient override is always applied since
/// it is a `Resource` mutation, not a spawn.
///
/// The Sun is a new-scene template, not an editor fixture: it is registered in
/// the document and saved like any other authored entity. It is therefore
/// seeded only into a world holding no document, since the binary calls this on
/// entering the editor, after the project's persisted tabs have opened, and a
/// document whose author kept no light must not gain one on the next save.
///
/// Also skipped when a `DirectionalLight` already exists, so a scene that
/// carries its own lighting does not get a second one.
pub fn spawn_default_lighting(world: &mut World) {
    world.insert_resource(GlobalAmbientLight::NONE);

    let holds_document = world
        .get_resource::<jackdaw_bsn::SceneBsnAst>()
        .is_some_and(|ast| !ast.roots.is_empty());
    if holds_document {
        return;
    }

    let has_directional = world
        .query::<&DirectionalLight>()
        .iter(world)
        .next()
        .is_some();
    if has_directional {
        return;
    }

    let sun = world
        .spawn((
            Name::new("Sun"),
            DirectionalLight {
                shadow_maps_enabled: true,
                illuminance: 10000.0,
                ..default()
            },
            Transform::from_xyz(10.0, 20.0, 10.0).with_rotation(Quat::from_euler(
                EulerRot::XYZ,
                -0.8,
                0.4,
                0.0,
            )),
        ))
        .id();
    register_entity_in_ast(world, sun);
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

/// Drop every drag that is holding on to a scene entity.
///
/// A drag remembers the entity ids it grabbed and keeps writing transforms onto
/// them every frame. A reload or a tab switch replaces the whole scene, leaving
/// those ids either despawned or reused by unrelated entities, so every drag
/// ends with the scene it belongs to.
fn forget_dragged_entities(world: &mut World) {
    if let Some(mut drag) = world.get_resource_mut::<crate::modal_transform::ViewportDragState>() {
        drag.pending = None;
        drag.active = None;
    }
    if let Some(mut gizmo) = world.get_resource_mut::<crate::gizmos::GizmoDragState>() {
        gizmo.active = false;
        gizmo.axis = None;
        gizmo.targets.clear();
        gizmo.camera = None;
        gizmo.viewport = None;
    }

    // Every other gesture that parks entity ids in a resource for the length of
    // a drag: the canvas gesture on the 2D stage, and the brush sub-element
    // modals, which also hold the brush they were started on.
    use crate::brush::topology_ops as topo;
    forget_drag_resource::<crate::ui_stage::UiManipulation>(world);
    forget_drag_resource::<topo::extrude::ExtrudeModalState>(world);
    forget_drag_resource::<topo::inset::InsetModalState>(world);
    forget_drag_resource::<topo::edge_bevel::EdgeBevelModalState>(world);
    forget_drag_resource::<topo::vertex_bevel::VertexBevelModalState>(world);
    forget_drag_resource::<topo::edge_slide_modal::EdgeSlideModalState>(world);
    forget_drag_resource::<topo::vertex_slide_modal::VertexSlideModalState>(world);
}

/// Put one drag-state resource back to its resting value, if the app has it.
fn forget_drag_resource<R: Resource<Mutability = bevy::ecs::component::Mutable> + Default>(
    world: &mut World,
) {
    if world.contains_resource::<R>() {
        *world.resource_mut::<R>() = R::default();
    }
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
    // The rows the outliner was still waiting on, and the ones it gave up on,
    // named entities in the scene that is going away.
    crate::hierarchy::forget_withheld_rows(world);

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
    forget_dragged_entities(world);
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

                // The dialog resolves against the active tab, which need not be
                // the one it was opened over: the user may switch tabs while
                // the picker is up. Re-check before the retarget below renames
                // a tab whose world is not its file.
                if let Some(reason) = crate::scene_io::save::active_tab_refusal(world) {
                    error!(
                        "Cannot save this tab: its scene was not loaded ({reason}). \
                         Fix the file and reopen it; nothing has been written."
                    );
                    crate::status_bar::notify_error(
                        world,
                        "Not saved: this tab's scene was not loaded".to_string(),
                    );
                    return;
                }

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

/// Tests for the UI-scene detection that decides whether opening or activating
/// a document brings the 2D viewport forward. Covers the cases a roots-only
/// walk would miss: a root nested in a subtree, and the short type path a
/// hand-authored document may use.
#[cfg(test)]
mod ui_scene_detection_tests {
    use super::declares_ui_scene_root;

    fn declares(bsn: &str) -> bool {
        let ast = jackdaw_bsn::parse_bsn_text(bsn).expect("the fixture parses");
        declares_ui_scene_root(&ast)
    }

    #[test]
    fn a_fully_qualified_ui_scene_root_is_detected() {
        assert!(declares(
            r#"
#Overlay
jackdaw_scene_types::UiSceneRoot
"#
        ));
    }

    #[test]
    fn the_short_form_a_hand_authored_document_may_use_is_detected() {
        // The runtime resolver accepts a short type path, so the editor has to
        // agree with it about what counts as a UI scene.
        assert!(declares(
            r#"
#Overlay
UiSceneRoot
"#
        ));
    }

    #[test]
    fn a_root_nested_in_a_subtree_is_detected() {
        assert!(
            declares(
                r#"
bevy_ecs::hierarchy::Children [
    #World
    bevy_transform::components::transform::Transform
    Children [
        #Overlay
        jackdaw_scene_types::UiSceneRoot { reference_size: glam::UVec2 { x: 800, y: 600 } }
    ]
]
"#
            ),
            "a walk that only visited document roots would miss this"
        );
    }

    #[test]
    fn a_scene_with_no_ui_root_is_not_a_ui_scene() {
        assert!(!declares(
            r#"
#World
bevy_transform::components::transform::Transform
bevy_camera::visibility::Visibility::Inherited
"#
        ));
    }

    #[test]
    fn a_type_merely_ending_in_the_name_is_not_a_ui_root() {
        // Suffix matching is on a whole path segment, so a longer type name
        // ending in the same letters must not focus the panel.
        assert!(!declares(
            r#"
#World
some_crate::NotAUiSceneRoot
"#
        ));
    }
}

/// Tests for reading a saved document's kind back out of it, which is how a
/// reopened scene reaches the viewport mode it was authored in.
#[cfg(test)]
mod declared_scene_kind_tests {
    use super::declared_scene_kind;
    use crate::scenes::operators::SceneKind;

    fn kind_of(bsn: &str) -> SceneKind {
        let ast = jackdaw_bsn::parse_bsn_text(bsn).expect("the fixture parses");
        declared_scene_kind(&ast)
    }

    #[test]
    fn a_document_with_a_ui_root_is_a_ui_scene() {
        assert_eq!(
            kind_of(
                r#"
#Overlay
UiSceneRoot
"#
            ),
            SceneKind::Ui
        );
    }

    #[test]
    fn a_qualified_2d_root_is_a_2d_scene() {
        assert_eq!(
            kind_of(
                r#"
#World
jackdaw_scene_types::Scene2dRoot
"#
            ),
            SceneKind::TwoD
        );
    }

    #[test]
    fn a_2d_root_nested_in_a_subtree_is_found() {
        assert_eq!(
            kind_of(
                r#"
bevy_ecs::hierarchy::Children [
    #World
    bevy_transform::components::transform::Transform
    Children [
        #Root
        jackdaw_scene_types::Scene2dRoot
    ]
]
"#
            ),
            SceneKind::TwoD,
            "a walk that only visited document roots would miss this"
        );
    }

    #[test]
    fn a_document_with_neither_marker_is_a_3d_scene() {
        assert_eq!(
            kind_of(
                r#"
#World
bevy_transform::components::transform::Transform
"#
            ),
            SceneKind::ThreeD
        );
    }

    #[test]
    fn a_type_merely_ending_in_the_2d_root_name_is_not_a_2d_scene() {
        assert_eq!(
            kind_of(
                r#"
#World
some_crate::NotAScene2dRoot
"#
            ),
            SceneKind::ThreeD
        );
    }
}

/// Tests for reading terrain sidecars back into the store.
///
/// A terrain reaches the world by two routes: `finish_load_scene` for an
/// explicit open, and `scenes::swap::activate_tab` for a tab pushed onto the
/// strip by `scene_open_system`. Only the first takes disk as the truth;
/// treating the second as a reload would discard unsaved sculpting on every tab
/// switch.
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

    /// A world with one terrain naming `data_path`, and a sidecar for it
    /// written beside `zone.bsn` in a fresh temp dir. The sidecar is in the
    /// pre-region format, which is the migration path existing projects take.
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
