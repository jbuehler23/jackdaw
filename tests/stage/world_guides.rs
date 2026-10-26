//! Guides on a 2D world scene: kept on the scene's root in world units (y up),
//! drawn and marked through the panel's view, pulled off and dropped back on
//! the rulers, and offered to a sprite drag as lines to land on.

use bevy::{
    prelude::*,
    ui::{ComputedNode, UiGlobalTransform, UiTargetCamera},
};
use jackdaw::{
    active_tool::ActiveTool,
    sprite_gizmo::SpriteHandle,
    ui_stage::{CanvasAxis, GuideLine},
    viewport_2d::{
        CanvasRuler, RulerGuideMark, Ui2dView, Viewport2dPanelHost, world_to_target_pixel,
    },
};
use jackdaw_api::prelude::*;
use jackdaw_scene_types::{CanvasGuides, Scene2dRoot};

use crate::sprite_gizmo::{
    Scene, add_panel, drag, history_len, near, pointer, run, select, settle, transform, undo,
    world_app, world_app_sized,
};
use crate::sprite_snap::{ghost, handle, highlights, snap_only};
use crate::util::{self, OperatorResultExt as _};

/// World y inside the top ruler's gutter under the test view: the area's top
/// edge is at y = 130 and the ruler is 9 world units tall at zoom 2.
const ON_TOP_RULER: f32 = 135.0;

fn call_guide_op(app: &mut App, id: &'static str, axis: &str, position: f64) {
    app.world_mut()
        .operator(id)
        .param("axis", axis.to_string())
        .param("position", position)
        .call()
        .unwrap_or_else(|err| panic!("{id} dispatches: {err}"))
        .assert_finished();
    settle(app);
}

fn guides(app: &App, root: Entity) -> Option<CanvasGuides> {
    app.world().get::<CanvasGuides>(root).cloned()
}

fn vertical(app: &App, root: Entity) -> Vec<f32> {
    guides(app, root).map(|g| g.vertical).unwrap_or_default()
}

fn host(app: &App, panel: Entity) -> (Ui2dView, UVec2) {
    let host = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("the panel has a host");
    (host.view, host.target_size)
}

/// Where the line of `axis` is drawn over the panel, from its hit slab.
fn drawn_line(app: &mut App, panel: Entity, axis: CanvasAxis) -> f32 {
    let mut query = app.world_mut().query::<(&GuideLine, &Node)>();
    let (_, node) = query
        .iter(app.world())
        .find(|(line, _)| line.host == panel && line.axis == axis)
        .expect("the panel draws the guide");
    let edge = match axis {
        CanvasAxis::Vertical => node.left,
        CanvasAxis::Horizontal => node.top,
    };
    let Val::Px(edge) = edge else {
        panic!("a guide slab is placed in pixels");
    };
    edge + 2.5
}

/// Where the guide of `axis` is marked on its ruler.
fn ruler_mark(app: &mut App, panel: Entity, axis: CanvasAxis) -> f32 {
    let mut query = app.world_mut().query::<(&RulerGuideMark, &Node)>();
    let (_, node) = query
        .iter(app.world())
        .find(|(mark, _)| mark.host == panel && mark.axis == axis)
        .expect("the ruler marks the guide");
    let edge = match axis {
        CanvasAxis::Vertical => node.left,
        CanvasAxis::Horizontal => node.top,
    };
    let Val::Px(edge) = edge else {
        panic!("a guide mark is placed in pixels");
    };
    edge + 1.5
}

fn assert_near(actual: f32, expected: f32, what: &str) {
    assert!(
        (actual - expected).abs() < 0.6,
        "{what}: {actual} vs {expected}"
    );
}

