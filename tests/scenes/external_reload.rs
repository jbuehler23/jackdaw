//! A scene file edited outside the editor surfaces in the editor as a
//! prompt: Reload, or Keep what is open.

use std::path::Path;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use jackdaw::scenes::external_watch::{
    ExternalReloadChoice, ExternalSceneChanges, ExternalSceneWatchPlugin, answer_external_change,
};

const ALPHA: &str = "#Alpha\nbevy_transform::components::transform::Transform\n";
const BETA: &str = "#Beta\nbevy_transform::components::transform::Transform\n";
/// The removed facade UI vocabulary, which every load path refuses.
const RETIRED: &str = "#Overlay\njackdaw_ui::UiCanvas\n";

fn make_app() -> App {
    use bevy::render::RenderPlugin;
    use bevy::render::settings::{RenderCreation, WgpuSettings};
    use bevy::winit::WinitPlugin;

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(RenderPlugin {
                render_creation: RenderCreation::Automatic(Box::new(WgpuSettings {
                    backends: None,
                    ..default()
                })),
                ..default()
            })
            .disable::<WinitPlugin>(),
    );
    app.add_plugins(jackdaw_scene_types::SceneTypesPlugin::default());
    app.add_plugins(jackdaw_bsn::JackdawBsnPlugin);
    app.init_resource::<jackdaw::commands::CommandHistory>();
    app.init_resource::<jackdaw::scene_io::SceneFilePath>();
    app.init_resource::<jackdaw::scene_io::SceneDirtyState>();
    app.init_resource::<jackdaw::selection::Selection>();
    app.init_resource::<jackdaw::scenes::Scenes>();
    app.init_resource::<jackdaw::scenes::operators::UntitledCounter>();
    app.add_plugins(ExternalSceneWatchPlugin);
    app
}

fn spawned_names(app: &mut App) -> Vec<String> {
    let mut q = app.world_mut().query::<&Name>();
    q.iter(app.world())
        .map(|n| n.as_str().to_string())
        .collect()
}

/// Run frames until `until` holds, or give up. Filesystem event latency
/// varies by OS, so the deadline is generous rather than tight.
fn pump_until(app: &mut App, until: impl Fn(&App) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        app.update();
        if until(app) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    app.update();
    until(app)
}

fn prompts(app: &App) -> &[jackdaw::scenes::external_watch::ExternalSceneChange] {
    &app.world().resource::<ExternalSceneChanges>().prompts
}

fn prompt_names(app: &App, file: &Path) -> bool {
    let canonical = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    prompts(app).iter().any(|change| change.path == canonical)
}

/// Open `scene` as a tab and let the watcher install itself over it.
fn open_and_settle(app: &mut App, scene: &Path) {
    jackdaw::scenes::operators::scene_open_system(app.world_mut(), scene);
    for _ in 0..3 {
        app.update();
    }
}

/// Write over the file the way another program would, not through any editor
/// boundary.
fn external_write(path: &Path, text: &str) {
    std::fs::write(path, text).expect("the outside write lands");
}

#[test]
fn an_outside_edit_to_an_open_scene_raises_a_prompt_naming_the_file() {
    let tmp = tempfile::tempdir().unwrap();
    let scene = tmp.path().join("zone.bsn");
    std::fs::write(&scene, ALPHA).unwrap();

    let mut app = make_app();
    open_and_settle(&mut app, &scene);
    assert!(
        spawned_names(&mut app).iter().any(|n| n == "Alpha"),
        "the open tab holds the file's first contents",
    );

    external_write(&scene, BETA);

    assert!(
        pump_until(&mut app, |app| prompt_names(app, &scene)),
        "an outside edit to an open scene must surface as a prompt",
    );
    assert!(
        spawned_names(&mut app).iter().any(|n| n == "Alpha"),
        "and nothing reloads until the user says so",
    );
}

