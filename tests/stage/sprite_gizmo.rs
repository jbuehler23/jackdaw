//! The 2D move, rotate and scale gizmo on sprites of a 2D world scene, and the
//! arrow-key nudge, driven through the synthetic pointer in world coordinates.

use crate::util;

use crate::util::OperatorResultExt as _;
use bevy::{
    camera::visibility::RenderLayers,
    gizmos::config::GizmoConfigStore,
    prelude::*,
    sprite::Anchor,
    window::{PrimaryWindow, WindowResolution},
};
use jackdaw::{
    active_tool::ActiveTool,
    canvas_snap::CanvasSnap,
    commands::CommandHistory,
    gizmos::TransformGizmoGroup,
    selection::Selection,
    sprite_gizmo::{SpriteGizmoHandle, SpriteHandle, SpriteManipulation},
    test_input::SyntheticInput,
    viewport::MainViewportCamera,
    viewport_2d::{
        RULER_SIZE, Ui2dView, Viewport2dMode, Viewport2dPanelHost, build_viewport_2d_panel,
    },
};
use jackdaw_api::prelude::*;
use jackdaw_scene_types::UiSceneRoot;

const VIEW: Ui2dView = Ui2dView {
    pan: Vec2::new(100.0, -20.0),
    zoom: 2.0,
    grid: 8.0,
};

const TOOLS: [&str; 4] = ["tool.select", "tool.translate", "tool.rotate", "tool.scale"];

pub(crate) struct Scene {
    pub(crate) panel: Entity,
    pub(crate) root: Entity,
    /// 20x20, centre-anchored at the origin.
    pub(crate) tile: Entity,
    /// 20x20, centre-anchored at (100, 0).
    pub(crate) other: Entity,
    /// 40x20, bottom-left anchored at (-100, 50).
    pub(crate) corner: Entity,
}

pub(crate) fn world_app() -> (App, Scene) {
    let mut app = util::editor_test_app();
    util::fixed_frame_clock(&mut app);
    {
        let mut windows = app
            .world_mut()
            .query_filtered::<&mut Window, With<PrimaryWindow>>();
        let mut window = windows
            .single_mut(app.world_mut())
            .expect("headless apps still have a primary window");
        window.resolution = WindowResolution::new(1600, 1000);
    }
    let panel = app
        .world_mut()
        .spawn((
            jackdaw::EditorEntity,
            Node {
                width: px(1200.0 + RULER_SIZE),
                height: px(600.0 + RULER_SIZE + jackdaw_feathers::tokens::TOOLBAR_HEIGHT),
                ..default()
            },
        ))
        .id();
    build_viewport_2d_panel(app.world_mut(), panel);
    {
        let mut host = app
            .world_mut()
            .get_mut::<Viewport2dPanelHost>(panel)
            .expect("host on panel parent");
        host.set_view(VIEW);
        host.fit_pending = false;
    }

    let root = jackdaw::entity_ops::seed_2d_scene_root(app.world_mut());
    let square = || Sprite::from_color(Color::WHITE, Vec2::splat(20.0));
    let tile = sprite(&mut app, root, "Tile", square(), Anchor::CENTER, Vec3::ZERO);
    let other = sprite(
        &mut app,
        root,
        "Other",
        square(),
        Anchor::CENTER,
        Vec3::new(100.0, 0.0, 0.0),
    );
    let corner = sprite(
        &mut app,
        root,
        "Corner",
        Sprite::from_color(Color::WHITE, Vec2::new(40.0, 20.0)),
        Anchor::BOTTOM_LEFT,
        Vec3::new(-100.0, 50.0, 0.0),
    );
    jackdaw::selection::clear_selection_in_world(app.world_mut());
    // The gizmo's own arithmetic, free of the canvas snapping.
    app.world_mut().insert_resource(CanvasSnap {
        enabled: false,
        ..default()
    });
    settle(&mut app);
    (
        app,
        Scene {
            panel,
            root,
            tile,
            other,
            corner,
        },
    )
}

