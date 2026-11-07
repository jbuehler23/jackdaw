//! A save the user asks for writes its file off the main thread: what lands is
//! the document as it stood when the save was asked for, read back first, and
//! nothing claims the save happened before it did.

use super::save_baseline;
use crate::util::{self, OperatorResultExt as _};

use std::any::Any;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bevy::prelude::*;
use jackdaw::boot_ops::{run_op_clause, run_op_clause_as_user};
use jackdaw::scene_io::hooks::{DocumentIoHook, DocumentIoHooks, SidecarWrite};
use jackdaw::scene_io::save_worker::{
    hold_saves_after_writing, hold_saves_before_renaming, hold_saves_before_writing,
};
use jackdaw::scene_io::{SidecarImport, wait_for_saves};
use jackdaw::scenes::Scenes;
use jackdaw::scenes::external_watch::ExternalSceneChanges;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::document::SceneSaved;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};

const ZONE: &str = "#Zone\nbevy_transform::components::transform::Transform\n";
const YARD: &str = "#Yard\nbevy_transform::components::transform::Transform\n";

struct Project {
    app: App,
    tmp: tempfile::TempDir,
    scene: PathBuf,
}

impl Project {
    fn asset(&self, name: &str) -> PathBuf {
        self.tmp.path().join("assets").join(name)
    }

    fn on_disk(&self) -> String {
        std::fs::read_to_string(&self.scene).expect("the scene reads")
    }

    fn active_dirty(&self) -> bool {
        let scenes = self.app.world().resource::<Scenes>();
        scenes.tabs[scenes.active].dirty
    }

    fn notice(&self) -> String {
        self.app
            .world()
            .resource::<jackdaw::status_bar::StatusNotice>()
            .text()
            .to_string()
    }

    fn phase(&self) -> String {
        self.app
            .world()
            .resource::<jackdaw::status_bar::EditorPhase>()
            .current()
            .unwrap_or_default()
            .to_string()
    }

    fn edit(&mut self, name: &str) {
        let clause = format!("entity.add.group name={name} parent=Zone");
        run_op_clause_as_user(self.app.world_mut(), &clause)
            .expect("the group dispatched")
            .assert_finished();
        self.app.update();
    }

    /// Save the way a keybind, a menu row or a button does.
    fn save_as_user(&mut self) {
        run_op_clause_as_user(self.app.world_mut(), "scene.save")
            .expect("the save dispatched")
            .assert_finished();
    }

    fn settle(&mut self) {
        wait_for_saves(self.app.world_mut());
        self.app.update();
    }
}

fn project() -> Project {
    project_with_files(|_| {})
}

/// [`project`], with `files` adding to its folder before the project opens.
fn project_with_files(files: impl FnOnce(&std::path::Path)) -> Project {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("assets")).expect("assets folder");
    files(tmp.path());
    let scene = tmp.path().join("assets/zone.bsn");
    std::fs::write(&scene, ZONE).expect("scene written");
    std::fs::write(tmp.path().join("assets/yard.bsn"), YARD).expect("scene written");

    let mut app = util::editor_test_app();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: tmp.path().to_path_buf(),
            config: default(),
        });
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    run_op_clause(app.world_mut(), "scene.open path=assets/zone.bsn")
        .expect("the scene opens")
        .assert_finished();
    app.update();
    let mut project = Project { app, tmp, scene };
    project.edit("Props");
    project
}

fn add_unreadable_root(app: &mut App) -> Entity {
    let mut ast = app.world_mut().resource_mut::<jackdaw_bsn::SceneBsnAst>();
    let node = ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::Struct(
        jackdaw_bsn::BsnStructData {
            type_path: String::new(),
            fields: jackdaw_bsn::BsnStructFields(vec![jackdaw_bsn::BsnField {
                name: "0".into(),
                value: jackdaw_bsn::BsnValue::Int(1),
            }]),
        },
    )]);
    ast.add_to_roots(node);
    node
}

fn remove_root(app: &mut App, node: Entity) {
    app.world_mut()
        .resource_mut::<jackdaw_bsn::SceneBsnAst>()
        .roots
        .retain(|root| *root != node);
}

/// Let go of `hold` from another thread, for a call that blocks until it is gone.
fn release_soon(hold: impl Send + 'static) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        drop(hold);
    });
}

fn play(app: &mut App) {
    app.world_mut()
        .operator("pie.play")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: false,
        })
        .call()
        .expect("dispatch")
        .assert_finished();
}

fn exits(app: &App) -> usize {
    app.world()
        .resource::<bevy::ecs::message::Messages<AppExit>>()
        .len()
}

#[test]
fn a_save_as_the_user_returns_before_the_file_is_written() {
    let mut project = project();
    let hold = hold_saves_before_writing(project.app.world_mut());

    project.save_as_user();
    project.app.update();

    assert_eq!(project.on_disk(), ZONE, "the worker is held");
    assert!(
        project.active_dirty(),
        "and nothing claims the save happened"
    );
    assert!(jackdaw::scene_io::saves_in_flight(project.app.world()));
    assert!(project.phase().contains("zone.bsn"), "{}", project.phase());

    drop(hold);
    project.settle();

    assert!(project.on_disk().contains("Props"));
    assert!(!project.active_dirty());
    assert!(!project.phase().contains("zone.bsn"), "{}", project.phase());
}

#[test]
fn a_failed_validation_keeps_the_file_and_the_dirty_state_and_says_so() {
    let mut project = project();
    add_unreadable_root(&mut project.app);

    project.save_as_user();
    project.settle();

    assert_eq!(project.on_disk(), ZONE, "the old file is untouched");
    assert!(project.active_dirty());
    assert!(
        project.notice().contains("Not saved"),
        "{}",
        project.notice()
    );
    assert!(
        project.notice().contains("zone.bsn"),
        "{}",
        project.notice()
    );
}

#[test]
fn an_edit_during_a_save_leaves_the_tab_dirty_and_the_next_save_writes_it() {
    let mut project = project();
    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    project.edit("Later");
    drop(hold);
    project.settle();

    let saved = project.on_disk();
    assert!(saved.contains("Props"), "the snapshot landed:\n{saved}");
    assert!(!saved.contains("Later"), "as of the save:\n{saved}");
    assert!(project.active_dirty(), "the edit is not on disk");

    project.save_as_user();
    project.settle();
    assert!(project.on_disk().contains("Later"));
    assert!(!project.active_dirty());
}

#[test]
fn an_undo_during_a_save_of_a_clean_tab_leaves_it_dirty() {
    let mut project = project();
    project.save_as_user();
    project.settle();
    assert!(!project.active_dirty());

    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    run_op_clause(project.app.world_mut(), "history.undo")
        .expect("the undo dispatched")
        .assert_finished();
    project.app.update();
    drop(hold);
    project.settle();

    assert!(project.on_disk().contains("Props"));
    assert!(
        project.active_dirty(),
        "the world no longer matches the file"
    );
}

#[test]
fn saves_in_a_row_coalesce_and_the_last_state_wins() {
    #[derive(Resource, Default)]
    struct Saved(usize);

    let mut project = project();
    project.app.world_mut().init_resource::<Saved>();
    project
        .app
        .world_mut()
        .add_observer(|_: On<SceneSaved>, mut saved: ResMut<Saved>| saved.0 += 1);
    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    project.edit("Second");
    project.save_as_user();
    project.edit("Third");
    project.save_as_user();
    drop(hold);
    project.settle();

    let saved = project.on_disk();
    assert!(
        saved.contains("Second") && saved.contains("Third"),
        "{saved}"
    );
    assert!(!project.active_dirty());
    assert_eq!(
        project.app.world().resource::<Saved>().0,
        2,
        "the one running and the last one asked for"
    );
}

