//! Modal-operator coverage: every operator declared `modal = true` is
//! dispatched, must answer `Running` or `Cancelled` (never `Finished`), and
//! cancelling must roll back to the pre-dispatch snapshot and clear
//! `ActiveModalOperator`. The sweep picks up new modal operators on its own.
//!
//! `assert_modal_round_trip_op` takes an `Op: Operator` type parameter rather
//! than an id string, so a rename is a compile error.

use crate::util;

use bevy::feathers::controls::ButtonVariant;
use bevy::prelude::*;
use jackdaw::draw_brush::ActivateDrawBrushModalOp;
use jackdaw::tool_ops::{ToolRotateOp, ToolSelectOp};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::{ActiveModalOperator, OperatorEntity};
use jackdaw_feathers::button::ButtonOperatorCall;

/// True iff at least one entity in the world has `ActiveModalOperator`
/// attached. Matches the dispatcher's view of "modal is running."
fn modal_running(app: &mut App) -> bool {
    app.world_mut()
        .query::<&ActiveModalOperator>()
        .iter(app.world())
        .next()
        .is_some()
}

/// Round-trip core, by id. Used by the sweep.
fn assert_modal_round_trip_id(app: &mut App, id: &'static str) {
    let before = util::snapshot(app);
    let result = app
        .world_mut()
        .operator(id)
        .call()
        .unwrap_or_else(|err| panic!("{id}: dispatch errored: {err}"));
    match result {
        OperatorResult::Running => {
            assert!(
                modal_running(app),
                "{id}: returned Running but no ActiveModalOperator was inserted"
            );
            app.world_mut()
                .operator(id)
                .cancel()
                .unwrap_or_else(|err| panic!("{id}: cancel errored: {err}"));
            // Cancel queues commands; advance one frame so the
            // dispatcher actually tears the modal down.
            app.update();
            assert!(
                !modal_running(app),
                "{id}: cancel did not clear ActiveModalOperator"
            );
            let after = util::snapshot(app);
            assert!(before.equals(&*after), "{id}: cancel left state mutated");
        }
        OperatorResult::Cancelled => {
            // Gate refused: acceptable for modals needing a real cursor or
            // scene fixture; dispatch still did not panic.
        }
        OperatorResult::Finished => {
            panic!("{id}: modal operator returned Finished, expected Running or Cancelled");
        }
    }
}

/// Typed round-trip for one modal operator, resolving the id from `O::ID`.
#[expect(
    dead_code,
    reason = "exposed for future per-modal tests that need extra fixtures around the round-trip"
)]
fn assert_modal_round_trip<O: Operator>(app: &mut App) {
    assert_modal_round_trip_id(app, O::ID);
}

/// Every operator declared `modal = true`, round-tripped.
#[test]
fn every_modal_operator_round_trips() {
    let mut app = util::editor_test_app();
    let modal_ids: Vec<&'static str> = app
        .world_mut()
        .query::<&OperatorEntity>()
        .iter(app.world())
        .filter(|op| op.is_modal())
        .map(OperatorEntity::id)
        .collect();
    assert!(
        !modal_ids.is_empty(),
        "expected at least one modal operator to be registered"
    );

    for id in modal_ids {
        // Each iteration starts fresh: cancel any modal a previous
        // round-trip left running before driving the next one.
        let _ = app.world_mut().operator("modal.cancel").call();
        assert_modal_round_trip_id(&mut app, id);
    }
}

/// While a modal operator runs, its toolbar button is the only one carrying
/// `ButtonVariant::Primary`.
#[test]
fn modal_dispatch_steals_toolbar_highlight() {
    let mut app = util::editor_test_app();

    // The real toolbar mounts behind `OnEnter(Editor)`, so model only what the
    // highlight observer reads: a `ButtonOperatorCall` and a `ButtonVariant`.
    let object_button = app
        .world_mut()
        .spawn((
            ButtonOperatorCall::new(ToolSelectOp::ID),
            ButtonVariant::Plain,
        ))
        .id();
    let rotate_button = app
        .world_mut()
        .spawn((
            ButtonOperatorCall::new(ToolRotateOp::ID),
            ButtonVariant::Plain,
        ))
        .id();
    let draw_button = app
        .world_mut()
        .spawn((
            ButtonOperatorCall::new(ActivateDrawBrushModalOp::ID),
            ButtonVariant::Plain,
        ))
        .id();

    let _ = app
        .world_mut()
        .operator(ActivateDrawBrushModalOp::ID)
        .call()
        .unwrap_or_else(|err| panic!("draw brush dispatch errored: {err}"));

    // The highlight is an `On<RefreshOperatorButtons>` observer.
    app.world_mut().trigger(RefreshOperatorButtons);

    let variant_of = |app: &mut App, e: Entity| {
        app.world()
            .entity(e)
            .get::<ButtonVariant>()
            .expect("button has ButtonVariant")
            .clone()
    };

    assert_eq!(
        variant_of(&mut app, draw_button),
        ButtonVariant::Primary,
        "draw-brush button should highlight while its modal is running"
    );
    assert_eq!(
        variant_of(&mut app, object_button),
        ButtonVariant::Plain,
        "object-mode button should drop its highlight while a modal is running"
    );
    assert_eq!(
        variant_of(&mut app, rotate_button),
        ButtonVariant::Plain,
        "gizmo-rotate button should drop its highlight while a modal is running"
    );
}