pub(crate) fn sprite(
    app: &mut App,
    root: Entity,
    name: &str,
    sprite: Sprite,
    anchor: Anchor,
    at: Vec3,
) -> Entity {
    let entity = app
        .world_mut()
        .spawn((
            Name::new(name.to_string()),
            sprite,
            anchor,
            Transform::from_translation(at),
            ChildOf(root),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    entity
}

pub(crate) fn settle(app: &mut App) {
    for _ in 0..8 {
        app.update();
    }
}

pub(crate) fn run(app: &mut App, clause: &str) {
    jackdaw::boot_ops::run_op_clause(app.world_mut(), clause)
        .expect("the clause dispatches")
        .assert_finished();
    for _ in 0..600 {
        app.update();
        if app.world().resource::<SyntheticInput>().is_idle() {
            break;
        }
    }
    assert!(
        app.world().resource::<SyntheticInput>().is_idle(),
        "the gesture drained"
    );
    settle(app);
}

pub(crate) fn pointer(app: &mut App, at: Vec2, action: &str, mods: &str) {
    let mods = if mods.is_empty() {
        String::new()
    } else {
        format!(" mods={mods}")
    };
    run(
        app,
        &format!(
            "input.pointer space=world x={} y={} action={action}{mods}",
            at.x, at.y
        ),
    );
}

pub(crate) fn drag(app: &mut App, from: Vec2, to: Vec2, mods: &str) {
    pointer(app, from, "press", "");
    pointer(app, to, "move", mods);
    pointer(app, to, "release", "");
}

pub(crate) fn select(app: &mut App, at: Vec2, tool: ActiveTool) {
    app.world_mut().insert_resource(tool);
    pointer(app, at, "click", "");
}

/// Where to grab the gizmo's `kind` handle, in world units.
fn handle(app: &mut App, kind: SpriteHandle) -> Vec2 {
    app.world_mut()
        .query::<&SpriteGizmoHandle>()
        .iter(app.world())
        .find(|handle| handle.kind == kind)
        .map(|handle| handle.world)
        .unwrap_or_else(|| panic!("the gizmo shows a {kind:?} handle"))
}

pub(crate) fn transform(app: &App, entity: Entity) -> Transform {
    *app.world()
        .get::<Transform>(entity)
        .expect("the sprite has a transform")
}

pub(crate) fn history_len(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

pub(crate) fn undo(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    settle(app);
}

fn redo(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.redo(world));
    settle(app);
}

pub(crate) fn near(a: Vec3, b: Vec3) -> bool {
    (a - b).length() < 1e-3
}

fn angle_of(transform: &Transform) -> f32 {
    transform.rotation.to_euler(EulerRot::ZYX).0.to_degrees()
}

fn available(app: &mut App, id: &'static str) -> bool {
    app.world_mut()
        .operator(id)
        .is_available()
        .unwrap_or_else(|err| panic!("{id}: is_available errored: {err}"))
}

#[test]
fn the_tools_are_available_on_a_2d_world_and_refused_on_a_ui_canvas() {
    let (mut app, scene) = world_app();
    pointer(&mut app, Vec2::ZERO, "move", "");
    for id in TOOLS {
        assert!(available(&mut app, id), "{id} works on a 2D world scene");
    }

    app.world_mut().entity_mut(scene.root).despawn();
    let ui = app
        .world_mut()
        .spawn((
            UiSceneRoot {
                reference_size: UVec2::new(1200, 600),
            },
            Name::new("UiRoot"),
            Node {
                width: percent(100),
                height: percent(100),
                ..default()
            },
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), ui);
    settle(&mut app);
    pointer(&mut app, Vec2::ZERO, "move", "");
    for id in TOOLS {
        assert!(!available(&mut app, id), "{id} is refused on a UI canvas");
    }
}

#[test]
fn the_3d_gizmo_draws_on_a_layer_the_2d_view_never_renders() {
    let (mut app, scene) = world_app();
    let gizmo = app
        .world()
        .resource::<GizmoConfigStore>()
        .config::<TransformGizmoGroup>()
        .0
        .render_layers
        .clone();
    let camera = app
        .world()
        .get::<Viewport2dPanelHost>(scene.panel)
        .expect("host on panel parent")
        .camera;
    let two_d = app
        .world()
        .get::<RenderLayers>(camera)
        .cloned()
        .unwrap_or_default();
    assert!(!gizmo.intersects(&two_d), "{gizmo:?} against {two_d:?}");
    let mut three_d = app
        .world_mut()
        .query_filtered::<Option<&RenderLayers>, With<MainViewportCamera>>();
    for layers in three_d.iter(app.world()) {
        let layers = layers.cloned().unwrap_or_default();
        assert!(gizmo.intersects(&layers), "a 3D viewport renders the gizmo");
    }
}

#[test]
fn the_tools_are_refused_on_a_2d_panel_that_is_playing() {
    let (mut app, scene) = world_app();
    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(scene.panel)
        .expect("host on panel parent")
        .mode = Viewport2dMode::Interact;
    settle(&mut app);
    pointer(&mut app, Vec2::ZERO, "move", "");
    for id in TOOLS {
        assert!(!available(&mut app, id), "{id} is refused while playing");
    }
    run(&mut app, "input.key key=KeyW");
    assert_eq!(*app.world().resource::<ActiveTool>(), ActiveTool::Select);
}

#[test]
fn a_body_drag_moves_the_selection_by_the_world_delta() {
    let (mut app, scene) = world_app();
    select(&mut app, Vec2::ZERO, ActiveTool::Translate);
    pointer(&mut app, Vec2::new(100.0, 0.0), "click", "shift");
    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![scene.tile, scene.other]
    );
    let entries = history_len(&app);

    drag(&mut app, Vec2::new(2.0, 3.0), Vec2::new(32.0, -17.0), "");
    assert!(near(
        transform(&app, scene.tile).translation,
        Vec3::new(30.0, -20.0, 0.0)
    ));
    assert!(near(
        transform(&app, scene.other).translation,
        Vec3::new(130.0, -20.0, 0.0)
    ));
    assert_eq!(history_len(&app), entries + 1, "one drag, one entry");

    undo(&mut app);
    assert_eq!(transform(&app, scene.tile), Transform::IDENTITY);
    assert_eq!(
        transform(&app, scene.other),
        Transform::from_xyz(100.0, 0.0, 0.0)
    );
    redo(&mut app);
    assert!(near(
        transform(&app, scene.other).translation,
        Vec3::new(130.0, -20.0, 0.0)
    ));
}

#[test]
fn the_axis_handles_keep_to_their_axis() {
    let (mut app, scene) = world_app();
    select(&mut app, Vec2::ZERO, ActiveTool::Translate);

    let x = handle(&mut app, SpriteHandle::AxisX);
    drag(&mut app, x, x + Vec2::new(25.0, 40.0), "");
    assert!(near(
        transform(&app, scene.tile).translation,
        Vec3::new(25.0, 0.0, 0.0)
    ));

    let y = handle(&mut app, SpriteHandle::AxisY);
    drag(&mut app, y, y + Vec2::new(-15.0, 10.0), "");
    assert!(near(
        transform(&app, scene.tile).translation,
        Vec3::new(25.0, 10.0, 0.0)
    ));
}

#[test]
fn escape_mid_drag_restores_and_records_nothing() {
    let (mut app, scene) = world_app();
    select(&mut app, Vec2::ZERO, ActiveTool::Translate);
    let entries = history_len(&app);

    pointer(&mut app, Vec2::new(2.0, 3.0), "press", "");
    pointer(&mut app, Vec2::new(40.0, 30.0), "move", "");
    assert!(app.world().resource::<SpriteManipulation>().is_running());
    assert!(near(
        transform(&app, scene.tile).translation,
        Vec3::new(38.0, 27.0, 0.0)
    ));
    run(&mut app, "input.key key=Escape");
    assert_eq!(transform(&app, scene.tile), Transform::IDENTITY);
    assert!(!app.world().resource::<SpriteManipulation>().is_running());
    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![scene.tile],
        "Escape took the drag, not the selection"
    );
    pointer(&mut app, Vec2::new(50.0, 30.0), "move", "");
    pointer(&mut app, Vec2::new(50.0, 30.0), "release", "");
    assert_eq!(transform(&app, scene.tile), Transform::IDENTITY);
    assert_eq!(history_len(&app), entries);
}

