//! The 2D view operators on a 2D world scene: frame, pan and pixel-perfect.

use crate::sprite_gizmo::{add_panel, settle, sprite, world_app_sized};
use crate::util;

use crate::util::OperatorResultExt as _;
use bevy::{prelude::*, sprite::Anchor, ui::ComputedNode};
use jackdaw::{
    selection::Selection,
    viewport_2d::{MAX_ZOOM, Ui2dView, Viewport2dPanelHost, fit_world_rect, request_2d_fit},
};
use jackdaw_api::prelude::*;
use jackdaw_scene_types::UiSceneRoot;

fn view_of(app: &App, panel: Entity) -> Ui2dView {
    app.world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("host on panel")
        .view
}

fn area_of(app: &App, panel: Entity) -> Vec2 {
    let area = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("host on panel")
        .area;
    let computed = app
        .world()
        .get::<ComputedNode>(area)
        .expect("the area is laid out");
    computed.size() * computed.inverse_scale_factor()
}

/// The sprites' union: x -100..110, y -10..70.
fn all_rect() -> Rect {
    Rect::from_corners(Vec2::new(-100.0, -10.0), Vec2::new(110.0, 70.0))
}

fn frame(app: &mut App) -> OperatorResult {
    let result = app
        .world_mut()
        .operator("viewport2d.frame")
        .call()
        .expect("viewport2d.frame dispatches");
    settle(app);
    result
}

fn select(app: &mut App, entities: &[Entity]) {
    app.world_mut().resource_mut::<Selection>().entities = entities.to_vec();
}

/// A viewport panel in front in 2D, which is what lets Home mean the canvas.
fn front_2d(app: &mut App) {
    use jackdaw_panels::{
        area::{DockAreaStyle, DockTabContent},
        tree::{DockLeaf, DockNode, DockTree},
    };
    let mut tree = DockTree::new();
    let leaf = tree.insert(DockNode::Leaf(
        DockLeaf::new("root", DockAreaStyle::TabBar)
            .with_windows(vec![jackdaw::viewport::VIEWPORT_WINDOW_ID.into()]),
    ));
    tree.root = Some(leaf);
    let tab = tree
        .get(leaf)
        .and_then(|node| node.as_leaf())
        .expect("a leaf")
        .windows[0]
        .id;
    *app.world_mut().resource_mut::<DockTree>() = tree;
    let three_d = app.world_mut().spawn_empty().id();
    let two_d = app.world_mut().spawn_empty().id();
    app.world_mut().spawn((
        jackdaw::viewport_host::ViewportHost {
            mode: jackdaw::viewport_host::ViewportMode::TwoD,
            mode_chosen: true,
            three_d,
            two_d,
        },
        DockTabContent {
            window_id: jackdaw::viewport::VIEWPORT_WINDOW_ID.to_string(),
            tab_id: tab,
        },
    ));
    settle(app);
}

#[test]
fn frame_fits_every_sprite_when_none_is_selected() {
    let (mut app, scene) = world_app_sized(1200.0);
    front_2d(&mut app);
    frame(&mut app).assert_finished();

    let expected = fit_world_rect(
        view_of(&app, scene.panel),
        all_rect(),
        area_of(&app, scene.panel),
    );
    let view = view_of(&app, scene.panel);
    assert_eq!(view.pan, Vec2::new(5.0, 30.0), "centred on the sprites");
    assert_eq!(view.zoom, expected.zoom);
    assert!(view.zoom < 1200.0 / 210.0, "with a margin to spare");
    let host = app.world().get::<Viewport2dPanelHost>(scene.panel).unwrap();
    assert!(host.view_touched && !host.fit_pending);
}

