//! End-to-end tooltip pipeline: `auto_attach_button_tooltip` reads the live BEI
//! bindings for the operator behind a `ButtonOperatorCall` and seeds
//! `Tooltip::keybind`, and action entities are auto-tagged `OperatorAction` so
//! the lookup is id-keyed.

use crate::util;

use bevy::prelude::*;
use jackdaw_api_internal::lifecycle::OperatorAction;
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_feathers::tooltip::Tooltip;

/// True if the world contains at least one entity with
/// `OperatorAction(operator_id)`.
fn operator_is_tagged(app: &mut App, operator_id: &str) -> bool {
    app.world_mut()
        .query::<&OperatorAction>()
        .iter(app.world())
        .any(|action| action.0 == operator_id)
}

/// Spawn a button bound to `op_id`, advance one frame so observers run, and
/// return the `Tooltip::keybind` text the pipeline wrote.
fn keybind_for(app: &mut App, op_id: &'static str) -> String {
    let entity = app.world_mut().spawn(ButtonOperatorCall::new(op_id)).id();
    app.update();
    app.world()
        .entity(entity)
        .get::<Tooltip>()
        .map(|tip| tip.keybind.clone())
        .unwrap_or_default()
}

/// The auto-tag plumbing inserts `OperatorAction(<id>)` on action entities under
/// both registration orderings the editor uses.
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

    // Spawn-then-register (draw_brush::add_to_extension): the retroactive scan in
    // `register_operator` is what tags this one.
    assert!(
        operator_is_tagged(&mut app, "viewport.draw_brush_modal"),
        "viewport.draw_brush_modal action entity should carry OperatorAction \
         (retroactive scan must cover spawn-before-register modules)",
    );
}

/// `view.toggle_wireframe` is bound to `Ctrl + Shift + W` via the classic
/// keymap preset, which the startup applier applies before the first frame.
#[test]
fn tooltip_picks_up_keyboard_modifier_binding() {
    let mut app = util::editor_test_app();
    let keybind = keybind_for(&mut app, "view.toggle_wireframe");
    assert_eq!(
        keybind, "Ctrl + Shift + W",
        "wireframe toggle should display its modifier binding",
    );
}

/// `clip.delete_keyframes` binds both `Delete` and `Backspace`; the tooltip joins
/// bindings with `" / "` and `KeyCode::Delete` stringifies to `Del`.
#[test]
fn tooltip_joins_multiple_bindings() {
    let mut app = util::editor_test_app();
    let keybind = keybind_for(&mut app, "clip.delete_keyframes");
    assert!(
        keybind == "Del / Backspace" || keybind == "Backspace / Del",
        "expected del + backspace joined; got `{keybind}`",
    );
}

/// `viewport.draw_brush_modal` mixes a key with a mouse button, and mouse-button
/// glyphs use the friendly aliases rather than the raw enum name.
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

/// A button whose operator id has no BEI binding gets no `Tooltip` at all, and
/// the keybind path does not panic on the unknown id.
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

/// A chord is only discoverable if the button it mirrors says what pressing it
/// does, so the magnet toggle's tooltip carries label, chord and description.
#[test]
fn the_snap_toggle_tooltip_names_its_chord_and_what_it_does() {
    let mut app = util::editor_test_app();
    let entity = app
        .world_mut()
        .spawn(ButtonOperatorCall::new("snap.toggle"))
        .id();
    app.update();
    let tip = app
        .world()
        .entity(entity)
        .get::<Tooltip>()
        .expect("the snap toggle is a registered operator")
        .clone();
    assert_eq!(tip.title, "Toggle Snapping");
    assert_eq!(tip.keybind, "M");
    assert!(
        !tip.description.is_empty(),
        "the chord is only discoverable with a description behind it",
    );
}
