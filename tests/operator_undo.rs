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
//! `EditMode`, `ActiveTool`, `GizmoSpace`, `SnapSettings`,
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
fn entity_place_gltf_survives_later_history_and_scene_tabs() {
    let path = "models/dungeon.glb";
    let position = Vec3::new(1.25, -2.0, 3.5);
    let mut app = util::editor_test_app();
    let stack_before = app.world().resource::<CommandHistory>().undo_stack.len();

    app.world_mut()
        .operator("entity.place_gltf")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        })
        .param("path", path.to_string())
        .param("pos_x", position.x as f64)
        .param("pos_y", position.y as f64)
        .param("pos_z", position.z as f64)
        .call()
        .expect("entity.place_gltf dispatch resolves")
        .assert_finished_or_panic("entity.place_gltf");

    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        stack_before + 1,
        "placement should create exactly one undo entry"
    );

    let placed = assert_single_renderable_gltf(&mut app, path, position);
    assert!(
        app.world()
            .resource::<jackdaw_bsn::SceneBsnAst>()
            .ast_for(placed)
            .is_some(),
        "placed GLB must be part of the authoritative scene document"
    );

    // Reproduce the reported sequence: a later rotate action creates another
    // snapshot, then undo/redo reloads the scene document. The authored GLB
    // and its derived render root must both survive those reloads.
    app.world_mut()
        .operator("tool.rotate")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        })
        .call()
        .expect("tool.rotate dispatch resolves")
        .assert_finished_or_panic("tool.rotate");
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        stack_before + 2,
        "rotate should create the history entry after placement"
    );

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    assert_single_renderable_gltf(&mut app, path, position);

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.redo(world));
    assert_single_renderable_gltf(&mut app, path, position);

    // Undo both actions, then redo just the placement to verify the placement
    // snapshot itself also restores a renderable GLB.
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    assert_eq!(
        app.world_mut()
            .query::<&jackdaw_scene_types::GltfSource>()
            .iter(app.world())
            .count(),
        0,
        "undo should remove the placed GLB"
    );
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.redo(world));
    assert_single_renderable_gltf(&mut app, path, position);

    // Scene tabs capture and restore the same authoritative document. A trip
    // to an empty tab and back must rehydrate the GLB's derived render root.
    {
        let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
        *scenes = jackdaw::scenes::Scenes::default();
        scenes.tabs.push(jackdaw::scenes::SceneTab::new_untitled(1));
        scenes.tabs.push(jackdaw::scenes::SceneTab::new_untitled(2));
        scenes.active = 0;
    }
    jackdaw::scenes::swap::swap_active_tab(app.world_mut(), 1);
    assert_eq!(
        app.world_mut()
            .query::<&jackdaw_scene_types::GltfSource>()
            .iter(app.world())
            .count(),
        0,
        "the second tab should remain empty"
    );
    jackdaw::scenes::swap::swap_active_tab(app.world_mut(), 0);
    assert_single_renderable_gltf(&mut app, path, position);
}

#[track_caller]
fn assert_single_renderable_gltf(app: &mut App, path: &str, position: Vec3) -> Entity {
    let (entity, source, transform, _) = app
        .world_mut()
        .query::<(
            Entity,
            &jackdaw_scene_types::GltfSource,
            &Transform,
            &bevy::world_serialization::WorldAssetRoot,
        )>()
        .single(app.world())
        .expect("expected one GLB root with its derived render asset");
    assert_eq!(source.path, path);
    assert_eq!(transform.translation, position);
    entity
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
fn tool_rotate_round_trip() {
    // Default `ActiveTool` is `Select`; rotate diverges, so the
    // snapshot diff is non-empty.
    let mut app = util::editor_test_app();
    assert_undo_redo_round_trip(&mut app, "tool.rotate");
}

#[test]
fn tool_select_round_trip_from_translate() {
    // `tool.select` flips both `ActiveTool` and `EditMode`; starting
    // from a non-default state proves the snapshot captures both.
    let mut app = util::editor_test_app();
    *app.world_mut()
        .resource_mut::<jackdaw::active_tool::ActiveTool>() =
        jackdaw::active_tool::ActiveTool::Translate;
    *app.world_mut().resource_mut::<jackdaw::brush::EditMode>() =
        jackdaw::brush::EditMode::BrushEdit(jackdaw::brush::BrushEditMode::Vertex);
    assert_undo_redo_round_trip(&mut app, "tool.select");
}

#[test]
fn tool_scale_round_trip() {
    let mut app = util::editor_test_app();
    assert_undo_redo_round_trip(&mut app, "tool.scale");
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
