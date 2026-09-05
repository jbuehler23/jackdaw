//! Multi-scene operators. Each is a Bevy system that mutates the
//! `Scenes` resource and (where appropriate) triggers a tab swap.

use bevy::input::ButtonInput;
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;

use crate::scene_io::SceneFilePath;
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
    ctx.entity_mut().world_scope(|w| {
        w.init_resource::<UntitledCounter>();
    });

    ctx.bind_operator::<crate::core_extension::CoreExtensionInputContext, SceneNewOp>([
        PresetInput::key("KeyT").ctrl(),
    ]);
    ctx.bind_operator::<crate::core_extension::CoreExtensionInputContext, SceneOpenOp>([
        PresetInput::key("KeyO").ctrl_or_super(),
    ]);
    ctx.bind_operator::<crate::core_extension::CoreExtensionInputContext, SceneCloseOp>([
        PresetInput::key("KeyW").ctrl(),
    ]);
    ctx.bind_operator::<crate::core_extension::CoreExtensionInputContext, SceneCycleNextOp>([
        PresetInput::key("Tab").ctrl(),
    ]);
    ctx.bind_operator::<crate::core_extension::CoreExtensionInputContext, SceneCyclePrevOp>([
        PresetInput::key("Tab").ctrl().shift(),
    ]);
}

/// Which kind of scene a `scene.new` makes.
///
/// The kind is the scene's own concept rather than a flag on the operator:
/// it decides what the document is seeded with, which panel comes forward,
/// and which marker component a save writes so a reopened document is
/// recognised as the same kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SceneKind {
    /// A 3D world scene. The editor's default.
    #[default]
    ThreeD,
    /// A 2D world scene: sprites in the world viewport, no 3D furniture.
    TwoD,
    /// A UI screen, authored on the 2D canvas.
    Ui,
}

impl SceneKind {
    /// Read the operator's `kind` clause: `3d`, `2d` or `ui`. Unknown
    /// values fall back to 3D, the kind a bare `scene.new` has always made.
    pub fn from_clause(value: &str) -> Self {
        match value {
            "2d" => Self::TwoD,
            "ui" => Self::Ui,
            _ => Self::ThreeD,
        }
    }
}

#[operator(
    id = "scene.new",
    label = "New Scene",
    allows_undo = false,
    params(
        kind(
            String,
            default = "3d",
            doc = "Which kind of scene to make: 3d, 2d or ui."
        ),
        ui(
            bool,
            default = false,
            doc = "Deprecated alias for kind=ui. Start the scene with a UI root."
        ),
        path(String, doc = "File the new scene saves to. Untitled when omitted."),
    )
)]
pub fn scene_new(In(params): In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    // `ui=true` is the spelling scripted runs and older keymaps use. It is
    // kept as an alias for one release, and only speaks when `kind` is
    // absent, so an explicit kind always wins.
    let kind = match params.as_str("kind") {
        Some(kind) => SceneKind::from_clause(kind),
        None if params.as_bool("ui").unwrap_or(false) => SceneKind::Ui,
        None => SceneKind::ThreeD,
    };
    let path = params.as_str("path").map(std::path::PathBuf::from);
    commands.queue(move |world: &mut World| {
        scene_new_configured(world, kind, path.as_deref());
    });
    OperatorResult::Finished
}

/// Sync system body. Public so tests can run it directly.
pub fn scene_new_system(world: &mut World) {
    scene_new_configured(world, SceneKind::ThreeD, None);
}

