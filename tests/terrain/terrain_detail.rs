//! Authoring a terrain's detail layers: adding them, what each one draws, and
//! the density brush that decides where it grows.

use std::time::Duration;

use bevy::input::ButtonInput;
use bevy::prelude::*;
use jackdaw::selection::Selection;
use jackdaw::terrain::{
    PaintDomain, TerrainBrushSettings, TerrainDataStore, TerrainEditMode, TerrainPaintState,
};
use jackdaw_api::prelude::*;
use jackdaw_scene_types::PropertyValue;

use crate::util;

/// A cell well inside the footprint generating lays down.
const AIMED: u32 = 300;

#[track_caller]
fn dispatch(app: &mut App, id: &'static str) {
    let result = app
        .world_mut()
        .operator(id)
        .call()
        .unwrap_or_else(|err| panic!("{id} dispatch errored: {err}"));
    assert_eq!(result, OperatorResult::Finished, "{id} did not finish");
}

fn advance_a_frame(app: &mut App) {
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(Duration::from_millis(16));
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
    advance_a_frame(&mut app);
    app
}

/// A generated terrain that already carries one grass layer, projected into
/// what the renderer seeds from.
fn layered_app() -> App {
    let mut app = terrain_app();
    add_layer(&mut app, None);
    project_detail(&mut app);
    app
}

/// Fold the store and the layers into the source the renderer seeds tiles from.
/// The editor runs this every frame in a state a headless app does not enter.
fn project_detail(app: &mut App) {
    app.world_mut()
        .run_system_cached(jackdaw::terrain::detail::sync_terrain_detail)
        .expect("the detail projection is installed");
}

#[track_caller]
fn add_layer(app: &mut App, name: Option<&'static str>) {
    let mut call = app.world_mut().operator("terrain.detail.add");
    if let Some(name) = name {
        call = call.param("name", name);
    }
    let result = call.call().expect("terrain.detail.add dispatches");
    assert_eq!(result, OperatorResult::Finished);
    app.update();
}

/// The density a named channel carries at one cell.
fn density_at(
    app: &App,
    terrain: &jackdaw_scene_types::Terrain,
    channel: &str,
    x: u32,
    z: u32,
) -> u16 {
    let index = terrain
        .channels
        .iter()
        .position(|declared| declared.name == channel)
        .unwrap_or_else(|| panic!("the terrain declares a {channel:?} channel"));
    app.world()
        .resource::<TerrainDataStore>()
        .get(&terrain.data_path)
        .expect("the terrain has a document")
        .regions
        .channel_at(index, x as i32, z as i32)
}

/// The terrain-local metres of grid cell `(x, z)`, which the stamp takes.
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

#[track_caller]
fn set_field(app: &mut App, field: &'static str, value: &str) -> OperatorResult {
    let result = app
        .world_mut()
        .operator("terrain.detail.set")
        .param("field", field)
        .param("value", value.to_string())
        .call()
        .expect("terrain.detail.set dispatches");
    app.update();
    result
}

/// Lay a full-strength stamp on one layer at the aimed cell.
#[track_caller]
fn stamp(app: &mut App, layer: Option<&'static str>, erase: bool) -> OperatorResult {
    let (_, terrain) = the_terrain(app);
    let (mx, mz) = metres_at(app, &terrain, AIMED, AIMED);
    let mut call = app
        .world_mut()
        .operator("terrain.detail.stamp")
        .param("x", mx)
        .param("z", mz)
        .param("radius", 12.0)
        .param("opacity", 1.0)
        .param("erase", erase);
    if let Some(layer) = layer {
        call = call.param("layer", layer);
    }
    let result = call.call().expect("terrain.detail.stamp dispatches");
    app.update();
    result
}

