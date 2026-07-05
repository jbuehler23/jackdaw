//! Modal-operator coverage. Iterates every operator declared
//! `modal = true` and round-trips each:
//!  1. Dispatch starts the operator.
//!  2. Either the call returns `Running` (modal session active), or
//!     `Cancelled` because its availability gate refused.
//!     `Finished` is invalid for `modal = true`.
//!  3. If we got `Running`, `world.operator(id).cancel()` ends the
//!     session and clears `ActiveModalOperator`.
//!  4. After cancel the snapshot equals the pre-dispatch snapshot
//!     (modal cancellation is rollback, not commit).
//!
//! The sweep auto-picks up new modal operators, so coverage scales
//! with the codebase without per-modal hand-rolled tests.
//!
//! Per-modal round-trip helpers ([`assert_modal_round_trip_op`]) take
//! an `Op: Operator` type parameter rather than a raw id string, so
//! call sites compile-fail when the operator is renamed instead of
//! silently going stale.

use bevy::feathers::controls::ButtonVariant;
use bevy::prelude::*;
use jackdaw::draw_brush::ActivateDrawBrushModalOp;
use jackdaw::tool_ops::{ToolRotateOp, ToolSelectOp};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::{ActiveModalOperator, OperatorEntity};
use jackdaw_feathers::button::ButtonOperatorCall;

mod util;

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
            // Gate refused. Acceptable for modals that need a real
            // cursor or scene fixture (no viewport camera, no
            // selection, etc.); the smoke test still proved dispatch
            // doesn't panic.
        }
        OperatorResult::Finished => {
            panic!("{id}: modal operator returned Finished, expected Running or Cancelled");
        }
    }
}

/// Typed round-trip for a specific modal operator. Resolves the id
/// from `O::ID` so a rename of `O` is a build error, not a stale
/// string literal.
#[expect(
    dead_code,
    reason = "exposed for future per-modal tests that need extra fixtures around the round-trip"
)]
fn assert_modal_round_trip<O: Operator>(app: &mut App) {
    assert_modal_round_trip_id(app, O::ID);
}

/// Sweep: enumerate every operator declared `modal = true` and run
/// the round-trip on each. New modal operators get coverage
/// automatically; CI flags any modal that panics on dispatch or
/// fails to clear `ActiveModalOperator` on cancel.
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

/// Regression: while a modal operator runs, its toolbar button is the
/// only one carrying `ButtonVariant::Primary`. Mode and gizmo buttons
/// drop to `Plain` so the user sees a single active tool. Covers the
/// case where Object Mode stayed highlighted while Draw Brush was
/// armed.
#[test]
fn modal_dispatch_steals_toolbar_highlight() {
    let mut app = util::editor_test_app();

    // Synthetic toolbar buttons. The real toolbar mounts behind
    // `OnEnter(Editor)`, so model just the surface the highlight
    // observer reads: a `ButtonOperatorCall` and a `ButtonVariant`.
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

    // Activate the Draw Brush modal.
    let _ = app
        .world_mut()
        .operator(ActivateDrawBrushModalOp::ID)
        .call()
        .unwrap_or_else(|err| panic!("draw brush dispatch errored: {err}"));

    // The highlight is an `On<RefreshOperatorButtons>` observer. Fire
    // the event so it flips the variants against the running modal.
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
