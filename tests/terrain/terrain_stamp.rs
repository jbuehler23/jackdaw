//! The parametric brush stamps, which is how a caller with no pointer sculpts and
//! paints. A stamp is the stroke without the drag: same kernel, and an undo entry
//! that covers the brushed rectangle and restores that alone.

use std::time::Duration;

use bevy::prelude::*;
use jackdaw::selection::Selection;
use jackdaw::terrain::TerrainDataStore;
use jackdaw_api::prelude::*;
use jackdaw_terrain::Control;

use crate::util;

/// What generating a fresh terrain lays down: four regions, 1024 cells
/// on a side, so two stamps can land in different regions.
const GENERATED_RESOLUTION: u32 = 1024;

#[track_caller]
fn dispatch(app: &mut App, id: &'static str) {
    let result = app
        .world_mut()
        .operator(id)
        .call()
        .unwrap_or_else(|err| panic!("{id} dispatch errored: {err}"));
    assert_eq!(result, OperatorResult::Finished, "{id} did not finish");
}

fn settle_terrain(app: &mut App) {
    app.world_mut()
        .run_system_cached(jackdaw::terrain::ensure_terrain_dirty_chunks)
        .expect("dirty-chunk tracking is installed");
    app.world_mut()
        .run_system_cached(jackdaw::terrain::ensure_terrain_data_path)
        .expect("sidecar paths are minted");
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

/// A generated terrain, selected, with its sidecar in the store.
fn terrain_app() -> App {
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
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(Duration::from_millis(16));

    let (_, terrain) = the_terrain(&mut app);
    assert_eq!(
        app.world()
            .resource::<TerrainDataStore>()
            .grid_shape(&terrain)
            .resolution,
        GENERATED_RESOLUTION,
        "the stamps below need the multi-region footprint generating lays down"
    );
    app
}

/// The terrain-local metres of grid cell `(x, z)`, which is what the
/// stamps take.
fn metres_at(app: &App, terrain: &jackdaw_scene_types::Terrain, x: u32, z: u32) -> (f64, f64) {
    let shape = app
        .world()
        .resource::<TerrainDataStore>()
        .grid_shape(terrain);
    let cell = shape.size / (shape.resolution - 1) as f32;
    (
        f64::from(shape.origin.x + cell.x * x as f32),
        f64::from(shape.origin.y + cell.y * z as f32),
    )
}

fn control_at(app: &App, data_path: &str, x: u32, z: u32) -> Control {
    let control = app
        .world()
        .resource::<TerrainDataStore>()
        .control(data_path);
    control[(z * GENERATED_RESOLUTION + x) as usize]
}

#[track_caller]
fn stamp(app: &mut App, x: f64, z: f64, slot: i64) {
    let result = app
        .world_mut()
        .operator("terrain.paint.stamp")
        .param("x", x)
        .param("z", z)
        .param("radius", 12.0)
        .param("slot", slot)
        .call()
        .expect("terrain.paint.stamp dispatches");
    assert_eq!(result, OperatorResult::Finished);
    app.update();
}

/// A stamp records the block it brushed, not the whole map: two dense copies of a
/// 4096-cell-edge terrain is 134 MiB per stamp, which exhausts the history budget
/// in a handful of edits.
#[test]
fn a_stamp_records_the_block_it_brushed_rather_than_the_whole_map() {
    let mut app = terrain_app();
    let (_, terrain) = the_terrain(&mut app);
    let (mx, mz) = metres_at(&app, &terrain, 200, 200);
    stamp(&mut app, mx, mz, 1);

    let cells = (GENERATED_RESOLUTION as usize).pow(2);
    let whole_map = cells * 2 * size_of::<Control>();
    let held = app
        .world()
        .resource::<jackdaw_commands::CommandHistory>()
        .undo_stack
        .last()
        .expect("the stamp recorded nothing to undo")
        .heap_bytes();
    assert!(held > 0, "the stamp's entry holds nothing to put back");
    assert!(
        held * 8 < whole_map,
        "the stamp's entry holds {held} bytes of a {whole_map}-byte whole-map snapshot"
    );
}

/// Undoing one stamp takes back that stamp and nothing else. The two land in
/// different regions, so an entry writing back more than its block would wipe the
/// first one's cells.
#[test]
fn undoing_a_paint_stamp_leaves_paint_elsewhere_alone() {
    let mut app = terrain_app();
    let (_, terrain) = the_terrain(&mut app);
    let data_path = terrain.data_path.clone();

    // Two cells far enough apart to sit in different regions of the
    // generated footprint, each well outside the other's 12 m brush.
    let (far_x, far_z) = (200, 200);
    let (near_x, near_z) = (800, 800);
    let pristine = control_at(&app, &data_path, far_x, far_z);

    let (mx, mz) = metres_at(&app, &terrain, far_x, far_z);
    stamp(&mut app, mx, mz, 1);
    let painted = control_at(&app, &data_path, far_x, far_z);
    assert_ne!(
        painted, pristine,
        "the first stamp painted nothing at ({far_x}, {far_z})"
    );

    let (mx, mz) = metres_at(&app, &terrain, near_x, near_z);
    stamp(&mut app, mx, mz, 2);
    assert_ne!(
        control_at(&app, &data_path, near_x, near_z),
        pristine,
        "the second stamp painted nothing at ({near_x}, {near_z})"
    );

    dispatch(&mut app, "history.undo");
    app.update();

    assert_eq!(
        control_at(&app, &data_path, far_x, far_z),
        painted,
        "undoing the second stamp erased the first one's paint"
    );
    assert_eq!(
        control_at(&app, &data_path, near_x, near_z),
        pristine,
        "the second stamp's own cells were not taken back"
    );
}

/// A sculpt stamp raises the ground where it is aimed, and one undo puts it back,
/// with an entry covering only the brushed rectangle.
#[test]
fn a_sculpt_stamp_raises_the_ground_and_undoes() {
    let mut app = terrain_app();
    let (_, terrain) = the_terrain(&mut app);
    let data_path = terrain.data_path.clone();
    let (cell_x, cell_z) = (300, 300);
    let index = (cell_z * GENERATED_RESOLUTION + cell_x) as usize;

    let before = app
        .world()
        .resource::<TerrainDataStore>()
        .heights(&data_path)[index];

    let (mx, mz) = metres_at(&app, &terrain, cell_x, cell_z);
    let result = app
        .world_mut()
        .operator("terrain.sculpt.stamp")
        .param("x", mx)
        .param("z", mz)
        .param("radius", 20.0)
        .param("strength", 5.0)
        .call()
        .expect("terrain.sculpt.stamp dispatches");
    assert_eq!(result, OperatorResult::Finished);
    app.update();

    let raised = app
        .world()
        .resource::<TerrainDataStore>()
        .heights(&data_path)[index];
    assert!(
        raised > before,
        "the stamp did not raise the ground: {before} -> {raised}"
    );

    dispatch(&mut app, "history.undo");
    app.update();
    let restored = app
        .world()
        .resource::<TerrainDataStore>()
        .heights(&data_path)[index];
    assert!(
        (restored - before).abs() < f32::EPSILON,
        "undo left the ground at {restored}, not {before}"
    );
}

/// The tint layer's own stamp, and its undo: same shape as the paint stamp.
mod tint {
    use super::*;

    fn color_at(app: &App, data_path: &str, x: u32, z: u32) -> [u8; 4] {
        app.world().resource::<TerrainDataStore>().tint(data_path)
            [(z * GENERATED_RESOLUTION + x) as usize]
    }

    #[track_caller]
    fn tint_stamp(app: &mut App, x: f64, z: f64, r: f64, g: f64, b: f64) {
        let result = app
            .world_mut()
            .operator("terrain.tint.stamp")
            .param("x", x)
            .param("z", z)
            .param("radius", 12.0)
            .param("opacity", 1.0)
            .param("hardness", 1.0)
            .param("r", r)
            .param("g", g)
            .param("b", b)
            .call()
            .expect("terrain.tint.stamp dispatches");
        assert_eq!(result, OperatorResult::Finished);
        app.update();
    }

    #[test]
    fn undoing_a_tint_stamp_leaves_tint_elsewhere_alone() {
        let mut app = terrain_app();
        let (_, terrain) = the_terrain(&mut app);
        let data_path = terrain.data_path.clone();

        let (far_x, far_z) = (200, 200);
        let (near_x, near_z) = (800, 800);
        assert_eq!(
            color_at(&app, &data_path, far_x, far_z),
            [255, 255, 255, 255],
            "an untinted terrain reads as white"
        );

        let (mx, mz) = metres_at(&app, &terrain, far_x, far_z);
        tint_stamp(&mut app, mx, mz, 1.0, 0.0, 0.0);
        let painted = color_at(&app, &data_path, far_x, far_z);
        assert_eq!(painted[..3], [255, 0, 0], "the first stamp tinted nothing");

        let (mx, mz) = metres_at(&app, &terrain, near_x, near_z);
        tint_stamp(&mut app, mx, mz, 0.0, 0.0, 1.0);
        assert_eq!(color_at(&app, &data_path, near_x, near_z)[..3], [0, 0, 255]);

        dispatch(&mut app, "history.undo");
        app.update();

        assert_eq!(
            color_at(&app, &data_path, far_x, far_z),
            painted,
            "undoing the second stamp erased the first one's tint"
        );
        assert_eq!(
            color_at(&app, &data_path, near_x, near_z),
            [255, 255, 255, 255],
            "the second stamp's own cells were not taken back"
        );
    }

    /// The variation wash covers the whole layer and comes back off it in
    /// one undo, which is what makes it safe to try a seed and change it.
    #[test]
    fn the_variation_wash_covers_the_layer_and_undoes() {
        let mut app = terrain_app();
        let (_, terrain) = the_terrain(&mut app);
        let data_path = terrain.data_path.clone();

        let result = app
            .world_mut()
            .operator("terrain.tint.variation")
            .param("seed", 5_i64)
            .param("frequency", 0.02)
            .param("amount", 0.2)
            .call()
            .expect("terrain.tint.variation dispatches");
        assert_eq!(result, OperatorResult::Finished);
        app.update();

        let varied: Vec<[u8; 4]> = app
            .world()
            .resource::<TerrainDataStore>()
            .tint(&data_path)
            .into_owned();
        assert!(
            varied.iter().any(|texel| *texel != [255, 255, 255, 255]),
            "the wash wrote nothing"
        );
        assert!(
            varied
                .iter()
                .all(|texel| 255 - texel[0] <= (0.2_f32 * 255.0).ceil() as u8),
            "the wash travelled further from white than the amount asked for"
        );

        dispatch(&mut app, "history.undo");
        app.update();
        assert!(
            app.world()
                .resource::<TerrainDataStore>()
                .tint(&data_path)
                .iter()
                .all(|texel| *texel == [255, 255, 255, 255]),
            "one undo must take the whole wash back off"
        );
    }

    /// The two surface dials reach the terrain document, which is what
    /// carries them into the sidecar and out again into a built game.
    #[test]
    fn the_surface_dials_reach_the_terrain_document() {
        let mut app = terrain_app();
        let (_, terrain) = the_terrain(&mut app);
        let data_path = terrain.data_path.clone();

        for (id, value) in [
            ("terrain.tint.strength", 0.25),
            ("terrain.material.blend_sharpness", 0.75),
        ] {
            let result = app
                .world_mut()
                .operator(id)
                .param("value", value)
                .call()
                .unwrap_or_else(|err| panic!("{id} dispatch errored: {err}"));
            assert_eq!(result, OperatorResult::Finished, "{id} did not finish");
            app.update();
        }

        let surface = app
            .world()
            .resource::<TerrainDataStore>()
            .surface(&data_path);
        assert_eq!(surface.tint_strength, 0.25);
        assert_eq!(surface.blend_sharpness, 0.75);

        // Out of range clamps rather than reaching the shader.
        let clamped = app
            .world_mut()
            .operator("terrain.tint.strength")
            .param("value", 9.0)
            .call()
            .expect("dispatches");
        assert_eq!(clamped, OperatorResult::Finished);
        app.update();
        assert_eq!(
            app.world()
                .resource::<TerrainDataStore>()
                .surface(&data_path)
                .tint_strength,
            1.0
        );
    }
}