#[test]
fn quitting_during_a_save_waits_for_a_complete_file_and_says_so() {
    let mut project = project();
    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();

    project
        .app
        .world_mut()
        .write_message(bevy::window::WindowCloseRequested {
            window: Entity::PLACEHOLDER,
        });
    project.app.update();
    project.app.update();

    assert_eq!(exits(&project.app), 0, "the file is not there yet");
    assert!(project.phase().contains("Waiting"), "{}", project.phase());

    drop(hold);
    util::update_until(
        &mut project.app,
        "the editor quits",
        Duration::from_secs(30),
        |app| exits(app) > 0,
    );
    let saved = project.on_disk();
    assert!(saved.contains("Props"), "{saved}");
    assert!(jackdaw_bsn::parse_bsn_text(&saved).is_ok());
}

#[test]
fn an_app_exit_from_elsewhere_during_a_save_still_lands_the_file() {
    let mut project = project();
    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    release_soon(hold);
    project.app.world_mut().write_message(AppExit::Success);
    project.app.update();
    assert!(project.on_disk().contains("Props"), "before the app ends");
}

#[test]
fn closing_a_tab_during_its_save_waits_then_closes() {
    let mut project = project();
    let yard_file = project.asset("yard.bsn");
    jackdaw::scenes::operators::scene_open_system(project.app.world_mut(), &yard_file);
    project.app.update();
    let zone = project
        .app
        .world()
        .resource::<Scenes>()
        .tabs
        .iter()
        .position(|tab| tab.display_name == "zone")
        .expect("the zone tab");
    jackdaw::scenes::swap::swap_active_tab(project.app.world_mut(), zone);
    project.app.update();

    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    jackdaw::scenes::operators::scene_close_system(project.app.world_mut(), zone);
    project.app.update();
    assert_eq!(project.app.world().resource::<Scenes>().tabs.len(), 2);
    assert!(project.phase().contains("Waiting"), "{}", project.phase());
    let dialog = project
        .app
        .world()
        .resource::<jackdaw::scenes::confirm_dialog::PendingTabClose>()
        .tab_index;
    assert_eq!(dialog, None, "no question about edits being saved");

    drop(hold);
    project.settle();
    assert_eq!(project.app.world().resource::<Scenes>().tabs.len(), 1);
    assert!(project.on_disk().contains("Props"));
}

#[test]
fn leaving_the_project_during_a_save_waits() {
    let mut project = project();
    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    assert!(
        !jackdaw::scenes::confirm_dialog::leave_project_or_confirm(project.app.world_mut()),
        "the caller stays until the file is there"
    );
    project.app.update();
    assert!(project.phase().contains("Waiting"), "{}", project.phase());
    drop(hold);
    project.settle();
    assert!(project.on_disk().contains("Props"));
    project.app.update();
    assert_eq!(
        *project
            .app
            .world()
            .resource::<State<jackdaw::AppState>>()
            .get(),
        jackdaw::AppState::ProjectSelect,
    );
}