#[test]
fn reload_replaces_the_open_scene_and_empties_its_history() {
    struct CounterCommand;
    impl jackdaw::commands::EditorCommand for CounterCommand {
        fn execute(&mut self, _world: &mut World) {}
        fn undo(&mut self, _world: &mut World) {}
        fn description(&self) -> &str {
            "counter"
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    let scene = tmp.path().join("zone.bsn");
    std::fs::write(&scene, ALPHA).unwrap();

    let mut app = make_app();
    open_and_settle(&mut app, &scene);
    app.world_mut()
        .resource_mut::<jackdaw::commands::CommandHistory>()
        .push_executed(Box::new(CounterCommand));

    external_write(&scene, BETA);
    assert!(
        pump_until(&mut app, |app| prompt_names(app, &scene)),
        "the prompt has to appear before it can be answered",
    );

    answer_external_change(app.world_mut(), &scene, ExternalReloadChoice::Reload);

    let names = spawned_names(&mut app);
    assert!(
        names.iter().any(|n| n == "Beta"),
        "Reload puts the file's new contents in the world: {names:?}",
    );
    assert!(
        !names.iter().any(|n| n == "Alpha"),
        "and takes the old ones out: {names:?}",
    );
    let history = app.world().resource::<jackdaw::commands::CommandHistory>();
    assert!(
        history.undo_stack.is_empty() && history.redo_stack.is_empty(),
        "the history described a document that no longer exists",
    );
    assert!(
        prompts(&app).is_empty(),
        "an answered prompt does not stay up",
    );
    assert!(
        !app.world().resource::<jackdaw::scenes::Scenes>().tabs[0].dirty,
        "a tab that just came off disk has nothing unsaved",
    );
}

#[test]
fn keep_leaves_the_open_scene_alone_and_the_next_save_overwrites_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let scene = tmp.path().join("zone.bsn");
    std::fs::write(&scene, ALPHA).unwrap();

    let mut app = make_app();
    open_and_settle(&mut app, &scene);

    external_write(&scene, BETA);
    assert!(
        pump_until(&mut app, |app| prompt_names(app, &scene)),
        "the prompt has to appear before it can be answered",
    );

    answer_external_change(app.world_mut(), &scene, ExternalReloadChoice::Keep);

    let names = spawned_names(&mut app);
    assert!(
        names.iter().any(|n| n == "Alpha") && !names.iter().any(|n| n == "Beta"),
        "Keep leaves the editor holding what it had: {names:?}",
    );
    assert!(prompts(&app).is_empty(), "and dismisses the prompt");

    assert!(
        jackdaw::scene_io::save_scene(app.world_mut()),
        "the scene saves",
    );
    let on_disk = std::fs::read_to_string(&scene).expect("the scene is on disk");
    assert!(
        on_disk.contains("Alpha") && !on_disk.contains("Beta"),
        "Keep means the next save overwrites the outside edit; disk holds:\n{on_disk}",
    );
}

#[test]
fn the_editors_own_save_raises_no_prompt() {
    let tmp = tempfile::tempdir().unwrap();
    let scene = tmp.path().join("zone.bsn");
    std::fs::write(&scene, ALPHA).unwrap();

    let mut app = make_app();
    open_and_settle(&mut app, &scene);

    let before = std::fs::read_to_string(&scene).unwrap();
    assert!(
        jackdaw::scene_io::save_scene(app.world_mut()),
        "the scene saves",
    );
    let after = std::fs::read_to_string(&scene).unwrap();
    assert_ne!(
        before, after,
        "the save has to really change the bytes on disk, or this proves nothing",
    );

    let raised = pump_until(&mut app, |app| prompt_names(app, &scene));
    assert!(
        !raised,
        "the editor writing the file is not an outside edit: {:?}",
        prompts(&app)
            .iter()
            .map(|c| c.path.clone())
            .collect::<Vec<_>>(),
    );
}

/// The watch survives the save it suppresses: a save lands by rename, replacing
/// the file the watch went on, and nothing re-arms it afterwards.
#[test]
fn an_outside_edit_after_the_editors_own_save_still_raises_a_prompt() {
    let tmp = tempfile::tempdir().unwrap();
    let scene = tmp.path().join("zone.bsn");
    std::fs::write(&scene, ALPHA).unwrap();

    let mut app = make_app();
    open_and_settle(&mut app, &scene);
    assert!(
        jackdaw::scene_io::save_scene(app.world_mut()),
        "the scene saves",
    );
    for _ in 0..3 {
        app.update();
    }

    external_write(&scene, BETA);

    assert!(
        pump_until(&mut app, |app| prompt_names(app, &scene)),
        "an outside edit still has to be seen after the editor wrote the file",
    );
}

#[test]
fn a_refused_reload_keeps_the_open_scene_and_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    let scene = tmp.path().join("zone.bsn");
    std::fs::write(&scene, ALPHA).unwrap();

    let mut app = make_app();
    open_and_settle(&mut app, &scene);

    external_write(&scene, RETIRED);
    assert!(
        pump_until(&mut app, |app| prompt_names(app, &scene)),
        "the prompt appears for any outside edit, refusable or not",
    );

    answer_external_change(app.world_mut(), &scene, ExternalReloadChoice::Reload);

    let names = spawned_names(&mut app);
    assert!(
        names.iter().any(|n| n == "Alpha"),
        "a document the editor refuses must not cost the user the open scene: {names:?}",
    );
    assert!(
        !names.iter().any(|n| n == "Overlay"),
        "and the refused document spawns nothing: {names:?}",
    );

    let canonical = std::fs::canonicalize(&scene).unwrap();
    let refused = app
        .world()
        .resource::<ExternalSceneChanges>()
        .refused
        .as_ref()
        .expect("a refused reload is reported, not swallowed");
    assert_eq!(refused.path, canonical, "the report names the file");
    assert_eq!(
        refused.category,
        jackdaw::scene_io::RefusalCategory::Retired,
        "and classifies why, so the notice can lead with it",
    );
    let notice = refused.description();
    assert!(
        notice.contains("zone.bsn")
            && notice.contains(jackdaw::scene_io::RefusalCategory::Retired.label()),
        "the notice the user reads names the file and the reason: {notice}",
    );
    assert!(
        notice.contains("untouched"),
        "and says the open copy survived: {notice}",
    );
}

