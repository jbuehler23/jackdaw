//! Answering a modal dialog without a pointer.
//!
//! A dialog stops the editor until one of its buttons is pressed. The
//! `dialog.answer` operator presses one by label or index, so a caller
//! with no pointer can get past "Scene Changed on Disk" and its kin.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_feathers::dialog::{
    DialogChoice, DialogChoiceError, DialogChoices, EditorDialog, OpenDialogEvent,
    resolve_dialog_choice,
};

use crate::util;
use crate::util::OperatorResultExt as _;

/// Raise a dialog the way the editor's own prompts do, and settle.
fn open_dialog(app: &mut App, event: OpenDialogEvent) {
    app.world_mut().commands().trigger(event);
    app.world_mut().flush();
    app.update();
}

fn dialog_count(app: &mut App) -> usize {
    app.world_mut()
        .query_filtered::<Entity, With<EditorDialog>>()
        .iter(app.world())
        .count()
}

fn choices(app: &mut App) -> DialogChoices {
    app.world_mut()
        .query_filtered::<&DialogChoices, With<EditorDialog>>()
        .iter(app.world())
        .next()
        .cloned()
        .expect("a dialog is up")
}

/// A dialog records what it is asking, so a caller can read the question
/// before answering it. Drawing the labels is not enough: they live in
/// `Text` children several levels down.
#[test]
fn a_dialog_carries_the_question_and_its_buttons() {
    let mut app = util::editor_test_app();
    open_dialog(
        &mut app,
        OpenDialogEvent::new("Scene Changed on Disk", "Reload")
            .with_description("level.bsn changed on disk."),
    );

    let choices = choices(&mut app);
    assert_eq!(choices.title.as_deref(), Some("Scene Changed on Disk"));
    assert_eq!(choices.action.as_deref(), Some("Reload"));
    assert_eq!(choices.cancel.as_deref(), Some("Cancel"));
    assert_eq!(choices.labels(), vec!["Reload", "Cancel"]);
}

/// Answering by label takes the dialog down. The primary action is what
/// a bare `dialog.answer` means, because that is the answer a dialog is
/// asking for.
#[test]
fn answering_by_label_presses_the_button_and_closes_the_dialog() {
    let mut app = util::editor_test_app();
    open_dialog(
        &mut app,
        OpenDialogEvent::new("Scene Changed on Disk", "Reload"),
    );
    assert_eq!(dialog_count(&mut app), 1);

    app.world_mut()
        .operator("dialog.answer")
        .param("choice", "Reload")
        .call()
        .expect("dialog.answer dispatches")
        .assert_finished();
    app.update();

    assert_eq!(dialog_count(&mut app), 0, "the dialog is still up");
}

/// A caller should not have to reproduce the parenthetical the editor
/// appends when a tab is dirty, so a prefix answers the button.
#[test]
fn a_prefix_answers_a_button_whose_label_says_more() {
    let mut app = util::editor_test_app();
    open_dialog(
        &mut app,
        OpenDialogEvent::new(
            "Scene Changed on Disk",
            "Reload (discards your unsaved changes)",
        ),
    );

    let choices = choices(&mut app);
    assert_eq!(
        resolve_dialog_choice(&choices, "Reload"),
        Ok(DialogChoice::Action)
    );
    assert_eq!(
        resolve_dialog_choice(&choices, "reload"),
        Ok(DialogChoice::Action),
        "matching is case-insensitive once the exact spelling misses"
    );

    app.world_mut()
        .operator("dialog.answer")
        .param("choice", "Reload")
        .call()
        .expect("dialog.answer dispatches")
        .assert_finished();
    app.update();
    assert_eq!(dialog_count(&mut app), 0);
}