fn run_watch_frames(app: &mut App, ms: u64) {
    let until = std::time::Instant::now() + Duration::from_millis(ms);
    while std::time::Instant::now() < until {
        app.update();
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn prompts(app: &App) -> usize {
    app.world().resource::<ExternalSceneChanges>().prompts.len()
}

#[test]
fn the_watcher_does_not_read_our_own_write_as_an_outside_edit() {
    let mut project = project();
    run_watch_frames(&mut project.app, 100);
    let hold = hold_saves_after_writing(project.app.world_mut());
    project.save_as_user();
    run_watch_frames(&mut project.app, 600);
    assert!(
        project.on_disk().contains("Props"),
        "the rename has happened"
    );
    assert_eq!(prompts(&project.app), 0, "while the hash is not noted yet");

    drop(hold);
    project.settle();
    run_watch_frames(&mut project.app, 400);
    assert_eq!(prompts(&project.app), 0);
}

#[test]
fn an_outside_edit_landing_during_a_save_is_still_offered() {
    let mut project = project();
    run_watch_frames(&mut project.app, 100);
    let hold = hold_saves_after_writing(project.app.world_mut());
    project.save_as_user();
    let scene = project.scene.clone();
    util::update_until(
        &mut project.app,
        "the rename",
        Duration::from_secs(10),
        move |_| std::fs::read_to_string(&scene).is_ok_and(|text| text.contains("Props")),
    );
    std::fs::write(&project.scene, YARD).expect("the outside edit");
    run_watch_frames(&mut project.app, 400);
    assert_eq!(
        prompts(&project.app),
        0,
        "the save still has to say what it wrote"
    );
    drop(hold);
    project.settle();
    run_watch_frames(&mut project.app, 400);
    assert_eq!(prompts(&project.app), 1, "the outside bytes are not ours");
}

#[test]
fn an_outside_edit_before_the_rename_is_kept_and_offered() {
    let mut project = project();
    run_watch_frames(&mut project.app, 100);
    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    std::fs::write(&project.scene, YARD).expect("the outside edit");
    drop(hold);
    project.settle();

    assert_eq!(project.on_disk(), YARD, "the outside edit stands");
    assert_eq!(
        std::fs::read_to_string(project.asset("zone.bsn.outside.bak")).expect("the copy"),
        YARD,
        "and a copy outlives a later Keep Mine"
    );
    assert!(project.active_dirty(), "the edits here reached no file");
    assert!(project.notice().contains("outside"), "{}", project.notice());
    run_watch_frames(&mut project.app, 400);
    assert_eq!(prompts(&project.app), 1);

    jackdaw::scenes::external_watch::answer_external_change(
        project.app.world_mut(),
        &project.scene,
        jackdaw::scenes::external_watch::ExternalReloadChoice::Keep,
    );
    assert!(project.active_dirty(), "the file holds the outside bytes");
    undo(&mut project.app);
    assert!(
        project.active_dirty(),
        "and no state on the history is what the file holds"
    );
    assert!(jackdaw::scene_io::is_scene_dirty(project.app.world()));
    redo(&mut project.app);

    project.save_as_user();
    project.settle();
    assert!(project.on_disk().contains("Props"), "Keep Mine then saves");
    assert!(!project.active_dirty());
}

fn answer(project: &mut Project, choice: jackdaw::scenes::external_watch::ExternalReloadChoice) {
    let scene = project.scene.clone();
    jackdaw::scenes::external_watch::answer_external_change(
        project.app.world_mut(),
        &scene,
        choice,
    );
    project.app.update();
}

/// Save in the background and have an outside edit reach the file just ahead of
/// the rename, after the first look at it, with the sidecars written.
fn fail_at_the_rename(project: &mut Project) {
    run_watch_frames(&mut project.app, 100);
    let hold = hold_saves_before_renaming(project.app.world_mut());
    project.save_as_user();
    let sidecar = project.asset("zone.log-1");
    util::update_until(
        &mut project.app,
        "the sidecars",
        Duration::from_secs(10),
        move |_| sidecar.exists(),
    );
    std::fs::write(&project.scene, YARD).expect("the outside edit");
    drop(hold);
    project.settle();
}

#[test]
fn an_outside_edit_after_the_first_look_is_still_kept_and_offered() {
    let (mut project, log) = logged_project();
    fail_at_the_rename(&mut project);
    assert_eq!(project.on_disk(), YARD, "the outside edit stands");
    assert_eq!(
        std::fs::read_to_string(project.asset("zone.bsn.outside.bak")).expect("the copy"),
        YARD,
    );
    assert_eq!(*log.0.lock().expect("log"), ["encode", "finish false"]);
    assert!(project.active_dirty());
    assert!(project.notice().contains("outside"), "{}", project.notice());
    run_watch_frames(&mut project.app, 400);
    assert_eq!(prompts(&project.app), 1);
}

#[test]
fn a_failure_after_the_sidecars_landed_leaves_no_state_reading_saved() {
    let (mut project, _log) = logged_project();
    fail_at_the_rename(&mut project);
    assert!(project.active_dirty());
    undo(&mut project.app);
    assert!(
        project.active_dirty(),
        "the document is as it was opened but the sidecars are not"
    );
}

#[test]
fn a_write_that_dies_after_the_sidecars_landed_leaves_no_state_reading_saved() {
    let (mut project, _log) = logged_project();
    let hold = hold_saves_before_renaming(project.app.world_mut());
    project.save_as_user();
    let sidecar = project.asset("zone.log-1");
    util::update_until(
        &mut project.app,
        "the sidecars",
        Duration::from_secs(10),
        move |_| sidecar.exists(),
    );
    hold.stop_the_writes();
    project.settle();
    assert_eq!(project.on_disk(), ZONE);
    undo(&mut project.app);
    assert!(project.active_dirty());
}

#[test]
fn a_failure_before_any_sidecar_keeps_the_saved_state() {
    let (mut project, log) = logged_project();
    add_unreadable_root(&mut project.app);
    project.save_as_user();
    project.settle();
    assert_eq!(*log.0.lock().expect("log"), ["encode", "finish false"]);
    let node = project
        .app
        .world()
        .resource::<jackdaw_bsn::SceneBsnAst>()
        .roots
        .last()
        .copied()
        .expect("the unreadable root");
    remove_root(&mut project.app, node);
    undo(&mut project.app);
    assert!(!project.active_dirty(), "nothing on disk moved");
}

#[test]
fn reloading_after_a_failed_save_takes_the_file_and_drops_its_copy() {
    let mut project = project();
    run_watch_frames(&mut project.app, 100);
    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    std::fs::write(&project.scene, YARD).expect("the outside edit");
    drop(hold);
    project.settle();
    run_watch_frames(&mut project.app, 400);
    assert_eq!(prompts(&project.app), 1);

    answer(
        &mut project,
        jackdaw::scenes::external_watch::ExternalReloadChoice::Reload,
    );
    assert!(!project.active_dirty(), "the tab holds the file");
    assert_eq!(project.on_disk(), YARD);
    assert!(
        !project.asset("zone.bsn.outside.bak").exists(),
        "the copy says nothing the file does not"
    );
    project.save_as_user();
    project.settle();
    assert!(project.on_disk().contains("Yard"), "{}", project.on_disk());
    assert!(!project.on_disk().contains("Props"));
}

#[test]
fn reloading_keeps_a_copy_that_differs_from_the_file() {
    let mut project = project();
    run_watch_frames(&mut project.app, 100);
    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    std::fs::write(&project.scene, YARD).expect("the outside edit");
    drop(hold);
    project.settle();
    run_watch_frames(&mut project.app, 400);
    std::fs::write(&project.scene, ZONE).expect("a second outside edit");

    answer(
        &mut project,
        jackdaw::scenes::external_watch::ExternalReloadChoice::Reload,
    );
    assert_eq!(
        std::fs::read_to_string(project.asset("zone.bsn.outside.bak")).expect("the copy"),
        YARD,
    );
}

#[test]
fn a_blocking_save_leaves_an_unanswered_outside_edit_standing() {
    let mut project = project();
    run_watch_frames(&mut project.app, 100);
    std::fs::write(&project.scene, YARD).expect("the outside edit");

    assert!(!jackdaw::scene_io::save_scene(project.app.world_mut()));

    assert_eq!(project.on_disk(), YARD, "the outside edit stands");
    assert_eq!(
        std::fs::read_to_string(project.asset("zone.bsn.outside.bak")).expect("the copy"),
        YARD,
    );
    assert!(project.active_dirty());
    assert!(project.notice().contains("outside"), "{}", project.notice());
}

#[test]
fn saving_all_leaves_an_outside_edit_of_a_tab_behind_standing() {
    let mut project = project();
    let (_zone, yard) = two_edited_tabs(&mut project);
    jackdaw::scenes::swap::swap_active_tab(project.app.world_mut(), yard);
    project.app.update();
    run_watch_frames(&mut project.app, 100);
    std::fs::write(&project.scene, YARD).expect("the outside edit of the tab behind");

    run_op_clause_as_user(project.app.world_mut(), "scene.save_all")
        .expect("the save dispatched")
        .assert_finished();
    project.settle();

    assert_eq!(project.on_disk(), YARD, "the outside edit stands");
    assert!(project.asset("zone.bsn.outside.bak").exists());
}

#[test]
fn scene_save_tells_its_caller_it_left_an_outside_edit_standing() {
    let mut project = project();
    run_watch_frames(&mut project.app, 100);
    std::fs::write(&project.scene, YARD).expect("the outside edit");
    let result = run_op_clause(project.app.world_mut(), "scene.save").expect("dispatched");
    assert_eq!(result, OperatorResult::Cancelled);
    assert_eq!(project.on_disk(), YARD);
}

#[cfg(unix)]
#[test]
fn a_file_that_cannot_be_read_is_not_overwritten() {
    use std::os::unix::fs::PermissionsExt as _;
    let mut project = project();
    run_watch_frames(&mut project.app, 100);
    let unreadable = std::fs::Permissions::from_mode(0o000);
    std::fs::set_permissions(&project.scene, unreadable).expect("permissions");
    if std::fs::read(&project.scene).is_ok() {
        return;
    }

    project.save_as_user();
    project.settle();

    let readable = std::fs::Permissions::from_mode(0o644);
    std::fs::set_permissions(&project.scene, readable).expect("permissions");
    assert_eq!(project.on_disk(), ZONE, "whatever it holds, it stands");
    assert!(project.active_dirty());
    assert!(
        project.notice().contains("Not saved"),
        "{}",
        project.notice()
    );
}

#[test]
fn a_missing_file_is_written() {
    let mut project = project();
    run_watch_frames(&mut project.app, 100);
    std::fs::remove_file(&project.scene).expect("removed");
    project.save_as_user();
    project.settle();
    assert!(project.on_disk().contains("Props"));
}

#[test]
fn what_waits_on_a_save_holds_while_a_modal_runs() {
    let mut project = project();
    let (_zone, yard) = two_edited_tabs(&mut project);
    let hold = hold_saves_before_writing(project.app.world_mut());
    jackdaw::scenes::operators::scene_close_system(project.app.world_mut(), yard);
    jackdaw::scenes::confirm_dialog::apply_confirm_dialog_action(
        project.app.world_mut(),
        jackdaw::scenes::confirm_dialog::ConfirmDialogButton::Save,
        yard,
    );
    let started = project
        .app
        .world_mut()
        .operator(jackdaw::draw_brush::ActivateDrawBrushModalOp::ID)
        .call()
        .expect("dispatches");
    assert_eq!(started, OperatorResult::Running);

    drop(hold);
    project.settle();
    for _ in 0..3 {
        project.app.update();
    }
    assert_eq!(
        project.app.world().resource::<Scenes>().tabs.len(),
        2,
        "the tab does not go out from under the modal"
    );
    assert!(jackdaw::scene_io::saves_in_flight(project.app.world()));

    project
        .app
        .world_mut()
        .cancel_active_modal()
        .expect("cancels");
    for _ in 0..3 {
        project.app.update();
    }
    assert_eq!(project.app.world().resource::<Scenes>().tabs.len(), 1);
    assert!(!jackdaw::scene_io::saves_in_flight(project.app.world()));
}

#[test]
fn closing_a_tab_unprompted_waits_for_its_write() {
    let mut project = project();
    let (_zone, yard) = two_edited_tabs(&mut project);
    let hold = hold_saves_before_writing(project.app.world_mut());
    let on_disk_at_close = FrontAtFinish::default();
    project
        .app
        .world_mut()
        .resource_mut::<DocumentIoHooks>()
        .register(on_disk_at_close.clone());
    project.save_as_user();
    release_soon(hold);
    jackdaw::scenes::operators::scene_close_system_unprompted(project.app.world_mut(), yard);
    assert!(!jackdaw::scene_io::saves_in_flight(project.app.world()));
    assert_eq!(project.app.world().resource::<Scenes>().tabs.len(), 1);
    assert_eq!(
        on_disk_at_close
            .0
            .lock()
            .expect("log")
            .last()
            .map(String::as_str),
        Some("yard"),
        "the save ended with its tab in front"
    );
}

/// Logs which tab is in front as each save ends.
#[derive(Clone, Default)]
struct FrontAtFinish(Arc<Mutex<Vec<String>>>);

impl DocumentIoHook for FrontAtFinish {
    fn begin_save(&self, _: &mut World, _: &str) -> Box<dyn Any + Send> {
        Box::new(())
    }

    fn encode_sidecars(
        &self,
        _: &mut World,
        _: &str,
        _: &mut (dyn Any + Send),
    ) -> Result<Vec<SidecarWrite>, BevyError> {
        Ok(Vec::new())
    }

    fn finish_save(&self, world: &mut World, _: Box<dyn Any + Send>, _: bool) {
        let scenes = world.resource::<Scenes>();
        let front = scenes
            .tabs
            .get(scenes.active)
            .map(|tab| tab.display_name.clone());
        self.0.lock().expect("log").push(front.unwrap_or_default());
    }

    fn import_sidecars(&self, _: &mut World, _: &str, _: SidecarImport) {}
}

fn add_named_root(app: &mut App, name: &str) {
    let mut ast = app.world_mut().resource_mut::<jackdaw_bsn::SceneBsnAst>();
    let node = ast.create_entity_node(vec![
        jackdaw_bsn::BsnPatch::Name(name.to_string()),
        jackdaw_bsn::BsnPatch::Type("bevy_transform::components::transform::Transform".to_string()),
    ]);
    ast.add_to_roots(node);
}

#[test]
fn a_second_save_is_not_skipped_for_a_document_change_outside_the_history() {
    let mut project = project();
    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    project.app.update();
    add_named_root(&mut project.app, "Unrecorded");
    project.app.update();
    project.save_as_user();
    drop(hold);
    project.settle();
    assert!(
        project.on_disk().contains("Unrecorded"),
        "{}",
        project.on_disk()
    );
    assert!(!project.active_dirty());
}

#[test]
fn a_document_change_outside_the_history_during_a_write_does_not_settle_clean() {
    let mut project = project();
    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    project.app.update();
    add_named_root(&mut project.app, "Unrecorded");
    drop(hold);
    project.settle();
    assert!(!project.on_disk().contains("Unrecorded"));
    assert!(project.active_dirty(), "the file does not hold it");
}

#[test]
fn frames_during_a_write_do_not_read_as_a_change_to_the_document() {
    let mut project = project();
    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    run_watch_frames(&mut project.app, 300);
    drop(hold);
    project.settle();
    assert!(!project.active_dirty());
}

#[test]
fn a_background_save_writes_the_bytes_a_blocking_save_writes() {
    let (mut app, _tmp, scene) = save_baseline::fixture_project();
    run_op_clause_as_user(app.world_mut(), "scene.save")
        .expect("the save dispatched")
        .assert_finished();
    wait_for_saves(app.world_mut());
    let background = std::fs::read_to_string(&scene).expect("the file");
    let sidecar = scene.with_file_name(save_baseline::TERRAIN_FILE);
    let background_sidecar = std::fs::read(&sidecar).expect("the sidecar");
    assert_eq!(
        save_baseline::entity_blocks(&background),
        save_baseline::entity_blocks(&save_baseline::recorded()),
    );

    assert!(jackdaw::scene_io::save_scene(app.world_mut()));
    assert_eq!(
        background,
        std::fs::read_to_string(&scene).expect("the file")
    );
    assert_eq!(
        background_sidecar,
        std::fs::read(&sidecar).expect("the sidecar")
    );
}

#[test]
fn save_bytes_are_stable_across_saves() {
    let (mut app, _tmp, scene) = save_baseline::fixture_project();
    let save = |app: &mut App| {
        run_op_clause_as_user(app.world_mut(), "scene.save")
            .expect("the save dispatched")
            .assert_finished();
        wait_for_saves(app.world_mut());
        app.update();
        std::fs::read(&scene).expect("the file")
    };
    let first = save(&mut app);
    assert_eq!(first, save(&mut app));
    assert_eq!(first, save(&mut app));
}

#[test]
fn a_tab_switch_during_a_save_waits_and_the_save_lands_on_its_own_tab() {
    let mut project = project();
    let yard_file = project.asset("yard.bsn");
    jackdaw::scenes::operators::scene_open_system(project.app.world_mut(), &yard_file);
    project.app.update();
    let scenes = project.app.world().resource::<Scenes>();
    let (yard, zone) = (
        scenes.active,
        scenes
            .tabs
            .iter()
            .position(|tab| tab.display_name == "zone")
            .expect("zone"),
    );
    jackdaw::scenes::swap::swap_active_tab(project.app.world_mut(), zone);
    project.app.update();

    #[derive(Resource, Default)]
    struct FrontAtLanding(Vec<usize>);
    project.app.world_mut().init_resource::<FrontAtLanding>();
    project.app.world_mut().add_observer(
        |_: On<SceneSaved>, scenes: Res<Scenes>, mut front: ResMut<FrontAtLanding>| {
            front.0.push(scenes.active);
        },
    );

    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    release_soon(hold);
    assert!(jackdaw::scenes::swap::swap_active_tab(
        project.app.world_mut(),
        yard
    ));
    assert!(
        !jackdaw::scene_io::saves_in_flight(project.app.world()),
        "the switch waited for the save"
    );
    assert_eq!(
        project.app.world().resource::<FrontAtLanding>().0,
        [zone],
        "which landed with its own tab in front"
    );
    project.settle();

    assert!(project.on_disk().contains("Props"));
    let scenes = project.app.world().resource::<Scenes>();
    assert!(!scenes.tabs[zone].dirty, "the saved tab reads saved");
    assert_eq!(
        std::fs::read_to_string(project.asset("yard.bsn")).expect("yard"),
        YARD,
        "and the tab now in front was not written"
    );
}

#[test]
fn play_starts_without_waiting_on_a_save() {
    let mut project = project();
    jackdaw::run_config::read_run_configs(project.app.world_mut());
    jackdaw::pie::insert_build_in_progress(project.app.world_mut());
    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    play(&mut project.app);
    assert_eq!(
        jackdaw::pie::launches_waiting_on_build(project.app.world()).len(),
        1,
        "the game starts while the save is held"
    );
    drop(hold);
    project.settle();
    let saved = project.on_disk();
    assert!(saved.contains("Props"), "{saved}");
    assert!(jackdaw_bsn::parse_bsn_text(&saved).is_ok());
    assert!(!project.active_dirty());
}

#[test]
fn the_blocking_operator_call_has_the_file_on_return_even_during_a_save() {
    let mut project = project();
    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    project.edit("Later");
    release_soon(hold);

    project
        .app
        .world_mut()
        .operator("scene.save")
        .call()
        .expect("dispatch")
        .assert_finished();

    assert!(
        project.on_disk().contains("Later"),
        "as remote tools rely on"
    );
    assert!(!project.active_dirty());
}

/// A second scene open in front of the project's, with an edit of its own, and
/// the indexes of the two tabs.
fn two_edited_tabs(project: &mut Project) -> (usize, usize) {
    let yard_file = project.asset("yard.bsn");
    jackdaw::scenes::operators::scene_open_system(project.app.world_mut(), &yard_file);
    project.app.update();
    run_op_clause_as_user(
        project.app.world_mut(),
        "entity.add.group name=Sheds parent=Yard",
    )
    .expect("the group dispatched")
    .assert_finished();
    project.app.update();
    let scenes = project.app.world().resource::<Scenes>();
    let zone = scenes
        .tabs
        .iter()
        .position(|tab| tab.display_name == "zone")
        .expect("zone");
    (zone, scenes.active)
}

/// Run frames until the saves are over, and say which tabs were in front after each.
fn fronts_until_settled(app: &mut App) -> Vec<usize> {
    let mut fronts = Vec::new();
    for _ in 0..2000 {
        app.update();
        fronts.push(app.world().resource::<Scenes>().active);
        if !jackdaw::scene_io::saves_in_flight(app.world()) {
            return fronts;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("the saves never ended");
}

#[test]
fn save_all_as_the_user_writes_every_tab_and_never_leaves_the_front_one() {
    let mut project = project();
    let (zone, yard) = two_edited_tabs(&mut project);

    run_op_clause_as_user(project.app.world_mut(), "scene.save_all")
        .expect("dispatch")
        .assert_finished();
    let fronts = fronts_until_settled(&mut project.app);

    assert!(fronts.iter().all(|front| *front == yard), "{fronts:?}");
    assert!(project.on_disk().contains("Props"), "the tab behind");
    assert!(
        std::fs::read_to_string(project.asset("yard.bsn"))
            .expect("yard")
            .contains("Sheds"),
        "and the tab in front"
    );
    let scenes = project.app.world().resource::<Scenes>();
    assert!(!scenes.tabs[zone].dirty && !scenes.tabs[yard].dirty);
}

#[test]
fn save_all_writes_the_tabs_behind_at_once_and_the_front_one_in_the_background() {
    let mut project = project();
    two_edited_tabs(&mut project);
    let hold = hold_saves_before_writing(project.app.world_mut());

    run_op_clause_as_user(project.app.world_mut(), "scene.save_all")
        .expect("dispatch")
        .assert_finished();
    project.app.update();

    assert!(project.on_disk().contains("Props"), "the tab behind landed");
    assert!(
        !std::fs::read_to_string(project.asset("yard.bsn"))
            .expect("yard")
            .contains("Sheds"),
        "the one in front is on its way"
    );
    assert!(jackdaw::scene_io::saves_in_flight(project.app.world()));
    drop(hold);
    project.settle();
    assert!(
        std::fs::read_to_string(project.asset("yard.bsn"))
            .expect("yard")
            .contains("Sheds")
    );
}

#[test]
fn a_failure_in_save_all_is_told_and_the_other_tabs_still_save() {
    let mut project = project();
    let (zone, yard) = two_edited_tabs(&mut project);
    jackdaw::scenes::swap::swap_active_tab(project.app.world_mut(), zone);
    add_unreadable_root(&mut project.app);
    jackdaw::scenes::swap::swap_active_tab(project.app.world_mut(), yard);

    run_op_clause_as_user(project.app.world_mut(), "scene.save_all")
        .expect("dispatch")
        .assert_finished();
    assert!(
        project.notice().contains("Not saved"),
        "{}",
        project.notice()
    );
    fronts_until_settled(&mut project.app);

    assert_eq!(project.on_disk(), ZONE, "its old file stands");
    assert!(
        std::fs::read_to_string(project.asset("yard.bsn"))
            .expect("yard")
            .contains("Sheds")
    );
    let scenes = project.app.world().resource::<Scenes>();
    assert!(scenes.tabs[zone].dirty);
    assert!(!scenes.tabs[yard].dirty);
}

#[cfg(unix)]
#[test]
fn a_read_only_folder_fails_the_save_loudly_and_leaves_the_file() {
    use std::os::unix::fs::PermissionsExt as _;

    let mut project = project();
    let folder = project.tmp.path().join("assets");
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).expect("read only");
    project.save_as_user();
    project.settle();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).expect("writable");

    assert_eq!(project.on_disk(), ZONE);
    assert!(project.active_dirty());
    assert!(
        project.notice().contains("Not saved"),
        "{}",
        project.notice()
    );
}

const TILEMAP_TYPE_PATH: &str = "jackdaw_scene_types::tilemap::Tilemap";
const TILES_FILE: &str = "zone.tilemap-0.jdtiles";

/// A square layer of `tile` over `side` cells.
fn painted_map(side: i32, tile: u16) -> jackdaw_tilemap::TilemapData {
    use jackdaw_tilemap::{CellCoord, ChunkedLayer, LayerData, LayerKind, TileRef};
    let mut tiles = ChunkedLayer::new();
    for y in 0..side {
        for x in 0..side {
            tiles.set(CellCoord::new(x, y), TileRef::new(tile));
        }
    }
    jackdaw_tilemap::TilemapData {
        layers: vec![LayerData {
            kind: LayerKind::Tile,
            placement: Default::default(),
            tiles,
            authoring: None,
        }],
    }
}

fn map_entity(app: &mut App) -> Entity {
    app.world_mut()
        .query_filtered::<Entity, With<jackdaw_scene_types::Tilemap>>()
        .single(app.world())
        .expect("one map")
}

fn put_map(app: &mut App, data: jackdaw_tilemap::TilemapData) {
    let entity = map_entity(app);
    let key = jackdaw_editor_tile::tilemap::store_key_of(app.world_mut(), entity)
        .expect("a valid data path");
    app.world_mut()
        .resource_mut::<jackdaw_editor_tile::tilemap::TilemapDataStore>()
        .insert(key, data);
}

fn live_map_cells(app: &mut App) -> Option<jackdaw_tilemap::TilemapData> {
    let entity = map_entity(app);
    let key = jackdaw_editor_tile::tilemap::store_key_of(app.world_mut(), entity)?;
    app.world()
        .resource::<jackdaw_editor_tile::tilemap::TilemapDataStore>()
        .get(&key)
        .cloned()
}

/// The project with a tilemap in its scene, saved to its sidecar.
fn project_with_a_map() -> Project {
    let mut project = project();
    let tilemap = jackdaw_scene_types::Tilemap {
        data_path: TILES_FILE.to_string(),
        layers: vec![jackdaw_scene_types::TilemapLayer {
            name: "Ground".to_string(),
            tileset: "tiles/overworld.tileset.bsn".to_string(),
            kind: jackdaw_scene_types::TilemapLayerKind::Tile,
            ..Default::default()
        }],
        ..Default::default()
    };
    let entity = project
        .app
        .world_mut()
        .spawn((tilemap.clone(), Name::new("map")))
        .id();
    let mut ast = project
        .app
        .world_mut()
        .resource_mut::<jackdaw_bsn::SceneBsnAst>();
    let node = ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::Name("map".to_string())]);
    ast.add_to_roots(node);
    ast.link(entity, node);
    jackdaw::commands::sync_component_to_ast(
        project.app.world_mut(),
        entity,
        TILEMAP_TYPE_PATH,
        &tilemap,
    );
    put_map(&mut project.app, painted_map(4, 7));
    project.save_as_user();
    project.settle();
    assert!(project.asset(TILES_FILE).is_file());
    project
}

