//! The project's pixels per unit and the unit 2D world lengths are shown in:
//! kept in the project's settings, read by the rulers, the grid and the zoom,
//! and the step a pixel snap or an arrow-key nudge takes.

use std::path::Path;

use bevy::prelude::*;
use jackdaw::{
    active_tool::ActiveTool,
    project::{ProjectConfig, ProjectRoot},
    project_settings::settings_path,
    ui_stage::CanvasAxis,
    viewport_2d::{CanvasRuler, Viewport2dPanelHost, world_to_target_pixel},
    world_units::{LengthUnit, WorldUnits},
};
use jackdaw_api::prelude::*;

use crate::sprite_gizmo::{drag, near, select, settle, transform, world_app};
use crate::sprite_snap::snap_only;
use crate::util::{self, OperatorResultExt as _};

fn set_ppu(app: &mut App, value: f64) {
    app.world_mut()
        .operator("project.set_pixels_per_unit")
        .param("value", value)
        .call()
        .expect("project.set_pixels_per_unit dispatches")
        .assert_finished();
    settle_quick(app);
}

fn set_unit(app: &mut App, unit: &str) {
    app.world_mut()
        .operator("project.set_length_unit")
        .param("unit", unit.to_string())
        .call()
        .expect("project.set_length_unit dispatches")
        .assert_finished();
    settle_quick(app);
}

fn settle_quick(app: &mut App) {
    for _ in 0..3 {
        app.update();
    }
}

fn open(app: &mut App, root: &Path) {
    app.world_mut().insert_resource(ProjectRoot {
        root: root.to_path_buf(),
        config: ProjectConfig::default(),
    });
    app.update();
}

#[test]
fn the_units_round_trip_through_the_settings_beside_other_keys() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join(".jackdaw")).expect("the settings folder");
    std::fs::write(
        settings_path(root),
        "{ \"auto_build\": true, \"canvas\": { \"grid\": false } }",
    )
    .expect("settings");

    let mut app = util::editor_test_app();
    open(&mut app, root);
    assert_eq!(*app.world().resource::<WorldUnits>(), WorldUnits::default());
    set_ppu(&mut app, 16.0);
    set_unit(&mut app, "units");

    let text = std::fs::read_to_string(settings_path(root)).expect("the settings");
    let json: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert_eq!(json["auto_build"], true, "other keys stay: {text}");
    assert_eq!(json["canvas"]["grid"], false, "other sections stay: {text}");
    assert_eq!(json["units"]["pixels_per_unit"], 16.0, "{text}");
    assert_eq!(json["units"]["display"], "units", "{text}");

    let mut reopened = util::editor_test_app();
    open(&mut reopened, root);
    assert_eq!(
        *reopened.world().resource::<WorldUnits>(),
        WorldUnits {
            pixels_per_unit: 16.0,
            display: LengthUnit::Units,
        }
    );
}

#[test]
fn a_ppu_of_zero_or_less_is_refused() {
    let mut app = util::editor_test_app();
    for value in [0.0, -2.0, f64::INFINITY] {
        let result = app
            .world_mut()
            .operator("project.set_pixels_per_unit")
            .param("value", value)
            .call()
            .expect("project.set_pixels_per_unit dispatches");
        result.assert_cancelled();
    }
    assert_eq!(app.world().resource::<WorldUnits>().pixels_per_unit, 1.0);
}

/// Each label of `axis`'s ruler: its figure and where it sits along the ruler.
fn labels(app: &mut App, panel: Entity, axis: CanvasAxis) -> Vec<(f32, f32)> {
    let ruler = app
        .world_mut()
        .query::<(Entity, &CanvasRuler)>()
        .iter(app.world())
        .find(|(_, ruler)| ruler.host == panel && ruler.axis == axis)
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
            let Val::Px(edge) = node.left else {
                return None;
            };
            Some((figure, edge - 2.0))
        })
        .collect();
    labels.sort_by(|a, b| a.1.total_cmp(&b.1));
    labels
}

