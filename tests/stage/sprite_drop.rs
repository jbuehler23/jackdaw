//! Dropping an image out of the Project window onto a 2D world scene: one
//! sprite under the scene root at the drop point, the same entity
//! `sprite.add` makes.

use crate::sprite_gizmo::{history_len, pointer, settle, undo, world_app};
use crate::util::OperatorResultExt as _;

use bevy::{
    camera::{NormalizedRenderTarget, RenderTarget},
    picking::{
        backend::HitData,
        events::{Pointer, PointerDragDrop},
        pointer::{Location, PointerButton, PointerId},
    },
    prelude::*,
    sprite::Anchor,
    ui::{ComputedNode, UiGlobalTransform},
    window::{PrimaryWindow, WindowRef},
};
use jackdaw::{asset_drag::ActiveAssetDrag, viewport_2d::Viewport2dPanelHost};
use jackdaw_api::prelude::*;
use jackdaw_bsn::SceneBsnAst;
use path_slash::PathExt as _;

const IMAGE: &str = "textures/hero.png";

fn absolute(relative: &str) -> std::path::PathBuf {
    jackdaw::entity_ops::get_assets_base_dir()
        .expect("the editor resolves an assets directory")
        .join(relative)
}

/// Where `world_point` is drawn on `panel`'s stage, in physical window pixels.
fn screen_position_of(app: &App, panel: Entity, world_point: Vec2) -> Vec2 {
    let host = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent");
    let authored =
        jackdaw::viewport_2d::world_to_target_pixel(host.view, host.target_size, world_point);
    let computed = *app
        .world()
        .get::<ComputedNode>(host.stage)
        .expect("the stage is laid out");
    let centre = app
        .world()
        .get::<UiGlobalTransform>(host.stage)
        .expect("the stage is laid out")
        .translation;
    let target_scale =
        jackdaw::viewport_2d::target_pixels_per_stage_pixel(computed.size(), host.target_size);
    let offset = authored - host.target_size.as_vec2() / 2.0;
    let logical = (offset / target_scale + centre) * computed.inverse_scale_factor();
    logical * app.world().resource::<UiScale>().0
}

fn drop_at(app: &mut App, panel: Entity, world_point: Vec2, path: std::path::PathBuf) {
    app.world_mut().resource_mut::<ActiveAssetDrag>().image = Some(path);
    let (stage, camera) = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .map(|host| (host.stage, host.camera))
        .expect("host on panel parent");
    let position = screen_position_of(app, panel, world_point);
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(app.world())
        .expect("headless apps still have a primary window");
    let target: NormalizedRenderTarget = RenderTarget::Window(WindowRef::Primary)
        .normalize(Some(window))
        .expect("the primary window normalizes");
    let dropped = app.world_mut().spawn_empty().id();
    app.world_mut().trigger(PointerDragDrop {
        entity: stage,
        pointer: Pointer::new(PointerId::Mouse, Location { target, position }),
        button: PointerButton::Primary,
        dropped,
        hit: HitData::new(camera, 0.0, None, None),
    });
    settle(app);
}

fn sprites(app: &mut App) -> Vec<Entity> {
    let mut query = app
        .world_mut()
        .query_filtered::<Entity, (With<Sprite>, Without<jackdaw::EditorEntity>)>();
    query.iter(app.world()).collect()
}

fn new_sprite(app: &mut App, before: &[Entity]) -> Entity {
    let made: Vec<Entity> = sprites(app)
        .into_iter()
        .filter(|entity| !before.contains(entity))
        .collect();
    assert_eq!(made.len(), 1, "one sprite was added: {made:?}");
    made[0]
}

fn image_path(app: &App, entity: Entity) -> Option<String> {
    let sprite = app.world().get::<Sprite>(entity)?;
    Some(sprite.image.path()?.path().to_slash_lossy().into_owned())
}

fn authored_types(app: &App, entity: Entity) -> Vec<String> {
    let ast = app.world().resource::<SceneBsnAst>();
    let node = ast.ast_for(entity).expect("the sprite is in the document");
    let mut types = ast.component_type_paths(node);
    types.sort();
    types
}

#[test]
fn a_drop_on_a_world_scene_adds_one_sprite_under_the_root_at_the_point() {
    let (mut app, scene) = world_app();
    let before = sprites(&mut app);
    let depth = history_len(&app);
    let at = Vec2::new(-60.0, 40.0);

    drop_at(&mut app, scene.panel, at, absolute(IMAGE));

    let sprite = new_sprite(&mut app, &before);
    assert_eq!(
        app.world().get::<ChildOf>(sprite).map(ChildOf::parent),
        Some(scene.root)
    );
    let translation = app
        .world()
        .get::<Transform>(sprite)
        .expect("a transform")
        .translation;
    assert!(
        (translation.truncate() - at).length() < 0.5,
        "centred on the drop point: {translation}"
    );
    assert_eq!(translation.z, 1.0, "above the scene's sprites, all at 0");
    assert_eq!(image_path(&app, sprite).as_deref(), Some(IMAGE));
    assert_eq!(history_len(&app) - depth, 1, "one drop is one entry");

    undo(&mut app);
    assert_eq!(sprites(&mut app).len(), before.len(), "undo takes it back");
}