#[test]
fn an_operator_guide_sits_on_the_2d_root_in_world_units_and_reopens() {
    let (mut app, scene) = world_app();
    call_guide_op(&mut app, "canvas.guide.add", "vertical", 40.0);
    call_guide_op(&mut app, "canvas.guide.add", "horizontal", -30.0);
    let stored = CanvasGuides {
        horizontal: vec![-30.0],
        vertical: vec![40.0],
    };
    assert_eq!(guides(&app, scene.root), Some(stored.clone()));

    for view in [
        None,
        Some(Ui2dView {
            pan: Vec2::new(-60.0, 45.0),
            zoom: 0.5,
            grid: 8.0,
        }),
    ] {
        if let Some(view) = view {
            app.world_mut()
                .get_mut::<Viewport2dPanelHost>(scene.panel)
                .expect("the panel has a host")
                .set_view(view);
            settle(&mut app);
        }
        let (view, target) = host(&app, scene.panel);
        let at = world_to_target_pixel(view, target, Vec2::new(40.0, -30.0));
        assert_near(
            drawn_line(&mut app, scene.panel, CanvasAxis::Vertical),
            at.x,
            "the vertical line follows the view",
        );
        assert_near(
            drawn_line(&mut app, scene.panel, CanvasAxis::Horizontal),
            at.y,
            "the horizontal line follows the view, y up",
        );
        assert_near(
            ruler_mark(&mut app, scene.panel, CanvasAxis::Vertical),
            at.x,
            "the top ruler marks the vertical guide",
        );
        assert_near(
            ruler_mark(&mut app, scene.panel, CanvasAxis::Horizontal),
            at.y,
            "the left ruler marks the horizontal guide",
        );
        assert_eq!(
            guides(&app, scene.root),
            Some(stored.clone()),
            "pan and zoom leave the world positions alone",
        );
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("guides.bsn");
    let text = jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), tmp.path())
        .expect("the scene emits");
    assert!(text.contains("jackdaw_scene_types::CanvasGuides"), "{text}");
    std::fs::write(&path, text).expect("write the scene");

    let mut app = util::editor_test_app();
    jackdaw::migrate_dialog::request_open_with_conversion(app.world_mut(), &path);
    settle(&mut app);
    let reopened = app
        .world_mut()
        .query_filtered::<Entity, (With<Scene2dRoot>, Without<ChildOf>)>()
        .single(app.world())
        .expect("the reopened document is a 2D world scene");
    assert_eq!(guides(&app, reopened), Some(stored));
}

/// Drag a guide line from `from` to `to`. A drag starts where the pointer has
/// first moved to, which has to be on the line still, as a hand's first step
/// would be.
fn drag_line(app: &mut App, from: Vec2, to: Vec2) {
    pointer(app, from, "press", "");
    pointer(app, from + Vec2::new(0.0, 0.25), "move", "");
    pointer(app, to, "move", "");
    pointer(app, to, "release", "");
}

#[test]
fn a_guide_pulled_off_the_ruler_lands_in_world_units_and_drops_back() {
    let (mut app, scene) = world_app();
    snap_only(&mut app, &["pixel"]);
    let entries = history_len(&app);

    pointer(&mut app, Vec2::new(40.0, ON_TOP_RULER), "press", "");
    pointer(&mut app, Vec2::new(40.0, 0.0), "move", "");
    pointer(&mut app, Vec2::new(40.0, 0.0), "release", "");
    assert_eq!(vertical(&app, scene.root), vec![40.0]);
    assert_eq!(history_len(&app), entries + 1, "one drag is one entry");

    drag_line(&mut app, Vec2::new(40.0, 0.0), Vec2::new(64.0, -50.0));
    assert_eq!(vertical(&app, scene.root), vec![64.0], "the line moves");
    assert_eq!(history_len(&app), entries + 2);

    drag_line(
        &mut app,
        Vec2::new(64.0, 0.0),
        Vec2::new(64.0, ON_TOP_RULER),
    );
    assert!(
        guides(&app, scene.root).is_none(),
        "released over its ruler, the guide is gone"
    );
    assert_eq!(history_len(&app), entries + 3);

    undo(&mut app);
    assert_eq!(vertical(&app, scene.root), vec![64.0]);
    undo(&mut app);
    assert_eq!(vertical(&app, scene.root), vec![40.0]);
    undo(&mut app);
    assert!(guides(&app, scene.root).is_none());
}

#[test]
fn operator_guide_edits_undo_one_at_a_time() {
    let (mut app, scene) = world_app();
    call_guide_op(&mut app, "canvas.guide.add", "horizontal", -30.0);
    call_guide_op(&mut app, "canvas.guide.add", "horizontal", 12.0);
    call_guide_op(&mut app, "canvas.guide.remove", "horizontal", -30.2);
    let horizontal = |app: &App| guides(app, scene.root).map(|g| g.horizontal);
    assert_eq!(horizontal(&app), Some(vec![12.0]));

    undo(&mut app);
    assert_eq!(horizontal(&app), Some(vec![-30.0, 12.0]));
    undo(&mut app);
    assert_eq!(horizontal(&app), Some(vec![-30.0]));
    undo(&mut app);
    assert_eq!(horizontal(&app), None);
}