/// New tab of `kind`, optionally pointed at a file.
///
/// Seeding runs after the tab is active: activating replaces the live entities,
/// so a root spawned first would be despawned with the previous scene.
///
/// The three kinds seed different things and share nothing. A 3D scene gets a
/// directional light, without which it opens black; a UI scene gets a
/// [`crate::ui_palette::seed_ui_scene_root`] and nothing else, because a light
/// in a UI document is furniture the author never asked for that a save then
/// writes to disk; a 2D scene gets its root marker alone, since its contents
/// are sprites the author places. So the seeding is a branch, not a common
/// step with addenda.
pub fn scene_new_configured(world: &mut World, kind: SceneKind, path: Option<&std::path::Path>) {
    let n = {
        let mut c = world.resource_mut::<UntitledCounter>();
        c.0 += 1;
        c.0
    };
    let mut tab = SceneTab::new_untitled(n);
    if let Some(path) = path {
        tab.display_name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("scene")
            .to_string();
        tab.path = Some(path.to_path_buf());
    }
    let target = world.resource_mut::<Scenes>().push_tab(tab);
    activate_pushed_tab(world, target);
    match kind {
        SceneKind::ThreeD => crate::entity_ops::seed_new_scene_defaults(world),
        SceneKind::TwoD | SceneKind::Ui => crate::entity_ops::ensure_scene_document(world),
    }

    // Point the save path at the new tab, or clear it when the tab is
    // untitled. Clearing matters: a leftover path from the previous tab would
    // send the next `scene.save` at that file, overwriting it with the new
    // empty scene.
    if let Some(mut file_path) = world.get_resource_mut::<SceneFilePath>() {
        file_path.path = path.map(|path| path.to_string_lossy().into_owned());
    }

    match kind {
        SceneKind::TwoD => {
            crate::entity_ops::seed_2d_scene_root(world);
        }
        SceneKind::Ui => {
            crate::ui_palette::seed_ui_scene_root(world);
        }
        SceneKind::ThreeD => {}
    }

    // The kind picks the mode; the user can switch afterwards. A flat scene
    // is also the moment its canvas is wanted, and the dock leaves whatever
    // was in front there otherwise, so those two kinds front the panel as
    // well. The load and tab-swap paths front it the same way.
    let mode = crate::viewport_host::ViewportMode::for_scene_kind(kind);
    match kind {
        SceneKind::TwoD | SceneKind::Ui => crate::viewport_host::focus_viewport(world, mode),
        SceneKind::ThreeD => crate::viewport_host::set_viewport_mode(world, mode, false),
    }
}

/// Activate a tab that was just appended. The first tab cannot go through
/// `swap_active_tab`: `Scenes.active` defaults to 0, so swap would see
/// `current == target` and no-op without loading the document.
fn activate_pushed_tab(world: &mut World, target: usize) {
    let tab_count = world.resource::<Scenes>().tabs.len();
    if tab_count == 1 {
        world.resource_mut::<Scenes>().active = target;
        crate::scenes::swap::activate_tab(world, target);
    } else {
        swap_active_tab(world, target);
    }
}

#[operator(
    id = "scene.open",
    label = "Open Scene...",
    allows_undo = false,
    params(path(
        String,
        doc = "Scene file to open, absolute or relative to the project's assets \
               directory. Asks for one when omitted."
    ))
)]
pub fn scene_open(In(params): In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let path = params.as_str("path").map(std::path::PathBuf::from);
    commands.queue(move |world: &mut World| {
        let path = match path.map(|path| resolve_scene_path(world, path)) {
            Some(Ok(path)) => Some(path),
            Some(Err(refusal)) => {
                warn!("scene.open: {refusal}");
                return;
            }
            None => None,
        };
        let Some(path) = path.or_else(pick_scene_file) else {
            return;
        };
        // Legacy .jsn picks confirm conversion before opening.
        crate::migrate_dialog::request_open_with_conversion(world, &path);
    });
    OperatorResult::Finished
}

/// Where a `scene.open path=` lands, or `None` when it names a file the
/// caller has no business opening.
///
/// A caller that has not clicked through a file dialog spells the scene
/// the way the project does -- `level.bsn`, or `scenes/level.bsn` -- and
/// has no business knowing the editor's working directory. A relative
/// path is tried under the project's `assets/`, then under the project
/// root, and only then against the working directory, which is what a
/// shell-typed path means.
///
/// With a project open, the file has to be inside it. `path=` is reachable
/// from the remote surface, where an unconfined path would let a caller
/// read any file on the machine into the editor and then save it back
/// somewhere else. The user's own File > Open dialog is not this path: it
/// hands its pick to [`crate::migrate_dialog::request_open_with_conversion`]
/// directly, so opening a scene from outside the project by hand still
/// works.
fn resolve_scene_path(
    world: &World,
    path: std::path::PathBuf,
) -> Result<std::path::PathBuf, String> {
    let Some(project) = world.get_resource::<crate::project::ProjectRoot>() else {
        return Ok(path);
    };
    let root = dunce::canonicalize(&project.root).unwrap_or_else(|_| project.root.clone());
    let candidate = if path.is_absolute() {
        path
    } else {
        [project.assets_dir(), project.root.clone()]
            .into_iter()
            .map(|base| base.join(&path))
            .find(|candidate| candidate.is_file())
            .unwrap_or_else(|| project.root.join(&path))
    };
    let resolved = dunce::canonicalize(&candidate).unwrap_or(candidate);
    if resolved.starts_with(&root) {
        Ok(resolved)
    } else {
        Err(format!(
            "{} is outside the open project at {}",
            resolved.display(),
            root.display()
        ))
    }
}

