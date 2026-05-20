//! Multi-scene operators. Each is a Bevy system that mutates the
//! `Scenes` resource and (where appropriate) triggers a tab swap.

use bevy::input::ButtonInput;
use bevy::prelude::*;
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;

use crate::scene_io::{SceneDirtyState, SceneFilePath};
use crate::scenes::{SceneTab, Scenes, swap::swap_active_tab};

/// Counter for default `untitled-N` names. Persists across the editor
/// session so closing unsaved tabs and creating new ones doesn't reuse
/// names.
#[derive(Resource, Default)]
pub struct UntitledCounter(pub u32);

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<SceneNewOp>()
        .register_operator::<SceneOpenOp>()
        .register_operator::<SceneCloseOp>()
        .register_operator::<SceneSwitchOp>()
        .register_operator::<SceneSaveAllOp>()
        .register_operator::<SceneCycleNextOp>()
        .register_operator::<SceneCyclePrevOp>();
    ctx.register_menu_entry::<SceneNewOp>(TopLevelMenu::File)
        .register_menu_entry::<SceneOpenOp>(TopLevelMenu::File)
        .register_menu_entry::<SceneSaveAllOp>(TopLevelMenu::File)
        .register_menu_entry::<SceneCloseOp>(TopLevelMenu::File);
    let ext = ctx.id();
    ctx.entity_mut().world_scope(|w| {
        w.init_resource::<UntitledCounter>();
        w.spawn((
            Action::<SceneNewOp>::new(),
            ActionOf::<crate::core_extension::CoreExtensionInputContext>::new(ext),
            bindings![(
                KeyCode::KeyT.with_mod_keys(ModKeys::CONTROL),
                Press::default(),
            )],
        ));
        w.spawn((
            Action::<SceneCloseOp>::new(),
            ActionOf::<crate::core_extension::CoreExtensionInputContext>::new(ext),
            bindings![(
                KeyCode::KeyW.with_mod_keys(ModKeys::CONTROL),
                Press::default(),
            )],
        ));
        w.spawn((
            Action::<SceneCycleNextOp>::new(),
            ActionOf::<crate::core_extension::CoreExtensionInputContext>::new(ext),
            bindings![(
                KeyCode::Tab.with_mod_keys(ModKeys::CONTROL),
                Press::default(),
            )],
        ));
        w.spawn((
            Action::<SceneCyclePrevOp>::new(),
            ActionOf::<crate::core_extension::CoreExtensionInputContext>::new(ext),
            bindings![(
                KeyCode::Tab.with_mod_keys(ModKeys::CONTROL | ModKeys::SHIFT),
                Press::default(),
            )],
        ));
    });
}

#[operator(id = "scene.new", label = "New Scene", allows_undo = false)]
pub fn scene_new(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(scene_new_system);
    OperatorResult::Finished
}

/// Sync system body. Public so tests can run it directly.
pub fn scene_new_system(world: &mut World) {
    let n = {
        let mut c = world.resource_mut::<UntitledCounter>();
        c.0 += 1;
        c.0
    };
    let tab = SceneTab::new_untitled(n);
    let target = world.resource_mut::<Scenes>().push_tab(tab);

    // First tab: no active to swap FROM. Just set active.
    let tab_count = world.resource::<Scenes>().tabs.len();
    if tab_count == 1 {
        world.resource_mut::<Scenes>().active = 0;
        return;
    }

    swap_active_tab(world, target);
}

#[operator(id = "scene.open", label = "Open Scene...", allows_undo = false)]
pub fn scene_open(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        let Some(path) = pick_scene_file() else {
            return;
        };
        scene_open_system(world, &path);
    });
    OperatorResult::Finished
}

