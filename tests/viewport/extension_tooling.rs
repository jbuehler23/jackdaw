//! An extension's tooling in the editor's own surfaces: a toolbar button
//! that starts its modal tool, a status line while the tool runs, a toggle
//! in an inspector category's header, a row in the viewport's overlays, and
//! a component every new brush starts with. Disabling the extension takes
//! all of them away.

use bevy::feathers::controls::ButtonVariant as ToolButtonVariant;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, WindowResolution};
use jackdaw::selection::Selection;
use jackdaw::test_input::SyntheticInput;
use jackdaw_api::document::SceneDocument;
use jackdaw_api::prelude::*;
use jackdaw_api::tools::{ModalInputs, ToolStatus, ToolbarButton};
use jackdaw_api::viewport::ViewportOverlay;
use jackdaw_api_internal::lifecycle::disable_extension;
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_scene_types::Brush;

use crate::util;
use crate::util::OperatorResultExt as _;

const EXTENSION: &str = "test.tooling";

/// What the test extension adds to every new brush.
#[derive(Component, Reflect, Default, Clone, PartialEq, Debug)]
#[reflect(Component, Default)]
struct Ballast {
    kilograms: f32,
}

/// Whether the test overlay is drawn.
#[derive(Resource, Default)]
struct BallastOverlay(bool);

#[derive(Default)]
struct ToolingExtension;

impl JackdawExtension for ToolingExtension {
    fn id(&self) -> String {
        EXTENSION.to_string()
    }

    fn register(&self, ctx: &mut ExtensionRegistrar<'_>) {
        ctx.init_resource::<BallastOverlay>();
        ctx.register_operator::<BallastToolOp>()
            .register_operator::<BallastOverlayOp>()
            .register_toolbar_button::<BallastToolOp>(Icon::Anchor)
            .register_inspector_chip("components", "Ballast", Ballast::type_path())
            .register_viewport_overlay::<BallastOverlayOp>("Ballast", false)
            .register_brush_default(Ballast { kilograms: 40.0 });
    }
}

#[operator(
    id = "test.ballast_tool",
    label = "Ballast Tool",
    modal = true,
    allows_undo = false
)]
fn ballast_tool(
    _: In<OperatorParameters>,
    keys: ModalInputs,
    active: ActiveModalQuery,
    mut status: ResMut<ToolStatus>,
) -> OperatorResult {
    if !active.is_modal_running() {
        status.0 = Some("Ballast | Esc to stop".to_string());
    }
    if keys.cancel() {
        return OperatorResult::Cancelled;
    }
    OperatorResult::Running
}

#[operator(
    id = "test.ballast_overlay",
    label = "Toggle Ballast Overlay",
    allows_undo = false
)]
fn ballast_overlay(
    _: In<OperatorParameters>,
    mut overlay: ResMut<BallastOverlay>,
    mut rows: Query<&mut ViewportOverlay>,
) -> OperatorResult {
    overlay.0 = !overlay.0;
    for mut row in &mut rows {
        if row.operator_id == BallastOverlayOp::ID {
            row.shown = overlay.0;
        }
    }
    OperatorResult::Finished
}

fn settle(app: &mut App) {
    for _ in 0..8 {
        app.update();
    }
}

fn run(app: &mut App, clause: &str) {
    jackdaw::boot_ops::run_op_clause(app.world_mut(), clause)
        .unwrap_or_else(|error| panic!("{clause}: {error}"))
        .assert_finished();
    for _ in 0..200 {
        app.update();
        if app.world().resource::<SyntheticInput>().is_idle() {
            break;
        }
    }
    settle(app);
}

fn editor() -> App {
    let mut app = util::headless_app();
    app.register_type::<Ballast>();
    util::register_and_enable_extension::<ToolingExtension>(&mut app);
    {
        let mut windows = app
            .world_mut()
            .query_filtered::<&mut Window, With<PrimaryWindow>>();
        let mut window = windows
            .single_mut(app.world_mut())
            .expect("headless apps still have a primary window");
        window.resolution = WindowResolution::new(1600, 1000);
    }
    app.world_mut()
        .resource_mut::<NextState<AppState>>()
        .set(AppState::Editor);
    settle(&mut app);
    app
}

fn toolbar_button(app: &mut App) -> Option<Entity> {
    app.world_mut()
        .query::<(Entity, &ButtonOperatorCall)>()
        .iter(app.world())
        .find(|(_, call)| call.id == BallastToolOp::ID)
        .map(|(entity, _)| entity)
}

fn new_brush(app: &mut App) -> Entity {
    app.world_mut()
        .operator("entity.add.cube")
        .call()
        .expect("dispatch")
        .assert_finished();
    settle(app);
    app.world_mut()
        .query_filtered::<Entity, With<Brush>>()
        .single(app.world())
        .expect("one brush")
}