#[test]
fn frame_fits_the_selection_when_a_sprite_is_selected() {
    let (mut app, scene) = world_app_sized(1200.0);
    front_2d(&mut app);
    select(&mut app, &[scene.other]);
    frame(&mut app).assert_finished();
    let view = view_of(&app, scene.panel);
    assert_eq!(view.pan, Vec2::new(100.0, 0.0));
    assert_eq!(
        view.zoom,
        fit_world_rect(
            view,
            Rect::from_center_size(Vec2::new(100.0, 0.0), Vec2::splat(20.0)),
            area_of(&app, scene.panel)
        )
        .zoom
    );

    select(&mut app, &[scene.tile, scene.corner]);
    frame(&mut app).assert_finished();
    assert_eq!(
        view_of(&app, scene.panel).pan,
        Vec2::new(-45.0, 30.0),
        "x -100..10, y -10..70",
    );
}

#[test]
fn a_selection_outside_the_scene_frames_every_sprite() {
    let (mut app, scene) = world_app_sized(1200.0);
    front_2d(&mut app);
    let stray = app.world_mut().spawn(Transform::default()).id();
    select(&mut app, &[stray]);
    frame(&mut app).assert_finished();
    assert_eq!(view_of(&app, scene.panel).pan, Vec2::new(5.0, 30.0));
}

#[test]
fn frame_on_an_empty_scene_resets_to_the_origin_at_zoom_one() {
    let (mut app, scene) = world_app_sized(1200.0);
    front_2d(&mut app);
    for entity in [scene.tile, scene.other, scene.corner] {
        app.world_mut().entity_mut(entity).despawn();
    }
    settle(&mut app);
    frame(&mut app).assert_finished();
    let view = view_of(&app, scene.panel);
    assert_eq!(view.pan, Vec2::ZERO);
    assert_eq!(view.zoom, 1.0);
}

#[test]
fn a_sprite_whose_image_is_not_loaded_counts_as_its_origin() {
    let (mut app, scene) = world_app_sized(1200.0);
    front_2d(&mut app);
    let pending = sprite(
        &mut app,
        scene.root,
        "Pending",
        Sprite {
            image: Handle::Uuid(
                bevy::asset::uuid::Uuid::from_u128(0x7e57),
                core::marker::PhantomData,
            ),
            ..default()
        },
        Anchor::CENTER,
        Vec3::new(300.0, 200.0, 0.0),
    );
    settle(&mut app);

    frame(&mut app).assert_finished();
    let rect = Rect::from_corners(Vec2::new(-100.0, -10.0), Vec2::new(300.0, 200.0));
    let view = view_of(&app, scene.panel);
    assert_eq!(view.pan, rect.center(), "the origin point widens the rect");

    select(&mut app, &[pending]);
    frame(&mut app).assert_finished();
    let view = view_of(&app, scene.panel);
    assert_eq!(view.pan, Vec2::new(300.0, 200.0));
    assert_eq!(
        view.zoom, MAX_ZOOM,
        "a lone point frames at the closest zoom"
    );
}

#[test]
fn frame_still_fits_a_ui_scene_canvas() {
    let (mut app, scene) = world_app_sized(1200.0);
    front_2d(&mut app);
    app.world_mut().entity_mut(scene.root).despawn();
    app.world_mut().spawn((
        UiSceneRoot {
            reference_size: UVec2::new(1280, 720),
        },
        Node::default(),
    ));
    settle(&mut app);
    {
        let mut host = app
            .world_mut()
            .get_mut::<Viewport2dPanelHost>(scene.panel)
            .unwrap();
        host.view = Ui2dView {
            pan: Vec2::new(400.0, -250.0),
            zoom: 6.0,
            ..default()
        };
    }
    request_2d_fit(app.world_mut());
    settle(&mut app);
    let requested = view_of(&app, scene.panel);

    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(scene.panel)
        .unwrap()
        .view = Ui2dView::default();
    frame(&mut app).assert_finished();
    assert_eq!(
        view_of(&app, scene.panel),
        requested,
        "the canvas is fitted"
    );
    assert_ne!(requested.zoom, 1.0, "the fit moved the zoom");
    assert_eq!(requested.pan, Vec2::ZERO);
}