/// Drag the tile so its right edge sits at 47.5, half a unit past where a
/// guide at 47 would take it, and report where it is and the lines shown.
fn tile_toward_guide(app: &mut App, scene: &Scene) -> (Vec3, Vec<CanvasAxis>) {
    select(app, Vec2::ZERO, ActiveTool::Translate);
    pointer(app, Vec2::new(2.0, 3.0), "press", "");
    pointer(app, Vec2::new(39.5, 3.0), "move", "");
    let landed = (transform(app, scene.tile).translation, highlights(app));
    pointer(app, Vec2::new(39.5, 3.0), "release", "");
    landed
}

const ON_GUIDE: Vec3 = Vec3::new(37.0, 0.0, 0.0);
const PAST_GUIDE: Vec3 = Vec3::new(37.5, 0.0, 0.0);

#[test]
fn a_sprite_drag_lands_its_edge_on_a_guide_and_shows_where() {
    let (mut app, scene) = world_app();
    call_guide_op(&mut app, "canvas.guide.add", "vertical", 47.0);
    snap_only(&mut app, &["guides"]);
    let (at, lines) = tile_toward_guide(&mut app, &scene);
    assert!(
        near(at, ON_GUIDE),
        "the right edge rests on the guide: {at:?}"
    );
    assert_eq!(lines, vec![CanvasAxis::Vertical]);
    assert!(highlights(&mut app).is_empty(), "the line goes on release");
}

#[test]
fn guides_switched_off_or_hidden_do_not_snap() {
    for hide in [false, true] {
        let (mut app, scene) = world_app();
        call_guide_op(&mut app, "canvas.guide.add", "vertical", 47.0);
        snap_only(&mut app, if hide { &["guides"] } else { &[] });
        if hide {
            app.world_mut()
                .operator("canvas.guides")
                .param("on", false)
                .call()
                .expect("canvas.guides dispatches")
                .assert_finished();
        }
        let (at, lines) = tile_toward_guide(&mut app, &scene);
        assert!(near(at, PAST_GUIDE), "hidden {hide}: {at:?}");
        assert!(lines.is_empty());
    }
}

#[test]
fn an_anchor_only_move_still_lands_on_a_guide() {
    let (mut app, scene) = world_app();
    let ghost = ghost(&mut app, scene.root, Vec3::new(-150.3, -100.6, 0.0));
    call_guide_op(&mut app, "canvas.guide.add", "vertical", -136.0);
    snap_only(&mut app, &["guides", "sprite_edges"]);
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
        "an image of unknown size lands its anchor on the guide: {:?}",
        transform(&app, ghost).translation
    );
}

#[test]
fn only_the_shown_world_roots_guides_snap() {
    let (mut app, scene) = world_app();
    let other = app
        .world_mut()
        .spawn((Name::new("Other 2D"), Scene2dRoot))
        .id();
    let higher = scene.root.max(other);
    let lower = scene.root.min(other);
    app.world_mut().entity_mut(higher).insert(CanvasGuides {
        horizontal: Vec::new(),
        vertical: vec![47.0],
    });
    app.world_mut().entity_mut(lower).insert(CanvasGuides {
        horizontal: Vec::new(),
        vertical: vec![-60.0],
    });
    snap_only(&mut app, &["guides"]);
    settle(&mut app);
    let (view, target) = host(&app, scene.panel);
    assert_near(
        drawn_line(&mut app, scene.panel, CanvasAxis::Vertical),
        world_to_target_pixel(view, target, Vec2::new(-60.0, 0.0)).x,
        "the panel draws the lowest root's guides",
    );
    let (at, lines) = tile_toward_guide(&mut app, &scene);
    assert!(near(at, PAST_GUIDE), "and snaps to no other: {at:?}");
    assert!(lines.is_empty());
}

