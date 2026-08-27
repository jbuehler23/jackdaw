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
    "field.set",
    "binding.add",
    "binding.set",
];

/// Operators gated on `can_act_on_entities`. A fresh editor refuses
/// none of them: no modal is running, no text field is focused, and
/// the edit mode is `Object`.
const CAN_ACT_ON_ENTITIES_OPS: &[&str] = &[
    "entity.delete",
    "entity.duplicate",
    "entity.copy_components",
    "entity.paste_components",
    "entity.toggle_visibility",
    "entity.unhide_all",
    "entity.hide_unselected",
];

/// Tool and gizmo operators that should be available in a clean app
/// (no modal running, no text field focused).
const TOOL_AND_GIZMO_OPS: &[&str] = &[
    "tool.select",
    "tool.translate",
    "tool.rotate",
    "tool.scale",
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
fn tool_and_gizmo_ops_available_in_clean_app() {
    let mut app = util::editor_test_app();
    for id in TOOL_AND_GIZMO_OPS {
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

/// Bevy's `set_initial_focus` parks `InputFocus` on the primary window
/// in `PostStartup`, so "nothing is focused" reads as `Some(window)`.
/// A gate that asks `input_focus.get().is_some()` therefore refuses
/// every entity operator until something clears focus, which in the
/// running editor means the first viewport click. `KeybindFocus` exists
/// to avoid that, and this pins the entity gate onto it.
#[test]
fn entity_ops_are_available_when_focus_still_sits_on_the_window() {
    let mut app = util::editor_test_app();
    let focused = app
        .world()
        .resource::<bevy::input_focus::InputFocus>()
        .get();
    assert!(
        focused.is_some(),
        "precondition: a settled editor app parks focus on the primary window"
    );
    for id in CAN_ACT_ON_ENTITIES_OPS {
        let ready = app
            .world_mut()
            .operator(*id)
            .is_available()
            .unwrap_or_else(|err| panic!("{id}: is_available errored: {err}"));
        assert!(
            ready,
            "{id} should be available with focus parked on the window, but is_available returned false"
        );
    }
}

/// The other side of the same gate: a real text edit holding focus is
/// the case the guard is for, and it still refuses.
#[test]
fn entity_ops_refuse_while_a_text_field_holds_focus() {
    let mut app = util::editor_test_app();
    let field = app
        .world_mut()
        .spawn(jackdaw_feathers::text_edit::EditorTextEdit)
        .id();
    app.world_mut()
        .resource_mut::<bevy::input_focus::InputFocus>()
        .set(field, bevy::input_focus::FocusCause::Pressed);
    for id in CAN_ACT_ON_ENTITIES_OPS {
        let ready = app
            .world_mut()
            .operator(*id)
            .is_available()
            .unwrap_or_else(|err| panic!("{id}: is_available errored: {err}"));
        assert!(
            !ready,
            "{id} should refuse while a text field holds focus, but is_available returned true"
        );
    }
}

/// The gate decides whether the operator runs at all, so the dispatch
/// it was blocking is worth one pass end to end: a selected entity is
/// gone after `entity.delete` reports `Finished`.
#[test]
fn a_dispatched_delete_takes_the_selected_entity_with_it() {
    let mut app = util::editor_test_app();
    let entity = app
        .world_mut()
        .spawn(bevy::prelude::Name::new("Doomed"))
        .id();
    app.world_mut()
        .resource_mut::<Selection>()
        .entities
        .push(entity);
    app.world_mut().entity_mut(entity).insert(Selected);

    let result = app
        .world_mut()
        .operator("entity.delete")
        .call()
        .expect("entity.delete dispatches");
    assert!(
        matches!(result, OperatorResult::Finished),
        "entity.delete reported {result:?}"
    );
    app.update();
    assert!(
        app.world().get_entity(entity).is_err(),
        "the selected entity survived the delete"
    );
}