#[test]
fn frame_is_offered_only_while_the_2d_panel_is_in_front() {
    let (mut app, _scene) = world_app_sized(1200.0);
    assert!(!available(&mut app, "viewport2d.frame"));
    front_2d(&mut app);
    assert!(available(&mut app, "viewport2d.frame"));
}

fn available(app: &mut App, id: &'static str) -> bool {
    app.world_mut()
        .operator(id)
        .is_available()
        .unwrap_or_else(|err| panic!("{id}: is_available errored: {err}"))
}

#[test]
fn pan_centres_the_named_panel_and_stands_a_pending_fit_down() {
    let (mut app, scene) = world_app_sized(1200.0);
    let second = add_panel(&mut app, 800.0);
    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(scene.panel)
        .unwrap()
        .fit_pending = true;
    let before = view_of(&app, second);

    app.world_mut()
        .operator("viewport2d.pan")
        .param("x", 12.5)
        .param("y", -40.0)
        .param("panel", scene.panel.to_bits() as i64)
        .call()
        .expect("viewport2d.pan dispatches")
        .assert_finished();
    settle(&mut app);

    let view = view_of(&app, scene.panel);
    assert_eq!(view.pan, Vec2::new(12.5, -40.0));
    assert_eq!(view.zoom, 2.0, "the zoom is untouched");
    assert!(
        !app.world()
            .get::<Viewport2dPanelHost>(scene.panel)
            .unwrap()
            .fit_pending
    );
    assert_eq!(view_of(&app, second), before, "the other panel stays put");

    app.world_mut()
        .operator("viewport2d.pan")
        .param("x", 1.0)
        .param("y", 2.0)
        .call()
        .expect("viewport2d.pan dispatches")
        .assert_finished();
    assert_eq!(view_of(&app, scene.panel).pan, Vec2::new(1.0, 2.0));
    assert_eq!(view_of(&app, second).pan, Vec2::new(1.0, 2.0));
}

#[test]
fn pan_refuses_missing_or_non_finite_coordinates() {
    let (mut app, scene) = world_app_sized(1200.0);
    let before = view_of(&app, scene.panel);
    app.world_mut()
        .operator("viewport2d.pan")
        .param("x", 5.0)
        .call()
        .expect("viewport2d.pan dispatches")
        .assert_cancelled();
    app.world_mut()
        .operator("viewport2d.pan")
        .param("x", f64::NAN)
        .param("y", 0.0)
        .call()
        .expect("viewport2d.pan dispatches")
        .assert_cancelled();
    assert_eq!(view_of(&app, scene.panel), before);
}

#[test]
fn pan_and_pixel_perfect_do_nothing_without_a_panel() {
    let mut app = util::editor_test_app();
    app.world_mut()
        .operator("viewport2d.pan")
        .param("x", 1.0)
        .param("y", 1.0)
        .call()
        .expect("viewport2d.pan dispatches")
        .assert_cancelled();
    app.world_mut()
        .operator("viewport2d.pixel_perfect")
        .call()
        .expect("viewport2d.pixel_perfect dispatches")
        .assert_cancelled();
}

/// How far, in physical pixels, the nearest texel edge of the unit grid
/// (shifted by `edge_offset` texels) sits from a pixel edge, on each axis.
fn edge_misses(app: &App, panel: Entity, edge_offset: Vec2) -> Vec2 {
    let host = app.world().get::<Viewport2dPanelHost>(panel).unwrap();
    let area = app.world().get::<ComputedNode>(host.area).unwrap();
    let scale = 1.0 / area.inverse_scale_factor();
    let centre = area.size() / 2.0;
    let at = centre + (edge_offset - host.view.pan) * host.view.zoom * scale;
    let miss = |value: f32| (value - value.round()).abs();
    Vec2::new(miss(at.x), miss(at.y))
}

