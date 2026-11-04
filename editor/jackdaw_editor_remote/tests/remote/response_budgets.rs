//! Byte budgets for the answers an agent reads most. `jd mcp` hands each BRP
//! result on as one compact JSON text block, so the length of the result's
//! compact JSON is what lands in the agent's context. A response that grows
//! past its budget fails here rather than quietly costing every session.

use base64::Engine as _;
use bevy::prelude::*;
use jackdaw_api_internal::operator::OperatorWorldExt as _;
use jackdaw_editor_core::project::{ProjectConfig, ProjectRoot};
use jackdaw_editor_core::screenshot::{ThumbnailFormat, write_thumbnail};
use jackdaw_editor_remote::server::{
    batch_handler, brush_topology_handler, call_operator_handler, operators_handler,
    scene_summary_handler, status_handler,
};
use serde_json::{Value, json};

use crate::util;
use crate::util::OperatorResultExt as _;

const STATUS_BUDGET: usize = 1024;
const CALL_BUDGET: usize = 300;
const RECIPE_RUN_BUDGET: usize = 300;
const DESCRIBE_BUDGET: usize = 2048;
const THUMBNAIL_BUDGET: usize = 60 * 1024;
const TOPOLOGY_BUDGET: usize = 4 * 1024;
const SUMMARY_BUDGET: usize = 12 * 1024;
const VIEWS_BUDGET: usize = 80 * 1024;

#[track_caller]
fn answer<M>(
    app: &mut App,
    handler: impl bevy::ecs::system::IntoSystem<In<Option<Value>>, bevy::remote::BrpResult, M> + 'static,
    params: Value,
) -> Value {
    let answer = app
        .world_mut()
        .run_system_cached_with(handler, Some(params))
        .expect("the handler ran")
        .unwrap_or_else(|err| panic!("the handler refused: {}", err.message));
    app.update();
    answer
}

/// The bytes a result costs as the text block `jd mcp` sends.
fn wire_len(value: &Value) -> usize {
    value.to_string().len()
}

#[track_caller]
fn assert_within(what: &str, value: &Value, budget: usize) {
    let len = wire_len(value);
    assert!(
        len < budget,
        "{what} is {len} bytes, over its {budget} byte budget: {value}"
    );
}

fn budget_scene() -> (App, tempfile::TempDir) {
    let dir = tempfile::Builder::new()
        .prefix("jackdaw-budgets-")
        .tempdir()
        .expect("temp dir");
    std::fs::create_dir_all(dir.path().join("assets")).expect("an assets dir");
    let mut app = util::editor_test_app();
    app.world_mut().insert_resource(ProjectRoot::new(
        dir.path().to_path_buf(),
        ProjectConfig::default(),
    ));
    app.world_mut()
        .operator("scene.new")
        .call()
        .expect("scene.new dispatches")
        .assert_finished();
    app.update();
    (app, dir)
}

#[test]
fn status_and_an_operator_call_stay_within_budget() {
    let (mut app, _project) = budget_scene();
    let added = answer(
        &mut app,
        call_operator_handler,
        json!({
            "id": "entity.add.cube",
            "params": { "name": "Wall_North_01", "size": [4.0, 3.0, 0.25], "pos": [0.0, 1.5, -6.0] },
        }),
    );
    assert_within("an entity.add.cube call", &added, CALL_BUDGET);

    let moved = answer(
        &mut app,
        call_operator_handler,
        json!({
            "id": "entity.set_transform",
            "params": { "entity": added["entities"][0], "x": 2.0, "y": 1.5, "z": -6.0 },
        }),
    );
    assert_within("an entity.set_transform call", &moved, CALL_BUDGET);

    let recipe = answer(
        &mut app,
        call_operator_handler,
        json!({
            "id": "recipe.run",
            "params": { "recipe": "house", "inputs": { "width": 7.5, "floors": 2 }, "pos": [12.0, 0.0, -4.0] },
        }),
    );
    assert_eq!(recipe["result"], json!("finished"), "{recipe}");
    assert_within("a recipe.run call", &recipe, RECIPE_RUN_BUDGET);

    let status = answer(&mut app, status_handler, json!({}));
    assert_within("status", &status, STATUS_BUDGET);
}

