//! Confirmation dialogs for dirty-tab close and quit. Modal scrim + card +
//! three buttons. State lives in `PendingTabClose` / `PendingQuit` so the
//! dialogs can be opened from any operator and resolved by any of three
//! buttons.
//!
//! If the tab being saved has no path (untitled), the Save action falls
//! back to Discard with a warning log; there is no `rfd::FileDialog`
//! sub-flow to pick a path for an untitled tab.

use bevy::picking::events::{Click, Pointer};
use bevy::prelude::*;
use jackdaw_feathers::{icons::EditorFont, tokens};

/// Holds the pending tab index when the user tried to close a dirty tab
/// but has not yet confirmed the action.
#[derive(Resource, Default)]
pub struct PendingTabClose {
    /// `Some(idx)` while the confirm dialog is displayed.
    /// `None` otherwise.
    pub tab_index: Option<usize>,
}

/// Whether the "save-all before quit" dialog is currently shown.
#[derive(Resource, Default)]
pub struct PendingQuit {
    /// `true` while the quit confirmation dialog is displayed.
    pub active: bool,
    /// `true` when confirming leaves the open project for the launcher rather
    /// than exiting. Both discard the editor's live scene and ask the same
    /// question with different wording.
    pub leaving_project: bool,
}

/// Marker on the dialog root (the scrim node). Used to despawn the whole
/// dialog tree in one step.
#[derive(Component)]
pub struct ConfirmDialogRoot;

/// Discriminates the three action buttons.
#[derive(Component, Clone, Copy)]
pub enum ConfirmDialogButton {
    Save,
    Discard,
    Cancel,
}

/// Spawn the confirm dialog. The caller must have already written the
/// target index into `PendingTabClose.tab_index` before calling this.
///
/// Skips UI spawning when `EditorFont` is absent (e.g. headless tests).
/// In that case, `PendingTabClose.tab_index` is still set, so test
/// assertions against the resource still work.
pub fn spawn_confirm_dialog(world: &mut World, tab_display_name: &str) {
    let Some(editor_font) = world.get_resource::<EditorFont>().map(|f| f.0.clone()) else {
        return;
    };

    // Full-window scrim that dims content behind the modal.
    let scrim = world
        .spawn((
            ConfirmDialogRoot,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..Default::default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
            GlobalZIndex(100),
        ))
        .id();

    // Centered card.
    let card = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(12.0),
                padding: UiRect::all(Val::Px(24.0)),
                min_width: Val::Px(380.0),
                max_width: Val::Px(480.0),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                ..Default::default()
            },
            BackgroundColor(tokens::PANEL_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            ChildOf(scrim),
        ))
        .id();

    // Title text.
    world.spawn((
        Text::new("Unsaved Changes"),
        TextFont {
            font: editor_font.clone().into(),
            font_size: tokens::TEXT_SIZE_LG,
            ..Default::default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        ChildOf(card),
    ));

    // Body message.
    let message = format!(
        "\"{}\" has unsaved changes. Save before closing?",
        tab_display_name
    );
    world.spawn((
        Text::new(message),
        TextFont {
            font: editor_font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(card),
    ));

    // Button row.
    let button_row = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::FlexEnd,
                column_gap: Val::Px(8.0),
                margin: UiRect::top(Val::Px(8.0)),
                ..Default::default()
            },
            ChildOf(card),
        ))
        .id();

    spawn_dialog_button(
        world,
        button_row,
        editor_font.clone(),
        "Cancel",
        ConfirmDialogButton::Cancel,
        tokens::TOOLBAR_BG,
    );

    spawn_dialog_button(
        world,
        button_row,
        editor_font.clone(),
        "Discard",
        ConfirmDialogButton::Discard,
        tokens::TOOLBAR_BG,
    );

    spawn_dialog_button(
        world,
        button_row,
        editor_font,
        "Save",
        ConfirmDialogButton::Save,
        tokens::SELECTED_BG,
    );
}

