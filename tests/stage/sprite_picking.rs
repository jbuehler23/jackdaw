//! Picking and outlining sprites on the 2D panel's stage in a 2D world scene,
//! driven through the synthetic pointer in world coordinates.

use crate::util;

use crate::util::OperatorResultExt as _;
use bevy::{
    asset::RenderAssetUsages,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
    sprite::Anchor,
    ui::ComputedNode,
    window::{PrimaryWindow, WindowResolution},
};
use bevy::{camera::RenderTarget, prelude::*};
use jackdaw::{
    selection::Selection,
    sprite_stage::SpriteSelectionOutline,
    test_input::SyntheticInput,
    ui_stage::{MarqueeSelect, stage_pixels_per_target_pixel},
    viewport_2d::{
        RULER_SIZE, Ui2dView, Viewport2dPanelHost, build_viewport_2d_panel,
        target_pixels_per_stage_pixel, world_to_target_pixel,
    },
};

/// The view every test frames the panel with: off-centre and zoomed, so a
/// world-space click only lands if the mapping honours both.
const VIEW: Ui2dView = Ui2dView {
    pan: Vec2::new(100.0, -20.0),
    zoom: 2.0,
    grid: 8.0,
};

struct Scene {
    panel: Entity,
    root: Entity,
    /// 40x20, bottom-left anchored at the origin; its right half is transparent.
    half_clear: Entity,
    /// 10x20 colour sprite at (200, 0), turned a quarter and scaled by two.
    turned: Entity,
    /// Two 20x20 colour sprites stacked at (0, -100), `over` the higher z.
    under: Entity,
    over: Entity,
}

fn world_app() -> (App, Scene) {
    world_app_at(1.0)
}

/// [`world_app`] laid out at `scale` physical pixels per logical one.
fn world_app_at(scale: f32) -> (App, Scene) {
    let mut app = util::editor_test_app();
    util::fixed_frame_clock(&mut app);
    {
        let mut windows = app
            .world_mut()
            .query_filtered::<&mut Window, With<PrimaryWindow>>();
        let mut window = windows
            .single_mut(app.world_mut())
            .expect("headless apps still have a primary window");
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a small whole scale factor"
        )]
        let physical = |logical: f32| (logical * scale) as u32;
        window.resolution = WindowResolution::new(physical(1600.0), physical(1000.0));
    }
    // How the editor is laid out on a hidpi screen, as the outliner tests
    // emulate it: every node figure doubled, the panel the same logical size.
    app.world_mut().insert_resource(UiScale(scale));
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
    let image = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::new(
            Extent3d {
                width: 2,
                height: 1,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            vec![255, 255, 255, 255, 0, 0, 0, 0],
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        ));
    let half_clear = sprite(
        &mut app,
        root,
        Sprite {
            image,
            custom_size: Some(Vec2::new(40.0, 20.0)),
            ..default()
        },
        Anchor::BOTTOM_LEFT,
        Transform::default(),
    );
    let turned = sprite(
        &mut app,
        root,
        Sprite::from_color(Color::WHITE, Vec2::new(10.0, 20.0)),
        Anchor::CENTER,
        Transform::from_xyz(200.0, 0.0, 0.0)
            .with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2))
            .with_scale(Vec3::splat(2.0)),
    );
    // Spawned first, so only its z can put it on top of the later one.
    let over = sprite(
        &mut app,
        root,
        Sprite::from_color(Color::BLACK, Vec2::splat(20.0)),
        Anchor::CENTER,
        Transform::from_xyz(0.0, -100.0, 2.0),
    );
    let under = sprite(
        &mut app,
        root,
        Sprite::from_color(Color::WHITE, Vec2::splat(20.0)),
        Anchor::CENTER,
        Transform::from_xyz(0.0, -100.0, 1.0),
    );
    jackdaw::selection::clear_selection_in_world(app.world_mut());
    settle(&mut app);

    (
        app,
        Scene {
            panel,
            root,
            half_clear,
            turned,
            under,
            over,
        },
    )
}