/// Where world point `at` shows on `panel`, in window logical pixels.
fn window_point(app: &App, panel: Entity, at: Vec2) -> Vec2 {
    let host = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("the panel has a host");
    let target = host.target_size.as_vec2();
    let pixel = world_to_target_pixel(host.view, host.target_size, at);
    let stage = app
        .world()
        .get::<ComputedNode>(host.stage)
        .expect("the stage is laid out");
    let centre = app
        .world()
        .get::<UiGlobalTransform>(host.stage)
        .expect("the stage is placed")
        .translation;
    (centre + (pixel - target / 2.0) * stage.size() / target) * stage.inverse_scale_factor()
}

fn window_pointer(app: &mut App, panel: Entity, at: Vec2, action: &str) {
    let point = window_point(app, panel, at);
    run(
        app,
        &format!("input.pointer x={} y={} action={action}", point.x, point.y),
    );
}

/// A UI scene open beside the 2D world scene, with a second panel. The UI
/// root is routed to one panel, which shows it; the other shows the world.
/// Returns the UI root and the panels showing the UI and the world.
fn mixed_document(app: &mut App, scene: &Scene) -> (Entity, Entity, Entity) {
    let second = add_panel(app, 1200.0);
    let ui_root = jackdaw::ui_palette::seed_ui_scene_root(app.world_mut());
    settle(app);
    let routed = app
        .world()
        .get::<UiTargetCamera>(ui_root)
        .expect("the UI root is routed to a panel")
        .entity();
    let camera_of = |panel: Entity| {
        app.world()
            .get::<Viewport2dPanelHost>(panel)
            .expect("the panel has a host")
            .camera
    };
    if camera_of(scene.panel) == routed {
        (ui_root, scene.panel, second)
    } else {
        assert_eq!(camera_of(second), routed);
        (ui_root, second, scene.panel)
    }
}

#[test]
fn each_panel_draws_the_guides_of_the_scene_it_shows() {
    let (mut app, scene) = world_app();
    let (ui_root, ui_panel, world_panel) = mixed_document(&mut app, &scene);
    app.world_mut().entity_mut(ui_root).insert(CanvasGuides {
        horizontal: Vec::new(),
        vertical: vec![320.0],
    });
    app.world_mut().entity_mut(scene.root).insert(CanvasGuides {
        horizontal: Vec::new(),
        vertical: vec![40.0],
    });
    settle(&mut app);

    let (view, target) = host(&app, world_panel);
    let world_x = world_to_target_pixel(view, target, Vec2::new(40.0, 0.0)).x;
    assert_near(
        drawn_line(&mut app, world_panel, CanvasAxis::Vertical),
        world_x,
        "the world panel draws the world guide",
    );
    assert_near(
        ruler_mark(&mut app, world_panel, CanvasAxis::Vertical),
        world_x,
        "and marks it on its ruler",
    );

    let (_, ui_target) = host(&app, ui_panel);
    let ui_stage = app
        .world()
        .get::<Viewport2dPanelHost>(ui_panel)
        .expect("the panel has a host")
        .stage;
    let ui_scale = app
        .world()
        .get::<ComputedNode>(ui_stage)
        .expect("the stage is laid out")
        .size()
        .x
        / ui_target.x as f32;
    assert_near(
        drawn_line(&mut app, ui_panel, CanvasAxis::Vertical) / ui_scale,
        320.0,
        "the UI panel draws the canvas guide in authored pixels",
    );
}

#[test]
fn a_ui_scenes_guides_never_snap_a_sprite_on_the_world_panel() {
    let (mut app, scene) = world_app();
    let (ui_root, _, world_panel) = mixed_document(&mut app, &scene);
    app.world_mut().entity_mut(ui_root).insert(CanvasGuides {
        horizontal: Vec::new(),
        vertical: vec![47.0],
    });
    snap_only(&mut app, &["guides"]);
    app.world_mut().insert_resource(ActiveTool::Translate);
    jackdaw::selection::select_only(app.world_mut(), scene.tile);
    settle(&mut app);

    window_pointer(&mut app, world_panel, Vec2::new(2.0, 3.0), "press");
    window_pointer(&mut app, world_panel, Vec2::new(39.5, 3.0), "move");
    window_pointer(&mut app, world_panel, Vec2::new(39.5, 3.0), "release");
    let at = transform(&app, scene.tile).translation;
    assert!(
        near(at, PAST_GUIDE),
        "the UI guide is not the world's: {at:?}"
    );
}