#[test]
fn a_failed_validation_leaves_the_tile_sidecar_alone_and_the_map_unsaved() {
    let mut project = project_with_a_map();
    let before = std::fs::read(project.asset(TILES_FILE)).expect("sidecar");

    put_map(&mut project.app, painted_map(8, 9));
    let unreadable = add_unreadable_root(&mut project.app);
    project.save_as_user();
    project.settle();
    assert_eq!(
        before,
        std::fs::read(project.asset(TILES_FILE)).expect("sidecar"),
        "the sidecar still goes with the document on disk"
    );

    remove_root(&mut project.app, unreadable);
    project.save_as_user();
    project.settle();
    assert_eq!(
        jackdaw_tilemap::decode(&std::fs::read(project.asset(TILES_FILE)).expect("sidecar"))
            .expect("decodes"),
        painted_map(8, 9),
        "the next save writes it"
    );
}

#[test]
fn a_sidecar_waits_for_the_read_back_of_its_document() {
    let mut project = project_with_a_map();
    let before = std::fs::read(project.asset(TILES_FILE)).expect("sidecar");
    put_map(&mut project.app, painted_map(8, 9));

    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    project.app.update();
    assert_eq!(
        before,
        std::fs::read(project.asset(TILES_FILE)).expect("sidecar"),
    );
    drop(hold);
    project.settle();
    assert_ne!(
        before,
        std::fs::read(project.asset(TILES_FILE)).expect("sidecar"),
    );
}