#[test]
fn adding_a_layer_mints_its_density_channel_and_removing_the_layer_keeps_it() {
    let mut app = terrain_app();
    let (_, terrain) = the_terrain(&mut app);
    assert!(
        terrain.detail.is_empty(),
        "a fresh terrain carries no layers"
    );
    assert!(
        !terrain.channels.iter().any(|c| c.name == "grass"),
        "and declares no density channel"
    );

    add_layer(&mut app, None);
    let (_, terrain) = the_terrain(&mut app);
    let layer = terrain
        .detail
        .first()
        .cloned()
        .expect("the layer was added");
    assert_eq!(layer.name, "grass");
    assert_eq!(layer.density_channel, "grass");
    assert_eq!(
        terrain
            .channels
            .iter()
            .filter(|c| c.name == layer.density_channel)
            .count(),
        1,
        "the density channel was minted exactly once"
    );

    dispatch(&mut app, "terrain.detail.remove");
    app.update();
    let (_, terrain) = the_terrain(&mut app);
    assert!(terrain.detail.is_empty(), "the layer is gone");
    assert!(
        terrain.channels.iter().any(|c| c.name == "grass"),
        "removing a layer must keep what was painted into its channel"
    );
}

#[test]
fn undoing_the_add_takes_back_both_the_layer_and_the_channel() {
    let mut app = layered_app();
    dispatch(&mut app, "history.undo");
    app.update();

    let (_, terrain) = the_terrain(&mut app);
    assert!(terrain.detail.is_empty(), "one undo took the layer back");
    assert!(
        !terrain.channels.iter().any(|c| c.name == "grass"),
        "the same undo took the channel it minted back"
    );
}

#[test]
fn a_second_layer_grows_from_a_channel_of_its_own() {
    let mut app = layered_app();
    add_layer(&mut app, Some("flowers"));

    let (_, terrain) = the_terrain(&mut app);
    let names: Vec<&str> = terrain
        .detail
        .iter()
        .map(|layer| layer.density_channel.as_str())
        .collect();
    assert_eq!(names, ["grass", "flowers"]);
    for channel in names {
        assert!(
            terrain.channels.iter().any(|c| c.name == channel),
            "{channel} was minted for its layer"
        );
    }
}

#[test]
fn each_stamp_reaches_only_the_layer_it_names() {
    let mut app = layered_app();
    add_layer(&mut app, Some("flowers"));

    assert_eq!(
        stamp(&mut app, Some("grass"), false),
        OperatorResult::Finished
    );
    let (_, terrain) = the_terrain(&mut app);
    assert!(density_at(&app, &terrain, "grass", AIMED, AIMED) > 0);
    assert_eq!(
        density_at(&app, &terrain, "flowers", AIMED, AIMED),
        0,
        "a stamp on one layer left the other layer's channel alone"
    );

    assert_eq!(
        stamp(&mut app, Some("flowers"), false),
        OperatorResult::Finished
    );
    let (_, terrain) = the_terrain(&mut app);
    assert!(density_at(&app, &terrain, "flowers", AIMED, AIMED) > 0);
}

#[test]
fn a_layer_is_addressed_by_name_or_by_index() {
    let mut app = layered_app();
    add_layer(&mut app, Some("flowers"));

    assert_eq!(stamp(&mut app, Some("0"), false), OperatorResult::Finished);
    let (_, terrain) = the_terrain(&mut app);
    assert!(
        density_at(&app, &terrain, "grass", AIMED, AIMED) > 0,
        "index 0 addressed the first layer"
    );

    assert_eq!(
        stamp(&mut app, Some("flowers"), false),
        OperatorResult::Finished
    );
    let (_, terrain) = the_terrain(&mut app);
    assert!(
        density_at(&app, &terrain, "flowers", AIMED, AIMED) > 0,
        "a name addressed the second layer"
    );

    assert_eq!(
        stamp(&mut app, Some("meadow"), false),
        OperatorResult::Cancelled,
        "a name no layer carries is refused"
    );
}

