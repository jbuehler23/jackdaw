use bevy::prelude::*;
use bevy::ui_widgets::Activate;
use jackdaw_bind::{BindContext, BindFailures, BindPath, Binding, Bindings, JackdawBindPlugin};

#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct AbilitySlot {
    index: u8,
}

#[derive(EntityEvent, Reflect, Clone)]
#[reflect(Event, Default)]
struct CastAbility {
    entity: Entity,
    slot: u8,
}

impl Default for CastAbility {
    fn default() -> Self {
        Self {
            entity: Entity::PLACEHOLDER,
            slot: 0,
        }
    }
}

/// No `#[reflect(Default)]`: reflection cannot fill the fields a binding
/// leaves unmapped.
#[derive(EntityEvent, Reflect, Clone)]
#[reflect(Event)]
struct Undefaultable {
    entity: Entity,
    slot: u8,
    extra: u8,
}

/// A number too big for the `u8` field an action binding maps it onto.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Charge {
    amount: f32,
}

#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Caption {
    name: String,
    ready: bool,
}

/// Non-numeric fields that a binding is allowed to fill.
#[derive(EntityEvent, Reflect, Clone)]
#[reflect(Event, Default)]
struct Describe {
    entity: Entity,
    name: String,
    ready: bool,
}

impl Default for Describe {
    fn default() -> Self {
        Self {
            entity: Entity::PLACEHOLDER,
            name: String::new(),
            ready: false,
        }
    }
}

/// Every field mappable, but no `#[reflect(Default)]` to fall back on when a
/// mapped value is the wrong type for the field it lands in.
#[derive(EntityEvent, Reflect, Clone)]
#[reflect(Event)]
struct Mistyped {
    entity: Entity,
    slot: u8,
}

/// A field whose declared type is not one of the primitives a numeric bind
/// value can be narrowed into.
#[derive(Reflect, Default, Clone, Copy, PartialEq, Debug)]
struct SlotId(u8);

#[derive(EntityEvent, Reflect, Clone)]
#[reflect(Event, Default)]
struct UnlistedField {
    entity: Entity,
    id: SlotId,
}

impl Default for UnlistedField {
    fn default() -> Self {
        Self {
            entity: Entity::PLACEHOLDER,
            id: SlotId(0),
        }
    }
}

/// An event whose target field is named by `#[event_target]` rather than by the
/// literal name `entity`, which leaves no trace in the type registry.
#[derive(EntityEvent, Reflect, Clone)]
#[reflect(Event, Default)]
struct Aimed {
    #[event_target]
    at: Entity,
    slot: u8,
}

impl Default for Aimed {
    fn default() -> Self {
        Self {
            at: Entity::PLACEHOLDER,
            slot: 0,
        }
    }
}

/// An event whose fields have no names for a binding to fill.
#[derive(Event, Reflect, Clone, Default)]
#[reflect(Event, Default)]
struct Fired(u8);

/// The same, with no fields at all.
#[derive(Event, Reflect, Clone, Default)]
#[reflect(Event, Default)]
enum Mode {
    #[default]
    Idle,
    Running,
}

#[derive(Resource, Default)]
struct Casts(Vec<(Entity, u8)>);

#[derive(Resource, Default)]
struct Described(Vec<(String, bool)>);

/// Anything dispatched by an event that should never have been built.
#[derive(Resource, Default)]
struct Stray(u32);

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(JackdawBindPlugin);
    app.register_type::<AbilitySlot>();
    app.register_type::<Caption>();
    app.register_type::<Charge>();
    app.register_type::<CastAbility>();
    app.register_type::<Undefaultable>();
    app.register_type::<Mistyped>();
    app.register_type::<UnlistedField>();
    app.register_type::<Describe>();
    app.register_type::<Aimed>();
    app.register_type::<Fired>();
    app.register_type::<Mode>();
    app.init_resource::<Casts>();
    app.init_resource::<Stray>();
    app.init_resource::<Described>();
    app.add_observer(|_: On<Aimed>, mut stray: ResMut<Stray>| stray.0 += 1);
    app.add_observer(|d: On<Describe>, mut seen: ResMut<Described>| {
        seen.0.push((d.event().name.clone(), d.event().ready));
    });
    app.add_observer(|cast: On<CastAbility>, mut casts: ResMut<Casts>| {
        casts.0.push((cast.event().entity, cast.event().slot));
    });
    app.add_observer(|_: On<Mistyped>, mut stray: ResMut<Stray>| stray.0 += 1);
    app.add_observer(|_: On<Fired>, mut stray: ResMut<Stray>| stray.0 += 1);
    app.add_observer(|_: On<Mode>, mut stray: ResMut<Stray>| stray.0 += 1);
    app.add_observer(|_: On<UnlistedField>, mut stray: ResMut<Stray>| stray.0 += 1);
    app
}