/// Spawn a single labeled button into `parent` and attach the click observer.
fn spawn_dialog_button(
    world: &mut World,
    parent: Entity,
    editor_font: Handle<Font>,
    label: &str,
    kind: ConfirmDialogButton,
    bg: Color,
) {
    let btn = world
        .spawn((
            kind,
            Node {
                padding: UiRect::axes(Val::Px(16.0), Val::Px(8.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                ..Default::default()
            },
            BackgroundColor(bg),
            ChildOf(parent),
        ))
        .id();

    world.spawn((
        Text::new(label.to_string()),
        TextFont {
            font: editor_font.into(),
            font_size: tokens::TEXT_SIZE,
            ..Default::default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        Pickable::IGNORE,
        ChildOf(btn),
    ));

    world.entity_mut(btn).observe(on_dialog_button_click);
}

// ---------------------------------------------------------------------------
// Quit dialog (Save All / Discard All / Cancel)
// ---------------------------------------------------------------------------

/// Discriminates the three action buttons in the quit-confirmation dialog.
#[derive(Component, Clone, Copy)]
pub enum ConfirmQuitButton {
    SaveAll,
    DiscardAll,
    Cancel,
}

/// Spawn the "unsaved changes on quit" dialog.
///
/// Skips UI spawning when `EditorFont` is absent (e.g. headless tests).
/// In that case `PendingQuit.active` is still expected to have been set by
/// the caller before this is invoked, so test assertions still work.
pub fn spawn_confirm_quit_dialog(world: &mut World) {
    let Some(editor_font) = world.get_resource::<EditorFont>().map(|f| f.0.clone()) else {
        return;
    };

    // Full-window scrim.
    let scrim = world
        .spawn((
            ConfirmDialogRoot,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..Default::default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
            GlobalZIndex(100),
        ))
        .id();

    // Centered card.
    let card = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(12.0),
                padding: UiRect::all(Val::Px(24.0)),
                min_width: Val::Px(380.0),
                max_width: Val::Px(480.0),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                ..Default::default()
            },
            BackgroundColor(tokens::PANEL_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            ChildOf(scrim),
        ))
        .id();

    // Title.
    world.spawn((
        Text::new("Unsaved Changes"),
        TextFont {
            font: editor_font.clone().into(),
            font_size: tokens::TEXT_SIZE_LG,
            ..Default::default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        ChildOf(card),
    ));

    // Body.
    let body = if world.resource::<PendingQuit>().leaving_project {
        "You have unsaved changes. Save all before closing this project?"
    } else {
        "You have unsaved changes. Save all before quitting?"
    };
    world.spawn((
        Text::new(body),
        TextFont {
            font: editor_font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(card),
    ));

    // Button row.
    let button_row = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::FlexEnd,
                column_gap: Val::Px(8.0),
                margin: UiRect::top(Val::Px(8.0)),
                ..Default::default()
            },
            ChildOf(card),
        ))
        .id();

    spawn_quit_button(
        world,
        button_row,
        editor_font.clone(),
        "Cancel",
        ConfirmQuitButton::Cancel,
        tokens::TOOLBAR_BG,
    );

    spawn_quit_button(
        world,
        button_row,
        editor_font.clone(),
        "Discard All",
        ConfirmQuitButton::DiscardAll,
        tokens::TOOLBAR_BG,
    );

    spawn_quit_button(
        world,
        button_row,
        editor_font,
        "Save All",
        ConfirmQuitButton::SaveAll,
        tokens::SELECTED_BG,
    );
}

