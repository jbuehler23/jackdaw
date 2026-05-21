//! Tab-switch mechanics. The pure pipeline lives here so it can be
//! tested independently of UI and operator wiring.

use std::collections::HashMap;
use std::path::Path;

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_log::prelude::*;
use jackdaw_api::prelude::*;

use crate::commands::CommandHistory;
use crate::scene_io::{clear_scene_entities, load_scene_from_jsn, serialize_world_to_jsn_scene};
use crate::scenes::{Scenes, ViewState};

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

/// Serialize the live world into the active tab's snapshot and stash
/// the per-tab history and view state. Pre-condition: a tab exists at
/// `Scenes.active`.
fn capture_active_tab(world: &mut World) {
    let active = world.resource::<Scenes>().active;
    let snapshot = serialize_world_to_jsn_scene(world);
    let view_state = capture_view_state(world);
    let history = std::mem::take(&mut *world.resource_mut::<CommandHistory>());

    let mut scenes = world.resource_mut::<Scenes>();
    let tab = &mut scenes.tabs[active];
    tab.snapshot = Some(snapshot);
    tab.view_state = view_state;
    tab.history = history;
}

/// Spawn the target tab's snapshot into the live world and restore
/// per-tab history and view state.
pub(crate) fn activate_tab(world: &mut World, target: usize) {
    let (snapshot, view_state, history, tab_path) = {
        let mut scenes = world.resource_mut::<Scenes>();
        let tab = &mut scenes.tabs[target];
        (
            tab.snapshot.take(),
            std::mem::take(&mut tab.view_state),
            std::mem::take(&mut tab.history),
            tab.path.clone(),
        )
    };

    if let Some(jsn) = snapshot {
        let parent = Path::new(".");
        let local_assets = HashMap::new();
        let _ = load_scene_from_jsn(world, &jsn.scene, parent, &local_assets);
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
