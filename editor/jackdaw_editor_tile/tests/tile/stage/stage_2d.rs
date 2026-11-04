//! A 2D panel framed at a fixed view, and the synthetic input clauses that
//! drive it.

use bevy::prelude::*;
use bevy::sprite::Anchor;
use bevy::window::{PrimaryWindow, WindowResolution};
use jackdaw_editor_core::canvas_snap::CanvasSnap;
use jackdaw_editor_core::test_input::SyntheticInput;
use jackdaw_editor_core::viewport_2d::{
    RULER_SIZE, Ui2dView, Viewport2dPanelHost, build_viewport_2d_panel,
};

use crate::util::OperatorResultExt as _;

const VIEW: Ui2dView = Ui2dView {
    pan: Vec2::new(100.0, -20.0),
    zoom: 2.0,
    grid: 8.0,
};

pub(crate) struct Scene {
    pub(crate) panel: Entity,
}

/// A 2D world scene of three sprites in a panel 1200 logical pixels wide.
pub(crate) fn world_app() -> (App, Scene) {
    world_app_sized(1200.0)
}

/// [`world_app`] with a stage area `area_width` logical pixels wide.
pub(crate) fn world_app_sized(area_width: f32) -> (App, Scene) {
    let mut app = crate::util::editor_test_app();
    crate::util::fixed_frame_clock(&mut app);
    {
        let mut windows = app
            .world_mut()
            .query_filtered::<&mut Window, With<PrimaryWindow>>();
        let mut window = windows
            .single_mut(app.world_mut())
            .expect("headless apps still have a primary window");
        window.resolution = WindowResolution::new(1600, 1000);
    }
    let panel = add_panel(&mut app, area_width);
    let root = jackdaw_editor_core::entity_ops::seed_2d_scene_root(app.world_mut());
    let square = || Sprite::from_color(Color::WHITE, Vec2::splat(20.0));
    sprite(&mut app, root, "Tile", square(), Anchor::CENTER, Vec3::ZERO);
    sprite(
        &mut app,
        root,
        "Other",
        square(),
        Anchor::CENTER,
        Vec3::new(100.0, 0.0, 0.0),
    );
    sprite(
        &mut app,
        root,
        "Corner",
        Sprite::from_color(Color::WHITE, Vec2::new(40.0, 20.0)),
        Anchor::BOTTOM_LEFT,
        Vec3::new(-100.0, 50.0, 0.0),
    );
    jackdaw_editor_core::selection::clear_selection_in_world(app.world_mut());
    app.world_mut().insert_resource(CanvasSnap {
        enabled: false,
        ..default()
    });
    settle(&mut app);
    (app, Scene { panel })
}

/// A 2D panel with a stage area `area_width` by 600 logical pixels, framed
/// at the test view.
pub(crate) fn add_panel(app: &mut App, area_width: f32) -> Entity {
    let panel = app
        .world_mut()
        .spawn((
            jackdaw_editor_core::EditorEntity,
            Node {
                width: px(area_width + RULER_SIZE),
                height: px(600.0 + RULER_SIZE + jackdaw_feathers::tokens::TOOLBAR_HEIGHT),
                ..default()
            },
        ))
        .id();
    build_viewport_2d_panel(app.world_mut(), panel);
    let mut host = app
        .world_mut()
        .get_mut::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent");
    host.set_view(VIEW);
    host.fit_pending = false;
    panel
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
    jackdaw_editor_core::scene_io::register_entity_in_ast(app.world_mut(), entity);
    entity
}

fn settle(app: &mut App) {
    for _ in 0..8 {
        app.update();
    }
}

pub(crate) fn run(app: &mut App, clause: &str) {
    jackdaw_editor_core::boot_ops::run_op_clause(app.world_mut(), clause)
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

pub(crate) fn transform(app: &App, entity: Entity) -> Transform {
    *app.world()
        .get::<Transform>(entity)
        .expect("the sprite has a transform")
}
