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
use jackdaw::draw_brush::ActivateDrawBrushModalOp;
use jackdaw::edit_mode_ops::EditModeObjectOp;
use jackdaw::gizmo_ops::GizmoModeRotateOp;
use jackdaw::layout::update_toolbar_button_variants;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::{ActiveModalOperator, OperatorEntity};
use jackdaw_feathers::button::{ButtonOperatorCall, ButtonVariant};

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

/// Regression: when a modal operator is running, its toolbar button
/// is the only one carrying `ButtonVariant::Active`. Mode/gizmo
/// buttons must drop their highlight so the user sees a single
/// active tool. Reproduces the bug where Object Mode stayed
/// highlighted while Draw Brush was armed because the system
/// short-circuited on `Res::is_changed()` and never observed
/// `ActiveModalOperator` being inserted.
#[test]
fn modal_dispatch_steals_toolbar_highlight() {
    let mut app = util::editor_test_app();

    // Spawn synthetic toolbar entities. The real toolbar isn't
    // mounted in the test app (it lives behind `OnEnter(Editor)`),
    // so we model just enough surface for the highlight system to
    // run against: a `ButtonOperatorCall` and a `ButtonVariant`.
    let object_button = app
        .world_mut()
        .spawn((
            ButtonOperatorCall::new(EditModeObjectOp::ID),
            ButtonVariant::Active,
        ))
        .id();
    let rotate_button = app
        .world_mut()
        .spawn((
            ButtonOperatorCall::new(GizmoModeRotateOp::ID),
            ButtonVariant::Active,
        ))
        .id();
    let draw_button = app
        .world_mut()
        .spawn((
            ButtonOperatorCall::new(ActivateDrawBrushModalOp::ID),
            ButtonVariant::Ghost,
        ))
        .id();

    // Activate the Draw Brush modal.
    let _ = app
        .world_mut()
        .operator(ActivateDrawBrushModalOp::ID)
        .call()
        .unwrap_or_else(|err| panic!("draw brush dispatch errored: {err}"));

    // Run the highlight system once. It's gated to AppState::Editor
    // in the editor's plugin schedule, so we drive it directly here.
    app.world_mut()
        .run_system_cached(update_toolbar_button_variants)
        .expect("update_toolbar_button_variants ran");

    let variant_of = |app: &mut App, e: Entity| {
        *app.world()
            .entity(e)
            .get::<ButtonVariant>()
            .expect("button has ButtonVariant")
    };

    assert_eq!(
        variant_of(&mut app, draw_button),
        ButtonVariant::Active,
        "draw-brush button should highlight while its modal is running"
    );
    assert_eq!(
        variant_of(&mut app, object_button),
        ButtonVariant::Ghost,
        "object-mode button should drop its highlight while a modal is running"
    );
    assert_eq!(
        variant_of(&mut app, rotate_button),
        ButtonVariant::Ghost,
        "gizmo-rotate button should drop its highlight while a modal is running"
    );
}
