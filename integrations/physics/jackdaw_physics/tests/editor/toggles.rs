//! The Physics module's controls in the editor's own surfaces: the Rigid
//! Body and Collider toggles in the inspector's Physics category, the
//! collider overlay, and the physics tool's toolbar button.

use crate::util;

use avian3d::prelude::RigidBody;
use bevy::feathers::controls::ButtonVariant as ToolButtonVariant;
use bevy::prelude::*;
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_api::tools::ToolStatus;
use jackdaw_api::viewport::ViewportOverlay;
use jackdaw_avian_integration::{AvianCollider, ImpliedStaticBody};
use jackdaw_feathers::button::{ButtonOperatorCall, ButtonVariant};
use jackdaw_physics::{PhysicsOverlayConfig, STATUS};
use jackdaw_scene_types::Brush;

fn call(app: &mut App, id: &str, params: &[(&'static str, &str)]) {
    let mut call = app.world_mut().operator(id.to_string());
    for (key, value) in params {
        call = call.param(*key, value.to_string());
    }
    let result = call.call().unwrap_or_else(|err| panic!("{id}: {err}"));
    assert_eq!(result, OperatorResult::Finished, "{id} reported {result:?}");
    util::settle(app);
}

/// A cube brush, selected, with the inspector open on its Physics category.
fn brush_in_the_physics_category(app: &mut App) -> Entity {
    call(app, "entity.add.cube", &[]);
    let brush = app
        .world_mut()
        .query_filtered::<Entity, With<Brush>>()
        .single(app.world())
        .expect("one brush");
    app.world_mut().resource_mut::<Selection>().entities = vec![brush];
    util::settle(app);
    call(app, "inspector.category", &[("category", "physics")]);
    brush
}

/// The button whose text reads `label`.
fn button_reading(app: &mut App, label: &str) -> Option<Entity> {
    let mut texts = app.world_mut().query::<(&Text, &ChildOf)>();
    let parents: Vec<Entity> = texts
        .iter(app.world())
        .filter(|(text, _)| text.0 == label)
        .map(|(_, parent)| parent.parent())
        .collect();
    let mut buttons = app.world_mut().query::<(Entity, &ButtonVariant)>();
    parents.into_iter().find_map(|candidate| {
        let mut at = candidate;
        for _ in 0..3 {
            if buttons.get(app.world(), at).is_ok() {
                return Some(at);
            }
            at = app.world().get::<ChildOf>(at)?.parent();
        }
        None
    })
}

fn click(app: &mut App, entity: Entity) {
    let at = util::centre_of(app, entity);
    util::run(
        app,
        &format!("input.pointer x={} y={} action=click", at.x, at.y),
    );
}

/// Activate a button the way a click or Enter on it does.
fn activate(app: &mut App, entity: Entity) {
    app.world_mut()
        .trigger(bevy::ui_widgets::Activate { entity });
    util::settle(app);
}

#[test]
fn the_physics_category_toggles_take_the_body_off_and_put_it_back() {
    let mut app = util::editor_with_viewport((1600, 1000));
    let brush = brush_in_the_physics_category(&mut app);
    assert!(
        app.world().get::<RigidBody>(brush).is_some(),
        "a new brush starts with a body"
    );

    let on = button_reading(&mut app, "Rigid Body  x").expect("the Rigid Body toggle reads on");
    activate(&mut app, on);
    assert!(
        app.world().get::<ImpliedStaticBody>(brush).is_some(),
        "clicking the lit toggle removes the authored body, leaving the collider an implied static one"
    );
    assert!(
        app.world().get::<AvianCollider>(brush).is_some(),
        "and leaves the collider"
    );

    let off = button_reading(&mut app, "Rigid Body").expect("the toggle now reads off");
    activate(&mut app, off);
    assert!(
        app.world().get::<ImpliedStaticBody>(brush).is_none(),
        "clicking it again authors a body"
    );
    assert!(
        button_reading(&mut app, "Collider  x").is_some(),
        "the Collider toggle reads on throughout"
    );
}

#[test]
fn the_collider_overlay_toggle_flips_the_overlay_and_its_menu_row() {
    let mut app = util::editor_app();
    let shown = |app: &mut App| {
        app.world_mut()
            .query::<&ViewportOverlay>()
            .iter(app.world())
            .find(|overlay| overlay.label == "Colliders")
            .map(|overlay| overlay.shown)
            .expect("the module lists a Colliders overlay")
    };
    assert!(!shown(&mut app));
    call(&mut app, "view.toggle_collider_gizmos", &[]);
    assert!(
        app.world()
            .resource::<PhysicsOverlayConfig>()
            .show_colliders
    );
    assert!(shown(&mut app), "the menu row follows the overlay");
}

#[test]
fn the_toolbar_button_starts_the_physics_tool_and_lights_while_it_runs() {
    let mut app = util::editor_with_viewport((1600, 1000));
    let button = app
        .world_mut()
        .query::<(Entity, &ButtonOperatorCall)>()
        .iter(app.world())
        .find(|(_, call)| call.id == "physics.activate")
        .map(|(entity, _)| entity)
        .expect("the toolbar carries the physics tool");

    click(&mut app, button);
    assert_eq!(
        app.world().resource::<ToolStatus>().0.as_deref(),
        Some(STATUS),
        "the tool runs and says how to end it"
    );
    assert_eq!(
        app.world().get::<ToolButtonVariant>(button),
        Some(&ToolButtonVariant::Primary),
        "its button is lit while it runs"
    );

    util::run(&mut app, "input.key key=Escape");
    assert_eq!(app.world().resource::<ToolStatus>().0, None);
    assert_eq!(
        app.world().get::<ToolButtonVariant>(button),
        Some(&ToolButtonVariant::Plain),
        "and goes dark once it ends"
    );
}
