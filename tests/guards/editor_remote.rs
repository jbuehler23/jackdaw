//! The editor's remote-control surface, driven in process. The BRP handlers are
//! ordinary Bevy systems, so a test can run them without a socket; the `jd mcp`
//! end of the wire is covered by `tests/mcp_smoke.rs`.

use bevy::prelude::*;
use jackdaw::project::{ProjectConfig, ProjectRoot};
use jackdaw::remote::server::{
    apply_bsn_handler, assets_handler, batch_handler, call_operator_handler, cancel_handler,
    entity_handler, operators_handler, publish_endpoint, retract_endpoint, scene_tree_handler,
    screenshot_handler, status_handler, wait_handler,
};
use jackdaw_api_internal::operator::OperatorWorldExt as _;
use jackdaw_commands::CommandHistory;
use serde_json::{Value, json};

use crate::util;
use crate::util::OperatorResultExt as _;

/// Run one BRP handler and unwrap its answer.
#[track_caller]
fn call<M>(
    app: &mut App,
    handler: impl bevy::ecs::system::IntoSystem<In<Option<Value>>, bevy::remote::BrpResult, M> + 'static,
    params: Value,
) -> Value {
    app.world_mut()
        .run_system_cached_with(handler, Some(params))
        .expect("the handler ran")
        .unwrap_or_else(|err| panic!("the handler refused: {}", err.message))
}

/// Poll a watching handler until it answers, the way the HTTP layer does
/// while it holds the connection open.
#[track_caller]
fn poll<M>(
    app: &mut App,
    handler: impl bevy::ecs::system::IntoSystem<
        In<Option<Value>>,
        bevy::remote::BrpResult<Option<Value>>,
        M,
    > + Clone
    + 'static,
    params: Value,
) -> Value {
    for _ in 0..600 {
        let answer = app
            .world_mut()
            .run_system_cached_with(handler.clone(), Some(params.clone()))
            .expect("the handler ran")
            .unwrap_or_else(|err| panic!("the handler refused: {}", err.message));
        if let Some(answer) = answer {
            return answer;
        }
        app.update();
    }
    panic!("the handler never answered");
}

/// The names of the scene's root nodes, as the tree reports them.
fn root_names(app: &mut App) -> Vec<String> {
    let tree = call(app, scene_tree_handler, json!({ "depth": 0 }));
    tree["tree"]
        .as_array()
        .expect("an array")
        .iter()
        .filter_map(|node| node["name"].as_str().map(str::to_string))
        .collect()
}

/// The file the active tab holds.
fn open_scene_path(app: &App) -> Option<std::path::PathBuf> {
    let scenes = app.world().resource::<jackdaw::scenes::Scenes>();
    scenes.tabs.get(scenes.active)?.path.clone()
}

/// An editor with a project rooted in a temp directory, which is what every path
/// the surface accepts is measured against. The directory is handed back with
/// the app so it outlives the calls instead of leaking a project tree.
fn editor_with_a_project() -> (App, tempfile::TempDir) {
    let dir = tempfile::Builder::new()
        .prefix("jackdaw-remote-")
        .tempdir()
        .expect("temp dir");
    std::fs::create_dir_all(dir.path().join("assets")).expect("an assets dir");
    let mut app = editor_with_a_scene();
    app.world_mut().insert_resource(ProjectRoot::new(
        dir.path().to_path_buf(),
        ProjectConfig::default(),
    ));
    app.update();
    (app, dir)
}

/// An editor with a scene document open, which is what every authoring
/// operator needs before it will do anything.
fn editor_with_a_scene() -> App {
    let mut app = util::editor_test_app();
    app.world_mut()
        .operator("scene.new")
        .call()
        .expect("scene.new dispatches")
        .assert_finished();
    app.update();
    app
}

/// A remote rename answers with its outcome, shows in the tree and takes one
/// undo to revert.
#[test]
fn a_remote_rename_reports_its_outcome_and_the_tree_follows() {
    let mut app = editor_with_a_scene();
    let added = call(
        &mut app,
        call_operator_handler,
        json!({ "id": "entity.add.cube", "params": { "name": "Wall" } }),
    );
    let id = added["entities"][0].as_u64().expect("an entity id");
    app.update();
    let before = app.world().resource::<CommandHistory>().undo_stack.len();

    let renamed = call(
        &mut app,
        call_operator_handler,
        json!({ "id": "entity.rename", "params": { "entity": id, "name": "Gate" } }),
    );
    assert_eq!(renamed["result"], json!("finished"), "{renamed}");
    assert_eq!(renamed["outcome"], json!("changed"), "{renamed}");
    app.update();
    let tree = call(&mut app, scene_tree_handler, json!({}));
    let names: Vec<&Value> = tree["tree"]
        .as_array()
        .expect("roots")
        .iter()
        .map(|node| &node["name"])
        .collect();
    assert!(names.contains(&&json!("Gate")), "{tree}");
    assert!(!names.contains(&&json!("Wall")), "{tree}");
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        before + 1
    );

    let same = call(
        &mut app,
        call_operator_handler,
        json!({ "id": "entity.rename", "params": { "entity": id, "name": "Gate" } }),
    );
    assert_eq!(same["outcome"], json!("unchanged"), "{same}");
    let refused = call(
        &mut app,
        call_operator_handler,
        json!({ "id": "entity.rename", "params": { "name": "Gate" } }),
    );
    assert_eq!(refused["outcome"], json!("invalid"), "{refused}");
}

/// `jackdaw/status` is the first call any client makes, so it has to
/// answer on a cold editor rather than only once a project is open.
#[test]
fn status_answers_before_a_project_is_open() {
    let mut app = util::editor_test_app();
    let status = call(&mut app, status_handler, json!({}));
    assert_eq!(status["pid"], json!(std::process::id()));
    assert!(status["selection"].is_array(), "{status}");
    assert_eq!(status["pie"], json!("stopped"), "{status}");
}

/// The operator list is the whole remote vocabulary, so an operator the
/// editor registers has to appear in it with its parameter schema.
#[test]
fn the_operator_list_carries_ids_and_parameter_schemas() {
    let mut app = util::editor_test_app();
    let listed = call(&mut app, operators_handler, json!({ "prefix": "terrain." }));
    let operators = listed["operators"].as_array().expect("an array");
    assert!(
        operators.iter().all(|op| op["id"]
            .as_str()
            .is_some_and(|id| id.starts_with("terrain."))),
        "the prefix filter let something else through: {listed}"
    );

    let stamp = operators
        .iter()
        .find(|op| op["id"] == json!("terrain.sculpt.stamp"))
        .expect("terrain.sculpt.stamp is registered");
    let params = stamp["params"].as_array().expect("a parameter array");
    let radius = params
        .iter()
        .find(|param| param["name"] == json!("radius"))
        .expect("the stamp declares a radius");
    assert_eq!(radius["type"], json!("Float"));
    assert!(
        !radius["doc"].as_str().unwrap_or_default().is_empty(),
        "a parameter with no doc tells a caller nothing: {radius}"
    );
}

/// The call every remote session starts with, end to end: a cube arrives,
/// the tree shows it, its BSN reads back, and one undo takes it away.
#[test]
fn a_call_adds_an_entity_that_the_tree_and_undo_both_see() {
    let mut app = editor_with_a_scene();
    let before = app.world().resource::<CommandHistory>().undo_stack.len();

    let outcome = call(
        &mut app,
        call_operator_handler,
        json!({ "id": "entity.add.cube" }),
    );
    assert_eq!(outcome["result"], json!("finished"), "{outcome}");
    app.update();

    let tree = call(&mut app, scene_tree_handler, json!({}));
    let roots = tree["tree"].as_array().expect("an array of roots");
    let cube = roots
        .iter()
        .find(|node| node["name"] == json!("Cube"))
        .unwrap_or_else(|| panic!("no Cube in the tree: {tree}"));
    assert!(
        cube["components"]
            .as_array()
            .expect("components")
            .iter()
            .any(|path| path.as_str().is_some_and(|p| p.contains("Transform"))),
        "the node reports no Transform: {cube}"
    );

    let bsn = call(&mut app, entity_handler, json!({ "name": "Cube" }));
    assert!(
        bsn["bsn"].as_str().is_some_and(|text| !text.is_empty()),
        "the node emitted no BSN: {bsn}"
    );

    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        before + 1,
        "one call is one undo entry"
    );
    app.world_mut()
        .operator("history.undo")
        .call()
        .expect("history.undo dispatches")
        .assert_finished();
    app.update();
    let tree = call(&mut app, scene_tree_handler, json!({}));
    assert!(
        !tree["tree"]
            .as_array()
            .expect("an array")
            .iter()
            .any(|node| node["name"] == json!("Cube")),
        "undo left the cube behind: {tree}"
    );
}

/// A parametric add answers with what it made: the outcome and the new id,
/// which then names the node. A refused one answers `invalid` and adds nothing.
#[test]
fn a_parametric_add_reports_its_outcome_and_the_id_it_created() {
    let mut app = editor_with_a_scene();
    let outcome = call(
        &mut app,
        call_operator_handler,
        json!({
            "id": "entity.add.cylinder",
            "params": { "size": [2.0, 1.0, 2.0], "segments": 3, "name": "Post" },
        }),
    );
    assert_eq!(outcome["result"], json!("finished"), "{outcome}");
    assert_eq!(outcome["outcome"], json!("changed"), "{outcome}");
    let ids = outcome["entities"].as_array().expect("entities");
    assert_eq!(ids.len(), 1, "{outcome}");
    app.update();
    let post = call(&mut app, entity_handler, json!({ "entity": ids[0] }));
    assert!(
        post["bsn"]
            .as_str()
            .is_some_and(|text| text.contains("Post")),
        "the created id names the post: {post}"
    );

    let refused = call(
        &mut app,
        call_operator_handler,
        json!({ "id": "entity.add.cylinder", "params": { "segments": 2 } }),
    );
    assert_eq!(refused["outcome"], json!("invalid"), "{refused}");
    assert_eq!(refused["entities"], json!([]), "{refused}");
}

