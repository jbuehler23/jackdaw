//! Parameterised-operator dispatch coverage. The goal here is to
//! exercise the param-passing path: each test passes typed params via
//! `OperatorCallBuilder::param()` and asserts the dispatcher resolves
//! the operator + the parameters reach the invoke system without
//! triggering a panic, type mismatch, or `UnknownId`.
//!
//! For ops whose invoke-system needs a fixture we don't have (camera,
//! viewport, registered window), we accept either `Finished` or
//! `Cancelled` and document why. The smoke test in `operator_smoke.rs`
//! covers the empty-param dispatch path; this file covers the typed
//! param path.
//!
//! A wrong-type case proves `OperatorParameters::as_int` / `as_str`
//! coerce or refuse the way the runtime expects.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_scene_types::PropertyValue;

mod util;

/// Helper: dispatch an op with one named param and assert the call
/// resolved (i.e. didn't error with `UnknownId` or panic) and produced
/// one of the well-formed result variants.
#[track_caller]
fn dispatch_with_param(
    app: &mut App,
    id: &'static str,
    key: &'static str,
    value: impl Into<PropertyValue>,
) -> OperatorResult {
    app.world_mut()
        .operator(id)
        .param(key, value)
        .call()
        .unwrap_or_else(|err| panic!("{id}: dispatch errored with {key}: {err}"))
}

#[test]
fn viewport_bookmark_save_with_slot_param() {
    let mut app = util::editor_test_app();
    // No camera in headless app, so the op cancels at the camera
    // single-query, but the dispatcher must still parse the i64
    // `slot` param and route through the gate. Cancelled here proves
    // the dispatch path; Finished would require a real camera fixture.
    let result = dispatch_with_param(&mut app, "viewport.bookmark.save", "slot", 0_i64);
    assert!(
        matches!(result, OperatorResult::Finished | OperatorResult::Cancelled),
        "viewport.bookmark.save: got {result:?}, expected Finished or Cancelled",
    );
}

#[test]
fn viewport_bookmark_save_invalid_slot_cancels() {
    // `slot_param` only accepts 0..=8; out-of-range cancels.
    let mut app = util::editor_test_app();
    let result = dispatch_with_param(&mut app, "viewport.bookmark.save", "slot", 99_i64);
    assert_eq!(
        result,
        OperatorResult::Cancelled,
        "out-of-range slot=99 should cancel"
    );
}

#[test]
fn viewport_bookmark_save_wrong_type_cancels() {
    // Passing a string where i64 is expected: `as_int` returns None,
    // `slot_param` returns None, op cancels. Proves the parameter
    // type-coercion path bottoms out without panicking.
    let mut app = util::editor_test_app();
    let result = dispatch_with_param(&mut app, "viewport.bookmark.save", "slot", "not a number");
    assert_eq!(
        result,
        OperatorResult::Cancelled,
        "string param where i64 was expected should cancel"
    );
}

#[test]
fn asset_cycle_array_layer_uses_default_direction() {
    // `asset.cycle_array_layer` has `direction(i64, default = 1)`. It
    // cancels via the `has_array_preview` gate when no array preview
    // is loaded; that's expected in headless. Either Cancelled (gate)
    // or Finished is fine; what we care about is that the dispatch
    // resolves and doesn't panic on the default-fill path.
    let mut app = util::editor_test_app();
    let result = app
        .world_mut()
        .operator("asset.cycle_array_layer")
        .call()
        .unwrap();
    assert!(
        matches!(result, OperatorResult::Finished | OperatorResult::Cancelled),
        "asset.cycle_array_layer empty-param dispatch returned {result:?}",
    );
}

#[test]
fn window_open_with_unknown_id_cancels() {
    let mut app = util::editor_test_app();
    let result = dispatch_with_param(
        &mut app,
        "window.open",
        "window_id",
        "definitely-not-a-real-window".to_string(),
    );
    assert_eq!(
        result,
        OperatorResult::Cancelled,
        "unknown window id should cancel, not silently no-op + Finished"
    );
}

/// The capture operators. Both stand in for a pointer gesture, so both are
/// dispatched here with the params a capture script passes rather than empty,
/// which is all the smoke pass can do.
#[test]
fn menu_open_opens_the_menu_it_names() {
    let mut app = util::editor_test_app();
    let world = app.world_mut();
    let bar = world
        .spawn(jackdaw_feathers::menu_bar::menu_bar_shell())
        .id();
    jackdaw_feathers::menu_bar::populate_menu_bar(
        world,
        bar,
        [(
            "File".to_string(),
            vec![("op:scene.new".to_string(), "New".to_string())],
        )],
    );
    app.update();

    let result = dispatch_with_param(&mut app, "menu.open", "name", "File");
    app.update();

    assert_eq!(result, OperatorResult::Finished);
    let world = app.world_mut();
    assert!(
        world
            .resource::<jackdaw_widgets::menu_bar::MenuBarState>()
            .open_menu
            .is_some(),
        "the bar records the open menu",
    );
    assert_eq!(
        world
            .query_filtered::<Entity, With<jackdaw_widgets::menu_bar::MenuBarDropdown>>()
            .iter(world)
            .count(),
        1,
        "and a dropdown is on screen to photograph",
    );
}

/// A menu bar with no such label refuses rather than opening something else.
#[test]
fn menu_open_cancels_on_a_label_no_menu_carries() {
    let mut app = util::editor_test_app();
    let world = app.world_mut();
    let bar = world
        .spawn(jackdaw_feathers::menu_bar::menu_bar_shell())
        .id();
    jackdaw_feathers::menu_bar::populate_menu_bar(world, bar, [("File".to_string(), Vec::new())]);
    app.update();

    let result = dispatch_with_param(&mut app, "menu.open", "name", "Sculpt");

    assert_eq!(result, OperatorResult::Cancelled);
}

#[test]
fn preview_set_starts_and_stops_the_session() {
    let mut app = util::editor_test_app();

    let result = dispatch_with_param(&mut app, "preview.set", "on", true);
    app.update();

    assert_eq!(result, OperatorResult::Finished);
    assert!(
        app.world()
            .resource::<jackdaw::preview_context::PreviewSession>()
            .is_on(),
        "the session is running",
    );

    let stopped = dispatch_with_param(&mut app, "preview.set", "on", false);
    app.update();
    assert_eq!(stopped, OperatorResult::Finished);

    assert!(
        !app.world()
            .resource::<jackdaw::preview_context::PreviewSession>()
            .is_on(),
        "and stops again",
    );
}
