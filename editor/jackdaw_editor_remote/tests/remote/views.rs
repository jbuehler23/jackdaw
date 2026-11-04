//! `jackdaw/views`: offscreen cameras framed on a part, read back into one
//! sheet. The renderer does not run here, so each tile's readback is stood in
//! for by a synthetic frame; what is checked is the request flow, the cameras'
//! framing, the sheet and that the viewport camera never moves.

use bevy::camera::visibility::RenderLayers;
use bevy::image::ToExtents as _;
use bevy::prelude::*;
use bevy::render::render_resource::{TextureDimension, TextureFormat};
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use jackdaw_api_internal::operator::OperatorWorldExt as _;
use jackdaw_editor_core::project::{ProjectConfig, ProjectRoot};
use jackdaw_editor_core::viewport::MainViewportCamera;
use jackdaw_editor_core::views::ViewTile;
use jackdaw_editor_remote::server::{call_operator_handler, views_handler};
use serde_json::{Value, json};

use crate::util;
use crate::util::OperatorResultExt as _;

/// An editor with a project, a scene and one viewport camera.
pub(super) fn editor_for_views() -> (App, Entity, tempfile::TempDir) {
    let dir = tempfile::Builder::new()
        .prefix("jackdaw-views-")
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
    let camera = app
        .world_mut()
        .spawn((
            MainViewportCamera,
            Camera3d::default(),
            Transform::from_xyz(3.0, 4.0, 9.0).looking_at(Vec3::ZERO, Vec3::Y),
        ))
        .id();
    app.world_mut()
        .resource_mut::<jackdaw_editor_core::viewport::ActiveViewport>()
        .camera = Some(camera);
    app.update();
    (app, camera, dir)
}

/// Add a cube through the remote and return its id.
pub(super) fn add_cube(app: &mut App, name: &str, size: [f32; 3], pos: [f32; 3]) -> u64 {
    let added = app
        .world_mut()
        .run_system_cached_with(
            call_operator_handler,
            Some(json!({
                "id": "entity.add.cube",
                "params": { "name": name, "size": size, "pos": pos },
            })),
        )
        .expect("the handler ran")
        .unwrap_or_else(|err| panic!("the cube was refused: {}", err.message));
    app.update();
    added["entities"][0].as_u64().expect("an entity id")
}

#[track_caller]
pub(super) fn views_poll(app: &mut App, params: &Value) -> Option<Value> {
    app.world_mut()
        .run_system_cached_with(views_handler, Some(params.clone()))
        .expect("the handler ran")
        .unwrap_or_else(|err| panic!("views refused: {}", err.message))
}

fn queued_readbacks(app: &mut App) -> Vec<Entity> {
    app.world_mut()
        .query_filtered::<Entity, With<Screenshot>>()
        .iter(app.world())
        .collect()
}

fn tile_cameras(app: &mut App) -> Vec<(Entity, usize)> {
    let mut tiles: Vec<(Entity, usize)> = app
        .world_mut()
        .query::<(Entity, &ViewTile)>()
        .iter(app.world())
        .map(|(entity, tile)| (entity, tile.index))
        .collect();
    tiles.sort_by_key(|(_, index)| *index);
    tiles
}

/// Run frames until all `count` tiles have queued their readback, then stand
/// in for each one with `frame(edge)`.
pub(super) fn land_tiles(app: &mut App, count: usize, frame: impl Fn(u32) -> image::RgbImage) {
    util::update_until(
        app,
        "every tile to queue its readback",
        std::time::Duration::from_secs(10),
        |app| queued_readbacks(app).len() == count,
    );
    for shot in queued_readbacks(app) {
        let edge = {
            let target = app
                .world()
                .get::<Screenshot>(shot)
                .and_then(|shot| match &shot.0 {
                    bevy::camera::RenderTarget::Image(image) => Some(image.handle.clone()),
                    _ => None,
                })
                .expect("a tile reads back an image");
            app.world()
                .resource::<Assets<Image>>()
                .get(&target)
                .expect("the tile image exists")
                .width()
        };
        let rgba: Vec<u8> = frame(edge)
            .pixels()
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
            .collect();
        let image = Image::new(
            UVec2::splat(edge).to_extents(),
            TextureDimension::D2,
            rgba,
            TextureFormat::Rgba8UnormSrgb,
            default(),
        );
        app.world_mut().trigger(ScreenshotCaptured {
            entity: shot,
            image,
        });
        app.world_mut().despawn(shot);
    }
}