fn run_pixel_perfect(app: &mut App, panel: Option<Entity>) {
    let mut call = app.world_mut().operator("viewport2d.pixel_perfect");
    if let Some(panel) = panel {
        call = call.param("panel", panel.to_bits() as i64);
    }
    call.call()
        .expect("viewport2d.pixel_perfect dispatches")
        .assert_finished();
}

#[test]
fn pixel_perfect_snaps_a_world_view_to_whole_texels() {
    let (mut app, scene) = world_app_sized(1200.0);
    let second = add_panel(&mut app, 800.0);
    for panel in [scene.panel, second] {
        app.world_mut()
            .get_mut::<Viewport2dPanelHost>(panel)
            .unwrap()
            .set_view(Ui2dView {
                pan: Vec2::new(10.4, -3.6),
                zoom: 2.6,
                grid: 8.0,
            });
    }

    run_pixel_perfect(&mut app, Some(scene.panel));
    assert_eq!(view_of(&app, scene.panel).zoom, 3.0);
    assert_eq!(
        view_of(&app, second).zoom,
        2.6,
        "the other panel is not named"
    );

    run_pixel_perfect(&mut app, None);
    assert_eq!(view_of(&app, second).zoom, 3.0);
}

#[test]
fn pixel_perfect_puts_texel_edges_on_pixel_edges_at_odd_and_even_areas() {
    let (mut app, scene) = world_app_sized(1200.0);
    let panels = [
        scene.panel,
        add_panel(&mut app, 1201.0),
        add_panel(&mut app, 1202.0),
        add_panel(&mut app, 1203.0),
    ];
    settle(&mut app);
    let mut parities = Vec::new();
    for panel in panels {
        app.world_mut()
            .get_mut::<Viewport2dPanelHost>(panel)
            .unwrap()
            .set_view(Ui2dView {
                pan: Vec2::new(10.4, -3.6),
                zoom: 2.6,
                grid: 8.0,
            });
        run_pixel_perfect(&mut app, Some(panel));
        let miss = edge_misses(&app, panel, Vec2::ZERO);
        assert!(miss.x < 1e-3 && miss.y < 1e-3, "edges off pixels by {miss}");
        let host = app.world().get::<Viewport2dPanelHost>(panel).unwrap();
        let area = app.world().get::<ComputedNode>(host.area).unwrap();
        parities.push(area.size().x.round() % 2.0);
    }
    assert!(
        parities.contains(&0.0) && parities.contains(&1.0),
        "both parities were exercised: {parities:?}",
    );
}

fn ui_canvas_app(reference: UVec2) -> (App, Entity) {
    let (mut app, scene) = world_app_sized(1200.0);
    app.world_mut().entity_mut(scene.root).despawn();
    app.world_mut().spawn((
        UiSceneRoot {
            reference_size: reference,
        },
        Node::default(),
    ));
    settle(&mut app);
    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(scene.panel)
        .unwrap()
        .set_view(Ui2dView {
            pan: Vec2::new(0.4, 7.6),
            zoom: 0.3,
            grid: 8.0,
        });
    (app, scene.panel)
}

#[test]
fn pixel_perfect_snaps_a_ui_canvas_to_whole_pixels() {
    let (mut app, panel) = ui_canvas_app(UVec2::new(1280, 720));
    run_pixel_perfect(&mut app, None);
    let view = view_of(&app, panel);
    assert_eq!(view.zoom, 1.0);
    let miss = edge_misses(&app, panel, Vec2::ZERO);
    assert!(miss.x < 1e-3 && miss.y < 1e-3, "edges off pixels by {miss}");
}

#[test]
fn pixel_perfect_follows_an_odd_ui_canvas_to_half_pixels() {
    let (mut app, panel) = ui_canvas_app(UVec2::new(1281, 721));
    run_pixel_perfect(&mut app, None);
    let miss = edge_misses(&app, panel, Vec2::splat(0.5));
    assert!(miss.x < 1e-3 && miss.y < 1e-3, "edges off pixels by {miss}");
}