#[test]
fn escape_with_no_drag_still_clears_the_selection() {
    let (mut app, scene) = world_app();
    select(&mut app, Vec2::ZERO, ActiveTool::Translate);
    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![scene.tile]
    );
    run(&mut app, "input.key key=Escape");
    assert!(app.world().resource::<Selection>().entities.is_empty());
}

#[test]
fn a_click_on_one_of_several_selected_sprites_narrows_to_it() {
    let (mut app, scene) = world_app();
    select(&mut app, Vec2::ZERO, ActiveTool::Translate);
    pointer(&mut app, Vec2::new(100.0, 0.0), "click", "shift");
    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![scene.tile, scene.other]
    );

    pointer(&mut app, Vec2::new(2.0, 3.0), "press", "");
    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![scene.tile, scene.other],
        "the press keeps the selection for a drag"
    );
    pointer(&mut app, Vec2::new(2.0, 3.0), "release", "");
    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![scene.tile],
        "released without a drag, the click narrows"
    );
    assert_eq!(transform(&app, scene.tile), Transform::IDENTITY);
}

#[test]
fn a_tool_key_mid_drag_neither_switches_nor_strands_the_drag() {
    let (mut app, scene) = world_app();
    select(&mut app, Vec2::ZERO, ActiveTool::Translate);
    let entries = history_len(&app);

    pointer(&mut app, Vec2::new(2.0, 3.0), "press", "");
    pointer(&mut app, Vec2::new(32.0, -17.0), "move", "");
    run(&mut app, "input.key key=KeyE");
    assert_eq!(*app.world().resource::<ActiveTool>(), ActiveTool::Translate);
    pointer(&mut app, Vec2::new(42.0, -17.0), "move", "");
    pointer(&mut app, Vec2::new(42.0, -17.0), "release", "");
    assert!(!app.world().resource::<SpriteManipulation>().is_running());
    assert!(near(
        transform(&app, scene.tile).translation,
        Vec3::new(40.0, -20.0, 0.0)
    ));
    assert_eq!(history_len(&app), entries + 1);
    let tmp = tempfile::tempdir().expect("tempdir");
    let text = jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), tmp.path())
        .expect("the scene emits");
    assert!(
        text.contains("x: 40.0"),
        "the document has the move:\n{text}"
    );

    run(&mut app, "input.key key=KeyE");
    assert_eq!(
        *app.world().resource::<ActiveTool>(),
        ActiveTool::Rotate,
        "with no drag running the key switches the tool"
    );
}