/// Every label of the top ruler sits where world x = `figure / shown_per_unit`
/// shows.
fn assert_top_ruler_reads(app: &mut App, panel: Entity, shown_per_unit: f32) -> Vec<f32> {
    let host = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("the panel has a host");
    let (view, target) = (host.view, host.target_size);
    let top = labels(app, panel, CanvasAxis::Vertical);
    assert!(top.len() >= 3, "the ruler labels the view: {top:?}");
    for (figure, at) in &top {
        let shows = world_to_target_pixel(view, target, Vec2::new(figure / shown_per_unit, 0.0));
        assert!(
            (shows.x - at).abs() < 1e-2,
            "label {figure} at {at}, but world x {} shows at {}",
            figure / shown_per_unit,
            shows.x
        );
    }
    top.into_iter().map(|(figure, _)| figure).collect()
}

#[test]
fn the_rulers_read_pixels_or_units_at_the_project_ppu() {
    let (mut app, scene) = world_app();
    set_ppu(&mut app, 16.0);
    settle(&mut app);
    let pixels = assert_top_ruler_reads(&mut app, scene.panel, 16.0);

    set_unit(&mut app, "units");
    settle(&mut app);
    let units = assert_top_ruler_reads(&mut app, scene.panel, 1.0);
    assert_ne!(pixels, units, "the figures change with the unit");
    assert!(
        pixels.iter().copied().fold(0.0, f32::max) > units.iter().copied().fold(0.0, f32::max),
        "a pixel reading counts further: {pixels:?} vs {units:?}"
    );
}

/// The heading of the panel's canvas view menu that starts with `title`.
fn view_heading(app: &App, panel: Entity, title: &str) -> String {
    jackdaw::viewport_2d::view_menu_rows(app.world(), panel)
        .into_iter()
        .find_map(|(action, _)| {
            action
                .strip_prefix(&format!("##{title}: "))
                .map(str::to_string)
        })
        .unwrap_or_else(|| panic!("the canvas view menu has a {title} heading"))
}

fn grid_readout(app: &mut App, panel: Entity) -> String {
    view_heading(app, panel, "Grid")
}

fn zoom_readout(app: &mut App, panel: Entity) -> String {
    view_heading(app, panel, "Zoom")
}

/// Run the panel's canvas view row that steps the grid finer or coarser.
fn click_grid_step(app: &mut App, panel: Entity, steps: i32) {
    let label = if steps < 0 { "Finer" } else { "Coarser" };
    let action = jackdaw::viewport_2d::view_menu_rows(app.world(), panel)
        .into_iter()
        .find(|(_, row)| row == label)
        .map(|(action, _)| action)
        .expect("the canvas view menu steps the grid");
    let call = jackdaw_feathers::button::ButtonOperatorCall::try_from(action.as_str())
        .expect("the row calls an operator");
    let mut dispatch = app.world_mut().operator(call.id.to_string());
    for (key, value) in &call.params {
        dispatch = dispatch.param(key.to_string(), value.clone());
    }
    dispatch
        .call()
        .expect("the row dispatches")
        .assert_finished();
    settle_quick(app);
}

#[test]
fn the_grid_readout_names_pixels_or_units() {
    let (mut app, scene) = world_app();
    settle(&mut app);
    assert_eq!(
        grid_readout(&mut app, scene.panel),
        "8 px",
        "ppu 1 reads as px"
    );

    set_ppu(&mut app, 16.0);
    assert_eq!(grid_readout(&mut app, scene.panel), "128 px");

    set_unit(&mut app, "units");
    assert_eq!(grid_readout(&mut app, scene.panel), "8 u");
}

#[test]
fn a_pixel_snap_lands_on_sixteenths_at_ppu_sixteen() {
    let (mut app, scene) = world_app();
    set_ppu(&mut app, 16.0);
    snap_only(&mut app, &["pixel"]);
    select(&mut app, Vec2::ZERO, ActiveTool::Translate);

    drag(&mut app, Vec2::new(2.0, 3.0), Vec2::new(14.27, -1.71), "");
    let at = transform(&app, scene.tile).translation;
    assert!(
        near(at, Vec3::new(12.25, -4.6875, 0.0)),
        "the lower-left corner rests on a sixteenth of a unit: {at:?}"
    );
}