/// Does this document describe a prefab rather than a scene?
///
/// Shared with the reload path, which asks the same question of a file edited
/// into a prefab underneath an open scene tab.
pub fn document_is_prefab(doc: &jackdaw_bsn::SceneBsnAst) -> bool {
    doc.roots.first().is_some_and(|&root| {
        doc.component_type_paths(root)
            .iter()
            .any(|tp| tp == "jackdaw::prefab::components::Prefab")
    })
}

/// Sync system body. Public so tests and the asset browser can call it
/// without going through the file-dialog path.
pub fn scene_open_system(world: &mut World, path: &std::path::Path) {
    let canonical = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    // De-dupe: if a tab with this path is already open, switch to it. A
    // legacy `.jsn` pick also matches its converted `.bsn` sibling, since
    // opening it would convert to (or already produced) that file.
    let bsn_sibling = canonical
        .extension()
        .is_some_and(|e| e == "jsn")
        .then(|| canonical.with_extension("bsn"));
    let existing = world.resource::<Scenes>().tabs.iter().position(|t| {
        t.path
            .as_ref()
            .map(|p| {
                let tab_path = dunce::canonicalize(p).unwrap_or_else(|_| p.clone());
                tab_path == canonical || Some(&tab_path) == bsn_sibling.as_ref()
            })
            .unwrap_or(false)
    });
    if let Some(idx) = existing {
        // Opening a scene that is already open is a request to see it, and
        // `swap_active_tab` refreshes any sidecar the file has moved on
        // from, so a terrain rewritten on disk is the one that comes up.
        swap_active_tab(world, idx);
        return;
    }

    // Read the file.
    let file_text = match std::fs::read_to_string(&canonical) {
        Ok(t) => t,
        Err(err) => {
            warn!("scene.open: failed to read {canonical:?}: {err}");
            return;
        }
    };

    // Build the tab's scene document. `.bsn` parses directly. Legacy `.jsn`
    // is not imported: it converts to a `.bsn` document that the tab opens,
    // and the conversion is held in memory until that document is accepted
    // below. The interactive open path confirms before reaching here.
    let mut saved_camera: Option<Transform> = None;
    // The path the user picked, for messages about a document not yet written:
    // `canonical` becomes the `.bsn` the conversion targets, which does not
    // exist until the commit below.
    let opened = canonical.clone();
    let (canonical, file_text, pending_conversion) =
        if canonical.extension().is_some_and(|e| e == "bsn") {
            (canonical, file_text, None)
        } else {
            // Read the camera framing sidecar before the source is renamed.
            saved_camera = serde_json::from_str::<jackdaw_jsn::format::JsnScene>(&file_text)
                .ok()
                .and_then(|jsn| jsn.editor.as_ref().and_then(|e| e.camera.clone()))
                .map(std::convert::Into::into);
            let pending = match crate::jsn_to_bsn::convert_scene_file_pending(world, &canonical) {
                Ok(pending) => pending,
                Err(err) => {
                    warn!("scene.open: legacy conversion of {canonical:?} failed: {err}");
                    return;
                }
            };
            (
                pending.bsn_path.clone(),
                pending.scene_bsn.clone(),
                Some(pending),
            )
        };
    let dirty = false;
    let doc = match jackdaw_bsn::parse_bsn_text(&file_text) {
        Ok(doc) => doc,
        Err(err) => {
            warn!("scene.open: failed to parse {opened:?}: {err}");
            return;
        }
    };

    // The same refusal every other load path applies: a document naming the
    // removed facade UI vocabulary would open as a scene silently missing its
    // UI, so no tab is created.
    if let Err(err) = jackdaw_bsn::reject_retired_ui_components(&doc) {
        warn!("scene.open: cannot open {opened:?}: {err}");
        return;
    }

    // The document parsed and passed the gate, so the conversion can be
    // written to disk.
    if let Some(pending) = pending_conversion {
        let bsn_path = pending.bsn_path.clone();
        if let Err(err) = crate::jsn_to_bsn::commit_conversion(world, pending) {
            warn!(
                "scene.open: failed to write converted {}: {err}",
                bsn_path.display()
            );
            return;
        }
        info!(
            "Converted legacy scene to {}; original kept as .jsn.bak",
            bsn_path.display()
        );
    }
    // Record the bytes read as what is at this path, before the tab exists: the
    // open-tab watcher starts only once the tab does, and an edit landing in
    // that gap still has to be reported.
    crate::scenes::external_watch::note_known_content(world, &canonical, file_text.as_bytes());

    let is_prefab = document_is_prefab(&doc);

    // Build the new tab.
    let display_name = canonical
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("scene")
        .to_string();
    let kind = if is_prefab {
        crate::scenes::TabKind::Prefab
    } else {
        crate::scenes::TabKind::Scene
    };
    let mut tab = SceneTab::new_untitled(0);
    tab.kind = kind.clone();
    tab.path = Some(canonical.clone());
    tab.display_name = display_name;
    tab.dirty = dirty;
    // Restore the saved viewport camera framing if the scene file
    // carried one; otherwise leave the default (0, 4, 8) from
    // `new_untitled`.
    if let Some(camera) = saved_camera {
        tab.view_state.camera_transform = camera;
    }
    tab.content = match kind {
        crate::scenes::TabKind::Prefab => {
            let canonical_path = crate::prefab::canonical_prefab_path(&canonical);
            let needs_cache = world
                .get_resource::<crate::prefab::PrefabAstCache>()
                .is_some_and(|cache| cache.get_canonical(&canonical_path).is_none());
            if needs_cache {
                world
                    .resource_mut::<crate::prefab::PrefabAstCache>()
                    .insert(canonical_path.as_path(), doc);
            }
            crate::scenes::TabContent::Prefab(canonical_path)
        }
        crate::scenes::TabKind::Scene => crate::scenes::TabContent::Scene(Some(Box::new(doc))),
    };

    let target = world.resource_mut::<Scenes>().push_tab(tab);
    activate_pushed_tab(world, target);
}