fn flat(edge: u32) -> image::RgbImage {
    image::RgbImage::from_pixel(edge, edge, image::Rgb([120, 140, 160]))
}

/// The default call renders four angles onto a 512 px sheet, answers with
/// the tile layout once every tile is back, and the viewport camera stands
/// exactly where it stood.
#[test]
fn a_views_call_answers_with_a_four_angle_sheet_and_leaves_the_camera() {
    let (mut app, camera, _project) = editor_for_views();
    let house = add_cube(&mut app, "House", [4.0, 3.0, 5.0], [2.0, 1.5, -1.0]);
    let before = (
        *app.world().get::<Transform>(camera).expect("a transform"),
        format!("{:?}", app.world().get::<Projection>(camera)),
    );

    let params = json!({ "targets": [house], "request": "house" });
    assert!(views_poll(&mut app, &params).is_none());
    assert_eq!(tile_cameras(&mut app).len(), 4);
    land_tiles(&mut app, 4, flat);
    let answer = views_poll(&mut app, &params).expect("every tile landed");

    assert_eq!(answer["width"], json!(512), "{answer}");
    assert_eq!(answer["height"], json!(512), "{answer}");
    let labels: Vec<&str> = answer["tiles"]
        .as_array()
        .expect("tiles")
        .iter()
        .map(|tile| tile["label"].as_str().expect("a label"))
        .collect();
    assert_eq!(labels, ["front", "right", "top", "iso"]);
    assert_eq!(answer["tiles"][3]["at"], json!([256, 256, 256, 256]));
    let path = answer["path"].as_str().expect("a sheet path");
    assert!(path.ends_with(".jpg"), "{answer}");
    let sheet = image::open(path).expect("the sheet decodes");
    assert_eq!((sheet.width(), sheet.height()), (512, 512));

    let after = (
        *app.world().get::<Transform>(camera).expect("a transform"),
        format!("{:?}", app.world().get::<Projection>(camera)),
    );
    assert_eq!(before, after, "the viewport camera moved");
    assert!(tile_cameras(&mut app).is_empty(), "a tile camera stayed");
    assert!(
        views_poll_error(&mut app, &params).contains("already answered"),
        "a reused id is told so"
    );
}

fn views_poll_error(app: &mut App, params: &Value) -> String {
    app.world_mut()
        .run_system_cached_with(views_handler, Some(params.clone()))
        .expect("the handler ran")
        .expect_err("refused")
        .message
}

/// An unknown target is the caller's mistake; nothing is queued.
#[test]
fn an_unknown_views_target_is_invalid_and_queues_nothing() {
    let (mut app, _camera, _project) = editor_for_views();
    add_cube(&mut app, "Crate", [1.0, 1.0, 1.0], [0.0, 0.5, 0.0]);
    for targets in [json!([4_294_967_295_u64]), json!(["Nowhere"])] {
        let err = app
            .world_mut()
            .run_system_cached_with(
                views_handler,
                Some(json!({ "targets": targets, "request": "unknown" })),
            )
            .expect("the handler ran")
            .expect_err("an unknown target is refused");
        assert_eq!(err.code, bevy::remote::error_codes::INVALID_PARAMS);
    }
    app.update();
    assert!(tile_cameras(&mut app).is_empty());
    assert!(queued_readbacks(&mut app).is_empty());
    let err = views_poll_error(
        &mut app,
        &json!({ "angles": ["sideways"], "request": "sideways" }),
    );
    assert!(err.contains("front"), "{err}");
    let err = views_poll_error(&mut app, &json!({}));
    assert!(err.contains("`request`"), "{err}");
    assert!(tile_cameras(&mut app).is_empty());
}

