//! Text fields.
//!
//! A field is a `FeathersTextInputContainer` around a `FeathersTextInput`:
//! the caret, the selection, the focus ring and the keyboard are the
//! widget's, and the editor supplies only the label, prefix, suffix and
//! the commit the inspector listens for.

use bevy::feathers::controls::{FeathersTextInput, FeathersTextInputContainer};
use bevy::input_focus::{FocusCause, InputFocus};
use bevy::prelude::*;
use bevy::text::{EditableText, TextEdit};

use jackdaw_feathers::text_edit::{TextEditCommitEvent, TextEditProps, TextEditValue, text_edit};

mod util;

fn descendant_with<C: Component>(world: &mut World, root: Entity) -> Option<Entity> {
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        if world.get::<C>(entity).is_some() && entity != root {
            return Some(entity);
        }
        if let Some(children) = world.get::<Children>(entity) {
            stack.extend(children.iter());
        }
    }
    None
}

/// The field's frame and its input entity.
fn field(app: &mut App, root: Entity) -> (Entity, Entity) {
    let frame = descendant_with::<FeathersTextInputContainer>(app.world_mut(), root)
        .expect("the field has a frame");
    let input = descendant_with::<FeathersTextInput>(app.world_mut(), frame)
        .expect("the frame holds an input");
    (frame, input)
}

/// The two entities a field spawns are the two feathers text input
/// scenes, and neither is a hand-rolled interaction control.
#[test]
fn a_text_field_is_a_feathers_text_input() {
    let mut app = util::editor_test_app();

    let root = app
        .world_mut()
        .spawn(text_edit(TextEditProps::default()))
        .id();
    app.update();
    app.update();

    let (frame, input) = field(&mut app, root);
    assert!(
        app.world().get::<EditableText>(input).is_some(),
        "the input carries the editing model",
    );
    assert!(
        app.world().get::<Interaction>(frame).is_none(),
        "the frame is not a hand-rolled control",
    );
    assert!(
        app.world().get::<Interaction>(input).is_none(),
        "and neither is the input",
    );
}

/// The field keeps the layout it was spawned with. The container scene
/// writes a form-row frame over the entity; the editor's field is
/// taller than that and says so.
#[test]
fn a_text_field_keeps_the_frame_layout_it_was_spawned_with() {
    let mut app = util::editor_test_app();

    let root = app
        .world_mut()
        .spawn(text_edit(TextEditProps::default()))
        .id();
    app.update();
    app.update();

    let (frame, _) = field(&mut app, root);
    assert_eq!(
        app.world()
            .get::<Node>(frame)
            .expect("the frame has a layout")
            .height,
        px(28.0),
        "the editor's own frame height survives the scene",
    );
}

/// Typing into the widget and taking the focus away commits the text.
#[test]
fn typing_into_a_text_field_commits_on_blur() {
    let mut app = util::editor_test_app();

    let root = app
        .world_mut()
        .spawn(text_edit(TextEditProps::default()))
        .id();
    app.update();
    app.update();

    let (_, input) = field(&mut app, root);
    app.world_mut()
        .resource_mut::<InputFocus>()
        .set(input, FocusCause::Pressed);
    app.update();

    let mut editable = app
        .world_mut()
        .get_mut::<EditableText>(input)
        .expect("the input is editable");
    editable.queue_edit(TextEdit::Insert("hello".into()));
    app.update();
    app.update();

    assert_eq!(
        app.world()
            .get::<TextEditValue>(root)
            .expect("the field publishes its value")
            .0,
        "hello",
        "what was typed reaches the field's value",
    );

    let committed = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen = committed.clone();
    app.world_mut()
        .add_observer(move |commit: On<TextEditCommitEvent>| {
            seen.lock()
                .expect("no other thread holds it")
                .push(commit.text.clone());
        });

    app.world_mut().resource_mut::<InputFocus>().clear();
    app.update();
    app.update();

    assert_eq!(
        committed
            .lock()
            .expect("no other thread holds it")
            .as_slice(),
        ["hello".to_string()],
        "blurring the widget commits what was typed, once",
    );
}
