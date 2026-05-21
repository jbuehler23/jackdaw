//! Tab-switch mechanics. The pure pipeline lives here so it can be
//! tested independently of UI and operator wiring.

use std::collections::HashMap;

use bevy::prelude::*;
use jackdaw_api::prelude::*;

use crate::commands::CommandHistory;
use crate::scene_io::{clear_scene_entities, jsn_scene_from_ast, load_scene_from_jsn};
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
}

/// Spawn the target tab's snapshot into a world that has just been
/// cleared. Used by `scene_close_system` when the closed tab was the
/// active tab (so the normal `capture_active_tab` step would try to
/// re-capture a tab that's being dropped).
pub fn reactivate_after_close(world: &mut World, target: usize) {
    activate_tab(world, target);
}

/// Capture the live AST into the active tab and stash the per-tab
/// history and view state. Pre-condition: a tab exists at
/// `Scenes.active`.
fn capture_active_tab(world: &mut World) {
    let active = world.resource::<Scenes>().active;
    let ast_snapshot = std::mem::take(&mut *world.resource_mut::<jackdaw_jsn::SceneJsnAst>());
    let view_state = capture_view_state(world);
    let history = std::mem::take(&mut *world.resource_mut::<CommandHistory>());

    let prefab_target = {
        let scenes = world.resource::<Scenes>();
        match &scenes.tabs[active].content {
            TabContent::Prefab(path) => Some(path.clone()),
            TabContent::Scene(_) => None,
        }
    };

    if let Some(path) = prefab_target {
        // Prefab tab: flush the live AST into the cache entry rather
        // than onto the tab. The `TabContent::Prefab` key keeps
        // pointing at the same cache entry from here on.
        if let Some(mut cache) = world.get_resource_mut::<crate::prefab::PrefabAstCache>() {
            // `insert` overwrites or creates, bumps the epoch, and
            // marks the path dirty. That matches the semantics we
            // want for both the "first capture" and "subsequent
            // re-capture" cases without branching on existence.
            cache.insert(path.as_path(), ast_snapshot);
        }
    } else {
        // Scene tab: store the captured AST directly on the tab.
        let mut scenes = world.resource_mut::<Scenes>();
        scenes.tabs[active].content = TabContent::Scene(Some(ast_snapshot));
    }

    let mut scenes = world.resource_mut::<Scenes>();
    let tab = &mut scenes.tabs[active];
    tab.view_state = view_state;
    tab.history = history;
}

/// Spawn the target tab's AST into the live world and restore per-tab
/// history and view state.
pub fn activate_tab(world: &mut World, target: usize) {
    let (content, view_state, history, tab_path) = {
        let mut scenes = world.resource_mut::<Scenes>();
        let tab = &mut scenes.tabs[target];
        (
            std::mem::take(&mut tab.content),
            std::mem::take(&mut tab.view_state),
            std::mem::take(&mut tab.history),
            tab.path.clone(),
        )
    };

    // Materialize the AST to install. For `Prefab` tabs, read from
    // cache; for `Scene` tabs, take the captured AST (or default).
    let new_ast = match &content {
        TabContent::Scene(Some(ast)) => ast.clone(),
        TabContent::Scene(None) => jackdaw_jsn::SceneJsnAst::default(),
        TabContent::Prefab(path) => world
            .get_resource::<crate::prefab::PrefabAstCache>()
            .and_then(|c| c.get_canonical(path).cloned())
            .unwrap_or_default(),
    };

    // Mirror `finish_load_scene`: any IsA references in the captured AST
    // need their prefab files loaded into the cache, then resolved into a
    // transient JsnScene with sparse-override merging applied, before we
    // spawn. Spawning from the unresolved AST directly would feed Bevy's
    // reflect deserializer a partial Transform like
    // {"translation": [..]} (no rotation / scale), which fails and leaves
    // the entity in a broken state -- previously surfaced as orphan
    // render meshes after a tab swap.
    let parent = tab_path
        .as_ref()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    // PrefabAstCache may be absent in minimal test harnesses; if so,
    // skip the resolver step entirely (there can't be any cached
    // prefabs to merge against). In the editor the cache is always
    // present because PrefabPlugin initializes it.
    let resolved_ast = if world
        .get_resource::<crate::prefab::PrefabAstCache>()
        .is_some()
    {
        {
            let mut cache = world.resource_mut::<crate::prefab::PrefabAstCache>();
            crate::prefab::save_load::populate_cache_for_scene(&new_ast, &mut cache, &parent);
        }
        let cache = world.resource::<crate::prefab::PrefabAstCache>();
        match crate::prefab::resolver::resolve_scene(&new_ast, cache) {
            Ok(r) => r,
            Err(e) => {
                warn!("activate_tab: prefab resolution failed: {e}; spawning unresolved");
                new_ast.clone()
            }
        }
    } else {
        new_ast.clone()
    };
    let jsn_for_spawn = jsn_scene_from_ast(&resolved_ast);

    *world.resource_mut::<jackdaw_jsn::SceneJsnAst>() = new_ast;

    let local_assets = HashMap::new();
    let spawned = load_scene_from_jsn(world, &jsn_for_spawn.scene, &parent, &local_assets);

    // Re-bind the unresolved AST's per-node ecs_entity to the freshly
    // spawned entities and rebuild ecs_to_jsn. The first N spawned entries
    // (where N == unresolved AST node count) correspond to authored
    // entities one-to-one; later entries are inherited-from-prefab
    // descendants that live ECS-only until the user edits them.
    {
        let mut ast = world.resource_mut::<jackdaw_jsn::SceneJsnAst>();
        let authored_count = ast.nodes.len();
        for (i, node) in ast.nodes.iter_mut().enumerate().take(authored_count) {
            node.ecs_entity = spawned.get(i).copied();
        }
        let remap: HashMap<bevy::prelude::Entity, usize> = ast
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| n.ecs_entity.map(|e| (e, i)))
            .collect();
        ast.ecs_to_jsn = remap;
    }

    // Restore the per-tab content marker. For `Prefab` tabs the marker
    // is the canonical path; for `Scene` tabs the AST is live in the
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

    let mut scenes = world.resource_mut::<Scenes>();
    scenes.active = target;
    scenes.tabs[target].history_depth_at_last_check = history_depth;
}

/// Captures camera transform, edit mode, and selection as stable IDs.
fn capture_view_state(world: &mut World) -> ViewState {
    use crate::brush::{BrushSelection, EditMode};
    use crate::draw_brush::BrushStableId;
    use crate::selection::Selected;
    use crate::viewport::MainViewportCamera;

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

    let mut sel_q = world.query_filtered::<&BrushStableId, With<Selected>>();
    let selection: Vec<BrushStableId> = sel_q.iter(world).copied().collect();

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
    use crate::draw_brush::BrushStableId;
    use crate::selection::{Selected, Selection};
    use crate::viewport::MainViewportCamera;

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

    // Object selection: rebuild from stable IDs.
    let mut sid_q = world.query::<(Entity, &BrushStableId)>();
    let sid_map: std::collections::HashMap<BrushStableId, Entity> =
        sid_q.iter(world).map(|(e, sid)| (*sid, e)).collect();

    let entities: Vec<Entity> = view_state
        .selection
        .iter()
        .filter_map(|sid| sid_map.get(sid).copied())
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