/// Spawn a single labeled button for the quit dialog.
fn spawn_quit_button(
    world: &mut World,
    parent: Entity,
    editor_font: Handle<Font>,
    label: &str,
    kind: ConfirmQuitButton,
    bg: Color,
) {
    let btn = world
        .spawn((
            kind,
            Node {
                padding: UiRect::axes(Val::Px(16.0), Val::Px(8.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                ..Default::default()
            },
            BackgroundColor(bg),
            ChildOf(parent),
        ))
        .id();

    world.spawn((
        Text::new(label.to_string()),
        TextFont {
            font: editor_font.into(),
            font_size: tokens::TEXT_SIZE,
            ..Default::default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        Pickable::IGNORE,
        ChildOf(btn),
    ));

    world.entity_mut(btn).observe(on_quit_dialog_button_click);
}

/// Observer attached to each quit-dialog button.
pub fn on_quit_dialog_button_click(
    trigger: On<Pointer<Click>>,
    buttons: Query<&ConfirmQuitButton>,
    dialog: Query<Entity, With<ConfirmDialogRoot>>,
    mut commands: Commands,
) -> Result<(), BevyError> {
    let Ok(kind) = buttons.get(trigger.event_target()) else {
        return Ok(());
    };
    let action = *kind;

    // Despawn the entire dialog tree immediately.
    for root in dialog.iter() {
        if let Ok(mut ec) = commands.get_entity(root) {
            ec.despawn();
        }
    }

    commands.queue(move |world: &mut World| apply_confirm_quit_action(world, action));

    Ok(())
}

/// Resolve the quit dialog. Separate from the observer so each of the three
/// answers can be run against a world directly.
pub(crate) fn apply_confirm_quit_action(world: &mut World, action: ConfirmQuitButton) {
    let leaving_project = {
        let Some(mut pending) = world.get_resource_mut::<PendingQuit>() else {
            return;
        };
        pending.active = false;
        pending.leaving_project
    };

    match action {
        ConfirmQuitButton::SaveAll => {
            if crate::scenes::operators::scene_save_all_system(world) {
                confirmed_leave(world, leaving_project);
            } else {
                warn!("leaving cancelled because one or more scenes could not be saved");
                // A failed save leaves the editor where it was, so it disarms what
                // the confirmed path would have acted on.
                disarm_leave(world);
                report_save_failure(world);
            }
        }
        ConfirmQuitButton::DiscardAll => confirmed_leave(world, leaving_project),
        ConfirmQuitButton::Cancel => disarm_leave(world),
    }
}

/// Clear what the dialog armed: the project the user picked would otherwise
/// open the next time the launcher is reached, and the wording flag would
/// outlive the question it belonged to.
fn disarm_leave(world: &mut World) {
    world.remove_resource::<crate::project_select::PendingAutoOpen>();
    if let Some(mut pending) = world.get_resource_mut::<PendingQuit>() {
        pending.leaving_project = false;
    }
}

/// Report that nothing was saved. The dialog is gone by the time a save is
/// attempted, so without a toast the user reads its disappearance as success.
fn report_save_failure(world: &mut World) {
    let (Some(editor_font), Some(icon_font)) = (
        world.get_resource::<EditorFont>().map(|f| f.0.clone()),
        world
            .get_resource::<jackdaw_feathers::icons::IconFont>()
            .map(|f| f.0.clone()),
    ) else {
        return;
    };
    world.spawn(jackdaw_feathers::toast::toast(
        jackdaw_feathers::toast::ToastVariant::Error,
        "Could not save every scene. Nothing was closed.",
        jackdaw_feathers::toast::DEFAULT_TOAST_DURATION,
        &editor_font,
        &icon_font,
    ));
}

/// Act on a confirmed Save All / Discard All: back to the launcher when the
/// user was closing the project, out of the app otherwise.
fn confirmed_leave(world: &mut World, leaving_project: bool) {
    if leaving_project {
        world.resource_mut::<PendingQuit>().leaving_project = false;
        world
            .resource_mut::<NextState<crate::AppState>>()
            .set(crate::AppState::ProjectSelect);
        return;
    }
    world
        .resource_mut::<bevy::ecs::message::Messages<bevy::app::AppExit>>()
        .write(bevy::app::AppExit::Success);
}

/// Leave the open project for the launcher, asking about unsaved work first.
/// Returns `true` when the caller may flip the state itself; `false` when a
/// dialog owns the decision.
///
/// Both ways out of a project (Home and Open Recent) go through here, since
/// leaving clears the live scene either way.
pub fn leave_project_or_confirm(world: &mut World) -> bool {
    let any_dirty = world
        .get_resource::<crate::scenes::Scenes>()
        .is_some_and(|scenes| scenes.tabs.iter().any(|tab| tab.dirty));
    if !any_dirty {
        return true;
    }
    let Some(mut pending) = world.get_resource_mut::<PendingQuit>() else {
        return true;
    };
    if pending.active {
        return false;
    }
    pending.active = true;
    pending.leaving_project = true;
    spawn_confirm_quit_dialog(world);
    false
}

// ---------------------------------------------------------------------------
// Tab-close dialog (Save / Discard / Cancel)
// ---------------------------------------------------------------------------

/// Observer attached to each button. Routes to Save / Discard / Cancel logic.
pub fn on_dialog_button_click(
    trigger: On<Pointer<Click>>,
    buttons: Query<&ConfirmDialogButton>,
    dialog: Query<Entity, With<ConfirmDialogRoot>>,
    mut commands: Commands,
) -> Result<(), BevyError> {
    let Ok(kind) = buttons.get(trigger.event_target()) else {
        return Ok(());
    };
    let action = *kind;

    // Despawn the entire dialog tree immediately.
    for root in dialog.iter() {
        if let Ok(mut ec) = commands.get_entity(root) {
            ec.despawn();
        }
    }

    commands.queue(move |world: &mut World| {
        let Some(target) = world.resource::<PendingTabClose>().tab_index else {
            return;
        };
        apply_confirm_dialog_action(world, action, target);
    });

    Ok(())
}

/// The Save / Discard / Cancel decision for a pending tab close, factored
/// out of the click observer so it is callable (and testable) directly
/// with a target index rather than only through `PendingTabClose`.
fn apply_confirm_dialog_action(world: &mut World, action: ConfirmDialogButton, target: usize) {
    match action {
        ConfirmDialogButton::Save => {
            let tab_count = world.resource::<crate::scenes::Scenes>().tabs.len();
            if target >= tab_count {
                world.resource_mut::<PendingTabClose>().tab_index = None;
                return;
            }

            let tab_path = world.resource::<crate::scenes::Scenes>().tabs[target]
                .path
                .clone();

            if let Some(path) = tab_path {
                // Swap to the target tab if it is not active.
                let active = world.resource::<crate::scenes::Scenes>().active;
                if active != target {
                    crate::scenes::swap::swap_active_tab(world, target);
                }

                // Point SceneFilePath at this tab so save works correctly.
                let path_str = path.to_string_lossy().into_owned();
                if let Some(mut sfp) = world.get_resource_mut::<crate::scene_io::SceneFilePath>() {
                    sfp.path = Some(path_str);
                }

                // Cleared regardless of outcome: siblings (untitled tab,
                // Discard, Cancel) all clear it unconditionally, and
                // leaving it set on a failed save here used to make
                // scene_close_system treat every later close request on
                // any dirty tab as "a dialog is already up" and silently
                // ignore it.
                world.resource_mut::<PendingTabClose>().tab_index = None;

                if crate::scene_io::save_scene(world) {
                    // The save boundary cleared dirty state only after
                    // every authoritative file reached disk.
                    crate::scenes::operators::scene_close_system_unprompted(world, target);
                } else {
                    warn!("confirm_dialog: save failed; keeping tab {target} open");
                }
            } else {
                // Untitled tab: no path available.
                // Deviation: falling back to Discard with a warning.
                // A full file-save-dialog sub-flow for this case is deferred.
                warn!(
                    "confirm_dialog: tab {} is untitled; treating Save as Discard (deferred)",
                    target
                );
                world.resource_mut::<PendingTabClose>().tab_index = None;
                crate::scenes::operators::scene_close_system_unprompted(world, target);
            }
        }
        ConfirmDialogButton::Discard => {
            world.resource_mut::<PendingTabClose>().tab_index = None;
            crate::scenes::operators::scene_close_system_unprompted(world, target);
        }
        ConfirmDialogButton::Cancel => {
            world.resource_mut::<PendingTabClose>().tab_index = None;
        }
    }
}

#[cfg(test)]
mod apply_confirm_dialog_action_tests {
    use crate::scene_io::SceneFilePath;
    use crate::scenes::{SceneTab, Scenes};

    use super::*;

    /// A world with one dirty tab pointed at `path`, and a confirm dialog
    /// already pending on it -- enough for the Save branch of
    /// `apply_confirm_dialog_action` to run without touching BSN AST or
    /// asset-catalog machinery (both are optional resources that
    /// `save_scene_inner` tolerates being absent).
    fn world_with_dirty_tab_at(path: std::path::PathBuf) -> World {
        let mut world = World::new();
        world.init_resource::<jackdaw_commands::CommandHistory>();
        world.init_resource::<crate::scene_io::SceneDirtyState>();
        world.init_resource::<SceneFilePath>();

        let mut tab = SceneTab::new_untitled(1);
        tab.path = Some(path);
        tab.dirty = true;
        world.insert_resource(Scenes {
            tabs: vec![tab],
            active: 0,
        });
        world.insert_resource(PendingTabClose { tab_index: Some(0) });
        world
    }

    /// I10(b) pinning test, the exact scenario from the review finding:
    /// a failed save from the tab-close confirm dialog used to leave
    /// `PendingTabClose.tab_index` set, which made `scene_close_system`
    /// treat every later close request on any dirty tab as "a dialog is
    /// already up" and silently ignore it.
    #[test]
    fn a_failed_save_still_clears_pending_tab_close() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let blocked_parent = tmp.path().join("not-a-directory");
        std::fs::write(&blocked_parent, b"blocks directory creation").expect("seed blocker");
        let scene_path = blocked_parent.join("zone.bsn");

        let mut world = world_with_dirty_tab_at(scene_path);

        apply_confirm_dialog_action(&mut world, ConfirmDialogButton::Save, 0);

        assert_eq!(
            world.resource::<PendingTabClose>().tab_index,
            None,
            "a failed save must still clear tab_index so later closes are not ignored",
        );
        assert!(
            world.resource::<Scenes>().tabs[0].dirty,
            "a failed save must leave the tab dirty",
        );
    }

    /// Closing a project throws away the live scene as quitting does, so unsaved work stops
    /// the caller and raises the dialog.
    #[test]
    fn unsaved_work_holds_the_project_open_until_it_is_answered() {
        let mut world = world_with_dirty_tab_at(std::path::PathBuf::from("/nowhere/zone.bsn"));
        world.init_resource::<PendingQuit>();

        assert!(
            !leave_project_or_confirm(&mut world),
            "the caller must not leave while the question is unanswered"
        );
        let pending = world.resource::<PendingQuit>();
        assert!(pending.active);
        assert!(pending.leaving_project, "the wording follows this flag");
    }

    /// A save that could not happen leaves nothing armed: a project pick left behind would
    /// have the next Home relaunch into that project instead of reaching the launcher.
    #[test]
    fn a_failed_save_disarms_the_project_pick_and_the_wording() {
        let mut world = world_with_dirty_tab_at(std::path::PathBuf::from("/nowhere/zone.bsn"));
        // Untitled, so there is no path to save it to.
        world.resource_mut::<Scenes>().tabs[0].path = None;
        world.insert_resource(PendingQuit {
            active: true,
            leaving_project: true,
        });
        world.insert_resource(crate::project_select::PendingAutoOpen {
            path: std::path::PathBuf::from("/projects/beta"),
            skip_build: false,
        });

        apply_confirm_quit_action(&mut world, ConfirmQuitButton::SaveAll);

        assert!(
            world
                .get_resource::<crate::project_select::PendingAutoOpen>()
                .is_none(),
            "a pick nobody confirmed must not survive to fire later",
        );
        let pending = world.resource::<PendingQuit>();
        assert!(!pending.active);
        assert!(!pending.leaving_project);
        assert!(
            world.resource::<Scenes>().tabs[0].dirty,
            "the work is still unsaved",
        );
    }

    /// With nothing unsaved there is no question, so the caller leaves at once.
    #[test]
    fn a_saved_project_closes_without_a_prompt() {
        let mut world = world_with_dirty_tab_at(std::path::PathBuf::from("/nowhere/zone.bsn"));
        world.init_resource::<PendingQuit>();
        world.resource_mut::<Scenes>().tabs[0].dirty = false;

        assert!(leave_project_or_confirm(&mut world));
        assert!(!world.resource::<PendingQuit>().active);
    }
}