#[test]
fn a_map_edited_during_its_save_stays_unsaved() {
    let mut project = project_with_a_map();
    put_map(&mut project.app, painted_map(8, 9));
    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    put_map(&mut project.app, painted_map(16, 3));
    drop(hold);
    project.settle();
    assert_eq!(
        jackdaw_tilemap::decode(&std::fs::read(project.asset(TILES_FILE)).expect("sidecar"))
            .expect("decodes"),
        painted_map(8, 9),
        "as of the snapshot"
    );

    project.save_as_user();
    project.settle();
    assert_eq!(
        jackdaw_tilemap::decode(&std::fs::read(project.asset(TILES_FILE)).expect("sidecar"))
            .expect("decodes"),
        painted_map(16, 3),
        "the later edit goes out with the next save"
    );
}

/// A terrain linked into the project's scene, with cells in its sidecar.
const TERRAIN_FILE: &str = "zone.terrain-0.jdterrain";

fn terrain_cells(scale: f32) -> jackdaw_terrain::RegionTerrainData {
    jackdaw_terrain::RegionTerrainData::from_legacy_v1(&jackdaw_terrain::TerrainData {
        resolution: 4,
        heights: (0..16).map(|cell| cell as f32 * scale).collect(),
        channels: vec![],
    })
    .expect("a power-of-two resolution migrates")
}