/// `isolate` frames only the targets and draws them on a layer of their own:
/// every view's box holds the target and leaves the neighbour out, and the
/// layer comes off once the views answer.
#[test]
fn isolated_views_frame_only_their_targets() {
    let (mut app, _camera, _project) = editor_for_views();
    let shed = add_cube(&mut app, "Shed", [2.0, 2.0, 2.0], [0.0, 1.0, 0.0]);
    let tower = add_cube(&mut app, "Tower", [2.0, 8.0, 2.0], [30.0, 4.0, 0.0]);
    let shed_entity = Entity::try_from_bits(shed).expect("an entity");
    let tower_entity = Entity::try_from_bits(tower).expect("an entity");

    // Brush chunk meshes are not built without the renderer; a mesh child
    // stands in for each.
    for parent in [shed_entity, tower_entity] {
        app.world_mut().spawn((
            Mesh3d(Handle::default()),
            Transform::default(),
            ChildOf(parent),
        ));
    }
    let lamp = app.world_mut().spawn(PointLight::default()).id();
    let stage_lamp = app
        .world_mut()
        .spawn((jackdaw_editor_core::EditorEntity, PointLight::default()))
        .id();
    let lantern_layers = RenderLayers::from_layers(&[0, 20]);
    let lantern = app
        .world_mut()
        .spawn((
            Mesh3d(Handle::default()),
            PointLight::default(),
            lantern_layers.clone(),
            Transform::default(),
            ChildOf(shed_entity),
        ))
        .id();
    let params = json!({
        "targets": [shed],
        "angles": ["front", "top"],
        "isolate": true,
        "request": "iso",
    });
    assert!(views_poll(&mut app, &params).is_none());
    for (camera, _) in tile_cameras(&mut app) {
        let transform = *app.world().get::<Transform>(camera).expect("a transform");
        let Some(Projection::Orthographic(ortho)) = app.world().get::<Projection>(camera) else {
            panic!("axis views are orthographic");
        };
        let bevy::camera::ScalingMode::FixedVertical { viewport_height } = ortho.scaling_mode
        else {
            panic!("unexpected scaling");
        };
        let half = viewport_height * 0.5;
        let to_view = transform.compute_affine().inverse();
        let seen = |point: Vec3| {
            let local = to_view.transform_point3(point);
            local.x.abs() <= half && local.y.abs() <= half
        };
        assert!(seen(Vec3::new(1.0, 2.0, 1.0)) && seen(Vec3::new(-1.0, 0.0, -1.0)));
        assert!(!seen(Vec3::new(30.0, 4.0, 0.0)), "the tower is in frame");
        assert!(half < 3.0, "the frame is wider than the shed: {half}");
        let layers = app
            .world()
            .get::<RenderLayers>(camera)
            .cloned()
            .expect("an isolated camera has its own layer");
        assert!(!layers.intersects(&RenderLayers::layer(0)));
    }
    let first = tile_cameras(&mut app)[0].0;
    let camera_layers = app
        .world()
        .get::<RenderLayers>(first)
        .cloned()
        .expect("layers");
    let drawn_on = |app: &mut App, root: Entity| -> Vec<bool> {
        let mut stack = vec![root];
        let mut found = Vec::new();
        while let Some(at) = stack.pop() {
            if app.world().get::<Mesh3d>(at).is_some() {
                found.push(
                    app.world()
                        .get::<RenderLayers>(at)
                        .is_some_and(|layers| layers.intersects(&camera_layers)),
                );
            }
            if let Some(children) = app.world().get::<Children>(at) {
                stack.extend(children.iter());
            }
        }
        found
    };
    let shed_drawn = drawn_on(&mut app, shed_entity);
    assert!(!shed_drawn.is_empty(), "the shed has no meshes");
    assert!(shed_drawn.iter().all(|drawn| *drawn), "{shed_drawn:?}");
    assert!(drawn_on(&mut app, tower_entity).iter().all(|drawn| !drawn));
    assert!(
        app.world()
            .get::<RenderLayers>(lamp)
            .is_some_and(|layers| layers.intersects(&camera_layers)
                && layers.intersects(&RenderLayers::layer(0))),
        "a light does not reach the isolated views"
    );
    assert!(
        app.world().get::<RenderLayers>(stage_lamp).is_none(),
        "an editor light joined the isolated views"
    );

    // A brush rebuild replaces its chunk meshes with new entities mid-settle;
    // the new mesh joins the isolated views on the next frame.
    let old_chunk = app
        .world()
        .get::<Children>(shed_entity)
        .and_then(|children| {
            children
                .iter()
                .find(|child| *child != lantern && app.world().get::<Mesh3d>(*child).is_some())
        })
        .expect("a stand-in chunk");
    app.world_mut().despawn(old_chunk);
    let new_chunk = app
        .world_mut()
        .spawn((
            Mesh3d(Handle::default()),
            Transform::default(),
            ChildOf(shed_entity),
        ))
        .id();
    app.update();
    assert!(
        app.world()
            .get::<RenderLayers>(new_chunk)
            .is_some_and(|layers| layers.intersects(&camera_layers)),
        "a rebuilt chunk is missing from the isolated views"
    );

    land_tiles(&mut app, 2, flat);
    let answer = views_poll(&mut app, &params).expect("both tiles landed");
    assert_eq!(
        answer["bounds"],
        json!([[-1.0, 0.0, -1.0], [1.0, 2.0, 1.0]]),
        "{answer}"
    );
    assert!(
        drawn_on(&mut app, shed_entity).iter().all(|drawn| !drawn),
        "the isolation layer stayed on the shed"
    );
    assert!(
        app.world().get::<RenderLayers>(lamp).is_none(),
        "the light kept its layer"
    );
    assert_eq!(
        app.world().get::<RenderLayers>(lantern),
        Some(&lantern_layers),
        "a mesh that is also a light came back changed"
    );
    assert!(
        app.world().get::<RenderLayers>(new_chunk).is_none(),
        "a rebuilt chunk kept the isolation layer"
    );
}

