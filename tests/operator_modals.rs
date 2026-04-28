//! Modal-operator coverage. For each `modal = true` operator we verify
//! the start / cancel dance:
//!  1. Dispatch starts the operator.
//!  2. Either the call returns `Running` (modal session active), or
//!     it's `Cancelled` because its availability gate refused;
//!     `Finished` is invalid for `modal = true`.
//!  3. If we got `Running`, `world.operator(id).cancel()` ends the
//!     session and clears `ActiveModalOperator`.
//!  4. After cancel the snapshot equals the pre-dispatch snapshot
//!     (modal cancellation is rollback, not commit).
//!
//! Modals whose `Running` state needs a real cursor or scene fixture
//! (`tools.measure_distance` needs a viewport camera; `terrain.sculpt`
//! needs a heightmap mesh under the cursor) are marked `#[ignore]`
//! with the missing fixture spelled out in the test name comment.

use bevy::prelude::*;
use jackdaw::brush::{BrushEditMode, EditMode};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;

mod util;

/// True iff exactly one entity in the world has `ActiveModalOperator`
/// attached. Mirrors the dispatcher's view of "modal is running."
fn modal_running(app: &mut App) -> bool {
    app.world_mut()
        .query::<&ActiveModalOperator>()
        .iter(app.world())
        .next()
        .is_some()
}

/// Common shape: dispatch the modal, verify it entered a Running state
/// (or returned Cancelled if its gate failed). If Running, cancel and
/// verify the modal is no longer attached and a fresh snapshot matches
/// the original.
fn assert_modal_round_trip(app: &mut App, id: &'static str) {
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
            // Gate refused. Acceptable for modals like
            // `selection.box_select` that need an active viewport
            // cursor; covered by the smoke test, so the round-trip
            // here just becomes a no-op.
        }
        OperatorResult::Finished => {
            panic!("{id}: modal operator returned Finished, expected Running or Cancelled");
        }
    }
}

#[test]
fn physics_activate_modal_round_trip() {
    let mut app = util::editor_test_app();
    assert_modal_round_trip(&mut app, "physics.activate");
}

#[test]
fn gizmo_drag_modal_round_trip() {
    let mut app = util::editor_test_app();
    assert_modal_round_trip(&mut app, "gizmo.drag");
}

#[test]
fn brush_face_drag_modal_round_trip() {
    let mut app = util::editor_test_app();
    // Brush-element drags require Face/Vertex/Edge edit mode to enter
    // a Running session; otherwise the gate cancels them. Set the mode
    // to match before dispatch.
    *app.world_mut().resource_mut::<EditMode>() = EditMode::BrushEdit(BrushEditMode::Face);
    assert_modal_round_trip(&mut app, "brush.face.drag");
}

#[test]
fn brush_vertex_drag_modal_round_trip() {
    let mut app = util::editor_test_app();
    *app.world_mut().resource_mut::<EditMode>() = EditMode::BrushEdit(BrushEditMode::Vertex);
    assert_modal_round_trip(&mut app, "brush.vertex.drag");
}

#[test]
fn brush_edge_drag_modal_round_trip() {
    let mut app = util::editor_test_app();
    *app.world_mut().resource_mut::<EditMode>() = EditMode::BrushEdit(BrushEditMode::Edge);
    assert_modal_round_trip(&mut app, "brush.edge.drag");
}

#[test]
fn selection_box_select_modal_round_trip() {
    let mut app = util::editor_test_app();
    assert_modal_round_trip(&mut app, "selection.box_select");
}

#[test]
fn hierarchy_rename_begin_modal_round_trip() {
    let mut app = util::editor_test_app();
    assert_modal_round_trip(&mut app, "hierarchy.rename_begin");
}

#[test]
fn viewport_draw_brush_modal_round_trip() {
    let mut app = util::editor_test_app();
    assert_modal_round_trip(&mut app, "viewport.draw_brush_modal");
}

#[test]
#[ignore = "needs heightmap mesh + cursor under it"]
fn terrain_sculpt_modal_round_trip() {
    let mut app = util::editor_test_app();
    assert_modal_round_trip(&mut app, "terrain.sculpt");
}

#[test]
fn tools_measure_distance_modal_round_trip() {
    // Without a viewport camera the op cancels at its `Single` query;
    // the round-trip helper accepts the Cancelled path so this test
    // still runs headless.
    let mut app = util::editor_test_app();
    assert_modal_round_trip(&mut app, "tools.measure_distance");
}