#[test]
fn an_index_addresses_a_layer_before_a_name_that_reads_as_one() {
    let mut app = layered_app();
    add_layer(&mut app, Some("0"));
    let (_, terrain) = the_terrain(&mut app);
    assert_eq!(terrain.detail[1].name, "0");
    assert_eq!(terrain.detail[1].density_channel, "0");

    assert_eq!(stamp(&mut app, Some("1"), false), OperatorResult::Finished);
    let (_, terrain) = the_terrain(&mut app);
    assert!(
        density_at(&app, &terrain, "0", AIMED, AIMED) > 0,
        "layer=1 reached the second layer"
    );
    assert_eq!(density_at(&app, &terrain, "grass", AIMED, AIMED), 0);

    assert_eq!(stamp(&mut app, Some("0"), false), OperatorResult::Finished);
    let (_, terrain) = the_terrain(&mut app);
    assert!(
        density_at(&app, &terrain, "grass", AIMED, AIMED) > 0,
        "layer=0 reached the first layer, not the layer named 0"
    );
}

#[test]
fn two_layers_asked_for_one_name_get_names_of_their_own() {
    let mut app = layered_app();
    add_layer(&mut app, Some("grass"));

    let (_, terrain) = the_terrain(&mut app);
    let names: Vec<&str> = terrain
        .detail
        .iter()
        .map(|layer| layer.name.as_str())
        .collect();
    assert_eq!(names, ["grass", "grass-1"], "a name addresses one layer");
    assert_eq!(terrain.detail[1].density_channel, "grass-1");

    assert_eq!(
        stamp(&mut app, Some("grass-1"), false),
        OperatorResult::Finished
    );
    let (_, terrain) = the_terrain(&mut app);
    assert!(density_at(&app, &terrain, "grass-1", AIMED, AIMED) > 0);
    assert_eq!(
        density_at(&app, &terrain, "grass", AIMED, AIMED),
        0,
        "the second layer grows from a channel of its own"
    );
}

#[test]
fn removing_a_layer_keeps_the_selection_on_the_layer_it_was_on() {
    let mut app = layered_app();
    add_layer(&mut app, Some("flowers"));
    add_layer(&mut app, Some("moss"));
    assert_eq!(app.world().resource::<TerrainPaintState>().detail_layer, 2);

    let removed = app
        .world_mut()
        .operator("terrain.detail.remove")
        .param("layer", "flowers")
        .call()
        .expect("terrain.detail.remove dispatches");
    assert_eq!(removed, OperatorResult::Finished);
    app.update();

    assert_eq!(
        app.world().resource::<TerrainPaintState>().detail_layer,
        1,
        "the selection followed moss into the slot flowers left"
    );
    assert_eq!(stamp(&mut app, None, false), OperatorResult::Finished);
    let (_, terrain) = the_terrain(&mut app);
    assert!(
        density_at(&app, &terrain, "moss", AIMED, AIMED) > 0,
        "an unqualified stamp still reaches the layer that was selected"
    );
    assert_eq!(density_at(&app, &terrain, "grass", AIMED, AIMED), 0);
}

#[test]
fn undoing_an_add_leaves_the_selection_inside_the_list() {
    let mut app = layered_app();
    add_layer(&mut app, Some("flowers"));
    assert_eq!(app.world().resource::<TerrainPaintState>().detail_layer, 1);

    dispatch(&mut app, "history.undo");
    app.update();
    let (_, terrain) = the_terrain(&mut app);
    assert_eq!(terrain.detail.len(), 1, "the second layer is gone");
    assert_eq!(
        app.world().resource::<TerrainPaintState>().detail_layer,
        0,
        "the selection came back inside the list"
    );

    assert_eq!(
        set_field(&mut app, "cull_distance", "80"),
        OperatorResult::Finished,
        "an operator with no layer= still finds the selected layer"
    );
    let (_, terrain) = the_terrain(&mut app);
    assert_eq!(terrain.detail[0].cull_distance, 80.0);
}

