//! Canvas snapping on a 2D world scene: a sprite drag landing on the grid, on
//! another sprite's edge and on a whole pixel, the Ctrl modifier, the snap
//! highlight, and the Snap menu offering the world scene's kinds.

use bevy::{prelude::*, sprite::Anchor};
use jackdaw::{
    active_tool::ActiveTool,
    sprite_gizmo::{SpriteGizmoHandle, SpriteHandle},
    ui_stage::{CanvasAxis, SnapHighlight},
};
use jackdaw_api::prelude::*;

use crate::sprite_gizmo::{
    drag, history_len, near, pointer, run, select, settle, sprite, transform, undo, world_app,
};
use crate::util::OperatorResultExt as _;

pub(crate) fn set_kind(app: &mut App, kind: &str, on: bool) {
    app.world_mut()
        .operator("canvas.snap")
        .param("kind", kind.to_string())
        .param("on", on)
        .call()
        .expect("canvas.snap dispatches")
        .assert_finished();
}

/// Snapping on with only the kinds named.
pub(crate) fn snap_only(app: &mut App, kinds: &[&str]) {
    set_kind(app, "enabled", true);
    for kind in ["pixel", "grid", "sprite_edges", "guides"] {
        set_kind(app, kind, kinds.contains(&kind));
    }
}

pub(crate) fn highlights(app: &mut App) -> Vec<CanvasAxis> {
    let mut axes: Vec<CanvasAxis> = app
        .world_mut()
        .query::<&SnapHighlight>()
        .iter(app.world())
        .map(|highlight| highlight.axis)
        .collect();
    axes.sort_by_key(|axis| *axis == CanvasAxis::Horizontal);
    axes
}

#[test]
fn a_drag_lands_the_sprite_on_a_grid_line() {
    let (mut app, scene) = world_app();
    snap_only(&mut app, &["grid", "pixel"]);
    select(&mut app, Vec2::ZERO, ActiveTool::Translate);

    drag(&mut app, Vec2::new(2.0, 3.0), Vec2::new(15.0, 8.0), "");
    assert!(
        near(
            transform(&app, scene.tile).translation,
            Vec3::new(10.0, 2.0, 0.0)
        ),
        "the lower-left corner rests on the 8-unit lattice: {:?}",
        transform(&app, scene.tile).translation
    );
}

#[test]
fn a_drag_lands_on_a_neighbours_edge_and_shows_where() {
    let (mut app, scene) = world_app();
    snap_only(&mut app, &["sprite_edges", "grid"]);
    select(&mut app, Vec2::ZERO, ActiveTool::Translate);
    let entries = history_len(&app);

    pointer(&mut app, Vec2::new(2.0, 3.0), "press", "");
    pointer(&mut app, Vec2::new(80.5, 4.0), "move", "");
    assert!(
        near(
            transform(&app, scene.tile).translation,
            Vec3::new(80.0, 0.0, 0.0)
        ),
        "the right edge meets the neighbour's left edge and the edges line up: {:?}",
        transform(&app, scene.tile).translation
    );
    assert_eq!(
        highlights(&mut app),
        vec![CanvasAxis::Vertical, CanvasAxis::Horizontal],
        "a line is drawn on each axis that landed",
    );
    pointer(&mut app, Vec2::new(80.5, 4.0), "release", "");
    assert!(highlights(&mut app).is_empty(), "the lines go on release");
    assert_eq!(history_len(&app), entries + 1);

    undo(&mut app);
    assert_eq!(transform(&app, scene.tile), Transform::IDENTITY);
}

#[test]
fn a_drag_lands_the_anchor_on_a_whole_pixel() {
    let (mut app, scene) = world_app();
    snap_only(&mut app, &["pixel"]);
    select(&mut app, Vec2::ZERO, ActiveTool::Translate);

    drag(&mut app, Vec2::new(2.0, 3.0), Vec2::new(14.25, -1.75), "");
    assert!(
        near(
            transform(&app, scene.tile).translation,
            Vec3::new(12.0, -5.0, 0.0)
        ),
        "{:?}",
        transform(&app, scene.tile).translation
    );
}

#[test]
fn with_snapping_off_nothing_snaps() {
    let (mut app, scene) = world_app();
    snap_only(&mut app, &["pixel", "grid", "sprite_edges"]);
    set_kind(&mut app, "enabled", false);
    select(&mut app, Vec2::ZERO, ActiveTool::Translate);

    pointer(&mut app, Vec2::new(2.0, 3.0), "press", "");
    pointer(&mut app, Vec2::new(80.5, 3.5), "move", "");
    assert!(highlights(&mut app).is_empty());
    pointer(&mut app, Vec2::new(80.5, 3.5), "release", "");
    assert!(near(
        transform(&app, scene.tile).translation,
        Vec3::new(78.5, 0.5, 0.0)
    ));
}