fn sprite(app: &mut App, root: Entity, sprite: Sprite, anchor: Anchor, at: Transform) -> Entity {
    let entity = app
        .world_mut()
        .spawn((Name::new("Sprite"), sprite, anchor, at, ChildOf(root)))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    entity
}

fn settle(app: &mut App) {
    for _ in 0..8 {
        app.update();
    }
}

fn run(app: &mut App, clause: &str) {
    jackdaw::boot_ops::run_op_clause(app.world_mut(), clause)
        .expect("the clause dispatches")
        .assert_finished();
    play(app);
}

fn play(app: &mut App) {
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

fn click(app: &mut App, at: Vec2, mods: &str) {
    let mods = if mods.is_empty() {
        String::new()
    } else {
        format!(" mods={mods}")
    };
    run(
        app,
        &format!(
            "input.pointer space=world x={} y={} action=click{mods}",
            at.x, at.y
        ),
    );
}

fn selected(app: &App) -> Vec<Entity> {
    app.world().resource::<Selection>().entities.clone()
}

#[test]
fn the_render_image_matches_the_stage_in_a_world_scene() {
    let (app, scene) = world_app();
    let host = app
        .world()
        .get::<Viewport2dPanelHost>(scene.panel)
        .expect("host on panel parent");
    let stage = app
        .world()
        .get::<ComputedNode>(host.stage)
        .expect("the stage is laid out");
    assert_eq!(host.target_size, stage.size().as_uvec2());
    let Some(RenderTarget::Image(target)) = app.world().get::<RenderTarget>(host.camera) else {
        panic!("the 2D camera renders to an image");
    };
    let image = app
        .world()
        .resource::<Assets<Image>>()
        .get(&target.handle)
        .expect("the target image exists");
    assert_eq!(image.size(), host.target_size, "the image is not stretched");
}

#[test]
fn a_world_click_selects_the_sprite_by_its_anchor() {
    let (mut app, scene) = world_app();
    click(&mut app, Vec2::new(10.0, 10.0), "");
    assert_eq!(selected(&app), vec![scene.half_clear]);

    // Inside where a centre-anchored rect would be, outside the anchored one.
    click(&mut app, Vec2::new(-10.0, -5.0), "");
    assert!(selected(&app).is_empty(), "the anchor moved the rect");
}

#[test]
fn a_transparent_texel_is_clicked_through() {
    let (mut app, _scene) = world_app();
    click(&mut app, Vec2::new(30.0, 10.0), "");
    assert!(selected(&app).is_empty());
}

#[test]
fn rotation_and_scale_are_respected() {
    let (mut app, scene) = world_app();
    click(&mut app, Vec2::new(217.0, 0.0), "");
    assert_eq!(
        selected(&app),
        vec![scene.turned],
        "the turned rect reaches x=220"
    );
    click(&mut app, Vec2::new(200.0, 15.0), "");
    assert!(selected(&app).is_empty(), "the turned rect stops at y=10");
}

#[test]
fn the_topmost_sprite_wins() {
    let (mut app, scene) = world_app();
    click(&mut app, Vec2::new(0.0, -100.0), "");
    assert_eq!(selected(&app), vec![scene.over]);
    assert!(!selected(&app).contains(&scene.under));
}

#[test]
fn a_miss_clears_and_modifiers_extend_or_toggle() {
    let (mut app, scene) = world_app();
    click(&mut app, Vec2::new(10.0, 10.0), "");
    click(&mut app, Vec2::new(200.0, 0.0), "shift");
    assert_eq!(selected(&app), vec![scene.half_clear, scene.turned]);

    click(&mut app, Vec2::new(10.0, 10.0), "ctrl");
    assert_eq!(selected(&app), vec![scene.turned], "ctrl toggles it off");

    click(&mut app, Vec2::new(300.0, 50.0), "shift");
    assert_eq!(selected(&app), vec![scene.turned], "a shift miss keeps it");

    click(&mut app, Vec2::new(300.0, 50.0), "");
    assert!(selected(&app).is_empty(), "a plain miss clears");
}

#[test]
fn a_drag_starts_no_band_and_keeps_the_selection() {
    let (mut app, scene) = world_app();
    run(&mut app, "input.pointer space=world x=10 y=10 action=press");
    run(
        &mut app,
        "input.pointer space=world x=150 y=-60 action=move",
    );
    assert!(
        app.world().resource::<MarqueeSelect>().corners().is_none(),
        "a drag from a sprite pulls out no band"
    );
    run(
        &mut app,
        "input.pointer space=world x=150 y=-60 action=release",
    );
    assert_eq!(
        selected(&app),
        vec![scene.half_clear],
        "a drag from a sprite keeps it selected"
    );

    run(
        &mut app,
        "input.pointer space=world x=300 y=50 action=press mods=shift",
    );
    run(&mut app, "input.pointer space=world x=330 y=30 action=move");
    assert!(
        app.world().resource::<MarqueeSelect>().corners().is_none(),
        "no band runs over a world scene"
    );
    run(
        &mut app,
        "input.pointer space=world x=330 y=30 action=release",
    );
    assert_eq!(
        selected(&app),
        vec![scene.half_clear],
        "a drag over empty space leaves the selection"
    );
}

#[test]
fn each_selected_sprite_gets_one_outline_on_its_transformed_rect() {
    let (mut app, scene) = world_app();
    click(&mut app, Vec2::new(10.0, 10.0), "");
    click(&mut app, Vec2::new(200.0, 0.0), "shift");

    let outlines: Vec<SpriteSelectionOutline> = app
        .world_mut()
        .query::<&SpriteSelectionOutline>()
        .iter(app.world())
        .copied()
        .collect();
    assert_eq!(outlines.len(), 2, "one outline per selected sprite");

    let host = app
        .world()
        .get::<Viewport2dPanelHost>(scene.panel)
        .expect("host on panel parent");
    let stage = app
        .world()
        .get::<ComputedNode>(host.stage)
        .expect("the stage is laid out");
    let scale = stage_pixels_per_target_pixel(
        target_pixels_per_stage_pixel(stage.size(), host.target_size),
        stage.inverse_scale_factor(),
    );
    let expected = [
        Vec2::new(220.0, -10.0),
        Vec2::new(220.0, 10.0),
        Vec2::new(180.0, 10.0),
        Vec2::new(180.0, -10.0),
    ]
    .map(|world| world_to_target_pixel(VIEW, host.target_size, world) * scale);
    let turned = outlines
        .iter()
        .find(|outline| outline.sprite == scene.turned)
        .expect("the turned sprite is outlined");
    for (corner, want) in turned.corners.iter().zip(expected) {
        assert!(
            (*corner - want).length() < 0.01,
            "corners {:?} against {expected:?}",
            turned.corners
        );
    }

    click(&mut app, Vec2::new(300.0, 50.0), "");
    let left = app
        .world_mut()
        .query::<&SpriteSelectionOutline>()
        .iter(app.world())
        .count();
    assert_eq!(left, 0, "clearing the selection takes the outlines down");
}

/// At a scale factor of two a world click still lands, and a sprite's outline
/// on the stage is its world size times the zoom in logical pixels, the measure
/// the view is driven in.
#[test]
fn a_scale_factor_of_two_keeps_picking_and_sizes_in_step() {
    let (mut app, scene) = world_app_at(2.0);
    {
        let host = app
            .world()
            .get::<Viewport2dPanelHost>(scene.panel)
            .expect("host on panel parent");
        let stage = app
            .world()
            .get::<ComputedNode>(host.stage)
            .expect("the stage is laid out");
        assert_eq!(
            stage.inverse_scale_factor(),
            0.5,
            "the stage is laid out at scale factor two: {:?} physical, target {}",
            stage.size(),
            host.target_size
        );
    }
    // Both sprites framed left of the stage's centre: under a doubled UI
    // scale this headless app drops presses further right than about that.
    let zoom = 1.5;
    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(scene.panel)
        .expect("host on panel parent")
        .set_view(Ui2dView {
            pan: Vec2::new(300.0, -20.0),
            zoom,
            ..VIEW
        });
    settle(&mut app);

    click(&mut app, Vec2::new(10.0, 10.0), "");
    assert_eq!(selected(&app), vec![scene.half_clear]);
    click(&mut app, Vec2::new(212.0, 0.0), "");
    assert_eq!(selected(&app), vec![scene.turned]);

    let outline = app
        .world_mut()
        .query::<&SpriteSelectionOutline>()
        .iter(app.world())
        .copied()
        .find(|outline| outline.sprite == scene.turned)
        .expect("the turned sprite is outlined");
    let [a, b, c, _] = outline.corners;
    assert!(
        ((b - a).length() - 20.0 * zoom).abs() < 0.01,
        "the 20-unit side spans {} logical pixels",
        (b - a).length()
    );
    assert!(
        ((c - b).length() - 40.0 * zoom).abs() < 0.01,
        "the 40-unit side spans {} logical pixels",
        (c - b).length()
    );
}

/// Equal z goes to the sprite later in the scene document, not to the larger
/// entity id: the later sprite here reuses a freed, smaller index.
#[test]
fn a_tie_on_z_goes_to_the_later_sprite_in_the_document() {
    let (mut app, scene) = world_app();
    let freed = app.world_mut().spawn_empty().id();
    let earlier = sprite(
        &mut app,
        scene.root,
        Sprite::from_color(Color::WHITE, Vec2::splat(20.0)),
        Anchor::CENTER,
        Transform::from_xyz(-150.0, 100.0, 0.0),
    );
    // Released and taken again at once, as an allocator recycling the index
    // would: the same index under a newer generation.
    let recycled = app
        .world_mut()
        .despawn_no_free(freed)
        .expect("the placeholder was spawned");
    let later = app
        .world_mut()
        .spawn_at(
            recycled,
            (
                Name::new("Sprite"),
                Sprite::from_color(Color::BLACK, Vec2::splat(20.0)),
                Anchor::CENTER,
                Transform::from_xyz(-150.0, 100.0, 0.0),
                ChildOf(scene.root),
            ),
        )
        .expect("the recycled index is free")
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), later);
    assert!(
        later.index() < earlier.index(),
        "the later sprite recycled a smaller index: {later} against {earlier}"
    );
    assert!(
        later.generation() != freed.generation(),
        "under a newer generation"
    );
    settle(&mut app);

    click(&mut app, Vec2::new(-150.0, 100.0), "");
    assert_eq!(selected(&app), vec![later]);
}