#[test]
fn a_ruler_guide_honours_the_grid_and_pixel_switches() {
    let (mut app, scene) = world_app();
    let pull = |app: &mut App, x: f32| {
        pointer(app, Vec2::new(x, ON_TOP_RULER), "press", "");
        pointer(app, Vec2::new(x, 0.0), "move", "");
        pointer(app, Vec2::new(x, 0.0), "release", "");
    };
    snap_only(&mut app, &["pixel"]);
    pull(&mut app, 43.3);
    assert_eq!(
        vertical(&app, scene.root),
        vec![43.0],
        "grid off: a whole unit"
    );
    snap_only(&mut app, &["grid"]);
    pull(&mut app, -21.0);
    assert_eq!(
        vertical(&app, scene.root),
        vec![-24.0, 43.0],
        "grid on: the 8-unit lattice"
    );
}

#[test]
fn a_ruler_mark_and_its_guide_line_coincide_on_a_fractional_stage() {
    let (mut app, scene) = world_app_sized(1200.5);
    app.world_mut().insert_resource(UiScale(1.5));
    settle(&mut app);
    call_guide_op(&mut app, "canvas.guide.add", "vertical", 40.0);
    let stage = app
        .world()
        .get::<Viewport2dPanelHost>(scene.panel)
        .expect("the panel has a host")
        .stage;
    let width = app
        .world()
        .get::<ComputedNode>(stage)
        .map(|stage| stage.size().x * stage.inverse_scale_factor())
        .expect("the stage is laid out");
    let (_, target) = host(&app, scene.panel);
    assert!(
        (width - target.x as f32).abs() > 0.01,
        "the stage is a fraction off its target: {width} vs {}",
        target.x
    );
    let line = drawn_line(&mut app, scene.panel, CanvasAxis::Vertical);
    let mark = ruler_mark(&mut app, scene.panel, CanvasAxis::Vertical);
    assert!((line - mark).abs() < 1e-3, "line {line} vs mark {mark}");
}

#[test]
fn the_rulers_read_world_units_with_y_up() {
    let (mut app, scene) = world_app();
    let (view, target) = host(&app, scene.panel);
    let origin = world_to_target_pixel(view, target, Vec2::ZERO);
    let labels = |app: &mut App, axis: CanvasAxis| -> Vec<(f32, f32)> {
        let ruler = app
            .world_mut()
            .query::<(Entity, &CanvasRuler)>()
            .iter(app.world())
            .find(|(_, ruler)| ruler.host == scene.panel && ruler.axis == axis)
            .map(|(entity, _)| entity)
            .expect("the panel has a ruler on each axis");
        let mut labels: Vec<(f32, f32)> = app
            .world()
            .get::<Children>(ruler)
            .into_iter()
            .flatten()
            .filter_map(|child| {
                let text = app
                    .world()
                    .get::<Children>(*child)?
                    .iter()
                    .next()
                    .and_then(|text| app.world().get::<Text>(text))?;
                let figure: f32 = text.0.parse().ok()?;
                let node = app.world().get::<Node>(*child)?;
                let edge = match axis {
                    CanvasAxis::Vertical => node.left,
                    CanvasAxis::Horizontal => node.top,
                };
                let Val::Px(edge) = edge else {
                    return None;
                };
                Some((figure, edge))
            })
            .collect();
        labels.sort_by(|a, b| a.1.total_cmp(&b.1));
        labels
    };

    let top = labels(&mut app, CanvasAxis::Vertical);
    let zero = top
        .iter()
        .find(|(figure, _)| *figure == 0.0)
        .expect("the top ruler labels world x = 0");
    assert_near(zero.1 - 2.0, origin.x, "x = 0 is labelled where it shows");
    assert!(top.windows(2).all(|pair| pair[0].0 < pair[1].0));

    let left = labels(&mut app, CanvasAxis::Horizontal);
    assert!(left.iter().any(|(figure, _)| *figure == 0.0));
    assert!(
        left.windows(2).all(|pair| pair[0].0 > pair[1].0),
        "y counts up the screen: {left:?}"
    );
}