fn button_with(app: &mut App, subject: Entity, event: &str, field: &str, path: &str) -> Entity {
    app.world_mut()
        .spawn((
            Node::default(),
            BindContext(subject),
            Bindings(vec![Binding::Action {
                event: event.into(),
                fields: vec![(field.into(), BindPath::new(path))],
            }]),
        ))
        .id()
}

#[test]
fn activate_triggers_mapped_game_event() {
    let mut app = app();
    let subject = app.world_mut().spawn(AbilitySlot { index: 3 }).id();
    let button = app
        .world_mut()
        .spawn((
            Node::default(),
            BindContext(subject),
            Bindings(vec![Binding::Action {
                event: "CastAbility".into(),
                fields: vec![("slot".into(), BindPath::new("AbilitySlot.index"))],
            }]),
        ))
        .id();
    app.update();
    app.world_mut().trigger(Activate { entity: button });
    app.update();
    assert_eq!(app.world().resource::<Casts>().0, vec![(subject, 3)]);
}

#[test]
fn activate_without_action_bindings_dispatches_nothing() {
    let mut app = app();
    let subject = app.world_mut().spawn(AbilitySlot { index: 3 }).id();
    let button = app
        .world_mut()
        .spawn((Node::default(), BindContext(subject)))
        .id();
    app.update();
    app.world_mut().trigger(Activate { entity: button });
    app.update();
    assert!(app.world().resource::<Casts>().0.is_empty());
}

#[test]
fn unfillable_event_warns_instead_of_panicking() {
    let mut app = app();
    let subject = app.world_mut().spawn(AbilitySlot { index: 3 }).id();
    let button = app
        .world_mut()
        .spawn((
            Node::default(),
            BindContext(subject),
            Bindings(vec![Binding::Action {
                event: "Undefaultable".into(),
                fields: vec![("slot".into(), BindPath::new("AbilitySlot.index"))],
            }]),
        ))
        .id();
    app.update();
    app.world_mut().trigger(Activate { entity: button });
    app.update();
}

#[test]
fn string_and_bool_fields_dispatch_when_the_types_line_up() {
    let mut app = app();
    let subject = app
        .world_mut()
        .spawn(Caption {
            name: "hilt".into(),
            ready: true,
        })
        .id();
    let button = app
        .world_mut()
        .spawn((
            Node::default(),
            BindContext(subject),
            Bindings(vec![Binding::Action {
                event: "Describe".into(),
                fields: vec![
                    ("name".into(), BindPath::new("Caption.name")),
                    ("ready".into(), BindPath::new("Caption.ready")),
                ],
            }]),
        ))
        .id();
    app.update();
    app.world_mut().trigger(Activate { entity: button });
    app.update();
    assert_eq!(
        app.world().resource::<Described>().0,
        vec![("hilt".to_string(), true)]
    );
}

#[test]
fn string_mapped_onto_a_numeric_field_warns_instead_of_panicking() {
    let mut app = app();
    let subject = app
        .world_mut()
        .spawn(Caption {
            name: "three".into(),
            ready: false,
        })
        .id();
    let button = button_with(&mut app, subject, "Mistyped", "slot", "Caption.name");
    app.update();
    app.world_mut().trigger(Activate { entity: button });
    app.update();
    assert_eq!(app.world().resource::<Stray>().0, 0);
}

#[test]
fn field_of_an_unlisted_type_is_an_error_not_a_silent_default() {
    let mut app = app();
    let subject = app.world_mut().spawn(AbilitySlot { index: 3 }).id();
    let button = button_with(
        &mut app,
        subject,
        "UnlistedField",
        "id",
        "AbilitySlot.index",
    );
    app.update();
    app.world_mut().trigger(Activate { entity: button });
    app.update();
    assert_eq!(app.world().resource::<Stray>().0, 0);
}

#[test]
fn field_not_declared_on_the_event_is_an_error() {
    let mut app = app();
    let subject = app.world_mut().spawn(AbilitySlot { index: 3 }).id();
    let button = button_with(
        &mut app,
        subject,
        "CastAbility",
        "nope",
        "AbilitySlot.index",
    );
    app.update();
    app.world_mut().trigger(Activate { entity: button });
    app.update();
    assert!(app.world().resource::<Casts>().0.is_empty());
}

/// A widget with nothing above it naming a subject has no entity to send, and
/// the `Default` placeholder is not one an observer could look up.
#[test]
fn an_action_with_no_resolved_context_sends_nothing() {
    let mut app = app();
    let button = app
        .world_mut()
        .spawn((
            Node::default(),
            Bindings(vec![Binding::Action {
                event: "CastAbility".into(),
                fields: vec![],
            }]),
        ))
        .id();
    app.update();
    app.world_mut().trigger(Activate { entity: button });
    app.update();
    assert!(
        app.world().resource::<Casts>().0.is_empty(),
        "an unresolved context must not reach the observer as Entity::PLACEHOLDER"
    );
}