#[test]
fn a_detail_stamp_thickens_the_density_and_undoes() {
    let mut app = layered_app();
    let (_, terrain) = the_terrain(&mut app);
    assert_eq!(density_at(&app, &terrain, "grass", AIMED, AIMED), 0);

    assert_eq!(stamp(&mut app, None, false), OperatorResult::Finished);
    let grown = density_at(&app, &terrain, "grass", AIMED, AIMED);
    assert!(
        grown > 0,
        "the stamp grew nothing at the cell it was aimed at"
    );
    assert_eq!(
        density_at(&app, &terrain, "grass", 0, 0),
        0,
        "and nothing well outside its radius"
    );

    dispatch(&mut app, "history.undo");
    app.update();
    assert_eq!(
        density_at(&app, &terrain, "grass", AIMED, AIMED),
        0,
        "undo left {grown} density standing"
    );
}

#[test]
fn a_stamp_and_its_undo_mark_only_the_cells_they_moved() {
    let mut app = layered_app();
    let (entity, terrain) = the_terrain(&mut app);
    let resolution = app
        .world()
        .resource::<TerrainDataStore>()
        .grid_shape(&terrain)
        .resolution;
    clear_detail_mark(&mut app, entity);

    let (mx, mz) = metres_at(&app, &terrain, AIMED, AIMED);
    let stamped = app
        .world_mut()
        .operator("terrain.detail.stamp")
        .param("x", mx)
        .param("z", mz)
        .param("radius", 12.0)
        .param("opacity", 1.0)
        .call()
        .expect("terrain.detail.stamp dispatches");
    assert_eq!(stamped, OperatorResult::Finished);

    let marked = detail_mark(&app, entity).expect("the stamp marked the ground it covered");
    assert!(
        marked.width < resolution && marked.height < resolution,
        "the stamp marked {}x{} of a {resolution}-cell grid",
        marked.width,
        marked.height
    );
    assert!(
        marked.x <= AIMED && AIMED < marked.x + marked.width,
        "the mark misses the cell the stamp was aimed at"
    );

    clear_detail_mark(&mut app, entity);
    dispatch(&mut app, "history.undo");
    let undone = detail_mark(&app, entity).expect("the undo marked the ground it put back");
    assert!(
        undone.width < resolution && undone.height < resolution,
        "undoing one stamp reseeded the whole {resolution}-cell field"
    );
    assert!(
        undone.x <= AIMED && AIMED < undone.x + undone.width,
        "the undo's mark misses the cell the stamp was aimed at"
    );
}

#[test]
fn undoing_a_sculpt_marks_the_ground_the_detail_stands_on() {
    let mut app = layered_app();
    let (entity, terrain) = the_terrain(&mut app);
    let (mx, mz) = metres_at(&app, &terrain, AIMED, AIMED);
    let sculpted = app
        .world_mut()
        .operator("terrain.sculpt.stamp")
        .param("x", mx)
        .param("z", mz)
        .param("radius", 12.0)
        .param("strength", 4.0)
        .call()
        .expect("terrain.sculpt.stamp dispatches");
    assert_eq!(sculpted, OperatorResult::Finished);
    app.update();

    clear_detail_mark(&mut app, entity);
    dispatch(&mut app, "history.undo");
    let undone = detail_mark(&app, entity).expect("the undo marked the ground it put back");
    assert!(
        undone.x <= AIMED && AIMED < undone.x + undone.width,
        "the undo's mark misses the cell the sculpt raised"
    );
}

/// The ground a terrain's detail has yet to catch up with.
fn detail_mark(app: &App, entity: Entity) -> Option<jackdaw_terrain::GridRect> {
    app.world()
        .get::<jackdaw_terrain::render::DetailDirty>(entity)
        .expect("a terrain that carries layers carries a mark")
        .rect
}

fn clear_detail_mark(app: &mut App, entity: Entity) {
    app.world_mut()
        .get_mut::<jackdaw_terrain::render::DetailDirty>(entity)
        .expect("a terrain that carries layers carries a mark")
        .rect = None;
}