#[test]
fn a_toolbar_button_starts_the_tool_and_lights_while_it_runs() {
    let mut app = editor();
    let button = toolbar_button(&mut app).expect("the extension's button is on the toolbar");
    let transform = app
        .world()
        .get::<bevy::ui::UiGlobalTransform>(button)
        .expect("placed");
    let computed = app.world().get::<ComputedNode>(button).expect("laid out");
    let at = transform.translation * computed.inverse_scale_factor();

    run(
        &mut app,
        &format!("input.pointer x={} y={} action=click", at.x, at.y),
    );
    assert_eq!(
        app.world().resource::<ToolStatus>().0.as_deref(),
        Some("Ballast | Esc to stop"),
        "the click starts the tool, which says how to end it"
    );
    assert_eq!(
        app.world().get::<ToolButtonVariant>(button),
        Some(&ToolButtonVariant::Primary)
    );

    run(&mut app, "input.key key=Escape");
    assert_eq!(
        app.world().resource::<ToolStatus>().0,
        None,
        "the status line goes with the tool"
    );
    assert_eq!(
        app.world().get::<ToolButtonVariant>(button),
        Some(&ToolButtonVariant::Plain)
    );
}

#[test]
fn a_new_brush_starts_with_the_extensions_component_in_the_document() {
    let mut app = editor();
    let brush = new_brush(&mut app);
    assert_eq!(
        app.world().get::<Ballast>(brush),
        Some(&Ballast { kilograms: 40.0 })
    );
    assert!(
        SceneDocument::authored_components(app.world(), brush)
            .iter()
            .any(|path| path == Ballast::type_path()),
        "the component is authored, not only live"
    );
}

#[test]
fn an_inspector_chip_removes_its_component_from_the_selected_entity() {
    let mut app = editor();
    let brush = new_brush(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![brush];
    settle(&mut app);
    app.world_mut()
        .operator("inspector.category")
        .param("category", "components".to_string())
        .call()
        .expect("dispatch")
        .assert_finished();
    settle(&mut app);

    let chip_text = |app: &mut App| {
        app.world_mut()
            .query::<(&Text, &ChildOf)>()
            .iter(app.world())
            .find(|(text, _)| text.0 == "Ballast  x")
            .map(|(_, parent)| parent.parent())
    };
    let mut chip = chip_text(&mut app);
    for _ in 0..60 {
        if chip.is_some() {
            break;
        }
        app.update();
        chip = chip_text(&mut app);
    }
    let chip = chip.expect("the chip reads on for a brush carrying the component");
    let mut button = chip;
    while app
        .world()
        .get::<jackdaw_feathers::button::ButtonVariant>(button)
        .is_none()
    {
        button = app
            .world()
            .get::<ChildOf>(button)
            .map(ChildOf::parent)
            .expect("the chip's text sits in its button");
    }
    app.world_mut()
        .trigger(bevy::ui_widgets::Activate { entity: button });
    for _ in 0..60 {
        if app.world().get::<Ballast>(brush).is_none() {
            break;
        }
        app.update();
    }

    assert!(app.world().get::<Ballast>(brush).is_none());
    assert!(
        !SceneDocument::authored_components(app.world(), brush)
            .iter()
            .any(|path| path == Ballast::type_path()),
        "the removal reaches the document"
    );
}

#[test]
fn a_viewport_overlay_row_follows_its_operator() {
    let mut app = editor();
    let shown = |app: &mut App| {
        app.world_mut()
            .query::<&ViewportOverlay>()
            .iter(app.world())
            .find(|row| row.label == "Ballast")
            .map(|row| row.shown)
            .expect("the overlay is listed")
    };
    assert!(!shown(&mut app));
    app.world_mut()
        .operator(BallastOverlayOp::ID)
        .call()
        .expect("dispatch")
        .assert_finished();
    assert!(shown(&mut app));
    assert!(app.world().resource::<BallastOverlay>().0);
}

#[test]
fn disabling_the_extension_takes_its_tooling_away() {
    let mut app = editor();
    assert!(toolbar_button(&mut app).is_some());
    assert!(disable_extension(app.world_mut(), EXTENSION));
    settle(&mut app);

    assert!(
        toolbar_button(&mut app).is_none(),
        "the button leaves the toolbar"
    );
    assert_eq!(
        app.world_mut()
            .query::<&ToolbarButton>()
            .iter(app.world())
            .count(),
        0
    );
    assert_eq!(
        app.world_mut()
            .query::<&ViewportOverlay>()
            .iter(app.world())
            .count(),
        0,
        "the overlay row goes"
    );
    let brush = new_brush(&mut app);
    assert!(
        app.world().get::<Ballast>(brush).is_none(),
        "new brushes no longer start with the component"
    );
}