#[test]
fn a_panel_that_is_not_laid_out_keeps_the_frame_pending_until_it_is() {
    let (mut app, scene) = world_app_sized(1200.0);
    front_2d(&mut app);
    app.world_mut()
        .get_mut::<Node>(scene.panel)
        .unwrap()
        .display = Display::None;
    settle(&mut app);
    let before = view_of(&app, scene.panel);

    frame(&mut app).assert_finished();
    let host = app.world().get::<Viewport2dPanelHost>(scene.panel).unwrap();
    assert!(host.fit_pending, "no size to fit into yet");
    assert_eq!(host.view, before);

    app.world_mut()
        .get_mut::<Node>(scene.panel)
        .unwrap()
        .display = Display::Flex;
    settle(&mut app);
    let host = app.world().get::<Viewport2dPanelHost>(scene.panel).unwrap();
    assert!(
        !host.fit_pending,
        "the fit landed once the panel had a size"
    );
    assert_eq!(host.view.pan, Vec2::new(5.0, 30.0));
}

#[test]
fn a_world_scene_is_fitted_when_its_panel_first_has_it() {
    let (mut app, scene) = world_app_sized(1200.0);
    {
        let mut host = app
            .world_mut()
            .get_mut::<Viewport2dPanelHost>(scene.panel)
            .unwrap();
        host.view = Ui2dView::default();
        host.view_touched = false;
        host.fit_pending = true;
    }
    settle(&mut app);
    let host = app.world().get::<Viewport2dPanelHost>(scene.panel).unwrap();
    assert!(!host.fit_pending);
    assert_eq!(host.view.pan, Vec2::new(5.0, 30.0));
}

#[test]
fn frame_skips_hidden_and_editor_sprites() {
    let (mut app, scene) = world_app_sized(1200.0);
    front_2d(&mut app);
    let far = Sprite::from_color(Color::WHITE, Vec2::splat(20.0));
    let hidden = sprite(
        &mut app,
        scene.root,
        "Hidden",
        far.clone(),
        Anchor::CENTER,
        Vec3::new(900.0, 900.0, 0.0),
    );
    app.world_mut()
        .entity_mut(hidden)
        .insert(Visibility::Hidden);
    let chrome = sprite(
        &mut app,
        scene.root,
        "Chrome",
        far,
        Anchor::CENTER,
        Vec3::new(-900.0, -900.0, 0.0),
    );
    app.world_mut()
        .entity_mut(chrome)
        .insert(jackdaw::EditorEntity);
    settle(&mut app);

    frame(&mut app).assert_finished();
    assert_eq!(view_of(&app, scene.panel).pan, Vec2::new(5.0, 30.0));
}

fn group_of_two(app: &mut App, root: Entity) -> Entity {
    let group = app
        .world_mut()
        .spawn((Name::new("Group"), Transform::default(), ChildOf(root)))
        .id();
    for (name, x) in [("A", 500.0), ("B", 600.0)] {
        sprite(
            app,
            group,
            name,
            Sprite::from_color(Color::WHITE, Vec2::splat(20.0)),
            Anchor::CENTER,
            Vec3::new(x, 500.0, 0.0),
        );
    }
    settle(app);
    group
}

#[test]
fn a_selected_group_frames_the_sprites_under_it() {
    let (mut app, scene) = world_app_sized(1200.0);
    front_2d(&mut app);
    let group = group_of_two(&mut app, scene.root);
    select(&mut app, &[group]);
    frame(&mut app).assert_finished();
    assert_eq!(view_of(&app, scene.panel).pan, Vec2::new(550.0, 500.0));
}