#[test]
fn the_stamp_erases_what_it_grew() {
    let mut app = layered_app();
    assert_eq!(stamp(&mut app, None, false), OperatorResult::Finished);
    let (_, terrain) = the_terrain(&mut app);
    assert!(
        density_at(&app, &terrain, "grass", AIMED, AIMED) > 0,
        "the first stamp grew something for the second to erase"
    );

    assert_eq!(stamp(&mut app, None, true), OperatorResult::Finished);
    let (_, terrain) = the_terrain(&mut app);
    assert_eq!(density_at(&app, &terrain, "grass", AIMED, AIMED), 0);
}

#[test]
fn the_brush_refuses_a_terrain_with_no_layers() {
    let mut app = terrain_app();
    assert_eq!(stamp(&mut app, None, false), OperatorResult::Cancelled);
    assert_eq!(
        set_field(&mut app, "width", "0.2,0.4"),
        OperatorResult::Cancelled
    );

    let painted = app
        .world_mut()
        .operator("terrain.detail.paint")
        .call()
        .expect("terrain.detail.paint dispatches");
    assert_eq!(painted, OperatorResult::Cancelled);
    assert_ne!(
        app.world().resource::<TerrainPaintState>().domain,
        PaintDomain::Detail,
        "a refused brush must not have been loaded"
    );
}

#[test]
fn setting_a_field_changes_the_layer_and_an_unknown_field_is_refused() {
    let mut app = layered_app();
    assert_eq!(
        set_field(&mut app, "cull_distance", "80"),
        OperatorResult::Finished
    );
    assert_eq!(
        set_field(&mut app, "color_tip", "0.2,0.9,0.1"),
        OperatorResult::Finished
    );
    let (_, terrain) = the_terrain(&mut app);
    let layer = terrain.detail[0].clone();
    assert_eq!(layer.cull_distance, 80.0);
    assert_eq!(layer.color_tip, [0.2, 0.9, 0.1]);
    assert_eq!(
        layer.color_base,
        jackdaw_scene_types::DetailLayer::default().color_base,
        "one field at a time: the rest of the layer is untouched"
    );

    assert_eq!(
        set_field(&mut app, "colour", "0.2,0.9,0.1"),
        OperatorResult::Cancelled
    );
    assert_eq!(
        set_field(&mut app, "cull_distance", "far"),
        OperatorResult::Cancelled
    );
    let (_, terrain) = the_terrain(&mut app);
    assert_eq!(
        terrain.detail[0].cull_distance, 80.0,
        "a refusal writes nothing"
    );
}

#[test]
fn the_mesh_field_takes_the_card_or_a_model_and_refuses_a_missing_file() {
    let mut app = layered_app();
    let model = "models/dungeon.glb";
    assert!(
        std::path::Path::new("assets").join(model).is_file(),
        "the fixture model is where the operator looks for it"
    );

    assert_eq!(set_field(&mut app, "mesh", model), OperatorResult::Finished);
    let (_, terrain) = the_terrain(&mut app);
    assert_eq!(
        terrain.detail[0].varieties[0].mesh,
        jackdaw_scene_types::DetailMesh::Asset(model.to_string())
    );

    assert_eq!(
        set_field(&mut app, "mesh", "models/absent.gltf"),
        OperatorResult::Cancelled
    );
    let (_, terrain) = the_terrain(&mut app);
    assert_eq!(
        terrain.detail[0].varieties[0].mesh,
        jackdaw_scene_types::DetailMesh::Asset(model.to_string()),
        "a refusal writes nothing"
    );

    assert_eq!(
        set_field(&mut app, "mesh", "card"),
        OperatorResult::Finished
    );
    let (_, terrain) = the_terrain(&mut app);
    assert_eq!(
        terrain.detail[0].varieties[0].mesh,
        jackdaw_scene_types::DetailMesh::Card
    );
}

