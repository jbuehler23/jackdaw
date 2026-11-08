//! What a terrain's tools reach after the scene it lives in has been saved and
//! opened again. A terrain's extent is its regions', and the component's two
//! extent fields are drained back to their defaults on every load, so a tool
//! that strides the grid by them addresses only a 256-cell corner.

use crate::util;

use std::path::PathBuf;
use std::time::Duration;

use bevy::input::ButtonInput;
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_editor_core::scenes::Scenes;
use jackdaw_editor_core::scenes::external_watch::{
    ExternalReloadChoice, ExternalSceneChange, ExternalSceneChanges, answer_external_change,
};
use jackdaw_editor_core::selection::Selection;
use jackdaw_editor_terrain::{
    PaintDomain, TerrainBrushSettings, TerrainDataStore, TerrainEditMode, TerrainPaintState,
    TerrainSculptState,
};

/// Far enough out that the drained `resolution` (256) cannot address it,
/// and well inside the four-region footprint generating lays down (1024).
const FAR: u32 = 700;

/// What generating a fresh terrain reaches: four default-sized regions.
const GENERATED_RESOLUTION: u32 = 1024;

/// One frame's worth of time, so a brush whose strength is per-second has
/// something to multiply by. A headless app can tick faster than the clock's
/// resolution, which leaves every brush a no-op.
fn advance_a_frame(app: &mut App) {
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(Duration::from_millis(16));
}

/// Give every terrain in the world its sidecar path and dirty-chunk tracking.
/// The editor's schedule runs these gated on `AppState::Editor`, which a headless
/// test never enters, and without them a terrain has no document to edit.
fn settle_terrain(app: &mut App) {
    app.world_mut()
        .run_system_cached(jackdaw_editor_terrain::ensure_terrain_dirty_chunks)
        .expect("dirty-chunk tracking is installed");
    app.world_mut()
        .run_system_cached(jackdaw_editor_terrain::ensure_terrain_data_path)
        .expect("sidecar paths are minted");
    app.update();
}

#[track_caller]
fn dispatch(app: &mut App, id: &'static str) {
    let result = app
        .world_mut()
        .operator(id)
        .call()
        .unwrap_or_else(|err| panic!("{id} dispatch errored: {err}"));
    assert_eq!(result, OperatorResult::Finished, "{id} did not finish");
}

/// Start a brush modal the way the viewport does: the button goes down and one
/// dispatch runs the operator's first frame, leaving it running.
#[track_caller]
fn start_stroke(app: &mut App, id: &'static str) {
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    advance_a_frame(app);
    let result = app
        .world_mut()
        .operator(id)
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: false,
        })
        .call()
        .unwrap_or_else(|err| panic!("{id} dispatch errored: {err}"));
    assert_eq!(
        result,
        OperatorResult::Running,
        "{id} did not start a stroke"
    );
}

/// Let go of the button and tick, so the modal ends and the next stroke
/// has the floor.
fn end_stroke(app: &mut App) {
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    app.update();
    app.update();
}

fn the_terrain(app: &mut App) -> (Entity, jackdaw_scene_types::Terrain) {
    let mut query = app
        .world_mut()
        .query::<(Entity, &jackdaw_scene_types::Terrain)>();
    let mut found: Vec<(Entity, jackdaw_scene_types::Terrain)> = query
        .iter(app.world())
        .map(|(entity, terrain)| (entity, terrain.clone()))
        .collect();
    assert_eq!(found.len(), 1, "expected exactly one terrain in the scene");
    found.pop().expect("just checked")
}

fn heights(app: &App, data_path: &str) -> Vec<f32> {
    app.world()
        .resource::<TerrainDataStore>()
        .heights(data_path)
        .into_owned()
}

