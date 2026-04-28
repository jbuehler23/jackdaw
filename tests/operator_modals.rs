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

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::{ActiveModalOperator, OperatorEntity};

mod util;

/// True iff at least one entity in the world has `ActiveModalOperator`
/// attached. Mirrors the dispatcher's view of "modal is running."
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
