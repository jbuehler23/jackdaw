//! Dropping an image out of the asset browser onto the 2D canvas.

use crate::util;

use bevy::{
    camera::{NormalizedRenderTarget, RenderTarget},
    picking::{
        backend::HitData,
        events::{DragDrop, Pointer},
        pointer::{Location, PointerButton, PointerId},
    },
    prelude::*,
    ui::ComputedNode,
    window::{PrimaryWindow, WindowRef},
};
use jackdaw::asset_browser::ActiveAssetDrag;
use jackdaw::commands::CommandHistory;
use jackdaw::viewport_2d::{Viewport2dPanelHost, build_viewport_2d_panel};
use jackdaw_feathers::tokens::TOOLBAR_HEIGHT;
use jackdaw_scene_types::UiSceneRoot;

const REFERENCE: UVec2 = UVec2::new(2400, 1200);
const DROPPED: &str = "textures/dropped.png";

fn settle(app: &mut App) {
    for _ in 0..4 {
        app.update();
    }
}

fn panel(app: &mut App) -> Entity {
    let parent = app
        .world_mut()
        .spawn((
            jackdaw::EditorEntity,
            Node {
                width: px(1200.0 + jackdaw::viewport_2d::RULER_SIZE),
                height: px(600.0 + jackdaw::viewport_2d::RULER_SIZE + TOOLBAR_HEIGHT),
                ..default()
            },
        ))
        .id();
    build_viewport_2d_panel(app.world_mut(), parent);
    let mut host = app
        .world_mut()
        .get_mut::<Viewport2dPanelHost>(parent)
        .expect("host on panel parent");
    host.view.zoom = 0.5;
    host.fit_pending = false;
    parent
}

/// A root filling the canvas, with an image node and a container in it.
fn scene(app: &mut App) -> (Entity, Entity, Entity) {
    let root = app
        .world_mut()
        .spawn((
            Name::new("UiRoot"),
            UiSceneRoot {
                reference_size: REFERENCE,
            },
            Node {
                width: percent(100),
                height: percent(100),
                ..default()
            },
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), root);
    let picture = app
        .world_mut()
        .spawn((
            Name::new("Picture"),
            Node {
                position_type: PositionType::Absolute,
                left: px(100.0),
                top: px(100.0),
                width: px(200.0),
                height: px(200.0),
                ..default()
            },
            ImageNode::default(),
            ChildOf(root),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), picture);
    let container = app
        .world_mut()
        .spawn((
            Name::new("Container"),
            Node {
                position_type: PositionType::Absolute,
                left: px(600.0),
                top: px(100.0),
                width: px(300.0),
                height: px(300.0),
                ..default()
            },
            ChildOf(root),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), container);
    settle(app);
    (root, picture, container)
}

fn screen_position_of(app: &mut App, panel: Entity, authored: Vec2) -> Vec2 {
    let (area, view, target_size) = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .map(|host| (host.area, host.view, host.target_size))
        .expect("host on panel parent");
    let computed = *app
        .world()
        .get::<ComputedNode>(area)
        .expect("the stage area is laid out");
    let centre = app
        .world()
        .get::<bevy::ui::UiGlobalTransform>(area)
        .expect("the stage area is laid out")
        .translation;
    let focus = target_size.as_vec2() / 2.0 + Vec2::new(view.pan.x, -view.pan.y);
    let area_centre_logical = centre * computed.inverse_scale_factor();
    let logical = area_centre_logical + (authored - focus) * view.zoom;
    logical * app.world().resource::<UiScale>().0
}

/// Drag an image out of the browser and drop it at an authored point.
fn drop_image_at(app: &mut App, panel: Entity, authored: Vec2) {
    drop_path_at(app, panel, authored, DROPPED.into());
}

/// `drop_image_at` for a drag carrying a particular path, which is how the
/// asset browser hands one over: the file's own, and absolute.
fn drop_path_at(app: &mut App, panel: Entity, authored: Vec2, path: std::path::PathBuf) {
    app.world_mut().resource_mut::<ActiveAssetDrag>().image = Some(path);
    let (stage, camera) = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .map(|host| (host.stage, host.camera))
        .expect("host on panel parent");
    let position = screen_position_of(app, panel, authored);
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(app.world())
        .expect("headless apps still have a primary window");
    let render_target: NormalizedRenderTarget = RenderTarget::Window(WindowRef::Primary)
        .normalize(Some(window))
        .expect("the primary window normalizes");
    let dropped = app.world_mut().spawn_empty().id();
    app.world_mut().trigger(Pointer::new(
        PointerId::Mouse,
        Location {
            target: render_target,
            position,
        },
        DragDrop {
            button: PointerButton::Primary,
            dropped,
            hit: HitData::new(camera, 0.0, None, None),
        },
        stage,
    ));
    settle(app);
}

/// The asset path the node's texture was loaded from.
fn texture_path(app: &App, entity: Entity) -> Option<String> {
    let image = app.world().get::<ImageNode>(entity)?;
    Some(
        image
            .image
            .path()?
            .path()
            .to_string_lossy()
            .replace('\\', "/"),
    )
}

fn undo_depth(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

fn undo(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    settle(app);
}

fn redo(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.redo(world));
    settle(app);
}

/// The scene as a save would write it.
fn saved(app: &mut App) -> String {
    jackdaw::scene_io::emit_bsn_scene_with_inline_assets(app.world_mut(), std::path::Path::new("."))
}

/// Where the node is placed, as the saved document spells it.
fn placement(app: &App, entity: Entity) -> (PositionType, Val, Val) {
    let node = app.world().get::<Node>(entity).expect("a node");
    (node.position_type, node.left, node.top)
}