fn stage_dir() -> tempfile::TempDir {
    let stage = tempfile::Builder::new()
        .prefix("terrain-reload-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("tempdir under target/");
    std::fs::create_dir_all(stage.path().join("assets")).expect("create assets dir");
    stage
}

/// Author a terrain, generate ground under it, and save the scene into
/// `stage`. Returns the scene file it wrote.
fn author_and_save(stage: &std::path::Path) -> PathBuf {
    let mut app = util::editor_test_app();
    dispatch(&mut app, "scene.new");
    app.update();
    dispatch(&mut app, "entity.add.terrain");
    app.update();
    settle_terrain(&mut app);

    let (entity, _) = the_terrain(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![entity];
    dispatch(&mut app, "terrain.generate");
    app.update();

    let (_, terrain) = the_terrain(&mut app);
    assert_eq!(
        app.world()
            .resource::<TerrainDataStore>()
            .grid_shape(&terrain)
            .resolution,
        GENERATED_RESOLUTION,
        "generating must lay down the footprint the reload assertions edit past"
    );

    let scene_path = stage.join("assets").join("ground.bsn");
    {
        let mut scenes = app.world_mut().resource_mut::<Scenes>();
        let active = scenes.active;
        scenes
            .tabs
            .get_mut(active)
            .expect("scene.new left an active tab")
            .path = Some(scene_path.clone());
    }
    dispatch(&mut app, "scene.save");
    app.update();
    assert!(
        scene_path.exists(),
        "scene.save wrote nothing to {}",
        scene_path.display()
    );

    scene_path
}

fn reopen(scene_path: &std::path::Path) -> App {
    let mut app = util::editor_test_app();
    dispatch(&mut app, "scene.new");
    app.update();
    jackdaw_editor_core::scene_io::load_scene_from_file(app.world_mut(), scene_path);
    settle_terrain(&mut app);
    app
}

/// Point the active tab at a file, so a `scene.save` in a reopened app has
/// somewhere to write. Loading into a fresh tab leaves it pathless.
fn aim_tab_at(app: &mut App, scene_path: &std::path::Path) {
    let mut scenes = app.world_mut().resource_mut::<Scenes>();
    let active = scenes.active;
    scenes
        .tabs
        .get_mut(active)
        .expect("scene.new left an active tab")
        .path = Some(scene_path.to_path_buf());
}

#[test]
fn tools_reach_the_far_extent_after_a_reload() {
    let stage = stage_dir();
    let scene_path = author_and_save(stage.path());
    let mut app = reopen(&scene_path);

    let (entity, terrain) = the_terrain(&mut app);
    let data_path = terrain.data_path.clone();
    assert!(
        !data_path.is_empty(),
        "the reloaded terrain lost the sidecar it was saved with"
    );
    let resolution = app
        .world()
        .resource::<TerrainDataStore>()
        .grid_shape(&terrain)
        .resolution;
    assert_eq!(
        resolution, GENERATED_RESOLUTION,
        "the reloaded terrain holds fewer cells than it was saved with"
    );
    assert_eq!(
        terrain.resolution,
        jackdaw_scene_types::Terrain::default().resolution,
        "loading must drain the declared resolution, which is what makes \
         addressing the terrain by it wrong from here on"
    );

    let far = (FAR * resolution + FAR) as usize;
    app.world_mut()
        .resource_mut::<TerrainBrushSettings>()
        .radius = 8.0;

    app.world_mut()
        .resource_mut::<TerrainDataStore>()
        .set_materials(
            data_path.clone(),
            vec![jackdaw_terrain::sidecar::TerrainMaterialSlot::new("grass")],
        )
        .expect("a single well-named material");
    assert!(
        !app.world()
            .resource::<TerrainDataStore>()
            .control(&data_path)[far]
            .manual(),
        "the far cell starts unpainted"
    );
    *app.world_mut().resource_mut::<TerrainEditMode>() = TerrainEditMode::Paint;
    {
        let mut paint = app.world_mut().resource_mut::<TerrainPaintState>();
        paint.domain = PaintDomain::Textures;
        paint.target = Some(entity);
        paint.active_texture_id = 0;
        paint.texture_opacity = 1.0;
        paint.brush_position = Some(Vec2::splat(FAR as f32));
    }
    start_stroke(&mut app, "terrain.paint");
    assert!(
        app.world()
            .resource::<TerrainDataStore>()
            .control(&data_path)[far]
            .manual(),
        "the stroke did not reach cell ({FAR}, {FAR}): paint is still addressing \
         the grid by the drained declared resolution"
    );

    end_stroke(&mut app);

    let before = heights(&app, &data_path);
    *app.world_mut().resource_mut::<TerrainEditMode>() =
        TerrainEditMode::Sculpt(jackdaw_terrain::SculptTool::Raise);
    {
        let mut sculpt = app.world_mut().resource_mut::<TerrainSculptState>();
        sculpt.target = Some(entity);
        sculpt.brush_position = Some(Vec2::splat(FAR as f32));
    }
    start_stroke(&mut app, "terrain.sculpt");
    let after = heights(&app, &data_path);
    assert_ne!(
        before[far], after[far],
        "the stroke did not reach cell ({FAR}, {FAR})"
    );
    end_stroke(&mut app);
    *app.world_mut().resource_mut::<TerrainEditMode>() = TerrainEditMode::None;

    // Striding the real heights by a smaller resolution walks a window over the
    // front of the array, so the signature to rule out is nothing beyond row
    // `resolution` moving. Droplets land at random, so this counts cells.
    let before = heights(&app, &data_path);
    app.world_mut().resource_mut::<Selection>().entities = vec![entity];
    dispatch(&mut app, "terrain.erode");
    let after = heights(&app, &data_path);
    let moved_in_the_far_half = (resolution as usize / 2..resolution as usize)
        .flat_map(|z| (0..resolution as usize).map(move |x| z * resolution as usize + x))
        .filter(|&i| before[i] != after[i])
        .count();
    assert!(
        moved_in_the_far_half > 0,
        "erosion left the far half of the terrain untouched: it is striding the \
         real heights by the drained declared resolution"
    );
}

/// A sidecar rewritten on disk is what a reopened scene shows. The store is the
/// live terrain document and outlives the file it came from, so a clean tab
/// returning to the foreground has to take what the file says.
#[test]
fn a_sidecar_rewritten_on_disk_reaches_a_reopened_scene() {
    let stage = stage_dir();
    let scene_path = author_and_save(stage.path());

    let mut app = reopen(&scene_path);
    let (_, terrain) = the_terrain(&mut app);
    let data_path = terrain.data_path.clone();
    let before = heights(&app, &data_path);
    assert!(!before.is_empty(), "the reopened scene has no heights");

    let raised = rewrite_sidecar_on_disk(&app, &scene_path, &data_path, 25.0);

    // Open the same scene again. It is already the active tab, so this is
    // the de-duped path: the tab is clean, so disk wins.
    jackdaw_editor_core::scenes::operators::scene_open_system(app.world_mut(), &scene_path);
    settle_terrain(&mut app);
    app.update();

    let after = heights(&app, &data_path);
    assert_eq!(
        after.len(),
        raised.len(),
        "the reload changed the grid size"
    );
    assert!(
        after
            .iter()
            .zip(&raised)
            .all(|(got, want)| (got - want).abs() < 0.001),
        "the store is still holding the heights from before the sidecar was rewritten"
    );
}

/// A tab with unsaved sculpting keeps it: the refresh above must not become
/// "disk always wins" and throw away edits nobody has written.
#[test]
fn returning_to_a_dirty_tab_keeps_its_unsaved_heights() {
    let stage = stage_dir();
    let scene_path = author_and_save(stage.path());

    let mut app = reopen(&scene_path);
    let (_, terrain) = the_terrain(&mut app);
    let data_path = terrain.data_path.clone();

    // The tab this scene is in, named and marked the way an edit leaves it.
    let sculpted: Vec<f32> = heights(&app, &data_path)
        .iter()
        .map(|height| height + 7.0)
        .collect();
    {
        let mut store = app.world_mut().resource_mut::<TerrainDataStore>();
        let mut entry = store
            .entry_for(&terrain)
            .expect("the terrain is in the store");
        entry.set_heights(&sculpted);
    }
    {
        let mut scenes = app.world_mut().resource_mut::<Scenes>();
        let active = scenes.active;
        scenes.tabs[active].path = Some(scene_path.clone());
        scenes.tabs[active].dirty = true;
    }

    // A second tab to leave and come back from. `scene.new` activates it,
    // so returning is the swap the tab strip makes.
    let sculpting_tab = app.world().resource::<Scenes>().active;
    dispatch(&mut app, "scene.new");
    app.update();
    assert_ne!(
        app.world().resource::<Scenes>().active,
        sculpting_tab,
        "scene.new did not open a second tab to leave"
    );
    swap_to(&mut app, sculpting_tab);
    settle_terrain(&mut app);

    let after = heights(&app, &data_path);
    assert!(
        after
            .iter()
            .zip(&sculpted)
            .all(|(got, want)| (got - want).abs() < 0.001),
        "returning to a dirty tab threw away its unsaved sculpting"
    );
}

/// Rewrite the sidecar the way something outside the editor would: the
/// store's document with every height moved by `delta`. Returns the heights
/// written.
fn rewrite_sidecar_on_disk(
    app: &App,
    scene_path: &std::path::Path,
    data_path: &str,
    delta: f32,
) -> Vec<f32> {
    let sidecar_path = sidecar_file(scene_path, data_path);
    let mut data = app
        .world()
        .resource::<TerrainDataStore>()
        .get(data_path)
        .cloned()
        .expect("the store holds the terrain");
    let raised: Vec<f32> = data
        .grid_heights()
        .iter()
        .map(|height| height + delta)
        .collect();
    data.set_grid_heights(&raised);
    std::fs::write(
        &sidecar_path,
        jackdaw_terrain::sidecar::save(&data).expect("encode the sidecar"),
    )
    .expect("write the sidecar");
    // Staleness is decided by mtime, and a test can write a file twice
    // inside one filesystem timestamp tick, so the write is dated
    // forward explicitly.
    let later = std::time::SystemTime::now() + Duration::from_secs(5);
    std::fs::File::options()
        .write(true)
        .open(&sidecar_path)
        .expect("open the sidecar")
        .set_modified(later)
        .expect("date the sidecar forward");
    raised
}

fn sidecar_file(scene_path: &std::path::Path, data_path: &str) -> PathBuf {
    jackdaw_terrain::sidecar::resolve_path(
        scene_path.parent().expect("the scene has a directory"),
        data_path,
    )
    .expect("the data path resolves")
}

/// Sculpt every height of the terrain by `delta` straight into the store, as a
/// brush stroke writes it, and return the heights it left.
fn sculpt_in_store(app: &mut App, terrain: &jackdaw_scene_types::Terrain, delta: f32) -> Vec<f32> {
    let sculpted: Vec<f32> = heights(app, &terrain.data_path)
        .iter()
        .map(|height| height + delta)
        .collect();
    let mut store = app.world_mut().resource_mut::<TerrainDataStore>();
    let mut entry = store
        .entry_for(terrain)
        .expect("the terrain is in the store");
    entry.set_heights(&sculpted);
    sculpted
}

/// Open a second tab and come back to `tab`, the swap the tab strip makes.
fn leave_and_return(app: &mut App, tab: usize) {
    dispatch(app, "scene.new");
    app.update();
    assert_ne!(
        app.world().resource::<Scenes>().active,
        tab,
        "scene.new did not open a second tab to leave"
    );
    swap_to(app, tab);
    settle_terrain(app);
}

fn reload_prompts(app: &App) -> Vec<ExternalSceneChange> {
    app.world()
        .resource::<ExternalSceneChanges>()
        .prompts
        .clone()
}

/// The reload prompt naming `sidecar` among the files that changed under edits.
fn terrain_prompt(app: &App, sidecar: &std::path::Path) -> Option<ExternalSceneChange> {
    reload_prompts(app)
        .into_iter()
        .find(|prompt| prompt.changed_files.iter().any(|file| file == sidecar))
}

/// A sculpt stroke through the brush operator, as the viewport runs one.
fn sculpt_stroke(app: &mut App, entity: Entity) {
    *app.world_mut().resource_mut::<TerrainEditMode>() =
        TerrainEditMode::Sculpt(jackdaw_terrain::SculptTool::Raise);
    {
        let mut sculpt = app.world_mut().resource_mut::<TerrainSculptState>();
        sculpt.target = Some(entity);
        sculpt.brush_position = Some(Vec2::splat(64.0));
    }
    start_stroke(app, "terrain.sculpt");
    end_stroke(app);
    *app.world_mut().resource_mut::<TerrainEditMode>() = TerrainEditMode::None;
}

fn same_heights(got: &[f32], want: &[f32]) -> bool {
    got.len() == want.len()
        && got
            .iter()
            .zip(want)
            .all(|(got, want)| (got - want).abs() < 0.001)
}

fn active_tab(app: &App) -> usize {
    app.world().resource::<Scenes>().active
}

fn tab_dirty(app: &App, tab: usize) -> bool {
    app.world().resource::<Scenes>().tabs[tab].dirty
}

/// The heights the sidecar on disk holds.
fn heights_on_disk(scene_path: &std::path::Path, data_path: &str) -> Vec<f32> {
    let bytes = std::fs::read(sidecar_file(scene_path, data_path)).expect("read the sidecar");
    jackdaw_terrain::sidecar::load_from(&bytes, None)
        .expect("decode the sidecar")
        .data
        .grid_heights()
}

/// A reopened scene with a terrain, its tab pointed at the file. Returns the app,
/// the terrain entity, its data path and its tab.
fn reopen_aimed(scene_path: &std::path::Path) -> (App, Entity, String, usize) {
    let mut app = reopen(scene_path);
    aim_tab_at(&mut app, scene_path);
    let (entity, terrain) = the_terrain(&mut app);
    let tab = active_tab(&app);
    (app, entity, terrain.data_path, tab)
}

/// Write over the scene file the way another program would, keeping it loadable.
fn touch_scene_outside(scene_path: &std::path::Path) {
    let mut bytes = std::fs::read(scene_path).expect("read the scene");
    bytes.push(b'\n');
    std::fs::write(scene_path, bytes).expect("write the scene");
}

/// A sculpt stroke outlives its file changing on disk while the tab is in the
/// background, and the user is asked whether to reload, told that the tab's
/// other edits would go too.
#[test]
fn a_sculpt_stroke_survives_its_file_changing_on_disk() {
    let stage = stage_dir();
    let scene_path = author_and_save(stage.path());
    let (mut app, entity, data_path, tab) = reopen_aimed(&scene_path);

    sculpt_stroke(&mut app, entity);
    assert!(tab_dirty(&app, tab), "a sculpt stroke leaves the tab dirty");
    let sculpted = heights(&app, &data_path);
    rewrite_sidecar_on_disk(&app, &scene_path, &data_path, 25.0);
    leave_and_return(&mut app, tab);

    assert!(
        same_heights(&heights(&app, &data_path), &sculpted),
        "returning to the tab threw away its unsaved sculpting for the file on disk"
    );
    let sidecar = sidecar_file(&scene_path, &data_path);
    let prompt = terrain_prompt(&app, &sidecar)
        .expect("the user was not told the terrain file changed on disk");
    assert!(
        prompt.discards_edits(),
        "the reload is not marked as losing edits"
    );
    assert!(
        prompt.description().contains("other unsaved changes"),
        "the prompt does not say the tab's other edits go with a reload: {}",
        prompt.description()
    );

    answer_external_change(app.world_mut(), &prompt.path, ExternalReloadChoice::Keep);
    leave_and_return(&mut app, tab);
    assert!(
        reload_prompts(&app).is_empty(),
        "keeping the edits once did not settle the question"
    );
    assert!(
        tab_dirty(&app, tab),
        "keeping the edits left the tab reading as saved"
    );
}

/// A save that would write over a terrain file changed on disk since it was
/// read leaves the file alone and asks; once the edits are kept, it writes.
#[test]
fn saving_over_a_terrain_file_changed_on_disk_asks_first() {
    let stage = stage_dir();
    let scene_path = author_and_save(stage.path());
    let (mut app, entity, data_path, _) = reopen_aimed(&scene_path);

    sculpt_stroke(&mut app, entity);
    let sculpted = heights(&app, &data_path);
    let outside = rewrite_sidecar_on_disk(&app, &scene_path, &data_path, 25.0);

    let _ = app.world_mut().operator("scene.save").call();
    app.update();
    assert!(
        same_heights(&heights_on_disk(&scene_path, &data_path), &outside),
        "the save wrote over the terrain file changed on disk"
    );
    let sidecar = sidecar_file(&scene_path, &data_path);
    let prompt = terrain_prompt(&app, &sidecar).expect("the save did not ask about the file");

    answer_external_change(app.world_mut(), &prompt.path, ExternalReloadChoice::Keep);
    dispatch(&mut app, "scene.save");
    app.update();
    assert!(
        same_heights(&heights_on_disk(&scene_path, &data_path), &sculpted),
        "keeping the edits did not let the next save write them"
    );
}

/// A tab with unsaved edits elsewhere re-reads a terrain it holds no edits to
/// whose file changed on disk, so a later save cannot write the old one back.
#[test]
fn a_dirty_tab_rereads_an_unedited_terrain_whose_file_changed() {
    let stage = stage_dir();
    let scene_path = author_and_save(stage.path());
    let (mut app, _, data_path, tab) = reopen_aimed(&scene_path);

    app.world_mut().resource_mut::<Scenes>().tabs[tab].dirty = true;
    let outside = rewrite_sidecar_on_disk(&app, &scene_path, &data_path, 25.0);
    leave_and_return(&mut app, tab);

    assert!(
        same_heights(&heights(&app, &data_path), &outside),
        "the dirty tab kept the old terrain over the file on disk"
    );
    assert!(
        reload_prompts(&app).is_empty(),
        "an unedited terrain raised a prompt"
    );
}

/// A reload prompt already waiting on a scene takes on a terrain file that
/// changed under edits, rather than the change going unasked.
#[test]
fn a_waiting_scene_prompt_also_names_a_changed_terrain_file() {
    let stage = stage_dir();
    let scene_path = author_and_save(stage.path());
    let (mut app, entity, data_path, tab) = reopen_aimed(&scene_path);
    for _ in 0..3 {
        app.update();
    }

    sculpt_stroke(&mut app, entity);
    let sculpted = heights(&app, &data_path);
    touch_scene_outside(&scene_path);
    let canonical = dunce::canonicalize(&scene_path).expect("the scene exists");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline
        && !reload_prompts(&app)
            .iter()
            .any(|prompt| prompt.path == canonical)
    {
        app.update();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        reload_prompts(&app)
            .iter()
            .any(|prompt| prompt.path == canonical),
        "the outside edit to the scene raised no prompt"
    );

    rewrite_sidecar_on_disk(&app, &scene_path, &data_path, 25.0);
    leave_and_return(&mut app, tab);

    assert!(same_heights(&heights(&app, &data_path), &sculpted));
    let prompts = reload_prompts(&app);
    assert_eq!(prompts.len(), 1, "the scene is asked about once");
    let sidecar = sidecar_file(&scene_path, &data_path);
    assert!(
        prompts[0].document_changed && prompts[0].changed_files.contains(&sidecar),
        "the waiting prompt does not name both the scene and the terrain file"
    );
}

/// Undoing a stroke back to what was saved leaves nothing unsaved, so a terrain
/// file changed on disk is read without asking.
#[test]
fn undoing_back_to_saved_lets_an_outside_terrain_change_reread() {
    let stage = stage_dir();
    let scene_path = author_and_save(stage.path());
    let (mut app, entity, data_path, tab) = reopen_aimed(&scene_path);

    sculpt_stroke(&mut app, entity);
    dispatch(&mut app, "history.undo");
    app.update();
    assert!(
        !tab_dirty(&app, tab),
        "undoing the only stroke leaves the tab clean"
    );
    let outside = rewrite_sidecar_on_disk(&app, &scene_path, &data_path, 25.0);
    leave_and_return(&mut app, tab);

    assert!(
        same_heights(&heights(&app, &data_path), &outside),
        "the undone terrain was not read again from the changed file"
    );
    assert!(
        reload_prompts(&app).is_empty(),
        "an undone terrain raised a prompt"
    );
}

/// Opening a scene already open in a tab that reads clean does not reread it
/// over unsaved terrain edits, even when the scene file moved on.
#[test]
fn reopening_a_scene_keeps_unsaved_terrain_edits() {
    let stage = stage_dir();
    let scene_path = author_and_save(stage.path());
    let (mut app, _, data_path, tab) = reopen_aimed(&scene_path);
    // The watcher learns what the aimed tab's file holds over the next frames.
    for _ in 0..3 {
        app.update();
    }
    let (_, terrain) = the_terrain(&mut app);

    let sculpted = sculpt_in_store(&mut app, &terrain, 7.0);
    assert!(
        !tab_dirty(&app, tab),
        "an edit outside history leaves the tab reading clean"
    );
    touch_scene_outside(&scene_path);
    let _ = app
        .world_mut()
        .operator("scene.open")
        .param("path", scene_path.to_string_lossy().into_owned())
        .call()
        .expect("scene.open dispatches");
    app.update();
    settle_terrain(&mut app);

    assert!(
        same_heights(&heights(&app, &data_path), &sculpted),
        "reopening the scene reread it over unsaved terrain edits"
    );
}

/// Choosing to reload takes the terrain the file holds.
#[test]
fn reloading_after_a_terrain_file_changed_takes_the_file() {
    let stage = stage_dir();
    let scene_path = author_and_save(stage.path());
    let (mut app, entity, data_path, tab) = reopen_aimed(&scene_path);

    sculpt_stroke(&mut app, entity);
    let on_disk = rewrite_sidecar_on_disk(&app, &scene_path, &data_path, 25.0);
    leave_and_return(&mut app, tab);

    let sidecar = sidecar_file(&scene_path, &data_path);
    let prompt = terrain_prompt(&app, &sidecar)
        .expect("the user was not told the terrain file changed on disk");
    answer_external_change(app.world_mut(), &prompt.path, ExternalReloadChoice::Reload);
    settle_terrain(&mut app);

    let (_, terrain) = the_terrain(&mut app);
    assert!(
        same_heights(&heights(&app, &terrain.data_path), &on_disk),
        "reloading did not take the terrain the file holds"
    );
    assert!(
        reload_prompts(&app).is_empty(),
        "the reload left a prompt behind"
    );
}

/// Switch tabs the way the tab strip does.
fn swap_to(app: &mut App, target: usize) {
    jackdaw_editor_core::scenes::swap::swap_active_tab(app.world_mut(), target);
    app.update();
}

/// A scatter run survives a save and a reload, and running it again re-stamps the
/// same group instead of placing a second copy. The placements live in the
/// sidecar, and the group key is what makes the second run replace them.
#[test]
fn scatter_groups_round_trip_and_a_rescatter_does_not_duplicate() {
    let stage = stage_dir();
    let scene_path = author_and_save(stage.path());

    let mut app = reopen(&scene_path);
    let (entity, _) = the_terrain(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![entity];
    scatter(&mut app);

    let placed = placements(&mut app);
    assert!(placed > 0, "the run placed nothing to round-trip");
    let key = group_key(&mut app).expect("the run stamped a group");

    // `reopen` loads into a fresh tab, which holds no path of its own; a
    // save with none asks for one instead of writing.
    {
        let mut scenes = app.world_mut().resource_mut::<Scenes>();
        let active = scenes.active;
        scenes.tabs[active].path = Some(scene_path.clone());
    }
    dispatch(&mut app, "scene.save");
    app.update();

    let mut reloaded = reopen(&scene_path);
    assert_eq!(
        group_key(&mut reloaded).as_deref(),
        Some(key.as_str()),
        "the saved sidecar carried no scatter group"
    );
    assert_eq!(
        placements(&mut reloaded),
        placed,
        "the reloaded scene is missing placements the run stored"
    );

    let (entity, _) = the_terrain(&mut reloaded);
    reloaded.world_mut().resource_mut::<Selection>().entities = vec![entity];
    scatter(&mut reloaded);

    assert_eq!(
        placements(&mut reloaded),
        placed,
        "the second run duplicated the scatter instead of replacing it"
    );
}

/// One scatter run over the selected terrain, with a palette the operator
/// call carries so the test does not depend on panel state.
fn scatter(app: &mut App) {
    let result = app
        .world_mut()
        .operator("terrain.scatter")
        .param("assets", "kit/Tree.gltf")
        .param("density", 0.000_02_f64)
        .call()
        .expect("terrain.scatter dispatch errored");
    assert_eq!(result, OperatorResult::Finished);
    app.update();
}

/// Placements the terrain's document holds.
fn placements(app: &mut App) -> usize {
    let (_, terrain) = the_terrain(app);
    app.world()
        .resource::<TerrainDataStore>()
        .get(&terrain.data_path)
        .map_or(0, jackdaw_terrain::RegionTerrainData::placement_count)
}

/// The one stamp identity the terrain's document holds placements for.
fn group_key(app: &mut App) -> Option<String> {
    let (_, terrain) = the_terrain(app);
    jackdaw_editor_terrain::scatter_data::group_counts(
        app.world().resource::<TerrainDataStore>(),
        &terrain.data_path,
    )
    .into_iter()
    .next()
    .map(|(key, _)| key)
}

/// The colour layer and the surface block are both sidecar data, so a stroke and
/// a dial have to survive a save and a reopen.
#[test]
fn the_tint_layer_and_its_dials_survive_a_save_and_reload() {
    let stage = stage_dir();
    let scene_path = author_and_save(stage.path());
    let mut app = reopen(&scene_path);

    let (entity, terrain) = the_terrain(&mut app);
    let data_path = terrain.data_path.clone();
    let resolution = app
        .world()
        .resource::<TerrainDataStore>()
        .grid_shape(&terrain)
        .resolution;
    let far = (FAR * resolution + FAR) as usize;

    assert_eq!(
        app.world().resource::<TerrainDataStore>().tint(&data_path)[far],
        [255, 255, 255, 255],
        "an untinted terrain reads as white everywhere"
    );

    app.world_mut()
        .resource_mut::<TerrainBrushSettings>()
        .radius = 8.0;
    *app.world_mut().resource_mut::<TerrainEditMode>() = TerrainEditMode::Paint;
    {
        let mut paint = app.world_mut().resource_mut::<TerrainPaintState>();
        paint.domain = PaintDomain::Color;
        paint.target = Some(entity);
        paint.tint_color = [40, 90, 200];
        paint.tint_opacity = 1.0;
        paint.tint_hardness = 1.0;
        paint.brush_position = Some(Vec2::splat(FAR as f32));
    }
    start_stroke(&mut app, "terrain.paint");
    let painted = app.world().resource::<TerrainDataStore>().tint(&data_path)[far];
    assert_ne!(
        painted,
        [255, 255, 255, 255],
        "the tint stroke did not reach cell ({FAR}, {FAR})"
    );
    end_stroke(&mut app);
    *app.world_mut().resource_mut::<TerrainEditMode>() = TerrainEditMode::None;

    // The stroke's own undo entry takes it back, and redoing puts it on
    // again: a colour stroke is one history entry like every other stroke.
    dispatch(&mut app, "history.undo");
    app.update();
    assert_eq!(
        app.world().resource::<TerrainDataStore>().tint(&data_path)[far],
        [255, 255, 255, 255],
        "undoing the stroke left tint behind"
    );
    dispatch(&mut app, "history.redo");
    app.update();
    assert_eq!(
        app.world().resource::<TerrainDataStore>().tint(&data_path)[far],
        painted
    );

    app.world_mut().resource_mut::<Selection>().entities = vec![entity];
    let result = app
        .world_mut()
        .operator("terrain.tint.strength")
        .param("value", 0.4)
        .call()
        .expect("terrain.tint.strength dispatches");
    assert_eq!(result, OperatorResult::Finished);
    app.update();

    aim_tab_at(&mut app, &scene_path);
    dispatch(&mut app, "scene.save");
    app.update();

    let reopened = reopen(&scene_path);
    let store = reopened.world().resource::<TerrainDataStore>();
    assert_eq!(
        store.tint(&data_path)[far],
        painted,
        "the sidecar did not carry the colour layer back"
    );
    assert_eq!(
        store.surface(&data_path).tint_strength,
        0.4,
        "the sidecar did not carry the surface block back"
    );
}