/// Sync system body. Public so tests and the asset browser can call it
/// without going through the file-dialog path.
pub fn scene_open_system(world: &mut World, path: &std::path::Path) {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    // De-dupe: if a tab with this path is already open, switch to it.
    let existing = world.resource::<Scenes>().tabs.iter().position(|t| {
        t.path
            .as_ref()
            .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()) == canonical)
            .unwrap_or(false)
    });
    if let Some(idx) = existing {
        swap_active_tab(world, idx);
        return;
    }

    // Read the file.
    let jsn_text = match std::fs::read_to_string(&canonical) {
        Ok(t) => t,
        Err(err) => {
            warn!("scene.open: failed to read {canonical:?}: {err}");
            return;
        }
    };
    let jsn: jackdaw_jsn::format::JsnScene = match serde_json::from_str(&jsn_text) {
        Ok(j) => j,
        Err(err) => {
            warn!("scene.open: failed to parse {canonical:?}: {err}");
            return;
        }
    };

    // Build the new tab.
    let display_name = canonical
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("scene")
        .to_string();
    let kind = if jsn
        .scene
        .first()
        .map(|e| {
            e.components
                .contains_key("jackdaw::prefab::components::Prefab")
        })
        .unwrap_or(false)
    {
        crate::scenes::TabKind::Prefab
    } else {
        crate::scenes::TabKind::Scene
    };
    let mut tab = SceneTab::new_untitled(0);
    tab.kind = kind.clone();
    tab.path = Some(canonical.clone());
    tab.display_name = display_name;
    tab.dirty = false;
    // Restore the saved viewport camera framing if the scene file
    // carried one; otherwise leave the default (0, 4, 8) from
    // `new_untitled`.
    if let Some(camera) = jsn.editor.as_ref().and_then(|e| e.camera.as_ref()) {
        tab.view_state.camera_transform = camera.clone().into();
    }
    let ast = jackdaw_jsn::SceneJsnAst::from_jsn_scene(&jsn, &[]);
    tab.content = match kind {
        crate::scenes::TabKind::Prefab => {
            let canonical_path = crate::prefab::canonical_prefab_path(&canonical);
            if let Some(mut cache) = world.get_resource_mut::<crate::prefab::PrefabAstCache>()
                && cache.get_canonical(&canonical_path).is_none()
            {
                cache.insert(canonical_path.as_path(), ast);
            }
            crate::scenes::TabContent::Prefab(canonical_path)
        }
        crate::scenes::TabKind::Scene => crate::scenes::TabContent::Scene(Some(ast)),
    };

    let target = world.resource_mut::<Scenes>().push_tab(tab);

    // If this is the first tab we've pushed, there is nothing to swap
    // away from, so `swap_active_tab` would bail at `current == target`
    // and the snapshot would never get loaded into the world. Skip
    // straight to activation in that case.
    let tab_count = world.resource::<Scenes>().tabs.len();
    if tab_count == 1 {
        world.resource_mut::<Scenes>().active = target;
        crate::scenes::swap::activate_tab(world, target);
    } else {
        swap_active_tab(world, target);
    }
}

fn pick_scene_file() -> Option<std::path::PathBuf> {
    rfd::FileDialog::new()
        .add_filter("Jackdaw scene", &["jsn"])
        .pick_file()
}

#[operator(id = "scene.close", label = "Close Tab", allows_undo = false)]
pub fn scene_close(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        let active = world.resource::<Scenes>().active;
        scene_close_system(world, active);
    });
    OperatorResult::Finished
}

/// Sync system body. Closes the tab at `target`. Blocks closing the
/// last open tab. If the tab is dirty, defers to the confirm dialog.
/// If the target IS the active tab, the live entities are despawned
/// and a neighbor tab is activated. If the target is inactive, just
/// remove it from the list.
pub fn scene_close_system(world: &mut World, target: usize) {
    let tab_count = world.resource::<Scenes>().tabs.len();
    if tab_count <= 1 {
        info!("scene.close: cannot close the last open tab");
        return;
    }
    if target >= tab_count {
        warn!("scene.close: target {target} out of range");
        return;
    }

    let dirty = world.resource::<Scenes>().tabs[target].dirty;
    if dirty {
        // If a dialog is already up, ignore the new request.
        if world
            .resource::<crate::scenes::confirm_dialog::PendingTabClose>()
            .tab_index
            .is_some()
        {
            return;
        }
        world
            .resource_mut::<crate::scenes::confirm_dialog::PendingTabClose>()
            .tab_index = Some(target);
        let display_name = world.resource::<Scenes>().tabs[target].display_name.clone();
        crate::scenes::confirm_dialog::spawn_confirm_dialog(world, &display_name);
        return;
    }

    scene_close_system_unprompted(world, target);
}

/// The actual close logic, called either directly (clean tab) or from
/// the dialog's Save/Discard branches (after the user has confirmed).
/// Does not check the dirty flag.
pub fn scene_close_system_unprompted(world: &mut World, target: usize) {
    let tab_count = world.resource::<Scenes>().tabs.len();
    if tab_count <= 1 {
        info!("scene.close: cannot close the last open tab");
        return;
    }
    if target >= tab_count {
        warn!("scene.close: target {target} out of range");
        return;
    }

    let active = world.resource::<Scenes>().active;
    if target == active {
        // Despawn the live world entities (we are NOT capturing them
        // back into the closed tab).
        crate::scene_io::clear_scene_entities(world);
        // Pick a neighbor BEFORE removing the closed tab.
        let neighbor = if active + 1 < tab_count {
            active + 1
        } else {
            active - 1
        };
        // Remove the closed tab.
        world.resource_mut::<Scenes>().tabs.remove(target);
        // Indices shift if the removed tab came BEFORE the neighbor.
        let new_target = if neighbor > target {
            neighbor - 1
        } else {
            neighbor
        };
        world.resource_mut::<Scenes>().active = new_target;
        crate::scenes::swap::reactivate_after_close(world, new_target);
    } else {
        world.resource_mut::<Scenes>().tabs.remove(target);
        let mut scenes = world.resource_mut::<Scenes>();
        if scenes.active > target {
            scenes.active -= 1;
        }
    }
}