#[test]
fn every_operator_description_stays_within_budget() {
    let (mut app, _project) = budget_scene();
    let catalogue = answer(&mut app, operators_handler, json!({}));
    let operators = catalogue["operators"].as_array().expect("operators");
    assert!(operators.len() > 50, "{} operators", operators.len());
    let over: Vec<String> = operators
        .iter()
        .filter(|op| wire_len(op) >= DESCRIBE_BUDGET)
        .map(|op| format!("{} ({} bytes)", op["id"], wire_len(op)))
        .collect();
    assert!(
        over.is_empty(),
        "describe_operator answers over {DESCRIBE_BUDGET} bytes: {}",
        over.join(", ")
    );
}

#[test]
fn a_cube_topology_of_positions_and_polygons_stays_within_budget() {
    let (mut app, _project) = budget_scene();
    let added = answer(
        &mut app,
        call_operator_handler,
        json!({ "id": "entity.add.cube", "params": { "name": "Crate" } }),
    );
    let topology = answer(
        &mut app,
        brush_topology_handler,
        json!({ "entity": added["entities"][0], "fields": ["positions", "polygons"] }),
    );
    assert_eq!(topology["counts"]["faces"], json!(6), "{topology}");
    assert_within("a cube's brush_topology", &topology, TOPOLOGY_BUDGET);
}

#[test]
fn a_hundred_node_summary_stays_within_budget() {
    let (mut app, _project) = budget_scene();
    let calls: Vec<Value> = (0..100)
        .map(|index| {
            let (x, z) = ((index % 10) as f32 * 3.25, (index / 10) as f32 * -2.75);
            json!({
                "id": "entity.add.cube",
                "params": {
                    "name": format!("Fence_Post_{index:03}"),
                    "size": [0.25, 1.2 + (index % 3) as f32 * 0.1, 0.25],
                    "pos": [x + 0.125, 0.6, z - 0.375],
                },
            })
        })
        .collect();
    answer(&mut app, batch_handler, json!({ "calls": calls }));
    let summary = answer(&mut app, scene_summary_handler, json!({}));
    let rows = summary["rows"].as_array().expect("rows");
    assert!(rows.len() >= 100, "{} rows", rows.len());
    assert_within("a 100-node scene_summary", &summary, SUMMARY_BUDGET);
}

/// A stand-in for a 3D viewport capture: a sky gradient, a lit ground with
/// grid lines, shaded boxes and fine noise, at the editor's usual capture size.
fn viewport_like_frame() -> image::RgbImage {
    let (width, height) = (1377u32, 753u32);
    let horizon = height * 2 / 5;
    let boxes = [
        (180u32, 260u32, 420u32, 520u32, [176u8, 120, 84]),
        (520, 300, 760, 560, [140, 150, 160]),
        (820, 220, 1120, 600, [196, 172, 120]),
        (1150, 330, 1300, 540, [90, 120, 80]),
    ];
    image::RgbImage::from_fn(width, height, |x, y| {
        let noise = ((x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663)) % 9) as i32 - 4;
        let base = if let Some((x0, y0, x1, y1, color)) = boxes
            .iter()
            .find(|(x0, y0, x1, y1, _)| x >= *x0 && x < *x1 && y >= *y0 && y < *y1)
        {
            let light = 0.6 + 0.4 * (x - x0) as f32 / (x1 - x0) as f32;
            let shade = 1.0 - 0.3 * (y - y0) as f32 / (y1 - y0) as f32;
            color.map(|c| (c as f32 * light * shade) as i32)
        } else if y < horizon {
            let t = y as f32 / horizon as f32;
            [
                (110.0 + 80.0 * t) as i32,
                (160.0 + 60.0 * t) as i32,
                (230.0 - 10.0 * t) as i32,
            ]
        } else {
            let depth = (y - horizon) as f32 / (height - horizon) as f32;
            let line = x % (24 + (depth * 60.0) as u32) == 0 || (y - horizon) % 18 == 0;
            let g = if line {
                70
            } else {
                (90.0 + 60.0 * depth) as i32
            };
            [g - 10, g + 15, g - 20]
        };
        image::Rgb(base.map(|c| (c + noise).clamp(0, 255) as u8))
    })
}

