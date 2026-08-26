//! What a terrain's tools reach after the scene it lives in has been saved and
//! opened again.
//!
//! A terrain's extent is its regions', not anything the component declares, and
//! the component's two extent fields are drained back to their defaults on
//! every load. A tool that strides the grid by those fields works while
//! spawning has left them large and addresses only a 256-cell corner once the
//! scene is reloaded. These run through the save and load paths and then edit
//! past cell 255.

use std::path::PathBuf;
use std::time::Duration;

use bevy::input::ButtonInput;
use bevy::prelude::*;
use jackdaw::scenes::Scenes;
use jackdaw::selection::Selection;
use jackdaw::terrain::{
    PaintDomain, TerrainBrushSettings, TerrainDataStore, TerrainEditMode, TerrainPaintState,
    TerrainSculptState,
};
use jackdaw_api::prelude::*;

mod util;

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

/// Give every terrain in the world its sidecar path and its dirty-chunk
/// tracking.
///
/// The editor's schedule runs these every frame gated on `AppState::Editor`,
/// which a headless test never enters, and without them a terrain has no
/// document to edit.
fn settle_terrain(app: &mut App) {
    app.world_mut()
        .run_system_cached(jackdaw::terrain::ensure_terrain_dirty_chunks)
        .expect("dirty-chunk tracking is installed");
    app.world_mut()
        .run_system_cached(jackdaw::terrain::ensure_terrain_data_path)
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
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let stage = tempfile::Builder::new()
        .prefix("terrain-reload-")
        .tempdir_in(root.join("target"))
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
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), scene_path);
    settle_terrain(&mut app);
    app
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

    // --- Painting a texture reaches the far cell.
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

    // --- Sculpting reaches the far cell.
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

    // --- Eroding works the whole grid, not a wrapped band of it.
    //
    // Striding the real heights by a smaller resolution walks a window over the
    // front of the array and leaves everything past it unchanged, so the
    // signature to rule out is nothing beyond row `resolution` moving. Droplets
    // land at random, so this counts cells rather than pinning any one of them.
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