#[operator(id = "scene.switch", label = "Switch Scene", allows_undo = false)]
pub fn scene_switch(In(params): In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let Some(target) = params.as_int("tab") else {
        warn!("scene.switch: missing 'tab' parameter");
        return OperatorResult::Cancelled;
    };
    let target = target.max(0) as usize;
    commands.queue(move |world: &mut World| scene_switch_system(world, target));
    OperatorResult::Finished
}

pub fn scene_switch_system(world: &mut World, target: usize) {
    crate::scenes::swap::swap_active_tab(world, target);
}

#[operator(id = "scene.save_all", label = "Save All", allows_undo = false)]
pub fn scene_save_all(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(scene_save_all_system);
    OperatorResult::Finished
}

/// Iterate tabs, switching to each in turn, serializing tabs with a path
/// to disk synchronously, then return to the originally-active tab.
///
/// Tabs without a path (untitled) are skipped.
pub fn scene_save_all_system(world: &mut World) {
    let original_active = world.resource::<Scenes>().active;
    let count = world.resource::<Scenes>().tabs.len();

    // Snapshot original SceneFilePath.path so we can restore it.
    let original_path = world
        .get_resource::<SceneFilePath>()
        .and_then(|r| r.path.clone());

    for i in 0..count {
        let Some(path) = world.resource::<Scenes>().tabs[i].path.clone() else {
            continue;
        };

        if i != world.resource::<Scenes>().active {
            swap_active_tab(world, i);
        }

        // Point SceneFilePath at this tab's path so serialize_world_to_jsn_scene
        // resolves relative asset references correctly.
        let path_str = path.to_string_lossy().into_owned();
        if let Some(mut sfp) = world.get_resource_mut::<SceneFilePath>() {
            sfp.path = Some(path_str.clone());
        }

        let jsn = crate::scene_io::serialize_world_to_jsn_scene(world);
        match serde_json::to_string_pretty(&jsn) {
            Ok(json) => {
                match std::fs::write(&path, &json) {
                    Ok(()) => {
                        let depth = world
                            .resource::<crate::commands::CommandHistory>()
                            .undo_stack
                            .len();
                        {
                            let mut scenes = world.resource_mut::<Scenes>();
                            scenes.tabs[i].dirty = false;
                            scenes.tabs[i].history_depth_at_last_check = depth;
                        }
                        // Sync the dirty state counter.
                        if let Some(history_len) = world
                            .get_resource::<jackdaw_commands::CommandHistory>()
                            .map(|h| h.undo_stack.len())
                            && let Some(mut ds) = world.get_resource_mut::<SceneDirtyState>()
                        {
                            ds.undo_len_at_save = history_len;
                        }
                    }
                    Err(err) => warn!("scene.save_all: failed to write {path:?}: {err}"),
                }
            }
            Err(err) => warn!("scene.save_all: failed to serialize tab {i}: {err}"),
        }
    }

    // Restore to the originally active tab.
    if original_active != world.resource::<Scenes>().active {
        swap_active_tab(world, original_active);
    }

    // Restore the original SceneFilePath.path.
    if let Some(mut sfp) = world.get_resource_mut::<SceneFilePath>() {
        sfp.path = original_path;
    }
}

#[operator(id = "scene.cycle_next", label = "Next Scene Tab", allows_undo = false)]
pub fn scene_cycle_next(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        // Ctrl+Tab also fires the Ctrl-only binding because the modifier
        // matcher is "must include these mods, others ignored". Bail when
        // Shift is held so cycle_prev runs alone.
        let shift_held = world
            .get_resource::<ButtonInput<KeyCode>>()
            .is_some_and(|kb| kb.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]));
        if shift_held {
            return;
        }
        let scenes = world.resource::<Scenes>();
        let count = scenes.tabs.len();
        if count <= 1 {
            return;
        }
        let target = (scenes.active + 1) % count;
        scene_switch_system(world, target);
    });
    OperatorResult::Finished
}

#[operator(
    id = "scene.cycle_prev",
    label = "Previous Scene Tab",
    allows_undo = false
)]
pub fn scene_cycle_prev(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        let scenes = world.resource::<Scenes>();
        let count = scenes.tabs.len();
        if count <= 1 {
            return;
        }
        let target = (scenes.active + count - 1) % count;
        scene_switch_system(world, target);
    });
    OperatorResult::Finished
}