#[track_caller]
fn call(
    app: &mut App,
    id: &'static str,
    params: &[(&'static str, PropertyValue)],
) -> OperatorResult {
    let mut call = app.world_mut().operator(id);
    for (key, value) in params {
        call = call.param(*key, value.clone());
    }
    let result = call.call().expect("the operator dispatches");
    app.update();
    result
}

/// A layer grows several meshes: adding one takes a share of the instances,
/// each keeps its own weight and tint, the last one stays, and one undo takes
/// an add back.
#[test]
fn a_layer_gains_and_loses_meshes_as_single_undo_entries() {
    let mut app = layered_app();
    let model = "models/dungeon.glb";

    assert_eq!(
        call(
            &mut app,
            "terrain.detail.variety.add",
            &[("mesh", model.into()), ("weight", 3.0.into())],
        ),
        OperatorResult::Finished
    );
    let (_, terrain) = the_terrain(&mut app);
    let varieties = &terrain.detail[0].varieties;
    assert_eq!(varieties.len(), 2);
    assert_eq!(
        varieties[1].mesh,
        jackdaw_scene_types::DetailMesh::Asset(model.to_string())
    );
    assert_eq!(varieties[1].weight, 3.0);

    assert_eq!(
        call(
            &mut app,
            "terrain.detail.set",
            &[
                ("field", "tint".into()),
                ("value", "0.5,0.9,0.5".into()),
                ("variety", 1_i64.into()),
            ],
        ),
        OperatorResult::Finished
    );
    let (_, terrain) = the_terrain(&mut app);
    assert_eq!(terrain.detail[0].varieties[1].tint, [0.5, 0.9, 0.5]);
    assert_eq!(
        terrain.detail[0].varieties[0].tint,
        [1.0, 1.0, 1.0],
        "the other variety keeps its own look"
    );

    assert_eq!(
        call(
            &mut app,
            "terrain.detail.variety.remove",
            &[("variety", 0_i64.into())],
        ),
        OperatorResult::Finished
    );
    assert_eq!(
        call(
            &mut app,
            "terrain.detail.variety.remove",
            &[("variety", 0_i64.into())],
        ),
        OperatorResult::Cancelled,
        "the last mesh stays"
    );
    let (_, terrain) = the_terrain(&mut app);
    assert_eq!(terrain.detail[0].varieties.len(), 1);

    dispatch(&mut app, "history.undo");
    app.update();
    let (_, terrain) = the_terrain(&mut app);
    assert_eq!(
        terrain.detail[0].varieties.len(),
        2,
        "one undo puts the removed mesh back"
    );
}

/// A layer from a scene written before layers carried meshes of their own
/// names one model; the editor reads it as one variety of that model and
/// writes the document back without the old field.
#[test]
fn a_layer_naming_one_model_is_folded_into_a_variety_of_it() {
    let mut app = layered_app();
    let model = "models/dungeon.glb";
    let (entity, _) = the_terrain(&mut app);
    app.world_mut()
        .get_mut::<jackdaw_scene_types::Terrain>(entity)
        .expect("a terrain")
        .detail[0]
        .mesh = jackdaw_scene_types::DetailMesh::Asset(model.to_string());
    app.world_mut()
        .run_system_cached(jackdaw::terrain::fold_single_meshes_into_varieties)
        .expect("the fold is installed");
    app.update();

    let (_, terrain) = the_terrain(&mut app);
    assert_eq!(
        terrain.detail[0].varieties,
        vec![jackdaw_scene_types::DetailVariety::of(
            jackdaw_scene_types::DetailMesh::Asset(model.to_string())
        )]
    );
    assert_eq!(
        terrain.detail[0].mesh,
        jackdaw_scene_types::DetailMesh::Card
    );
    let bsn = jackdaw_remote::bsn_methods::entity_bsn(app.world_mut(), entity)
        .expect("the terrain reports as BSN");
    assert!(
        bsn.contains("DetailVariety") && bsn.contains(model),
        "the document carries the variety:\n{bsn}"
    );
}

#[test]
fn the_reported_bsn_carries_the_detail_block() {
    let mut app = layered_app();
    assert_eq!(
        set_field(&mut app, "cull_distance", "83"),
        OperatorResult::Finished
    );
    let (entity, _) = the_terrain(&mut app);

    let bsn = jackdaw_remote::bsn_methods::entity_bsn(app.world_mut(), entity)
        .expect("the terrain reports as BSN");
    assert!(
        bsn.contains("DetailLayer"),
        "the reported BSN carries no detail layer:\n{bsn}"
    );
    assert!(
        bsn.contains("cull_distance: 83"),
        "the reported BSN carries no edited cull distance:\n{bsn}"
    );
}

#[test]
fn a_stroke_marks_the_ground_it_is_crossing_rather_than_all_it_has_crossed() {
    let mut app = layered_app();
    let (entity, _) = the_terrain(&mut app);
    load_detail_brush(&mut app, entity);

    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    advance_a_frame(&mut app);
    let running = app
        .world_mut()
        .operator("terrain.paint")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: false,
        })
        .call()
        .expect("terrain.paint dispatches");
    assert_eq!(running, OperatorResult::Running, "the stroke did not start");

    let mut crossed = AIMED;
    for _ in 0..3 {
        crossed += 40;
        clear_detail_mark(&mut app, entity);
        app.world_mut()
            .resource_mut::<TerrainPaintState>()
            .brush_position = Some(Vec2::splat(crossed as f32));
        advance_a_frame(&mut app);
        app.update();
    }

    let marked = detail_mark(&app, entity).expect("the frame marked the ground under the brush");
    assert!(
        marked.x + marked.width <= crossed + 16 && marked.x + 16 >= AIMED + 40,
        "a brush at cell {crossed} marked {}..{}, which is more than the ground it is on",
        marked.x,
        marked.x + marked.width
    );
}