/// The image nodes under `parent`, which is what a drop adds to.
fn image_children(app: &mut App, parent: Entity) -> Vec<Entity> {
    let world = app.world();
    world
        .get::<Children>(parent)
        .into_iter()
        .flatten()
        .copied()
        .filter(|&child| world.get::<ImageNode>(child).is_some())
        .collect()
}

#[test]
fn a_drop_on_an_image_node_sets_its_texture() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let (root, picture, _container) = scene(&mut app);
    let depth = undo_depth(&app);

    drop_image_at(&mut app, panel, Vec2::new(200.0, 200.0));

    assert_eq!(
        texture_path(&app, picture).as_deref(),
        Some(DROPPED),
        "the node that was already there took the texture",
    );
    assert_eq!(
        image_children(&mut app, root),
        vec![picture],
        "and no second image was made",
    );
    assert_eq!(undo_depth(&app) - depth, 1, "one drop is one entry");

    undo(&mut app);
    assert_ne!(
        texture_path(&app, picture).as_deref(),
        Some(DROPPED),
        "undo puts the texture back",
    );
}

#[test]
fn a_drop_on_a_container_puts_an_image_in_it() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let (_root, _picture, container) = scene(&mut app);
    let depth = undo_depth(&app);

    drop_image_at(&mut app, panel, Vec2::new(700.0, 200.0));

    let made = image_children(&mut app, container);
    assert_eq!(made.len(), 1, "the container took one image");
    assert_eq!(
        texture_path(&app, made[0]).as_deref(),
        Some(DROPPED),
        "carrying the texture that was dropped",
    );
    assert_eq!(
        undo_depth(&app) - depth,
        1,
        "the node and its texture are one entry, not two",
    );

    undo(&mut app);
    assert!(
        image_children(&mut app, container).is_empty(),
        "one undo takes the whole drop back",
    );
}

#[test]
fn a_drop_on_bare_canvas_places_an_image_where_it_landed() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let (root, picture, container) = scene(&mut app);
    let depth = undo_depth(&app);

    drop_image_at(&mut app, panel, Vec2::new(1400.0, 700.0));

    let made: Vec<Entity> = image_children(&mut app, root)
        .into_iter()
        .filter(|&entity| entity != picture)
        .collect();
    assert_eq!(made.len(), 1, "the canvas took one image");
    let node = app.world().get::<Node>(made[0]).expect("a node");
    assert_eq!(
        (node.position_type, node.left, node.top),
        (PositionType::Absolute, px(1400.0), px(700.0)),
        "placed where the drop landed",
    );
    assert_eq!(
        texture_path(&app, made[0]).as_deref(),
        Some(DROPPED),
        "carrying the texture that was dropped",
    );
    assert_eq!(undo_depth(&app) - depth, 1, "one drop is one entry");
    let _ = container;

    undo(&mut app);
    assert_eq!(
        image_children(&mut app, root),
        vec![picture],
        "one undo takes the whole drop back",
    );
}

/// The browser carries the file's own absolute path, and an absolute path is
/// not an approved asset path, so the document has to end up with the
/// project-relative one.
#[test]
fn a_drop_records_the_project_relative_path_of_the_file_it_carried() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let (root, picture, _container) = scene(&mut app);
    let absolute = jackdaw::entity_ops::get_assets_base_dir()
        .expect("the editor resolves an assets directory")
        .join(DROPPED);

    drop_path_at(&mut app, panel, Vec2::new(1400.0, 700.0), absolute);

    let made: Vec<Entity> = image_children(&mut app, root)
        .into_iter()
        .filter(|&entity| entity != picture)
        .collect();
    assert_eq!(made.len(), 1, "the canvas took one image");
    assert_eq!(
        texture_path(&app, made[0]).as_deref(),
        Some(DROPPED),
        "the absolute path was reduced to the one the project reads",
    );

    let text = saved(&mut app);
    assert!(
        text.contains(DROPPED),
        "the saved document carries the texture:\n{text}",
    );
}

/// The placement reached the node but not the document, so a save wrote the
/// palette's default position.
#[test]
fn a_canvas_drop_saves_where_it_landed() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let _scene = scene(&mut app);

    drop_image_at(&mut app, panel, Vec2::new(1400.0, 700.0));

    let text = saved(&mut app);
    assert!(
        text.contains("left: bevy_ui::geometry::Val::Px(1400.0)")
            && text.contains("top: bevy_ui::geometry::Val::Px(700.0)"),
        "the saved document places the image where it was dropped:\n{text}",
    );
}

/// Written outside the entry, a redo replayed the palette's spawn and left the
/// image at the palette's default position.
#[test]
fn redo_puts_a_dropped_image_back_where_it_landed() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let (root, picture, _container) = scene(&mut app);

    drop_image_at(&mut app, panel, Vec2::new(1400.0, 700.0));
    undo(&mut app);
    assert_eq!(
        image_children(&mut app, root),
        vec![picture],
        "undo took the whole drop back",
    );

    redo(&mut app);
    let made: Vec<Entity> = image_children(&mut app, root)
        .into_iter()
        .filter(|&entity| entity != picture)
        .collect();
    assert_eq!(made.len(), 1, "redo put the image back");
    assert_eq!(
        placement(&app, made[0]),
        (PositionType::Absolute, px(1400.0), px(700.0)),
        "and back where it was dropped, not where the palette makes one",
    );
    assert_eq!(
        texture_path(&app, made[0]).as_deref(),
        Some(DROPPED),
        "carrying its texture",
    );
}
