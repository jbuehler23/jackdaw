use std::collections::HashSet;
use std::path::{Path, PathBuf};

use bevy::{
    ecs::reflect::AppTypeRegistry,
    prelude::*,
    tasks::{Task, futures_lite::future},
};
use rfd::FileHandle;
use serde::de::DeserializeSeed;

use jackdaw_api_internal::operator::warn_caller;

use crate::EditorEntity;

use super::SceneFilePath;
use super::registration::{register_entities_in_ast, register_entity_in_ast};

pub(crate) fn prefab_cache_epoch(world: &World) -> Option<u64> {
    world
        .get_resource::<crate::prefab::PrefabAstCache>()
        .map(crate::prefab::PrefabAstCache::epoch)
}

/// Mark the prefab-cache bumps this load caused as answered, so the driver
/// that respawns the scene on a cache change does not respawn the scene that
/// was just built from that very cache. A bump that was already pending stays
/// pending.
pub(crate) fn forget_prefab_cache_bump(world: &mut World, before: Option<u64>) {
    let (Some(before), Some(now)) = (before, prefab_cache_epoch(world)) else {
        return;
    };
    let ours = now.wrapping_sub(before);
    if ours == 0 {
        return;
    }
    if let Some(mut cache) = world.get_resource_mut::<crate::prefab::PrefabAstCache>() {
        cache.answer_changes_after(before);
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
    Open(Task<Option<FileHandle>>),
    Save(Task<Option<FileHandle>>),
}

/// Open the scene file picker behind File > Open. No-op while another
/// scene dialog is already up.
pub fn spawn_open_dialog(world: &mut World) {
    if world.contains_resource::<SceneDialogTask>() {
        return;
    }
    let dialog =
        crate::native_dialog::file_dialog(world, crate::native_dialog::DialogPurpose::Scene)
            .set_title("Open scene")
            .add_filter("Jackdaw scene", &["bsn", "bsb", "jsn"]);
    let task =
        bevy::tasks::AsyncComputeTaskPool::get().spawn(async move { dialog.pick_file().await });
    world.insert_resource(SceneDialogTask::Open(task));
}

/// Whether a load put its document in the world. Every refusal is fail-soft:
/// the scene already open is left standing.
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

/// The kinds of refusal, so a caller can lead with what went wrong.
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
    let _span = info_span!("open.finish_load").entered();
    super::save_worker::land_saves(world);
    let mut path = chosen.to_string_lossy().to_string();

    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) => {
            return refuse(
                RefusalCategory::Unreadable,
                format!("failed to read scene file '{path}': {err}"),
            );
        }
    };
    let json = match jackdaw_bsn::document_text_from_bytes(&bytes, Path::new(&path)) {
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
    let mut pending_upgrade = None;

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
        // document below is accepted, so a refused scene leaves nothing behind.
        // The legacy metadata and camera framing carry over below.
        let (bsn_text, legacy_jsn, pending_conversion) = if !path.ends_with(".jsn") {
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

        // Hash of what was read rather than of a later look at disk, so an edit
        // landing in between still registers as an external edit, and as the
        // bytes it was stored as, which is what the watcher reads back.
        loaded_hash = Some(crate::scenes::external_watch::hash_bytes(
            match legacy_jsn.is_none() {
                true => &bytes,
                false => bsn_text.as_bytes(),
            },
        ));

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

        // A saved scene names its prefabs under the assets folder; in memory
        // they are absolute, since the cache is keyed by path.
        let assets_root = crate::prefab::save_load::source_root(world, &parent_path);
        for stray in
            crate::prefab::save_load::relativize_for_file(world, &mut authored, &parent_path)
        {
            warn!(
                "scene '{path}': the prefab source '{stray}' names a place on this machine \
                 rather than a file under the project's assets folder"
            );
        }
        jackdaw_prefab::absolutize_isa_sources(&mut authored, &assets_root, &parent_path);
        if !crate::scenes::operators::document_is_prefab(&authored) {
            let registry = world.resource::<AppTypeRegistry>().clone();
            crate::scene_nodes::name_scene_nodes(&mut authored, &registry.read());
        }

        // A legacy scene's prefabs may be legacy too, and the cache reads
        // `.bsn` only. They convert here because the resolve that needs them
        // comes next.
        if let Some(pending) = &pending_conversion {
            crate::jsn_to_bsn::convert_prefab_dependencies(world, pending);
        }
        crate::prefab::save_load::retarget_isa_sources(&mut authored, &assets_root, &parent_path);

        // Populate the prefab cache from the document's IsA references, then
        // resolve instances so the spawn produces complete entities. A
        // resolution failure (e.g. cycle) falls back to the authored text so
        // the editor stays usable. Worlds without a prefab cache (headless
        // harnesses) spawn the authored text directly.
        //
        // Runs before the retired-component gate and before the open scene is
        // cleared: merged-in components face that gate too, and a refusal must
        // leave the open scene standing.
        let epoch_before = prefab_cache_epoch(world);
        let mut upgrade = None;
        let resolved: Option<jackdaw_bsn::SceneBsnAst> = if world
            .contains_resource::<crate::prefab::PrefabAstCache>()
        {
            {
                let mut cache = world.resource_mut::<crate::prefab::PrefabAstCache>();
                crate::prefab::save_load::populate_cache_for_scene_bsn(
                    &authored,
                    &mut cache,
                    &assets_root,
                    &parent_path,
                );
            }
            if !crate::scenes::operators::document_is_prefab(&authored) {
                upgrade = crate::lod_upgrade::upgrade_on_open(world, &mut authored, &assets_root);
            }
            let missing = crate::prefab::save_load::missing_source_complaints(
                &authored,
                world.resource::<crate::prefab::PrefabAstCache>(),
                &path,
            );
            for complaint in missing {
                warn_caller(world, complaint);
            }
            let cache = world.resource::<crate::prefab::PrefabAstCache>();
            let get_prefab = |p: &Path| cache.get(p);
            match crate::prefab::resolver_bsn::resolve_scene(&authored, &get_prefab) {
                Ok(mut resolved) => {
                    if !crate::scenes::operators::document_is_prefab(&authored) {
                        let registry = world.resource::<AppTypeRegistry>().clone();
                        crate::scene_nodes::separate_from_inherited_ids(
                            &mut resolved,
                            &registry.read(),
                        );
                    }
                    Some(resolved)
                }
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
        pending_upgrade = upgrade;

        // The document passed the gate, so the conversion can be written.
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
        crate::asset_index::index_files_named_in(world, || match &resolved {
            Some(resolved) => std::borrow::Cow::Owned(jackdaw_bsn::emit_scene(resolved)),
            None => std::borrow::Cow::Borrowed(bsn_text.as_str()),
        });

        // The resolved document is spawned as it stands, not written out and
        // read back. Without one the file's own text is what spawns.
        let loading = match resolved {
            Some(resolved) => Ok(jackdaw_bsn::load_bsn_document(world, resolved)),
            None => jackdaw_bsn::load_bsn_scene(world, &bsn_text),
        };
        match loading {
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

        // The document's own kind picks the mode. Only a UI screen also fronts
        // the panel; an ordinary scene must not yank the viewport.
        let mode = crate::viewport_host::mode_for_scene_kind(scene_kind);
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
            scene_path.metadata = super::jsn_scene_metadata(jsn.metadata);
        }
    }

    // Bulk data lives beside the scene rather than in it. An explicit load
    // means disk is the truth, so this overwrites whatever was held for it.
    super::hooks::import_sidecars(world, &path, SidecarImport::Reload);

    if let Some(hash) = loaded_hash {
        crate::scenes::external_watch::note_known_hash(world, Path::new(&path), hash);
    }
    world.resource_mut::<SceneFilePath>().path = Some(path.clone());

    // A UI root authored before roots stated a size shrinks to fit its
    // content, and every placement then resolves against that box.
    crate::ui_palette::backfill_ui_root_size(world);

    world
        .resource_mut::<jackdaw_commands::CommandHistory>()
        .mark_current_saved();
    if let Some(upgrade) = pending_upgrade {
        crate::lod_upgrade::finish_upgrade(world, upgrade);
    }
    world.trigger(jackdaw_api_internal::document::SceneOpened {
        path: Some(PathBuf::from(path)),
    });

    LoadOutcome::Loaded
}

/// Type name of the UI scene root marker. Matched on the last path segment, so
/// a hand-authored document may name it short or fully qualified.
const UI_SCENE_ROOT_TYPE: &str = "UiSceneRoot";

/// `UI_SCENE_ROOT_TYPE` as the tail of a qualified path.
const UI_SCENE_ROOT_PATH_TAIL: &str = "::UiSceneRoot";

/// Does this document declare a UI scene?
///
/// Walks every patch component rather than following `Children` from the roots,
/// so a root nested inside a subtree is still found.
pub fn declares_ui_scene_root(ast: &jackdaw_bsn::SceneBsnAst) -> bool {
    ast.all_patch_type_paths().any(is_ui_scene_root_type_path)
}

/// Does this component type path name the UI scene root marker? Shared with
/// [`declares_ui_scene_root`] so a per-entity caller matches the document walk.
pub fn is_ui_scene_root_type_path(path: &str) -> bool {
    path == UI_SCENE_ROOT_TYPE || path.ends_with(UI_SCENE_ROOT_PATH_TAIL)
}

/// Type name of the 2D world scene root marker, matched the same way as
/// `UI_SCENE_ROOT_TYPE`.
const SCENE_2D_ROOT_TYPE: &str = "Scene2dRoot";

/// `SCENE_2D_ROOT_TYPE` as the tail of a qualified path.
const SCENE_2D_ROOT_PATH_TAIL: &str = "::Scene2dRoot";

/// Which kind of scene this document is, read from the root markers a save
/// writes. A document carrying neither marker is a 3D scene.
pub fn declared_scene_kind(ast: &jackdaw_bsn::SceneBsnAst) -> crate::scenes::operators::SceneKind {
    use crate::scenes::operators::SceneKind;

    let mut two_d = false;
    for path in ast.all_patch_type_paths() {
        if is_ui_scene_root_type_path(path) {
            // A document declaring both is authored as UI.
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
pub enum SidecarImport {
    /// Disk wins. Used when the user explicitly loads or reloads a scene.
    Reload,
    /// Only fill paths the store has never heard of.
    FillMissing,
    /// Fill paths the store has never heard of, and re-read the ones holding
    /// no unsaved edits whose file has been written since the store last read
    /// it. Used on tab activation.
    RefreshChanged,
}

/// Spawn default lighting for a new / empty scene (Sun directional
/// light + no ambient). The ambient override is always applied since
/// it is a `Resource` mutation, not a spawn.
///
/// The Sun is registered in the document and saved like any authored entity,
/// so it is seeded only into a world holding no document and no
/// `DirectionalLight`: a scene whose author kept no light must not gain one on
/// the next save.
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
/// A drag keeps writing transforms onto the ids it grabbed, which a reload or
/// tab switch leaves despawned or reused by unrelated entities.
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
    // a drag.
    forget_drag_resource::<crate::ui_stage::UiManipulation>(world);
    forget_drag_resource::<crate::sprite_gizmo::SpriteManipulation>(world);
    forget_drag_resource::<crate::sprite_stage::SpritePress>(world);
    super::hooks::forget_drags(world);
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
    let _span = info_span!("scene.clear").entered();
    super::save_worker::land_saves(world);
    if let Some(mut live) = world.get_resource_mut::<jackdaw_bsn::SceneBsnAst>() {
        jackdaw_bsn::drop_document(std::mem::take(&mut *live));
    }

    // Extensions' per-scene state belongs to the scene being cleared, not to the tab it was
    // in. A tab switch has stashed it by the time this runs (`capture_active_tab`).
    crate::scenes::reset_scene_resources(world);

    world
        .resource_mut::<crate::selection::Selection>()
        .entities
        .clear();

    let rows = info_span!("outliner.clear")
        .in_scope(|| world.run_system_cached(crate::hierarchy::clear_all_tree_rows));
    if let Err(err) = rows {
        error!("Failed to clear tree rows: {err}");
    }
    // The rows the outliner was still waiting on, and the ones it gave up on,
    // named entities in the scene that is going away.
    crate::hierarchy::forget_withheld_rows(world);

    // Clear undo/redo stacks; they hold entity references that become
    // stale when the scene is dropped. Callers who want to preserve
    // history (e.g. undo/redo itself) use `despawn_scene_entities`
    // directly.
    world
        .resource_mut::<jackdaw_commands::CommandHistory>()
        .clear();

    if let Err(err) = despawn_scene_entities(world) {
        error!("clear_scene_entities failed: {err}");
    }
}

/// Despawn every non-editor scene entity, leaving editor infrastructure
/// (cameras, grids, gizmos) and the undo/redo stacks intact. Used by
/// snapshot apply during undo/redo.
///
/// Input action entities carry `ActionSettings`, so excluding those keeps
/// every operator's input routing alive across an `apply_ast_to_world`
/// pass, whatever names they were given; without its action entities a
/// context fires nothing and every editor keybind goes silent.
///
/// An entity the document spawned answers to `AstNodeRef` whether or not it
/// carries a name, so an instance that inherited nothing leaves with its
/// scene rather than standing in the next one.
pub(crate) fn despawn_scene_entities(world: &mut World) -> Result<(), BevyError> {
    let _span = info_span!("scene.despawn").entered();
    forget_dragged_entities(world);
    let editor_set = world.run_system_cached(collect_editor_entities)?;

    let roots: Vec<Entity> = world
        .query_filtered::<Entity, (
            Or<(With<Name>, With<jackdaw_bsn::AstNodeRef>)>,
            Without<jackdaw_input::ActionSettings>,
        )>()
        .iter(world)
        .filter(|e| !editor_set.contains(e))
        .collect();

    let scene_set = collect_subtree(world, roots);

    // Models still queued for these entities belong to the scene going away.
    // Whoever respawns asks for them again, and holding them would both keep
    // the glTFs they name alive and hand render roots to entities of a scene
    // that is no longer open.
    crate::entity_ops::forget_model_roots(world, &scene_set);

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
        SceneDialogTask::Open(t) => {
            let Some(result) = future::block_on(future::poll_once(t)) else {
                world.insert_resource(task); // Not ready, put it back
                return;
            };
            if let Some(file) = result {
                let path = file.path().to_path_buf();
                crate::native_dialog::remember_pick(
                    world,
                    crate::native_dialog::DialogPurpose::Scene,
                    &path,
                );
                crate::migrate_dialog::request_open_with_conversion(world, &path);
            }
        }
        SceneDialogTask::Save(t) => {
            let Some(result) = future::block_on(future::poll_once(t)) else {
                world.insert_resource(task); // Not ready, put it back
                return;
            };
            if let Some(file) = result {
                let path = file.path().to_path_buf();
                let last_dir = path.parent().map(std::path::Path::to_path_buf);

                // The dialog resolves against the active tab, which need not
                // be the one it was opened over, so re-check before the
                // retarget below renames a tab whose world is not its file.
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
                if let Err(why) =
                    crate::scene_io::retarget_active_scene(world, &path.to_string_lossy())
                {
                    error!("Cannot save to {}: {why}", path.display());
                    return;
                }
                world.resource_mut::<SceneFilePath>().last_directory = last_dir;
                crate::native_dialog::remember_pick(
                    world,
                    crate::native_dialog::DialogPurpose::Scene,
                    &path,
                );

                super::save_worker::save_active_tab_in_background(world);
            }
        }
    }
}

/// Tests for the UI-scene detection that decides whether opening or activating
/// a document brings the 2D viewport forward.
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
        // The runtime resolver accepts a short type path, so the editor agrees
        // with it about what counts as a UI scene.
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
        // Suffix matching is on a whole path segment.
        assert!(!declares(
            r#"
#World
some_crate::NotAUiSceneRoot
"#
        ));
    }
}

/// Tests for reading a saved document's kind back out of it.
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