/// A batch is one action, so it is one undo entry: without the shared history
/// span a caller that placed twelve props would need twelve Ctrl-Zs.
#[test]
fn a_batch_of_calls_is_one_undo_entry() {
    let mut app = editor_with_a_scene();
    let before = app.world().resource::<CommandHistory>().undo_stack.len();

    let outcome = call(
        &mut app,
        batch_handler,
        json!({
            "label": "Three cubes",
            "calls": [
                { "id": "entity.add.cube" },
                { "id": "entity.add.cube" },
                { "id": "entity.add.cube" },
            ],
        }),
    );
    assert_eq!(
        outcome["calls"].as_array().map(Vec::len),
        Some(3),
        "{outcome}"
    );
    app.update();

    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        before + 1,
        "the batch pushed more than one entry"
    );
}

/// A batch stops at the first call that does not finish and says which,
/// rather than running the rest against a scene the caller did not mean.
#[test]
fn a_batch_reports_the_call_that_failed() {
    let mut app = editor_with_a_scene();
    let error = app
        .world_mut()
        .run_system_cached_with(
            batch_handler,
            Some(json!({
                "calls": [
                    { "id": "entity.add.cube" },
                    { "id": "no.such.operator" },
                ],
            })),
        )
        .expect("the handler ran")
        .expect_err("an unknown operator is a failure");
    assert!(
        error.message.contains("call 1"),
        "the failure does not name the call: {}",
        error.message
    );
}

/// The tree the remote reports has to be the tree the outliner draws:
/// a caller that saw editor furniture would try to author it.
#[test]
fn the_tree_hides_what_the_outliner_hides() {
    let mut app = editor_with_a_scene();
    app.world_mut()
        .operator("entity.add.cube")
        .call()
        .expect("entity.add.cube dispatches")
        .assert_finished();
    app.update();

    let hidden = app
        .world_mut()
        .spawn((
            Name::new("EditorFurniture"),
            Transform::default(),
            jackdaw::EditorEntity,
        ))
        .id();
    app.update();

    let tree = call(&mut app, scene_tree_handler, json!({}));
    let names: Vec<&str> = tree["tree"]
        .as_array()
        .expect("an array")
        .iter()
        .filter_map(|node| node["name"].as_str())
        .collect();
    assert!(names.contains(&"Cube"), "{tree}");
    assert!(
        !names.contains(&"EditorFurniture"),
        "editor furniture reached the caller: {tree}"
    );
    app.world_mut().despawn(hidden);
}

/// `depth` bounds the answer, so a caller can ask what the roots are
/// without reading a thousand-node scene.
#[test]
fn depth_zero_reports_roots_without_their_children() {
    let mut app = editor_with_a_scene();
    let parent = app
        .world_mut()
        .spawn((Name::new("Parent"), Transform::default()))
        .id();
    app.world_mut()
        .spawn((Name::new("Child"), Transform::default(), ChildOf(parent)));
    app.update();

    let tree = call(&mut app, scene_tree_handler, json!({ "depth": 0 }));
    let node = tree["tree"]
        .as_array()
        .expect("an array")
        .iter()
        .find(|node| node["name"] == json!("Parent"))
        .expect("the parent is a root");
    assert_eq!(node["children"], json!([]), "{node}");
}

/// Applied BSN lands under the node the caller named, which is what lets
/// a caller build into an existing scene rather than beside it.
#[test]
fn applied_bsn_lands_under_the_named_parent() {
    let mut app = editor_with_a_scene();
    app.world_mut()
        .operator("entity.add.group")
        .param("name", "Props")
        .call()
        .expect("entity.add.group dispatches")
        .assert_finished();
    app.update();

    let spawned = call(
        &mut app,
        apply_bsn_handler,
        json!({
            "source": "#Placed\nbevy_transform::components::transform::Transform\n",
            "parent": "Props",
        }),
    );
    app.update();

    let entities = spawned["entities"].as_array().expect("an array");
    assert_eq!(entities.len(), 1, "{spawned}");
    let entity = Entity::try_from_bits(entities[0].as_u64().expect("entity bits"))
        .expect("a live entity id");
    let parent = app
        .world()
        .get::<ChildOf>(entity)
        .map(ChildOf::parent)
        .expect("the spawned node has a parent");
    assert_eq!(
        app.world().get::<Name>(parent).map(Name::as_str),
        Some("Props")
    );
}

/// A source that spells a hierarchy keeps it: the children the caller wrote
/// stay under their own root rather than landing beside it on the parent.
#[test]
fn applied_bsn_keeps_the_hierarchy_the_source_spells() {
    let mut app = editor_with_a_scene();
    app.world_mut()
        .operator("entity.add.group")
        .param("name", "Props")
        .call()
        .expect("entity.add.group dispatches")
        .assert_finished();
    app.update();

    call(
        &mut app,
        apply_bsn_handler,
        json!({
            "source": "#Crate\n\
                bevy_transform::components::transform::Transform\n\
                bevy_ecs::hierarchy::Children [\n\
                    #Lid bevy_transform::components::transform::Transform\n\
                ]\n",
            "parent": "Props",
        }),
    );
    app.update();

    let tree = call(&mut app, scene_tree_handler, json!({ "root": "Props" }));
    let props = &tree["tree"][0];
    let crates = props["children"].as_array().expect("an array");
    assert_eq!(crates.len(), 1, "{tree}");
    assert_eq!(crates[0]["name"], json!("Crate"), "{tree}");
    let lids = crates[0]["children"].as_array().expect("an array");
    assert_eq!(lids.len(), 1, "{tree}");
    assert_eq!(lids[0]["name"], json!("Lid"), "{tree}");
}

/// Several roots in one source all arrive, each under the name the source
/// gives it, rather than the last one overwriting the rest.
#[test]
fn applied_bsn_spawns_every_root_in_the_source() {
    let mut app = editor_with_a_scene();

    let spawned = call(
        &mut app,
        apply_bsn_handler,
        json!({
            "source": "#Barrel bevy_transform::components::transform::Transform,\n\
                #Sack bevy_transform::components::transform::Transform\n",
        }),
    );
    app.update();

    assert_eq!(
        spawned["entities"].as_array().expect("an array").len(),
        2,
        "{spawned}"
    );
    let names = root_names(&mut app);
    assert!(names.contains(&"Barrel".to_string()), "{names:?}");
    assert!(names.contains(&"Sack".to_string()), "{names:?}");
}

/// A UI screen hangs off `Node`, which carries `UiTransform` rather than
/// `Transform`, so the walk has to follow it or the screen reads as empty.
#[test]
fn the_tree_walks_a_ui_screen() {
    let mut app = editor_with_a_scene();
    let root = app
        .world_mut()
        .spawn((
            Name::new("Hud"),
            Node::default(),
            jackdaw_scene_types::UiSceneRoot::default(),
        ))
        .id();
    app.world_mut()
        .spawn((Name::new("HealthBar"), Node::default(), ChildOf(root)));
    app.update();

    // The editor's own panels are `Node` all the way down, so a bare node is
    // not a scene root however the walk treats one under a screen.
    app.world_mut()
        .spawn((Name::new("Chrome"), Node::default()));
    app.update();

    let tree = call(&mut app, scene_tree_handler, json!({ "root": "Hud" }));
    let children = tree["tree"][0]["children"].as_array().expect("an array");
    assert_eq!(children.len(), 1, "{tree}");
    assert_eq!(children[0]["name"], json!("HealthBar"), "{tree}");

    assert!(
        !root_names(&mut app).contains(&"Chrome".to_string()),
        "a bare UI node reached the caller as a scene root"
    );
}

/// Naming a root is how a caller reaches into a UI screen, and the editor's
/// own panels are the same `Node` a screen is made of. A panel is not the
/// scene, so naming one answers nothing rather than handing back the chrome.
#[test]
fn the_tree_refuses_a_panel_of_the_editors_own_chrome() {
    let mut app = editor_with_a_scene();
    let panel = app
        .world_mut()
        .spawn((Name::new("Sidebar"), Node::default()))
        .id();
    app.world_mut()
        .spawn((Name::new("SidebarRow"), Node::default(), ChildOf(panel)));
    app.update();

    for named in ["Sidebar", "SidebarRow"] {
        let answer = app
            .world_mut()
            .run_system_cached_with(scene_tree_handler, Some(json!({ "root": named })))
            .expect("the handler ran");
        if let Ok(tree) = answer {
            assert_eq!(
                tree["tree"].as_array().expect("an array").len(),
                0,
                "{named} reached the caller: {tree}"
            );
        }
    }
}

