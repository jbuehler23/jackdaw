//! The 2D world view's grid: one `InfiniteGrid` per 2D panel, on that panel's
//! own render layer, shown while it shows a 2D world, spaced by its snap grid
//! and switched with the editor's one grid overlay.

use crate::sprite_gizmo::{settle, world_app};
use crate::util::OperatorResultExt as _;
use crate::world_guides::mixed_document;

use bevy::camera::visibility::RenderLayers;
use bevy::dev_tools::infinite_grid::{InfiniteGrid, InfiniteGridSettings};
use bevy::prelude::*;
use jackdaw::canvas_grid::{CANVAS_GRID_MAJOR_INTERVAL, CanvasGrid};
use jackdaw::viewport::{MainViewportCamera, ViewportGrid};
use jackdaw::viewport_2d::{Ui2dView, Viewport2dPanelHost};
use jackdaw::viewport_overlays::OverlaySettings;
use jackdaw_api::prelude::*;

struct Drawn {
    host: Entity,
    layers: RenderLayers,
    settings: InfiniteGridSettings,
    transform: Transform,
    editor: bool,
}

/// The 2D grids currently shown.
fn shown_grids(app: &mut App) -> Vec<Drawn> {
    app.world_mut()
        .query_filtered::<(
            &CanvasGrid,
            &RenderLayers,
            &InfiniteGridSettings,
            &Transform,
            &Visibility,
            Has<jackdaw::EditorEntity>,
        ), With<InfiniteGrid>>()
        .iter(app.world())
        .filter(|(.., visibility, _)| **visibility != Visibility::Hidden)
        .map(
            |(grid, render_layers, settings, transform, _, editor)| Drawn {
                host: grid.host,
                layers: render_layers.clone(),
                settings: *settings,
                transform: *transform,
                editor,
            },
        )
        .collect()
}

fn host(app: &App, panel: Entity) -> &Viewport2dPanelHost {
    app.world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
}

fn camera_layers(app: &App, panel: Entity) -> RenderLayers {
    app.world()
        .get::<RenderLayers>(host(app, panel).camera)
        .cloned()
        .unwrap_or_default()
}

fn set_view(app: &mut App, panel: Entity, edit: impl FnOnce(Ui2dView) -> Ui2dView) {
    let mut host = app
        .world_mut()
        .get_mut::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent");
    let view = edit(host.view);
    host.set_view(view);
    settle(app);
}

fn toggle_grid(app: &mut App) {
    app.world_mut()
        .operator("view.toggle_grid")
        .call()
        .expect("view.toggle_grid dispatches")
        .assert_finished();
    settle(app);
}

fn grid_row(app: &mut App) -> Option<String> {
    app.world_mut()
        .operator("menu.open")
        .param("name", jackdaw::toolbar_2d::CANVAS_VIEW_MENU)
        .call()
        .expect("menu.open dispatches")
        .assert_finished();
    app.update();
    app.world_mut()
        .query::<&jackdaw_widgets::menu_bar::MenuBarItem>()
        .iter(app.world())
        .find(|item| item.label == jackdaw::toolbar_2d::CANVAS_VIEW_MENU)
        .expect("the 2D panel's toolbar carries a canvas view menu")
        .actions
        .iter()
        .find(|(_, label)| label == "Show Grid")
        .map(|(action, _)| action.clone())
}

fn hidden_3d_grids(app: &mut App) -> Vec<bool> {
    let grids: Vec<Entity> = app
        .world_mut()
        .query::<&ViewportGrid>()
        .iter(app.world())
        .map(|grid| grid.0)
        .collect();
    grids
        .into_iter()
        .map(|grid| app.world().get::<Visibility>(grid) == Some(&Visibility::Hidden))
        .collect()
}