#[test]
fn the_prompt_says_what_a_reload_would_cost_a_dirty_tab() {
    let tmp = tempfile::tempdir().unwrap();
    let scene = tmp.path().join("zone.bsn");
    std::fs::write(&scene, ALPHA).unwrap();

    let mut app = make_app();
    open_and_settle(&mut app, &scene);
    app.world_mut()
        .resource_mut::<jackdaw::scenes::Scenes>()
        .tabs[0]
        .dirty = true;

    external_write(&scene, BETA);
    assert!(
        pump_until(&mut app, |app| prompt_names(app, &scene)),
        "a dirty tab gets the same prompt, with different words",
    );

    let change = prompts(&app)[0].clone();
    assert!(
        change
            .action_label()
            .contains("discards your unsaved changes"),
        "a dirty tab is told what Reload costs: {:?}",
        change.action_label(),
    );
    assert!(
        change.description().contains("zone.bsn"),
        "the prompt names the file: {:?}",
        change.description(),
    );
}

#[test]
fn a_clean_tab_is_not_warned_about_changes_it_does_not_have() {
    let tmp = tempfile::tempdir().unwrap();
    let scene = tmp.path().join("zone.bsn");
    std::fs::write(&scene, ALPHA).unwrap();

    let mut app = make_app();
    open_and_settle(&mut app, &scene);

    external_write(&scene, BETA);
    assert!(pump_until(&mut app, |app| prompt_names(app, &scene)));

    let change = prompts(&app)[0].clone();
    assert!(
        !change.action_label().contains("discards"),
        "nothing is discarded when nothing is unsaved: {:?}",
        change.action_label(),
    );
}

#[test]
fn closing_the_tab_takes_its_prompt_with_it() {
    let tmp = tempfile::tempdir().unwrap();
    let scene = tmp.path().join("zone.bsn");
    std::fs::write(&scene, ALPHA).unwrap();

    let mut app = make_app();
    open_and_settle(&mut app, &scene);

    external_write(&scene, BETA);
    assert!(pump_until(&mut app, |app| prompt_names(app, &scene)));

    app.world_mut()
        .resource_mut::<jackdaw::scenes::Scenes>()
        .tabs
        .clear();
    app.update();

    assert!(
        prompts(&app).is_empty(),
        "a prompt about a file nobody has open has nothing to offer",
    );
}

/// The queue moves under a dialog that is already up. The answer belongs to
/// the file the dialog named, not to whatever reached the front meanwhile.
#[test]
fn an_answer_goes_to_its_own_file_not_whichever_prompt_is_in_front() {
    let tmp = tempfile::tempdir().unwrap();
    let first = tmp.path().join("first.bsn");
    let second = tmp.path().join("second.bsn");
    std::fs::write(&first, ALPHA).unwrap();
    std::fs::write(&second, ALPHA).unwrap();

    let mut app = make_app();
    open_and_settle(&mut app, &first);
    open_and_settle(&mut app, &second);

    external_write(&first, BETA);
    external_write(&second, BETA);
    assert!(
        pump_until(&mut app, |app| prompt_names(app, &first)
            && prompt_names(app, &second)),
        "both open files changed, so both are asked about",
    );

    answer_external_change(app.world_mut(), &second, ExternalReloadChoice::Reload);

    assert!(
        prompt_names(&app, &first),
        "answering the second file must leave the first still asking",
    );
    assert!(
        !prompt_names(&app, &second),
        "and must take its own prompt off the queue",
    );
    assert_eq!(
        std::fs::read_to_string(&first).unwrap(),
        BETA,
        "the file that was not answered for is untouched on disk",
    );
}