/// A mask's values are numbers in the file and names on the screen. The entity
/// view is where a caller with no screen learns which name carries which
/// number, so `terrain.scatter` can be given one.
#[test]
fn the_entity_view_reports_a_terrains_mask_palettes() {
    use jackdaw_scene_types::{Terrain, TerrainChannel, TerrainPaletteEntry};

    let mut app = editor_with_a_scene();
    app.world_mut().spawn((
        Name::new("Ground"),
        Transform::default(),
        Terrain {
            channels: vec![TerrainChannel {
                name: "biome".to_string(),
                element: jackdaw_scene_types::TerrainChannelElement::U8,
                palette: vec![
                    TerrainPaletteEntry {
                        value: 0,
                        label: "unset".to_string(),
                        color: Color::BLACK,
                    },
                    TerrainPaletteEntry {
                        value: 3,
                        label: "meadow".to_string(),
                        color: Color::WHITE,
                    },
                ],
            }],
            ..default()
        },
    ));
    app.update();

    let view = call(&mut app, entity_handler, json!({ "name": "Ground" }));
    let masks = view["masks"].as_array().expect("an array");
    assert_eq!(masks.len(), 1, "{view}");
    assert_eq!(masks[0]["name"], json!("biome"), "{view}");
    assert_eq!(
        masks[0]["palette"],
        json!([
            { "value": 0, "label": "unset" },
            { "value": 3, "label": "meadow" },
        ]),
        "{view}"
    );
}

/// The screenshot method answers only once the PNG is on disk. The capture
/// itself needs a GPU, so this drives the completion half.
#[test]
fn a_screenshot_resolves_once_the_capture_log_has_the_file() {
    let (mut app, _project) = editor_with_a_project();
    let project = app.world().resource::<ProjectRoot>().root.clone();
    // The handler resolves the project folder, as the capture it queues does.
    let path = dunce::canonicalize(&project).unwrap().join("shot.png");
    // A window capture is the one kind that cannot fail up front: it targets the
    // primary window by reference. The log entry below stands in for the frame a
    // GPU readback would have landed.
    let params = Some(json!({ "kind": "window", "path": "shot.png", "request": "one" }));

    let queued = app
        .world_mut()
        .run_system_cached_with(screenshot_handler, params.clone())
        .expect("the handler ran")
        .expect("queueing a window capture cannot fail");
    assert!(
        queued.is_none(),
        "the first poll answered before the capture"
    );
    app.world_mut()
        .resource_mut::<jackdaw::screenshot::CaptureLog>()
        .record(path.clone(), (1280, 720));

    let answer = app
        .world_mut()
        .run_system_cached_with(screenshot_handler, params)
        .expect("the handler ran")
        .expect("the handler answered")
        .expect("a finished capture answers");
    assert_eq!(answer["width"], json!(1280));
    assert_eq!(answer["height"], json!(720));

    // The entry is consumed, so a later capture to the same path does
    // not answer with the previous one's size.
    assert!(
        app.world()
            .resource::<jackdaw::screenshot::CaptureLog>()
            .size_of(&path)
            .is_none()
    );
}

/// A region rides the request into the capture, and the answer reports the
/// cropped size. The readback is stood in for by a synthetic frame.
#[test]
fn a_screenshot_region_crops_the_capture_it_answers_for() {
    use bevy::image::ToExtents as _;
    use bevy::render::render_resource::{TextureDimension, TextureFormat};
    use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};

    let (mut app, _project) = editor_with_a_project();
    let panel = app
        .world_mut()
        .spawn((
            jackdaw::EditorEntity,
            Node {
                width: px(600),
                height: px(400),
                ..default()
            },
        ))
        .id();
    jackdaw::viewport_2d::build_viewport_2d_panel(app.world_mut(), panel);
    app.update();

    let params = Some(json!({
        "kind": "viewport2d",
        "path": "region.png",
        "request": "one",
        "region": { "x": 8, "y": 8, "w": 24, "h": 10, "space": "pixels" },
    }));
    let first = app
        .world_mut()
        .run_system_cached_with(screenshot_handler, params.clone())
        .expect("the handler ran")
        .expect("a region within the image queues");
    assert!(first.is_none());

    let shot = app
        .world_mut()
        .query_filtered::<Entity, With<Screenshot>>()
        .single(app.world())
        .expect("one capture queued");
    app.world_mut().trigger(ScreenshotCaptured {
        entity: shot,
        image: Image::new_fill(
            UVec2::new(200, 100).to_extents(),
            TextureDimension::D2,
            &[0, 255, 0, 255],
            TextureFormat::Rgba8UnormSrgb,
            default(),
        ),
    });

    let answer = app
        .world_mut()
        .run_system_cached_with(screenshot_handler, params)
        .expect("the handler ran")
        .expect("the handler answered")
        .expect("a landed capture answers");
    assert_eq!(answer["width"], json!(24), "{answer}");
    assert_eq!(answer["height"], json!(10), "{answer}");
}

/// A malformed region is the caller's mistake, reported as one.
#[test]
fn a_malformed_screenshot_region_is_an_invalid_params_error() {
    let (mut app, _project) = editor_with_a_project();
    for region in [
        json!({ "x": 1, "y": 1, "w": 5 }),
        json!({ "x": 1, "y": 1, "w": 5, "h": 0 }),
        json!({ "x": 1, "y": 1, "w": 5, "h": 5, "space": "screen" }),
    ] {
        let refused = app
            .world_mut()
            .run_system_cached_with(
                screenshot_handler,
                Some(json!({ "kind": "window", "region": region })),
            )
            .expect("the handler ran");
        let err = refused.expect_err("a malformed region is refused");
        assert_eq!(err.code, bevy::remote::error_codes::INVALID_PARAMS);
    }
}

/// A window capture takes a pixel region like a viewport one, and the answer
/// reports the cropped size.
#[test]
fn a_window_screenshot_crops_to_a_pixel_region() {
    use bevy::image::ToExtents as _;
    use bevy::render::render_resource::{TextureDimension, TextureFormat};
    use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};

    let (mut app, _project) = editor_with_a_project();
    let params = Some(json!({
        "kind": "window",
        "path": "window-region.png",
        "request": "one",
        "region": { "x": 5, "y": 6, "w": 40, "h": 20, "space": "pixels" },
    }));
    let first = app
        .world_mut()
        .run_system_cached_with(screenshot_handler, params.clone())
        .expect("the handler ran")
        .expect("a pixel region on a window capture queues");
    assert!(first.is_none());

    let shot = app
        .world_mut()
        .query_filtered::<Entity, With<Screenshot>>()
        .single(app.world())
        .expect("one capture queued");
    app.world_mut().trigger(ScreenshotCaptured {
        entity: shot,
        image: Image::new_fill(
            UVec2::new(200, 100).to_extents(),
            TextureDimension::D2,
            &[0, 0, 255, 255],
            TextureFormat::Rgba8UnormSrgb,
            default(),
        ),
    });
    let answer = app
        .world_mut()
        .run_system_cached_with(screenshot_handler, params)
        .expect("the handler ran")
        .expect("the handler answered")
        .expect("a landed capture answers");
    assert_eq!(answer["width"], json!(40), "{answer}");
    assert_eq!(answer["height"], json!(20), "{answer}");
}

/// A region the capture cannot honour is the caller's mistake, not the
/// editor's.
#[test]
fn a_region_the_capture_cannot_honour_is_an_invalid_params_error() {
    let (mut app, _project) = editor_with_a_project();
    for params in [
        json!({ "kind": "window", "region": { "x": 0, "y": 0, "w": 5, "h": 5, "space": "world" } }),
        json!({ "kind": "window", "region": { "x": -50, "y": 0, "w": 5, "h": 5, "space": "pixels" } }),
        json!({ "kind": "window", "region": { "space": "pixels" } }),
        json!({ "kind": "window", "region": { "x": 0, "y": 0, "w": 5, "h": 5, "space": "" } }),
    ] {
        let refused = app
            .world_mut()
            .run_system_cached_with(screenshot_handler, Some(params.clone()))
            .expect("the handler ran");
        let err = refused.expect_err("the region is refused");
        assert_eq!(
            err.code,
            bevy::remote::error_codes::INVALID_PARAMS,
            "{params}"
        );
    }
}

/// A frame wait answers after the frames it was asked for and not before, so a
/// caller waiting for a scene load does wait. Frames, not polls.
#[test]
fn a_frame_wait_answers_only_after_its_frames() {
    let mut app = util::editor_test_app();
    let params = Some(json!({ "frames": 2, "request": "one" }));
    for poll in 0..2 {
        for _ in 0..2 {
            let answer = app
                .world_mut()
                .run_system_cached_with(wait_handler, params.clone())
                .expect("the handler ran")
                .expect("the handler answered");
            assert!(answer.is_none(), "poll {poll} answered early: {answer:?}");
        }
        app.update();
    }
    let answer = app
        .world_mut()
        .run_system_cached_with(wait_handler, params)
        .expect("the handler ran")
        .expect("the handler answered")
        .expect("the poll after the second frame answers");
    assert_eq!(answer["frames"], json!(2));
}

/// Two waits are two waits: a client that asks for 60 frames and drops at 40
/// must not leave the next identical request answering after 20.
#[test]
fn a_dropped_wait_does_not_shorten_the_next_one() {
    let mut app = util::editor_test_app();
    let abandoned = Some(json!({ "frames": 3, "request": "dropped" }));
    for _ in 0..2 {
        let _ = app
            .world_mut()
            .run_system_cached_with(wait_handler, abandoned.clone());
        app.update();
    }

    let fresh = Some(json!({ "frames": 3, "request": "fresh" }));
    for poll in 0..3 {
        let answer = app
            .world_mut()
            .run_system_cached_with(wait_handler, fresh.clone())
            .expect("the handler ran")
            .expect("the handler answered");
        assert!(
            answer.is_none(),
            "poll {poll} inherited the abandoned wait's countdown: {answer:?}"
        );
        app.update();
    }
    let answer = app
        .world_mut()
        .run_system_cached_with(wait_handler, fresh)
        .expect("the handler ran")
        .expect("the handler answered");
    assert!(answer.is_some(), "the fresh wait never answered");
}