#[test]
fn a_mixed_selection_frames_the_group_and_the_sprite_together() {
    let (mut app, scene) = world_app_sized(1200.0);
    front_2d(&mut app);
    let group = group_of_two(&mut app, scene.root);
    select(&mut app, &[group, scene.tile]);
    frame(&mut app).assert_finished();
    assert_eq!(view_of(&app, scene.panel).pan, Vec2::new(300.0, 250.0));
}

fn at_scale_factor_two(app: &mut App) {
    use bevy::window::{PrimaryWindow, WindowScaleFactorChanged};

    let mut windows = app
        .world_mut()
        .query_filtered::<(Entity, &mut Window), With<PrimaryWindow>>();
    let (window_entity, mut window) = windows
        .single_mut(app.world_mut())
        .expect("a primary window");
    window.resolution.set_scale_factor_override(Some(2.0));
    app.world_mut().write_message(WindowScaleFactorChanged {
        window: window_entity,
        scale_factor: 2.0,
    });
    settle(app);
}

/// The panel's target size, its image's size and sampler, the camera's
/// logical viewport and the stage's physical size.
fn panel_target(app: &App, panel: Entity) -> (UVec2, UVec2, bool, Vec2, Vec2) {
    use bevy::camera::RenderTarget;

    let host = app.world().get::<Viewport2dPanelHost>(panel).unwrap();
    let Some(RenderTarget::Image(target)) = app.world().get::<RenderTarget>(host.camera) else {
        panic!("the panel camera renders into an image");
    };
    assert_eq!(target.scale_factor, 2.0);
    let image = app
        .world()
        .resource::<Assets<Image>>()
        .get(&target.handle)
        .expect("the panel image");
    let camera = app.world().get::<Camera>(host.camera).unwrap();
    let stage = app.world().get::<ComputedNode>(host.stage).unwrap();
    (
        host.target_size,
        image.size(),
        image.sampler == bevy::image::ImageSampler::nearest(),
        camera.logical_viewport_size().expect("a laid-out viewport"),
        stage.size(),
    )
}

#[test]
fn a_world_panel_renders_one_image_pixel_per_physical_pixel_at_scale_factor_two() {
    let (mut app, scene) = world_app_sized(1200.0);
    at_scale_factor_two(&mut app);

    let (target, image, nearest, viewport, stage) = panel_target(&app, scene.panel);
    assert_eq!(target, UVec2::new(1200, 600), "measured in logical pixels");
    assert_eq!(image, UVec2::new(2400, 1200));
    assert!(nearest, "the panel image is shown pixel for pixel");
    assert_eq!(viewport, target.as_vec2());
    assert_eq!(stage, image.as_vec2());
}

#[test]
fn an_odd_physical_area_keeps_the_camera_and_the_target_in_step() {
    let (mut app, _) = world_app_sized(1200.0);
    let panel = add_panel(&mut app, 600.5);
    at_scale_factor_two(&mut app);

    let (target, image, _, viewport, stage) = panel_target(&app, panel);
    assert_eq!(
        target.x, 601,
        "the 1201 physical pixels round to whole logical ones"
    );
    assert_eq!(image, target * 2);
    assert_eq!(
        viewport,
        target.as_vec2(),
        "the camera centres where the view does"
    );
    assert_eq!(
        stage,
        image.as_vec2(),
        "the stage shows the image pixel for pixel"
    );

    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(panel)
        .unwrap()
        .set_view(Ui2dView {
            pan: Vec2::new(10.4, -3.6),
            zoom: 2.6,
            grid: 8.0,
        });
    run_pixel_perfect(&mut app, Some(panel));
    let host = app.world().get::<Viewport2dPanelHost>(panel).unwrap();
    let edge =
        jackdaw::viewport_2d::world_to_target_pixel(host.view, host.target_size, Vec2::ZERO) * 2.0;
    let miss = (edge - edge.round()).abs();
    assert!(
        miss.x < 1e-3 && miss.y < 1e-3,
        "texel edges off image pixels by {miss}"
    );
}