fn project_with_a_terrain() -> Project {
    use jackdaw::terrain_mode::TerrainDataStore;
    let mut project = project();
    let terrain = jackdaw_scene_types::Terrain {
        resolution: 4,
        data_path: TERRAIN_FILE.to_string(),
        ..default()
    };
    project
        .app
        .world_mut()
        .resource_mut::<TerrainDataStore>()
        .insert(TERRAIN_FILE.to_string(), terrain_cells(0.5));
    let entity = project
        .app
        .world_mut()
        .spawn((terrain.clone(), Name::new("ground")))
        .id();
    let mut ast = project
        .app
        .world_mut()
        .resource_mut::<jackdaw_bsn::SceneBsnAst>();
    let node = ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::Name("ground".to_string())]);
    ast.add_to_roots(node);
    ast.link(entity, node);
    jackdaw::commands::sync_component_to_ast(
        project.app.world_mut(),
        entity,
        "jackdaw_scene_types::types::Terrain",
        &terrain,
    );
    project.save_as_user();
    project.settle();
    assert!(project.asset(TERRAIN_FILE).is_file());
    project
}

#[test]
fn a_failed_validation_leaves_the_terrain_sidecar_alone() {
    use jackdaw::terrain_mode::TerrainDataStore;
    let mut project = project_with_a_terrain();
    let before = std::fs::read(project.asset(TERRAIN_FILE)).expect("sidecar");

    project
        .app
        .world_mut()
        .resource_mut::<TerrainDataStore>()
        .insert(TERRAIN_FILE.to_string(), terrain_cells(2.0));
    add_unreadable_root(&mut project.app);
    project.save_as_user();
    project.settle();

    assert_eq!(
        before,
        std::fs::read(project.asset(TERRAIN_FILE)).expect("sidecar"),
    );
    assert!(
        project
            .app
            .world()
            .resource::<TerrainDataStore>()
            .has_unsaved_edits(TERRAIN_FILE),
        "and the edit is still unsaved"
    );
}

#[test]
fn a_terrain_edited_during_its_save_stays_unsaved() {
    use jackdaw::terrain_mode::TerrainDataStore;
    let mut project = project_with_a_terrain();
    project
        .app
        .world_mut()
        .resource_mut::<TerrainDataStore>()
        .insert(TERRAIN_FILE.to_string(), terrain_cells(2.0));
    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    project
        .app
        .world_mut()
        .resource_mut::<TerrainDataStore>()
        .insert(TERRAIN_FILE.to_string(), terrain_cells(3.0));
    drop(hold);
    project.settle();

    assert!(
        project
            .app
            .world()
            .resource::<TerrainDataStore>()
            .has_unsaved_edits(TERRAIN_FILE),
        "the file holds the snapshot, not this edit"
    );
}

/// The saved document names the data path the live map has, and that sidecar holds
/// its cells.
fn assert_map_is_where_its_file_says(project: &mut Project, scene: &str) {
    let entity = map_entity(&mut project.app);
    let data_path = project
        .app
        .world()
        .get::<jackdaw_scene_types::Tilemap>(entity)
        .expect("a map")
        .data_path
        .clone();
    let saved = std::fs::read_to_string(project.asset(scene)).expect("the file");
    assert!(saved.contains(&data_path), "{data_path} in:\n{saved}");
    assert_eq!(
        jackdaw_tilemap::decode(&std::fs::read(project.asset(&data_path)).expect("sidecar"))
            .expect("decodes"),
        painted_map(4, 7),
    );
}

#[test]
fn a_save_as_of_a_map_keeps_its_cells_in_view_with_frames_running() {
    let mut project = project_with_a_map();
    let hold = hold_saves_before_writing(project.app.world_mut());

    run_op_clause_as_user(
        project.app.world_mut(),
        "scene.save path=assets/elsewhere.bsn",
    )
    .expect("the save dispatched")
    .assert_finished();
    project.app.update();
    project.app.update();

    assert_eq!(
        live_map_cells(&mut project.app),
        Some(painted_map(4, 7)),
        "the map has its cells wherever its key now is"
    );
    drop(hold);
    project.settle();
    assert_eq!(live_map_cells(&mut project.app), Some(painted_map(4, 7)));
    assert_map_is_where_its_file_says(&mut project, "elsewhere.bsn");
}

#[test]
fn the_first_save_of_an_untitled_map_keeps_its_cells_in_view_with_frames_running() {
    let mut project = project();
    run_op_clause(project.app.world_mut(), "scene.new")
        .expect("a new scene")
        .assert_finished();
    project.app.update();
    let tilemap = jackdaw_scene_types::Tilemap {
        layers: vec![jackdaw_scene_types::TilemapLayer {
            name: "Ground".to_string(),
            tileset: "tiles/overworld.tileset.bsn".to_string(),
            kind: jackdaw_scene_types::TilemapLayerKind::Tile,
            ..Default::default()
        }],
        ..Default::default()
    };
    let entity = project
        .app
        .world_mut()
        .spawn((tilemap.clone(), Name::new("map")))
        .id();
    let mut ast = project
        .app
        .world_mut()
        .resource_mut::<jackdaw_bsn::SceneBsnAst>();
    let node = ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::Name("map".to_string())]);
    ast.add_to_roots(node);
    ast.link(entity, node);
    jackdaw::commands::sync_component_to_ast(
        project.app.world_mut(),
        entity,
        TILEMAP_TYPE_PATH,
        &tilemap,
    );
    project.app.update();
    put_map(&mut project.app, painted_map(4, 7));

    let hold = hold_saves_before_writing(project.app.world_mut());
    run_op_clause_as_user(project.app.world_mut(), "scene.save path=assets/fresh.bsn")
        .expect("the save dispatched")
        .assert_finished();
    project.app.update();
    project.app.update();
    assert_eq!(live_map_cells(&mut project.app), Some(painted_map(4, 7)));
    drop(hold);
    project.settle();
    assert_eq!(live_map_cells(&mut project.app), Some(painted_map(4, 7)));
    assert_map_is_where_its_file_says(&mut project, "fresh.bsn");
}

/// What rewrites a scene file waits for the save writing it, so the save cannot
/// land over the rewrite.
#[test]
fn a_migration_during_a_save_waits_for_it() {
    let mut project = project();
    project.save_as_user();
    project.settle();

    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    release_soon(hold);
    run_op_clause(project.app.world_mut(), "project.migrate_asset_references")
        .expect("dispatch")
        .assert_finished();
    project.app.update();
    assert!(
        !jackdaw::scene_io::saves_in_flight(project.app.world()),
        "the save is over by the time the files are rewritten"
    );
}