/// `until_idle` answers even while a modal operator holds the editor: nothing is
/// going to end that modal on its own, since there is no pointer.
#[test]
fn waiting_for_idle_does_not_block_on_a_modal() {
    let mut app = util::editor_test_app();
    app.world_mut()
        .operator("tools.measure_distance")
        .call()
        .expect("the modal dispatches")
        .assert_running();
    app.update();

    let answer = app
        .world_mut()
        .run_system_cached_with(wait_handler, Some(json!({ "until": "idle" })))
        .expect("the handler ran")
        .expect("the handler answered")
        .expect("idle answers rather than blocking on the modal");
    assert_eq!(answer["idle"], json!(true), "{answer}");
}

/// A screenshot path is the project's to write. The editor runs as the
/// user, so a path taken verbatim would put a PNG over anything they own.
#[test]
fn a_screenshot_refuses_a_path_outside_the_project() {
    let (mut app, _project) = editor_with_a_project();
    for refused in ["/etc/passwd", "../escape.png", "sub/../../escape.png"] {
        let err = app
            .world_mut()
            .run_system_cached_with(screenshot_handler, Some(json!({ "path": refused })))
            .expect("the handler ran")
            .expect_err("the path is outside the project");
        assert!(
            err.message.contains(refused),
            "the refusal does not name the path: {}",
            err.message
        );
    }
}

/// `scene.open` is reachable from the remote, so its path is confined the same
/// way: an unconfined one would read any file on the machine into the editor.
#[test]
fn scene_open_refuses_a_file_outside_the_project() {
    let (mut app, _project) = editor_with_a_project();
    let outside = tempfile::tempdir().expect("temp dir");
    let path = outside.path().join("elsewhere.bsn");
    std::fs::write(
        &path,
        "#Elsewhere
bevy_transform::components::transform::Transform
",
    )
    .expect("write the scene");

    let before = open_scene_path(&app);
    app.world_mut()
        .operator("scene.open")
        .param("path", path.to_string_lossy().to_string())
        .call()
        .expect("scene.open dispatches")
        .assert_finished();
    app.update();

    assert_eq!(
        open_scene_path(&app),
        before,
        "a scene outside the project was opened anyway"
    );
}

