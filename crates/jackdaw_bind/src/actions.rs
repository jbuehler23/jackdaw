use bevy::ecs::reflect::{ReflectEvent, ReflectFromWorld};
use bevy::prelude::*;
use bevy::reflect::prelude::ReflectDefault;
use bevy::reflect::structs::{DynamicStruct, Struct};
use bevy::reflect::{NamedField, TypePath, TypeRegistry};
use bevy::ui_widgets::{Activate, ValueChange};

use crate::evaluate::report;
use crate::resolve::{
    BindValue, WriteValue, fits, lookup_registration, read_path, resolve_context, write_source_path,
};
use crate::{BindError, BindFailures, BindPath, Binding, Bindings};

type ActionFields = Vec<(String, BindPath)>;

/// The only field name an `EntityEvent`'s target can be recognised by.
///
/// bevy's derive also accepts a field carrying `#[event_target]` under any
/// name, but that is a derive helper attribute: it produces the trait impl and
/// leaves nothing in the type registry, and there is no `ReflectEntityEvent`
/// to ask at runtime (bevy 0.19). An action binding therefore only fills the
/// field literally named `entity`; see the crate doc.
const ENTITY_TARGET_FIELD: &str = "entity";

pub fn on_activate(activate: On<Activate>, bindings: Query<&Bindings>, mut commands: Commands) {
    let target = activate.event().entity;
    let Ok(found) = bindings.get(target) else {
        return;
    };
    // The index travels with each action so a failure reaches the same
    // warn-once ledger the evaluator reports through: a button whose binding
    // is wrong is wrong on every click, and one line per binding is what makes
    // the log readable.
    let actions: Vec<(usize, String, ActionFields)> = found
        .0
        .iter()
        .enumerate()
        .filter_map(|(index, b)| match b {
            Binding::Action { event, fields } => Some((index, event.clone(), fields.clone())),
            _ => None,
        })
        .collect();
    if actions.is_empty() {
        return;
    }
    commands.queue(move |world: &mut World| {
        for (index, event_path, fields) in actions {
            if let Err(err) = dispatch(world, target, &event_path, &fields) {
                match world.get_resource_mut::<BindFailures>() {
                    Some(mut failures) => report(&mut failures, target, index, err),
                    None => warn!("binding {index} on {target}: {err}"),
                }
            }
        }
    });
}

/// Picks the first two-way `Value` binding on the widget that raised the change
/// and pairs its path with the new value. One-way bindings are left alone: they
/// only ever flow data to the widget.
fn value_write<T>(
    change: &ValueChange<T>,
    bindings: &Query<&Bindings>,
    to_write: impl Fn(&T) -> WriteValue,
) -> Option<(Entity, BindPath, WriteValue)> {
    let found = bindings.get(change.source).ok()?;
    found.0.iter().find_map(|b| match b {
        Binding::Value { with, two_way } if *two_way => {
            Some((change.source, with.clone(), to_write(&change.value)))
        }
        _ => None,
    })
}

/// Observers carry no binding index, so failures warn every time rather than
/// through the warn-once ledger the evaluator uses.
fn queue_value_write(commands: &mut Commands, widget: Entity, path: BindPath, value: WriteValue) {
    commands.queue(move |world: &mut World| {
        let context = resolve_context(world, widget);
        if let Err(err) = write_source_path(world, context, &path, &value) {
            warn!("value binding on {widget}: {err}");
        }
    });
}

pub fn on_value_change_f32(
    change: On<ValueChange<f32>>,
    bindings: Query<&Bindings>,
    mut commands: Commands,
) {
    let Some((widget, path, value)) =
        value_write(change.event(), &bindings, |v| WriteValue::F32(*v))
    else {
        return;
    };
    queue_value_write(&mut commands, widget, path, value);
}

pub fn on_value_change_bool(
    change: On<ValueChange<bool>>,
    bindings: Query<&Bindings>,
    mut commands: Commands,
) {
    let Some((widget, path, value)) =
        value_write(change.event(), &bindings, |v| WriteValue::Bool(*v))
    else {
        return;
    };
    queue_value_write(&mut commands, widget, path, value);
}

pub fn on_value_change_string(
    change: On<ValueChange<String>>,
    bindings: Query<&Bindings>,
    mut commands: Commands,
) {
    let Some((widget, path, value)) =
        value_write(change.event(), &bindings, |v| WriteValue::Str(v.clone()))
    else {
        return;
    };
    queue_value_write(&mut commands, widget, path, value);
}