/// Framing skips targets without bounds while any target has some: a light
/// high above a house does not shrink the house to a speck.
#[test]
fn views_frame_the_bounded_targets_and_skip_a_light() {
    let (mut app, _camera, _project) = editor_for_views();
    let house = add_cube(&mut app, "House", [4.0, 3.0, 4.0], [0.0, 1.5, 0.0]);
    let lamp = app
        .world_mut()
        .spawn((
            Name::new("Sky_Lamp"),
            PointLight::default(),
            Transform::from_xyz(0.0, 60.0, 0.0),
        ))
        .id();
    app.update();
    let params = json!({
        "targets": [house, lamp.to_bits()],
        "angles": ["front"],
        "request": "lamp",
    });
    assert!(views_poll(&mut app, &params).is_none());
    land_tiles(&mut app, 1, flat);
    let answer = views_poll(&mut app, &params).expect("the tile landed");
    assert_eq!(
        answer["bounds"],
        json!([[-2.0, 0.0, -2.0], [2.0, 3.0, 2.0]]),
        "{answer}"
    );

    let params = json!({ "targets": [lamp.to_bits()], "angles": ["front"], "request": "alone" });
    assert!(views_poll(&mut app, &params).is_none());
    land_tiles(&mut app, 1, flat);
    let answer = views_poll(&mut app, &params).expect("the tile landed");
    assert_eq!(
        answer["bounds"],
        json!([[-0.5, 59.5, -0.5], [0.5, 60.5, 0.5]]),
        "{answer}"
    );
}

/// Tiles written one per file are capped in size and in total.
#[test]
fn views_without_a_sheet_refuse_oversized_tiles() {
    let (mut app, _camera, _project) = editor_for_views();
    add_cube(&mut app, "Crate", [1.0, 1.0, 1.0], [0.0, 0.5, 0.0]);
    let err = views_poll_error(
        &mut app,
        &json!({ "sheet": false, "max_px": 2048, "angles": ["iso"], "request": "big" }),
    );
    assert!(err.contains("1024"), "{err}");
    let err = views_poll_error(
        &mut app,
        &json!({
            "sheet": false,
            "max_px": 1024,
            "angles": ["front", "back", "left", "right", "top", "iso"],
            "request": "many",
        }),
    );
    assert!(err.contains("pixels"), "{err}");
    assert!(tile_cameras(&mut app).is_empty());
}

/// `per_target` gives each target a tile of its own, framed on it alone and
/// named after it.
#[test]
fn a_contact_sheet_has_one_tile_per_target() {
    let (mut app, _camera, _project) = editor_for_views();
    let ids: Vec<u64> = (0..3)
        .map(|index| {
            add_cube(
                &mut app,
                &format!("Hut_{index}"),
                [2.0, 2.0, 2.0],
                [index as f32 * 10.0, 1.0, 0.0],
            )
        })
        .collect();
    let params = json!({ "targets": ids, "per_target": true, "max_px": 256, "request": "huts" });
    assert!(views_poll(&mut app, &params).is_none());
    let cameras = tile_cameras(&mut app);
    assert_eq!(cameras.len(), 3);
    for (camera, index) in cameras {
        let transform = app.world().get::<Transform>(camera).expect("a transform");
        let hut = Vec3::new(index as f32 * 10.0, 1.0, 0.0);
        let toward = (hut - transform.translation).normalize();
        assert!(
            transform.forward().dot(toward) > 0.9999,
            "tile {index} does not look at its hut"
        );
        assert!(
            transform.translation.distance(hut) < 8.0,
            "tile {index} frames more than its hut"
        );
    }
    land_tiles(&mut app, 3, flat);
    let answer = views_poll(&mut app, &params).expect("every tile landed");
    assert_eq!(
        (answer["width"].clone(), answer["height"].clone()),
        (json!(256), json!(256))
    );
    let tiles = answer["tiles"].as_array().expect("tiles");
    assert_eq!(tiles.len(), 3);
    assert_eq!(tiles[1]["label"], json!("Hut_1 iso"));
    assert_eq!(tiles[1]["entity"], json!(ids[1]));
    assert_eq!(tiles[2]["at"], json!([0, 128, 128, 128]));
}