/// Applying BSN is a write, and every write is undoable: without a command on
/// the stack the user's next Ctrl-Z would revert whatever they did before.
#[test]
fn applied_bsn_is_taken_back_by_one_undo() {
    let mut app = editor_with_a_scene();
    let before = app.world().resource::<CommandHistory>().undo_stack.len();

    let spawned = call(
        &mut app,
        apply_bsn_handler,
        json!({ "source": "#Applied
bevy_transform::components::transform::Transform
" }),
    );
    app.update();
    let entity = Entity::try_from_bits(
        spawned["entities"].as_array().expect("an array")[0]
            .as_u64()
            .expect("entity bits"),
    )
    .expect("a live entity id");
    assert!(app.world().get_entity(entity).is_ok());
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        before + 1,
        "apply_bsn pushed no undo entry"
    );

    app.world_mut()
        .operator("history.undo")
        .call()
        .expect("history.undo dispatches")
        .assert_finished();
    app.update();
    assert!(
        app.world().get_entity(entity).is_err(),
        "undo left the applied node in the scene"
    );
}

/// A batch whose entries push the history past its budget must group its own
/// work and nothing else. The span is keyed on a push counter for that reason: a
/// recorded stack length slides under earlier edits as the budget drops them.
#[test]
fn a_batch_never_swallows_an_earlier_edit_when_history_trims() {
    let mut app = editor_with_a_scene();
    app.world_mut()
        .operator("entity.add.group")
        .param("name", "UsersOwnWork")
        .call()
        .expect("entity.add.group dispatches")
        .assert_finished();
    app.update();

    // A budget of one byte means every push trims the stack to its
    // newest entry, which is exactly the state the span must survive.
    app.world_mut()
        .resource_mut::<CommandHistory>()
        .budget_bytes = 1;

    let outcome = call(
        &mut app,
        batch_handler,
        json!({
            "label": "Remote work",
            "calls": [
                { "id": "entity.add.group", "params": { "name": "RemoteA" } },
                { "id": "entity.add.group", "params": { "name": "RemoteB" } },
            ],
        }),
    );
    assert_eq!(outcome["calls"].as_array().map(Vec::len), Some(2));
    app.update();

    app.world_mut()
        .operator("history.undo")
        .call()
        .expect("history.undo dispatches")
        .assert_finished();
    app.update();

    let names = root_names(&mut app);
    assert!(
        !names.contains(&"RemoteA".to_string()) && !names.contains(&"RemoteB".to_string()),
        "the batch did not undo as one entry: {names:?}"
    );
    assert!(
        names.contains(&"UsersOwnWork".to_string()),
        "one undo of the batch also took back the user's own edit: {names:?}"
    );
}

/// A modal call inside a batch stops it and is cancelled on the way out: left
/// running it refuses every later modal call, with no pointer coming to end it.
#[test]
fn a_batch_cancels_a_modal_it_started() {
    let mut app = editor_with_a_scene();
    let outcome = call(
        &mut app,
        batch_handler,
        json!({
            "calls": [
                { "id": "tools.measure_distance" },
                { "id": "entity.add.cube" },
            ],
        }),
    );
    let calls = outcome["calls"].as_array().expect("an array");
    assert_eq!(calls.len(), 1, "the batch ran past the modal: {outcome}");
    assert_eq!(calls[0]["result"], json!("running"));
    app.update();

    let status = call(&mut app, status_handler, json!({}));
    assert_eq!(
        status["modal"],
        Value::Null,
        "the batch left a modal holding the editor: {status}"
    );
}

/// `jackdaw/cancel` is the way out of a modal a caller entered on
/// purpose, and says which one it ended.
#[test]
fn cancel_ends_the_modal_holding_the_editor() {
    let mut app = editor_with_a_scene();
    call(
        &mut app,
        call_operator_handler,
        json!({ "id": "tools.measure_distance" }),
    );
    app.update();
    let status = call(&mut app, status_handler, json!({}));
    assert_eq!(status["modal"], json!("tools.measure_distance"), "{status}");

    let cancelled = call(&mut app, cancel_handler, json!({}));
    assert_eq!(cancelled["cancelled"], json!("tools.measure_distance"));
    app.update();
    let status = call(&mut app, status_handler, json!({}));
    assert_eq!(status["modal"], Value::Null, "{status}");
}

/// An operator declared `remote_hidden` stays out of the vocabulary: the
/// draw-brush sub-operators only continue a gesture the pointer starts.
#[test]
fn remote_hidden_operators_are_not_offered() {
    let mut app = util::editor_test_app();
    let listed = call(&mut app, operators_handler, json!({}));
    let ids: Vec<&str> = listed["operators"]
        .as_array()
        .expect("an array")
        .iter()
        .filter_map(|op| op["id"].as_str())
        .collect();

    assert!(
        ids.contains(&"entity.add.cube"),
        "the list is empty or wrong"
    );
    for hidden in [
        "viewport.draw_brush.commit_polygon",
        "viewport.draw_brush.remove_last_vertex",
        "viewport.draw_brush.cancel_cut",
        "viewport.draw_brush.toggle_mode",
        "tools.measure_distance.confirm",
    ] {
        assert!(
            !ids.contains(&hidden),
            "{hidden} is declared remote_hidden but was offered anyway"
        );
    }
}

/// `until_idle` holds while a model is still coming in and answers once nothing
/// is. It has to hold in both directions: a gate that always answered `idle`
/// would pass a one-sided test.
#[test]
fn waiting_for_idle_holds_while_a_model_is_still_coming_in() {
    let (mut app, _project) = editor_with_a_project();

    let handle: Handle<bevy::world_serialization::WorldAsset> = app
        .world()
        .resource::<AssetServer>()
        .load("pending.gltf#Scene0");
    app.world_mut().spawn((
        Name::new("PendingModel"),
        Transform::default(),
        bevy::world_serialization::WorldAssetRoot(handle),
    ));

    // Nothing has run yet, so the load is `NotLoaded` or `Loading`: work
    // the editor is still doing, and the handler answers nothing.
    let held = app
        .world_mut()
        .run_system_cached_with(wait_handler, Some(json!({ "until": "idle" })))
        .expect("the handler ran")
        .expect("the handler did not refuse");
    assert_eq!(
        held, None,
        "idle answered over a scene whose model had not arrived"
    );

    // The file is not there, so the load fails: a settled scene, since no
    // amount of waiting produces a missing file.
    for _ in 0..8 {
        app.update();
    }
    let answer = app
        .world_mut()
        .run_system_cached_with(wait_handler, Some(json!({ "until": "idle" })))
        .expect("the handler ran")
        .expect("the handler answered");
    assert_eq!(
        answer.map(|value| value["idle"].clone()),
        Some(json!(true)),
        "idle never answered once nothing was left to load"
    );
}

/// LOD levels the cameras want and that are not in yet hold the wait for
/// idle, and it answers once they are.
#[test]
fn waiting_for_idle_holds_while_lod_levels_are_still_coming_in() {
    let (mut app, _project) = editor_with_a_project();
    for _ in 0..8 {
        app.update();
    }
    let wait = |app: &mut App, refined: usize| {
        app.world_mut()
            .insert_resource(jackdaw_runtime::LiveLevelProgress {
                groups: 2,
                drawn: 2,
                refined,
                loading: 0,
            });
        app.world_mut()
            .run_system_cached_with(wait_handler, Some(json!({ "until": "idle" })))
            .expect("the handler ran")
            .expect("the handler did not refuse")
            .map(|value| value["idle"].clone())
    };

    assert_eq!(
        wait(&mut app, 1),
        None,
        "idle answered with a level still queued"
    );
    assert_eq!(wait(&mut app, 2), Some(json!(true)));
}

/// Models are handed to the scene spawner a batch a frame, so for the first
/// frames of a scene the entity that names one has no handle to ask the asset
/// server about. Idle has to hold over those frames too.
#[test]
fn waiting_for_idle_holds_while_a_model_is_still_queued_for_the_spawner() {
    let (mut app, _project) = editor_with_a_project();

    app.world_mut().spawn((
        Name::new("QueuedModel"),
        Transform::default(),
        jackdaw_scene_types::GltfSource {
            path: "pending.gltf".into(),
            scene_index: 0,
        },
    ));
    app.world_mut().flush();

    assert!(
        !app.world()
            .resource::<jackdaw::entity_ops::PendingModelRoots>()
            .is_empty(),
        "the model is waiting its turn, so the wait has something to hold for"
    );
    let held = app
        .world_mut()
        .run_system_cached_with(wait_handler, Some(json!({ "until": "idle" })))
        .expect("the handler ran")
        .expect("the handler did not refuse");

    assert_eq!(
        held, None,
        "idle answered over a scene whose models had not been handed over yet"
    );
}

/// An operator that could not use a parameter says so to whoever called it, not
/// only to the log: a remote caller has no terminal to read it in.
#[test]
fn a_call_reports_what_the_operator_refused() {
    let mut app = editor_with_a_scene();
    let outcome = call(
        &mut app,
        call_operator_handler,
        json!({
            "id": "input.pointer",
            "params": { "x": 10, "y": 10, "space": "viewport" },
        }),
    );
    app.update();

    let warnings = outcome["warnings"].as_array().expect("a warnings array");
    assert!(
        warnings
            .iter()
            .filter_map(Value::as_str)
            .any(|warning| warning.contains("viewport") && warning.contains("not a space")),
        "the refused space is not reported: {outcome}"
    );
}

/// A warning belongs to the call that produced it; left uncleared, the next call
/// would inherit it.
#[test]
fn warnings_do_not_carry_over_to_the_next_call() {
    let mut app = editor_with_a_scene();
    call(
        &mut app,
        call_operator_handler,
        json!({
            "id": "input.pointer",
            "params": { "x": 10, "y": 10, "button": "elbow" },
        }),
    );
    app.update();

    let outcome = call(
        &mut app,
        call_operator_handler,
        json!({ "id": "entity.add.cube" }),
    );
    assert!(
        outcome.get("warnings").is_none(),
        "the next call inherited the previous one's warning: {outcome}"
    );
}

/// A project that turns the surface off is answered by nothing. The methods
/// refuse, not merely the setting: a gate nothing consults reads as on.
#[test]
fn a_project_can_turn_the_surface_off() {
    let (mut app, _project) = editor_with_a_project();
    let root = app.world().resource::<ProjectRoot>().root.clone();
    assert!(
        app.world_mut()
            .run_system_cached_with(status_handler, None)
            .expect("the handler ran")
            .is_ok(),
        "the surface answered nothing while it was still on"
    );

    jackdaw::project_settings::store_section(
        &root,
        jackdaw::project_settings::Section::Key("remote"),
        &serde_json::json!({ "enabled": false }),
    );
    assert!(!jackdaw::remote::server::remote_enabled_for(&root));
    app.world_mut()
        .run_system_cached(jackdaw::remote::server::track_remote_enabled)
        .expect("the tracker ran");

    let refusal = app
        .world_mut()
        .run_system_cached_with(status_handler, None)
        .expect("the handler ran")
        .expect_err("a locked-down project still answered a method");
    assert!(
        refusal.message.contains("remote.enabled"),
        "the refusal does not say why: {}",
        refusal.message
    );
    let refusal = app
        .world_mut()
        .run_system_cached_with(
            call_operator_handler,
            Some(json!({ "id": "entity.add.cube" })),
        )
        .expect("the handler ran")
        .expect_err("a locked-down project still ran an operator");
    assert!(refusal.message.contains("remote.enabled"));
}

/// The editor takes its endpoint file back as it exits, so the next client reads
/// no live-looking editor at an address nothing answers. A test that only killed
/// the process would pass on the dead pid alone.
#[test]
fn the_editor_takes_its_endpoint_back_as_it_exits() {
    let (mut app, project) = editor_with_a_project();
    let endpoint = project.path().join(".jackdaw/editor.json");

    app.world_mut()
        .run_system_cached(publish_endpoint)
        .expect("the publisher ran");
    assert!(
        endpoint.exists(),
        "the editor published no endpoint for the open project"
    );

    app.world_mut().write_message(AppExit::Success);
    app.world_mut()
        .run_system_cached(retract_endpoint)
        .expect("the retractor ran");
    app.world_mut().flush();
    assert!(
        !endpoint.exists(),
        "the endpoint outlived the editor that wrote it"
    );
}

/// The ids a call reports are what a caller acts on next: it has just
/// placed a node and has to name it to move, rename or parent it.
#[test]
fn a_call_reports_the_entity_it_spawned() {
    let mut app = editor_with_a_scene();
    let outcome = call(
        &mut app,
        call_operator_handler,
        json!({ "id": "entity.add.cube" }),
    );
    app.update();

    let entities = outcome["entities"].as_array().expect("an array");
    assert_eq!(entities.len(), 1, "{outcome}");
    let entity = Entity::try_from_bits(entities[0].as_u64().expect("entity bits"))
        .expect("a live entity id");
    assert_eq!(
        app.world().get::<Name>(entity).map(Name::as_str),
        Some("Cube"),
        "the reported id is not the node the call added"
    );
}

/// Every call in a batch reports its own spawn, so a caller that built a
/// run of nodes in one undo entry can still address each of them.
#[test]
fn every_call_in_a_batch_reports_its_own_spawn() {
    let mut app = editor_with_a_scene();
    let outcome = call(
        &mut app,
        batch_handler,
        json!({
            "calls": [
                { "id": "entity.add.group", "params": { "name": "Props" } },
                { "id": "entity.add.cube" },
            ],
        }),
    );
    app.update();

    let calls = outcome["calls"].as_array().expect("an array");
    let names: Vec<Option<String>> = calls
        .iter()
        .map(|call| {
            let entities = call["entities"].as_array().expect("an array");
            assert_eq!(entities.len(), 1, "{call}");
            let entity = Entity::try_from_bits(entities[0].as_u64().expect("entity bits"))
                .expect("a live entity id");
            app.world()
                .get::<Name>(entity)
                .map(|name| name.as_str().to_string())
        })
        .collect();
    assert_eq!(
        names,
        vec![Some("Props".to_string()), Some("Cube".to_string())],
        "{outcome}"
    );
}

/// Instancing a prefab rebuilds the scene from the document and mints new ids
/// for every node, and the call still has to name the instance.
#[test]
fn a_prefab_instance_call_reports_the_instance_it_added() {
    let (mut app, project) = editor_with_a_project();
    let prefab = project.path().join("assets/rock.bsn");
    std::fs::write(
        &prefab,
        "#Rock
bevy_transform::components::transform::Transform
",
    )
    .expect("write the prefab");

    let outcome = call(
        &mut app,
        call_operator_handler,
        json!({
            "id": "prefab.spawn_instance",
            "params": {
                "path": "rock.bsn",
                "pos_x": 0, "pos_y": 0, "pos_z": 0,
            },
        }),
    );
    assert_eq!(outcome["result"], json!("finished"), "{outcome}");

    let entities = outcome["entities"].as_array().expect("an array");
    assert_eq!(entities.len(), 1, "{outcome}");
    let entity = Entity::try_from_bits(entities[0].as_u64().expect("entity bits"))
        .expect("a live entity id");
    assert!(
        app.world().get::<jackdaw::prefab::IsA>(entity).is_some(),
        "the reported id is not the instance root"
    );
}

/// Packing a group takes a root out of the document and puts one back, so the
/// count is where it was; the instance still has to be nameable.
#[test]
fn a_call_that_replaces_a_root_reports_only_the_one_it_added() {
    let (mut app, _project) = editor_with_a_project();
    call(
        &mut app,
        call_operator_handler,
        json!({ "id": "entity.add.cube" }),
    );
    app.update();
    let cube = call(
        &mut app,
        call_operator_handler,
        json!({ "id": "entity.add.sphere" }),
    );
    let sphere = Entity::try_from_bits(
        cube["entities"][0]
            .as_u64()
            .expect("the sphere was reported"),
    )
    .expect("a live entity id");

    let outcome = call(
        &mut app,
        call_operator_handler,
        json!({
            "id": "prefab.pack",
            "params": { "entity": sphere.to_bits(), "path": "prefabs/sphere.bsn" },
        }),
    );

    let entities = outcome["entities"].as_array().expect("an array");
    assert_eq!(entities.len(), 1, "{outcome}");
    let entity = Entity::try_from_bits(entities[0].as_u64().expect("entity bits"))
        .expect("a live entity id");
    assert!(
        app.world().get::<jackdaw::prefab::IsA>(entity).is_some(),
        "the reported id is not the instance the pack left behind"
    );
}

/// `root` takes a name as readily as an entity id: a caller reading the
/// tree knows what the nodes are called long before it knows their ids.
#[test]
fn the_tree_starts_from_a_root_named_by_name_or_by_id() {
    let mut app = editor_with_a_scene();
    let parent = app
        .world_mut()
        .spawn((Name::new("Ground"), Transform::default()))
        .id();
    app.world_mut()
        .spawn((Name::new("Rock"), Transform::default(), ChildOf(parent)));
    app.update();

    for root in [json!("Ground"), json!(parent.to_bits())] {
        let tree = call(&mut app, scene_tree_handler, json!({ "root": root }));
        let nodes = tree["tree"].as_array().expect("an array");
        assert_eq!(nodes.len(), 1, "{tree}");
        assert_eq!(nodes[0]["name"], json!("Ground"), "{tree}");
        assert_eq!(
            nodes[0]["children"]
                .as_array()
                .expect("an array")
                .first()
                .map(|child| child["name"].clone()),
            Some(json!("Rock")),
            "{tree}"
        );
    }
}

/// A caller places what the project already has: the tree reports what is
/// placed, not what is on disk.
#[test]
fn the_asset_listing_reports_project_files_matching_the_glob() {
    let (mut app, project) = editor_with_a_project();
    let assets = project.path().join("assets");
    std::fs::create_dir_all(assets.join("kit")).expect("a kit dir");
    std::fs::write(assets.join("kit/Prop_Fence_01.gltf"), "").expect("a model");
    std::fs::write(assets.join("village.bsn"), "").expect("a scene");

    let all = poll(&mut app, assets_handler, json!({ "request": "all" }));
    let listed: Vec<&str> = all["assets"]
        .as_array()
        .expect("an array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(
        listed,
        vec!["kit/Prop_Fence_01.gltf", "village.bsn"],
        "{all}"
    );

    let fences = poll(
        &mut app,
        assets_handler,
        json!({ "glob": "*Fence*", "request": "fences" }),
    );
    assert_eq!(
        fences["assets"],
        json!(["kit/Prop_Fence_01.gltf"]),
        "{fences}"
    );
}

/// Run one poll of a `pie_*` wait.
#[track_caller]
fn poll_pie(app: &mut App, params: Value) -> bevy::remote::BrpResult<Option<Value>> {
    app.world_mut()
        .run_system_cached_with(wait_handler, Some(params))
        .expect("the handler ran")
}

/// Set the editor's play state the way a launched game does, so a wait
/// has a transition to observe without a cargo build behind it.
fn set_play_state(app: &mut App, state: jackdaw_api::pie::PlayState) {
    app.world_mut()
        .insert_resource(bevy::state::state::State::new(state));
}

/// A caller that pressed play has nothing to poll but the state, and a launch is
/// a cargo build and then a process. `pie.play` returns while the editor still
/// reads as stopped, so a `pie_stopped` answered on the current state would
/// answer at once with the state the caller was trying to leave.
#[test]
fn a_pie_wait_answers_on_the_transition_rather_than_the_state_it_started_in() {
    let mut app = util::editor_test_app();
    let stopping = json!({ "until": "pie_stopped", "request": "stopping" });

    assert_eq!(
        poll_pie(&mut app, stopping.clone()).expect("the handler did not refuse"),
        None,
        "a wait for a stopped game answered before the game it is waiting on ever ran"
    );

    set_play_state(&mut app, jackdaw_api::pie::PlayState::Playing);
    let running = poll_pie(
        &mut app,
        json!({ "until": "pie_running", "request": "running" }),
    )
    .expect("the handler did not refuse")
    .expect("a game that is up answers a wait for running");
    assert_eq!(running["pie"], json!("running"), "{running}");
    assert_eq!(
        poll_pie(&mut app, stopping.clone()).expect("the handler did not refuse"),
        None,
        "a running game answered a wait for a stopped one"
    );

    set_play_state(&mut app, jackdaw_api::pie::PlayState::Stopped);
    let stopped = poll_pie(&mut app, stopping)
        .expect("the handler did not refuse")
        .expect("a game that has been up and is gone answers")["pie"]
        .clone();
    assert_eq!(stopped, json!("stopped"));
}

/// A wait that would otherwise hold for its whole frame cap ends when the
/// frames run out, so a caller is told rather than left on the line.
#[test]
fn a_pie_wait_gives_up_when_its_frames_run_out() {
    let mut app = util::editor_test_app();
    let params = json!({ "until": "pie_running", "frames": 1, "request": "brief" });

    assert_eq!(
        poll_pie(&mut app, params.clone()).expect("the handler did not refuse"),
        None,
        "the first poll gave up inside its own frame"
    );
    app.update();
    let refusal = poll_pie(&mut app, params).expect_err("the wait ran out of frames");
    assert!(
        refusal.message.contains("stopped"),
        "the refusal does not say what the game was doing: {}",
        refusal.message
    );
}

/// A call reports what it spawned and nothing else: the list is opened for the
/// call and taken back after it.
#[test]
fn a_call_reports_only_the_entities_it_spawned_itself() {
    let mut app = editor_with_a_scene();
    let first = call(
        &mut app,
        call_operator_handler,
        json!({ "id": "entity.add.cube" }),
    );
    assert_eq!(first["entities"].as_array().map(Vec::len), Some(1));

    // A spawn nobody is waiting on: the editor's own menus reach the same
    // command, and the ids they mint have no caller to be reported to.
    app.world_mut()
        .operator("entity.add.cube")
        .call()
        .expect("entity.add.cube dispatches")
        .assert_finished();
    app.update();

    let second = call(
        &mut app,
        call_operator_handler,
        json!({ "id": "entity.add.sphere" }),
    );
    let entities = second["entities"].as_array().expect("an array");
    assert_eq!(entities.len(), 1, "{second}");
    let entity = Entity::try_from_bits(entities[0].as_u64().expect("entity bits"))
        .expect("a live entity id");
    assert_eq!(
        app.world().get::<Name>(entity).map(Name::as_str),
        Some("Sphere"),
        "a call was handed an entity another one spawned"
    );
}

/// A synthetic key chord plays out over the frames after the call returns, so
/// the wait for idle holds until it has been delivered: a caller that reads the
/// scene after the wait sees what the chord did.
#[test]
fn waiting_for_idle_holds_until_a_synthetic_chord_has_landed() {
    let mut app = editor_with_a_scene();
    call(
        &mut app,
        call_operator_handler,
        json!({ "id": "entity.add.cube" }),
    );
    app.update();
    let cubes = |app: &mut App| {
        let tree = call(app, scene_tree_handler, json!({}));
        tree["tree"]
            .as_array()
            .expect("an array of roots")
            .iter()
            .filter(|node| node["name"] == json!("Cube"))
            .count()
    };
    assert_eq!(cubes(&mut app), 1, "the cube arrived");

    let outcome = call(
        &mut app,
        call_operator_handler,
        json!({
            "id": "input.key",
            "params": { "key": "KeyZ", "mods": util::primary_mod() },
        }),
    );
    assert_eq!(outcome["result"], json!("finished"), "{outcome}");
    let held = app
        .world_mut()
        .run_system_cached_with(wait_handler, Some(json!({ "until": "idle" })))
        .expect("the handler ran")
        .expect("the handler did not refuse");
    assert_eq!(held, None, "idle answered before the chord was delivered");

    let idle = poll(&mut app, wait_handler, json!({ "until": "idle" }));
    assert_eq!(idle["idle"], json!(true));
    assert_eq!(cubes(&mut app), 0, "the undo chord took the cube away");
}

/// The entity a compact outcome reports first.
fn first_spawned(outcome: &Value) -> Entity {
    let bits = outcome["entities"][0]
        .as_u64()
        .unwrap_or_else(|| panic!("no entity reported: {outcome}"));
    Entity::try_from_bits(bits).expect("a live entity id")
}

/// A later call names what an earlier call in the same batch made, so a parent
/// and the nodes under it build in one round trip.
#[test]
fn a_batch_call_names_an_earlier_calls_entity_by_reference() {
    let mut app = editor_with_a_scene();
    let outcome = call(
        &mut app,
        batch_handler,
        json!({
            "calls": [
                { "id": "entity.add.group", "params": { "name": "Props" } },
                { "id": "entity.add.cube", "params": { "parent": "$0.entities.0" } },
            ],
        }),
    );
    app.update();

    let calls = outcome["calls"].as_array().expect("an array");
    assert_eq!(calls.len(), 2, "{outcome}");
    let group = first_spawned(&calls[0]);
    let cube = first_spawned(&calls[1]);
    assert_eq!(
        app.world().get::<ChildOf>(cube).map(ChildOf::parent),
        Some(group),
        "the cube did not land under the group the reference named: {outcome}"
    );
}

/// A reference that is malformed, or names a call that has not run before it,
/// is refused before anything runs.
#[test]
fn a_bad_batch_reference_is_refused_before_any_call_runs() {
    let mut app = editor_with_a_scene();
    let before = app.world().resource::<CommandHistory>().undo_stack.len();
    let roots = root_names(&mut app);

    for reference in [
        "$1.entities.0",
        "$7.entities.0",
        "$0",
        "$0.entities..0",
        "$0.colour",
        "$0x.entities",
    ] {
        let error = app
            .world_mut()
            .run_system_cached_with(
                batch_handler,
                Some(json!({
                    "calls": [
                        { "id": "entity.add.cube" },
                        { "id": "entity.add.cube", "params": { "parent": reference } },
                    ],
                })),
            )
            .expect("the handler ran")
            .expect_err("a bad reference is refused");
        assert_eq!(
            error.code,
            bevy::remote::error_codes::INVALID_PARAMS,
            "{reference}: {}",
            error.message
        );
        assert!(
            error.message.contains("call 1") && error.message.contains(reference),
            "{reference}: the refusal names neither the call nor the reference: {}",
            error.message
        );
        app.update();
    }

    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        before,
        "a refused batch pushed an undo entry"
    );
    assert_eq!(root_names(&mut app), roots, "a refused batch added a node");
}