#[test]
fn undo_mid_drag_cancels_the_drag_first() {
    let (mut app, scene) = world_app();
    select(&mut app, Vec2::ZERO, ActiveTool::Translate);
    let entries = history_len(&app);
    drag(&mut app, Vec2::new(2.0, 3.0), Vec2::new(32.0, -17.0), "");

    pointer(&mut app, Vec2::new(32.0, -17.0), "press", "");
    pointer(&mut app, Vec2::new(52.0, -17.0), "move", "");
    assert!(app.world().resource::<SpriteManipulation>().is_running());
    run(&mut app, "history.undo");
    assert!(!app.world().resource::<SpriteManipulation>().is_running());
    assert_eq!(transform(&app, scene.tile), Transform::IDENTITY);
    pointer(&mut app, Vec2::new(60.0, -17.0), "move", "");
    pointer(&mut app, Vec2::new(60.0, -17.0), "release", "");
    assert_eq!(transform(&app, scene.tile), Transform::IDENTITY);
    assert_eq!(history_len(&app), entries);

    run(&mut app, "history.redo");
    assert!(near(
        transform(&app, scene.tile).translation,
        Vec3::new(30.0, -20.0, 0.0)
    ));
}

#[test]
fn a_reload_mid_drag_forgets_the_drag() {
    let (mut app, scene) = world_app();
    select(&mut app, Vec2::ZERO, ActiveTool::Translate);
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("again.bsn");
    let text = jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), tmp.path())
        .expect("the scene emits");
    std::fs::write(&path, text).expect("write the scene");

    pointer(&mut app, Vec2::new(2.0, 3.0), "press", "");
    pointer(&mut app, Vec2::new(32.0, -17.0), "move", "");
    assert!(app.world().resource::<SpriteManipulation>().is_running());
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &path);
    settle(&mut app);
    assert!(
        app.world().get_entity(scene.tile).is_err(),
        "the reload replaced the scene"
    );
    assert!(!app.world().resource::<SpriteManipulation>().is_running());
    let entries = history_len(&app);
    pointer(&mut app, Vec2::new(40.0, -17.0), "move", "");
    pointer(&mut app, Vec2::new(40.0, -17.0), "release", "");
    assert_eq!(
        history_len(&app),
        entries,
        "the forgotten drag records nothing"
    );
}

#[test]
fn a_jittery_click_still_narrows_and_records_nothing() {
    let (mut app, scene) = world_app();
    select(&mut app, Vec2::ZERO, ActiveTool::Translate);
    pointer(&mut app, Vec2::new(100.0, 0.0), "click", "shift");
    let entries = history_len(&app);

    pointer(&mut app, Vec2::new(2.0, 3.0), "press", "");
    pointer(&mut app, Vec2::new(2.5, 3.0), "move", "");
    pointer(&mut app, Vec2::new(2.5, 3.0), "release", "");
    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![scene.tile]
    );
    assert_eq!(transform(&app, scene.tile), Transform::IDENTITY);
    assert_eq!(history_len(&app), entries);
}

/// A group with no sprite of its own, turned a quarter and doubled, holding
/// one sprite at its origin.
fn group_with_kid(app: &mut App, root: Entity) -> (Entity, Entity) {
    let group = app
        .world_mut()
        .spawn((
            Name::new("Group"),
            Transform::from_xyz(-100.0, 50.0, 0.0)
                .with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2))
                .with_scale(Vec3::splat(2.0)),
            ChildOf(root),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), group);
    let kid = sprite(
        app,
        group,
        "Kid",
        Sprite::from_color(Color::WHITE, Vec2::splat(20.0)),
        Anchor::CENTER,
        Vec3::ZERO,
    );
    settle(app);
    (group, kid)
}

fn world_position(app: &App, entity: Entity) -> Vec3 {
    app.world()
        .get::<GlobalTransform>(entity)
        .expect("propagated")
        .translation()
}

