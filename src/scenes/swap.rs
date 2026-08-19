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
    let text = crate::scene_io::emit_bsn_scene_with_inline_assets(world, &parent);

    if let Some(path) = prefab_target {
        // Prefab tab: flush the snapshot into the cache entry rather than
        // onto the tab. The `TabContent::Prefab` key keeps pointing at the
        // same cache entry from here on. `insert` overwrites or creates,
        // bumps the epoch, and marks the path dirty, matching both the
        // "first capture" and "re-capture" cases without branching on
        // existence.
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

    let terrain_data_store = world
        .get_resource_mut::<crate::terrain::TerrainDataStore>()
        .map(|mut store| std::mem::take(&mut *store));
    let mut scenes = world.resource_mut::<Scenes>();
    let tab = &mut scenes.tabs[active];
    tab.view_state = view_state;
    tab.history = history;
    if let Some(terrain_data_store) = terrain_data_store {
        tab.terrain_data_store = terrain_data_store;
    }
}

/// Spawn the target tab's document into the live world and restore per-tab
/// history and view state.
pub fn activate_tab(world: &mut World, target: usize) {
    let has_terrain_data_store = world.contains_resource::<crate::terrain::TerrainDataStore>();
    let (mut content, view_state, history, tab_path, terrain_data_store) = {
        let mut scenes = world.resource_mut::<Scenes>();
        let tab = &mut scenes.tabs[target];
        (
            std::mem::take(&mut tab.content),
            std::mem::take(&mut tab.view_state),
            std::mem::take(&mut tab.history),
            tab.path.clone(),
            has_terrain_data_store.then(|| std::mem::take(&mut tab.terrain_data_store)),
        )
    };

    // Restore the target tab's bulk terrain data before spawning its scene
    // and importing any sidecars. `FillMissing` can now preserve unsaved
    // edits without confusing a same-named sidecar from another tab.
    if let Some(terrain_data_store) = terrain_data_store {
        *world.resource_mut::<crate::terrain::TerrainDataStore>() = terrain_data_store;
    }

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

    // Mirror `finish_load_scene`: any IsA references in the captured
    // document need their prefab files loaded into the cache, then resolved
    // (materializing inherited subtrees), before the spawn. PrefabAstCache
    // may be absent in minimal test harnesses; if so, skip the resolver
    // step entirely (there can't be any cached prefabs to merge against).
    let parent = tab_path
        .as_ref()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let resolved_text = if world
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
            Ok(resolved) => jackdaw_bsn::emit_scene(&resolved),
            Err(e) => {
                warn!("activate_tab: prefab resolution failed: {e}; spawning unresolved");
                jackdaw_bsn::emit_scene(&new_doc)
            }
        }
    } else {
        jackdaw_bsn::emit_scene(&new_doc)
    };

    // `load_bsn_scene` installs the resolved document as the live resource
    // and links the spawned entities to it.
    if let Err(err) = jackdaw_bsn::load_bsn_scene(world, &resolved_text) {
        error!("activate_tab: failed to spawn tab scene: {err}");
    }

    // Restore the per-tab content marker. For `Prefab` tabs the marker
    // is the canonical path; for `Scene` tabs the document is live in the
    // resource now, so the tab's own slot goes back to `Scene(None)`.
    {
        let mut scenes = world.resource_mut::<Scenes>();
        scenes.tabs[target].content = match content {
            TabContent::Prefab(p) => TabContent::Prefab(p),
            TabContent::Scene(_) => TabContent::Scene(None),
        };
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
    use jackdaw_scene_types::SceneNodeId;

    let mut cam_q = world.query_filtered::<&Transform, With<MainViewportCamera>>();
    let camera_transform = cam_q.iter(world).next().copied().unwrap_or_default();

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

    ViewState {
        camera_transform,
        camera_projection: None,
        edit_mode,
        selection,
        brush_sub_selection,
    }
}

/// Restores camera transform, edit mode, and selection.
fn apply_view_state(world: &mut World, view_state: &ViewState) {
    use crate::brush::{BrushSelection, EditMode};
    use crate::selection::{Selected, Selection};
    use crate::viewport::MainViewportCamera;
    use jackdaw_scene_types::SceneNodeId;

    // Camera transform.
    let mut cam_q = world.query_filtered::<&mut Transform, With<MainViewportCamera>>();
    if let Some(mut tf) = cam_q.iter_mut(world).next() {
        *tf = view_state.camera_transform;
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