#[test]
fn ctrl_suspends_snapping_for_a_move() {
    let (mut app, scene) = world_app();
    snap_only(&mut app, &["pixel", "grid", "sprite_edges"]);
    select(&mut app, Vec2::ZERO, ActiveTool::Translate);

    drag(&mut app, Vec2::new(2.0, 3.0), Vec2::new(80.5, 3.5), "ctrl");
    assert!(near(
        transform(&app, scene.tile).translation,
        Vec3::new(78.5, 0.5, 0.0)
    ));
}

#[test]
fn an_axis_handle_snaps_only_its_own_axis() {
    let (mut app, scene) = world_app();
    snap_only(&mut app, &["sprite_edges"]);
    select(&mut app, Vec2::ZERO, ActiveTool::Translate);

    let x = handle(&mut app, SpriteHandle::AxisX);
    drag(&mut app, x, x + Vec2::new(78.5, 0.5), "");
    assert!(near(
        transform(&app, scene.tile).translation,
        Vec3::new(80.0, 0.0, 0.0)
    ));
}

#[test]
fn a_scale_handle_lands_its_edge_on_a_neighbour() {
    let (mut app, scene) = world_app();
    snap_only(&mut app, &["sprite_edges"]);
    select(&mut app, Vec2::ZERO, ActiveTool::Scale);

    let right = handle(&mut app, SpriteHandle::Scale(1, 0));
    drag(&mut app, right, Vec2::new(88.5, 2.0), "");
    let pose = transform(&app, scene.tile);
    assert!(
        (pose.scale.x - 5.0).abs() < 1e-3 && near(pose.translation, Vec3::new(40.0, 0.0, 0.0)),
        "the right edge lands on x = 90 with the left edge held: {pose:?}"
    );
}

#[test]
fn the_snap_menu_offers_the_world_scenes_kinds() {
    let (mut app, _scene) = world_app();
    run(&mut app, "menu.open name=Snap");
    settle(&mut app);
    let rows: Vec<(String, String)> = app
        .world_mut()
        .query::<&jackdaw_widgets::menu_bar::MenuBarItem>()
        .iter(app.world())
        .find(|item| item.label == "Snap")
        .expect("the 2D viewport header carries a Snap menu")
        .actions
        .clone();
    let kinds: Vec<&str> = rows
        .iter()
        .filter_map(|(action, _)| action.split("kind=").nth(1))
        .collect();
    assert_eq!(
        kinds,
        vec!["enabled", "pixel", "guides", "grid", "sprite_edges"]
    );
    assert!(rows.contains(&(
        "[x]op:canvas.snap?kind=sprite_edges".to_string(),
        "Sprite Edges".to_string()
    )));
}

pub(crate) fn handle(app: &mut App, kind: SpriteHandle) -> Vec2 {
    app.world_mut()
        .query::<&SpriteGizmoHandle>()
        .iter(app.world())
        .find(|handle| handle.kind == kind)
        .map(|handle| handle.world)
        .unwrap_or_else(|| panic!("the gizmo shows a {kind:?} handle"))
}

fn whole(value: f32) -> bool {
    (value - value.round()).abs() < 1e-3
}

#[test]
fn an_odd_sized_sprite_keeps_its_edges_on_whole_pixels() {
    let (mut app, scene) = world_app();
    let odd = sprite(
        &mut app,
        scene.root,
        "Odd",
        Sprite::from_color(Color::WHITE, Vec2::splat(15.0)),
        Anchor::CENTER,
        Vec3::new(200.0, 80.0, 0.0),
    );
    settle(&mut app);
    snap_only(&mut app, &["grid", "pixel"]);
    select(&mut app, Vec2::new(200.0, 80.0), ActiveTool::Translate);

    drag(&mut app, Vec2::new(200.0, 80.0), Vec2::new(213.3, 85.2), "");
    let at = transform(&app, odd).translation;
    assert!(
        near(at, Vec3::new(215.5, 87.5, 0.0)),
        "the lower-left corner rests on the grid at (208, 80): {at:?}"
    );

    snap_only(&mut app, &["pixel"]);
    drag(&mut app, at.xy(), at.xy() + Vec2::new(3.3, -2.2), "");
    let at = transform(&app, odd).translation;
    assert!(
        whole(at.x - 7.5) && whole(at.y - 7.5),
        "pixel snap keeps the edges whole, not the anchor: {at:?}"
    );
    assert!(near(at, Vec3::new(218.5, 85.5, 0.0)), "{at:?}");
}

#[test]
fn a_scale_whose_edge_cannot_reach_the_line_does_not_land() {
    let (mut app, scene) = world_app();
    sprite(
        &mut app,
        scene.root,
        "Wall",
        Sprite::from_color(Color::WHITE, Vec2::splat(20.0)),
        Anchor::CENTER,
        Vec3::new(-20.0, 200.0, 0.0),
    );
    settle(&mut app);
    snap_only(&mut app, &["sprite_edges"]);
    select(&mut app, Vec2::ZERO, ActiveTool::Scale);

    let right = handle(&mut app, SpriteHandle::Scale(1, 0));
    pointer(&mut app, right, "press", "");
    pointer(&mut app, Vec2::new(-9.0, 0.0), "move", "");
    assert!(
        highlights(&mut app).is_empty(),
        "the line at x = -10 would need a zero scale, so nothing lands"
    );
    pointer(&mut app, Vec2::new(-9.0, 0.0), "release", "");
    let scale = transform(&app, scene.tile).scale.x;
    assert!(
        (scale - 0.05).abs() < 0.02,
        "the drag follows the pointer instead: {scale}"
    );
}