/// Inserts a bind value under the exact type the event declares for that field.
/// `read_path` widens every integer to f32, so a numeric value is narrowed back
/// to the declared width; every other pairing must match outright. A value that
/// does not fit the declared type is refused here rather than handed to
/// reflection, which would either panic building the event or quietly swap in
/// the type's `Default` for the field.
fn insert_coerced(
    dynamic: &mut DynamicStruct,
    field: &str,
    value: BindValue,
    type_path: Option<&str>,
) -> Result<(), BindError> {
    let Some(type_path) = type_path else {
        return Err(BindError::UnknownEventField {
            field: field.to_string(),
        });
    };
    match value {
        BindValue::F32(v) => {
            // Every integer a binding reads arrives widened to f32, so putting
            // one back is a narrowing. `as` would saturate a number too large
            // for the field and turn a NaN into zero, sending an event carrying
            // a number nobody computed. Refuse it instead.
            macro_rules! narrowed {
                ($ty:ty) => {{
                    if !fits(v, <$ty>::MIN as f64, <$ty>::MAX as f64) {
                        return Err(BindError::EventFieldOutOfRange {
                            field: field.to_string(),
                            type_path: type_path.to_string(),
                            value: v,
                        });
                    }
                    dynamic.insert(field, v as $ty)
                }};
            }
            match type_path {
                "u8" => narrowed!(u8),
                "u16" => narrowed!(u16),
                "u32" => narrowed!(u32),
                "u64" => narrowed!(u64),
                "usize" => narrowed!(usize),
                "i8" => narrowed!(i8),
                "i16" => narrowed!(i16),
                "i32" => narrowed!(i32),
                "i64" => narrowed!(i64),
                "isize" => narrowed!(isize),
                "f32" => dynamic.insert(field, v),
                "f64" => dynamic.insert(field, f64::from(v)),
                _ => return Err(mismatch(field, type_path, "numeric")),
            }
        }
        BindValue::Bool(v) if type_path == bool::type_path() => dynamic.insert(field, v),
        BindValue::Str(v) if type_path == String::type_path() => dynamic.insert(field, v),
        BindValue::Bool(_) => return Err(mismatch(field, type_path, "bool")),
        BindValue::Str(_) => return Err(mismatch(field, type_path, "string")),
    }
    Ok(())
}

fn mismatch(field: &str, type_path: &str, kind: &'static str) -> BindError {
    BindError::EventFieldTypeMismatch {
        field: field.to_string(),
        type_path: type_path.to_string(),
        kind,
    }
}

/// Field types are collected up front so the registry lock is released before
/// `read_path` runs, which takes the same lock again.
struct EventShape {
    reflect_event: ReflectEvent,
    /// A registry of one, holding the event's registration and nothing else.
    /// See [`dispatch`] for why the trigger is not handed the app's.
    registry: TypeRegistry,
    field_types: Vec<Option<String>>,
    declared_fields: Vec<String>,
    /// Every field the event declares as an `Entity`.
    ///
    /// The one named `entity` takes the widget's context; see
    /// [`ENTITY_TARGET_FIELD`] for why no other name can be recognised. The
    /// rest are here so an unfilled one can be refused rather than defaulted
    /// to `Entity::PLACEHOLDER`.
    entity_fields: Vec<String>,
    /// Whether reflection can build the event when the binding leaves a field
    /// unmapped. Without it bevy's fallback panics rather than erroring.
    fills_gaps: bool,
}

fn event_shape(
    world: &World,
    event_path: &str,
    fields: &[(String, BindPath)],
) -> Result<EventShape, BindError> {
    let registry_arc = world
        .get_resource::<AppTypeRegistry>()
        .ok_or(BindError::NoTypeRegistry)?
        .clone();
    let registry = registry_arc.read();
    let reg = lookup_registration(&registry, event_path, "event type")?;
    let reflect_event = reg
        .data::<ReflectEvent>()
        .ok_or_else(|| BindError::NotAnEvent {
            event_path: event_path.to_string(),
        })?
        .clone();
    // A tuple struct or an enum has no named fields, so every guard below
    // would pass on an empty list and reflection would be handed a
    // `DynamicStruct` it cannot build the event from, which panics rather than
    // erroring. Refuse the shape itself.
    let info = reg
        .type_info()
        .as_struct()
        .map_err(|_| BindError::EventNotNamedStruct {
            event_path: event_path.to_string(),
            kind: reg.type_info().kind().to_string(),
        })?;
    let field_types = fields
        .iter()
        .map(|(field, _)| info.field(field).map(|f| f.type_path().to_string()))
        .collect();
    let declared_fields = info.iter().map(|f| f.name().to_string()).collect();
    let entity_fields = info
        .iter()
        .filter(|f| NamedField::is::<Entity>(f))
        .map(|f| f.name().to_string())
        .collect();
    let fills_gaps =
        reg.data::<ReflectDefault>().is_some() || reg.data::<ReflectFromWorld>().is_some();
    let mut alone = TypeRegistry::empty();
    alone.add_registration(reg.clone());
    Ok(EventShape {
        reflect_event,
        registry: alone,
        field_types,
        declared_fields,
        entity_fields,
        fills_gaps,
    })
}

