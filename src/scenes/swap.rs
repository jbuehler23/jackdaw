//! Tab-switch mechanics. The pure pipeline lives here so it can be
//! tested independently of UI and operator wiring.

use bevy::prelude::*;
use jackdaw_api::prelude::*;

use crate::commands::CommandHistory;
use crate::scene_io::clear_scene_entities;
use crate::scenes::{Scenes, TabContent, ViewState};

/// Switch the active tab to `target`. No-op if `target == active`.
/// Cancels any in-flight modal first to avoid corrupt per-frame state.
pub fn swap_active_tab(world: &mut World, target: usize) {
    let current = world.resource::<Scenes>().active;
    if current == target {
        return;
    }
    let tab_count = world.resource::<Scenes>().tabs.len();
    if target >= tab_count {
        warn!("swap_active_tab: target {target} out of range (len {tab_count})");
        return;
    }

    // Cancel any in-flight modal so per-frame state doesn't dangle.
    let _ = world.cancel_active_modal();

    capture_active_tab(world);
    clear_scene_entities(world);
    activate_tab(world, target);
    // The respawn above replaced every authored entity, so any live game
    // projection now points at stale ids; rebuild it against the new tab.
    crate::pie_projection::reproject_focused(world);
}

/// Spawn the target tab's snapshot into a world that has just been
/// cleared. Used by `scene_close_system` when the closed tab was the
/// active tab (so the normal `capture_active_tab` step would try to
/// re-capture a tab that's being dropped).
pub fn reactivate_after_close(world: &mut World, target: usize) {
    activate_tab(world, target);
    crate::pie_projection::reproject_focused(world);
}

/// Re-spawn the active tab's authored scene from the live AST, reverting any
/// overlaid live values. Equivalent to a tab switch to the same tab: captures
/// the current AST, clears scene entities, then re-spawns from the captured
/// snapshot. The AST itself is never mutated; it is the baseline that comes
/// back after the clear.
///
/// Used by PIE stop to revert projected live state after the game process ends.
/// Safe to call when nothing is projected (clear + respawn of an unchanged
/// scene is harmless).
pub(crate) fn respawn_scene_from_ast(world: &mut World) {
    let active = world.resource::<Scenes>().active;
    if world.resource::<Scenes>().tabs.is_empty() {
        return;
    }
    capture_active_tab(world);
    clear_scene_entities(world);
    activate_tab(world, active);
}

/// Capture the live scene document into the active tab and stash the
/// per-tab history and view state. Pre-condition: a tab exists at
/// `Scenes.active`.
pub(crate) fn capture_active_tab(world: &mut World) {
    let active = world.resource::<Scenes>().active;

    // A refused tab never got its document into the world, so there is nothing
    // to snapshot. The rest of the capture still applies, since its history and
    // terrain data were restored on activation; only the document snapshot is
    // skipped, leaving the tab holding the document it could not spawn.
    let refused = world.resource::<Scenes>().tabs[active].is_refused();

    let view_state = capture_view_state(world);
    let history = std::mem::take(&mut *world.resource_mut::<CommandHistory>());

    let (prefab_target, tab_path) = {
        let scenes = world.resource::<Scenes>();
        let tab = &scenes.tabs[active];
        let prefab = match &tab.content {
            TabContent::Prefab(path) => Some(path.clone()),
            TabContent::Scene(_) => None,
        };
        (prefab, tab.path.clone())
    };

    // Both branches snapshot the live document through the inline-asset
    // emit, which embeds runtime asset handles as `#Name` roots and
    // reduces inherited prefab-instance descendants to sparse overrides,
    // so re-activation re-resolves them against the prefab cache.
    let parent = tab_path
        .as_ref()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    if !refused {
        let text = crate::scene_io::emit_bsn_scene_with_inline_assets(world, &parent);

        if let Some(path) = prefab_target {
            // Prefab tab: flush the snapshot into the cache entry rather than
            // onto the tab, which keeps pointing at that entry through its
            // `TabContent::Prefab` key. `insert` overwrites or creates, bumps
            // the epoch, and marks the path dirty, covering both first capture
            // and re-capture without branching on existence.
            if let Ok(bsn) = jackdaw_bsn::parse_bsn_text(&text) {
                world
                    .resource_mut::<crate::prefab::PrefabAstCache>()
                    .insert(path.as_path(), bsn);
            }
        } else {
            // Scene tab: store the captured document directly on the tab.
            let doc = match jackdaw_bsn::parse_bsn_text(&text) {
                Ok(doc) => doc,
                Err(err) => {
                    warn!("capture_active_tab: snapshot parse failed: {err}");
                    jackdaw_bsn::SceneBsnAst::default()
                }
            };
            let mut scenes = world.resource_mut::<Scenes>();
            scenes.tabs[active].content = TabContent::Scene(Some(Box::new(doc)));
        }
    }

    let terrain_data_store = world
        .get_resource_mut::<crate::terrain::TerrainDataStore>()
        .map(|mut store| std::mem::take(&mut *store));
    let navmesh = crate::terrain::navmesh_bake::take_from_world(world);
    let mut scenes = world.resource_mut::<Scenes>();
    let tab = &mut scenes.tabs[active];
    tab.view_state = view_state;
    tab.history = history;
    if let Some(terrain_data_store) = terrain_data_store {
        tab.terrain_data_store = terrain_data_store;
    }
    if let Some(navmesh) = navmesh {
        tab.navmesh = navmesh;
    }
}