/// The front prompt is evicted while its dialog is up, so the answer arrives
/// for a path that has left the queue.
#[test]
fn an_answer_for_an_evicted_prompt_does_not_touch_the_new_front() {
    let tmp = tempfile::tempdir().unwrap();
    let first = tmp.path().join("first.bsn");
    let second = tmp.path().join("second.bsn");
    std::fs::write(&first, ALPHA).unwrap();
    std::fs::write(&second, ALPHA).unwrap();

    let mut app = make_app();
    open_and_settle(&mut app, &first);
    open_and_settle(&mut app, &second);

    external_write(&first, BETA);
    external_write(&second, BETA);
    assert!(pump_until(&mut app, |app| prompt_names(app, &first)
        && prompt_names(app, &second)));

    // The front prompt's tab goes away under the open dialog.
    let front = prompts(&app)[0].path.clone();
    let survivor = prompts(&app)[1].path.clone();
    app.world_mut()
        .resource_mut::<jackdaw::scenes::Scenes>()
        .tabs
        .retain(|tab| {
            tab.path
                .as_ref()
                .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()) != front)
                .unwrap_or(true)
        });
    app.update();
    assert!(
        !prompts(&app).iter().any(|p| p.path == front),
        "the evicted prompt is gone, or this proves nothing",
    );

    answer_external_change(app.world_mut(), &front, ExternalReloadChoice::Reload);

    assert!(
        prompts(&app).iter().any(|p| p.path == survivor),
        "an answer for a path that left the queue must not consume the next one",
    );
}

/// A refusal must not park the user on a tab they never asked to visit: the load
/// installs into the live world, so the swap happens before it knows whether it
/// will accept the document.
#[test]
fn a_refused_reload_puts_the_user_back_where_they_were() {
    let tmp = tempfile::tempdir().unwrap();
    let watched = tmp.path().join("watched.bsn");
    let working = tmp.path().join("working.bsn");
    std::fs::write(&watched, ALPHA).unwrap();
    std::fs::write(&working, BETA).unwrap();

    let mut app = make_app();
    open_and_settle(&mut app, &watched);
    open_and_settle(&mut app, &working);
    let working_tab = app.world().resource::<jackdaw::scenes::Scenes>().active;

    external_write(&watched, RETIRED);
    assert!(pump_until(&mut app, |app| prompt_names(app, &watched)));

    answer_external_change(app.world_mut(), &watched, ExternalReloadChoice::Reload);

    assert_eq!(
        app.world().resource::<jackdaw::scenes::Scenes>().active,
        working_tab,
        "a refused reload leaves the user on the tab they were working in",
    );
    let names = spawned_names(&mut app);
    assert!(
        names.iter().any(|n| n == "Beta"),
        "and the world still holds that tab's scene: {names:?}",
    );
}

/// The watch goes on after the open has read the file, so an edit landing in that
/// gap is one notify never reports: the baseline has to be what the open read.
#[test]
fn an_edit_between_the_open_and_the_watch_is_still_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let scene = tmp.path().join("zone.bsn");
    std::fs::write(&scene, ALPHA).unwrap();

    let mut app = make_app();
    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &scene);
    // No frame has run, so no watch is installed yet.
    external_write(&scene, BETA);

    assert!(
        pump_until(&mut app, |app| prompt_names(app, &scene)),
        "a change the editor could not have been watching for is still a change",
    );
}

/// Keep says "I have seen those bytes". A repeat event carrying the same
/// bytes is not news.
#[test]
fn keep_is_not_asked_again_about_the_same_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let scene = tmp.path().join("zone.bsn");
    std::fs::write(&scene, ALPHA).unwrap();

    let mut app = make_app();
    open_and_settle(&mut app, &scene);

    external_write(&scene, BETA);
    assert!(pump_until(&mut app, |app| prompt_names(app, &scene)));
    answer_external_change(app.world_mut(), &scene, ExternalReloadChoice::Keep);

    // The same content written again: an event, but no change.
    external_write(&scene, BETA);
    let asked_again = pump_until(&mut app, |app| prompt_names(app, &scene));
    assert!(
        !asked_again,
        "the same bytes the user already answered about are not asked again",
    );
}

