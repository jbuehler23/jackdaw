//! A `Field` binding whose write path names a whole component rather than a
//! field inside one: the bool it reads puts the component on and takes it off.
//!
//! This is what "Create is disabled until the form is valid" is made of.
//! `InteractionDisabled` holds nothing, so there is no field to drive and the
//! only question a binding can answer about it is whether it is there.

use bevy::prelude::*;
use bevy::ui::InteractionDisabled;
use jackdaw_bind::{
    BindContext, BindError, BindFailures, BindPath, Binding, Bindings, JackdawBindPlugin,
    WriteValue, write_path,
};

/// The state a form's Create button watches.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Form {
    incomplete: bool,
    filled_fields: f32,
}

/// A marker reflection cannot build, which is what rules it out as a write
/// target: putting it on is the whole of what `true` means.
#[derive(Component, Reflect)]
#[reflect(Component)]
struct Undefaultable;

/// A marker declared immutable. Nothing inside it is touched either way, so
/// unlike a field write this is still a write a binding can make.
#[derive(Component, Reflect, Default)]
#[component(immutable)]
#[reflect(Component, Default)]
struct Frozen;

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        bevy::input::InputPlugin,
        bevy::asset::AssetPlugin::default(),
        bevy::text::TextPlugin,
        bevy::picking::PickingPlugin,
        bevy::picking::InteractionPlugin,
        bevy::ui::UiPlugin,
    ));
    app.init_asset::<Image>();
    app.init_asset::<TextureAtlasLayout>();
    app.add_plugins(JackdawBindPlugin);
    app.register_type::<Form>();
    app.register_type::<Undefaultable>();
    app.register_type::<Frozen>();
    app.register_type::<InteractionDisabled>();
    app
}

/// A button bound to the form's state through the marker.
fn disabled_when(app: &mut App, marker: &str) -> (Entity, Entity) {
    let subject = app.world_mut().spawn(Form::default()).id();
    let button = app
        .world_mut()
        .spawn((
            Node::default(),
            BindContext(subject),
            Bindings(vec![Binding::Field {
                read: vec![BindPath::new("Form.incomplete")],
                via: None,
                write: BindPath::new(marker),
                as_percent: false,
            }]),
        ))
        .id();
    (subject, button)
}

#[test]
fn a_true_read_puts_the_marker_on_and_a_false_read_takes_it_off() {
    let mut app = app();
    let (subject, button) =
        disabled_when(&mut app, "bevy_ui::interaction_states::InteractionDisabled");

    app.world_mut().get_mut::<Form>(subject).unwrap().incomplete = true;
    app.update();
    assert!(
        app.world().get::<InteractionDisabled>(button).is_some(),
        "an incomplete form disables the button",
    );

    app.world_mut().get_mut::<Form>(subject).unwrap().incomplete = false;
    app.update();
    assert!(
        app.world().get::<InteractionDisabled>(button).is_none(),
        "and completing it enables the button again",
    );
}

/// Presence is the equality guard: a binding that re-evaluates to the answer
/// already on the entity has to leave it alone, or every observer watching for
/// the marker fires on every frame the source is touched.
#[test]
fn re_evaluating_the_same_answer_does_not_put_the_marker_on_again() {
    #[derive(Resource, Default)]
    struct Insertions(usize);

    fn count(added: Query<(), Added<InteractionDisabled>>, mut count: ResMut<Insertions>) {
        count.0 += added.iter().count();
    }

    let mut app = app();
    app.init_resource::<Insertions>();
    app.add_systems(Last, count);
    let (subject, button) =
        disabled_when(&mut app, "bevy_ui::interaction_states::InteractionDisabled");

    app.world_mut().get_mut::<Form>(subject).unwrap().incomplete = true;
    app.update();
    assert_eq!(app.world().resource::<Insertions>().0, 1);
    assert!(app.world().get::<InteractionDisabled>(button).is_some());

    // Touching the source without moving the value is what gets the binding
    // past the change-tick gate and onto the guard being tested.
    for _ in 0..3 {
        let _ = app.world_mut().get_mut::<Form>(subject).unwrap();
        app.update();
    }
    assert_eq!(
        app.world().resource::<Insertions>().0,
        1,
        "the marker is put on once and left there",
    );
}

/// A marker is a whole component, not a field, so mutability never comes into
/// it, which is the one way a marker write can do something a field write
/// cannot.
#[test]
fn an_immutable_marker_is_still_set_and_cleared() {
    let mut app = app();
    let (subject, button) = disabled_when(&mut app, "Frozen");

    app.world_mut().get_mut::<Form>(subject).unwrap().incomplete = true;
    app.update();
    assert!(app.world().get::<Frozen>(button).is_some());

    app.world_mut().get_mut::<Form>(subject).unwrap().incomplete = false;
    app.update();
    assert!(app.world().get::<Frozen>(button).is_none());
    assert!(app.world().resource::<BindFailures>().0.is_empty());
}