/// `sheet: false` writes each view to its own file at `max_px`.
#[test]
fn views_without_a_sheet_write_a_file_per_tile() {
    let (mut app, _camera, _project) = editor_for_views();
    add_cube(&mut app, "Crate", [1.0, 1.0, 1.0], [0.0, 0.5, 0.0]);
    let params = json!({
        "targets": ["Crate"],
        "angles": ["iso", { "eye": [5.0, 5.0, 5.0] }],
        "sheet": false,
        "max_px": 128,
        "format": "png",
        "request": "files",
    });
    assert!(views_poll(&mut app, &params).is_none());
    land_tiles(&mut app, 2, flat);
    let answer = views_poll(&mut app, &params).expect("both tiles landed");
    let tiles = answer["tiles"].as_array().expect("tiles");
    assert_eq!(tiles.len(), 2, "{answer}");
    for tile in tiles {
        let path = tile["path"].as_str().expect("a path");
        assert!(path.ends_with(".png"), "{answer}");
        let image = image::open(path).expect("the tile decodes");
        assert_eq!((image.width(), image.height()), (128, 128));
    }
    assert_eq!(tiles[1]["label"], json!("eye"));
    assert!(answer.get("path").is_none());
}

/// A second request while one is rendering is refused rather than queued.
#[test]
fn one_views_request_renders_at_a_time() {
    let (mut app, _camera, _project) = editor_for_views();
    add_cube(&mut app, "Crate", [1.0, 1.0, 1.0], [0.0, 0.5, 0.0]);
    assert!(views_poll(&mut app, &json!({ "request": "a" })).is_none());
    let err = views_poll_error(&mut app, &json!({ "request": "b" }));
    assert!(err.contains("still rendering"), "{err}");
}

/// Tiles are read back only once the renderer has no pipeline left
/// compiling. One that waits past the cap is read back anyway, and the answer
/// warns it may be blank.
#[test]
fn views_wait_for_the_renderer_and_warn_when_it_never_finishes() {
    use jackdaw_editor_core::camera_capture::{PendingCapture, PipelinesPending, READY_CAP_FRAMES};

    let (mut app, _camera, _project) = editor_for_views();
    add_cube(&mut app, "Crate", [1.0, 1.0, 1.0], [0.0, 0.5, 0.0]);
    app.world_mut().insert_resource(PipelinesPending(Some(2)));
    let params = json!({ "angles": ["front"], "request": "ready" });
    assert!(views_poll(&mut app, &params).is_none());
    for _ in 0..40 {
        app.update();
    }
    assert!(
        queued_readbacks(&mut app).is_empty(),
        "read back while pipelines were compiling"
    );
    app.world_mut().insert_resource(PipelinesPending(Some(0)));
    land_tiles(&mut app, 1, flat);
    let answer = views_poll(&mut app, &params).expect("the tile landed");
    assert!(answer.get("warnings").is_none(), "{answer}");

    app.world_mut().insert_resource(PipelinesPending(Some(1)));
    let params = json!({ "angles": ["front"], "request": "never" });
    assert!(views_poll(&mut app, &params).is_none());
    let mut waiting = app.world_mut().query::<&mut PendingCapture>();
    for mut capture in waiting.iter_mut(app.world_mut()) {
        capture.waited = READY_CAP_FRAMES - 1;
    }
    land_tiles(&mut app, 1, flat);
    let answer = views_poll(&mut app, &params).expect("the tile landed");
    let warnings = answer["warnings"].to_string();
    assert!(warnings.contains("still compiling"), "{answer}");
}