fn pick_scene_file() -> Option<std::path::PathBuf> {
    rfd::FileDialog::new()
        .add_filter("Jackdaw scene", &["bsn", "jsn"])
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

#[operator(
    id = "scene.switch",
    label = "Switch Scene",
    allows_undo = false,
    params(tab(i64, doc = "Index of the tab to activate, counting from zero."))
)]
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
    commands.queue(|world: &mut World| {
        scene_save_all_system(world);
    });
    OperatorResult::Finished
}

/// Iterate tabs, switching to each in turn, serializing tabs with a path
/// to disk synchronously, then return to the originally-active tab.
///
/// Clean tabs without a path (untitled) are skipped. A dirty untitled tab or
/// any authoritative write failure makes the result `false` and leaves that
/// tab dirty, allowing callers such as quit confirmation to refuse to exit.
pub fn scene_save_all_system(world: &mut World) -> bool {
    let original_active = world.resource::<Scenes>().active;
    let count = world.resource::<Scenes>().tabs.len();
    let mut all_saved = true;

    for i in 0..count {
        let (path, dirty) = {
            let scenes = world.resource::<Scenes>();
            (scenes.tabs[i].path.clone(), scenes.tabs[i].dirty)
        };
        let Some(path) = path else {
            if dirty {
                warn!("scene.save_all: cannot save dirty untitled tab {i} without a path");
                all_saved = false;
            }
            continue;
        };

        if i != world.resource::<Scenes>().active {
            swap_active_tab(world, i);
        }

        // Use the same persistence boundary as a single-scene save: terrain
        // sidecars complete before scene text, legacy paths redirect safely,
        // and dirty state only clears after every authoritative write.
        let path_str = path.to_string_lossy().into_owned();
        if let Some(mut sfp) = world.get_resource_mut::<SceneFilePath>() {
            sfp.path = Some(path_str);
        }

        if let Err(err) = crate::scene_io::save_scene_inner(world) {
            warn!("scene.save_all: failed to save {path:?}: {err}");
            all_saved = false;
        }
    }

    // Restore to the originally active tab, then re-point SceneFilePath at
    // its (possibly redirected) path.
    if original_active != world.resource::<Scenes>().active {
        swap_active_tab(world, original_active);
    }
    let active_path = world.resource::<Scenes>().tabs[original_active]
        .path
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned());
    if let Some(mut sfp) = world.get_resource_mut::<SceneFilePath>() {
        sfp.path = active_path;
    }

    all_saved
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