pub(crate) fn ghost(app: &mut App, parent: Entity, at: Vec3) -> Entity {
    let missing = app
        .world()
        .resource::<AssetServer>()
        .load::<Image>("no/such/image.png");
    sprite(
        app,
        parent,
        "Ghost",
        Sprite {
            image: missing,
            ..default()
        },
        Anchor::CENTER,
        at,
    )
}

#[test]
fn the_grid_still_snaps_a_sprite_of_unknown_size() {
    let (mut app, scene) = world_app();
    let ghost = ghost(&mut app, scene.root, Vec3::new(-150.3, -100.6, 0.0));
    snap_only(&mut app, &["grid", "pixel", "sprite_edges"]);
    app.world_mut().insert_resource(ActiveTool::Translate);
    jackdaw::selection::select_only(app.world_mut(), ghost);
    settle(&mut app);

    let x = handle(&mut app, SpriteHandle::AxisX);
    drag(&mut app, x, x + Vec2::new(13.0, 0.0), "");
    assert!(
        near(
            transform(&app, ghost).translation,
            Vec3::new(-136.0, -100.6, 0.0)
        ),
        "the anchor lands on the grid: {:?}",
        transform(&app, ghost).translation
    );
}

#[test]
fn the_snap_reference_is_the_primary_even_when_its_parent_carries_it() {
    let (mut app, scene) = world_app();
    let parent = ghost(&mut app, scene.root, Vec3::new(-150.0, -100.0, 0.0));
    let child = ghost(&mut app, parent, Vec3::new(0.3, 0.6, 0.0));
    settle(&mut app);
    snap_only(&mut app, &["pixel"]);
    app.world_mut().insert_resource(ActiveTool::Translate);
    jackdaw::selection::select_many(app.world_mut(), &[parent, child]);
    settle(&mut app);

    let x = handle(&mut app, SpriteHandle::AxisX);
    drag(&mut app, x, x + Vec2::new(10.0, 0.0), "");
    let parent_at = transform(&app, parent).translation;
    assert!(
        near(parent_at, Vec3::new(-140.3, -100.0, 0.0)),
        "the child, not the parent, lands on a whole pixel: {parent_at:?}"
    );
}

/// A sprite turned 30 degrees under a parent scaled (2, 1) and turned so the
/// sprite's x axis runs along the world's x while its y axis is sheared. Its
/// right scale handle dragged level to x = 91, beside the neighbour's edge at
/// 90: the resulting scale, and whether a snap line showed.
fn sheared_scale(snap: bool) -> (f32, bool) {
    let (mut app, scene) = world_app();
    let turn = -(0.5_f32).atan2(3.0_f32.sqrt());
    let parent = app
        .world_mut()
        .spawn((
            Name::new("Shear"),
            Transform::from_xyz(50.0, -150.0, 0.0)
                .with_rotation(Quat::from_rotation_z(turn))
                .with_scale(Vec3::new(2.0, 1.0, 1.0)),
            ChildOf(scene.root),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), parent);
    let kid = sprite(
        &mut app,
        parent,
        "Kid",
        Sprite::from_color(Color::WHITE, Vec2::splat(20.0)),
        Anchor::CENTER,
        Vec3::ZERO,
    );
    app.world_mut()
        .get_mut::<Transform>(kid)
        .expect("the kid has a transform")
        .rotation = Quat::from_rotation_z(30_f32.to_radians());
    settle(&mut app);
    snap_only(&mut app, &["sprite_edges"]);
    if !snap {
        set_kind(&mut app, "enabled", false);
    }
    app.world_mut().insert_resource(ActiveTool::Scale);
    jackdaw::selection::select_only(app.world_mut(), kid);
    settle(&mut app);

    let right = handle(&mut app, SpriteHandle::Scale(1, 0));
    pointer(&mut app, right, "press", "");
    pointer(&mut app, Vec2::new(91.0, right.y), "move", "");
    let shown = !highlights(&mut app).is_empty();
    pointer(&mut app, Vec2::new(91.0, right.y), "release", "");
    (transform(&app, kid).scale.x, shown)
}

#[test]
fn a_sheared_sprite_scales_without_the_canvas_snap() {
    let (free, _) = sheared_scale(false);
    let (snapped, shown) = sheared_scale(true);
    assert!(!shown, "no snap line on a sheared sprite");
    assert!(
        (free - snapped).abs() < 1e-4,
        "the scale follows the pointer: {free} vs {snapped}"
    );
}
