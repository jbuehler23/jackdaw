//! Switching workspaces through `workspace.switch`, from the remote or from
//! the title bar dropdown.

use crate::util;

use bevy::camera::{NormalizedRenderTarget, RenderTarget};
use bevy::picking::{
    backend::HitData,
    events::{Pointer, PointerClick},
    pointer::{Location, PointerButton, PointerId},
};
use bevy::prelude::*;
use bevy::ui_widgets::Activate;
use bevy::window::{PrimaryWindow, WindowRef};
use jackdaw::draw_brush::ActivateDrawBrushModalOp;
use jackdaw::workspace_dropdown::{WorkspaceDropdownTrigger, workspace_dropdown_trigger};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;
use jackdaw_api_internal::operator::OperatorWarnings;
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_feathers::tooltip::Tooltip;
use jackdaw_panels::WorkspaceRegistry;
use jackdaw_panels::workspace::WorkspaceTab;
use jackdaw_scene_types::PropertyValue;

const SWITCH: &str = "workspace.switch";

fn app() -> App {
    let mut app = util::editor_test_app();
    settle(&mut app);
    app
}

fn settle(app: &mut App) {
    for _ in 0..3 {
        app.update();
    }
}

fn active(app: &App) -> Option<String> {
    app.world().resource::<WorkspaceRegistry>().active.clone()
}

fn switch(app: &mut App, param: &'static str, value: &str) -> OperatorResult {
    app.world_mut()
        .get_resource_or_init::<OperatorWarnings>()
        .0
        .clear();
    let result = app
        .world_mut()
        .operator(SWITCH)
        .param(param, PropertyValue::from(value.to_string()))
        .call()
        .expect("dispatches");
    settle(app);
    result
}

#[test]
fn workspace_switch_takes_an_id_or_a_name() {
    let mut app = app();
    assert_eq!(switch(&mut app, "id", "2d"), OperatorResult::Finished);
    assert_eq!(active(&app).as_deref(), Some("2d"));
    assert_eq!(
        switch(&mut app, "name", "Animation"),
        OperatorResult::Finished
    );
    assert_eq!(active(&app).as_deref(), Some("animation"));
    assert_eq!(
        switch(&mut app, "id", "animation"),
        OperatorResult::Finished
    );
    assert_eq!(active(&app).as_deref(), Some("animation"));

    assert_eq!(
        switch(&mut app, "name", "Sculpt"),
        OperatorResult::Cancelled
    );
    assert_eq!(active(&app).as_deref(), Some("animation"));
    let told = app.world().resource::<OperatorWarnings>().0.join("\n");
    assert!(
        told.contains("no workspace `Sculpt`") && told.contains("2d (2D)"),
        "{told}"
    );
}

fn click(app: &mut App, entity: Entity) {
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(app.world())
        .expect("a primary window");
    let target: NormalizedRenderTarget = RenderTarget::Window(WindowRef::Primary)
        .normalize(Some(window))
        .expect("the primary window normalizes");
    app.world_mut().trigger(PointerClick {
        entity,
        pointer: Pointer::new(
            PointerId::Mouse,
            Location {
                target,
                position: Vec2::ZERO,
            },
        ),
        button: PointerButton::Primary,
        hit: HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
        duration: core::time::Duration::ZERO,
        count: 1,
    });
    settle(app);
}

fn modal_running(app: &mut App) -> bool {
    app.world_mut()
        .query::<&ActiveModalOperator>()
        .iter(app.world())
        .next()
        .is_some()
}

#[test]
fn the_title_bar_dropdown_switches_and_keeps_a_running_modal() {
    let mut app = app();
    app.world_mut()
        .spawn(workspace_dropdown_trigger(default(), default()));
    settle(&mut app);
    let trigger = app
        .world_mut()
        .query_filtered::<Entity, With<WorkspaceDropdownTrigger>>()
        .single(app.world())
        .expect("the dropdown trigger");
    click(&mut app, trigger);
    let row = app
        .world_mut()
        .query::<(Entity, &WorkspaceTab)>()
        .iter(app.world())
        .find(|(_, tab)| tab.workspace_id == "2d")
        .map(|(entity, _)| entity)
        .expect("a row for the 2D workspace");
    assert!(app.world().get::<ButtonOperatorCall>(row).is_none());
    assert!(
        app.world().get::<Tooltip>(row).is_none(),
        "no tooltip per row"
    );

    let result = app
        .world_mut()
        .operator(ActivateDrawBrushModalOp::ID)
        .call()
        .expect("dispatches");
    assert_eq!(result, OperatorResult::Running);
    app.world_mut().trigger(Activate { entity: row });
    settle(&mut app);
    assert_eq!(active(&app).as_deref(), Some("2d"));
    assert!(modal_running(&mut app), "switching keeps the running modal");
}