/// The same widget with a context resolves and sends.
#[test]
fn an_action_with_a_resolved_context_still_sends() {
    let mut app = app();
    let subject = app.world_mut().spawn(AbilitySlot { index: 3 }).id();
    let button = app
        .world_mut()
        .spawn((
            Node::default(),
            BindContext(subject),
            Bindings(vec![Binding::Action {
                event: "CastAbility".into(),
                fields: vec![],
            }]),
        ))
        .id();
    app.update();
    app.world_mut().trigger(Activate { entity: button });
    app.update();
    assert_eq!(app.world().resource::<Casts>().0, vec![(subject, 0)]);
}

/// An entity target under a name other than `entity` is invisible to
/// reflection, and the event must not go out with the placeholder in it.
#[test]
fn an_entity_target_the_registry_cannot_see_is_refused_not_defaulted() {
    let mut app = app();
    let subject = app.world_mut().spawn(AbilitySlot { index: 3 }).id();
    let button = button_with(&mut app, subject, "Aimed", "slot", "AbilitySlot.index");
    app.update();
    app.world_mut().trigger(Activate { entity: button });
    app.update();
    assert_eq!(
        app.world().resource::<Stray>().0,
        0,
        "an entity field the binding cannot fill is an error, not Entity::PLACEHOLDER"
    );
}

#[test]
fn unknown_event_type_warns_instead_of_panicking() {
    let mut app = app();
    let subject = app.world_mut().spawn(AbilitySlot { index: 3 }).id();
    let button = app
        .world_mut()
        .spawn((
            Node::default(),
            BindContext(subject),
            Bindings(vec![Binding::Action {
                event: "NotAnEvent".into(),
                fields: vec![],
            }]),
        ))
        .id();
    app.update();
    app.world_mut().trigger(Activate { entity: button });
    app.update();
    assert!(app.world().resource::<Casts>().0.is_empty());
}

/// A tuple struct has no field names, so every guard would pass on an empty
/// list and reflection would panic building the event.
#[test]
fn a_tuple_struct_event_is_refused_instead_of_panicking() {
    let mut app = app();
    let subject = app.world_mut().spawn(AbilitySlot { index: 3 }).id();
    let button = app
        .world_mut()
        .spawn((
            Node::default(),
            BindContext(subject),
            Bindings(vec![Binding::Action {
                event: "Fired".into(),
                fields: vec![],
            }]),
        ))
        .id();
    app.update();
    app.world_mut().trigger(Activate { entity: button });
    app.update();
    assert_eq!(app.world().resource::<Stray>().0, 0);
}

#[test]
fn an_enum_event_is_refused_instead_of_panicking() {
    let mut app = app();
    let subject = app.world_mut().spawn(AbilitySlot { index: 3 }).id();
    let button = app
        .world_mut()
        .spawn((
            Node::default(),
            BindContext(subject),
            Bindings(vec![Binding::Action {
                event: "Mode".into(),
                fields: vec![],
            }]),
        ))
        .id();
    app.update();
    app.world_mut().trigger(Activate { entity: button });
    app.update();
    assert_eq!(app.world().resource::<Stray>().0, 0);
}

/// The binding is wrong the moment it is resolved, and is reported there rather
/// than waiting for a click.
#[test]
fn an_event_with_no_named_fields_is_reported_before_the_first_click() {
    let mut app = app();
    let subject = app.world_mut().spawn(AbilitySlot { index: 3 }).id();
    let button = button_with(&mut app, subject, "Fired", "0", "AbilitySlot.index");
    app.update();
    assert!(
        app.world()
            .resource::<BindFailures>()
            .0
            .contains(&(button, 0)),
        "a binding nothing can send has to say so when it is resolved",
    );
}

/// `as` narrows 300 into a `u8` as 44 and a NaN as 0, neither of which an
/// observer could tell from a number the game meant.
#[test]
fn a_number_too_big_for_the_field_is_refused_not_wrapped() {
    let mut app = app();
    let subject = app.world_mut().spawn(Charge { amount: 300.0 }).id();
    let button = button_with(&mut app, subject, "CastAbility", "slot", "Charge.amount");
    app.update();
    app.world_mut().trigger(Activate { entity: button });
    app.update();
    assert!(
        app.world().resource::<Casts>().0.is_empty(),
        "300 reached the event as some other number",
    );
}

#[test]
fn a_nan_is_refused_rather_than_sent_as_zero() {
    let mut app = app();
    let subject = app.world_mut().spawn(Charge { amount: f32::NAN }).id();
    let button = button_with(&mut app, subject, "CastAbility", "slot", "Charge.amount");
    app.update();
    app.world_mut().trigger(Activate { entity: button });
    app.update();
    assert!(app.world().resource::<Casts>().0.is_empty());
}
