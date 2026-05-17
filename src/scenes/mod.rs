//! Multi-scene editor state. Owns the tab list; the active tab's
//! contents live in the live Bevy world, inactive tabs hold a
//! `JsnScene` snapshot plus the per-tab view state and history.

pub mod confirm_dialog;
pub mod operators;
pub mod swap;
pub mod ui;

use std::path::PathBuf;

use bevy::prelude::*;
use jackdaw_jsn::format::JsnScene;

use crate::commands::CommandHistory;
use crate::project::ProjectRoot;

pub struct ScenesPlugin;

impl Plugin for ScenesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Scenes>();
        app.init_resource::<confirm_dialog::PendingTabClose>();
        app.init_resource::<confirm_dialog::PendingQuit>();
        app.add_systems(
            Update,
            (
                mark_active_dirty_on_history_growth,
                persist_tabs_to_project_config,
                ui::rebuild_scene_tab_strip,
                ui::update_scene_tab_visuals,
                ui::update_scene_tab_label_abbreviation,
                ui::show_scene_tab_close_on_hover,
                intercept_window_close,
            ),
        );
        app.add_observer(ui::on_scene_tab_context_action);
    }
}

/// Intercept `WindowCloseRequested`. If any tab is dirty, show the
/// confirm-quit dialog and consume the event. Otherwise emit `AppExit::Success`
/// so the normal X-click flow still works.
pub fn intercept_window_close(
    mut close_events: bevy::ecs::message::MessageReader<bevy::window::WindowCloseRequested>,
    mut exit: bevy::ecs::message::MessageWriter<bevy::app::AppExit>,
    scenes: Res<Scenes>,
    mut commands: Commands,
    mut pending: ResMut<confirm_dialog::PendingQuit>,
) {
    let mut close_requested = false;
    for _ in close_events.read() {
        close_requested = true;
    }
    if !close_requested {
        return;
    }

    let any_dirty = scenes.tabs.iter().any(|t| t.dirty);
    if !any_dirty {
        exit.write(bevy::app::AppExit::Success);
        return;
    }

    // Dialog already open; ignore the repeated event.
    if pending.active {
        return;
    }
    pending.active = true;

    commands.queue(|world: &mut World| {
        confirm_dialog::spawn_confirm_quit_dialog(world);
    });
}

#[derive(Resource, Default)]
pub struct Scenes {
    pub tabs: Vec<SceneTab>,
    pub active: usize,
}

impl Scenes {
    /// Append a tab and return its index. Does not activate it.
    pub fn push_tab(&mut self, tab: SceneTab) -> usize {
        let idx = self.tabs.len();
        self.tabs.push(tab);
        idx
    }
}

pub struct SceneTab {
    pub path: Option<PathBuf>,
    pub display_name: String,
    pub dirty: bool,
    pub snapshot: Option<JsnScene>,
    pub view_state: ViewState,
    pub history: CommandHistory,
    /// Recorded `CommandHistory.undo_stack.len()` as of the last time
    /// the dirty-tracking system ran (or the tab was activated, or
    /// saved). If the live history is deeper than this, the user has
    /// made a change since the last check, and the tab is marked
    /// `dirty`.
    pub history_depth_at_last_check: usize,
}

impl SceneTab {
    pub fn new_untitled(n: u32) -> Self {
        Self {
            path: None,
            display_name: format!("untitled-{n}"),
            dirty: false,
            snapshot: None,
            view_state: ViewState::with_default_camera(),
            history: CommandHistory::default(),
            history_depth_at_last_check: 0,
        }
    }
}

/// When `Scenes` mutates, mirror the open-tab paths into the project
/// config so they survive across sessions. Skipped if no project root
/// is loaded (e.g. project-selector screen, headless tests).
pub fn persist_tabs_to_project_config(
    scenes: Res<Scenes>,
    project_root: Option<ResMut<ProjectRoot>>,
) {
    if !scenes.is_changed() {
        return;
    }
    let Some(mut project_root) = project_root else {
        return;
    };

    let last_open_tabs: Vec<String> = scenes
        .tabs
        .iter()
        .filter_map(|t| t.path.as_ref())
        .filter_map(|p| project_root.to_relative(p).to_str().map(str::to_owned))
        .collect();
    let last_active_tab = scenes.active;

    let cfg = &mut project_root.config.project;
    if cfg.last_open_tabs == last_open_tabs && cfg.last_active_tab == last_active_tab {
        return;
    }
    cfg.last_open_tabs = last_open_tabs;
    cfg.last_active_tab = last_active_tab;

    let root = project_root.root.clone();
    let project = project_root.config.clone();
    if let Err(e) = crate::project::save_project_config(&root, &project) {
        warn!("Failed to persist tab list to project.jsn: {e}");
    }
}

/// Per-frame system: compare the active tab's recorded history depth
/// with the live `CommandHistory`. Any growth means the user did
/// something; mark the tab dirty. Shrinkage (undo) is ignored. Save
/// resets the recorded depth.
pub fn mark_active_dirty_on_history_growth(
    history: Res<CommandHistory>,
    mut scenes: ResMut<Scenes>,
) {
    if scenes.tabs.is_empty() {
        return;
    }
    let active = scenes.active;
    let current_depth = history.undo_stack.len();
    let tab = scenes.bypass_change_detection().tabs.get_mut(active);
    let Some(tab) = tab else { return };
    let recorded = tab.history_depth_at_last_check;
    let needs_dirty_flip = current_depth > recorded && !tab.dirty;
    let needs_depth_update = current_depth != recorded;
    if !needs_dirty_flip && !needs_depth_update {
        return;
    }
    scenes.set_changed();
    let tab = &mut scenes.tabs[active];
    if needs_dirty_flip {
        tab.dirty = true;
    }
    tab.history_depth_at_last_check = current_depth;
}

#[derive(Default, Clone)]
pub struct ViewState {
    pub camera_transform: Transform,
    /// Optional projection matrix. `None` means use the editor's default
    /// perspective on restore. Stored as a `Mat4` so we don't have to
    /// reflect the entire `Projection` enum across tab swaps.
    pub camera_projection: Option<bevy::math::Mat4>,
    pub edit_mode: crate::brush::EditMode,
    /// Object-level selection stored as stable IDs so it survives the
    /// despawn / respawn cycle of a tab swap. `BrushStableId` lives in
    /// `crate::draw_brush` since that's where the stable-ID counter is
    /// defined; not all selected entities are brushes, but the
    /// counter and component are the editor-wide identity mechanism.
    pub selection: Vec<crate::draw_brush::BrushStableId>,
    /// Brush sub-element selection (verts, edges, faces) for whichever
    /// brush is active in `selection`.
    pub brush_sub_selection: crate::brush::BrushSelection,
}

impl ViewState {
    /// Default `ViewState` for a freshly-created tab. Uses the same
    /// initial camera framing as the viewport setup (looking at the
    /// origin from `(0, 4, 8)`), so a new untitled scene shows the
    /// grid + axes instead of the camera sitting on the origin and
    /// rendering the grid edge-on.
    pub fn with_default_camera() -> Self {
        Self {
            camera_transform: Transform::from_xyz(0.0, 4.0, 8.0)
                .looking_at(bevy::math::Vec3::ZERO, bevy::math::Vec3::Y),
            ..Self::default()
        }
    }
}