#[test]
fn deleting_a_scene_during_its_save_leaves_it_deleted() {
    let mut project = project();
    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    release_soon(hold);
    let path = project.scene.to_string_lossy().into_owned();
    project
        .app
        .world_mut()
        .operator("file.delete")
        .param("path", path)
        .param("force", true)
        .call()
        .expect("dispatch")
        .assert_finished();
    project.app.update();
    project.settle();
    assert!(!project.scene.exists(), "the save did not bring it back");
}

#[test]
fn saving_as_a_prefab_over_the_file_being_saved_leaves_the_prefab() {
    let mut project = project();
    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    release_soon(hold);
    let scene = project.scene.clone();
    jackdaw::prefab::operators::save_scene_as_prefab(project.app.world_mut(), &scene);
    project.settle();
    let saved = project.on_disk();
    assert!(saved.contains("components::Prefab"), "{saved}");
}

#[test]
fn renaming_a_scene_during_its_save_leaves_no_file_at_the_old_name() {
    let mut project = project();
    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    release_soon(hold);
    let (old, new) = (project.scene.clone(), project.asset("moved.bsn"));
    project
        .app
        .world_mut()
        .operator("file.rename")
        .param("path", old.to_string_lossy().into_owned())
        .param("name", "moved.bsn")
        .call()
        .expect("dispatch")
        .assert_finished();
    project.app.update();
    project.settle();
    assert!(new.is_file());
    assert!(!old.exists(), "the save did not bring it back");
}

fn undo(app: &mut App) {
    run_op_clause(app.world_mut(), "history.undo")
        .expect("the undo dispatched")
        .assert_finished();
    app.update();
}

fn redo(app: &mut App) {
    run_op_clause(app.world_mut(), "history.redo")
        .expect("the redo dispatched")
        .assert_finished();
    app.update();
}

#[test]
fn undoing_a_saved_edit_reads_dirty_and_redoing_it_reads_clean() {
    let mut project = project();
    let yard_file = project.asset("yard.bsn");
    jackdaw::scenes::operators::scene_open_system(project.app.world_mut(), &yard_file);
    project.app.update();
    let zone_tab = project
        .app
        .world()
        .resource::<Scenes>()
        .tabs
        .iter()
        .position(|tab| tab.display_name == "zone")
        .expect("zone");
    jackdaw::scenes::swap::swap_active_tab(project.app.world_mut(), zone_tab);
    project.app.update();
    project.save_as_user();
    project.settle();
    assert!(!project.active_dirty());
    assert!(!jackdaw::scene_io::is_scene_dirty(project.app.world()));

    undo(&mut project.app);
    assert!(project.active_dirty(), "the file holds the undone edit");
    assert!(jackdaw::scene_io::is_scene_dirty(project.app.world()));

    let zone = project.app.world().resource::<Scenes>().active;
    jackdaw::scenes::operators::scene_close_system(project.app.world_mut(), zone);
    assert_eq!(
        project
            .app
            .world()
            .resource::<jackdaw::scenes::confirm_dialog::PendingTabClose>()
            .tab_index,
        Some(zone),
        "closing it asks"
    );
    project
        .app
        .world_mut()
        .resource_mut::<jackdaw::scenes::confirm_dialog::PendingTabClose>()
        .tab_index = None;

    redo(&mut project.app);
    assert!(!project.active_dirty(), "the file holds this state");
    assert!(!jackdaw::scene_io::is_scene_dirty(project.app.world()));
}

#[test]
fn undoing_every_edit_reads_clean_and_undoing_past_a_save_reads_dirty() {
    let mut project = project();
    assert!(project.active_dirty());
    undo(&mut project.app);
    assert!(!project.active_dirty(), "edit, undo: back at the file");

    project.save_as_user();
    project.settle();
    project.edit("Later");
    assert!(project.active_dirty());
    undo(&mut project.app);
    assert!(
        !project.active_dirty(),
        "save, edit, undo: back at the file"
    );

    project.edit("Saved");
    project.save_as_user();
    project.settle();
    undo(&mut project.app);
    assert!(project.active_dirty(), "save, undo past it: not the file");
}

#[test]
fn an_edit_after_undoing_past_a_save_never_reads_clean_again() {
    let mut project = project();
    project.save_as_user();
    project.settle();
    undo(&mut project.app);
    project.edit("Other");
    undo(&mut project.app);
    project.edit("Another");
    assert!(project.active_dirty());
    undo(&mut project.app);
    assert!(project.active_dirty(), "the saved state cannot be reached");
}

#[test]
fn an_undo_during_a_save_reads_dirty_everywhere_and_a_redo_clean() {
    let mut project = project();
    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    undo(&mut project.app);
    drop(hold);
    project.settle();
    assert!(project.active_dirty());
    assert!(
        jackdaw::scene_io::is_scene_dirty(project.app.world()),
        "the guard ahead of a migration agrees with the tab"
    );
    redo(&mut project.app);
    assert!(!project.active_dirty(), "this is the state the file holds");
    assert!(!jackdaw::scene_io::is_scene_dirty(project.app.world()));
}

#[test]
fn a_failure_nobody_waited_for_is_not_held_against_the_next_wait() {
    let mut project = project();
    let (zone, yard) = two_edited_tabs(&mut project);
    jackdaw::scenes::swap::swap_active_tab(project.app.world_mut(), zone);
    add_unreadable_root(&mut project.app);
    project.save_as_user();
    assert!(jackdaw::scenes::swap::swap_active_tab(
        project.app.world_mut(),
        yard
    ));
    assert!(
        project.notice().contains("Not saved"),
        "{}",
        project.notice()
    );

    jackdaw::scenes::operators::scene_close_system(project.app.world_mut(), yard);
    jackdaw::scenes::confirm_dialog::apply_confirm_dialog_action(
        project.app.world_mut(),
        jackdaw::scenes::confirm_dialog::ConfirmDialogButton::Save,
        yard,
    );
    project.settle();
    assert_eq!(
        project.app.world().resource::<Scenes>().tabs.len(),
        1,
        "the yard saved, so it closed"
    );
}

/// Saves again, blocking, the first time a save lands.
#[derive(Default)]
struct SaveWhenALandingFinishes(std::sync::atomic::AtomicBool);

impl DocumentIoHook for SaveWhenALandingFinishes {
    fn begin_save(&self, _: &mut World, _: &str) -> Box<dyn Any + Send> {
        Box::new(())
    }

    fn encode_sidecars(
        &self,
        _: &mut World,
        _: &str,
        _: &mut (dyn Any + Send),
    ) -> Result<Vec<SidecarWrite>, BevyError> {
        Ok(Vec::new())
    }

    fn finish_save(&self, world: &mut World, _: Box<dyn Any + Send>, _: bool) {
        if !self.0.swap(true, std::sync::atomic::Ordering::Relaxed) {
            assert!(jackdaw::scene_io::save_scene(world));
        }
    }

    fn import_sidecars(&self, _: &mut World, _: &str, _: SidecarImport) {}
}

#[test]
fn a_blocking_save_when_a_save_lands_is_not_overwritten_by_the_one_waiting() {
    let mut project = project();
    project
        .app
        .world_mut()
        .resource_mut::<DocumentIoHooks>()
        .register(SaveWhenALandingFinishes::default());
    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    project.edit("Second");
    project.save_as_user();
    project.edit("Third");
    drop(hold);
    project.settle();

    let saved = project.on_disk();
    assert!(
        saved.contains("Third"),
        "the newest state is last:\n{saved}"
    );
    assert!(!project.active_dirty());
}

#[derive(Resource, Default)]
struct SaveCount(usize);

fn count_saves(app: &mut App) {
    app.world_mut().init_resource::<SaveCount>();
    app.world_mut()
        .add_observer(|_: On<SceneSaved>, mut saved: ResMut<SaveCount>| saved.0 += 1);
}

#[test]
fn a_second_save_with_no_edit_writes_nothing_more() {
    let mut project = project();
    count_saves(&mut project.app);
    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    project.save_as_user();
    drop(hold);
    project.settle();
    assert_eq!(project.app.world().resource::<SaveCount>().0, 1);
    assert!(!project.active_dirty());
}

