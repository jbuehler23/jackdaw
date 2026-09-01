//! What a keybind row draws: its chords, and the badge saying another
//! command claims one of them.
//!
//! What is pinned here:
//!  * a row is drawn with its chords on the frame it appears, rather than
//!    waiting for the working copy to change again;
//!  * a chord the shipped keymap gives to several commands is marked as
//!    shared, not as a warning;
//!  * a chord this session made shared is the warning.

use bevy::prelude::*;
use jackdaw::keybind_settings::{KeybindChordList, KeymapConflictBadge, PendingKeymapChanges};
use jackdaw_api::prelude::*;
use jackdaw_feathers::icons::Icon;

/// An editor with the keybind dialog open and settled.
fn dialog_app() -> App {
    let mut app = crate::util::editor_test_app();
    crate::enter_editor(&mut app);
    let result = app
        .world_mut()
        .operator("app.open_keybinds")
        .call()
        .expect("the keybind dialog opens through an operator");
    assert_eq!(result, OperatorResult::Finished);
    for _ in 0..4 {
        app.update();
    }
    app
}

/// Every piece of text the row for `operator` shows in its chord list.
fn chords_shown(app: &mut App, operator: &str) -> Vec<String> {
    let list = app
        .world_mut()
        .query::<(Entity, &KeybindChordList)>()
        .iter(app.world())
        .find(|(_, list)| list.0 == operator)
        .map(|(entity, _)| entity)
        .unwrap_or_else(|| panic!("the dialog shows a chord list for {operator}"));
    text_under(app, list)
}

fn text_under(app: &App, entity: Entity) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(text) = app.world().get::<Text>(entity) {
        out.push(text.0.clone());
    }
    for child in app
        .world()
        .get::<Children>(entity)
        .into_iter()
        .flat_map(Children::iter)
    {
        out.extend(text_under(app, child));
    }
    out
}

/// The badge on `operator`'s row: whether it is shown, and the glyph it
/// carries.
fn badge_of(app: &mut App, operator: &str) -> (bool, String) {
    let badge = app
        .world_mut()
        .query::<(Entity, &KeymapConflictBadge)>()
        .iter(app.world())
        .find(|(_, badge)| badge.0 == operator)
        .map(|(entity, _)| entity)
        .unwrap_or_else(|| panic!("the dialog shows a badge slot for {operator}"));
    let shown = app
        .world()
        .get::<Node>(badge)
        .is_some_and(|node| node.display != Display::None);
    (shown, text_under(app, badge).join(""))
}

/// The dialog puts its working copy in the world and spawns its rows in
/// the same breath, so a row that only redrew on the next change to that
/// copy was drawn empty and stayed empty.
#[test]
fn a_row_shows_its_chords_on_the_frame_it_appears() {
    let mut app = dialog_app();
    let shown = chords_shown(&mut app, "entity.delete");
    assert!(
        shown.iter().any(|text| text.contains("Del")),
        "the row drew the chord it holds: {shown:?}",
    );
}

/// The shipped keymap gives Escape to several commands on purpose and
/// arbitrates between them. Marking that with a warning marks most of the
/// list with a warning, and a warning on everything is read as nothing.
#[test]
fn a_shipped_shared_chord_is_marked_as_shared_not_as_a_warning() {
    let mut app = dialog_app();
    let (shown, glyph) = badge_of(&mut app, "entity.delete");
    assert!(shown, "the row says its chord is shared");
    assert_eq!(
        glyph,
        Icon::Info.unicode().to_string(),
        "and says it with the neutral marker",
    );
}

/// A chord this session bound onto a command that already had one is the
/// user's own to sort out, so this one is the warning.
#[test]
fn a_chord_this_session_shared_is_the_warning() {
    let mut app = dialog_app();
    let (shown, _) = badge_of(&mut app, "entity.duplicate");
    assert!(!shown, "the fixture's row starts unshared");

    let chord = jackdaw_api_internal::keymap::PresetInput::key("KeyJ");
    {
        let mut pending = app.world_mut().resource_mut::<PendingKeymapChanges>();
        pending.rebind("entity.duplicate", chord.clone());
        pending.rebind("entity.group", chord);
    }
    app.update();

    let (shown, glyph) = badge_of(&mut app, "entity.duplicate");
    assert!(shown, "the row says the chord is claimed twice");
    assert_eq!(
        glyph,
        Icon::TriangleAlert.unicode().to_string(),
        "and says it as a warning, because this session made it",
    );

    let advisory = jackdaw::keybind_settings::advisory_text(
        app.world().resource::<PendingKeymapChanges>(),
        &[],
        &jackdaw_api_internal::keymap::KeymapLoadProblem::default(),
    );
    assert!(
        advisory.contains("Duplicate") && !advisory.contains("entity.duplicate"),
        "the paragraph names commands the way the rows do: {advisory}",
    );
}

/// Save writes the whole keymap file, and refuses while a file nobody could
/// read is still sitting where it would write. The dialog dismisses itself
/// on the click either way, so a refusal it did not report was a rebind
/// that worked all session and was gone the next morning with nothing
/// having said so.
#[test]
fn a_refused_save_says_so_and_keeps_the_rebind_for_the_session() {
    use jackdaw_api_internal::keymap::PresetInput;
    use jackdaw_feathers::dialog::{DialogActionEvent, EditorDialog};

    let guard = crate::CONFIG_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = crate::empty_config_dir();
    let mut app = dialog_app();

    // The state a failed rescue leaves, put in place after the load that
    // would otherwise have moved it aside.
    let path = dir.join("keymap.json");
    std::fs::write(&path, "{ this is not json").expect("write a corrupt keymap");

    app.world_mut()
        .resource_mut::<PendingKeymapChanges>()
        .rebind("entity.duplicate", PresetInput::key("KeyJ"));
    app.update();

    let dialog = app
        .world_mut()
        .query_filtered::<Entity, With<EditorDialog>>()
        .iter(app.world())
        .next()
        .expect("the keybind dialog is open");
    app.world_mut()
        .trigger(DialogActionEvent { entity: dialog });
    for _ in 0..4 {
        app.update();
    }

    let notice = app.world().resource::<jackdaw::status_bar::StatusNotice>();
    assert!(notice.is_active(), "the refusal was reported");
    let said = notice.text().to_string();
    assert!(
        said.contains("keymap.json") && said.contains("next launch"),
        "and named the file and what it costs: {said}",
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("the file is still there"),
        "{ this is not json",
        "the unread file was left exactly as it was",
    );

    // The rebind is this session's keymap all the same, so reopening the
    // dialog reads it back rather than starting from the shipped chords.
    let result = app
        .world_mut()
        .operator("app.open_keybinds")
        .call()
        .expect("the keybind dialog opens again");
    assert_eq!(result, OperatorResult::Finished);
    for _ in 0..4 {
        app.update();
    }
    assert_eq!(
        app.world()
            .resource::<PendingKeymapChanges>()
            .chords_of("entity.duplicate"),
        vec!["J".to_string()],
        "the working copy came back with the rebind in it",
    );

    let _ = std::fs::remove_file(&path);
    drop(guard);
}