/// A write that lands while the prompt is up was never shown to the user, so
/// Keep cannot have answered for it.
#[test]
fn a_write_during_the_prompt_is_asked_about_after_keep() {
    const GAMMA: &str = "#Gamma\nbevy_transform::components::transform::Transform\n";

    let tmp = tempfile::tempdir().unwrap();
    let scene = tmp.path().join("zone.bsn");
    std::fs::write(&scene, ALPHA).unwrap();

    let mut app = make_app();
    open_and_settle(&mut app, &scene);

    external_write(&scene, BETA);
    assert!(pump_until(&mut app, |app| prompt_names(app, &scene)));

    // Another program writes again while the question is still on screen.
    external_write(&scene, GAMMA);
    answer_external_change(app.world_mut(), &scene, ExternalReloadChoice::Keep);

    assert!(
        pump_until(&mut app, |app| prompt_names(app, &scene)),
        "Keep answered for the bytes it was shown, not for the ones that \
         arrived behind it",
    );
}

/// An outside edit can turn a scene file into a prefab, and a prefab document
/// left in a Scene tab would be saved back as a plain scene.
#[test]
fn a_reload_that_brings_a_prefab_retypes_the_tab() {
    const PREFAB: &str = "#Rock\njackdaw::prefab::components::Prefab\n";

    let tmp = tempfile::tempdir().unwrap();
    let scene = tmp.path().join("zone.bsn");
    std::fs::write(&scene, ALPHA).unwrap();

    let mut app = make_app();
    app.init_resource::<jackdaw::prefab::PrefabAstCache>();
    open_and_settle(&mut app, &scene);
    assert!(
        matches!(
            app.world().resource::<jackdaw::scenes::Scenes>().tabs[0].kind,
            jackdaw::scenes::TabKind::Scene
        ),
        "it opened as a scene, or this proves nothing",
    );

    external_write(&scene, PREFAB);
    assert!(pump_until(&mut app, |app| prompt_names(app, &scene)));
    answer_external_change(app.world_mut(), &scene, ExternalReloadChoice::Reload);

    let scenes = app.world().resource::<jackdaw::scenes::Scenes>();
    assert!(
        matches!(scenes.tabs[0].kind, jackdaw::scenes::TabKind::Prefab),
        "what came off disk decides what the tab is",
    );
    assert!(
        matches!(
            scenes.tabs[0].content,
            jackdaw::scenes::TabContent::Prefab(_)
        ),
        "and the content follows the kind, so a capture cannot write it back \
         out as a scene",
    );
}

/// A prefab file open in a tab is a document the user is editing, so an outside
/// edit to it has to be a question, the same way it is for a scene.
#[test]
fn an_outside_edit_to_an_open_prefab_raises_a_prompt() {
    const ROCK: &str = "#Rock\njackdaw::prefab::components::Prefab\n";
    const BOULDER: &str = "#Boulder\njackdaw::prefab::components::Prefab\n";

    let tmp = tempfile::tempdir().unwrap();
    let prefab = tmp.path().join("rock.bsn");
    std::fs::write(&prefab, ROCK).unwrap();

    let mut app = make_app();
    app.init_resource::<jackdaw::prefab::PrefabAstCache>();
    open_and_settle(&mut app, &prefab);
    assert!(
        matches!(
            app.world().resource::<jackdaw::scenes::Scenes>().tabs[0].kind,
            jackdaw::scenes::TabKind::Prefab
        ),
        "it opened as a prefab tab, or this proves nothing",
    );

    external_write(&prefab, BOULDER);

    assert!(
        pump_until(&mut app, |app| prompt_names(app, &prefab)),
        "an outside edit to an open prefab must surface as a prompt",
    );
    assert!(
        prompts(&app)[0].tab_is_prefab,
        "and the prompt knows it is asking about a prefab",
    );
    assert_eq!(
        prompts(&app)[0].title(),
        "Prefab Changed on Disk",
        "so the dialog names what actually changed",
    );
    assert!(
        spawned_names(&mut app).iter().any(|n| n == "Rock"),
        "and nothing reloads until the user says so",
    );
}