#[test]
fn a_body_drag_moves_a_sprite_whose_selected_group_is_no_sprite() {
    let (mut app, scene) = world_app();
    let (group, kid) = group_with_kid(&mut app, scene.root);
    app.world_mut().insert_resource(ActiveTool::Translate);
    jackdaw::selection::select_only(app.world_mut(), group);
    pointer(&mut app, Vec2::new(-100.0, 50.0), "click", "shift");
    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![group, kid]
    );

    drag(&mut app, Vec2::new(-98.0, 52.0), Vec2::new(-68.0, 42.0), "");
    assert!(near(world_position(&app, kid), Vec3::new(-70.0, 40.0, 0.0)));
    assert!(near(
        transform(&app, group).translation,
        Vec3::new(-100.0, 50.0, 0.0)
    ));
}

fn nudge(app: &mut App, id: &'static str) {
    app.world_mut()
        .operator(id)
        .call()
        .expect("the nudge operator is registered")
        .assert_finished();
    settle(app);
}

#[test]
fn a_nudge_steps_in_the_world_through_a_turned_parent() {
    let (mut app, scene) = world_app();
    let (_group, kid) = group_with_kid(&mut app, scene.root);
    jackdaw::selection::select_only(app.world_mut(), kid);
    nudge(&mut app, "transform.nudge_z_neg");
    assert!(near(
        world_position(&app, kid),
        Vec3::new(-100.0, 51.0, 0.0)
    ));
}

#[test]
fn a_nudge_moves_a_selected_parent_and_child_once() {
    let (mut app, scene) = world_app();
    let pup = sprite(
        &mut app,
        scene.tile,
        "Pup",
        Sprite::from_color(Color::WHITE, Vec2::splat(4.0)),
        Anchor::CENTER,
        Vec3::new(5.0, 0.0, 0.0),
    );
    jackdaw::selection::select_only(app.world_mut(), scene.tile);
    app.world_mut()
        .resource_mut::<Selection>()
        .entities
        .push(pup);
    let entries = history_len(&app);
    nudge(&mut app, "transform.nudge_x_pos");
    assert!(near(
        transform(&app, scene.tile).translation,
        Vec3::new(1.0, 0.0, 0.0)
    ));
    assert!(near(
        transform(&app, pup).translation,
        Vec3::new(5.0, 0.0, 0.0)
    ));
    assert_eq!(history_len(&app), entries + 1);
}

#[test]
fn the_page_keys_leave_a_sprite_where_it_is() {
    let (mut app, scene) = world_app();
    jackdaw::selection::select_only(app.world_mut(), scene.tile);
    let entries = history_len(&app);
    nudge(&mut app, "transform.nudge_y_pos");
    nudge(&mut app, "transform.nudge_y_neg");
    assert_eq!(transform(&app, scene.tile), Transform::IDENTITY);
    assert_eq!(history_len(&app), entries);
}

#[test]
fn a_saved_and_reopened_scene_keeps_the_dragged_transform() {
    let (mut app, _scene) = world_app();
    select(&mut app, Vec2::ZERO, ActiveTool::Translate);
    drag(&mut app, Vec2::new(2.0, 3.0), Vec2::new(32.0, 13.0), "");

    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("moved.bsn");
    let text = jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), tmp.path())
        .expect("the scene emits");
    assert!(
        !text.contains("bevy_mesh::mesh::Mesh") && !text.contains("SpriteMeshMaterial"),
        "the renderer's quad and material stay out of the file:\n{text}"
    );
    std::fs::write(&path, text).expect("write the scene");

    let mut app = util::editor_test_app();
    jackdaw::migrate_dialog::request_open_with_conversion(app.world_mut(), &path);
    settle(&mut app);
    let (tile, size) = app
        .world_mut()
        .query::<(&Name, &Transform, &Sprite)>()
        .iter(app.world())
        .find(|(name, ..)| name.as_str() == "Tile")
        .map(|(_, transform, sprite)| (*transform, sprite.custom_size))
        .expect("the reopened scene has the tile, still a sprite");
    assert!(
        near(tile.translation, Vec3::new(30.0, 10.0, 0.0)),
        "{tile:?}"
    );
    assert_eq!(size, Some(Vec2::splat(20.0)));
}

/// Only a sprite's renderer quad stays out of a save: a `Mesh2d` an author
/// put on an entity of their own is written.
#[test]
fn an_authored_mesh2d_still_saves() {
    let (mut app, scene) = world_app();
    let mesh = app
        .world()
        .resource::<AssetServer>()
        .load::<Mesh>("shapes/quad.glb#Mesh0/Primitive0");
    let quad = app
        .world_mut()
        .spawn((
            Name::new("Quad"),
            Mesh2d(mesh),
            Transform::default(),
            ChildOf(scene.root),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), quad);
    settle(&mut app);
    let tmp = tempfile::tempdir().expect("tempdir");
    let text = jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), tmp.path())
        .expect("the scene emits");
    assert_eq!(
        text.matches("bevy_mesh::components::Mesh2d").count(),
        1,
        "the quad keeps its mesh and the sprites write none:\n{text}"
    );
    assert!(text.contains("shapes/quad.glb"), "{text}");
}

