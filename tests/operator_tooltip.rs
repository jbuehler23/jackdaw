//! End-to-end tooltip pipeline coverage.
//!
//! Verifies that
//! `OperatorTooltipPlugin::auto_attach_button_tooltip` reads the live
//! BEI bindings for the operator behind a `ButtonOperatorCall` and
//! seeds the resulting `Tooltip::keybind` with the user-visible
//! shortcut. Also covers the auto-tagging of action entities via the
//! `OperatorAction` marker, which is what makes the keybind lookup
//! id-keyed instead of generic-over-`Op`.

use bevy::prelude::*;
use jackdaw_api_internal::lifecycle::OperatorAction;
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_feathers::tooltip::Tooltip;

mod util;

/// True if the world contains at least one entity with
/// `OperatorAction(operator_id)`.
fn operator_is_tagged(app: &mut App, operator_id: &str) -> bool {
    app.world_mut()
        .query::<&OperatorAction>()
        .iter(app.world())
        .any(|action| action.0 == operator_id)
}

/// Spawn a button bound to `op_id`, advance one frame so observers
/// run, and return the `Tooltip::keybind` text the pipeline wrote.
/// The keybind comes from BEI bindings registered by the editor's
/// `add_to_extension` modules at startup.
fn keybind_for(app: &mut App, op_id: &'static str) -> String {
    let entity = app.world_mut().spawn(ButtonOperatorCall::new(op_id)).id();
    app.update();
    app.world()
        .entity(entity)
        .get::<Tooltip>()
        .map(|tip| tip.keybind.clone())
        .unwrap_or_default()
}

/// Sanity: the auto-tag plumbing inserted `OperatorAction(<id>)` on
/// the action entities for representative built-in operators. Covers
/// both registration orderings the editor uses today: `view_ops` /
/// `entity_ops` register first then spawn, `draw_brush::add_to_extension`
/// spawns first then registers.
#[test]
fn action_entities_carry_operator_action_marker() {
    let mut app = util::editor_test_app();

    // Register-then-spawn (view_ops, entity_ops, edit_mode_ops, ...).
    assert!(
        operator_is_tagged(&mut app, "edit_mode.vertex"),
        "edit_mode.vertex action entity should carry OperatorAction",
    );
    assert!(
        operator_is_tagged(&mut app, "view.toggle_wireframe"),
        "view.toggle_wireframe action entity should carry OperatorAction",
    );
    assert!(
        operator_is_tagged(&mut app, "entity.delete"),
        "entity.delete action entity should carry OperatorAction",
    );

    // Spawn-then-register (draw_brush::add_to_extension): the
    // retroactive scan in `register_operator` is what tags this one.
    assert!(
        operator_is_tagged(&mut app, "viewport.draw_brush_modal"),
        "viewport.draw_brush_modal action entity should carry OperatorAction \
         (retroactive scan must cover spawn-before-register modules)",
    );
}

/// `view.toggle_wireframe` is bound to `Ctrl + Shift + W` in
/// `view_ops::add_to_extension`. The tooltip pipeline should surface
/// that exact string.
#[test]
fn tooltip_picks_up_keyboard_modifier_binding() {
    let mut app = util::editor_test_app();
    let keybind = keybind_for(&mut app, "view.toggle_wireframe");
    assert_eq!(
        keybind, "Ctrl + Shift + W",
        "wireframe toggle should display its modifier binding",
    );
}

/// `clip.delete_keyframes` binds both `Delete` and `Backspace`. The
/// tooltip joins multiple bindings with `" / "`. `KeyCode::Delete`
/// stringifies to `Del` via `key_display_name`.
#[test]
fn tooltip_joins_multiple_bindings() {
    let mut app = util::editor_test_app();
    let keybind = keybind_for(&mut app, "clip.delete_keyframes");
    assert!(
        keybind == "Del / Backspace" || keybind == "Backspace / Del",
        "expected del + backspace joined; got `{keybind}`",
    );
}

/// `viewport.draw_brush_modal` mixes a key (`B`) with a mouse button
/// (`Mouse Back`). Both should appear in the tooltip; mouse-button
/// glyphs use the friendly aliases (`Mouse Left` / `Mouse Right` /
/// `Mouse Back` / ...) instead of the raw enum name.
#[test]
fn tooltip_includes_mouse_button_bindings() {
    let mut app = util::editor_test_app();
    let keybind = keybind_for(&mut app, "viewport.draw_brush_modal");
    assert!(
        keybind.contains("Mouse Back"),
        "draw-brush modal should mention `Mouse Back`; got `{keybind}`",
    );
    assert!(
        keybind.contains('B'),
        "draw-brush modal should mention `B`; got `{keybind}`",
    );
}

/// Buttons whose operator id has no BEI binding (here a bogus id
/// that never resolves) should produce no `Tooltip` at all because
/// the auto-attach skips when the operator can't be found. Acts as a
/// regression guard against the keybind path panicking on an unknown
/// id.
#[test]
fn unknown_operator_id_skips_tooltip() {
    let mut app = util::editor_test_app();
    let entity = app
        .world_mut()
        .spawn(ButtonOperatorCall::new("does.not.exist"))
        .id();
    app.update();
    assert!(
        app.world().entity(entity).get::<Tooltip>().is_none(),
        "tooltip should not attach for an unknown operator id",
    );
}