#[test]
fn hidden_editor_and_unloaded_sprites_do_not_pick() {
    let (mut app, scene) = world_app();
    app.world_mut()
        .entity_mut(scene.over)
        .insert(Visibility::Hidden);
    let chrome = app
        .world_mut()
        .spawn((
            jackdaw::EditorEntity,
            Sprite::from_color(Color::WHITE, Vec2::splat(20.0)),
            Transform::from_xyz(-150.0, 100.0, 0.0),
            ChildOf(scene.root),
        ))
        .id();
    let missing = app
        .world()
        .resource::<AssetServer>()
        .load::<Image>("no/such/image.png");
    sprite(
        &mut app,
        scene.root,
        Sprite {
            image: missing,
            custom_size: Some(Vec2::splat(20.0)),
            ..default()
        },
        Anchor::CENTER,
        Transform::from_xyz(-150.0, -100.0, 0.0),
    );
    settle(&mut app);

    click(&mut app, Vec2::new(0.0, -100.0), "");
    assert_eq!(
        selected(&app),
        vec![scene.under],
        "the hidden sprite is skipped"
    );
    click(&mut app, Vec2::new(-150.0, 100.0), "");
    assert!(
        !selected(&app).contains(&chrome) && selected(&app).is_empty(),
        "editor chrome is not picked"
    );
    click(&mut app, Vec2::new(-150.0, -100.0), "");
    assert!(
        selected(&app).is_empty(),
        "an image that never loaded draws nothing"
    );
}