#[test]
fn the_arrows_nudge_a_sprite_along_world_y_not_z() {
    let (mut app, scene) = world_app();
    select(&mut app, Vec2::ZERO, ActiveTool::Select);
    let entries = history_len(&app);
    nudge(&mut app, "transform.nudge_z_neg");
    assert!(near(
        transform(&app, scene.tile).translation,
        Vec3::new(0.0, 1.0, 0.0)
    ));
    assert_eq!(history_len(&app), entries + 1, "one press, one entry");

    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::ShiftLeft);
    nudge(&mut app, "transform.nudge_x_pos");
    assert!(near(
        transform(&app, scene.tile).translation,
        Vec3::new(VIEW.grid, 1.0, 0.0)
    ));
}

#[test]
fn a_rotate_drag_turns_the_sprite_about_its_anchor() {
    let (mut app, scene) = world_app();
    select(&mut app, Vec2::new(-90.0, 60.0), ActiveTool::Rotate);
    let anchor = Vec2::new(-100.0, 50.0);
    let knob = handle(&mut app, SpriteHandle::Rotate);
    let reach = (knob - anchor).length();
    assert!(
        (knob - anchor).normalize().abs_diff_eq(Vec2::Y, 1e-3),
        "the knob sits above the anchor"
    );

    drag(&mut app, knob, anchor + Vec2::new(-reach, 0.0), "");
    let turned = transform(&app, scene.corner);
    assert!((angle_of(&turned) - 90.0).abs() < 1e-2, "a quarter turn");
    assert!(
        near(turned.translation, anchor.extend(0.0)),
        "the anchor holds"
    );
    let far_corner = app
        .world()
        .get::<GlobalTransform>(scene.corner)
        .expect("propagated")
        .transform_point(Vec3::new(40.0, 0.0, 0.0));
    assert!(near(far_corner, Vec3::new(-100.0, 90.0, 0.0)));
}

#[test]
fn ctrl_turns_in_rotate_increments() {
    let (mut app, scene) = world_app();
    select(&mut app, Vec2::ZERO, ActiveTool::Rotate);
    let knob = handle(&mut app, SpriteHandle::Rotate);
    let reach = knob.length();
    let at = Vec2::from_angle(140f32.to_radians()) * reach;
    drag(&mut app, knob, at, "ctrl");
    let angle = angle_of(&transform(&app, scene.tile));
    assert!(
        (angle - 45.0).abs() < 1e-2,
        "50 degrees lands on 45: {angle}"
    );
}

#[test]
fn a_scale_drag_holds_the_opposite_edge() {
    let (mut app, scene) = world_app();
    select(&mut app, Vec2::ZERO, ActiveTool::Scale);
    let right = handle(&mut app, SpriteHandle::Scale(1, 0));
    assert!(right.abs_diff_eq(Vec2::new(10.0, 0.0), 1e-3));

    drag(&mut app, right, Vec2::new(20.0, 3.0), "");
    let scaled = transform(&app, scene.tile);
    assert!(near(scaled.scale, Vec3::new(1.5, 1.0, 1.0)));
    assert!(near(scaled.translation, Vec3::new(5.0, 0.0, 0.0)));
}

#[test]
fn shift_scales_uniformly_and_ctrl_in_increments() {
    let (mut app, scene) = world_app();
    select(&mut app, Vec2::ZERO, ActiveTool::Scale);
    let corner = handle(&mut app, SpriteHandle::Scale(1, 1));
    drag(&mut app, corner, Vec2::new(20.0, 12.0), "shift");
    let scaled = transform(&app, scene.tile);
    assert!(near(scaled.scale, Vec3::new(1.3, 1.3, 1.0)), "{scaled:?}");
    assert!(near(scaled.translation, Vec3::new(3.0, 3.0, 0.0)));

    undo(&mut app);
    let right = handle(&mut app, SpriteHandle::Scale(1, 0));
    drag(&mut app, right, Vec2::new(20.6, 0.0), "ctrl");
    let scaled = transform(&app, scene.tile);
    assert!(near(scaled.scale, Vec3::new(1.5, 1.0, 1.0)), "{scaled:?}");
}

fn set_transform(app: &mut App, entity: Entity, transform: Transform) {
    *app.world_mut()
        .get_mut::<Transform>(entity)
        .expect("the sprite has a transform") = transform;
    settle(app);
}

