//! Guides on the 2D canvas: the operators that draw them and take them
//! away, the one history entry an edit is, and what a saved document
//! carries.
//!
//! Guides live in the scene rather than beside it, on the UI root, so a
//! layout opens with the lines it was drawn against. What a document
//! must never carry is an empty `CanvasGuides`: a component equal to its
//! default emits as a bare type path, so one left behind would sit in
//! every scene that ever had a guide.

use bevy::prelude::*;
use jackdaw_scene_types::CanvasGuides;

mod util;

use jackdaw_api::op::OperatorWorldExt as _;
use util::OperatorResultExt as _;

#[test]
fn guides_added_by_operator_round_trip_through_save_and_open() {
    let (mut app, root) = guide_app();
    add_guide(&mut app, "vertical", 320.0);
    add_guide(&mut app, "horizontal", 180.0);

    assert_eq!(
        guides(&app, root),
        Some(CanvasGuides {
            horizontal: vec![180.0],
            vertical: vec![320.0],
        }),
        "both operators wrote their line onto the scene's root",
    );

    let text = emit(&mut app);
    assert!(
        text.contains("jackdaw_scene_types::CanvasGuides"),
        "the guides have to reach the document:\n{text}",
    );

    jackdaw::prefab::watcher::respawn_from_sparse_text(app.world_mut(), &text);
    app.update();
    let reopened = ui_root(&mut app);
    assert_eq!(
        guides(&app, reopened),
        Some(CanvasGuides {
            horizontal: vec![180.0],
            vertical: vec![320.0],
        }),
        "and come back where they were when the scene is opened again",
    );
}

#[test]
fn removing_the_last_guide_takes_the_component_off_the_root() {
    let (mut app, root) = guide_app();
    add_guide(&mut app, "vertical", 320.0);
    add_guide(&mut app, "horizontal", 180.0);

    // A position names the guide nearest it, within half a pixel.
    remove_guide(&mut app, "vertical", 320.2);
    assert_eq!(
        guides(&app, root),
        Some(CanvasGuides {
            horizontal: vec![180.0],
            vertical: Vec::new(),
        }),
        "the line nearest the position is the one that goes",
    );

    // Nowhere near a guide is nothing to remove.
    let entries = history_len(&app);
    remove_guide(&mut app, "horizontal", 900.0);
    assert_eq!(history_len(&app), entries, "a miss changes nothing");

    remove_guide(&mut app, "horizontal", 180.0);
    assert!(
        app.world().get::<CanvasGuides>(root).is_none(),
        "the component goes off the root with its last guide",
    );
    let text = emit(&mut app);
    assert!(
        !text.contains("CanvasGuides"),
        "so an empty one never reaches a saved document:\n{text}",
    );
}

#[test]
fn a_guide_edit_is_one_undo_entry() {
    let (mut app, root) = guide_app();
    let entries = history_len(&app);

    add_guide(&mut app, "vertical", 320.0);
    assert_eq!(history_len(&app), entries + 1, "one guide is one entry");

    // Asking for a line that is already there says nothing new.
    add_guide(&mut app, "vertical", 320.0);
    assert_eq!(
        history_len(&app),
        entries + 1,
        "and a guide where one already sits is no entry at all",
    );

    undo(&mut app);
    assert!(
        app.world().get::<CanvasGuides>(root).is_none(),
        "one undo takes the guide, and the component, back off",
    );
}

/// A scene with no guides carries no `CanvasGuides` fields, so the line
/// a document could still hold is the bare type path. Loading it has to
/// give the root an empty component rather than dropping it.
#[test]
fn a_bare_canvas_guides_line_still_loads() {
    let (mut app, _) = guide_app();
    let text = emit(&mut app);
    assert!(
        !text.contains("CanvasGuides"),
        "a scene with no guides carries no guide component:\n{text}",
    );

    // The line a document would hold for a component whose value is its
    // default: the type path and nothing else.
    let anchor = "jackdaw_scene_types::UiSceneRoot";
    assert!(text.contains(anchor), "the root declares itself:\n{text}");
    let bare = text.replacen(
        anchor,
        &format!("{anchor}\njackdaw_scene_types::CanvasGuides"),
        1,
    );

    jackdaw::prefab::watcher::respawn_from_sparse_text(app.world_mut(), &bare);
    app.update();
    let reopened = ui_root(&mut app);
    assert_eq!(
        guides(&app, reopened),
        Some(CanvasGuides::default()),
        "a bare path is a component with no guides on it, not a missing one",
    );
}

/// An editor with one UI scene open, its root registered in the live
/// document. Returns the root.
fn guide_app() -> (App, Entity) {
    let mut app = util::editor_test_app();
    let root = jackdaw::ui_palette::seed_ui_scene_root(app.world_mut());
    app.update();
    (app, root)
}

fn add_guide(app: &mut App, axis: &str, position: f64) {
    call_guide_op(app, "canvas.guide.add", axis, position);
}

fn remove_guide(app: &mut App, axis: &str, position: f64) {
    call_guide_op(app, "canvas.guide.remove", axis, position);
}

fn call_guide_op(app: &mut App, id: &'static str, axis: &str, position: f64) {
    app.world_mut()
        .operator(id)
        .param("axis", axis.to_string())
        .param("position", position)
        .call()
        .unwrap_or_else(|err| panic!("{id} dispatches: {err}"))
        .assert_finished();
    app.update();
}

fn guides(app: &App, root: Entity) -> Option<CanvasGuides> {
    app.world().get::<CanvasGuides>(root).cloned()
}

fn ui_root(app: &mut App) -> Entity {
    app.world_mut()
        .query_filtered::<Entity, jackdaw::prefab::AuthoredUiSceneRoot>()
        .iter(app.world())
        .min()
        .expect("the document holds a UI scene root")
}

fn emit(app: &mut App) -> String {
    jackdaw::scene_io::emit_bsn_scene_with_inline_assets(app.world_mut(), std::path::Path::new(""))
}

fn history_len(app: &App) -> usize {
    app.world()
        .resource::<jackdaw::commands::CommandHistory>()
        .undo_stack
        .len()
}

fn undo(app: &mut App) {
    app.world_mut().resource_scope(
        |world, mut history: Mut<jackdaw::commands::CommandHistory>| {
            history.undo(world);
        },
    );
    app.update();
}