/// Point the paint brush at `entity`'s selected layer, in the paint tool, with
/// a brush the viewport would give it.
fn load_detail_brush(app: &mut App, entity: Entity) {
    let loaded = app
        .world_mut()
        .operator("terrain.detail.paint")
        .param("opacity", 1.0)
        .param("erase", false)
        .call()
        .expect("terrain.detail.paint dispatches");
    assert_eq!(loaded, OperatorResult::Finished);
    *app.world_mut().resource_mut::<TerrainEditMode>() = TerrainEditMode::Paint;
    app.world_mut()
        .resource_mut::<TerrainBrushSettings>()
        .radius = 8.0;
    let mut paint = app.world_mut().resource_mut::<TerrainPaintState>();
    paint.target = Some(entity);
    paint.brush_position = Some(Vec2::splat(AIMED as f32));
}

#[test]
fn a_detail_stroke_thickens_the_density_under_the_brush() {
    let mut app = layered_app();
    let (entity, terrain) = the_terrain(&mut app);
    assert_eq!(density_at(&app, &terrain, "grass", AIMED, AIMED), 0);

    load_detail_brush(&mut app, entity);
    assert_eq!(
        app.world().resource::<TerrainPaintState>().domain,
        PaintDomain::Detail
    );

    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    advance_a_frame(&mut app);
    let running = app
        .world_mut()
        .operator("terrain.paint")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: false,
        })
        .call()
        .expect("terrain.paint dispatches");
    assert_eq!(running, OperatorResult::Running, "the stroke did not start");
    assert!(
        density_at(&app, &terrain, "grass", AIMED, AIMED) > 0,
        "the stroke grew nothing under the brush"
    );

    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    app.update();
    app.update();

    let grown = density_at(&app, &terrain, "grass", AIMED, AIMED);
    assert!(grown > 0, "releasing the button undid the stroke");
    dispatch(&mut app, "history.undo");
    app.update();
    assert_eq!(
        density_at(&app, &terrain, "grass", AIMED, AIMED),
        0,
        "one undo takes the whole stroke back"
    );
}