#[test]
fn a_viewport_thumbnail_stays_within_budget() {
    let dir = tempfile::tempdir().expect("temp dir");
    let capture = dir.path().join("shot.png");
    let thumb = write_thumbnail(&viewport_like_frame(), &capture, 512, ThumbnailFormat::Jpeg)
        .expect("the thumbnail writes");
    assert_eq!(thumb.width, 512);
    let bytes = std::fs::read(&thumb.path).expect("the thumbnail reads");
    let encoded = base64::engine::general_purpose::STANDARD
        .encode(bytes)
        .len();
    assert!(
        encoded < THUMBNAIL_BUDGET,
        "a 512 px viewport thumbnail is {encoded} bytes of base64, over {THUMBNAIL_BUDGET}"
    );
}

/// One call to `views` on a single house returns a 512 px four-angle sheet
/// under its budget, and the viewport camera stays where it was. Each tile's
/// readback is a viewport-like frame scaled to the tile, as the renderer does
/// not run here.
#[test]
fn a_four_angle_views_sheet_of_a_house_stays_within_budget() {
    use super::views::{add_cube, editor_for_views, land_tiles, views_poll};

    let (mut app, camera, _project) = editor_for_views();
    let floor = add_cube(&mut app, "House_Floor", [6.0, 0.2, 5.0], [0.0, 0.1, 0.0]);
    for (name, size, pos) in [
        ("House_Wall_N", [6.0, 3.0, 0.25], [0.0, 1.7, -2.375]),
        ("House_Wall_S", [6.0, 3.0, 0.25], [0.0, 1.7, 2.375]),
        ("House_Wall_E", [0.25, 3.0, 5.0], [2.875, 1.7, 0.0]),
        ("House_Wall_W", [0.25, 3.0, 5.0], [-2.875, 1.7, 0.0]),
        ("House_Roof", [6.6, 0.3, 5.6], [0.0, 3.35, 0.0]),
    ] {
        add_cube(&mut app, name, size, pos);
    }
    let before = *app.world().get::<Transform>(camera).expect("a transform");
    let targets = json!([
        floor,
        "House_Wall_N",
        "House_Wall_S",
        "House_Wall_E",
        "House_Wall_W",
        "House_Roof"
    ]);
    let params = json!({ "targets": targets, "max_px": 512, "format": "jpeg", "request": "house" });
    assert!(views_poll(&mut app, &params).is_none());
    land_tiles(&mut app, 4, |edge| {
        image::DynamicImage::ImageRgb8(viewport_like_frame())
            .resize_exact(edge, edge, image::imageops::FilterType::Triangle)
            .to_rgb8()
    });
    let answer = views_poll(&mut app, &params).expect("every tile landed");
    assert_eq!(
        (answer["width"].clone(), answer["height"].clone()),
        (json!(512), json!(512))
    );
    assert_eq!(
        answer["tiles"].as_array().map(Vec::len),
        Some(4),
        "{answer}"
    );
    let bytes = std::fs::read(answer["path"].as_str().expect("a path")).expect("the sheet reads");
    let encoded = base64::engine::general_purpose::STANDARD
        .encode(&bytes)
        .len();
    assert!(
        encoded < VIEWS_BUDGET,
        "a 512 px four-angle sheet is {encoded} bytes of base64, over {VIEWS_BUDGET}"
    );
    assert_eq!(
        *app.world().get::<Transform>(camera).expect("a transform"),
        before,
        "the viewport camera moved"
    );
}
