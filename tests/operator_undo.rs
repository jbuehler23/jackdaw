//! Undo round-trip coverage. For a curated set of operators with
//! `allows_undo = true` (default), we verify the snapshot-based undo
//! pipeline:
//!
//! ```text
//! before = snapshot()
//! op.call() -> Finished
//! after = snapshot()        # mutated, before != after
//! CommandHistory::undo()
//! undo = snapshot()         # restored, before == undo
//! CommandHistory::redo()
//! redo = snapshot()         # re-applied, after == redo
//! ```
//!
//! The snapshot capture covers `ViewModeSettings`, `OverlaySettings`,
//! `EditMode`, `GizmoMode`, `GizmoSpace`, `SnapSettings`,
//! `PhysicsOverlayConfig`, `GroupEditState.active_group`, and the
//! scene AST (see `src/undo_snapshot.rs`).
//!
//!

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_commands::CommandHistory;

mod util;

#[track_caller]
fn assert_undo_redo_round_trip(app: &mut App, id: &'static str) {
    let before = util::snapshot(app);
    let stack_before = app.world().resource::<CommandHistory>().undo_stack.len();

    // Real user-facing dispatch (toolbar / menu / keybind) opts into
    // history-entry creation. The `.call()` default does not, since
    // operator-from-operator chaining doesn't want to spam the undo
    // stack. Tests covering undo must mirror the user-facing call.
    app.world_mut()
        .operator(id)
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        })
        .call()
        .unwrap_or_else(|err| panic!("{id}: dispatch errored: {err}"))
        .assert_finished_or_panic(id);

    let stack_after = app.world().resource::<CommandHistory>().undo_stack.len();
    assert!(
        stack_after > stack_before,
        "{id}: dispatch did not push an undo entry (stack stayed at {stack_after}); operator may have `allows_undo = false`",
    );

    let after = util::snapshot(app);
    assert!(
        !before.equals(&*after),
        "{id}: snapshot unchanged after dispatch (operator was a no-op or its mutation falls outside snapshot coverage)",
    );

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));

    let undo = util::snapshot(app);
    assert!(
        before.equals(&*undo),
        "{id}: undo did not restore the pre-dispatch state"
    );

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.redo(world));

    let redo = util::snapshot(app);
    assert!(
        after.equals(&*redo),
        "{id}: redo did not restore the post-dispatch state"
    );
}

/// Adapter for cleaner panic messages from `OperatorResult::Finished`
/// assertions inside parameterised helpers.
trait OperatorResultExt {
    fn assert_finished_or_panic(self, id: &'static str);
}

impl OperatorResultExt for OperatorResult {
    fn assert_finished_or_panic(self, id: &'static str) {
        assert_eq!(
            self,
            OperatorResult::Finished,
            "{id}: expected Finished, got {self:?}"
        );
    }
}

#[test]
fn view_toggle_wireframe_round_trip() {
    let mut app = util::editor_test_app();
    assert_undo_redo_round_trip(&mut app, "view.toggle_wireframe");
}

#[test]
fn view_toggle_bounding_boxes_round_trip() {
    let mut app = util::editor_test_app();
    assert_undo_redo_round_trip(&mut app, "view.toggle_bounding_boxes");
}

#[test]
fn view_toggle_brush_outline_round_trip() {
    let mut app = util::editor_test_app();
    assert_undo_redo_round_trip(&mut app, "view.toggle_brush_outline");
}

#[test]
fn view_toggle_face_grid_round_trip() {
    let mut app = util::editor_test_app();
    assert_undo_redo_round_trip(&mut app, "view.toggle_face_grid");
}

#[test]
fn grid_increase_round_trip() {
    let mut app = util::editor_test_app();
    assert_undo_redo_round_trip(&mut app, "grid.increase");
}

#[test]
fn grid_decrease_round_trip() {
    let mut app = util::editor_test_app();
    assert_undo_redo_round_trip(&mut app, "grid.decrease");
}

#[test]
fn gizmo_mode_rotate_round_trip() {
    // Default `GizmoMode` is `Translate`; rotate diverges, so the
    // snapshot diff is non-empty.
    let mut app = util::editor_test_app();
    assert_undo_redo_round_trip(&mut app, "gizmo.mode.rotate");
}

#[test]
fn gizmo_mode_scale_round_trip() {
    let mut app = util::editor_test_app();
    assert_undo_redo_round_trip(&mut app, "gizmo.mode.scale");
}

#[test]
fn gizmo_space_toggle_round_trip() {
    let mut app = util::editor_test_app();
    assert_undo_redo_round_trip(&mut app, "gizmo.space.toggle");
}

#[test]
fn view_cycle_bounding_box_mode_round_trip() {
    let mut app = util::editor_test_app();
    assert_undo_redo_round_trip(&mut app, "view.cycle_bounding_box_mode");
}