/// Spawn the target tab's document into the live world and restore per-tab
/// history and view state.
pub fn activate_tab(world: &mut World, target: usize) {
    let has_terrain_data_store = world.contains_resource::<crate::terrain::TerrainDataStore>();
    let (mut content, view_state, history, tab_path, terrain_data_store, navmesh) = {
        let mut scenes = world.resource_mut::<Scenes>();
        let tab = &mut scenes.tabs[target];
        (
            std::mem::take(&mut tab.content),
            std::mem::take(&mut tab.view_state),
            std::mem::take(&mut tab.history),
            tab.path.clone(),
            has_terrain_data_store.then(|| std::mem::take(&mut tab.terrain_data_store)),
            std::mem::take(&mut tab.navmesh),
        )
    };

    // Restore the target tab's bulk terrain data before spawning its scene
    // and importing any sidecars. `FillMissing` can now preserve unsaved
    // edits without confusing a same-named sidecar from another tab.
    if let Some(terrain_data_store) = terrain_data_store {
        *world.resource_mut::<crate::terrain::TerrainDataStore>() = terrain_data_store;
    }
    // The bake taken from that ground, restored with it.
    crate::terrain::navmesh_bake::install_in_world(world, navmesh);

    // Materialize the document to install. For `Prefab` tabs, clone from
    // the cache; for `Scene` tabs, take the captured document (or default).
    let new_doc = match &mut content {
        TabContent::Scene(slot) => slot.take().map(|b| *b).unwrap_or_default(),
        TabContent::Prefab(path) => world
            .get_resource::<crate::prefab::PrefabAstCache>()
            .and_then(|c| c.get_canonical(path))
            .map(crate::prefab::resolver_bsn::clone_scene)
            .unwrap_or_default(),
    };

    // A tab switch installs a document the same way an open does, so it sets
    // the viewport the same way: `finish_load_scene` reads the document's kind
    // for the mode and brings the panel forward for a UI scene, and a swap that
    // skipped it would leave the scene loaded behind whatever panel was in
    // front, in whatever mode the tab before it left.
    let scene_kind = crate::scene_io::declared_scene_kind(&new_doc);

    // Mirror `finish_load_scene`: any IsA references in the captured
    // document need their prefab files loaded into the cache, then resolved
    // (materializing inherited subtrees), before the spawn. PrefabAstCache
    // may be absent in minimal test harnesses; if so, skip the resolver
    // step entirely (there can't be any cached prefabs to merge against).
    let parent = tab_path
        .as_ref()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let resolved: Option<jackdaw_bsn::SceneBsnAst> = if world
        .get_resource::<crate::prefab::PrefabAstCache>()
        .is_some()
    {
        {
            let mut cache = world.resource_mut::<crate::prefab::PrefabAstCache>();
            crate::prefab::save_load::populate_cache_for_scene_bsn(&new_doc, &mut cache, &parent);
        }
        let cache = world.resource::<crate::prefab::PrefabAstCache>();
        let get_prefab = |p: &std::path::Path| cache.get(p);
        match crate::prefab::resolver_bsn::resolve_scene(&new_doc, &get_prefab) {
            Ok(resolved) => Some(resolved),
            Err(e) => {
                warn!("activate_tab: prefab resolution failed: {e}; spawning unresolved");
                None
            }
        }
    } else {
        None
    };
    // The gate reads the document about to spawn, not the one the tab captured.
    // A base carrying the retired vocabulary passes it to an instance whose own
    // document never names it, so a check before the merge would see a clean
    // document and the component would arrive afterwards.
    let spawning = resolved.as_ref().unwrap_or(&new_doc);

    // `load_bsn_scene` installs the resolved document as the live resource and
    // links the spawned entities to it. Unlike `finish_load_scene` this cannot
    // return early: the tab bookkeeping below has to run, or the editor is left
    // pointing at a tab it never finished activating.
    //
    // A tab switch applies the same refusal an open does: a scene carrying the
    // removed facade UI vocabulary is named and left unspawned rather than
    // half-loaded. The refusal is checked before the document is emitted, so a
    // tab that will not spawn does not pay for the text.
    let refusal = match jackdaw_bsn::reject_retired_ui_components(spawning) {
        Ok(()) => {
            let resolved_text = jackdaw_bsn::emit_scene(spawning);
            match jackdaw_bsn::load_bsn_scene(world, &resolved_text) {
                Ok(_) => None,
                Err(err) => {
                    error!("activate_tab: failed to spawn tab scene: {err}");
                    Some(crate::scenes::TabRefusal::SpawnFailed(err.to_string()))
                }
            }
        }
        Err(err) => {
            let name = tab_path
                .as_ref()
                .map_or_else(|| "this tab".to_string(), |p| p.display().to_string());
            error!("Cannot activate '{name}': {err}");
            Some(crate::scenes::TabRefusal::Rejected(err.to_string()))
        }
    };
    let spawned_ok = refusal.is_none();

    // Only a scene that spawned gets the viewport: bringing the canvas forward
    // over a failed activation would present an empty stage as the UI scene,
    // and a mode read from a document that is not there would be the wrong
    // mode for the tab still on screen.
    let mode = crate::viewport_host::ViewportMode::for_scene_kind(scene_kind);
    if spawned_ok && scene_kind == crate::scenes::operators::SceneKind::Ui {
        crate::viewport_host::focus_viewport(world, mode);
        // A tab framed before restores its own framing; `apply_view_state`
        // below withdraws this request when it does.
        crate::viewport_2d::request_2d_fit(world);
    } else {
        // The tab in front did not ask for the panel, so no held focus is
        // owed. Session restore opens every persisted tab in turn, so a UI
        // scene passed through on the way could otherwise leave a focus behind
        // and bring the panel forward over the tab actually restored.
        world.remove_resource::<crate::viewport_host::PendingViewportFocus>();
        if spawned_ok {
            crate::viewport_host::set_viewport_mode(world, mode, false);
        }
    }

    // Restore the per-tab content marker. For `Prefab` tabs the marker
    // is the canonical path; for `Scene` tabs the document is live in the
    // resource now, so the tab's own slot goes back to `Scene(None)`.
    //
    // Unless the spawn was refused, in which case the document is not live in
    // the resource and an empty slot would discard it. A refused tab keeps
    // holding the document it could not spawn, so a later activation has
    // something to retry with.
    {
        let mut scenes = world.resource_mut::<Scenes>();
        scenes.tabs[target].content = match content {
            TabContent::Prefab(p) => TabContent::Prefab(p),
            TabContent::Scene(_) if refusal.is_some() => TabContent::Scene(Some(Box::new(new_doc))),
            TabContent::Scene(_) => TabContent::Scene(None),
        };
        // Cleared by a successful spawn, so a refusal does not persist across
        // activations.
        scenes.tabs[target].refusal = refusal;
    }

    let history_depth = history.undo_stack.len();
    *world.resource_mut::<CommandHistory>() = history;
    apply_view_state(world, &view_state);

    // Critical: sync the global `SceneFilePath` to whichever tab is now
    // active. Without this, `save_scene` sees the previous tab's path
    // and overwrites the wrong file. Untitled tabs clear the path so
    // `save_scene` correctly delegates to `save_scene_as`.
    if let Some(mut spath) = world.get_resource_mut::<crate::scene_io::SceneFilePath>() {
        spath.path = tab_path.as_ref().map(|p| p.to_string_lossy().into_owned());
    }

    // Hydrate any terrain sidecar the store has not seen yet. A tab opened
    // by `scene_open_system` pushes a parsed document straight onto the
    // tab strip and never goes through `finish_load_scene`, so this is the
    // only place its bulk data gets read. `FillMissing` is deliberate: a
    // swap back to a tab the user has been sculpting must keep the unsaved
    // edits the store holds rather than re-reading the older file.
    if let Some(path) = tab_path.as_ref() {
        crate::scene_io::import_terrain_sidecars(
            world,
            &path.to_string_lossy(),
            crate::scene_io::SidecarImport::FillMissing,
        );
        crate::terrain::navmesh_bake::import_beside_scene(world, &path.to_string_lossy());
    }

    let mut scenes = world.resource_mut::<Scenes>();
    scenes.active = target;
    scenes.tabs[target].history_depth_at_last_check = history_depth;
}