#[test]
fn only_a_world_panel_shows_a_grid_on_its_own_layer() {
    let (mut app, scene) = world_app();
    let (_, ui_panel, world_panel) = mixed_document(&mut app, &scene);
    settle(&mut app);

    let drawn = shown_grids(&mut app);
    assert_eq!(drawn.len(), 1, "one grid shown, for the world panel");
    let grid = &drawn[0];
    assert_eq!(grid.host, world_panel);
    assert_eq!(
        app.world()
            .get::<CanvasGrid>(host(&app, world_panel).grid)
            .map(|grid| grid.host),
        Some(world_panel),
        "the host holds its own grid"
    );
    assert!(grid.editor, "the grid is an editor entity, never saved");
    assert!(
        grid.layers.intersects(&camera_layers(&app, world_panel)),
        "the world panel draws it"
    );
    assert!(
        !grid.layers.iter().any(|layer| layer == 0),
        "off the shared scene layer: {:?}",
        grid.layers
    );
    assert!(
        !grid.layers.intersects(&camera_layers(&app, ui_panel)),
        "the UI panel beside it does not"
    );
    let viewports: Vec<RenderLayers> = app
        .world_mut()
        .query_filtered::<Option<&RenderLayers>, With<MainViewportCamera>>()
        .iter(app.world())
        .map(|layers| layers.cloned().unwrap_or_default())
        .collect();
    for layers in viewports {
        assert!(
            !grid.layers.intersects(&layers),
            "no 3D viewport draws it: {layers:?}"
        );
    }
}

#[test]
fn the_grid_lies_flat_inside_the_panel_camera_depth_range() {
    let (mut app, scene) = world_app();
    let drawn = shown_grids(&mut app);
    assert_eq!(drawn.len(), 1);
    let transform = drawn[0].transform;
    let camera = host(&app, scene.panel).camera;
    let camera_z = app
        .world()
        .get::<Transform>(camera)
        .expect("the panel camera is placed")
        .translation
        .z;
    let Some(Projection::Orthographic(ortho)) = app.world().get::<Projection>(camera) else {
        panic!("the panel camera is orthographic");
    };
    let default_2d = OrthographicProjection::default_2d();
    assert_eq!(
        (ortho.near, ortho.far),
        (default_2d.near, default_2d.far),
        "the panel sees the depth range a game's 2D camera does"
    );
    let z = transform.translation.z;
    assert!(
        z > camera_z - ortho.far && z < camera_z - ortho.near,
        "{z} inside the camera's range"
    );
    assert!(
        z < -jackdaw_scene_types::Y_SORT_SPAN,
        "{z} behind a sprite at z 0 and its y-sort offset"
    );
    assert!(
        (transform.up().as_vec3() - Vec3::Z).length() < 1e-5,
        "the plane faces the 2D camera"
    );
    assert_eq!(
        drawn[0].settings.major_line_interval,
        CANVAS_GRID_MAJOR_INTERVAL
    );
}

#[test]
fn the_grid_spacing_follows_the_panel_snap_grid() {
    let (mut app, scene) = world_app();
    let scale = |app: &mut App| shown_grids(app)[0].settings.scale;
    assert_eq!(scale(&mut app), 1.0 / 8.0, "minor lines at the 8 unit grid");

    app.world_mut()
        .operator("viewport2d.grid")
        .param("size", 16.0)
        .param("panel", scene.panel.to_bits() as i64)
        .call()
        .expect("viewport2d.grid dispatches")
        .assert_finished();
    settle(&mut app);
    assert_eq!(scale(&mut app), 1.0 / 16.0, "the grid step moves the lines");

    set_view(&mut app, scene.panel, |view| Ui2dView {
        grid: 8.0,
        zoom: 0.25,
        ..view
    });
    assert_eq!(
        scale(&mut app),
        1.0 / (8.0 * CANVAS_GRID_MAJOR_INTERVAL as f32),
        "minor lines 2 pixels apart step up to the major spacing"
    );
}

#[test]
fn one_grid_toggle_hides_every_grid() {
    let (mut app, _scene) = world_app();
    assert!(app.world().resource::<OverlaySettings>().show_grid);
    assert_eq!(
        grid_row(&mut app).as_deref(),
        Some("[x]op:view.toggle_grid")
    );
    assert_eq!(shown_grids(&mut app).len(), 1);

    toggle_grid(&mut app);
    assert!(shown_grids(&mut app).is_empty(), "the 2D grid hides");
    assert!(
        hidden_3d_grids(&mut app).iter().all(|hidden| *hidden),
        "and so do the 3D grids"
    );
    assert_eq!(
        grid_row(&mut app).as_deref(),
        Some("[ ]op:view.toggle_grid")
    );

    toggle_grid(&mut app);
    assert_eq!(shown_grids(&mut app).len(), 1, "shown again");
}