fn world_x_axis(app: &App, entity: Entity) -> Vec2 {
    app.world()
        .get::<GlobalTransform>(entity)
        .expect("propagated")
        .affine()
        .matrix3
        .x_axis
        .truncate()
}

#[test]
fn a_move_takes_a_selection_whose_primary_is_no_sprite() {
    let (mut app, scene) = world_app();
    let (group, kid) = group_with_kid(&mut app, scene.root);
    app.world_mut().insert_resource(ActiveTool::Translate);
    jackdaw::selection::select_only(app.world_mut(), kid);
    app.world_mut()
        .resource_mut::<Selection>()
        .entities
        .push(group);
    settle(&mut app);
    drag(&mut app, Vec2::new(-98.0, 52.0), Vec2::new(-68.0, 42.0), "");
    assert!(near(world_position(&app, kid), Vec3::new(-70.0, 40.0, 0.0)));
}

#[test]
fn a_move_takes_a_selection_whose_primary_image_never_loaded() {
    let (mut app, scene) = world_app();
    let missing = app
        .world()
        .resource::<AssetServer>()
        .load::<Image>("no/such/image.png");
    let ghost = sprite(
        &mut app,
        scene.root,
        "Ghost",
        Sprite {
            image: missing,
            ..default()
        },
        Anchor::CENTER,
        Vec3::new(-150.0, -100.0, 0.0),
    );
    select(&mut app, Vec2::ZERO, ActiveTool::Translate);
    app.world_mut()
        .resource_mut::<Selection>()
        .entities
        .push(ghost);
    settle(&mut app);
    drag(&mut app, Vec2::new(2.0, 3.0), Vec2::new(32.0, -17.0), "");
    assert!(near(
        transform(&app, scene.tile).translation,
        Vec3::new(30.0, -20.0, 0.0)
    ));
    assert!(near(
        transform(&app, ghost).translation,
        Vec3::new(-120.0, -120.0, 0.0)
    ));
}

#[test]
fn a_scale_never_reaches_zero_and_flips_past_it() {
    let (mut app, scene) = world_app();
    select(&mut app, Vec2::ZERO, ActiveTool::Scale);
    let right = handle(&mut app, SpriteHandle::Scale(1, 0));

    drag(&mut app, right, Vec2::new(-10.0, 0.0), "");
    let onto_held = transform(&app, scene.tile).scale.x;
    assert!((onto_held - 0.01).abs() < 1e-6, "floored: {onto_held}");
    undo(&mut app);

    drag(&mut app, right, Vec2::new(-30.0, 0.0), "");
    let flipped = transform(&app, scene.tile).scale.x;
    assert!((flipped + 1.0).abs() < 1e-3, "flipped: {flipped}");
    undo(&mut app);

    drag(&mut app, right, Vec2::new(-9.6, 0.0), "ctrl");
    let snapped = transform(&app, scene.tile).scale.x;
    assert!(
        (snapped - 0.01).abs() < 1e-6,
        "a step to zero floors: {snapped}"
    );
}

#[test]
fn ctrl_steps_only_the_axes_a_scale_handle_drives() {
    let (mut app, scene) = world_app();
    set_transform(
        &mut app,
        scene.tile,
        Transform::from_scale(Vec3::new(1.0, 1.37, 1.37)),
    );
    select(&mut app, Vec2::ZERO, ActiveTool::Scale);
    let right = handle(&mut app, SpriteHandle::Scale(1, 0));
    drag(&mut app, right, right + Vec2::new(10.6, 0.0), "ctrl");
    assert!(near(
        transform(&app, scene.tile).scale,
        Vec3::new(1.5, 1.37, 1.37)
    ));

    undo(&mut app);
    set_transform(
        &mut app,
        scene.tile,
        Transform::from_scale(Vec3::new(1.0, 1.0, 1.37)),
    );
    let corner = handle(&mut app, SpriteHandle::Scale(1, 1));
    drag(&mut app, corner, Vec2::new(20.6, 15.4), "ctrl");
    assert!(near(
        transform(&app, scene.tile).scale,
        Vec3::new(1.5, 1.3, 1.37)
    ));
}

#[test]
fn shift_and_ctrl_step_the_uniform_factor_and_keep_proportions() {
    let (mut app, scene) = world_app();
    set_transform(
        &mut app,
        scene.tile,
        Transform::from_scale(Vec3::new(2.0, 1.0, 1.0)),
    );
    select(&mut app, Vec2::ZERO, ActiveTool::Scale);
    let corner = handle(&mut app, SpriteHandle::Scale(1, 1));
    assert!(corner.abs_diff_eq(Vec2::new(20.0, 10.0), 1e-3));
    drag(&mut app, corner, Vec2::new(41.2, 20.6), "shift,ctrl");
    assert!(near(
        transform(&app, scene.tile).scale,
        Vec3::new(3.0, 1.5, 1.0)
    ));
}