/// Captures camera transform, edit mode, and selection as scene node ids.
fn capture_view_state(world: &mut World) -> ViewState {
    use crate::brush::{BrushSelection, EditMode};
    use crate::selection::Selected;
    use crate::viewport::MainViewportCamera;
    use crate::viewport_2d::Viewport2dPanelHost;
    use jackdaw_scene_types::SceneNodeId;

    let mut cam_q = world.query_filtered::<&Transform, With<MainViewportCamera>>();
    let camera_transform = cam_q.iter(world).next().copied().unwrap_or_default();

    // The 2D viewport's framing lives on its panel host rather than on its
    // `Viewport2dCamera`, which is derived from it, so it is captured in its
    // own pass and never reaches the query above. With several 2D panels open
    // the first one wins; per-panel view state would need a per-panel key,
    // which a tab-level `ViewState` has no room for.
    //
    // Only a framing the user chose is captured. An untouched panel holds the
    // default view it was built with, and storing that would make `ui_view`
    // `Some` from the first swap onwards. The restore honours that, so a tab
    // that was never framed, because no 2D panel was docked when it opened,
    // could never be framed later.
    let mut host_q = world.query::<&Viewport2dPanelHost>();
    let ui_view = host_q
        .iter(world)
        .next()
        .filter(|host| host.view_touched)
        .map(|host| host.view);

    let edit_mode = world
        .get_resource::<EditMode>()
        .copied()
        .unwrap_or_default();
    let brush_sub_selection = world
        .get_resource::<BrushSelection>()
        .cloned()
        .unwrap_or_default();

    let mut sel_q = world.query_filtered::<&SceneNodeId, With<Selected>>();
    let selection: Vec<SceneNodeId> = sel_q.iter(world).copied().collect();

    // Only a mode the user picked. One that followed from the scene's kind is
    // recomputed on the next activation, so storing it would freeze a tab in
    // the mode it happened to be in the first time it was left.
    let viewport_mode = world
        .get_resource::<crate::viewport_host::ViewportModeIntent>()
        .and_then(|intent| intent.chosen.then_some(intent.mode));

    ViewState {
        camera_transform,
        camera_projection: None,
        edit_mode,
        selection,
        brush_sub_selection,
        ui_view,
        viewport_mode,
    }
}