fn view(app: &App, panel: Entity) -> jackdaw::viewport_2d::Ui2dView {
    app.world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("the panel has a host")
        .view
}

fn panel_op(app: &mut App, id: &'static str, panel: Entity, key: &'static str, value: f64) {
    app.world_mut()
        .operator(id)
        .param(key, value)
        .param("panel", panel.to_bits() as i64)
        .call()
        .unwrap_or_else(|err| panic!("{id} dispatches: {err}"))
        .assert_finished();
    settle_quick(app);
}

#[test]
fn the_zoom_readout_counts_screen_pixels_per_sprite_pixel() {
    let (mut app, scene) = world_app();
    set_ppu(&mut app, 16.0);
    panel_op(&mut app, "viewport2d.zoom", scene.panel, "zoom", 60.0);
    app.world_mut()
        .operator("viewport2d.pixel_perfect")
        .param("panel", scene.panel.to_bits() as i64)
        .call()
        .expect("viewport2d.pixel_perfect dispatches")
        .assert_finished();
    settle_quick(&mut app);
    assert_eq!(
        view(&app, scene.panel).zoom,
        64.0,
        "four screen pixels a texel"
    );
    assert_eq!(zoom_readout(&mut app, scene.panel), "400%");
}

#[test]
fn the_world_grid_is_set_in_sprite_pixels_down_to_one() {
    let (mut app, scene) = world_app();
    set_ppu(&mut app, 16.0);
    panel_op(&mut app, "viewport2d.grid", scene.panel, "size", 16.0);
    assert_eq!(view(&app, scene.panel).grid, 1.0, "16 px is one world unit");
    assert_eq!(grid_readout(&mut app, scene.panel), "16 px");

    click_grid_step(&mut app, scene.panel, -1);
    assert_eq!(view(&app, scene.panel).grid, 0.5);
    assert_eq!(grid_readout(&mut app, scene.panel), "8 px");

    panel_op(&mut app, "viewport2d.grid", scene.panel, "size", 1.0);
    assert_eq!(view(&app, scene.panel).grid, 1.0 / 16.0);
    assert_eq!(grid_readout(&mut app, scene.panel), "1 px");
    click_grid_step(&mut app, scene.panel, -1);
    assert_eq!(
        grid_readout(&mut app, scene.panel),
        "1 px",
        "one pixel is the finest"
    );
    click_grid_step(&mut app, scene.panel, 1);
    assert_eq!(grid_readout(&mut app, scene.panel), "2 px");
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
fn an_arrow_nudge_steps_one_sprite_pixel_and_shift_one_grid_step() {
    let (mut app, scene) = world_app();
    set_ppu(&mut app, 16.0);
    select(&mut app, Vec2::ZERO, ActiveTool::Select);
    nudge(&mut app, "transform.nudge_x_pos");
    assert!(
        near(
            transform(&app, scene.tile).translation,
            Vec3::new(1.0 / 16.0, 0.0, 0.0)
        ),
        "{:?}",
        transform(&app, scene.tile).translation
    );

    panel_op(&mut app, "viewport2d.grid", scene.panel, "size", 32.0);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::ShiftLeft);
    nudge(&mut app, "transform.nudge_z_neg");
    assert!(
        near(
            transform(&app, scene.tile).translation,
            Vec3::new(1.0 / 16.0, 2.0, 0.0)
        ),
        "a 32 px grid step is two units: {:?}",
        transform(&app, scene.tile).translation
    );
}

#[test]
fn closing_the_project_restores_the_default_units() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = util::editor_test_app();
    open(&mut app, dir.path());
    set_ppu(&mut app, 16.0);
    app.world_mut().remove_resource::<ProjectRoot>();
    settle_quick(&mut app);
    assert_eq!(*app.world().resource::<WorldUnits>(), WorldUnits::default());

    let other = tempfile::tempdir().expect("tempdir");
    open(&mut app, dir.path());
    assert_eq!(app.world().resource::<WorldUnits>().pixels_per_unit, 16.0);
    open(&mut app, other.path());
    assert_eq!(*app.world().resource::<WorldUnits>(), WorldUnits::default());
}