/// Indices count from the primary action, so `1` on a two-button dialog
/// is the cancel that leaves the editor's copy standing.
#[test]
fn an_index_names_a_button_from_the_primary_action() {
    let mut app = util::editor_test_app();
    let mut event = OpenDialogEvent::new("Scene Changed on Disk", "Reload");
    event.cancel = Some("Keep Mine".to_string());
    open_dialog(&mut app, event);

    let choices = choices(&mut app);
    assert_eq!(
        resolve_dialog_choice(&choices, "0"),
        Ok(DialogChoice::Action)
    );
    assert_eq!(
        resolve_dialog_choice(&choices, "1"),
        Ok(DialogChoice::Cancel)
    );
    assert_eq!(
        resolve_dialog_choice(&choices, "2"),
        Err(DialogChoiceError::NoMatch)
    );

    app.world_mut()
        .operator("dialog.answer")
        .param("choice", "1")
        .call()
        .expect("dialog.answer dispatches")
        .assert_finished();
    app.update();
    assert_eq!(dialog_count(&mut app), 0);
}

/// A name that is not on the dialog is refused with the dialog left up,
/// rather than pressing whichever button happened to be first.
#[test]
fn an_unknown_choice_leaves_the_dialog_alone() {
    let mut app = util::editor_test_app();
    open_dialog(
        &mut app,
        OpenDialogEvent::new("Scene Changed on Disk", "Reload"),
    );

    let result = app
        .world_mut()
        .operator("dialog.answer")
        .param("choice", "Obliterate")
        .call()
        .expect("dialog.answer dispatches");
    assert_eq!(result, OperatorResult::Cancelled);
    app.update();
    assert_eq!(dialog_count(&mut app), 1, "the dialog was answered anyway");
}

/// With nothing to answer the operator is unavailable, so it never shows
/// as something to try when no dialog is up.
#[test]
fn answering_is_unavailable_with_no_dialog_up() {
    let mut app = util::editor_test_app();
    assert_eq!(dialog_count(&mut app), 0);
    assert_eq!(
        app.world_mut()
            .operator("dialog.answer")
            .is_available()
            .ok(),
        Some(false)
    );
}

/// A prefix that fits two buttons is a coin toss over the user's data:
/// `D` on Don't Save and Delete has to press neither, and say which two
/// it could not choose between.
#[test]
fn an_ambiguous_prefix_presses_nothing_and_names_the_candidates() {
    let mut app = util::editor_test_app();
    let mut event = OpenDialogEvent::new("Unsaved Changes", "Delete");
    event.secondary_action = Some("Don't Save".to_string());
    event.cancel = Some("Cancel".to_string());
    open_dialog(&mut app, event);

    let choices = choices(&mut app);
    assert_eq!(
        resolve_dialog_choice(&choices, "D"),
        Err(DialogChoiceError::Ambiguous(vec![
            "Delete".to_string(),
            "Don't Save".to_string(),
        ]))
    );
    assert_eq!(
        resolve_dialog_choice(&choices, "Del"),
        Ok(DialogChoice::Action),
        "a prefix that fits one button still answers"
    );

    let result = app
        .world_mut()
        .operator("dialog.answer")
        .param("choice", "D")
        .call()
        .expect("dialog.answer dispatches");
    assert_eq!(result, OperatorResult::Cancelled);
    app.update();
    assert_eq!(dialog_count(&mut app), 1, "the dialog is still up");
}

/// A button literally spelled `1` is reachable by name: the exact label
/// is tried before the value is read as an index.
#[test]
fn a_label_that_looks_like_an_index_is_still_a_label() {
    let mut app = util::editor_test_app();
    let mut event = OpenDialogEvent::new("Pick a slot", "2");
    event.cancel = Some("1".to_string());
    open_dialog(&mut app, event);

    let choices = choices(&mut app);
    assert_eq!(
        resolve_dialog_choice(&choices, "1"),
        Ok(DialogChoice::Cancel),
        "the button spelled `1` wins over index 1"
    );
    assert_eq!(
        resolve_dialog_choice(&choices, "2"),
        Ok(DialogChoice::Action),
        "and the button spelled `2` over an index that addresses nothing"
    );
    assert_eq!(
        resolve_dialog_choice(&choices, "0"),
        Ok(DialogChoice::Action),
        "an index nothing is spelled like still counts from the primary action"
    );
}