/// Restores camera transform, edit mode, and selection.
fn apply_view_state(world: &mut World, view_state: &ViewState) {
    use crate::brush::{BrushSelection, EditMode};
    use crate::selection::{Selected, Selection};
    use crate::viewport::MainViewportCamera;
    use crate::viewport_2d::Viewport2dPanelHost;
    use jackdaw_scene_types::SceneNodeId;

    // Camera transform.
    let mut cam_q = world.query_filtered::<&mut Transform, With<MainViewportCamera>>();
    if let Some(mut tf) = cam_q.iter_mut(world).next() {
        *tf = view_state.camera_transform;
    }

    // 2D viewport framing. `apply_2d_view` carries this onto the
    // `Viewport2dCamera` next frame, so nothing writes that camera's
    // transform here.
    let mut host_q = world.query::<&mut Viewport2dPanelHost>();
    if let Some(mut host) = host_q.iter_mut(world).next() {
        match view_state.ui_view {
            // A remembered framing takes precedence over the fit the
            // activation requested.
            Some(view) => {
                host.set_view(view);
                host.fit_pending = false;
            }
            // Nothing to restore, so the panel returns to unframed and any fit
            // the activation requested still stands.
            None => host.reset_view(),
        }
    }

    // A mode the user picked for this tab outranks the one its kind implies,
    // which the activation above has already set.
    if let Some(mode) = view_state.viewport_mode {
        crate::viewport_host::set_viewport_mode(world, mode, true);
    }

    // Edit mode.
    if let Some(mut em) = world.get_resource_mut::<EditMode>() {
        *em = view_state.edit_mode;
    }

    // Brush sub-selection.
    if let Some(mut bs) = world.get_resource_mut::<BrushSelection>() {
        *bs = view_state.brush_sub_selection.clone();
    }

    // Object selection: rebuild from scene node ids.
    let mut nid_q = world.query::<(Entity, &SceneNodeId)>();
    let nid_map: std::collections::HashMap<SceneNodeId, Entity> =
        nid_q.iter(world).map(|(e, nid)| (*nid, e)).collect();

    let entities: Vec<Entity> = view_state
        .selection
        .iter()
        .filter_map(|nid| nid_map.get(nid).copied())
        .collect();

    // Clear any current Selected markers (the world was just repopulated).
    let mut prev_q = world.query_filtered::<Entity, With<Selected>>();
    let prev: Vec<Entity> = prev_q.iter(world).collect();
    for e in prev {
        if let Ok(mut ec) = world.get_entity_mut(e) {
            ec.remove::<Selected>();
        }
    }
    for &e in &entities {
        if let Ok(mut ec) = world.get_entity_mut(e) {
            ec.insert(Selected);
        }
    }

    if let Some(mut selection) = world.get_resource_mut::<Selection>() {
        selection.entities = entities;
    }
}
