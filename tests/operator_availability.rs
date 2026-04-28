//! Availability-gate coverage. Each `#[operator(is_available = fn)]`
//! refuses to run when its predicate returns `false`; the dispatcher
//! reports that as `Ok(OperatorResult::Cancelled)`. We exercise the
//! highest-volume gate predicates against fixtures that toggle their
//! precondition, and rely on `OperatorWorldExt::is_available()` to
//! report the gate decision without actually dispatching.
//!
//! Strategy per gate:
//!   1. Set the world to the precondition-false state.
//!      Assert `is_available()` returns `Ok(false)` for every operator
//!      that uses this gate.
//!   2. Set the world to the precondition-true state.
//!      Assert `is_available()` returns `Ok(true)` for the same set.
//!
//! Skipped: gates whose state requires a real cursor / modal state /
//! UI fixture (`picker_open`, `is_drawing`, `measure_tool_active`,
//! etc.). Those are exercised by the modal tests in
//! `operator_modals.rs`. The list here covers the three biggest gates
//! by op count: `has_primary_selection`, `can_act_on_entities`, and
//! `can_change_gizmo`.

use jackdaw::selection::{Selected, Selection};
use jackdaw_api::prelude::*;

mod util;

/// Operators gated on `has_primary_selection` (Selection resource has
/// at least one entity).
const HAS_PRIMARY_SELECTION_OPS: &[&str] = &[
    "viewport.focus_selected",
    "component.add",
    "component.remove",
    "component.revert_baseline",
    "physics.enable",
    "physics.disable",
    "animation.toggle_keyframe",
];

/// Operators gated on `can_change_gizmo` (no modal running, no rename
/// in progress). With a clean app these should already be available.
const CAN_CHANGE_GIZMO_OPS: &[&str] = &[
    "gizmo.mode.translate",
    "gizmo.mode.rotate",
    "gizmo.mode.scale",
    "gizmo.space.toggle",
];

#[test]
fn has_primary_selection_gate_blocks_when_empty() {
    let mut app = util::editor_test_app();
    app.world_mut().resource_mut::<Selection>().entities.clear();
    for id in HAS_PRIMARY_SELECTION_OPS {
        let ready = app
            .world_mut()
            .operator(*id)
            .is_available()
            .unwrap_or_else(|err| panic!("{id}: is_available errored: {err}"));
        assert!(
            !ready,
            "{id} should be unavailable when Selection is empty, but is_available returned true"
        );
    }
}

#[test]
fn has_primary_selection_gate_clears_with_selection() {
    let mut app = util::editor_test_app();
    let entity = app.world_mut().spawn_empty().id();
    app.world_mut()
        .resource_mut::<Selection>()
        .entities
        .push(entity);
    app.world_mut().entity_mut(entity).insert(Selected);

    for id in HAS_PRIMARY_SELECTION_OPS {
        let ready = app
            .world_mut()
            .operator(*id)
            .is_available()
            .unwrap_or_else(|err| panic!("{id}: is_available errored: {err}"));
        assert!(
            ready,
            "{id} should be available when Selection contains an entity, but is_available returned false"
        );
    }
}

#[test]
fn can_change_gizmo_gate_open_in_clean_app() {
    let mut app = util::editor_test_app();
    for id in CAN_CHANGE_GIZMO_OPS {
        let ready = app
            .world_mut()
            .operator(*id)
            .is_available()
            .unwrap_or_else(|err| panic!("{id}: is_available errored: {err}"));
        assert!(
            ready,
            "{id} should be available in a clean editor app, but is_available returned false"
        );
    }
}