/// A reference to a value the earlier call did not produce stops the batch at
/// the call that holds it.
#[test]
fn a_reference_to_a_missing_value_stops_the_batch_there() {
    let mut app = editor_with_a_scene();
    let error = app
        .world_mut()
        .run_system_cached_with(
            batch_handler,
            Some(json!({
                "calls": [
                    { "id": "entity.add.cube" },
                    { "id": "entity.add.cube", "params": { "parent": "$0.entities.4" } },
                    { "id": "entity.add.cube" },
                ],
            })),
        )
        .expect("the handler ran")
        .expect_err("a missing value stops the batch");
    assert!(
        error.message.contains("call 1") && error.message.contains("$0.entities.4"),
        "{}",
        error.message
    );
    let data = error.data.expect("the error carries what ran");
    assert_eq!(data["failed_at"], json!(1), "{data}");
    assert_eq!(data["calls"].as_array().map(Vec::len), Some(1), "{data}");
}

/// An outcome is compact unless the caller asks: the spawned ids and how the
/// call ended, without the operator id echoed back or empty lists.
#[test]
fn a_call_answers_compactly_unless_asked_to_be_verbose() {
    let mut app = editor_with_a_scene();
    let compact = call(
        &mut app,
        call_operator_handler,
        json!({ "id": "entity.add.cube" }),
    );
    app.update();
    assert_eq!(compact["result"], json!("finished"), "{compact}");
    assert_eq!(
        compact["entities"].as_array().map(Vec::len),
        Some(1),
        "{compact}"
    );
    for dropped in ["id", "warnings", "reports"] {
        assert!(compact.get(dropped).is_none(), "{dropped}: {compact}");
    }

    let verbose = call(
        &mut app,
        call_operator_handler,
        json!({ "id": "entity.add.cube", "verbose": true }),
    );
    app.update();
    assert_eq!(verbose["id"], json!("entity.add.cube"), "{verbose}");
    assert_eq!(verbose["warnings"], json!([]), "{verbose}");

    let batch = call(
        &mut app,
        batch_handler,
        json!({ "calls": [{ "id": "entity.add.cube" }] }),
    );
    app.update();
    assert!(batch["calls"][0].get("id").is_none(), "{batch}");
    let batch = call(
        &mut app,
        batch_handler,
        json!({ "calls": [{ "id": "entity.add.cube" }], "verbose": true }),
    );
    app.update();
    assert_eq!(batch["calls"][0]["id"], json!("entity.add.cube"), "{batch}");
}