#[test]
fn a_rotate_follows_the_pointer_under_a_mirrored_parent() {
    let (mut app, scene) = world_app();
    let mirror = app
        .world_mut()
        .spawn((
            Name::new("Mirror"),
            Transform::from_xyz(-100.0, -80.0, 0.0).with_scale(Vec3::new(-1.0, 1.0, 1.0)),
            ChildOf(scene.root),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), mirror);
    let flip = sprite(
        &mut app,
        mirror,
        "Flip",
        Sprite::from_color(Color::WHITE, Vec2::splat(20.0)),
        Anchor::CENTER,
        Vec3::ZERO,
    );
    select(&mut app, Vec2::new(-100.0, -80.0), ActiveTool::Rotate);
    assert!(world_x_axis(&app, flip).abs_diff_eq(Vec2::NEG_X, 1e-3));
    let pivot = Vec2::new(-100.0, -80.0);
    let knob = handle(&mut app, SpriteHandle::Rotate);
    let reach = (knob - pivot).length();
    drag(&mut app, knob, pivot + Vec2::new(-reach, 0.0), "");
    let axis = world_x_axis(&app, flip);
    assert!(
        axis.abs_diff_eq(Vec2::NEG_Y, 1e-3),
        "a quarter turn counter-clockwise on screen: {axis}"
    );
}

#[test]
fn ctrl_lands_the_rotation_itself_on_an_increment() {
    let (mut app, scene) = world_app();
    set_transform(
        &mut app,
        scene.tile,
        Transform::from_rotation(Quat::from_rotation_z(10f32.to_radians())),
    );
    select(&mut app, Vec2::ZERO, ActiveTool::Rotate);
    for (reach_to, lands_on) in [(91.0_f32, 0.0_f32), (106.0, 15.0), (133.0, 45.0)] {
        let knob = handle(&mut app, SpriteHandle::Rotate);
        let at = Vec2::from_angle(reach_to.to_radians()) * knob.length();
        drag(&mut app, knob, at, "ctrl");
        let angle = angle_of(&transform(&app, scene.tile));
        assert!(
            (angle - lands_on).abs() < 1e-2,
            "from 10 to {} lands on {lands_on}: {angle}",
            reach_to - 90.0
        );
        undo(&mut app);
    }
}

#[test]
fn rotate_and_scale_take_the_whole_selection_about_the_primary() {
    let (mut app, scene) = world_app();
    let third = sprite(
        &mut app,
        scene.root,
        "Third",
        Sprite::from_color(Color::WHITE, Vec2::splat(20.0)),
        Anchor::CENTER,
        Vec3::new(0.0, 100.0, 0.0),
    );
    select(&mut app, Vec2::new(100.0, 0.0), ActiveTool::Translate);
    pointer(&mut app, Vec2::new(0.0, 100.0), "click", "shift");
    pointer(&mut app, Vec2::ZERO, "click", "shift");
    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![scene.other, third, scene.tile]
    );
    app.world_mut().insert_resource(ActiveTool::Rotate);
    settle(&mut app);
    let entries = history_len(&app);

    let knob = handle(&mut app, SpriteHandle::Rotate);
    drag(&mut app, knob, Vec2::new(-knob.length(), 0.0), "");
    assert_eq!(history_len(&app), entries + 1, "one entry for the three");
    for (entity, at) in [
        (scene.tile, Vec3::ZERO),
        (scene.other, Vec3::new(0.0, 100.0, 0.0)),
        (third, Vec3::new(-100.0, 0.0, 0.0)),
    ] {
        let turned = transform(&app, entity);
        assert!(near(turned.translation, at), "{turned:?}");
        assert!((angle_of(&turned) - 90.0).abs() < 1e-2);
    }

    undo(&mut app);
    app.world_mut().insert_resource(ActiveTool::Scale);
    settle(&mut app);
    let corner = handle(&mut app, SpriteHandle::Scale(1, 1));
    drag(&mut app, corner, Vec2::new(20.0, 12.0), "shift");
    assert_eq!(history_len(&app), entries + 1);
    for (entity, at) in [
        (scene.tile, Vec3::new(3.0, 3.0, 0.0)),
        (scene.other, Vec3::new(133.0, 3.0, 0.0)),
        (third, Vec3::new(3.0, 133.0, 0.0)),
    ] {
        let scaled = transform(&app, entity);
        assert!(near(scaled.translation, at), "{scaled:?}");
        assert!(near(scaled.scale, Vec3::new(1.3, 1.3, 1.0)), "{scaled:?}");
    }
}