#[test]
fn a_marker_written_from_something_other_than_a_bool_is_a_typed_error() {
    let mut app = app();
    let button = app.world_mut().spawn(Node::default()).id();
    let err = write_path(
        app.world_mut(),
        button,
        &BindPath::new("bevy_ui::interaction_states::InteractionDisabled"),
        &WriteValue::F32(1.0),
    )
    .unwrap_err();
    assert!(
        matches!(err, BindError::MarkerNeedsBool { .. }),
        "wrong branch: {err}"
    );
    assert_eq!(
        err.to_string(),
        "marker 'bevy_ui::interaction_states::InteractionDisabled' is set by a bool: true puts it \
         on, false takes it off",
    );
    assert!(app.world().get::<InteractionDisabled>(button).is_none());
}

#[test]
fn a_marker_reflection_cannot_build_is_a_typed_error() {
    let mut app = app();
    let button = app.world_mut().spawn(Node::default()).id();
    let err = write_path(
        app.world_mut(),
        button,
        &BindPath::new("Undefaultable"),
        &WriteValue::Bool(true),
    )
    .unwrap_err();
    assert!(
        matches!(err, BindError::MarkerNotDefaultable { .. }),
        "wrong branch: {err}"
    );
    assert_eq!(
        err.to_string(),
        "marker 'Undefaultable' has no #[reflect(Default)], so nothing can put it on",
    );
}

/// The non-bool case through the evaluator rather than the write helper: it
/// has to reach the warn-once ledger and leave every other binding alone.
#[test]
fn a_non_bool_source_warns_once_and_does_not_panic() {
    let mut app = app();
    let subject = app.world_mut().spawn(Form::default()).id();
    let button = app
        .world_mut()
        .spawn((
            Node::default(),
            BindContext(subject),
            Bindings(vec![Binding::Field {
                read: vec![BindPath::new("Form.filled_fields")],
                via: None,
                write: BindPath::new("bevy_ui::interaction_states::InteractionDisabled"),
                as_percent: false,
            }]),
        ))
        .id();
    app.update();
    app.update();
    assert_eq!(app.world().resource::<BindFailures>().0.len(), 1);
    assert!(app.world().get::<InteractionDisabled>(button).is_none());
}

/// A short name is what the picker offers when only one type answers to it,
/// and the marker arm resolves it the same way every other path does.
#[test]
fn a_marker_named_by_its_short_path_resolves() {
    let mut app = app();
    let (subject, button) = disabled_when(&mut app, "InteractionDisabled");

    app.world_mut().get_mut::<Form>(subject).unwrap().incomplete = true;
    app.update();
    assert!(app.world().get::<InteractionDisabled>(button).is_some());
    assert!(app.world().resource::<BindFailures>().0.is_empty());
}

/// A path with no field is a marker write only when the type has no fields
/// either. `Node` without `.width` is a path missing its field half, and
/// taking it as one would strip the layout off a live widget the first time
/// the binding read `false`.
#[test]
fn a_component_with_fields_is_not_a_marker_however_the_path_is_spelled() {
    let mut app = app();
    let widget = app.world_mut().spawn(Node::default()).id();

    let err = write_path(
        app.world_mut(),
        widget,
        &BindPath::new("bevy_ui::ui_node::Node"),
        &WriteValue::Bool(false),
    )
    .unwrap_err();
    assert!(
        matches!(err, BindError::MalformedPath { .. }),
        "wrong branch: {err}"
    );
    assert_eq!(
        err.to_string(),
        "expected 'Type.field' in 'bevy_ui::ui_node::Node'",
    );
    assert!(
        app.world().get::<Node>(widget).is_some(),
        "the widget keeps the component the path half-named",
    );
}

/// The write follows the source's change ticks like any other `Field` binding:
/// the marker mirrors the source, and nothing else is meant to be toggling it.
/// Something that does is not put back until the source moves again.
#[test]
fn a_marker_removed_by_hand_stays_off_until_the_source_moves() {
    let mut app = app();
    let (subject, button) = disabled_when(&mut app, "InteractionDisabled");

    app.world_mut().get_mut::<Form>(subject).unwrap().incomplete = true;
    app.update();
    assert!(app.world().get::<InteractionDisabled>(button).is_some());

    app.world_mut()
        .entity_mut(button)
        .remove::<InteractionDisabled>();
    app.update();
    app.update();
    assert!(
        app.world().get::<InteractionDisabled>(button).is_none(),
        "the gate reads the source, and the source has not moved",
    );

    app.world_mut().get_mut::<Form>(subject).unwrap().incomplete = false;
    app.world_mut().get_mut::<Form>(subject).unwrap().incomplete = true;
    app.update();
    assert!(
        app.world().get::<InteractionDisabled>(button).is_some(),
        "and the next move of the source puts it back",
    );
}