/// One operator can be asked for by id, so a caller need not read the whole
/// catalogue to learn one schema.
#[test]
fn the_operator_list_answers_for_one_id() {
    let mut app = editor_with_a_scene();
    let listed = call(
        &mut app,
        operators_handler,
        json!({ "id": "entity.add.cube" }),
    );
    let operators = listed["operators"].as_array().expect("an array");
    assert_eq!(operators.len(), 1, "{listed}");
    assert_eq!(operators[0]["id"], json!("entity.add.cube"));
    assert!(operators[0]["params"].is_array(), "{listed}");

    let none = call(&mut app, operators_handler, json!({ "id": "entity.add" }));
    assert_eq!(
        none["operators"],
        json!([]),
        "an id is not a prefix: {none}"
    );
}

/// A `max_px` capture also writes a thumbnail that fits inside that many
/// pixels, in the format asked for, and keeps the full capture where it was.
#[test]
fn a_screenshot_writes_a_thumbnail_within_max_px() {
    use bevy::image::ToExtents as _;
    use bevy::render::render_resource::{TextureDimension, TextureFormat};
    use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};

    let (mut app, _project) = editor_with_a_project();
    let params = Some(json!({
        "kind": "window",
        "path": "thumbed.png",
        "request": "one",
        "max_px": 64,
        "format": "jpeg",
    }));
    let first = app
        .world_mut()
        .run_system_cached_with(screenshot_handler, params.clone())
        .expect("the handler ran")
        .expect("a window capture queues");
    assert!(first.is_none());

    let shot = app
        .world_mut()
        .query_filtered::<Entity, With<Screenshot>>()
        .single(app.world())
        .expect("one capture queued");
    app.world_mut().trigger(ScreenshotCaptured {
        entity: shot,
        image: Image::new_fill(
            UVec2::new(200, 100).to_extents(),
            TextureDimension::D2,
            &[255, 0, 0, 255],
            TextureFormat::Rgba8UnormSrgb,
            default(),
        ),
    });
    let answer = app
        .world_mut()
        .run_system_cached_with(screenshot_handler, params)
        .expect("the handler ran")
        .expect("the handler answered")
        .expect("a landed capture answers");
    assert_eq!(answer["width"], json!(200), "{answer}");
    assert_eq!(answer["height"], json!(100), "{answer}");
    let thumb = &answer["thumb"];
    assert_eq!(thumb["width"], json!(64), "{answer}");
    assert_eq!(thumb["height"], json!(32), "{answer}");
    let path = thumb["path"].as_str().expect("a thumbnail path");
    assert!(path.ends_with(".jpg"), "{answer}");
    let bytes = std::fs::read(path).expect("the thumbnail is on disk");
    assert_eq!(&bytes[..2], &[0xFF, 0xD8], "not a JPEG: {answer}");
    let full = answer["path"].as_str().expect("the capture path");
    assert!(
        std::fs::metadata(full).is_ok(),
        "the full capture went away"
    );
}

/// A thumbnail format the editor cannot write is the caller's mistake.
#[test]
fn an_unknown_thumbnail_format_is_an_invalid_params_error() {
    let (mut app, _project) = editor_with_a_project();
    let err = app
        .world_mut()
        .run_system_cached_with(
            screenshot_handler,
            Some(json!({ "kind": "window", "max_px": 64, "format": "gif" })),
        )
        .expect("the handler ran")
        .expect_err("gif is refused");
    assert_eq!(err.code, bevy::remote::error_codes::INVALID_PARAMS);
}

/// A call that holds a reference still has its other parameters typed before
/// anything runs, so a mistake there refuses the whole batch.
#[test]
fn a_referencing_calls_other_parameters_are_checked_before_any_call_runs() {
    let mut app = editor_with_a_scene();
    let roots = root_names(&mut app);
    let error = app
        .world_mut()
        .run_system_cached_with(
            batch_handler,
            Some(json!({
                "calls": [
                    { "id": "entity.add.group", "params": { "name": "Props" } },
                    {
                        "id": "entity.add.cube",
                        "params": { "parent": "$0.entities.0", "size": "tall" },
                    },
                ],
            })),
        )
        .expect("the handler ran")
        .expect_err("a mistyped parameter is refused");
    assert!(error.message.contains("call 1"), "{}", error.message);
    app.update();
    assert_eq!(root_names(&mut app), roots, "a refused batch added a node");
}

/// Text that starts with `$` is spelled `$$`, and reaches the operator with
/// one `$`.
#[test]
fn a_doubled_dollar_reaches_the_operator_as_text() {
    let mut app = editor_with_a_scene();
    let outcome = call(
        &mut app,
        batch_handler,
        json!({
            "calls": [{ "id": "entity.add.group", "params": { "name": "$$100 Prop" } }],
        }),
    );
    app.update();
    let group = first_spawned(&outcome["calls"][0]);
    assert_eq!(
        app.world().get::<Name>(group).map(Name::as_str),
        Some("$100 Prop"),
        "{outcome}"
    );
}

/// The batch failure names the operator of the call that failed.
#[test]
fn a_batch_failure_names_the_failing_operator() {
    let mut app = editor_with_a_scene();
    let error = app
        .world_mut()
        .run_system_cached_with(
            batch_handler,
            Some(json!({
                "calls": [
                    { "id": "entity.add.cube" },
                    { "id": "entity.add.cube", "params": { "parent": "$0.entities.4" } },
                ],
            })),
        )
        .expect("the handler ran")
        .expect_err("a missing value stops the batch");
    assert!(
        error.message.contains("call 1 (entity.add.cube)"),
        "{}",
        error.message
    );
}

/// A capture with no path is matched by the path made when it was queued,
/// even when it lands in a later second than the one that path is stamped
/// with.
#[test]
fn an_unnamed_screenshot_is_answered_when_it_lands_a_second_later() {
    use bevy::image::ToExtents as _;
    use bevy::render::render_resource::{TextureDimension, TextureFormat};
    use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};

    let (mut app, _project) = editor_with_a_project();
    let params = Some(json!({ "kind": "window", "request": "unnamed" }));
    let first = app
        .world_mut()
        .run_system_cached_with(screenshot_handler, params.clone())
        .expect("the handler ran")
        .expect("a window capture queues");
    assert!(first.is_none());

    std::thread::sleep(std::time::Duration::from_millis(1100));
    let shot = app
        .world_mut()
        .query_filtered::<Entity, With<Screenshot>>()
        .single(app.world())
        .expect("one capture queued");
    app.world_mut().trigger(ScreenshotCaptured {
        entity: shot,
        image: Image::new_fill(
            UVec2::new(40, 20).to_extents(),
            TextureDimension::D2,
            &[0, 0, 0, 255],
            TextureFormat::Rgba8UnormSrgb,
            default(),
        ),
    });
    let answer = app
        .world_mut()
        .run_system_cached_with(screenshot_handler, params)
        .expect("the handler ran")
        .expect("the handler answered")
        .expect("the capture is matched by the path it was queued with");
    assert_eq!(answer["width"], json!(40), "{answer}");
}