/// Everything about an action binding the type registry alone can answer: that
/// the event is registered, that it is an event, that its fields have names to
/// fill, and that the names the binding maps are among them.
///
/// Asked when the binding is resolved rather than only when the widget is
/// clicked, so a binding nothing can send is reported the way every other
/// broken binding is instead of waiting for the first click.
pub(crate) fn check_event(
    world: &World,
    event_path: &str,
    fields: &[(String, BindPath)],
) -> Result<(), BindError> {
    let shape = event_shape(world, event_path, fields)?;
    for ((field, _), declared) in fields.iter().zip(&shape.field_types) {
        if declared.is_none() {
            return Err(BindError::UnknownEventField {
                field: field.clone(),
            });
        }
    }
    Ok(())
}

/// Builds the event an action binding names and sends it.
///
/// `ReflectEvent::trigger` takes a type registry by reference and holds it for
/// the whole dispatch, observers included, so the one it is handed is not the
/// app's: it is the registry of one that `event_shape` built, holding the
/// event's registration. Reflection reads it only to build the event from the
/// fields the binding filled in, and an observer is then free to ask the app
/// for its registry, mutably to register a type, without deadlocking against a
/// lock this dispatch is still holding.
fn dispatch(
    world: &mut World,
    widget: Entity,
    event_path: &str,
    fields: &[(String, BindPath)],
) -> Result<(), BindError> {
    let shape = event_shape(world, event_path, fields)?;
    let context = resolve_context(world, widget);
    let mut dynamic = DynamicStruct::default();
    for ((field, path), field_type) in fields.iter().zip(&shape.field_types) {
        let value = read_path(world, context, path)?;
        insert_coerced(&mut dynamic, field, value, field_type.as_deref())?;
    }
    // The widget's context fills the event's entity target. A widget with no
    // context resolved has no entity to send, and the event's own `Default`
    // would put `Entity::PLACEHOLDER` there: an id that names nothing, which an
    // observer would still look up. Refuse instead.
    if shape.entity_fields.iter().any(|f| f == ENTITY_TARGET_FIELD)
        && dynamic.field(ENTITY_TARGET_FIELD).is_none()
    {
        let Some(subject) = context else {
            return Err(BindError::MissingContext {
                event_path: event_path.to_string(),
                field: ENTITY_TARGET_FIELD.to_string(),
            });
        };
        dynamic.insert(ENTITY_TARGET_FIELD, subject);
    }
    // Any other entity field is one nothing can fill: bind values carry
    // numbers, bools and strings, and only `entity` takes the context. Left
    // alone it would reach the observer as the `Default` placeholder.
    if let Some(field) = shape
        .entity_fields
        .iter()
        .find(|f| dynamic.field(f).is_none())
    {
        return Err(BindError::UnfillableEntityField {
            event_path: event_path.to_string(),
            field: field.clone(),
        });
    }
    if !shape.fills_gaps
        && let Some(missing) = shape
            .declared_fields
            .iter()
            .find(|f| dynamic.field(f).is_none())
    {
        return Err(BindError::UnfillableEvent {
            event_path: event_path.to_string(),
            field: missing.clone(),
        });
    }
    shape
        .reflect_event
        .trigger(world, &dynamic, &shape.registry);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Event, Reflect, Clone, Default)]
    #[reflect(Event, Default)]
    struct Fired(u8);

    #[derive(Event, Reflect, Clone, Default)]
    #[reflect(Event, Default)]
    enum Mode {
        #[default]
        Idle,
        Running,
    }

    fn world_with<T: bevy::reflect::GetTypeRegistration>() -> World {
        let mut world = World::new();
        let registry = AppTypeRegistry::default();
        registry.write().register::<T>();
        world.insert_resource(registry);
        world
    }

    #[test]
    fn a_tuple_struct_event_is_refused_when_the_binding_is_resolved() {
        let world = world_with::<Fired>();
        let err = check_event(
            &world,
            "Fired",
            &[("0".into(), BindPath::new("Slot.index"))],
        )
        .unwrap_err();
        assert!(
            matches!(err, BindError::EventNotNamedStruct { ref kind, .. } if kind == "tuple struct"),
            "wrong branch: {err}"
        );
    }

    #[test]
    fn an_enum_event_is_refused_when_the_binding_is_resolved() {
        let world = world_with::<Mode>();
        let err = check_event(&world, "Mode", &[]).unwrap_err();
        assert!(
            matches!(err, BindError::EventNotNamedStruct { ref kind, .. } if kind == "enum"),
            "wrong branch: {err}"
        );
    }
}