#[test]
fn a_drop_and_sprite_add_make_the_same_entity() {
    let (mut app, scene) = world_app();
    let before = sprites(&mut app);
    drop_at(
        &mut app,
        scene.panel,
        Vec2::new(30.0, -30.0),
        absolute(IMAGE),
    );
    let dropped = new_sprite(&mut app, &before);
    jackdaw::selection::clear_selection_in_world(app.world_mut());

    let before = sprites(&mut app);
    app.world_mut()
        .operator("sprite.add")
        .param("path", absolute(IMAGE).to_slash_lossy().into_owned())
        .param("x", 5.0_f64)
        .param("y", 5.0_f64)
        .call()
        .expect("dispatch")
        .assert_finished();
    settle(&mut app);
    let added = new_sprite(&mut app, &before);

    assert_eq!(authored_types(&app, dropped), authored_types(&app, added));
    assert_eq!(image_path(&app, dropped), image_path(&app, added));
    let sprite = |entity| {
        let sprite = app.world().get::<Sprite>(entity).expect("a sprite");
        (
            sprite.custom_size,
            sprite.flip_x,
            sprite.flip_y,
            sprite.color,
        )
    };
    assert_eq!(sprite(dropped), sprite(added));
    assert_eq!(
        app.world().get::<Anchor>(dropped),
        app.world().get::<Anchor>(added)
    );
    assert_eq!(
        app.world().get::<ChildOf>(dropped),
        app.world().get::<ChildOf>(added)
    );
}

#[test]
fn a_drop_with_the_grid_snap_on_lands_on_the_grid() {
    let (mut app, scene) = world_app();
    app.world_mut()
        .insert_resource(jackdaw::canvas_snap::CanvasSnap {
            enabled: true,
            grid: true,
            pixel: false,
            sprite_edges: false,
            guides: false,
            ..default()
        });
    let before = sprites(&mut app);
    drop_at(
        &mut app,
        scene.panel,
        Vec2::new(13.0, 21.0),
        absolute(IMAGE),
    );
    let sprite = new_sprite(&mut app, &before);
    let translation = app
        .world()
        .get::<Transform>(sprite)
        .expect("a transform")
        .translation;
    assert_eq!(translation.truncate(), Vec2::new(16.0, 24.0));
}

#[test]
fn a_drop_the_sprite_add_gate_refuses_leaves_the_drag_alone() {
    let (mut app, scene) = world_app();
    app.world_mut()
        .spawn(jackdaw_feathers::text_edit::text_edit(
            jackdaw_feathers::text_edit::TextEditProps::default().auto_focus(),
        ));
    settle(&mut app);
    assert!(
        app.world()
            .resource::<bevy::input_focus::InputFocus>()
            .get()
            .is_some(),
        "a field holds the keyboard"
    );
    let before = sprites(&mut app);

    drop_at(&mut app, scene.panel, Vec2::ZERO, absolute(IMAGE));

    assert_eq!(sprites(&mut app), before, "no sprite");
    assert!(
        app.world().resource::<ActiveAssetDrag>().image.is_some(),
        "the drag is still there for whoever can take it"
    );
}

#[test]
fn a_drop_off_the_stage_adds_nothing() {
    let (mut app, scene) = world_app();
    let before = sprites(&mut app);
    let depth = history_len(&app);
    drop_at(
        &mut app,
        scene.panel,
        Vec2::new(5000.0, 0.0),
        absolute(IMAGE),
    );
    assert_eq!(sprites(&mut app), before, "no sprite");
    assert_eq!(history_len(&app), depth);
}

#[test]
fn sprite_add_with_no_position_lands_at_the_centre_of_the_view() {
    let (mut app, scene) = world_app();
    let pan = app
        .world()
        .get::<Viewport2dPanelHost>(scene.panel)
        .expect("host on panel parent")
        .view
        .pan;
    pointer(&mut app, pan + Vec2::new(40.0, 40.0), "move", "");

    let before = sprites(&mut app);
    app.world_mut()
        .operator("sprite.add")
        .call()
        .expect("dispatch")
        .assert_finished();
    settle(&mut app);
    let centred = new_sprite(&mut app, &before);
    let at = |app: &App, entity| {
        app.world()
            .get::<Transform>(entity)
            .expect("a transform")
            .translation
            .truncate()
    };
    assert_eq!(at(&app, centred), pan);

    let before = sprites(&mut app);
    app.world_mut()
        .operator("sprite.add")
        .param("x", -7.0_f64)
        .call()
        .expect("dispatch")
        .assert_finished();
    settle(&mut app);
    let placed = new_sprite(&mut app, &before);
    assert_eq!(
        at(&app, placed),
        Vec2::new(-7.0, pan.y),
        "an explicit x wins, the missing y is the view's"
    );
}