/// A capture that already fits as a PNG is its own thumbnail; nothing extra
/// is written.
#[test]
fn a_capture_that_fits_is_its_own_thumbnail() {
    use bevy::image::ToExtents as _;
    use bevy::render::render_resource::{TextureDimension, TextureFormat};
    use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};

    let (mut app, _project) = editor_with_a_project();
    let params = Some(json!({
        "kind": "window",
        "path": "small.png",
        "request": "one",
        "max_px": 512,
    }));
    app.world_mut()
        .run_system_cached_with(screenshot_handler, params.clone())
        .expect("the handler ran")
        .expect("a window capture queues");
    let shot = app
        .world_mut()
        .query_filtered::<Entity, With<Screenshot>>()
        .single(app.world())
        .expect("one capture queued");
    app.world_mut().trigger(ScreenshotCaptured {
        entity: shot,
        image: Image::new_fill(
            UVec2::new(100, 50).to_extents(),
            TextureDimension::D2,
            &[0, 255, 0, 255],
            TextureFormat::Rgba8UnormSrgb,
            default(),
        ),
    });
    let answer = app
        .world_mut()
        .run_system_cached_with(screenshot_handler, params)
        .expect("the handler ran")
        .expect("the handler answered")
        .expect("a landed capture answers");
    assert_eq!(answer["thumb"]["path"], answer["path"], "{answer}");
    assert_eq!(answer["thumb"]["width"], json!(100), "{answer}");
}

/// An editor with a project and one viewport camera standing orthographic at
/// `(0, 0, 10)`, for screenshots that aim it.
fn editor_with_a_camera() -> (App, Entity, tempfile::TempDir) {
    let (mut app, project) = editor_with_a_project();
    let camera = app
        .world_mut()
        .spawn((
            jackdaw::viewport::MainViewportCamera,
            Transform::from_xyz(0.0, 0.0, 10.0),
            Projection::Orthographic(OrthographicProjection::default_3d()),
        ))
        .id();
    app.world_mut()
        .resource_mut::<jackdaw::viewport::ActiveViewport>()
        .camera = Some(camera);
    app.update();
    (app, camera, project)
}

/// Poll `params` once on the screenshot method.
#[track_caller]
fn screenshot_poll(app: &mut App, params: &Value) -> Option<Value> {
    app.world_mut()
        .run_system_cached_with(screenshot_handler, Some(params.clone()))
        .expect("the handler ran")
        .unwrap_or_else(|err| panic!("the screenshot refused: {}", err.message))
}

/// Stand in for the readback of the capture `name` under the project.
fn land_capture(app: &mut App, name: &str) {
    let project = app.world().resource::<ProjectRoot>().root.clone();
    let path = dunce::canonicalize(&project).unwrap().join(name);
    app.world_mut()
        .resource_mut::<jackdaw::screenshot::CaptureLog>()
        .record(path, (1280, 720));
}

/// Whether a capture is queued for the renderer.
fn capture_queued(app: &mut App) -> bool {
    app.world_mut()
        .query_filtered::<Entity, With<bevy::render::view::screenshot::Screenshot>>()
        .iter(app.world())
        .next()
        .is_some()
}

/// An aimed screenshot queues its capture a frame after aiming, once the
/// camera's world transform holds the aim, so the frame captured is the aimed
/// view. Once it lands the camera goes back where the user left it,
/// projection and all, and the poll after the answer neither captures nor
/// aims again.
#[test]
fn a_screenshot_that_aims_the_camera_captures_the_aim_and_puts_it_back() {
    let (mut app, camera, _project) = editor_with_a_camera();
    let params = json!({
        "kind": "window",
        "path": "aimed.png",
        "request": "aimed",
        "look_at": { "eye": [20.0, 15.0, 20.0], "target": [0.0, 0.0, 0.0] },
    });
    assert!(screenshot_poll(&mut app, &params).is_none());
    assert!(!capture_queued(&mut app), "queued in the frame it aimed");
    app.update();

    assert!(screenshot_poll(&mut app, &params).is_none());
    assert!(capture_queued(&mut app), "the capture was not queued");
    let at_capture = app
        .world()
        .get::<GlobalTransform>(camera)
        .expect("a world transform")
        .translation();
    assert_eq!(at_capture, Vec3::new(20.0, 15.0, 20.0));

    land_capture(&mut app, "aimed.png");
    let answer = screenshot_poll(&mut app, &params).expect("a landed capture answers");
    assert_eq!(answer["width"], json!(1280), "{answer}");
    let back = *app.world().get::<Transform>(camera).expect("a transform");
    assert_eq!(back, Transform::from_xyz(0.0, 0.0, 10.0));
    assert!(matches!(
        app.world().get::<Projection>(camera),
        Some(Projection::Orthographic(_))
    ));

    let trailing = app
        .world_mut()
        .run_system_cached_with(screenshot_handler, Some(params.clone()))
        .expect("the handler ran");
    let err = trailing.expect_err("an answered id is not a new screenshot");
    assert!(err.message.contains("already answered"), "{}", err.message);
    assert_eq!(
        *app.world().get::<Transform>(camera).expect("a transform"),
        back,
        "the poll after the answer aimed the camera again"
    );
}

/// `keep_camera` leaves the camera where the shot aimed it.
#[test]
fn a_screenshot_with_keep_camera_leaves_the_camera_aimed() {
    let (mut app, camera, _project) = editor_with_a_camera();
    let params = json!({
        "kind": "window",
        "path": "kept.png",
        "request": "kept",
        "keep_camera": true,
        "look_at": { "eye": [0.0, 30.0, 5.0] },
    });
    assert!(screenshot_poll(&mut app, &params).is_none());
    app.update();
    assert!(screenshot_poll(&mut app, &params).is_none());
    land_capture(&mut app, "kept.png");
    screenshot_poll(&mut app, &params).expect("a landed capture answers");
    let kept = app.world().get::<Transform>(camera).expect("a transform");
    assert_eq!(kept.translation, Vec3::new(0.0, 30.0, 5.0));
    assert!(matches!(
        app.world().get::<Projection>(camera),
        Some(Projection::Perspective(_))
    ));
}

/// A `look_at` is refused before anything moves when it has no eye, no
/// request id, or another aimed screenshot is still capturing.
#[test]
fn an_aimed_screenshot_is_refused_where_it_could_not_put_the_camera_back() {
    let (mut app, camera, _project) = editor_with_a_camera();
    let home = Transform::from_xyz(0.0, 0.0, 10.0);
    for (params, says) in [
        (
            json!({ "kind": "window", "request": "a", "look_at": { "target": [1.0, 0.0, 0.0] } }),
            "eye",
        ),
        (
            json!({ "kind": "window", "look_at": { "eye": [5.0, 5.0, 5.0] } }),
            "request",
        ),
    ] {
        let err = app
            .world_mut()
            .run_system_cached_with(screenshot_handler, Some(params.clone()))
            .expect("the handler ran")
            .expect_err("refused");
        assert_eq!(err.code, bevy::remote::error_codes::INVALID_PARAMS);
        assert!(err.message.contains(says), "{params}: {}", err.message);
        assert_eq!(*app.world().get::<Transform>(camera).unwrap(), home);
    }

    let first =
        json!({ "kind": "window", "request": "one", "look_at": { "eye": [5.0, 5.0, 5.0] } });
    assert!(screenshot_poll(&mut app, &first).is_none());
    let err = app
        .world_mut()
        .run_system_cached_with(
            screenshot_handler,
            Some(json!({ "kind": "window", "request": "two", "look_at": { "eye": [9.0, 9.0, 9.0] } })),
        )
        .expect("the handler ran")
        .expect_err("one aimed screenshot at a time");
    assert!(err.message.contains("still capturing"), "{}", err.message);
    assert_eq!(
        app.world().get::<Transform>(camera).unwrap().translation,
        Vec3::new(5.0, 5.0, 5.0)
    );
}

/// Make every pending request stale, then poll an unrelated screenshot so
/// the stale ones expire.
fn expire_pending(app: &mut App) {
    app.world_mut()
        .resource_mut::<bevy::diagnostic::FrameCount>()
        .0 += 1000;
    screenshot_poll(
        app,
        &json!({ "kind": "window", "path": "other.png", "request": "other" }),
    );
}

/// A capture abandoned by its caller puts the camera back when it expires,
/// unless the user has moved the camera since the aim.
#[test]
fn an_abandoned_aim_puts_the_camera_back_only_if_nobody_moved_it() {
    let (mut app, camera, _project) = editor_with_a_camera();
    let aimed =
        json!({ "kind": "window", "request": "left", "look_at": { "eye": [5.0, 5.0, 5.0] } });
    assert!(screenshot_poll(&mut app, &aimed).is_none());
    expire_pending(&mut app);
    assert_eq!(
        *app.world().get::<Transform>(camera).unwrap(),
        Transform::from_xyz(0.0, 0.0, 10.0)
    );

    let aimed =
        json!({ "kind": "window", "request": "moved", "look_at": { "eye": [5.0, 5.0, 5.0] } });
    assert!(screenshot_poll(&mut app, &aimed).is_none());
    let users = Transform::from_xyz(-3.0, 2.0, 7.0);
    app.world_mut().entity_mut(camera).insert(users);
    expire_pending(&mut app);
    assert_eq!(*app.world().get::<Transform>(camera).unwrap(), users);
}