#[test]
fn play_during_a_save_of_the_same_state_writes_nothing_more() {
    let mut project = project();
    jackdaw::run_config::read_run_configs(project.app.world_mut());
    jackdaw::pie::insert_build_in_progress(project.app.world_mut());
    count_saves(&mut project.app);
    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    play(&mut project.app);
    drop(hold);
    project.settle();
    assert_eq!(project.app.world().resource::<SaveCount>().0, 1);
    assert_eq!(
        jackdaw::pie::launches_waiting_on_build(project.app.world()).len(),
        1
    );
}

/// Encodes one file named for each save, and logs what the save does with it.
#[derive(Clone, Default)]
struct SidecarLog(Arc<Mutex<Vec<String>>>);

impl DocumentIoHook for SidecarLog {
    fn begin_save(&self, _: &mut World, _: &str) -> Box<dyn Any + Send> {
        Box::new(())
    }

    fn encode_sidecars(
        &self,
        _: &mut World,
        path: &str,
        _: &mut (dyn Any + Send),
    ) -> Result<Vec<SidecarWrite>, BevyError> {
        let mut log = self.0.lock().expect("log");
        log.push("encode".to_string());
        let number = log.iter().filter(|line| *line == "encode").count();
        let file = PathBuf::from(path).with_extension(format!("log-{number}"));
        Ok(vec![(file, b"bytes".to_vec())])
    }

    fn finish_save(&self, _: &mut World, _: Box<dyn Any + Send>, written: bool) {
        self.0
            .lock()
            .expect("log")
            .push(format!("finish {written}"));
    }

    fn import_sidecars(&self, _: &mut World, _: &str, _: SidecarImport) {}
}

fn logged_project() -> (Project, SidecarLog) {
    let log = SidecarLog::default();
    let mut project = project();
    project
        .app
        .world_mut()
        .resource_mut::<DocumentIoHooks>()
        .register(log.clone());
    (project, log)
}

#[test]
fn sidecars_land_after_the_read_back_and_before_the_document_is_told_of() {
    let (mut project, log) = logged_project();
    let sidecar = project.asset("zone.log-1");
    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    project.app.update();
    assert_eq!(*log.0.lock().expect("log"), ["encode"]);
    assert!(!sidecar.exists(), "nothing has been read back yet");

    drop(hold);
    project.settle();
    assert!(sidecar.exists());
    assert_eq!(*log.0.lock().expect("log"), ["encode", "finish true"]);
}

#[test]
fn sidecars_land_ahead_of_the_rename() {
    let (mut project, log) = logged_project();
    let hold = hold_saves_before_renaming(project.app.world_mut());
    project.save_as_user();
    let sidecar = project.asset("zone.log-1");
    let seen = sidecar.clone();
    util::update_until(
        &mut project.app,
        "the sidecars",
        Duration::from_secs(10),
        move |_| seen.exists(),
    );
    let temporary = jackdaw_api_internal::document::temporary_path(&project.scene).expect("named");
    util::update_until(
        &mut project.app,
        "the document's bytes",
        Duration::from_secs(10),
        move |_| temporary.exists(),
    );
    assert_eq!(project.on_disk(), ZONE, "the document is still the old one");
    assert_eq!(*log.0.lock().expect("log"), ["encode"]);

    drop(hold);
    project.settle();
    assert!(project.on_disk().contains("Props"));
    assert_eq!(*log.0.lock().expect("log"), ["encode", "finish true"]);
}

#[test]
fn sidecars_never_land_for_a_document_that_failed_validation() {
    let (mut project, log) = logged_project();
    add_unreadable_root(&mut project.app);
    project.save_as_user();
    project.settle();
    assert_eq!(*log.0.lock().expect("log"), ["encode", "finish false"]);
    assert!(!project.asset("zone.log-1").exists());
}

#[test]
fn a_replaced_save_writes_no_sidecars() {
    let (mut project, log) = logged_project();
    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    project.edit("Second");
    project.save_as_user();
    project.edit("Third");
    project.save_as_user();
    assert_eq!(
        *log.0.lock().expect("log"),
        ["encode", "encode", "encode", "finish false"],
    );
    drop(hold);
    project.settle();
    assert!(project.asset("zone.log-1").exists());
    assert!(!project.asset("zone.log-2").exists(), "the replaced one");
    assert!(project.asset("zone.log-3").exists());
}

#[test]
fn a_write_that_dies_before_the_rename_names_the_file_and_leaves_it() {
    let mut project = project();
    let hold = hold_saves_before_writing(project.app.world_mut());
    project.save_as_user();
    hold.stop_the_writes();
    project.settle();

    assert_eq!(project.on_disk(), ZONE);
    assert!(project.active_dirty());
    assert!(
        project.notice().contains("zone.bsn") && project.notice().contains("stopped"),
        "{}",
        project.notice()
    );
}

#[test]
fn a_write_that_dies_after_the_rename_is_a_save_that_landed() {
    let mut project = project();
    run_watch_frames(&mut project.app, 100);
    let hold = hold_saves_after_writing(project.app.world_mut());
    project.save_as_user();
    let scene = project.scene.clone();
    util::update_until(
        &mut project.app,
        "the rename",
        Duration::from_secs(10),
        move |_| std::fs::read_to_string(&scene).is_ok_and(|text| text.contains("Props")),
    );
    hold.stop_the_writes();
    project.settle();
    run_watch_frames(&mut project.app, 400);

    assert!(!project.active_dirty(), "the file holds this state");
    assert_eq!(
        prompts(&project.app),
        0,
        "and our own bytes raise no prompt"
    );
}

#[test]
fn a_write_leaves_no_temporary_file() {
    let mut project = project();
    project.save_as_user();
    project.settle();
    let left: Vec<_> = std::fs::read_dir(project.tmp.path().join("assets"))
        .expect("the folder")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| jackdaw_api_internal::document::is_temporary_name(name))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn opening_a_project_sweeps_what_a_dead_write_left() {
    let mut project = project_with_files(|root| {
        let left = root.join("assets/.zone.bsn.jackdaw-tmp-123");
        std::fs::write(&left, "half a scene").expect("written");
        std::fs::File::options()
            .write(true)
            .open(&left)
            .expect("opens")
            .set_modified(std::time::SystemTime::now() - Duration::from_secs(2 * 60 * 60))
            .expect("the time is set");
    });
    let left = project.asset(".zone.bsn.jackdaw-tmp-123");
    util::update_until(
        &mut project.app,
        "the sweep",
        Duration::from_secs(10),
        move |_| !left.exists(),
    );
}

#[test]
fn only_old_temporary_files_are_swept() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let folder = tmp.path().join("assets/deep");
    std::fs::create_dir_all(&folder).expect("folder");
    let old = folder.join(".zone.bsn.jackdaw-tmp-123");
    let young = folder.join(".yard.bsn.jackdaw-tmp-124");
    let other = folder.join(".zone.bsn.jackdaw-tmp-abc");
    let lookalike = folder.join(".env.tmp-20241101");
    let scene = folder.join("zone.bsn");
    for file in [&old, &young, &other, &lookalike, &scene] {
        std::fs::write(file, "bytes").expect("written");
    }
    let long_ago = std::time::SystemTime::now() - Duration::from_secs(2 * 60 * 60);
    for file in [&old, &other, &lookalike, &scene] {
        std::fs::File::options()
            .write(true)
            .open(file)
            .expect("opens")
            .set_modified(long_ago)
            .expect("the time is set");
    }

    let removed = jackdaw::scene_io::save_worker::remove_stale_temporaries(
        tmp.path(),
        Duration::from_secs(60 * 60),
    );

    assert_eq!(removed, 1);
    assert!(!old.exists());
    assert!(young.exists() && other.exists() && scene.exists());
    assert!(lookalike.exists(), "a file of the user's");
}
